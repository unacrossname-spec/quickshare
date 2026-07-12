use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, WebviewEvent};
use tauri_plugin_dialog::DialogExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use quickshare_core::transfer::batch::{self, BatchMeta, BatchSender};
use quickshare_core::transfer::chunk::ChunkReader;
use quickshare_core::transfer::receiver::FileReceiver;
use quickshare_core::transfer::sender::{recv_json, send_json, FileSender};
use quickshare_core::transport::tcp::{TcpListener, TcpStream};
use quickshare_core::types::{ControlMessage, FileMeta};

const CHUNK_SIZE: usize = 4 * 1024 * 1024;

// ── Drop guard for cleaning up temp files ──

struct TempFileGuard {
    path: Option<PathBuf>,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }
    fn disarm(&mut self) {
        self.path.take();
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

// ── State ──

pub struct AppState {
    pub server_shutdown: Arc<AtomicBool>,
    pub save_dir: Arc<Mutex<PathBuf>>,
    pub transfers: Arc<Mutex<Vec<TransferState>>>,
    pub history: Arc<Mutex<Vec<HistoryRecord>>>,
    pub cancel_flags: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    pub settings: Arc<Mutex<AppSettings>>,
    pub discovery_running: Arc<AtomicBool>,
    /// Pending incoming transfer requests waiting for user confirmation.
    /// Maps transfer_id → oneshot sender<bool> (true = accepted, false = declined).
    pub pending_requests: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
    /// Generation counter for debounced saves — each update_settings call
    /// increments it; only the latest generation writes to disk.
    pub save_gen: Arc<AtomicU64>,
    /// Server restart signal (emitted when port changes).
    pub server_restart: Arc<tokio::sync::Notify>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferState {
    pub id: String,
    pub file_name: String,
    pub total: u64,
    pub sent: u64,
    pub status: String,
    #[serde(skip_serializing)]
    pub started_at: u64,
    #[serde(skip_serializing)]
    pub file_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendOptions {
    pub addr: String,
    pub path: String,
    pub compress: bool,
    pub bundle: bool,
    pub encrypted: bool,
    #[serde(default)]
    pub password: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalInfo {
    pub name: String,
    pub ips: Vec<String>,
    pub save_dir: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub id: String,
    pub file_name: String,
    pub peer: String,
    pub direction: String,
    pub size: u64,
    pub status: String,
    pub timestamp: String,
    #[serde(default)]
    pub speed: f64,
    #[serde(default)]
    pub file_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub compress: bool,
    pub bundle: bool,
    pub notifications_enabled: bool,
    pub port: u16,
    #[serde(default)]
    pub encrypted: bool,
    #[serde(default)]
    pub password: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self { compress: true, bundle: true, notifications_enabled: true, port: 8877, encrypted: false, password: String::new() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredDevice {
    pub name: String,
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct PickedFile {
    pub path: String,
    pub name: String,
    pub size: u64,
}

// ── Persistence ──

fn default_save_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|_| PathBuf::from("."));
    let downloads = home.join("Downloads");
    if downloads.exists() { downloads } else { home }
}

fn data_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|_| PathBuf::from("."));
    let p = home.join(".local").join("share").join("quickshare");
    let _ = std::fs::create_dir_all(&p);
    p
}

fn state_path() -> PathBuf {
    data_dir().join("quickshare_state.json")
}

fn load_state_file() -> (Vec<HistoryRecord>, AppSettings) {
    let path = state_path();
    if let Ok(data) = std::fs::read_to_string(&path) {
        #[derive(Deserialize)]
        struct Persisted {
            history: Vec<HistoryRecord>,
            settings: AppSettings,
        }
        if let Ok(s) = serde_json::from_str::<Persisted>(&data) {
            return (s.history, s.settings);
        }
    }
    (Vec::new(), AppSettings::default())
}

fn save_state_file(history: &[HistoryRecord], settings: &AppSettings) {
    #[derive(Serialize)]
    struct Persisted<'a> {
        history: &'a [HistoryRecord],
        settings: &'a AppSettings,
    }
    let path = state_path();
    let data = match serde_json::to_string(&Persisted { history, settings }) {
        Ok(d) => d,
        Err(_) => return,
    };
    // Write to a temp file first, then atomically rename.
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, &data).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

// ── Commands ──

#[tauri::command]
async fn get_local_info(state: tauri::State<'_, AppState>) -> Result<LocalInfo, String> {
    let save_dir = state.save_dir.lock().await.clone();
    let port = state.settings.lock().await.port;
    Ok(LocalInfo {
        name: get_hostname(),
        ips: get_local_ips(),
        save_dir: save_dir.to_string_lossy().to_string(),
        port,
    })
}

#[tauri::command]
async fn send_files(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    opts: SendOptions,
) -> Result<String, String> {
    eprintln!("[send_files] addr={} path={} compress={} bundle={}", opts.addr, opts.path, opts.compress, opts.bundle);
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
) -> Result<String, String> {
    let addr: SocketAddr = opts.addr.parse().map_err(|e| format!("invalid addr: {e}"))?;
    let file_path = PathBuf::from(&opts.path);
    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    let file_size = std::fs::metadata(&file_path)
        .map(|m| m.len())
        .map_err(|e| format!("stat: {e}"))?;

    // Build FileMeta with estimate chunk_count (receiver never reads it)
    let meta = FileMeta {
        name: file_name.clone(),
        size: file_size,
        chunk_size: CHUNK_SIZE,
        chunk_count: (file_size + CHUNK_SIZE as u64 - 1) / CHUNK_SIZE as u64,
        file_hash: [0u8; 32],
        compressed: opts.compress,
        bundle: false,
        stream: true,  // per-chunk independent compression
        encrypted: opts.encrypted,
    };

    eprintln!("[send_single] file={file_name} size={file_size} addr={addr}");

    let tid = uuid::Uuid::new_v4().to_string();
    register_transfer(&state, &tid, &file_name, file_size).await;
    let data_password = if opts.encrypted { opts.password.clone() } else { String::new() };
    let cancel = Arc::new(AtomicBool::new(false));
    state.cancel_flags.lock().await.insert(tid.clone(), cancel.clone());
    drop(state);

    let tid2 = tid.clone();
    tauri::async_runtime::spawn(async move {
        eprintln!("[send_single] {tid2}: connecting to {addr}...");
        let r = do_send_file_streaming(addr, meta, &file_path, &app, &tid2, cancel, &data_password).await;
        eprintln!("[send_single] {tid2}: result={}", r.is_ok());
        finish_transfer(&app, &tid2, r).await;
    });

    Ok(tid)
}

async fn send_directory(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    opts: SendOptions,
) -> Result<String, String> {
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
    let cancel = Arc::new(AtomicBool::new(false));
    state.cancel_flags.lock().await.insert(tid.clone(), cancel.clone());
    drop(state);

    let encrypted = opts.encrypted;
    let password = if encrypted { opts.password.clone() } else { String::new() };

    if bundle {
        let tid2 = tid.clone();
        let pwd = password.clone();
        tauri::async_runtime::spawn(async move {
            let r = do_send_bundle(addr, dir_path, files, &root_name, &app, &tid2, cancel, encrypted, &pwd).await;
            finish_transfer(&app, &tid2, r).await;
        });
    } else {
        let tid2 = tid.clone();
        let pwd = password.clone();
        tauri::async_runtime::spawn(async move {
            let r = do_send_batch(addr, dir_path, files, compress, &app, &tid2, cancel, encrypted, &pwd).await;
            finish_transfer(&app, &tid2, r).await;
        });
    }

    Ok(tid)
}

#[tauri::command]
async fn get_transfers(state: tauri::State<'_, AppState>) -> Result<Vec<TransferState>, String> {
    Ok(state.transfers.lock().await.clone())
}

#[tauri::command]
async fn cancel_transfer(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    if let Some(flag) = state.cancel_flags.lock().await.get(&id) {
        flag.store(true, Ordering::SeqCst);
    }
    if let Some(t) = state.transfers.lock().await.iter_mut().find(|t| t.id == id) {
        t.status = "cancelled".into();
    }
    Ok(())
}

#[tauri::command]
async fn respond_transfer(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: String,
    accept: bool,
) -> Result<(), String> {
    let sender = state
        .pending_requests
        .lock()
        .await
        .remove(&request_id)
        .ok_or_else(|| format!("no pending request: {request_id}"))?;
    let _ = sender.send(accept);
    if !accept {
        // Remove the transfer entry we added when the request came in
        state.transfers.lock().await.retain(|t| t.id != request_id);
        let _ = app.emit(
            "transfer-complete",
            serde_json::json!({ "id": request_id, "success": false, "error": "declined" }),
        );
    }
    Ok(())
}

#[tauri::command]
async fn get_history(state: tauri::State<'_, AppState>) -> Result<Vec<HistoryRecord>, String> {
    Ok(state.history.lock().await.clone())
}

#[tauri::command]
async fn clear_history(_app: AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.history.lock().await.clear();
    let settings = state.settings.lock().await.clone();
    save_state_file(&[], &settings);
    Ok(())
}

#[tauri::command]
async fn get_discovery_status(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    Ok(state.discovery_running.load(Ordering::SeqCst))
}

#[tauri::command]
async fn pick_file(app: AppHandle) -> Result<Option<PickedFile>, String> {
    let result = app
        .dialog()
        .file()
        .set_title("选择文件")
        .blocking_pick_file();

    match result {
        Some(fp) => {
            let path_buf = fp.into_path().map_err(|e| format!("invalid path: {e}"))?;
            let name = path_buf
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let size = std::fs::metadata(&path_buf).map(|m| m.len()).unwrap_or(0);
            Ok(Some(PickedFile {
                path: path_buf.to_string_lossy().to_string(),
                name,
                size,
            }))
        }
        None => Ok(None),
    }
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    let result = app
        .dialog()
        .file()
        .set_title("选择文件夹")
        .blocking_pick_folder();

    match result {
        Some(fp) => {
            let path_buf = fp.into_path().map_err(|e| format!("invalid path: {e}"))?;
            Ok(Some(path_buf.to_string_lossy().to_string()))
        }
        None => Ok(None),
    }
}

#[tauri::command]
async fn get_settings(state: tauri::State<'_, AppState>) -> Result<AppSettings, String> {
    Ok(state.settings.lock().await.clone())
}

#[tauri::command]
async fn update_settings(
    state: tauri::State<'_, AppState>,
    save_dir: Option<String>,
    app_settings: Option<AppSettings>,
) -> Result<(), String> {
    if let Some(dir) = save_dir {
        let p = PathBuf::from(&dir);
        if !p.exists() {
            std::fs::create_dir_all(&p).map_err(|e| format!("mkdir: {e}"))?;
        }
        *state.save_dir.lock().await = p;
    }
    if let Some(s) = app_settings {
        let old_port = state.settings.lock().await.port;
        let hist = state.history.lock().await.clone();
        *state.settings.lock().await = s.clone();

        // Restart TCP server if port changed
        if s.port != old_port {
            state.server_restart.notify_one();
        }

        // Debounce: only the most recent generation writes to disk.
        // Rapid successive calls to update_settings skip intermediate writes.
        let gen = state.save_gen.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        let save_gen = Arc::clone(&state.save_gen);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if save_gen.load(Ordering::SeqCst) == gen {
                save_state_file(&hist, &s);
            }
        });
    }
    Ok(())
}

