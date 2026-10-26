//! Settings that name groups or contacts, kept encrypted at rest.
//!
//! A group's id is its Orchard `ak`, and the group's full viewing key is
//! derived from `ak` alone. So anything keyed by group id in the plaintext
//! `settings.json` (start heights, address rotation, the active wallet) let
//! anyone who could read the app's folder see that treasury's whole history.
//! Contact nicknames keyed by public key leaked who the user transacts with.
//!
//! Those now live in `private-settings.enc`, sealed under a key derived from
//! the keystore, so they are readable only while it is unlocked, and wallet
//! folders are named by an id derived the same way ([`AppState::wallet_dir_id`]).
//! The first unlock with this version moves the old plaintext values over.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::state::{AppState, ReceiveState, WalletKind};

const FILE: &str = "private-settings.enc";
const KEY_INFO: &[u8] = b"private-settings-v1";
const LABEL: &[u8] = b"cyze-private-settings-v1";

/// Per-group and per-contact settings, encrypted at rest (see the module docs).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrivateSettings {
    /// Where each group's wallet starts scanning, per network, keyed by
    /// `state::wallet_session_key`. Recorded at key generation and wallet setup.
    #[serde(default)]
    pub network_birthdays: HashMap<String, u64>,
    /// Older per-group start heights (not per network), read as a fallback.
    #[serde(default)]
    pub wallet_birthdays: HashMap<String, u64>,
    /// When each group was created on this device (unix seconds).
    #[serde(default)]
    pub group_created_at: HashMap<String, u64>,
    /// Rotating receive-address state per group.
    #[serde(default)]
    pub receive_state: HashMap<String, ReceiveState>,
    /// The active wallet's group id.
    #[serde(default)]
    pub active_group_id: Option<String>,
    /// Local nicknames for contacts, keyed by hex comm pubkey.
    #[serde(default)]
    pub contact_aliases: HashMap<String, String>,
    /// Contacts whose safety code the user compared with them (hex comm pubkeys).
    #[serde(default)]
    pub verified_contacts: std::collections::BTreeSet<String>,
    /// Digest of the group ids the last backup held, so the app can tell when
    /// a newer group isn't in any backup yet. Not plaintext either: anyone who
    /// knows a group id could check it against the digest.
    #[serde(default)]
    pub backup_fingerprint: Option<String>,
    /// Groups whose payments this device approves by itself within the
    /// group's spending limit (`frost_app_core::rules::SpendingLimit`).
    #[serde(default)]
    pub auto_approve: std::collections::BTreeSet<String>,
}

impl AppState {
    async fn private_key(&self) -> AppResult<zeroize::Zeroizing<[u8; 32]>> {
        let guard = self.unlocked.read().await;
        let unlocked = guard.as_ref().ok_or_else(AppError::locked)?;
        Ok(unlocked.file.derive_subkey(KEY_INFO))
    }

    /// The private settings (empty before anything was saved). Needs the
    /// keystore unlocked.
    pub async fn load_private(&self) -> AppResult<PrivateSettings> {
        let key = self.private_key().await?;
        let path = self.data_dir.join(FILE);
        let Ok(sealed) = std::fs::read(&path) else {
            return Ok(PrivateSettings::default());
        };
        let plain = frost_app_core::sealed::open(&key, LABEL, &sealed)?;
        serde_json::from_slice(&plain).map_err(|e| AppError::new("config", format!("private settings: {e}")))
    }

    /// Change the private settings. Serialized, so concurrent updates (the sync
    /// service recording a height while the user renames a contact) don't lose
    /// each other's writes.
    pub async fn update_private<T>(&self, change: impl FnOnce(&mut PrivateSettings) -> T) -> AppResult<T> {
        let _serialized = self.private_lock.lock().await;
        let mut settings = self.load_private().await?;
        let out = change(&mut settings);
        let plain = zeroize::Zeroizing::new(
            serde_json::to_vec(&settings).map_err(|e| AppError::new("config", e.to_string()))?,
        );
        let sealed = frost_app_core::sealed::seal(&*self.private_key().await?, LABEL, &plain)?;
        frost_app_core::keystore::write_atomic(&self.data_dir.join(FILE), &sealed)?;
        Ok(out)
    }

    /// The folder name for a group's wallet: derived from the keystore, so the
    /// folder doesn't reveal the group id (which is enough to view the wallet).
    pub async fn wallet_dir_id(&self, group_id: &str) -> AppResult<String> {
        self.wallet_dir_id_of(group_id, WalletKind::Current).await
    }

