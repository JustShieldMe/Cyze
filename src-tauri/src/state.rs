use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};

use frost_app_core::keystore::Keystore;
use frost_client::cli::config::Config;
use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

/// Current time in epoch milliseconds (monotonic-ish wall clock; only used for
/// idle-duration comparisons where small clock jumps are harmless).
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Non-secret app settings, stored as plaintext JSON so they are readable
/// before the keystore is unlocked.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Display name for the local user, shown wherever this identity appears
    /// (own participant entries in groups, signer lists, etc.).
    #[serde(default)]
    pub username: Option<String>,
    /// Last external server the user connected to, `host:port`.
    pub server_url: Option<String>,
    /// Port for the embedded frostd sidecar.
    pub sidecar_port: Option<u16>,
    /// When true, the embedded frostd binds `0.0.0.0` (reachable from the LAN).
    /// Defaults to false: bind loopback (`127.0.0.1`) only, so the server is not
    /// exposed to the local network unless the user explicitly opts in.
    #[serde(default)]
    pub sidecar_bind_lan: Option<bool>,
    /// PEM certs trusted for specific external servers, keyed by `host:port`.
    #[serde(default)]
    pub trusted_certs: HashMap<String, String>,
    /// Legacy: moved to the encrypted private settings (`private.rs`). Read once
    /// on unlock to migrate, and no longer written here.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub contact_aliases: HashMap<String, String>,
    /// Zcash network for wallet features: "main" (default), "test", or
    /// "regtest" (a local devnet).
    #[serde(default)]
    pub wallet_network: Option<String>,
    /// Regtest only: the upgrade level the user forced ("nu6" … "nu6_3").
    /// `None` means auto — use [`Settings::regtest_detected`].
    #[serde(default)]
    pub regtest_upgrade: Option<String>,
    /// Regtest only: the upgrade level last read from the devnet node's
    /// consensus branch id (recorded by the connection test).
    #[serde(default)]
    pub regtest_detected: Option<String>,
    /// lightwalletd endpoint for the selected network.
    #[serde(default)]
    pub lightwalletd_url: Option<String>,
    /// Route wallet traffic (syncing, sending) through Tor
    /// (`frost_app_core::wallet::tor`).
    #[serde(default)]
    pub use_tor: Option<bool>,
    /// Idle minutes before the keystore auto-locks. `None` uses the default
    /// (10 min); `Some(0)` disables auto-lock entirely.
    #[serde(default)]
    pub auto_lock_minutes: Option<u64>,
    /// Legacy: moved to the encrypted private settings (`private.rs`). Read once
    /// on unlock to migrate, and no longer written here.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub receive_state: HashMap<String, ReceiveState>,
    /// Legacy: moved to the encrypted private settings (`private.rs`). Read once
    /// on unlock to migrate, and no longer written here.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub wallet_birthdays: HashMap<String, u64>,
    /// Legacy: moved to the encrypted private settings (`private.rs`). Read once
    /// on unlock to migrate, and no longer written here.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub network_birthdays: HashMap<String, u64>,
    /// Legacy: moved to the encrypted private settings (`private.rs`). Read once
    /// on unlock to migrate, and no longer written here.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub group_created_at: HashMap<String, u64>,
    /// Blocks downloaded and scanned per sync batch. `None` uses the core
    /// default; larger values reduce per-batch overhead on slow links at the
    /// cost of memory. Clamped by the core to a sane range.
    #[serde(default)]
    pub sync_batch_size: Option<u32>,
    /// Active session profile: "coordinator" or "participant". Toggled by the
    /// sidebar profile switch; drives which saved configuration is in use.
    #[serde(default)]
    pub session_role: Option<String>,
    /// How a coordinator exposes the embedded server: "direct", "tunnel", or
    /// "nginx". Saved so it can be reused on future launches.
    #[serde(default)]
    pub coordinator_exposure: Option<String>,
    /// True once the user has completed the first-run Session Configuration, so
    /// they aren't prompted again unless they revisit it.
    #[serde(default)]
    pub session_configured: Option<bool>,
    /// Legacy: moved to the encrypted private settings (`private.rs`). Read once
    /// on unlock to migrate, and no longer written here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_group_id: Option<String>,
    /// Show developer and expert options: test networks and the local devnet,
    /// the diagnostics log, the Ed25519 ciphersuite, raw-message signing, and
    /// coinholder voting. Off by default so the everyday screens stay focused
    /// on the mainnet wallet.
    #[serde(default)]
    pub advanced_mode: Option<bool>,
    /// When the keystore was last backed up (unix seconds).
    #[serde(default)]
    pub last_backup_at: Option<u64>,
    /// Legacy: moved to the encrypted private settings (`private.rs`). Read once
    /// on unlock to migrate, and no longer written here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_fingerprint: Option<String>,
}