#[tauri::command]
async fn debug_log(msg: String) -> Result<(), String> {
    eprintln!("[js-debug] {msg}");
    Ok(())
}

#[tauri::command]
// ── Device Scanning (shared by IPC command and periodic background scan) ──

/// Core scan logic. Sends UDP broadcast probes and collects responses.
async fn do_scan() -> Result<Vec<DiscoveredDevice>, String> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("bind: {e}"))?;
    socket.set_broadcast(true).map_err(|e| format!("broadcast: {e}"))?;

    let probe = b"QUICKSHARE_DISCOVER";
    let targets = get_broadcast_addrs();
    let mut send_count = 0usize;
    for addr in &targets {
        if socket.send_to(probe, addr).await.is_ok() {
            send_count += 1;
        }
    }
    if send_count == 0 {
        return Err("failed to send discovery probe to any interface".into());
    }

    let our_ips = get_local_ips();
    let mut devices = Vec::new();
    let mut buf = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(
            std::time::Duration::from_millis(200),
            socket.recv_from(&mut buf),
        )
        .await
        {
            Ok(Ok((len, _src))) => {
                if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&buf[..len]) {
                    let name = val.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let ip = val.get("ip").and_then(|v| v.as_str()).unwrap_or("");
                    if !ip.is_empty() && !our_ips.iter().any(|i| i == ip)
                        && !devices.iter().any(|d: &DiscoveredDevice| d.ip == ip)
                    {
                        devices.push(DiscoveredDevice {
                            name: name.to_string(),
                            ip: ip.to_string(),
                            port: val.get("port").and_then(|v| v.as_u64()).unwrap_or(8877) as u16,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    Ok(devices)
}

/// Tauri IPC command for on-demand scanning.
#[tauri::command]
async fn scan_devices(_state: tauri::State<'_, AppState>) -> Result<Vec<DiscoveredDevice>, String> {
    do_scan().await
}

/// Periodic background scanner — runs independently of frontend IPC.
/// Pushes results into the webview via `window.eval()` so discovery works
/// even when `__TAURI__` is not available.
async fn run_periodic_scan(app: AppHandle) {
    // Wait for webview to be ready, then scan every 6 seconds.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    eprintln!("[periodic-scan] started");
    loop {
        if app.state::<AppState>().server_shutdown.load(Ordering::SeqCst) {
            break;
        }
        match do_scan().await {
            Ok(devices) => {
                eprintln!("[periodic-scan] found {} devices", devices.len());
                if let Some(window) = app.get_webview_window("main") {
                    let json = serde_json::to_string(&devices).unwrap_or_default();
                    let _ = window.eval(&format!("window.__DISCOVERED_DEVICES = {};", json));
                }
            }
            Err(e) => eprintln!("[periodic-scan] {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    }
}

/// Collect broadcast addresses: 255.255.255.255 + each subnet's directed broadcast.
fn get_broadcast_addrs() -> Vec<String> {
    let mut addrs = vec!["255.255.255.255:8879".to_string()];
    if let Ok(output) = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Each line: "... inet 192.168.1.100/24 brd 192.168.1.255 scope ..."
        for line in stdout.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if let Some(brd_pos) = fields.iter().position(|&w| w == "brd") {
                if let Some(brd) = fields.get(brd_pos + 1) {
                    let addr = format!("{brd}:8879");
                    if !addrs.contains(&addr) {
                        addrs.push(addr);
                    }
                }
            }
        }
    }
    // Fallback for Windows / when `ip` is not available:
    // Guess subnet-directed broadcast from each local IP (assumes /24).
    // This covers >95% of home/office LANs and is a safe addition.
    for ip in get_local_ips() {
        if let Some(broadcast) = guess_broadcast(&ip) {
            let addr = format!("{broadcast}:8879");
            if !addrs.contains(&addr) {
                addrs.push(addr);
            }
        }
    }
    addrs
}

/// Guess the subnet broadcast address for an IP, assuming /24.
fn guess_broadcast(ip: &str) -> Option<String> {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 { return None; }
    // Only for common private ranges where /24 is the norm
    if ip.starts_with("192.168.")
        || ip.starts_with("10.")
        || (ip.starts_with("172.") && {
            parts[1].parse::<u32>().map(|n| (16..=31).contains(&n)).unwrap_or(false)
        })
    {
        return Some(format!("{}.{}.{}.255", parts[0], parts[1], parts[2]));
    }
    None
}

// ── Background Server ──

pub async fn run_server(app: AppHandle) {
    let restart_notify = Arc::clone(&app.state::<AppState>().server_restart);

    'outer: loop {
        if app.state::<AppState>().server_shutdown.load(Ordering::SeqCst) {
            break;
        }

        let port = app.state::<AppState>().settings.lock().await.port;
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let listener = match TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[server] bind {addr}: {e}");
                // Wait before retrying in case of transient error
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
        };
        let _ = app.emit("server-ready", true);
        eprintln!("[server] listening on {addr}");

        loop {
            if app.state::<AppState>().server_shutdown.load(Ordering::SeqCst) {
                break 'outer;
            }

            tokio::select! {
                // Port-change notification — rebind
                _ = restart_notify.notified() => {
                    eprintln!("[server] restart signal received, rebinding...");
                    break; // break inner loop to rebind
                }
                result = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    listener.accept(),
                ) => {
                    match result {
                        Ok(Ok(s)) => {
                            let peer = s.peer_addr().unwrap_or(SocketAddr::from(([0, 0, 0, 0], 0)));
                            let app_c = app.clone();
                            let save_dir = app.state::<AppState>().save_dir.lock().await.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_incoming(s, save_dir, &app_c, peer).await {
                                    eprintln!("[server] {e}");
                                }
                            });
                        }
                        _ => continue,
                    }
                }
            }
        }
    }
}

