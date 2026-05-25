//! Password-based encryption for exports: Argon2id key derivation + AES-256-GCM.
//!
//! The on-disk/serialized form is `base64(nonce[12] || ciphertext)` plus a
//! hex-encoded 32-byte salt, both carried in [`EncryptedExport`](crate::EncryptedExport).

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use rand::RngCore;

use crate::PrivateStateError;

const SALT_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

// Pinned Argon2id parameters for the export format. They happen to match the
// `argon2` crate's current defaults, but are fixed here so an upstream default
// change can't make existing `-v1` exports undecryptable. Changing any of these
// is a format-breaking change and must bump the `FORMAT_*` tags.
const ARGON2_M_COST: u32 = 19_456; // KiB
const ARGON2_T_COST: u32 = 2;
const ARGON2_P_COST: u32 = 1;

fn derive_key(password: &[u8], salt: &[u8]) -> Result<[u8; KEY_LEN], PrivateStateError> {
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(KEY_LEN))
        .map_err(|e| PrivateStateError::KeyDerivation(e.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(password, salt, &mut key)
        .map_err(|e| PrivateStateError::KeyDerivation(e.to_string()))?;
    Ok(key)
}

/// Encrypt `plaintext` under `password`, returning `(salt_hex, ciphertext_base64)`.
///
/// `aad` is bound as AES-GCM additional authenticated data: it is not encrypted,
/// but decryption fails unless the same `aad` is supplied. Callers pass the
/// export's `format` so a tampered format tag cannot reinterpret the payload.
///
/// A fresh random salt and nonce are generated per call, so encrypting the same
/// plaintext twice yields different envelopes.
pub(crate) fn encrypt(
    password: &str,
    aad: &[u8],
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
        .encrypt(
            aes_gcm::Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        // AES-GCM encryption only fails if the plaintext exceeds the cipher's
        // length limit (~64 GiB); not reachable for our payloads.
        .map_err(|_| PrivateStateError::Encrypt("AES-GCM encryption failed".into()))?;

    let mut combined = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    combined.extend_from_slice(&nonce);
    combined.extend_from_slice(&ciphertext);

    Ok((hex::encode(salt), BASE64.encode(&combined)))
}

/// Decrypt an envelope produced by [`encrypt`]. Returns
/// [`PrivateStateError::Decrypt`] on a wrong password, a mismatched `aad`, or a
/// tampered ciphertext (all surface as an AES-GCM authentication failure).
pub(crate) fn decrypt(
    password: &str,
    aad: &[u8],
    salt_hex: &str,
    ciphertext_b64: &str,
) -> Result<Vec<u8>, PrivateStateError> {
    let salt = hex::decode(salt_hex)
        .map_err(|e| PrivateStateError::InvalidFormat(format!("salt is not valid hex: {e}")))?;
    if salt.len() != SALT_LEN {
        return Err(PrivateStateError::InvalidFormat(format!(
            "salt must be {SALT_LEN} bytes, got {}",
            salt.len()
        )));
    }

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
        .decrypt(
            aes_gcm::Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| PrivateStateError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "correct horse battery staple";

    #[test]
    fn round_trip() {
        let (salt, ct) = encrypt(PW, b"aad", b"secret bytes").unwrap();
        let out = decrypt(PW, b"aad", &salt, &ct).unwrap();
        assert_eq!(out, b"secret bytes");
    }

    #[test]
    fn wrong_password_fails_authentication() {
        let (salt, ct) = encrypt(PW, b"aad", b"secret bytes").unwrap();
        let err = decrypt("wrong password entirely", b"aad", &salt, &ct).unwrap_err();
        assert!(matches!(err, PrivateStateError::Decrypt));
    }

    #[test]
    fn mismatched_aad_fails_authentication() {
        // The aad (e.g. the export format tag) is authenticated: decrypting with
        // a different aad fails even with the correct password.
        let (salt, ct) = encrypt(PW, b"states", b"secret bytes").unwrap();
        let err = decrypt(PW, b"signing-keys", &salt, &ct).unwrap_err();
        assert!(matches!(err, PrivateStateError::Decrypt));
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let (salt, ct) = encrypt(PW, b"aad", b"secret bytes").unwrap();
        let mut bytes = BASE64.decode(&ct).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        let tampered = BASE64.encode(&bytes);
        let err = decrypt(PW, b"aad", &salt, &tampered).unwrap_err();
        assert!(matches!(err, PrivateStateError::Decrypt));
    }

    #[test]
    fn wrong_salt_length_is_invalid_format() {
        let (_salt, ct) = encrypt(PW, b"aad", b"secret bytes").unwrap();
        // A salt that isn't SALT_LEN bytes is a malformed envelope, not a
        // key-derivation failure.
        let err = decrypt(PW, b"aad", "00ff", &ct).unwrap_err();
        assert!(matches!(err, PrivateStateError::InvalidFormat(_)));
    }

    #[test]
    fn fresh_salt_and_nonce_per_call() {
        let (salt1, ct1) = encrypt(PW, b"aad", b"x").unwrap();
        let (salt2, ct2) = encrypt(PW, b"aad", b"x").unwrap();
        assert_ne!(salt1, salt2);
        assert_ne!(ct1, ct2);
    }
}
