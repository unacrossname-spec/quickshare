use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Instant;

use quickshare_core::transfer::chunk::ChunkReader;
use quickshare_core::transfer::receiver::FileReceiver;
use quickshare_core::transfer::sender::FileSender;
use quickshare_core::transport::tcp::{TcpListener, TcpStream};
use quickshare_core::types::FileMeta;

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
            if let Err(e) = handle_receive(stream, save_dir).await {
                eprintln!("[error] {:#}", e);
            }
        });
    }
}

async fn handle_receive(stream: TcpStream, save_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let mut receiver = FileReceiver::new(stream);
    receiver.handshake().await?;
    let meta = receiver.file_meta.clone().unwrap();

    let save_path = save_dir.unwrap_or_else(|| std::env::temp_dir());
    let out_path = save_path.join(&meta.name);

    let start = Instant::now();
    let mut file = tokio::fs::File::create(&out_path).await?;
    use tokio::io::AsyncWriteExt;

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
                        recvd as f64 / meta.size as f64 * 100.0, mbps);
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
            }
            None => break,
        }
    }

    let elapsed = start.elapsed();
    let secs = elapsed.as_secs_f64();
    let mbps = (recvd as f64 * 8.0 / 1_000_000.0) / secs;
    println!("\n[receive] Done! {} MB in {:.2?} = {:.0} Mbps",
        recvd / 1024 / 1024, elapsed, mbps);
    println!("[receive] Saved to: {}", out_path.display());
    Ok(())
}

// -------- Client --------
async fn run_send(addr: SocketAddr, file_path: PathBuf) -> anyhow::Result<()> {
    let data = std::fs::read(&file_path)?;
    let file_size = data.len();

    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let file_meta = FileMeta {
        name: file_name,
        size: file_size as u64,
        chunk_size: CHUNK_SIZE,
        chunk_count: (file_size + CHUNK_SIZE - 1) as u64 / CHUNK_SIZE as u64,
        file_hash: [0u8; 32],
    };

    println!("[send] Connecting to {}...", addr);
    let stream = TcpStream::connect(addr).await?;
    println!("[send] Connected!");

    let mut sender = FileSender::new(stream, file_meta);
    sender.handshake().await?;

    let start = Instant::now();
    let reader = ChunkReader::new(&data[..], CHUNK_SIZE);
    let total = data.len() as u64;

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

// -------- Main --------
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage:");
        eprintln!("  quickshare-cli serve [--port PORT] [--save DIR]");
        eprintln!("  quickshare-cli send <addr:port> <file>");
        return Ok(());
    }

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
                } else {
                    i += 1;
                }
            }
            run_server(port, save_dir).await
        }
        "send" | "client" => {
            if args.len() < 4 {
                anyhow::bail!("Usage: quickshare-cli send <addr:port> <file>");
            }
            let addr: SocketAddr = args[2].parse()?;
            let file = PathBuf::from(&args[3]);
            if !file.exists() {
                anyhow::bail!("File not found: {}", file.display());
            }
            run_send(addr, file).await
        }
        _ => {
            anyhow::bail!("Unknown command: {}. Use 'serve' or 'send'", args[1]);
        }
    }
}