// ── Discovery Service ──

pub async fn run_discovery(app: AppHandle) {
    let socket = match tokio::net::UdpSocket::bind("0.0.0.0:8879").await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[discovery] bind 0.0.0.0:8879 failed: {e}");
            return;
        }
    };

    // Enable broadcast on the discovery socket (belt-and-suspenders; receiving
    // broadcasts doesn't require SO_BROADCAST, but some platforms are picky).
    if let Err(e) = socket.set_broadcast(true) {
        eprintln!("[discovery] set_broadcast: {e}");
    }

    app.state::<AppState>().discovery_running.store(true, Ordering::SeqCst);
    eprintln!("[discovery] listening on UDP 0.0.0.0:8879");

    let mut buf = [0u8; 256];
    loop {
        if app.state::<AppState>().server_shutdown.load(Ordering::SeqCst) {
            break;
        }
        match tokio::time::timeout(
            std::time::Duration::from_secs(1),
            socket.recv_from(&mut buf),
        )
        .await
        {
            Ok(Ok((len, src))) => {
                if &buf[..len] == b"QUICKSHARE_DISCOVER" {
                    let name = get_hostname();
                    let ips = get_local_ips();
                    let port = app.state::<AppState>().settings.lock().await.port;
                    let response = serde_json::json!({
                        "name": name,
                        "ip": pick_lan_ip(&ips).unwrap_or(""),
                        "port": port,
                    });
                    if let Err(e) = socket
                        .send_to(response.to_string().as_bytes(), &src)
                        .await
                    {
                        eprintln!("[discovery] respond to {src}: {e}");
                    }
                }
            }
            Ok(Err(e)) => eprintln!("[discovery] recv: {e}"),
            _ => {}
        }
    }

    app.state::<AppState>().discovery_running.store(false, Ordering::SeqCst);
}

