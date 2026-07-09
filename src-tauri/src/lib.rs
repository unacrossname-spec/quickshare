use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use quickshare_core::transfer::batch::{self, BatchMeta, BatchSender};
use quickshare_core::transfer::chunk::ChunkReader;
use quickshare_core::transfer::receiver::FileReceiver;
use quickshare_core::transfer::sender::{recv_json, send_json, FileSender};
use quickshare_core::transport::tcp::{TcpListener, TcpStream};
use quickshare_core::types::{ControlMessage, FileMeta};

const CHUNK_SIZE: usize = 4 * 1024 * 1024;

// ── State ──

pub struct AppState {
    pub server_shutdown: Arc<AtomicBool>,
    pub save_dir: Arc<Mutex<PathBuf>>,
    pub transfers: Arc<Mutex<Vec<TransferState>>>,
    pub history: Arc<Mutex<Vec<HistoryRecord>>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferState {
    pub id: String,
    pub file_name: String,
    pub total: u64,
    pub sent: u64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendOptions {
    pub addr: String,
    pub path: String,
    pub compress: bool,
    pub bundle: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalInfo {
    pub name: String,
    pub ips: Vec<String>,
    pub save_dir: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryRecord {
    pub id: String,
    pub file_name: String,
    pub peer: String,
    pub direction: String,
    pub size: u64,
    pub status: String,
    pub timestamp: String,
}

// ── Commands ──

#[tauri::command]
async fn get_local_info(state: tauri::State<'_, AppState>) -> Result<LocalInfo, String> {
    let save_dir = state.save_dir.lock().await.clone();
    Ok(LocalInfo {
        name: get_hostname(),
        ips: get_local_ips(),
        save_dir: save_dir.to_string_lossy().to_string(),
        port: 8877,
    })
}

#[tauri::command]
async fn send_files(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    opts: SendOptions,
) -> Result<(), String> {
    let path = PathBuf::from(&opts.path);
    if path.is_dir() {
        send_directory(app, state, opts).await
    } else {
        send_single(app, state, opts).await
    }
}

async fn send_single(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    opts: SendOptions,
) -> Result<(), String> {
    let addr: SocketAddr = opts.addr.parse().map_err(|e| format!("invalid addr: {e}"))?;
    let file_path = PathBuf::from(&opts.path);
    let data = std::fs::read(&file_path).map_err(|e| format!("read: {e}"))?;
    let file_size = data.len();
    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let (send_data, compressed) = if opts.compress {
        let c = quickshare_core::compress::compress(&data);
        let shrunk = c.len() < data.len();
        (c, shrunk)
    } else {
        (data, false)
    };

    let meta = FileMeta {
        name: file_name.clone(),
        size: file_size as u64,
        chunk_size: CHUNK_SIZE,
        chunk_count: (send_data.len() + CHUNK_SIZE - 1) as u64 / CHUNK_SIZE as u64,
        file_hash: [0u8; 32],
        compressed,
        bundle: false,
    };

    let tid = uuid::Uuid::new_v4().to_string();
    register_transfer(&state, &tid, &file_name, file_size as u64).await;
    drop(state);

    tauri::async_runtime::spawn(async move {
        let r = do_send_file(addr, meta, &send_data, &app, &tid).await;
        finish_transfer(&app, &tid, r).await;
    });

    Ok(())
}

async fn send_directory(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    opts: SendOptions,
) -> Result<(), String> {
    let addr: SocketAddr = opts.addr.parse().map_err(|e| format!("invalid addr: {e}"))?;
    let dir_path = PathBuf::from(&opts.path);
    let bundle = opts.bundle;
    let compress = opts.compress;

    let files = batch::collect_files(&dir_path).map_err(|e| format!("collect: {e}"))?;
    if files.is_empty() {
        return Err("no files found".into());
    }

    let total_size: u64 = files.iter().map(|(_, s)| s).sum();
    let root_name = dir_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    let file_count = files.len();

    let tid = uuid::Uuid::new_v4().to_string();
    register_transfer(&state, &tid, &format!("{} ({} files)", root_name, file_count), total_size).await;
    drop(state);

    if bundle {
        tauri::async_runtime::spawn(async move {
            let r = do_send_bundle(addr, dir_path, files, &root_name, &app, &tid).await;
            finish_transfer(&app, &tid, r).await;
        });
    } else {
        tauri::async_runtime::spawn(async move {
            let r = do_send_batch(addr, dir_path, files, compress, &app, &tid).await;
            finish_transfer(&app, &tid, r).await;
        });
    }

    Ok(())
}

#[tauri::command]
async fn get_transfers(state: tauri::State<'_, AppState>) -> Result<Vec<TransferState>, String> {
    Ok(state.transfers.lock().await.clone())
}

#[tauri::command]
async fn cancel_transfer(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    if let Some(t) = state.transfers.lock().await.iter_mut().find(|t| t.id == id) {
        t.status = "cancelled".into();
    }
    Ok(())
}

#[tauri::command]
async fn get_history(state: tauri::State<'_, AppState>) -> Result<Vec<HistoryRecord>, String> {
    Ok(state.history.lock().await.clone())
}

#[tauri::command]
async fn update_settings(state: tauri::State<'_, AppState>, save_dir: String) -> Result<(), String> {
    let p = PathBuf::from(&save_dir);
    if !p.exists() {
        std::fs::create_dir_all(&p).map_err(|e| format!("mkdir: {e}"))?;
    }
    *state.save_dir.lock().await = p;
    Ok(())
}

// ── Background Server ──

pub async fn run_server(app: AppHandle) {
    let addr = SocketAddr::from(([0, 0, 0, 0], 8877u16));
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[server] bind: {e}");
            return;
        }
    };
    let _ = app.emit("server-ready", true);

    loop {
        if app.state::<AppState>().server_shutdown.load(Ordering::SeqCst) {
            break;
        }
        let stream = match tokio::time::timeout(
            std::time::Duration::from_secs(1),
            listener.accept(),
        )
        .await
        {
            Ok(Ok(s)) => s,
            _ => continue,
        };
        let peer = stream.peer_addr().unwrap_or(SocketAddr::from(([0, 0, 0, 0], 0)));
        let app_c = app.clone();
        let save_dir = app.state::<AppState>().save_dir.lock().await.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_incoming(stream, save_dir, &app_c, peer).await {
                eprintln!("[server] {e}");
            }
        });
    }
}

