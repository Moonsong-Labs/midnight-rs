//! Password-based encryption for exports — PBKDF2-HMAC-SHA256 + AES-256-GCM,
//! wire-compatible with midnight-js's
//! [`level-private-state-provider`](https://github.com/midnightntwrk/midnight-js/blob/main/packages/level-private-state-provider/src/storage-encryption.ts).
//!
//! Envelope layout (binary, base64-encoded for the JSON wrapper):
//!
//! ```text
//! byte 0       version (== ENCRYPTION_VERSION_V2)
//! bytes 1..33  salt (32 bytes)
//! bytes 33..45 IV / nonce (12 bytes)
//! bytes 45..61 AES-GCM authentication tag (16 bytes)
//! bytes 61..   ciphertext
//! ```
//!
//! The salt embedded in the envelope is the same salt used for key derivation,
//! duplicated for the salt-mismatch sanity check on decrypt.

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use sha2::Sha256;

use crate::PrivateStateError;

pub(crate) const SALT_LEN: usize = 32;
const IV_LEN: usize = 12;
const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;

/// PBKDF2 iteration count. Matches midnight-js's `PBKDF2_ITERATIONS_V2` —
/// changing it is a wire-format break.
const PBKDF2_ITERATIONS: u32 = 600_000;

/// Current envelope version. Matches midnight-js's `ENCRYPTION_VERSION_V2`.
const ENCRYPTION_VERSION_V2: u8 = 2;

const HEADER_LEN: usize = 1 + SALT_LEN + IV_LEN + TAG_LEN;

fn derive_key(password: &[u8], salt: &[u8]) -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    pbkdf2_hmac::<Sha256>(password, salt, PBKDF2_ITERATIONS, &mut key);
    key
}

/// Encrypt `plaintext` under `password`, returning `(salt[32], base64(envelope))`.
///
/// A fresh random salt and IV are generated per call.
pub(crate) fn encrypt(
    password: &str,
    plaintext: &[u8],
) -> Result<([u8; SALT_LEN], String), PrivateStateError> {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);

    let key = derive_key(password.as_bytes(), &salt);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| PrivateStateError::KeyDerivation(e.to_string()))?;

    let mut iv = [0u8; IV_LEN];
    rand::thread_rng().fill_bytes(&mut iv);

    // AES-GCM's `encrypt` returns `ciphertext || tag` concatenated; split it back
    // out so we can place each in its own envelope slot, matching midnight-js's
    // `version || salt || iv || tag || ct` layout.
    let ct_with_tag = cipher
        .encrypt(
            aes_gcm::Nonce::from_slice(&iv),
            Payload {
                msg: plaintext,
                aad: &[],
            },
        )
        // AES-GCM encryption only fails if the plaintext exceeds the cipher's
        // length limit (~64 GiB); not reachable for our payloads.
        .map_err(|_| PrivateStateError::Encrypt("AES-GCM encryption failed".into()))?;

    let (ciphertext, tag) = ct_with_tag.split_at(ct_with_tag.len() - TAG_LEN);

    let mut envelope = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    envelope.push(ENCRYPTION_VERSION_V2);
    envelope.extend_from_slice(&salt);
    envelope.extend_from_slice(&iv);
    envelope.extend_from_slice(tag);
    envelope.extend_from_slice(ciphertext);

    Ok((salt, BASE64.encode(&envelope)))
}

