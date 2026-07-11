use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Instant;

use tokio::io::AsyncWriteExt;

use quickshare_core::transfer::batch::{self, BatchMeta, BatchReceiver, BatchSender};
use quickshare_core::transfer::chunk::ChunkReader;
use quickshare_core::transfer::receiver::FileReceiver;
use quickshare_core::transfer::sender::{recv_json, send_json, FileSender};
use quickshare_core::transport::tcp::{TcpListener, TcpStream};
use quickshare_core::types::{ControlMessage, FileMeta};

const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4 MB

// -------- Server --------
async fn run_server(port: u16, save_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port))).await?;
    println!("Listening on 0.0.0.0:{}", port);

    loop {
        let stream = listener.accept().await?;
        let peer = stream.peer_addr()?;
        println!("[accept] Connection from {}", peer);
        let save_dir = save_dir.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, save_dir).await {
                eprintln!("[error] {:#}", e);
            }
        });
    }
}

/// Determine if this connection is a batch or single-file transfer.
async fn handle_connection(
    mut stream: TcpStream,
    save_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    // Read the first JSON message to determine the type
    let first: serde_json::Value = recv_json(&mut stream).await?;

    if first.get("total_files").is_some() {
        // ── Batch mode ──
        let meta: BatchMeta = serde_json::from_value(first)?;
        let save_root = save_dir.unwrap_or_else(|| std::env::temp_dir());
        let root_dir = save_root.join(&meta.root_name);

        let _ack = batch::Ack { ok: true };
        send_json(&mut stream, &_ack).await?;

        println!("[batch] Receiving {} files ({})...", meta.total_files, meta.root_name);
        let start = Instant::now();

        let mut receiver = BatchReceiver {
            stream,
            meta: Some(meta),
            bytes_received: 0,
        };
        let mut files_ok = 0u32;

        loop {
            match receiver.recv_file().await? {
                Some((rel_path, data)) => {
                    let full_path = root_dir.join(&rel_path);
                    if let Some(parent) = full_path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(&full_path, &data).await?;
                    files_ok += 1;
                    print!("\r  [{}/{}] {}", files_ok,
                        receiver.meta.as_ref().map(|m| m.total_files).unwrap_or(0), rel_path);
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
                None => break,
            }
        }

        let elapsed = start.elapsed();
        let mbps = (receiver.bytes_received() as f64 * 8.0 / 1_000_000.0) / elapsed.as_secs_f64().max(0.001);
        println!("\n[batch] Done! {} files, {:.2?} = {:.0} Mbps", files_ok, elapsed, mbps);
        println!("[batch] Saved to: {}", root_dir.display());
    } else {
        // ── Single file mode ──
        let req: ControlMessage = serde_json::from_value(first)?;
        let file_meta = match &req {
            ControlMessage::TransferRequest { file_meta, .. } => file_meta.clone(),
            _ => anyhow::bail!("expected TransferRequest"),
        };

        // Send TransferAccept
        let accept = ControlMessage::TransferAccept {
            transfer_id: quickshare_core::types::TransferId::new_v4(),
            received_chunks: vec![],
        };
        send_json(&mut stream, &accept).await?;

        let mut receiver = FileReceiver::from_handshake(stream, file_meta.clone());

        let save_path = save_dir.unwrap_or_else(|| std::env::temp_dir());
        let out_path = save_path.join(&file_meta.name);

        // If compressed, write to a temp file first so we can decompress in-place
        let write_path = if file_meta.compressed {
            out_path.with_extension("lz4.tmp")
        } else {
            out_path.clone()
        };

        let start = Instant::now();
        let mut file = tokio::fs::File::create(&write_path).await?;
        let mut recvd = 0u64;
        let mut chunk_count = 0u64;

        loop {
            match receiver.recv_chunk().await? {
                Some((_info, data)) => {
                    file.write_all(&data).await?;
                    recvd += data.len() as u64;
                    chunk_count += 1;
                    if chunk_count % 16 == 0 {
                        let elapsed = start.elapsed().as_secs_f64();
                        let mbps = (recvd as f64 * 8.0 / 1_000_000.0) / elapsed.max(0.001);
                        print!("\r  Progress: {:.1}% | {:.0} Mbps",
                            recvd as f64 / file_meta.size as f64 * 100.0, mbps);
                        use std::io::Write;
                        std::io::stdout().flush().ok();
                    }
                }
                None => break,
            }
        }
        drop(file);

        // Decompress if needed
        if file_meta.compressed {
            let compressed_data = std::fs::read(&write_path)?;
            let decompressed = quickshare_core::compress::decompress(&compressed_data)?;
            tokio::fs::remove_file(&write_path).await?;

            if file_meta.bundle {
                // Bundle mode: extract all files into a directory
                let root_dir = &out_path;
                tokio::fs::create_dir_all(root_dir).await?;
                let files = quickshare_core::bundle::extract_bundle(&decompressed)?;
                let file_count = files.len();
                let mut total_bytes = 0u64;
                for (rel_path, data) in files {
                    let full_path = root_dir.join(&rel_path);
                    if let Some(parent) = full_path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(&full_path, &data).await?;
                    total_bytes += data.len() as u64;
                }
                println!("\n[bundle] Extracted {} files, {} MB to {}", file_count, total_bytes / 1024 / 1024, root_dir.display());
            } else {
                tokio::fs::write(&out_path, &decompressed).await?;
            }
        }

        let elapsed = start.elapsed();
        let secs = elapsed.as_secs_f64();
        let mbps = (recvd as f64 * 8.0 / 1_000_000.0) / secs;
        println!("\n[receive] Done! {} MB in {:.2?} = {:.0} Mbps",
            recvd / 1024 / 1024, elapsed, mbps);
        if !file_meta.bundle {
            println!("[receive] Saved to: {}", out_path.display());
        }
    }
    Ok(())
}

// -------- Client: send single file --------
async fn run_send(addr: SocketAddr, file_path: PathBuf, compress: bool) -> anyhow::Result<()> {
    let data = std::fs::read(&file_path)?;
    let file_size = data.len();

    // Optionally compress
    let (send_data, is_compressed) = if compress {
        let c = quickshare_core::compress::compress(&data);
        let shrunk = c.len() < data.len();
        (c, shrunk)
    } else {
        (data.clone(), false)
    };

    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let file_meta = FileMeta {
        name: file_name,
        size: file_size as u64,
        chunk_size: CHUNK_SIZE,
        chunk_count: (send_data.len() + CHUNK_SIZE - 1) as u64 / CHUNK_SIZE as u64,
        file_hash: [0u8; 32],
        compressed: is_compressed,
        bundle: false,
        stream: false,
    };

    let send_total = send_data.len();

    println!("[send] Connecting to {}...", addr);
    let stream = TcpStream::connect(addr).await?;
    println!("[send] Connected!{}", if is_compressed {
        format!(" (lz4: {} -> {} MB)", file_size / 1024 / 1024, send_total / 1024 / 1024)
    } else { String::new() });

    let mut sender = FileSender::new(stream, file_meta);
    sender.handshake().await?;

    let start = Instant::now();
    let reader = ChunkReader::new(&send_data[..], CHUNK_SIZE);
    let total = send_total as u64;

    for (i, chunk) in reader.enumerate() {
        let chunk = chunk?;
        sender.send_chunk(&chunk).await?;
        if (i + 1) % 16 == 0 {
            let elapsed = start.elapsed().as_secs_f64();
            let sent = sender.bytes_sent();
            let mbps = (sent as f64 * 8.0 / 1_000_000.0) / elapsed.max(0.001);
            print!("\r  Progress: {:.1}% | {:.0} Mbps",
                sent as f64 / total as f64 * 100.0, mbps);
            use std::io::Write;
            std::io::stdout().flush().ok();
        }
    }

    sender.finish().await?;
    let elapsed = start.elapsed();
    let secs = elapsed.as_secs_f64();
    let sent = sender.bytes_sent();
    let mbps = (sent as f64 * 8.0 / 1_000_000.0) / secs;

    println!("\n[send] Done! {} MB in {:.2?} = {:.0} Mbps", sent / 1024 / 1024, elapsed, mbps);
    Ok(())
}

// -------- Client: send directory --------
async fn run_send_dir(addr: SocketAddr, dir_path: PathBuf, compress: bool, bundle: bool) -> anyhow::Result<()> {
    if !dir_path.is_dir() {
        anyhow::bail!("not a directory: {}", dir_path.display());
    }

    // Collect files (blocking I/O before async)
    let files = batch::collect_files(&dir_path)?;
    if files.is_empty() {
        anyhow::bail!("no files found in {}", dir_path.display());
    }

    let total_size: u64 = files.iter().map(|(_, s)| s).sum();
    let root_name = dir_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // ── Bundle mode: pack all files into one compressed blob, send as single file ──
    if bundle {
        // Read all files into memory
        let mut entries = Vec::with_capacity(files.len());
        for (rel_path, _) in &files {
            let path = dir_path.join(rel_path);
            let data = std::fs::read(&path)?;
            entries.push((rel_path.to_string_lossy().to_string(), data));
        }

        let bundle_data = quickshare_core::bundle::create_bundle(&entries);
        let compressed = quickshare_core::compress::compress(&bundle_data);
        let raw_mb = bundle_data.len() / (1024 * 1024);
        let comp_mb = compressed.len() / (1024 * 1024);

        println!("[bundle] Packing {} files, {} -> {} MB (lz4)",
            files.len(), raw_mb, comp_mb);

        let file_meta = FileMeta {
            name: root_name,
            size: bundle_data.len() as u64,
            chunk_size: CHUNK_SIZE,
            chunk_count: (compressed.len() + CHUNK_SIZE - 1) as u64 / CHUNK_SIZE as u64,
            file_hash: [0u8; 32],
            compressed: true,
            bundle: true,
            stream: false,
        };

        let start = Instant::now();

        println!("[bundle] Connecting to {}...", addr);
        let stream = TcpStream::connect(addr).await?;
        println!("[bundle] Connected!");

        let mut sender = FileSender::new(stream, file_meta);
        sender.handshake().await?;

        let reader = ChunkReader::new(&compressed[..], CHUNK_SIZE);
        let total = compressed.len() as u64;

        for (i, chunk) in reader.enumerate() {
            let chunk = chunk?;
            sender.send_chunk(&chunk).await?;
            if (i + 1) % 16 == 0 {
                let sent = sender.bytes_sent();
                let elapsed = start.elapsed().as_secs_f64();
                let mbps = (sent as f64 * 8.0 / 1_000_000.0) / elapsed.max(0.001);
                print!("\r  Progress: {:.1}% | {:.0} Mbps",
                    sent as f64 / total as f64 * 100.0, mbps);
                use std::io::Write;
                std::io::stdout().flush().ok();
            }
        }

        sender.finish().await?;
        let elapsed = start.elapsed();
        let secs = elapsed.as_secs_f64();
        let sent = sender.bytes_sent();
        let mbps = (sent as f64 * 8.0 / 1_000_000.0) / secs;

        println!("\n[bundle] Done! {} files ({} MB on wire) in {:.2?} = {:.0} Mbps",
            files.len(), sent / 1024 / 1024, elapsed, mbps);
        return Ok(());
    }

    // ── Legacy batch mode (per-file) ──
    let meta = BatchMeta {
        total_files: files.len() as u32,
        total_size,
        root_name,
    };

    println!("[send-dir] Connecting to {}...", addr);
    let stream = TcpStream::connect(addr).await?;
    println!("[send-dir] Connected! {} files, {} MB total{}",
        files.len(), total_size / 1024 / 1024,
        if compress { " (lz4 on)" } else { "" });

    let mut sender = BatchSender::new(stream, meta, CHUNK_SIZE)
        .with_compression(compress);
    sender.handshake().await?;

    let start = Instant::now();

    for (rel_path, _size) in &files {
        let path = dir_path.join(rel_path);
        let data = std::fs::read(&path)?;
        sender.send_file(
            rel_path.to_string_lossy().as_ref(),
            &data,
        ).await?;
        let (sent_files, sent_bytes) = sender.stats();
        let elapsed = start.elapsed().as_secs_f64();
        let mbps = (sent_bytes as f64 * 8.0 / 1_000_000.0) / elapsed.max(0.001);
        println!("\r  [{}/{}] {}  ({:.0} Mbps)",
            sent_files, files.len(), rel_path.display(), mbps);
        use std::io::Write;
        std::io::stdout().flush().ok();
    }

    sender.finish().await?;
    let elapsed = start.elapsed();
    let (sent_files, sent_bytes) = sender.stats();
    let secs = elapsed.as_secs_f64();
    let mbps = (sent_bytes as f64 * 8.0 / 1_000_000.0) / secs;

    println!("\n[send-dir] Done! {} files, {} MB in {:.2?} = {:.0} Mbps",
        sent_files, sent_bytes / 1024 / 1024, elapsed, mbps);
    Ok(())
}

/// Return the Nth positional argument using the same indexing as `args[n]`,
/// transparently skipping `--compress` and `--bundle` flags.
fn nth_non_flag(args: &[String], n: usize) -> anyhow::Result<String> {
    let mut pos = 0;
    for arg in args {
        if arg == "--compress" || arg == "--bundle" {
            continue;
        }
        if pos == n {
            return Ok(arg.clone());
        }
        pos += 1;
    }
    anyhow::bail!("missing positional argument at index {}", n)
}

// -------- Main --------
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage:");
        eprintln!("  quickshare-cli serve [--port PORT] [--save DIR]");
        eprintln!("  quickshare-cli send [--compress] <addr:port> <file>");
        eprintln!("  quickshare-cli send-dir [--compress] [--bundle] <addr:port> <directory>");
        return Ok(());
    }

    let compress = args.iter().any(|a| a == "--compress");
    let bundle = args.iter().any(|a| a == "--bundle");

    match args[1].as_str() {
        "serve" | "server" => {
            let mut port = 8877u16;
            let mut save_dir = None;
            let mut i = 2;
            while i < args.len() {
                if args[i] == "--port" && i + 1 < args.len() {
                    port = args[i + 1].parse()?;
                    i += 2;
                } else if args[i] == "--save" && i + 1 < args.len() {
                    save_dir = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                } else if args[i] == "--compress" || args[i] == "--bundle" {
                    i += 1;
                } else {
                    i += 1;
                }
            }
            run_server(port, save_dir).await
        }
        "send" | "client" => {
            if args.len() < 4 {
                anyhow::bail!("Usage: quickshare-cli send [--compress] <addr:port> <file>");
            }
            let addr: SocketAddr = nth_non_flag(&args, 2)?.parse()?;
            let file = PathBuf::from(nth_non_flag(&args, 3)?);
            if !file.exists() {
                anyhow::bail!("File not found: {}", file.display());
            }
            run_send(addr, file, compress).await
        }
        "send-dir" => {
            if args.len() < 4 {
                anyhow::bail!("Usage: quickshare-cli send-dir [--compress] [--bundle] <addr:port> <directory>");
            }
            let addr: SocketAddr = nth_non_flag(&args, 2)?.parse()?;
            let dir = PathBuf::from(nth_non_flag(&args, 3)?);
            if !dir.is_dir() {
                anyhow::bail!("Directory not found: {}", dir.display());
            }
            run_send_dir(addr, dir, compress, bundle).await
        }
        _ => {
            anyhow::bail!("Unknown command: {}. Use 'serve', 'send', or 'send-dir'", args[1]);
        }
    }
}
