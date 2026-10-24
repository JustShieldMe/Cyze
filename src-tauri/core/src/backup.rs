//! Encrypted backups of a member's keystore.
//!
//! A backup restores a member's key shares on another device, together with
//! their passphrase or recovery code (either one unlocks it, as on the original
//! device). Neither half is enough alone: the file is encrypted, and the
//! recovery code only unlocks a keystore file.
//!
//! The file holds:
//! - the keystore exactly as it is on disk, already encrypted under its
//!   passphrase and recovery-code slots;
//! - app metadata the shares can't rebuild, chiefly where each group's wallet
//!   starts scanning. Without it a restored wallet would start at the chain tip
//!   and miss every note received earlier. It is encrypted under a key derived
//!   from the keystore's data key, because it names groups by id, and a group's
//!   id is enough to view its wallet.
//!
//! The JSON envelope around them carries nothing secret.

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::CoreError;
use crate::keystore::{is_keystore, KeystoreFile};

/// File extension for backups.
pub const EXTENSION: &str = "cyzebackup";

const FORMAT: &str = "cyze-backup";
const VERSION: u32 = 1;
/// `KeystoreFile::derive_subkey` label for the metadata key.
const META_KEY_INFO: &[u8] = b"backup-meta-v1";
const META_AAD: &[u8] = b"cyze-backup-meta-v1";
const NONCE_LEN: usize = 24;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Serialize, Deserialize)]
struct Envelope {
    format: String,
    version: u32,
    /// When the backup was made (unix seconds).
    created_at: u64,
    /// The encrypted keystore file, base64.
    keystore: String,
    /// `nonce || ciphertext` of the metadata, base64.
    meta: String,
}

/// A parsed backup, before it is unlocked.
pub struct Backup {
    /// The encrypted keystore file, ready to install.
    pub keystore: Vec<u8>,
    /// The encrypted metadata; open it with [`open_meta`] once unlocked.
    pub sealed_meta: Vec<u8>,
    pub created_at: u64,
}

/// Build a backup from the keystore file's bytes, the unlocked keystore they
/// belong to (for the metadata key), and the app's metadata.
pub fn create(
    keystore_file: &[u8],
    unlocked: &KeystoreFile,
    meta: &[u8],
    created_at: u64,
) -> Result<Vec<u8>, CoreError> {
    if !is_keystore(keystore_file) {
        return Err(CoreError::MalformedKeystore("not a Cyze keystore".into()));
    }
    let key = unlocked.derive_subkey(META_KEY_INFO);
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ciphertext = XChaCha20Poly1305::new_from_slice(key.as_ref())
        .map_err(|e| CoreError::Crypto(e.to_string()))?
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: meta, aad: META_AAD })
        .map_err(|e| CoreError::Crypto(e.to_string()))?;
    let mut sealed = nonce.to_vec();
    sealed.extend_from_slice(&ciphertext);
    let envelope = Envelope {
        format: FORMAT.into(),
        version: VERSION,
        created_at,
        keystore: B64.encode(keystore_file),
        meta: B64.encode(sealed),
    };
    serde_json::to_vec_pretty(&envelope).map_err(|e| CoreError::Config(e.to_string()))
}

/// A digest of the groups a backup holds, to tell later whether a newer group
/// is missing from every backup. Stored instead of the ids themselves: a
/// group's id is enough to view its wallet.
pub fn groups_fingerprint(group_ids: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut ids: Vec<&String> = group_ids.iter().collect();
    ids.sort();
    let mut h = Sha256::new();
    h.update(b"cyze-backup-groups-v1");
    for id in ids {
        h.update((id.len() as u64).to_le_bytes());
        h.update(id.as_bytes());
    }
    hex::encode(h.finalize())
}