/// Per-group rotating receive-address bookkeeping (#3). Non-secret; the actual
/// address is derived on demand from the group's public `ak` at `index`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceiveState {
    /// Diversifier index of the address currently handed out for this group.
    pub index: u32,
    /// Orchard received-note count at the moment `index` was issued. When the
    /// live count exceeds this, the shown address may have been paid and is
    /// rotated to the next index.
    pub baseline_notes: u64,
}

/// Keystore contents held in memory while unlocked. The unlocked
/// [`KeystoreFile`] carries the data-encryption key and key slots, so config
/// mutations re-encrypt transparently without rotating the DEK or invalidating
/// the recovery slot.
pub struct UnlockedState {
    pub config: Config,
    /// App data kept in the keystore beside the config (viewing seeds).
    pub extras: frost_app_core::config::KeystoreExtras,
    pub file: frost_app_core::keystore::KeystoreFile,
}

impl UnlockedState {
    /// Read the keystore's plaintext.
    pub fn from_plaintext(plaintext: &str, file: frost_app_core::keystore::KeystoreFile) -> AppResult<Self> {
        let (config, extras) = frost_app_core::config::parse_keystore(plaintext)?;
        Ok(Self { config, extras, file })
    }

    /// The keystore's plaintext, for saving.
    pub fn plaintext(&self) -> AppResult<zeroize::Zeroizing<String>> {
        Ok(frost_app_core::config::serialize_keystore(&self.config, &self.extras)?)
    }
}

/// Handle to a running ceremony task (DKG or signing).
pub struct CeremonyHandle {
    pub cancel: CancellationToken,
    /// Present for participant signing ceremonies that are paused at the
    /// approval gate; resolving it releases the round-2 signature share.
    pub approval: Option<oneshot::Sender<bool>>,
    /// Whether an approval may be given (see `commands::signing`).
    pub gate: ApprovalGate,
}

/// Whether a participant's "approve" is accepted for a signing ceremony.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ApprovalGate {
    /// Nothing to verify: DKG and coordinator ceremonies, non-wallet groups,
    /// and raw-message signing when advanced options are on.
    #[default]
    Open,
    /// A wallet signing request whose transaction hasn't been verified yet.
    Pending,
    /// The transaction was verified. Approving it records the sighash so the
    /// same transaction's other per-note rounds are approved automatically.
    Verified { sighash_hex: String },
    /// Verification failed and the request was refused, for this reason.
    Refused(String),
}