    /// The folder name for one of a group's wallets (see [`wallet_dir_id`]).
    /// A group that made its viewing key private keeps its old wallet where it
    /// always was and starts the private one in a folder named after its seed,
    /// so a later new seed gets a fresh wallet too.
    ///
    /// [`wallet_dir_id`]: AppState::wallet_dir_id
    pub async fn wallet_dir_id_of(&self, group_id: &str, kind: WalletKind) -> AppResult<String> {
        let guard = self.unlocked.read().await;
        let unlocked = guard.as_ref().ok_or_else(AppError::locked)?;
        let info = match (kind, unlocked.extras.has_old_viewing_key(group_id)) {
            (WalletKind::Current, true) => {
                let seed = zeroize::Zeroizing::new(unlocked.extras.viewing_seed(group_id)?.unwrap_or_default());
                let ak = hex::decode(group_id).map_err(|e| AppError::new("config", e.to_string()))?;
                let tag = frost_app_core::viewing::digest(&ak, &seed);
                format!("wallet-dir-v1:{group_id}:{}", hex::encode(&tag[..8]))
            }
            _ => format!("wallet-dir-v1:{group_id}"),
        };
        let id = unlocked.file.derive_subkey(info.as_bytes());
        Ok(hex::encode(&id[..16]))
    }
}

/// After an unlock: move per-group values still in the plaintext settings into
/// the private store (first run of this version), and rename wallet folders
/// named by group id to their opaque ids, keeping their synced state and any
/// signed transaction still waiting to broadcast.
pub(crate) async fn migrate_plaintext(state: &AppState) {
    let mut settings = state.load_settings();
    let legacy = PrivateSettings {
        network_birthdays: std::mem::take(&mut settings.network_birthdays),
        wallet_birthdays: std::mem::take(&mut settings.wallet_birthdays),
        group_created_at: std::mem::take(&mut settings.group_created_at),
        receive_state: std::mem::take(&mut settings.receive_state),
        active_group_id: settings.active_group_id.take(),
        contact_aliases: std::mem::take(&mut settings.contact_aliases),
        verified_contacts: Default::default(),
        backup_fingerprint: settings.backup_fingerprint.take(),
        auto_approve: Default::default(),
    };
    let had_legacy = !(legacy.network_birthdays.is_empty()
        && legacy.wallet_birthdays.is_empty()
        && legacy.group_created_at.is_empty()
        && legacy.receive_state.is_empty()
        && legacy.active_group_id.is_none()
        && legacy.contact_aliases.is_empty()
        && legacy.backup_fingerprint.is_none());
    if had_legacy {
        let merged = state
            .update_private(|p| {
                p.network_birthdays.extend(legacy.network_birthdays);
                p.wallet_birthdays.extend(legacy.wallet_birthdays);
                p.group_created_at.extend(legacy.group_created_at);
                p.receive_state.extend(legacy.receive_state);
                p.active_group_id = p.active_group_id.take().or(legacy.active_group_id);
                p.contact_aliases.extend(legacy.contact_aliases);
                p.backup_fingerprint = p.backup_fingerprint.take().or(legacy.backup_fingerprint);
            })
            .await;
        match merged.and_then(|()| state.save_settings(&settings)) {
            Ok(()) => tracing::info!("moved per-group settings into encrypted storage"),
            Err(e) => {
                tracing::warn!("couldn't move per-group settings into encrypted storage: {}", e.message);
                return;
            }
        }
    }
    let groups: Vec<String> = state
        .with_config(|c| Ok(c.group.keys().cloned().collect()))
        .await
        .unwrap_or_default();
    for group_id in groups {
        let old = state.data_dir.join("wallets").join(&group_id);
        if !old.exists() {
            continue;
        }
        let Ok(wid) = state.wallet_dir_id(&group_id).await else {
            return;
        };
        let new = state.data_dir.join("wallets").join(wid);
        // A wallet already set up under the new name wins; the old one is a cache.
        let moved = if new.exists() { std::fs::remove_dir_all(&old) } else { std::fs::rename(&old, &new) };
        match moved {
            Ok(()) => tracing::info!("renamed a wallet folder named by group id"),
            Err(e) => tracing::warn!("couldn't rename an old wallet folder: {e}"),
        }
    }
}
