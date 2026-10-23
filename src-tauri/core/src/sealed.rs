//! Small authenticated-encryption helpers for app data kept next to the
//! keystore: per-group settings, UI history, backup metadata. Each store uses
//! its own key, derived from the keystore's data key
//! (`KeystoreFile::derive_subkey`), and its own label as associated data, so
//! one store's ciphertext can't be passed off as another's.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use zeroize::Zeroizing;

use crate::error::CoreError;

const NONCE_LEN: usize = 24;

/// Encrypt `plaintext` under `key`, binding `label`. Returns `nonce || ciphertext`.
pub fn seal(key: &[u8; 32], label: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, CoreError> {
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ciphertext = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| CoreError::Crypto(e.to_string()))?
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad: label })
        .map_err(|e| CoreError::Crypto(e.to_string()))?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt what [`seal`] produced with the same key and label.
pub fn open(key: &[u8; 32], label: &[u8], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, CoreError> {
    if sealed.len() < NONCE_LEN {
        return Err(CoreError::Crypto("sealed data is truncated".into()));
    }
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    XChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| CoreError::Crypto(e.to_string()))?
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ciphertext, aad: label })
        .map(Zeroizing::new)
        .map_err(|_| CoreError::Crypto("sealed data doesn't open with this key".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_binds_key_and_label() {
        let (k1, k2) = ([1u8; 32], [2u8; 32]);
        let sealed = seal(&k1, b"settings", b"secret").unwrap();
        assert_eq!(open(&k1, b"settings", &sealed).unwrap().as_slice(), b"secret");
        assert!(open(&k2, b"settings", &sealed).is_err(), "wrong key");
        assert!(open(&k1, b"history", &sealed).is_err(), "wrong label");
        assert!(open(&k1, b"settings", &sealed[..10]).is_err(), "truncated");
    }
}