pub struct AppState {
    pub data_dir: PathBuf,
    pub unlocked: RwLock<Option<UnlockedState>>,
    pub ceremonies: Mutex<HashMap<Uuid, CeremonyHandle>>,
    pub sidecar: Mutex<Option<crate::sidecar::SidecarHandle>>,
    /// Optional Cloudflare quick tunnel exposing the embedded server publicly.
    pub tunnel: Mutex<Option<crate::tunnel::TunnelHandle>>,
    /// Optional Tailscale `serve` mapping exposing the embedded server over the
    /// coordinator's tailnet with a stable MagicDNS URL.
    pub tailscale: Mutex<Option<crate::tailscale::TailscaleHandle>>,
    /// Cancellation token for the in-flight wallet sync of each group, so a
    /// "Sync Now" can abandon a stalled sync and restart it cleanly.
    pub sync_cancels: Mutex<HashMap<String, CancellationToken>>,
    /// App-wide "one wallet syncs at a time" gate. Scanning is CPU-bound, so two
    /// groups syncing at once only thrash. Every sync
    /// (`commands::wallet::sync_once`) holds this for its whole run; combined
    /// with cancelling the previous group's sync on an active-wallet switch, it
    /// keeps the app's processing focused on a single wallet.
    ///
    /// This global gate also subsumes the earlier per-group sync lock: since only
    /// one sync ever runs, a restarting sync can no longer race a cancelled one's
    /// still-open db connection, so the "database is locked" fix comes for free.
    pub sync_gate: Mutex<()>,
    /// The lightwalletd connection syncs reuse, so a re-sync every few seconds
    /// doesn't pay a fresh TCP + TLS handshake each time. Replaced when the
    /// configured endpoint changes; dropped after a connection error so the
    /// next sync reconnects.
    pub lightwalletd: Mutex<Option<frost_app_core::wallet::Lightwalletd>>,
    /// When each wallet (keyed by [`wallet_session_key`]) last fetched its
    /// note-commitment subtree roots this session. Roots change rarely, so syncs
    /// skip the fetch until [`SUBTREE_ROOTS_REFRESH`] has passed.
    pub subtree_roots_fetched: Mutex<HashMap<String, std::time::Instant>>,
    /// Epoch-millis of the last user activity, used to drive the idle auto-lock.
    pub last_activity: AtomicI64,
    /// Broadcasts whether the keystore is unlocked, so background work (the
    /// wallet sync service) can start on unlock and stop on lock. Updated only
    /// through [`AppState::set_unlocked`].
    unlocked_tx: tokio::sync::watch::Sender<bool>,
    /// Background wallet sync: wake-ups and the latest status per group.
    pub sync_service: crate::sync_service::SyncService,
    /// Sighashes of transactions this user approved, and when. A multi-note
    /// send signs each note in its own round; once one round of a verified
    /// transaction is approved, its other rounds are approved automatically.
    pub approved_sighashes: Mutex<HashMap<String, std::time::Instant>>,
    /// Sessions this app ran key generation in. The organizer keeps one open
    /// briefly after it finishes, and it must not look like a signing request.
    pub dkg_sessions: std::sync::Mutex<std::collections::HashSet<Uuid>>,
    /// Each group's run of agreeing on a private viewing key, while it runs
    /// (or after it failed, until dismissed).
    pub viewing_key_runs: std::sync::Mutex<HashMap<String, crate::commands::viewing::ViewingKeyRun>>,
    /// Viewing-key sessions this app took part in. The member who started one
    /// keeps it open briefly after it ends; it mustn't be joined again.
    pub viewing_key_sessions: std::sync::Mutex<std::collections::HashSet<Uuid>>,
    /// The sync server picked by itself for each network (keyed by its
    /// `as_str`), when the user hasn't chosen one. See
    /// `commands::wallet::pick_server`.
    pub auto_lightwalletd: std::sync::Mutex<HashMap<&'static str, frost_app_core::wallet::ServerProbe>>,
    /// Serializes read-modify-write cycles of the private settings.
    pub(crate) private_lock: Mutex<()>,
}

/// How long a sync may go without re-fetching subtree roots. A pool gains one
/// root per 2^16 notes (days apart on mainnet); hourly keeps them current at
/// negligible cost.
pub const SUBTREE_ROOTS_REFRESH: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Session-cache key for one group's wallet on one network.
pub fn wallet_session_key(group_id: &str, network: &str) -> String {
    format!("{network}/{group_id}")
}