/// Decrypt an envelope produced by [`encrypt`] under `password`. `expected_salt`
/// is the salt carried alongside the envelope in the outer JSON wrapper — its
/// hex value is verified against the salt embedded in the envelope, so a swap
/// of salt-vs-payload across two different exports surfaces as
/// [`PrivateStateError::InvalidFormat`] rather than a silent decrypt failure.
pub(crate) fn decrypt(
    password: &str,
    expected_salt: &[u8; SALT_LEN],
    envelope_b64: &str,
) -> Result<Vec<u8>, PrivateStateError> {
    let bytes = BASE64.decode(envelope_b64).map_err(|e| {
        PrivateStateError::InvalidFormat(format!("encryptedPayload is not base64: {e}"))
    })?;

    if bytes.len() < HEADER_LEN {
        return Err(PrivateStateError::InvalidFormat(format!(
            "encryptedPayload shorter than header ({} bytes minimum, got {})",
            HEADER_LEN,
            bytes.len()
        )));
    }

    let version = bytes[0];
    if version != ENCRYPTION_VERSION_V2 {
        return Err(PrivateStateError::InvalidFormat(format!(
            "unsupported envelope version {version}; expected {ENCRYPTION_VERSION_V2}"
        )));
    }

    let embedded_salt = &bytes[1..1 + SALT_LEN];
    if embedded_salt != expected_salt.as_slice() {
        return Err(PrivateStateError::InvalidFormat(
            "salt embedded in encryptedPayload does not match the wrapper salt".into(),
        ));
    }

    let iv = &bytes[1 + SALT_LEN..1 + SALT_LEN + IV_LEN];
    let tag = &bytes[1 + SALT_LEN + IV_LEN..HEADER_LEN];
    let ciphertext = &bytes[HEADER_LEN..];

    let key = derive_key(password.as_bytes(), expected_salt);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| PrivateStateError::KeyDerivation(e.to_string()))?;

    // AES-GCM's `decrypt` expects `ciphertext || tag`; reassemble.
    let mut ct_with_tag = Vec::with_capacity(ciphertext.len() + TAG_LEN);
    ct_with_tag.extend_from_slice(ciphertext);
    ct_with_tag.extend_from_slice(tag);

    cipher
        .decrypt(
            aes_gcm::Nonce::from_slice(iv),
            Payload {
                msg: &ct_with_tag,
                aad: &[],
            },
        )
        .map_err(|_| PrivateStateError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "correct horse battery staple xy";

    #[test]
    fn round_trip() {
        let (salt, env) = encrypt(PW, b"secret bytes").unwrap();
        let out = decrypt(PW, &salt, &env).unwrap();
        assert_eq!(out, b"secret bytes");
    }

    #[test]
    fn wrong_password_fails_authentication() {
        let (salt, env) = encrypt(PW, b"secret bytes").unwrap();
        let err = decrypt("wrong password entirely xy", &salt, &env).unwrap_err();
        assert!(matches!(err, PrivateStateError::Decrypt));
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let (salt, env) = encrypt(PW, b"secret bytes").unwrap();
        let mut bytes = BASE64.decode(&env).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        let tampered = BASE64.encode(&bytes);
        let err = decrypt(PW, &salt, &tampered).unwrap_err();
        assert!(matches!(err, PrivateStateError::Decrypt));
    }

    #[test]
    fn mismatched_outer_salt_is_invalid_format() {
        let (_salt, env) = encrypt(PW, b"secret bytes").unwrap();
        // Wrapper salt that doesn't match the envelope's embedded salt is a
        // malformed wrapper, not a key-derivation failure.
        let bad_salt = [0xAAu8; SALT_LEN];
        let err = decrypt(PW, &bad_salt, &env).unwrap_err();
        assert!(matches!(err, PrivateStateError::InvalidFormat(_)));
    }

    #[test]
    fn unsupported_version_byte_is_invalid_format() {
        let (salt, env) = encrypt(PW, b"secret bytes").unwrap();
        // Flip the version byte to V1 (1); we only support V2.
        let mut bytes = BASE64.decode(&env).unwrap();
        bytes[0] = 1;
        let tampered = BASE64.encode(&bytes);
        let err = decrypt(PW, &salt, &tampered).unwrap_err();
        assert!(matches!(err, PrivateStateError::InvalidFormat(_)));
    }

    #[test]
    fn fresh_salt_and_iv_per_call() {
        let (salt1, env1) = encrypt(PW, b"x").unwrap();
        let (salt2, env2) = encrypt(PW, b"x").unwrap();
        assert_ne!(salt1, salt2);
        assert_ne!(env1, env2);
    }
}
