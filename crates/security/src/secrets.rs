//! Encrypted secret storage format: base64(nonce || ChaCha20-Poly1305 ciphertext).
//! The secret name is bound as associated data so ciphertexts cannot be swapped between names.

use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use ma_types::{MaError, MaResult};

use crate::keys::Key;

pub fn encrypt(key: &Key, name: &str, plaintext: &str) -> MaResult<String> {
    let cipher = ChaCha20Poly1305::new_from_slice(&key.0).map_err(|_| MaError::internal("bad key length"))?;
    let mut nonce_bytes = [0u8; 12];
    getrandom::fill(&mut nonce_bytes).map_err(|e| MaError::internal(format!("rng: {e}")))?;
    let nonce = Nonce::try_from(&nonce_bytes[..]).map_err(|_| MaError::internal("nonce"))?;
    let ct = cipher
        .encrypt(&nonce, Payload { msg: plaintext.as_bytes(), aad: name.as_bytes() })
        .map_err(|_| MaError::internal("encryption failed"))?;
    let mut out = nonce_bytes.to_vec();
    out.extend_from_slice(&ct);
    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}

pub fn decrypt(key: &Key, name: &str, encoded: &str) -> MaResult<String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| MaError::config("corrupt secret encoding"))?;
    if raw.len() < 12 + 16 {
        return Err(MaError::config("corrupt secret"));
    }
    let cipher = ChaCha20Poly1305::new_from_slice(&key.0).map_err(|_| MaError::internal("bad key length"))?;
    let nonce = Nonce::try_from(&raw[..12]).map_err(|_| MaError::internal("nonce"))?;
    let pt = cipher
        .decrypt(&nonce, Payload { msg: &raw[12..], aad: name.as_bytes() })
        .map_err(|_| MaError::auth("secret could not be decrypted (wrong key or tampered data)"))?;
    String::from_utf8(pt).map_err(|_| MaError::config("secret is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_binding() {
        let key = Key([7u8; 32]);
        let enc = encrypt(&key, "db", "s3cret").unwrap();
        assert!(!enc.contains("s3cret"));
        assert_eq!(decrypt(&key, "db", &enc).unwrap(), "s3cret");
        // bound to the name
        assert!(decrypt(&key, "other", &enc).is_err());
        // wrong key
        assert!(decrypt(&Key([8u8; 32]), "db", &enc).is_err());
    }
}