async fn handle_incoming(
    mut stream: TcpStream,
    save_dir: PathBuf,
    app: &AppHandle,
    peer: SocketAddr,
) -> Result<(), anyhow::Error> {
    eprintln!("[server] new connection from {peer}");
    let first: serde_json::Value = recv_json(&mut stream).await?;
    let req: ControlMessage = serde_json::from_value(first)?;
    let (transfer_id, meta) = match &req {
        ControlMessage::TransferRequest { transfer_id, file_meta } => (*transfer_id, file_meta.clone()),
        _ => anyhow::bail!("expected TransferRequest"),
    };

    let tid_str = transfer_id.to_string();
    let peer_str = peer.to_string();
    eprintln!("[server] TransferRequest from {peer_str}: file={} size={}", meta.name, meta.size);

    // Register as incoming transfer so it shows in the UI immediately,
    // and create a oneshot channel for user confirmation.
    // Acquire pending_requests BEFORE transfers to match the lock order
    // in respond_transfer and avoid deadlock.
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let state = app.state::<AppState>();
        state.pending_requests.lock().await.insert(tid_str.clone(), tx);
        state.transfers.lock().await.push(TransferState {
            id: tid_str.clone(),
            file_name: format!("来自 {}: {}", peer_str, meta.name),
            total: meta.size,
            sent: 0,
            status: "pending".into(),
            started_at: 0,
            file_hash: String::new(),
        });
    }

    // Ask the frontend user for confirmation
    eprintln!("[server] {tid_str}: emitting incoming-transfer event");
    let _ = app.emit("incoming-transfer", serde_json::json!({
        "request_id": tid_str,
        "peer": peer_str,
        "file_name": meta.name,
        "file_size": meta.size,
    }));

    // Wait for user response (accept/decline) with a 120-second timeout
    eprintln!("[server] {tid_str}: waiting for user confirmation...");
    let accepted = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        rx,
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .unwrap_or(false);
    eprintln!("[server] {tid_str}: user responded accepted={accepted}");
    app.state::<AppState>().pending_requests.lock().await.remove(&tid_str);

    if !accepted {
        let reason = "declined or timed out".to_string();
        let _ = send_json(
            &mut stream,
            &ControlMessage::TransferReject {
                transfer_id,
                reason,
            },
        ).await;
        // Remove the pending transfer entry
        app.state::<AppState>().transfers.lock().await.retain(|t| t.id != tid_str);
        return Ok(());
    }

    // User accepted — update transfer status and send accept
    {
        let state = app.state::<AppState>();
        let mut transfers = state.transfers.lock().await;
        if let Some(t) = transfers.iter_mut().find(|t| t.id == tid_str) {
            t.status = "active".to_string();
            t.started_at = chrono_now_ms();
        }
    }

    let _ = send_json(
        &mut stream,
        &ControlMessage::TransferAccept {
            transfer_id,
            received_chunks: vec![],
        },
    ).await;

    // Receive chunks with progress tracking
    let mut receiver = FileReceiver::from_handshake(stream, meta.clone());
    let tmp = save_dir.join(&meta.name).with_extension("tmp");
    let mut tmp_guard = TempFileGuard::new(tmp.clone());
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut recvd = 0u64;

    // For streaming compressed single-file transfers, decompress each chunk
    // on the fly and write directly to the output file, avoiding a tmp file.
    // The `stream` flag indicates per-chunk independent compression (new format).
    let streaming = meta.compressed && !meta.bundle && meta.stream;
    let mut out_file: Option<tokio::fs::File> = if streaming {
        let out = save_dir.join(&meta.name);
        Some(tokio::fs::File::create(&out).await?)
    } else {
        None
    };

    // Derive decryption key from the configured password (if transfer is encrypted)
    let dec_key = if meta.encrypted {
        let pwd = app.state::<AppState>().settings.lock().await.password.clone();
        Some(quickshare_core::crypto::derive_key(&pwd))
    } else {
        None
    };

    // Full-file hash accumulator for received data
    let mut file_hasher: Option<blake3::Hasher> = if streaming {
        Some(blake3::Hasher::new())
    } else {
        None
    };

    loop {
        // Check if transfer was cancelled
        if app.state::<AppState>().cancel_flags.lock().await.get(&tid_str).map_or(false, |f| f.load(Ordering::SeqCst)) {
            eprintln!("[server] {tid_str}: cancelled by user");
            break;
        }
        let chunk = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            receiver.recv_chunk(),
        )
        .await;
        match chunk {
            Ok(Ok(Some((_, data)))) => {
                if let Some(ref mut of) = out_file {
                    // Streaming: decrypt (if enabled), decompress, write
                    let decrypted = if let Some(ref key) = dec_key {
                        quickshare_core::crypto::decrypt(key, &data)
                            .map_err(|e| anyhow::anyhow!("解密失败: {e}"))?
                    } else {
                        data
                    };
                    let decompressed = quickshare_core::compress::decompress(&decrypted)
                        .map_err(|e| anyhow::anyhow!("decompress chunk: {e}"))?;
                    // Update full-file hash with decompressed data
                    if let Some(ref mut h) = file_hasher {
                        h.update(&decompressed);
                    }
                    of.write_all(&decompressed).await?;
                    recvd += decompressed.len() as u64;
                } else {
                    file.write_all(&data).await?;
                    recvd += data.len() as u64;
                }
                // Emit receive progress so both sides see it
                let _ = app.emit(
                    "transfer-progress",
                    serde_json::json!({
                        "id": tid_str,
                        "sent": recvd,
                        "total": meta.size,
                        "direction": "received",
                    }),
                );
            }
            Ok(Ok(None)) => {
                eprintln!("[server] {tid_str}: recv_chunk returned None (done)");
                break;
            }
            Ok(Err(e)) => {
                anyhow::bail!("接收块失败: {e}");
            }
            Err(_) => {
                anyhow::bail!("接收数据超时: 发送端120秒未响应");
            }
        }
    }

    // Flush and close files
    if let Some(ref mut of) = out_file {
        of.shutdown().await?;
    }
    if streaming {
        // Store file hash in transfer state
        if let Some(h) = file_hasher {
            let hash_hex = h.finalize().to_hex().to_string();
            if let Some(st) = app.state::<AppState>().transfers.lock().await.iter_mut().find(|t| t.id == tid_str) {
                st.file_hash = hash_hex;
            }
        }
        tmp_guard.disarm(); // no tmp file was created
        finish_receive(app, &tid_str, &peer_str, &meta.name, meta.size, true).await;
        let _ = app.emit(
            "receive-complete",
            serde_json::json!({
                "peer": peer_str,
                "file": meta.name,
                "size": recvd,
            }),
        );
    } else {
        file.shutdown().await?;
        eprintln!("[server] {tid_str}: file closed, processing...");

        // Check if cancelled while receiving
        if app.state::<AppState>().cancel_flags.lock().await.get(&tid_str).map_or(false, |f| f.load(Ordering::SeqCst)) {
            finish_receive(app, &tid_str, &peer_str, &meta.name, recvd, false).await;
            return Ok(());
        }

        // Process received data (bundle, compressed, or single file)
        if meta.bundle {
            let mut data = if meta.compressed {
                let raw = std::fs::read(&tmp).map_err(|e| anyhow::anyhow!("read tmp for bundle: {e}"))?;
                tmp_guard.disarm();
                tokio::fs::remove_file(&tmp).await.map_err(|e| anyhow::anyhow!("remove tmp for bundle: {e}"))?;
                quickshare_core::compress::decompress(&raw)?
            } else {
                let raw = std::fs::read(&tmp)?;
                tmp_guard.disarm();
                tokio::fs::remove_file(&tmp).await?;
                raw
            };
            // Decrypt if sender used encryption
            if meta.encrypted {
                let key = dec_key.as_ref().ok_or_else(|| anyhow::anyhow!("encrypted bundle but no local password set"))?;
                data = quickshare_core::crypto::decrypt(key, &data)
                    .map_err(|e| anyhow::anyhow!("bundle decrypt failed (wrong password?): {e}"))?;
            }
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
            // Update transfer entry and history
            finish_receive(app, &tid_str, &peer_str, &meta.name, meta.size, true).await;
            let _ = app.emit(
                "receive-complete",
                serde_json::json!({
                    "peer": peer_str,
                    "name": meta.name,
                    "count": file_count,
                    "total_bytes": total,
                }),
            );
        } else if meta.compressed {
        // Backward compatibility: old senders compress the whole file as one
        // blob (no `stream` flag).  The tmp file holds the complete compressed
        // data — decompress it all at once.
        let raw = std::fs::read(&tmp)?;
        tmp_guard.disarm();
        tokio::fs::remove_file(&tmp).await?;
        let mut data = quickshare_core::compress::decompress(&raw)?;
        // Decrypt if sender used encryption
        if meta.encrypted {
            let key = dec_key.as_ref().ok_or_else(|| anyhow::anyhow!("encrypted file but no local password set"))?;
            data = quickshare_core::crypto::decrypt(key, &data)
                .map_err(|e| anyhow::anyhow!("decrypt failed (wrong password?): {e}"))?;
        }
        let out = save_dir.join(&meta.name);
        tokio::fs::write(&out, &data).await?;
        finish_receive(app, &tid_str, &peer_str, &meta.name, meta.size, true).await;
        let _ = app.emit(
            "receive-complete",
            serde_json::json!({
                "peer": peer_str,
                "file": meta.name,
                "size": recvd,
            }),
        );
        } else {
        let out = save_dir.join(&meta.name);
        // Decrypt if sender used encryption (uncompressed plain file)
        if meta.encrypted {
            let key = dec_key.as_ref().ok_or_else(|| anyhow::anyhow!("encrypted file but no local password set"))?;
            let raw = std::fs::read(&tmp).map_err(|e| anyhow::anyhow!("read tmp for decrypt: {e}"))?;
            tmp_guard.disarm();
            tokio::fs::remove_file(&tmp).await?;
            let decrypted = quickshare_core::crypto::decrypt(key, &raw)
                .map_err(|e| anyhow::anyhow!("decrypt failed (wrong password?): {e}"))?;
            tokio::fs::write(&out, &decrypted).await?;
        } else {
            tmp_guard.disarm();
            tokio::fs::rename(&tmp, &out).await.map_err(|e| anyhow::anyhow!("rename tmp: {e}"))?;
        }
        finish_receive(app, &tid_str, &peer_str, &meta.name, meta.size, true).await;
        let _ = app.emit(
            "receive-complete",
            serde_json::json!({
                "peer": peer_str,
                "file": meta.name,
                "size": recvd,
            }),
        );
        }
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
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    // Timeout for TCP connect (10s). A hung connect is worse than a fast failure.
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TcpStream::connect(addr),
    )
    .await
    .map_err(|_| format!("连接超时: {addr}"))?
    .map_err(|e| format!("连接失败: {e}"))?;

    let mut sender = FileSender::new(stream, meta);
    // Handshake sends TransferRequest and waits for the receiver's response.
    // This may take a while because the receiver shows a confirmation dialog.
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        sender.handshake(),
    )
    .await
    .map_err(|_| "握手超时: 对方未响应".to_string())?
    .map_err(|e| format!("握手失败: {e}"))?;

    // Check if accepted
    match &response {
        ControlMessage::TransferReject { reason, .. } => {
            return Err(format!("对方拒绝了传输: {reason}"));
        }
        _ => {}
    }

    let total = data.len() as u64;
    let reader = ChunkReader::new(data, CHUNK_SIZE);
    let mut sent = 0u64;
    for chunk in reader {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
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
    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    sender.finish().await.map_err(|e| format!("finish: {e}"))?;
    Ok(())
}