async fn handle_incoming(
    mut stream: TcpStream,
    save_dir: PathBuf,
    app: &AppHandle,
    peer: SocketAddr,
) -> Result<(), anyhow::Error> {
    let first: serde_json::Value = recv_json(&mut stream).await?;
    let req: ControlMessage = serde_json::from_value(first)?;
    let meta = match &req {
        ControlMessage::TransferRequest { file_meta, .. } => file_meta.clone(),
        _ => anyhow::bail!("expected TransferRequest"),
    };

    let accept = ControlMessage::TransferAccept {
        transfer_id: quickshare_core::types::TransferId::new_v4(),
        received_chunks: vec![],
    };
    send_json(&mut stream, &accept).await?;

    let mut receiver = FileReceiver::from_handshake(stream, meta.clone());
    let tmp = save_dir.join(&meta.name).with_extension("tmp");
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut recvd = 0u64;

    loop {
        match receiver.recv_chunk().await? {
            Some((_, data)) => {
                file.write_all(&data).await?;
                recvd += data.len() as u64;
            }
            None => break,
        }
    }
    drop(file);

    if meta.bundle {
        let data = if meta.compressed {
            let raw = std::fs::read(&tmp)?;
            tokio::fs::remove_file(&tmp).await?;
            quickshare_core::compress::decompress(&raw)?
        } else {
            let raw = std::fs::read(&tmp)?;
            tokio::fs::remove_file(&tmp).await?;
            raw
        };
        let root = save_dir.join(&meta.name);
        tokio::fs::create_dir_all(&root).await?;
        let files = quickshare_core::bundle::extract_bundle(&data)?;
        let file_count = files.len();
        let mut total = 0u64;
        for (rel, fdata) in &files {
            let full = root.join(rel);
            if let Some(p) = full.parent() {
                tokio::fs::create_dir_all(p).await?;
            }
            tokio::fs::write(&full, fdata).await?;
            total += fdata.len() as u64;
        }
        let _ = app.emit(
            "receive-complete",
            serde_json::json!({
                "peer": peer.to_string(),
                "name": meta.name,
                "count": file_count,
                "total_bytes": total,
            }),
        );
    } else if meta.compressed {
        let raw = std::fs::read(&tmp)?;
        tokio::fs::remove_file(&tmp).await?;
        let data = quickshare_core::compress::decompress(&raw)?;
        let out = save_dir.join(&meta.name);
        tokio::fs::write(&out, &data).await?;
        let _ = app.emit(
            "receive-complete",
            serde_json::json!({
                "peer": peer.to_string(),
                "file": meta.name,
                "size": recvd,
            }),
        );
    } else {
        let out = save_dir.join(&meta.name);
        tokio::fs::rename(&tmp, &out).await?;
        let _ = app.emit(
            "receive-complete",
            serde_json::json!({
                "peer": peer.to_string(),
                "file": meta.name,
                "size": recvd,
            }),
        );
    }

    Ok(())
}

// ── Internal ──