/// Parse a backup file.
pub fn parse(bytes: &[u8]) -> Result<Backup, CoreError> {
    let bad = |why: &str| CoreError::Config(format!("not a Cyze backup: {why}"));
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| bad("unreadable"))?;
    if envelope.format != FORMAT {
        return Err(bad("unknown format"));
    }
    if envelope.version != VERSION {
        return Err(CoreError::Config(format!(
            "this backup was made by a newer version of Cyze (format {}); update the app to restore it",
            envelope.version
        )));
    }
    let keystore = B64.decode(&envelope.keystore).map_err(|_| bad("damaged keystore"))?;
    if !is_keystore(&keystore) {
        return Err(bad("damaged keystore"));
    }
    let sealed_meta = B64.decode(&envelope.meta).map_err(|_| bad("damaged metadata"))?;
    Ok(Backup { keystore, sealed_meta, created_at: envelope.created_at })
}

/// Decrypt a backup's metadata with the keystore it was restored with.
pub fn open_meta(unlocked: &KeystoreFile, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, CoreError> {
    if sealed.len() < NONCE_LEN {
        return Err(CoreError::Config("backup metadata is damaged".into()));
    }
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    let key = unlocked.derive_subkey(META_KEY_INFO);
    XChaCha20Poly1305::new_from_slice(key.as_ref())
        .map_err(|e| CoreError::Crypto(e.to_string()))?
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ciphertext, aad: META_AAD })
        .map(Zeroizing::new)
        .map_err(|_| CoreError::Config("backup metadata doesn't belong to this keystore".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keystore::Keystore;

    #[test]
    fn a_backup_restores_the_keystore_and_its_metadata_on_another_device() {
        let dir = tempfile::tempdir().unwrap();
        let original = Keystore::new(dir.path().join("a.frost"));
        let (phrase, file) = original.create(b"shares", "pw").unwrap();
        let backup = create(&original.read_encrypted().unwrap(), &file, br#"{"b":1}"#, 1_700_000_000).unwrap();

        let text = String::from_utf8(backup.clone()).unwrap();
        assert!(!text.contains("\"b\""), "metadata must be encrypted");

        let parsed = parse(&backup).unwrap();
        assert_eq!(parsed.created_at, 1_700_000_000);
        let restored = Keystore::new(dir.path().join("b.frost"));
        restored.install(&parsed.keystore).unwrap();
        // Either secret works on the new device, as on the original.
        let (by_recovery, shares) = restored.unlock_with_recovery(&phrase).unwrap();
        assert_eq!(shares.as_slice(), b"shares");
        assert_eq!(open_meta(&by_recovery, &parsed.sealed_meta).unwrap().as_slice(), br#"{"b":1}"#);
        let (by_passphrase, _) = restored.unlock("pw").unwrap();
        assert!(open_meta(&by_passphrase, &parsed.sealed_meta).is_ok());
    }

    #[test]
    fn metadata_opens_only_with_its_own_keystore() {
        let dir = tempfile::tempdir().unwrap();
        let a = Keystore::new(dir.path().join("a.frost"));
        let (_, file_a) = a.create(b"a", "pw").unwrap();
        let b = Keystore::new(dir.path().join("b.frost"));
        let (_, file_b) = b.create(b"b", "pw").unwrap();
        let backup = parse(&create(&a.read_encrypted().unwrap(), &file_a, b"meta", 0).unwrap()).unwrap();
        assert!(open_meta(&file_b, &backup.sealed_meta).is_err());
    }

    #[test]
    fn group_fingerprints_ignore_order_and_see_new_groups() {
        let ab = groups_fingerprint(&["a".into(), "b".into()]);
        assert_eq!(ab, groups_fingerprint(&["b".into(), "a".into()]));
        assert_ne!(ab, groups_fingerprint(&["a".into(), "b".into(), "c".into()]));
        assert_eq!(ab.len(), 64);
    }

    #[test]
    fn rejects_files_that_arent_backups() {
        assert!(parse(b"hello").is_err());
        assert!(parse(br#"{"format":"other","version":1,"created_at":0,"keystore":"","meta":""}"#).is_err());
        let newer = br#"{"format":"cyze-backup","version":9,"created_at":0,"keystore":"","meta":""}"#;
        assert!(parse(newer).err().unwrap().to_string().contains("newer version"));
    }
}
