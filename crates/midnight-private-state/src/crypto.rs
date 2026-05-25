//! Password-based encryption for exports: Argon2id key derivation + AES-256-GCM.
//!
//! The on-disk/serialized form is `base64(nonce[12] || ciphertext)` plus a
//! hex-encoded 32-byte salt, both carried in [`EncryptedExport`](crate::EncryptedExport).

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit};
use argon2::Argon2;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use rand::RngCore;

use crate::PrivateStateError;

const SALT_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

fn derive_key(password: &[u8], salt: &[u8]) -> Result<[u8; KEY_LEN], PrivateStateError> {
    let mut key = [0u8; KEY_LEN];
    Argon2::default()
        .hash_password_into(password, salt, &mut key)
        .map_err(|e| PrivateStateError::KeyDerivation(e.to_string()))?;
    Ok(key)
}

/// Encrypt `plaintext` under `password`, returning `(salt_hex, ciphertext_base64)`.
///
/// A fresh random salt and nonce are generated per call, so encrypting the same
/// plaintext twice yields different envelopes.
pub(crate) fn encrypt(
    password: &str,
    plaintext: &[u8],
) -> Result<(String, String), PrivateStateError> {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);

    let key = derive_key(password.as_bytes(), &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| PrivateStateError::KeyDerivation(e.to_string()))?;

    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);

    let ciphertext = cipher
        .encrypt(aes_gcm::Nonce::from_slice(&nonce), plaintext)
        // AES-GCM encryption only fails if the plaintext exceeds the cipher's
        // length limit (~64 GiB); not reachable for our payloads.
        .map_err(|_| PrivateStateError::Serialize("AES-GCM encryption failed".into()))?;

    let mut combined = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    combined.extend_from_slice(&nonce);
    combined.extend_from_slice(&ciphertext);

    Ok((hex::encode(salt), BASE64.encode(&combined)))
}

/// Decrypt an envelope produced by [`encrypt`]. Returns
/// [`PrivateStateError::Decrypt`] on a wrong password or tampered ciphertext
/// (AES-GCM authentication failure).
pub(crate) fn decrypt(
    password: &str,
    salt_hex: &str,
    ciphertext_b64: &str,
) -> Result<Vec<u8>, PrivateStateError> {
    let salt = hex::decode(salt_hex)
        .map_err(|e| PrivateStateError::InvalidFormat(format!("salt is not valid hex: {e}")))?;

    let combined = BASE64
        .decode(ciphertext_b64)
        .map_err(|e| PrivateStateError::InvalidFormat(format!("ciphertext is not base64: {e}")))?;

    if combined.len() < NONCE_LEN {
        return Err(PrivateStateError::InvalidFormat(
            "ciphertext shorter than nonce".into(),
        ));
    }
    let (nonce, ciphertext) = combined.split_at(NONCE_LEN);

    let key = derive_key(password.as_bytes(), &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| PrivateStateError::KeyDerivation(e.to_string()))?;

    cipher
        .decrypt(aes_gcm::Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| PrivateStateError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let (salt, ct) = encrypt("correct horse battery staple", b"secret bytes").unwrap();
        let out = decrypt("correct horse battery staple", &salt, &ct).unwrap();
        assert_eq!(out, b"secret bytes");
    }

    #[test]
    fn wrong_password_fails_authentication() {
        let (salt, ct) = encrypt("correct horse battery staple", b"secret bytes").unwrap();
        let err = decrypt("wrong password entirely", &salt, &ct).unwrap_err();
        assert!(matches!(err, PrivateStateError::Decrypt));
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let (salt, ct) = encrypt("correct horse battery staple", b"secret bytes").unwrap();
        let mut bytes = BASE64.decode(&ct).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        let tampered = BASE64.encode(&bytes);
        let err = decrypt("correct horse battery staple", &salt, &tampered).unwrap_err();
        assert!(matches!(err, PrivateStateError::Decrypt));
    }

    #[test]
    fn fresh_salt_and_nonce_per_call() {
        let (salt1, ct1) = encrypt("correct horse battery staple", b"x").unwrap();
        let (salt2, ct2) = encrypt("correct horse battery staple", b"x").unwrap();
        assert_ne!(salt1, salt2);
        assert_ne!(ct1, ct2);
    }
}