async fn do_send_file(
    addr: SocketAddr,
    meta: FileMeta,
    data: &[u8],
    app: &AppHandle,
    tid: &str,
) -> Result<(), String> {
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let mut sender = FileSender::new(stream, meta);
    sender
        .handshake()
        .await
        .map_err(|e| format!("handshake: {e}"))?;

    let total = data.len() as u64;
    let reader = ChunkReader::new(data, CHUNK_SIZE);
    let mut sent = 0u64;
    for chunk in reader {
        let c = chunk.map_err(|e| format!("chunk: {e}"))?;
        sender
            .send_chunk(&c)
            .await
            .map_err(|e| format!("send: {e}"))?;
        sent += c.data.len() as u64;
        let _ = app.emit(
            "transfer-progress",
            serde_json::json!({ "id": tid, "sent": sent, "total": total }),
        );
    }
    sender.finish().await.map_err(|e| format!("finish: {e}"))?;
    Ok(())
}

async fn do_send_bundle(
    addr: SocketAddr,
    dir_path: PathBuf,
    files: Vec<(PathBuf, u64)>,
    root_name: &str,
    app: &AppHandle,
    tid: &str,
) -> Result<(), String> {
    let mut entries = Vec::with_capacity(files.len());
    for (rel, _) in files {
        let full = dir_path.join(&rel);
        let data = std::fs::read(&full).map_err(|e| format!("read: {e}"))?;
        entries.push((rel.to_string_lossy().to_string(), data));
    }

    let bundle = quickshare_core::bundle::create_bundle(&entries);
    let compressed = quickshare_core::compress::compress(&bundle);

    let file_meta = FileMeta {
        name: root_name.to_string(),
        size: bundle.len() as u64,
        chunk_size: CHUNK_SIZE,
        chunk_count: (compressed.len() + CHUNK_SIZE - 1) as u64 / CHUNK_SIZE as u64,
        file_hash: [0u8; 32],
        compressed: compressed.len() < bundle.len(),
        bundle: true,
    };

    do_send_file(addr, file_meta, &compressed, app, tid).await
}

async fn do_send_batch(
    addr: SocketAddr,
    dir_path: PathBuf,
    files: Vec<(PathBuf, u64)>,
    compress: bool,
    app: &AppHandle,
    tid: &str,
) -> Result<(), String> {
    let total: u64 = files.iter().map(|(_, s)| s).sum();
    let root = dir_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let meta = BatchMeta {
        total_files: files.len() as u32,
        total_size: total,
        root_name: root.to_string(),
    };

    let stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let mut sender = BatchSender::new(stream, meta, CHUNK_SIZE).with_compression(compress);
    sender
        .handshake()
        .await
        .map_err(|e| format!("handshake: {e}"))?;

    let mut sent = 0u64;
    for (rel, _) in files {
        let full = dir_path.join(&rel);
        let data = std::fs::read(&full).map_err(|e| format!("read: {e}"))?;
        sender
            .send_file(rel.to_string_lossy().as_ref(), &data)
            .await
            .map_err(|e| format!("send: {e}"))?;
        sent += data.len() as u64;
        let _ = app.emit(
            "transfer-progress",
            serde_json::json!({ "id": tid, "sent": sent, "total": total }),
        );
    }
    sender
        .finish()
        .await
        .map_err(|e| format!("finish: {e}"))?;
    Ok(())
}

async fn register_transfer(state: &AppState, id: &str, name: &str, total: u64) {
    state.transfers.lock().await.push(TransferState {
        id: id.to_string(),
        file_name: name.to_string(),
        total,
        sent: 0,
        status: "active".into(),
    });
}

async fn finish_transfer(app: &AppHandle, id: &str, result: Result<(), String>) {
    let state = app.state::<AppState>();
    let mut transfers = state.transfers.lock().await;
    if let Some(t) = transfers.iter_mut().find(|t| t.id == id) {
        t.status = if result.is_ok() {
            "completed".into()
        } else {
            "failed".into()
        };
        t.sent = t.total;

        // Save to history
        let record = HistoryRecord {
            id: t.id.clone(),
            file_name: t.file_name.clone(),
            peer: String::new(),
            direction: "sent".into(),
            size: t.total,
            status: t.status.clone(),
            timestamp: chrono_now(),
        };
        let mut hist = state.history.lock().await;
        hist.insert(0, record);
        if hist.len() > 200 {
            hist.truncate(200);
        }
    }
    let error = result.err();
    let _ = app.emit(
        "transfer-complete",
        serde_json::json!({ "id": id, "success": error.is_none(), "error": error }),
    );
}

fn get_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "QuickShare".into())
}

fn get_local_ips() -> Vec<String> {
    let mut ips = Vec::new();
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:53").is_ok() {
            if let Ok(local) = socket.local_addr() {
                let ip = local.ip().to_string();
                if !ip.starts_with("127.") {
                    ips.push(ip);
                }
            }
        }
    }
    ips
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

// ── App Entry ──

pub fn run() {
    let state = AppState {
        server_shutdown: Arc::new(AtomicBool::new(false)),
        save_dir: Arc::new(Mutex::new(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        )),
        transfers: Arc::new(Mutex::new(Vec::new())),
        history: Arc::new(Mutex::new(Vec::new())),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                run_server(handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_local_info,
            send_files,
            get_transfers,
            cancel_transfer,
            get_history,
            update_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