/// Which of a group's wallets. A group has one, its current wallet, except
/// after it made its viewing key private: then the wallet under the key
/// derived from its id is kept too, as `Old`, so funds still at the old
/// address show up and can be moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletKind {
    #[default]
    Current,
    Old,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let data_dir = std::env::var("FROST_APP_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::data_local_dir()
                    .expect("no local data dir on this platform")
                    .join("frost-app")
            });
        Self::with_dir(data_dir)
    }

    pub fn with_dir(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            unlocked: RwLock::new(None),
            ceremonies: Mutex::new(HashMap::new()),
            sidecar: Mutex::new(None),
            tunnel: Mutex::new(None),
            tailscale: Mutex::new(None),
            sync_cancels: Mutex::new(HashMap::new()),
            sync_gate: Mutex::new(()),
            lightwalletd: Mutex::new(None),
            subtree_roots_fetched: Mutex::new(HashMap::new()),
            last_activity: AtomicI64::new(now_millis()),
            unlocked_tx: tokio::sync::watch::channel(false).0,
            sync_service: crate::sync_service::SyncService::default(),
            approved_sighashes: Mutex::new(HashMap::new()),
            dkg_sessions: Default::default(),
            viewing_key_runs: Default::default(),
            viewing_key_sessions: Default::default(),
            auto_lightwalletd: Default::default(),
            private_lock: Mutex::new(()),
        }
    }

    /// Replace the unlocked keystore contents (`None` locks) and tell anything
    /// watching. Every lock and unlock must go through here.
    pub async fn set_unlocked(&self, value: Option<UnlockedState>) {
        let unlocked = value.is_some();
        *self.unlocked.write().await = value;
        if unlocked {
            // Bring this device's data up to date before the sync service or
            // the UI reads it: metadata from a restored backup, and per-group
            // settings an older version kept in plaintext.
            crate::commands::backup::apply_pending_restore(self).await;
            crate::private::migrate_plaintext(self).await;
        } else if let Ok(mut runs) = self.viewing_key_runs.lock() {
            // A run can't save its result into a locked keystore.
            runs.drain().for_each(|(_, run)| run.cancel.cancel());
        }
        self.unlocked_tx.send_replace(unlocked);
    }

    /// Follow lock state changes (`true` = unlocked).
    pub fn watch_unlocked(&self) -> tokio::sync::watch::Receiver<bool> {
        self.unlocked_tx.subscribe()
    }

    /// Record user activity now, deferring the idle auto-lock.
    pub fn touch_activity(&self) {
        self.last_activity.store(now_millis(), Ordering::Relaxed);
    }

    /// Milliseconds since the last recorded activity.
    pub fn idle_millis(&self) -> i64 {
        now_millis().saturating_sub(self.last_activity.load(Ordering::Relaxed))
    }

    pub fn keystore(&self) -> Keystore {
        Keystore::new(self.data_dir.join("keystore.frost"))
    }

    pub fn settings_path(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    pub fn load_settings(&self) -> Settings {
        std::fs::read_to_string(self.settings_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &Settings) -> AppResult<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        // Lock the app data directory to the owner so sibling files (keystore,
        // wallet dbs, settings) are not exposed to other local users.
        let _ = frost_app_core::keystore::restrict_dir_to_owner(&self.data_dir);
        let path = self.settings_path();
        std::fs::write(
            &path,
            serde_json::to_string_pretty(settings)
                .map_err(|e| AppError::new("config", e.to_string()))?,
        )?;
        let _ = frost_app_core::keystore::restrict_to_owner(&path);
        Ok(())
    }

    /// Run `f` against the unlocked config, then re-encrypt and persist it.
    pub async fn mutate_config<T>(
        &self,
        f: impl FnOnce(&mut Config) -> AppResult<T>,
    ) -> AppResult<T> {
        self.mutate_keystore(|config, _| f(config)).await
    }

    /// Change the config and the keystore's extras together, in one save.
    pub async fn mutate_keystore<T>(
        &self,
        f: impl FnOnce(&mut Config, &mut frost_app_core::config::KeystoreExtras) -> AppResult<T>,
    ) -> AppResult<T> {
        let mut guard = self.unlocked.write().await;
        let unlocked = guard.as_mut().ok_or_else(AppError::locked)?;
        let result = f(&mut unlocked.config, &mut unlocked.extras)?;
        self.keystore()
            .save_file(&unlocked.file, unlocked.plaintext()?.as_bytes())
            .map_err(AppError::from)?;
        Ok(result)
    }

    /// The viewing seed a group's members agreed on, if it has one (groups
    /// without one derive their viewing key from the group key alone).
    pub async fn viewing_seed(&self, group_id: &str) -> AppResult<Option<zeroize::Zeroizing<[u8; 32]>>> {
        let guard = self.unlocked.read().await;
        let unlocked = guard.as_ref().ok_or_else(AppError::locked)?;
        Ok(unlocked.extras.viewing_seed(group_id)?.map(zeroize::Zeroizing::new))
    }

    /// Whether a group made its viewing key private after it was created, so
    /// it also has an old wallet (see [`WalletKind`]).
    pub async fn has_old_viewing_key(&self, group_id: &str) -> AppResult<bool> {
        let guard = self.unlocked.read().await;
        let unlocked = guard.as_ref().ok_or_else(AppError::locked)?;
        Ok(unlocked.extras.has_old_viewing_key(group_id))
    }

    /// Read-only access to the unlocked config.
    pub async fn with_config<T>(
        &self,
        f: impl FnOnce(&Config) -> AppResult<T>,
    ) -> AppResult<T> {
        let guard = self.unlocked.read().await;
        let unlocked = guard.as_ref().ok_or_else(AppError::locked)?;
        f(&unlocked.config)
    }

    /// Derive the SQLCipher key for a group's wallet database from the unlocked
    /// keystore DEK. Deterministic per (keystore, group), so the same key is
    /// recovered on every unlock; requires the keystore to be unlocked.
    pub async fn wallet_db_key(
        &self,
        group_id: &str,
    ) -> AppResult<zeroize::Zeroizing<[u8; 32]>> {
        let guard = self.unlocked.read().await;
        let unlocked = guard.as_ref().ok_or_else(AppError::locked)?;
        let info = format!("wallet-db:{group_id}");
        Ok(unlocked.file.derive_subkey(info.as_bytes()))
    }

    /// Key for a group's signed transactions awaiting broadcast.
    pub async fn pending_tx_key(&self, group_id: &str) -> AppResult<zeroize::Zeroizing<[u8; 32]>> {
        let guard = self.unlocked.read().await;
        let unlocked = guard.as_ref().ok_or_else(AppError::locked)?;
        Ok(unlocked.file.derive_subkey(format!("pending-tx-v1:{group_id}").as_bytes()))
    }
}
