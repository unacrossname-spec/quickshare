use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use quinn::{ClientConfig, Endpoint, ServerConfig, TransportConfig, VarInt};
use tokio::io::AsyncWriteExt;

use quickshare_core::transfer::chunk::ChunkReader;
use quickshare_core::transfer::receiver::FileReceiver;
use quickshare_core::transfer::sender::FileSender;
use quickshare_core::types::FileMeta;

const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4 MB

#[derive(Debug)]
struct SkipVerify;
impl rustls::client::danger::ServerCertVerifier for SkipVerify {
    fn verify_server_cert(&self, _e: &rustls::pki_types::CertificateDer<'_>, _i: &[rustls::pki_types::CertificateDer<'_>], _n: &rustls::pki_types::ServerName<'_>, _o: &[u8], _t: rustls::pki_types::UnixTime) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> { Ok(rustls::client::danger::ServerCertVerified::assertion()) }
    fn verify_tls12_signature(&self, _m: &[u8], _c: &rustls::pki_types::CertificateDer<'_>, _d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> { Ok(rustls::client::danger::HandshakeSignatureValid::assertion()) }
    fn verify_tls13_signature(&self, _m: &[u8], _c: &rustls::pki_types::CertificateDer<'_>, _d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> { Ok(rustls::client::danger::HandshakeSignatureValid::assertion()) }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> { use rustls::SignatureScheme::*; vec![RSA_PKCS1_SHA256, RSA_PKCS1_SHA384, RSA_PKCS1_SHA512, RSA_PSS_SHA256, RSA_PSS_SHA384, RSA_PSS_SHA512, ECDSA_NISTP256_SHA256, ECDSA_NISTP384_SHA384, ECDSA_NISTP521_SHA512, ED25519] }
}

fn make_transport() -> TransportConfig {
    let mut t = TransportConfig::default();
    t.max_concurrent_bidi_streams(64u32.into());
    t.send_window(32 * 1024 * 1024);
    t.receive_window(VarInt::from_u64(32 * 1024 * 1024).unwrap());
    t.max_idle_timeout(None);
    t
}

fn gen_self_signed() -> (rustls::pki_types::CertificateDer<'static>, rustls::pki_types::PrivateKeyDer<'static>) {
    let kp = rcgen::KeyPair::generate().unwrap();
    let mut p = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    p.distinguished_name = rcgen::DistinguishedName::new();
    let c = p.self_signed(&kp).unwrap();
    let cert = c.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::try_from(kp.serialize_der()).map_err(|e| format!("{e:?}")).unwrap();
    (cert, key)
}

// -------- Server --------
async fn run_server(port: u16, save_dir: Option<PathBuf>) -> anyhow::Result<()> {
    rustls::crypto::aws_lc_rs::default_provider().install_default().ok();

    let (cert, key) = gen_self_signed();
    let mut sc = ServerConfig::with_single_cert(vec![cert], key)?;
    sc.transport_config(Arc::new(make_transport()));
    let endpoint = Endpoint::server(sc, SocketAddr::from(([0, 0, 0, 0], port)))?;
    println!("Listening on 0.0.0.0:{}", port);

    loop {
        let incoming = endpoint.accept().await;
        match incoming {
            Some(incoming) => {
                let conn = incoming.await?;
                println!("[accept] Connection from {}", conn.remote_address());
                let (send, recv) = conn.accept_bi().await?;
                let mut receiver = FileReceiver::new(send, recv);
                receiver.handshake().await?;
                let meta = receiver.file_meta.clone().unwrap();

                let save_path = save_dir.clone().unwrap_or_else(|| std::env::temp_dir());
                let out_path = save_path.join(&meta.name);

                let start = Instant::now();
                let mut file = tokio::fs::File::create(&out_path).await?;
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
            }
            None => break,
        }
    }
    Ok(())
}

// -------- Client --------
async fn run_send(addr: SocketAddr, file_path: PathBuf) -> anyhow::Result<()> {
    rustls::crypto::aws_lc_rs::default_provider().install_default().ok();

    // Read file
    let data = std::fs::read(&file_path)?;
    let file_size = data.len();

    let file_name = file_path.file_name()
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

    // Set up client endpoint
    let cc = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipVerify))
        .with_no_client_auth();
    let qc = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(cc))
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;
    let mut ccfg = ClientConfig::new(Arc::new(qc));
    ccfg.transport_config(Arc::new(make_transport()));
    let mut endpoint = Endpoint::client("0.0.0.0:0".parse::<SocketAddr>().unwrap())?;
    endpoint.set_default_client_config(ccfg);

    println!("[send] Connecting to {}...", addr);
    let conn = endpoint.connect(addr, "localhost")?.await?;
    println!("[send] Connected!");

    let (send_stream, recv_stream) = conn.open_bi().await?;
    let mut sender = FileSender::new(send_stream, recv_stream, file_meta);
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

    // Wait briefly for receiver to acknowledge
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
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