/// Streaming variant: connect and handshake first, then read file in chunks,
/// compress each chunk independently, and send.  Never loads the whole file
/// into memory.
async fn do_send_file_streaming(
    addr: SocketAddr,
    meta: FileMeta,
    file_path: &PathBuf,
    app: &AppHandle,
    tid: &str,
    cancel: Arc<AtomicBool>,
    password: &str,
) -> Result<(), String> {
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TcpStream::connect(addr),
    )
    .await
    .map_err(|_| format!("连接超时: {addr}"))?
    .map_err(|e| format!("连接失败: {e}"))?;

    let mut sender = FileSender::new(stream, meta.clone());
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        sender.handshake(),
    )
    .await
    .map_err(|_| "握手超时: 对方未响应".to_string())?
    .map_err(|e| format!("握手失败: {e}"))?;

    match &response {
        ControlMessage::TransferReject { reason, .. } => {
            return Err(format!("对方拒绝了传输: {reason}"));
        }
        _ => {}
    }

    // Open the file and stream chunks from disk
    let file = std::fs::File::open(file_path).map_err(|e| format!("open: {e}"))?;
    let reader = ChunkReader::new(file, CHUNK_SIZE);
    let mut sent = 0u64;

    // Derive encryption key from password (if encryption is enabled)
    let enc_key = if meta.encrypted {
        Some(quickshare_core::crypto::derive_key(password))
    } else {
        None
    };

    // Full-file hash accumulator (hashes original uncompressed data)
    let mut file_hasher = blake3::Hasher::new();

    for chunk in reader {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let c = chunk.map_err(|e| format!("chunk: {e}"))?;
        let chunk_len = c.data.len() as u64;

        // Full-file hash: hash the ORIGINAL data (before compression/encryption)
        file_hasher.update(&c.data);

        // Compress each chunk independently if compression is enabled.
        // Always produce a valid LZ4 frame so the receiver can decompress it.
        let send_data = if meta.compressed {
            quickshare_core::compress::compress_always(&c.data)
        } else {
            c.data
        };

        // Encrypt the chunk data if encryption is enabled
        let send_data = if let Some(ref key) = enc_key {
            quickshare_core::crypto::encrypt(key, &send_data)
        } else {
            send_data
        };

        let hash = *blake3::hash(&send_data).as_bytes();

        let wire_chunk = quickshare_core::transfer::chunk::Chunk {
            index: c.index,
            offset: c.offset,
            data: send_data,
            hash,
        };

        sender
            .send_chunk(&wire_chunk)
            .await
            .map_err(|e| format!("send: {e}"))?;

        // Track progress based on original (uncompressed) bytes
        sent += chunk_len;
        let _ = app.emit(
            "transfer-progress",
            serde_json::json!({ "id": tid, "sent": sent, "total": meta.size }),
        );
    }

    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
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
    cancel: Arc<AtomicBool>,
    encrypted: bool,
    password: &str,
) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }

    let mut entries = Vec::with_capacity(files.len());
    for (rel, _) in files {
        let full = dir_path.join(&rel);
        let data = std::fs::read(&full).map_err(|e| format!("read: {e}"))?;
        entries.push((rel.to_string_lossy().to_string(), data));
    }

    let bundle = quickshare_core::bundle::create_bundle(&entries);
    let bundle_size = bundle.len() as u64;
    let compressed = quickshare_core::compress::compress(&bundle);
    let is_compressed = compressed.len() < bundle.len();

    // Apply encryption after compression (or on raw bundle if not compressed)
    let mut payload = if is_compressed { compressed } else { bundle };
    if encrypted {
        let key = quickshare_core::crypto::derive_key(password);
        payload = quickshare_core::crypto::encrypt(&key, &payload);
    }

    let file_meta = FileMeta {
        name: root_name.to_string(),
        size: bundle_size,
        chunk_size: CHUNK_SIZE,
        chunk_count: (payload.len() + CHUNK_SIZE - 1) as u64 / CHUNK_SIZE as u64,
        file_hash: [0u8; 32],
        compressed: is_compressed,
        bundle: true,
        stream: false,
        encrypted,
    };

    do_send_file(addr, file_meta, &payload, app, tid, cancel).await
}

