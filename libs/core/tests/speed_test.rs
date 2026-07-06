use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use quickshare_core::transfer::chunk::ChunkReader;
use quickshare_core::transfer::receiver::FileReceiver;
use quickshare_core::transfer::sender::FileSender;
use quickshare_core::types::FileMeta;

const DATA_SIZE: usize = 256 * 1024 * 1024; // 256 MB
const CHUNK_SIZE: usize = 4 * 1024 * 1024;  // 4 MB

// ---------------------------------------------------------------------------
#[tokio::test]
async fn tcp_raw_loopback_throughput() {
    let _ = tracing_subscriber::fmt::try_init();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let data = Arc::new(vec![0xABu8; DATA_SIZE]);
    let start = Instant::now();

    let send_h = tokio::spawn({
        let d = data.clone();
        async move {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let mut offset = 0;
            while offset < DATA_SIZE {
                let end = (offset + CHUNK_SIZE).min(DATA_SIZE);
                stream.write_all(&d[offset..end]).await.unwrap();
                offset = end;
            }
            stream.shutdown().await.unwrap();
            offset as u64
        }
    });

    let recv_h = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut total = 0usize;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(_) => break,
            }
        }
        total as u64
    });

    let (sent, _recvd) = tokio::join!(send_h, recv_h);
    let elapsed = start.elapsed();
    let secs = elapsed.as_secs_f64();
    let sent = sent.unwrap();
    let mbps = (sent as f64 * 8.0 / 1_000_000.0) / secs;

    println!("=== Raw TCP Loopback Throughput ===");
    println!("  Data:        {} MB", sent / 1024 / 1024);
    println!("  Duration:    {:.2?}", elapsed);
    println!("  Throughput:  {:.2} Mbps ({:.2} MB/s)", mbps, sent as f64 / secs / 1024.0 / 1024.0);
    println!("================================");
}

// ---------------------------------------------------------------------------
#[tokio::test]
async fn tcp_protocol_loopback_throughput() {
    let _ = tracing_subscriber::fmt::try_init();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let data = Arc::new(vec![0xABu8; DATA_SIZE]);
    let file_meta = FileMeta {
        name: "speed_test.bin".into(),
        size: DATA_SIZE as u64,
        chunk_size: CHUNK_SIZE,
        chunk_count: (DATA_SIZE + CHUNK_SIZE - 1) as u64 / CHUNK_SIZE as u64,
        file_hash: [0u8; 32],
        compressed: false,
        bundle: false,
    };
    let start = Instant::now();

    let send_h = tokio::spawn(async move {
        let stream = quickshare_core::transport::TcpStream::connect(addr).await.unwrap();
        let mut s = FileSender::new(stream, file_meta);
        s.handshake().await.unwrap();
        let reader = ChunkReader::new(&data[..], CHUNK_SIZE);
        for ch in reader {
            s.send_chunk(&ch.unwrap()).await.unwrap();
        }
        s.finish().await.unwrap();
        s.bytes_sent()
    });

    let recv_h = tokio::spawn(async move {
        let (raw, _) = listener.accept().await.unwrap();
        let stream = quickshare_core::transport::TcpStream::new(raw);
        let mut r = FileReceiver::new(stream);
        r.handshake().await.unwrap();
        loop {
            match r.recv_chunk().await.unwrap() {
                Some((_info, _data)) => {}
                None => break,
            }
        }
        r.bytes_received()
    });

    let (sent, _recvd) = tokio::join!(send_h, recv_h);
    let elapsed = start.elapsed();
    let secs = elapsed.as_secs_f64();
    let sent = sent.unwrap();
    let mbps = (sent as f64 * 8.0 / 1_000_000.0) / secs;

    println!("=== TCP Protocol (FileSender) Loopback Throughput ===");
    println!("  Data:        {} MB", sent / 1024 / 1024);
    println!("  Duration:    {:.2?}", elapsed);
    println!("  Throughput:  {:.2} Mbps ({:.2} MB/s)", mbps, sent as f64 / secs / 1024.0 / 1024.0);
    println!("====================================================");
}

// ---------------------------------------------------------------------------
// Multi-threaded runtime variant to compare
// ---------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mt_tcp_protocol_loopback_throughput() {
    let _ = tracing_subscriber::fmt::try_init();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let data = std::sync::Arc::new(vec![0xABu8; DATA_SIZE]);
    let file_meta = FileMeta {
        name: "speed_test.bin".into(),
        size: DATA_SIZE as u64,
        chunk_size: CHUNK_SIZE,
        chunk_count: (DATA_SIZE + CHUNK_SIZE - 1) as u64 / CHUNK_SIZE as u64,
        file_hash: [0u8; 32],
        compressed: false,
        bundle: false,
    };
    let start = Instant::now();

    let send_h = tokio::spawn({
        let data = data.clone();
        async move {
            let stream = quickshare_core::transport::TcpStream::connect(addr).await.unwrap();
            let mut s = FileSender::new(stream, file_meta);
            s.handshake().await.unwrap();
            let reader = ChunkReader::new(&data[..], CHUNK_SIZE);
            for ch in reader {
                s.send_chunk(&ch.unwrap()).await.unwrap();
            }
            s.finish().await.unwrap();
            s.bytes_sent()
        }
    });

    let recv_h = tokio::spawn(async move {
        let (raw, _) = listener.accept().await.unwrap();
        let stream = quickshare_core::transport::TcpStream::new(raw);
        let mut r = FileReceiver::new(stream);
        r.handshake().await.unwrap();
        loop {
            match r.recv_chunk().await.unwrap() {
                Some((_info, _data)) => {}
                None => break,
            }
        }
        r.bytes_received()
    });

    let (sent, _recvd) = tokio::join!(send_h, recv_h);
    let elapsed = start.elapsed();
    let secs = elapsed.as_secs_f64();
    let sent = sent.unwrap();
    let mbps = (sent as f64 * 8.0 / 1_000_000.0) / secs;

    println!("=== TCP Protocol (multi-thread) Throughput ===");
    println!("  Data:        {} MB", sent / 1024 / 1024);
    println!("  Duration:    {:.2?}", elapsed);
    println!("  Throughput:  {:.2} Mbps ({:.2} MB/s)", mbps, sent as f64 / secs / 1024.0 / 1024.0);
    println!("==============================================");
}
