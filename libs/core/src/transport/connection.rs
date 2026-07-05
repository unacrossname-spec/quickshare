use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig, TransportConfig, VarInt};
use quinn::crypto::rustls::QuicClientConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::DigitallySignedStruct;

use super::config::QuicConfig;

/// Wraps a QUIC endpoint that can act as both client and server.
pub struct QuicEndpoint {
    endpoint: Endpoint,
}

/// A connected QUIC peer with helper methods for stream I/O.
pub struct QuicPeer {
    pub connection: Connection,
}

// ---------------------------------------------------------------------------
// Certificate verification that accepts any certificate (dev/test only)
// ---------------------------------------------------------------------------
#[derive(Debug)]
struct SkipVerify;

impl ServerCertVerifier for SkipVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        use rustls::SignatureScheme::*;
        vec![
            RSA_PKCS1_SHA256,
            RSA_PKCS1_SHA384,
            RSA_PKCS1_SHA512,
            RSA_PSS_SHA256,
            RSA_PSS_SHA384,
            RSA_PSS_SHA512,
            ECDSA_NISTP256_SHA256,
            ECDSA_NISTP384_SHA384,
            ECDSA_NISTP521_SHA512,
            ED25519,
        ]
    }
}

fn transport_config_builder() -> TransportConfig {
    let mut tc = TransportConfig::default();
    tc.max_concurrent_bidi_streams(VarInt::from_u64(64).unwrap());
    tc.send_window(32 * 1024 * 1024);
    tc.receive_window(VarInt::from_u64(32 * 1024 * 1024).unwrap());
    tc.max_idle_timeout(None);
    tc.mtu_discovery_config(Some(quinn::MtuDiscoveryConfig::default()));
    tc
}

impl QuicEndpoint {
    /// Ensure a rustls CryptoProvider is installed (call once).
    fn ensure_crypto() {
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| {
            rustls::crypto::aws_lc_rs::default_provider()
                .install_default()
                .ok();
        });
    }

    /// Create a server endpoint bound to `addr`.
    pub fn server(addr: SocketAddr) -> Result<Self> {
        Self::ensure_crypto();
        let (certs, key) = QuicConfig::gen_tls_config()?;

        let mut server_config = ServerConfig::with_single_cert(certs, key)?;
        server_config.transport_config(Arc::new(transport_config_builder()));

        let endpoint = Endpoint::server(server_config, addr)?;
        Ok(Self { endpoint })
    }

    /// Create a client endpoint bound to an ephemeral port.
    pub fn client() -> Result<Self> {
        Self::ensure_crypto();
        use rustls::ClientConfig as RustlsClientConfig;

        let crypto = RustlsClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipVerify))
            .with_no_client_auth();

        let quic_config = QuicClientConfig::try_from(Arc::new(crypto))
            .map_err(|e| anyhow::anyhow!("{:?}", e))?;
        let mut client_config = ClientConfig::new(Arc::new(quic_config));
        client_config.transport_config(Arc::new(transport_config_builder()));

        let mut endpoint = Endpoint::client((std::net::Ipv4Addr::LOCALHOST, 0).into())?;
        endpoint.set_default_client_config(client_config);
        Ok(Self { endpoint })
    }

    /// Connect to a remote peer.
    pub async fn connect(&self, addr: SocketAddr) -> Result<QuicPeer> {
        let connection = self
            .endpoint
            .connect(addr, "localhost")
            .context("connect failed")?
            .await
            .context("connection handshake failed")?;
        Ok(QuicPeer { connection })
    }

    /// Accept an incoming connection (call in a loop).
    pub async fn accept(&self) -> Result<QuicPeer> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .context("accept: no incoming connection")?;
        let conn = incoming.await.context("accept handshake failed")?;
        Ok(QuicPeer { connection: conn })
    }

    pub fn inner(&self) -> &Endpoint {
        &self.endpoint
    }
}

impl QuicPeer {
    /// Open a new bidirectional stream.
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream)> {
        Ok(self.connection.open_bi().await?)
    }

    /// Accept a bidirectional stream from the peer.
    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream)> {
        loop {
            match self.connection.accept_bi().await {
                Ok(streams) => return Ok(streams),
                Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                    anyhow::bail!("connection closed")
                }
                Err(e) => anyhow::bail!(e),
            }
        }
    }

    pub fn inner(&self) -> &Connection {
        &self.connection
    }
}
