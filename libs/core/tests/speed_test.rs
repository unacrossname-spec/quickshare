use std::sync::Arc;
use std::time::Instant;

use quickshare_core::transfer::chunk::ChunkReader;
use quickshare_core::transfer::receiver::FileReceiver;
use quickshare_core::transfer::sender::FileSender;
use quickshare_core::types::FileMeta;

const DATA_SIZE: usize = 256 * 1024 * 1024;  // 256 MB
const CHUNK_SIZE: usize = 4 * 1024 * 1024;   // 4 MB

fn make_transport() -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    t.max_concurrent_bidi_streams(64u32.into());
    t.send_window(32 * 1024 * 1024);
    t.receive_window(quinn::VarInt::from_u64(32 * 1024 * 1024).unwrap());
    t.max_idle_timeout(None);
    t
}

fn setup_crypto() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
}

fn gen_cert() -> (rustls::pki_types::CertificateDer<'static>, rustls::pki_types::PrivateKeyDer<'static>) {
    let kp = rcgen::KeyPair::generate().unwrap();
    let mut p = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    p.distinguished_name = rcgen::DistinguishedName::new();
    let c = p.self_signed(&kp).unwrap();
    let cert = c.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::try_from(kp.serialize_der())
        .map_err(|e| format!("{e:?}"))
        .unwrap();
    (cert, key)
}

/// Client + server handshake, returning both connections + endpoints
async fn handshake() -> (quinn::Connection, quinn::Connection, quinn::Endpoint, quinn::Endpoint) {
    setup_crypto();
    let (cert, key) = gen_cert();

    let mut sc = quinn::ServerConfig::with_single_cert(vec![cert.clone()], key).unwrap();
    sc.transport_config(Arc::new(make_transport()));
    let server = quinn::Endpoint::server(sc, "127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = server.local_addr().unwrap();

    let cc = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipVerify))
        .with_no_client_auth();
    let qc = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(cc))
        .map_err(|e| format!("{e:?}"))
        .unwrap();
    let mut ccfg = quinn::ClientConfig::new(Arc::new(qc));
    ccfg.transport_config(Arc::new(make_transport()));
    let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client.set_default_client_config(ccfg);

    let connecting = client.connect(addr, "localhost").unwrap();
    let incoming = server.accept().await.unwrap();
    let server_conn = incoming.await.unwrap();
    let client_conn = connecting.await.unwrap();

    (client_conn, server_conn, server, client)
}