async fn do_send_batch(
    addr: SocketAddr,
    dir_path: PathBuf,
    files: Vec<(PathBuf, u64)>,
    compress: bool,
    app: &AppHandle,
    tid: &str,
    cancel: Arc<AtomicBool>,
    encrypted: bool,
    password: &str,
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

    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TcpStream::connect(addr),
    )
    .await
    .map_err(|_| format!("连接超时: {addr}"))?
    .map_err(|e| format!("连接失败: {e}"))?;
    let mut sender = BatchSender::new(stream, meta, CHUNK_SIZE).with_compression(compress);
    let _response = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        sender.handshake(),
    )
    .await
    .map_err(|_| "握手超时: 对方未响应".to_string())?
    .map_err(|e| format!("握手失败: {e}"))?;

    let mut sent = 0u64;
    for (rel, _) in files {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let full = dir_path.join(&rel);
        let mut data = std::fs::read(&full).map_err(|e| format!("read: {e}"))?;
        if encrypted {
            let key = quickshare_core::crypto::derive_key(password);
            data = quickshare_core::crypto::encrypt(&key, &data);
        }
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
    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
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
        started_at: chrono_now_ms(),
        file_hash: String::new(),
    });
}

async fn finish_transfer(app: &AppHandle, id: &str, result: Result<(), String>) {
    let state = app.state::<AppState>();
    state.cancel_flags.lock().await.remove(id);

    let is_cancelled = matches!(&result, Err(e) if e == "cancelled");
    let status = if result.is_ok() { "completed" } else if is_cancelled { "cancelled" } else { "failed" };

    let mut transfers = state.transfers.lock().await;
    if let Some(t) = transfers.iter_mut().find(|t| t.id == id) {
        t.status = status.to_string();
        // Only mark as fully sent on success; preserve actual progress on failure
        if result.is_ok() {
            t.sent = t.total;
        }

        let elapsed = std::cmp::max(chrono_now_ms().saturating_sub(t.started_at), 1);
        let speed = (t.total as f64) / (elapsed as f64 / 1000.0);

        let file_hash = t.file_hash.clone();
        let record = HistoryRecord {
            id: t.id.clone(),
            file_name: t.file_name.clone(),
            peer: String::new(),
            direction: "sent".into(),
            size: t.total,
            status: status.to_string(),
            timestamp: chrono_now(),
            speed,
            file_hash,
        };
        let mut hist = state.history.lock().await;
        hist.insert(0, record);
        if hist.len() > 200 {
            hist.truncate(200);
        }
    }
    drop(transfers);

    // Persist after history changes
    let hist = state.history.lock().await.clone();
    let settings = state.settings.lock().await.clone();
    save_state_file(&hist, &settings);

    if !is_cancelled {
        let error = result.err();
        let _ = app.emit(
            "transfer-complete",
            serde_json::json!({ "id": id, "success": error.is_none(), "error": error }),
        );
    }
}

