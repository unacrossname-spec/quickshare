use std::sync::OnceLock;
use crate::types::AppConfig;

static CONFIG: OnceLock<AppConfig> = OnceLock::new();

pub struct QuicConfig;

impl QuicConfig {
    pub fn init(config: AppConfig) {
        CONFIG.set(config).ok();
    }

    pub fn get() -> AppConfig {
        CONFIG.get().cloned().unwrap_or_default()
    }

    /// Generate a self-signed certificate + private key for local-only QUIC transport.
    pub fn gen_tls_config() -> anyhow::Result<(Vec<rustls::pki_types::CertificateDer<'static>>, rustls::pki_types::PrivateKeyDer<'static>)> {
        use rcgen::{CertificateParams, KeyPair, DistinguishedName};
        use rustls::pki_types::PrivateKeyDer;

        let key_pair = KeyPair::generate()?;
        let mut params = CertificateParams::new(vec!["localhost".to_string()])?;
        params.distinguished_name = DistinguishedName::new();
        let cert = params.self_signed(&key_pair)?;

        let cert_der = cert.der().clone();
        let key_der = PrivateKeyDer::try_from(key_pair.serialize_der())
            .map_err(|e| anyhow::anyhow!("failed to create private key: {}", e))?;

        Ok((vec![cert_der], key_der))
    }
}