// ---------------------------------------------------------------------------
#[tokio::test]
async fn quic_raw_loopback_throughput() {
    let _ = tracing_subscriber::fmt::try_init();
    let (client_conn, server_conn, _server, _client) = handshake().await;

    let data = Arc::new(vec![0xABu8; DATA_SIZE]);
    let start = Instant::now();

    let send_h = tokio::spawn({
        let d = data.clone();
        async move {
            let (mut send, _recv) = client_conn.open_bi().await.unwrap();
            let mut offset = 0;
            while offset < DATA_SIZE {
                let end = (offset + CHUNK_SIZE).min(DATA_SIZE);
                send.write_all(&d[offset..end]).await.unwrap();
                offset = end;
            }
            send.finish().unwrap();
            offset as u64
        }
    });

    let recv_h = tokio::spawn(async move {
        let (_send, mut recv) = server_conn.accept_bi().await.unwrap();
        let mut total = 0usize;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            match recv.read(&mut buf).await {
                Ok(Some(n)) => total += n,
                Ok(None) => break,
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

    println!("=== Raw QUIC Loopback Throughput ===");
    println!("  Data:        256 MB");
    println!("  Duration:    {:.2?}", elapsed);
    println!("  Throughput:  {:.2} Mbps ({:.2} MB/s)", mbps, sent as f64 / secs / 1024.0 / 1024.0);
    println!("===================================");
}

// ---------------------------------------------------------------------------
#[tokio::test]
async fn quic_protocol_loopback_throughput() {
    let _ = tracing_subscriber::fmt::try_init();
    let (client_conn, server_conn, _server, _client) = handshake().await;

    let data = Arc::new(vec![0xABu8; DATA_SIZE]);
    let file_meta = FileMeta {
        name: "speed_test.bin".into(),
        size: DATA_SIZE as u64,
        chunk_size: CHUNK_SIZE,
        chunk_count: (DATA_SIZE + CHUNK_SIZE - 1) as u64 / CHUNK_SIZE as u64,
        file_hash: [0u8; 32],
    };
    let start = Instant::now();

    let send_h = tokio::spawn(async move {
        let (send, recv) = client_conn.open_bi().await.unwrap();
        let mut s = FileSender::new(send, recv, file_meta);
        s.handshake().await.unwrap();
        let reader = ChunkReader::new(&data[..], CHUNK_SIZE);
        for ch in reader {
            s.send_chunk(&ch.unwrap()).await.unwrap();
        }
        s.finish().await.unwrap();
        s.bytes_sent()
    });

    let recv_h = tokio::spawn(async move {
        let (send, recv) = server_conn.accept_bi().await.unwrap();
        let mut r = FileReceiver::new(send, recv);
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

    println!("=== Protocol (FileSender/Receiver) Loopback Throughput ===");
    println!("  Data:        {} MB", sent / 1024 / 1024);
    println!("  Duration:    {:.2?}", elapsed);
    println!("  Throughput:  {:.2} Mbps ({:.2} MB/s)", mbps, sent as f64 / secs / 1024.0 / 1024.0);
    println!("=========================================================");
}

// ---------------------------------------------------------------------------
#[tokio::test]
async fn multi_stream_loopback_throughput() {
    let _ = tracing_subscriber::fmt::try_init();
    let (client_conn, server_conn, _server, _client) = handshake().await;

    let data = Arc::new(vec![0xABu8; DATA_SIZE]);
    let num_streams = 4;
    let start = Instant::now();

    let send_h = tokio::spawn(async move {
        let mut handles = Vec::new();
        for _ in 0..num_streams {
            let (mut send, _recv) = client_conn.open_bi().await.unwrap();
            let d = data.clone();
            handles.push(tokio::spawn(async move {
                let per = DATA_SIZE / num_streams;
                let mut off = 0;
                while off < per {
                    let end = (off + CHUNK_SIZE).min(per);
                    send.write_all(&d[off..end]).await.unwrap();
                    off = end;
                }
                send.finish().unwrap();
                off as u64
            }));
        }
        let mut total = 0u64;
        for h in handles { total += h.await.unwrap(); }
        total
    });

    let recv_h = tokio::spawn(async move {
        let mut handles = Vec::new();
        for _ in 0..num_streams {
            let (_send, mut recv) = server_conn.accept_bi().await.unwrap();
            handles.push(tokio::spawn(async move {
                let mut total = 0usize;
                let mut buf = vec![0u8; 256 * 1024];
                loop {
                    match recv.read(&mut buf).await {
                        Ok(Some(n)) => total += n,
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
                total as u64
            }));
        }
        let mut total = 0u64;
        for h in handles { total += h.await.unwrap(); }
        total
    });

    let (sent, _recvd) = tokio::join!(send_h, recv_h);
    let elapsed = start.elapsed();
    let secs = elapsed.as_secs_f64();
    let sent = sent.unwrap();
    let mbps = (sent as f64 * 8.0 / 1_000_000.0) / secs;

    println!("=== Multi-Stream ({} streams) Throughput ===", num_streams);
    println!("  Data:        {} MB", sent / 1024 / 1024);
    println!("  Duration:    {:.2?}", elapsed);
    println!("  Throughput:  {:.2} Mbps ({:.2} MB/s)", mbps, sent as f64 / secs / 1024.0 / 1024.0);
    println!("============================================");
}

// ---------------------------------------------------------------------------
// Skip TLS verification for localhost testing
// ---------------------------------------------------------------------------
#[derive(Debug)]
struct SkipVerify;

impl rustls::client::danger::ServerCertVerifier for SkipVerify {
    fn verify_server_cert(
        &self,
        _e: &rustls::pki_types::CertificateDer<'_>,
        _i: &[rustls::pki_types::CertificateDer<'_>],
        _n: &rustls::pki_types::ServerName<'_>,
        _o: &[u8],
        _t: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _m: &[u8],
        _c: &rustls::pki_types::CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _m: &[u8],
        _c: &rustls::pki_types::CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        use rustls::SignatureScheme::*;
        vec![RSA_PKCS1_SHA256, RSA_PKCS1_SHA384, RSA_PKCS1_SHA512,
             RSA_PSS_SHA256, RSA_PSS_SHA384, RSA_PSS_SHA512,
             ECDSA_NISTP256_SHA256, ECDSA_NISTP384_SHA384, ECDSA_NISTP521_SHA512, ED25519]
    }
}