async fn finish_receive(app: &AppHandle, id: &str, peer: &str, file_name: &str, size: u64, success: bool) {
    let state = app.state::<AppState>();
    let mut transfers = state.transfers.lock().await;
    let speed = if let Some(t) = transfers.iter_mut().find(|t| t.id == id) {
        t.status = if success { "completed".to_string() } else { "failed".to_string() };
        t.total = size;
        t.sent = if success { size } else { t.sent };
        let elapsed = std::cmp::max(chrono_now_ms().saturating_sub(t.started_at), 1);
        (size as f64) / (elapsed as f64 / 1000.0)
    } else {
        0.0
    };
    // Emit final progress at 100% so the receiver's frontend shows full bar
    if success {
        let _ = app.emit("transfer-progress", serde_json::json!({
            "id": id,
            "sent": size,
            "total": size,
        }));
    }
    // Record in history
    let file_hash = transfers.iter().find(|t| t.id == id).map(|t| t.file_hash.clone()).unwrap_or_default();
    let record = HistoryRecord {
        id: id.to_string(),
        file_name: file_name.to_string(),
        peer: peer.to_string(),
        direction: "received".into(),
        size,
        status: if success { "completed".to_string() } else { "failed".to_string() },
        timestamp: chrono_now(),
        speed,
        file_hash,
    };
    let mut hist = state.history.lock().await;
    hist.insert(0, record);
    if hist.len() > 200 { hist.truncate(200); }
    // Persist
    let hist2 = hist.clone();
    let settings = state.settings.lock().await.clone();
    drop(transfers);
    drop(hist);
    save_state_file(&hist2, &settings);
}

fn pick_lan_ip(ips: &[String]) -> Option<&str> {
    // Prefer 192.168.x.x (most common home/office LAN)
    if let Some(ip) = ips.iter().find(|ip| ip.starts_with("192.168.")) {
        return Some(ip);
    }
    // Then 10.x.x.x (corporate networks)
    if let Some(ip) = ips.iter().find(|ip| ip.starts_with("10.")) {
        return Some(ip);
    }
    // Then 172.16-31.x.x (RFC 1918 — less common but valid private range)
    if let Some(ip) = ips.iter().find(|ip| {
        ip.starts_with("172.") && {
            ip.split('.').nth(1)
                .and_then(|s| s.parse::<u32>().ok())
                .map(|n| (16..=31).contains(&n))
                .unwrap_or(false)
        }
    }) {
        return Some(ip);
    }
    // Fallback: first non-loopback IP
    ips.first().map(|s| s.as_str())
}

fn get_hostname() -> String {
    // Try hostname command first (always accurate), then fallback
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().map(|s| s.trim().to_string()))
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "QuickShare".into())
}

fn looks_like_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 { return false; }
    parts.iter().all(|p| p.parse::<u8>().is_ok())
}

