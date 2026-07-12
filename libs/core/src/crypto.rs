use anyhow::Result;
use sha2::{Digest, Sha256};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};

/// Derive a 256-bit AES key from a password.
///
/// New transfers use Argon2id with a random, per-transfer salt. The all-zero
/// salt keeps interoperability with pre-0.4.3 peers that used SHA-256.
pub fn derive_key(password: &str, salt: &[u8; 16]) -> [u8; 32] {
    let mut key = [0u8; 32];
    if *salt == [0; 16] {
        key.copy_from_slice(&Sha256::digest(password.as_bytes()));
    } else {
        argon2::Argon2::default()
            .hash_password_into(password.as_bytes(), salt, &mut key)
            .expect("fixed Argon2 parameters and salt length are valid");
    }
    key
}

/// Encrypt data with AES-256-GCM.
/// Returns: `[12-byte random nonce || ciphertext + 16-byte tag]`.
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);

    // Generate a random 12-byte nonce per chunk
    let mut nonce_bytes = [0u8; 12];
    rand::Rng::fill(&mut rand::thread_rng(), &mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .expect("AES-GCM encryption should not fail");

    // Prepend nonce to ciphertext
    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    result
}

/// Decrypt data encrypted with `encrypt`.
/// Input: `[12-byte nonce || ciphertext + 16-byte tag]`.
pub fn decrypt(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 12 + 16 {
        anyhow::bail!("encrypted data too short (need at least 28 bytes, got {})", data.len());
    }

    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);

    let (nonce_bytes, ciphertext) = data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("decryption failed — password may be incorrect"))?;

    Ok(plaintext)
}