/// Returns true if this IPv4 address could plausibly be a LAN interface
/// (not loopback, link-local, Docker/WSL virtual, or benchmarking range).
fn is_plausible_lan_ip(s: &str) -> bool {
    if s.starts_with("127.")
        || s.starts_with("169.254.")   // link-local / APIPA
        || s.starts_with("198.18.")    // RFC 2544 benchmarking (WSL2, Docker Desktop)
        || s.starts_with("198.19.")    // RFC 2544 benchmarking
        || s.starts_with("255.")       // subnet mask
        || s.starts_with("0.")
    {
        return false;
    }
    true
}

fn get_local_ips() -> Vec<String> {
    let mut ips = Vec::new();

    // 1. Linux: ip -4 -o addr show
    if let Ok(output) = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(ip) = line
                .split_whitespace()
                .nth(3)
                .and_then(|s| s.split('/').next())
            {
                let ip = ip.trim();
                if is_plausible_lan_ip(ip) && !ips.contains(&ip.to_string()) {
                    ips.push(ip.to_string());
                }
            }
        }
    }

    // 2. Windows: ipconfig.
    //    Locale-independent: both "IPv4 Address" (en) and "IPv4 地址" (zh)
    //    contain the keyword "IPv4". Only parse those lines to avoid
    //    accidentally picking up gateway / subnet-mask / DNS-suffix lines.
    //
    //    Uses String::from_utf8_lossy because on non-English Windows the
    //    console output is encoded in the system ANSI code page (e.g. GBK on
    //    Chinese Windows), NOT UTF-8.
    if ips.is_empty() {
        if let Ok(output) = std::process::Command::new("ipconfig").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if !line.contains("IPv4") {
                    continue;
                }
                if let Some(pos) = line.rfind(": ") {
                    let candidate = line[pos + 2..].trim();
                    if looks_like_ipv4(candidate)
                        && is_plausible_lan_ip(candidate)
                        && !ips.contains(&candidate.to_string())
                    {
                        ips.push(candidate.to_string());
                    }
                }
            }
        }
    }

    // 3. Fallback: UDP connect to a well-known address and read local addr.
    //    Last resort — this picks the "default route" interface which is often
    //    a virtual adapter (WSL, Docker) on Windows. We use it only when both
    //    `ip` and `ipconfig` failed.
    if ips.is_empty() {
        if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if socket.connect("8.8.8.8:53").is_ok() {
                if let Ok(local) = socket.local_addr() {
                    let ip = local.ip().to_string();
                    if is_plausible_lan_ip(&ip) && !ips.contains(&ip) {
                        ips.push(ip);
                    }
                }
            }
        }
    }

    ips
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    // Store epoch seconds; frontend formats with browser's local timezone
    d.as_secs().to_string()
}

fn chrono_now_ms() -> u64 {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    d.as_millis() as u64
}

// ── App Entry ──

pub fn run() {
    let (saved_history, saved_settings) = load_state_file();

    let state = AppState {
        server_shutdown: Arc::new(AtomicBool::new(false)),
        save_dir: Arc::new(Mutex::new(default_save_dir())),
        transfers: Arc::new(Mutex::new(Vec::new())),
        history: Arc::new(Mutex::new(saved_history)),
        cancel_flags: Arc::new(Mutex::new(HashMap::new())),
        settings: Arc::new(Mutex::new(saved_settings)),
        discovery_running: Arc::new(AtomicBool::new(false)),
        pending_requests: Arc::new(Mutex::new(HashMap::new())),
        save_gen: Arc::new(AtomicU64::new(0)),
        server_restart: Arc::new(tokio::sync::Notify::new()),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .setup(|app| {
            let handle = app.handle().clone();
            let disc_handle = handle.clone();
            let _scan_handle = handle.clone();

            // Set up native file drag-and-drop handler.
            // Tauri's webview emits DragDrop events; we forward them to the
            // frontend as custom 'file-dropped' events so the JS layer doesn't
            // need the @tauri-apps/api npm package.
            if let Some(window) = app.get_webview_window("main") {
                let drop_handle = handle.clone();
                window.on_webview_event(move |event| {
                    if let WebviewEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                        let file_list: Vec<serde_json::Value> = paths
                            .iter()
                            .map(|p| {
                                let path_str = p.to_string_lossy().to_string();
                                let name = std::path::Path::new(&path_str)
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("unknown")
                                    .to_string();
                                let size = std::fs::metadata(&path_str)
                                    .map(|m| m.len())
                                    .unwrap_or(0);
                                serde_json::json!({
                                    "path": path_str,
                                    "name": name,
                                    "size": size,
                                })
                            })
                            .collect();
                        let _ = drop_handle.emit("file-dropped", file_list);
                    }
                });
            }

            // Inject bootstrap data into webview (bypasses __TAURI__ IPC dependency)
            let boot_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                // Give the webview a moment to initialize
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                if let Some(window) = boot_handle.get_webview_window("main") {
                    let name = get_hostname();
                    let ips = get_local_ips();
                    let save_dir_path = boot_handle.state::<AppState>().save_dir.lock().await.clone();
                    let data = serde_json::json!({
                        "name": name,
                        "ips": ips,
                        "saveDir": save_dir_path.to_string_lossy(),
                        "port": 8877u16,
                    });
                    let _ = window.eval(&format!("window.__BOOTSTRAP_DATA = {};", data));
                }
            });

            tauri::async_runtime::spawn(async move {
                run_server(handle).await;
            });
            tauri::async_runtime::spawn(async move {
                run_discovery(disc_handle).await;
            });
            // Periodic scan disabled — user clicks 刷新 button to scan manually.
            // tauri::async_runtime::spawn(async move {
            //     run_periodic_scan(scan_handle).await;
            // });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_local_info,
            send_files,
            get_transfers,
            cancel_transfer,
            respond_transfer,
            get_history,
            clear_history,
            get_discovery_status,
            get_settings,
            update_settings,
            scan_devices,
            pick_file,
            pick_folder,
            debug_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
