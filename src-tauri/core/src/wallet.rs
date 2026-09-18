//! Zcash light-client wallet (feature `wallet`).
//!
//! Phase 5.1, layer 1: lightwalletd connectivity. Connects to a configurable
//! lightwalletd endpoint over gRPC/TLS and fetches chain info — the foundation
//! the per-group sync + balance build on. Network is selectable (testnet for
//! testing, mainnet once the pipeline is complete).
//!
//! Compact-block sync, account import, and balance reads layer on top of this
//! `CompactTxStreamerClient` in the next increment.

use serde::{Deserialize, Serialize};
use tonic::transport::{Channel, ClientTlsConfig};
use zcash_client_backend::proto::service::{
    compact_tx_streamer_client::CompactTxStreamerClient, Empty,
};
use zcash_protocol::consensus::{BlockHeight, Network, NetworkType, NetworkUpgrade, Parameters};
use zcash_protocol::local_consensus::LocalNetwork;

use crate::error::CoreError;

/// Which Zcash network the wallet operates on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WalletNetwork {
    Test,
    Main,
    /// A local regtest devnet (e.g. thus-spoke-zakura). Regtest chains choose
    /// their own upgrade heights, so the network carries the latest upgrade the
    /// node has active; see [`RegtestUpgrade`].
    Regtest(RegtestUpgrade),
}

/// The newest network upgrade active on a local regtest devnet, from genesis.
///
/// Regtest has no fixed activation table: each node picks its own `nuparams`.
/// Devnets activate everything up to some upgrade at height 1, so "which
/// upgrade is the newest" is enough to rebuild the node's consensus parameters.
/// The wallet must agree with the node on this: it picks the consensus branch
/// id and the Orchard circuit a transaction is built with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegtestUpgrade {
    /// NU6 — what thus-spoke-zakura ships by default.
    #[default]
    Nu6,
    Nu6_1,
    Nu6_2,
    /// NU6.3 — Ironwood.
    Nu6_3,
}

impl RegtestUpgrade {
    pub const ALL: [RegtestUpgrade; 4] = [
        RegtestUpgrade::Nu6,
        RegtestUpgrade::Nu6_1,
        RegtestUpgrade::Nu6_2,
        RegtestUpgrade::Nu6_3,
    ];

    /// The settings/IPC spelling (`nu6`, `nu6_1`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            RegtestUpgrade::Nu6 => "nu6",
            RegtestUpgrade::Nu6_1 => "nu6_1",
            RegtestUpgrade::Nu6_2 => "nu6_2",
            RegtestUpgrade::Nu6_3 => "nu6_3",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|u| u.as_str() == s)
    }

    /// The upgrade whose consensus branch id a node reports (lowercase hex, as
    /// in [`LightwalletdInfo::consensus_branch_id`]). `None` for a branch older
    /// than NU6 or one this build doesn't know.
    pub fn from_branch_id(branch_hex: &str) -> Option<Self> {
        match branch_hex.trim().to_ascii_lowercase().as_str() {
            "c8e71055" => Some(RegtestUpgrade::Nu6),
            "4dec4df0" => Some(RegtestUpgrade::Nu6_1),
            "5437f330" => Some(RegtestUpgrade::Nu6_2),
            NU6_3_BRANCH_ID => Some(RegtestUpgrade::Nu6_3),
            _ => None,
        }
    }

    /// Regtest consensus parameters: every upgrade up to and including this one
    /// active at height 1, everything later inactive.
    pub fn local_network(self) -> LocalNetwork {
        let one = Some(BlockHeight::from_u32(1));
        let upto = |u: RegtestUpgrade| if self >= u { one } else { None };
        LocalNetwork {
            overwinter: one,
            sapling: one,
            blossom: one,
            heartwood: one,
            canopy: one,
            nu5: one,
            nu6: one,
            nu6_1: upto(RegtestUpgrade::Nu6_1),
            nu6_2: upto(RegtestUpgrade::Nu6_2),
            nu6_3: upto(RegtestUpgrade::Nu6_3),
            #[cfg(zcash_unstable = "nu7")]
            nu7: None,
        }
    }
}

/// Consensus parameters for any [`WalletNetwork`]: the fixed public networks or
/// a local regtest chain. The single `Parameters` type the wallet db, sync, and
/// tx builder are instantiated with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletParams {
    Consensus(Network),
    Local(LocalNetwork),
}

impl Parameters for WalletParams {
    fn network_type(&self) -> NetworkType {
        match self {
            WalletParams::Consensus(n) => n.network_type(),
            WalletParams::Local(n) => n.network_type(),
        }
    }

    fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
        match self {
            WalletParams::Consensus(n) => n.activation_height(nu),
            WalletParams::Local(n) => n.activation_height(nu),
        }
    }
}

impl WalletNetwork {
    /// Parse the settings spelling. A regtest network parsed this way assumes
    /// the default upgrade level; callers that know the node's level construct
    /// `WalletNetwork::Regtest(level)` directly.
    pub fn from_str(s: &str) -> Self {
        match s {
            "main" => WalletNetwork::Main,
            "regtest" => WalletNetwork::Regtest(RegtestUpgrade::default()),
            _ => WalletNetwork::Test,
        }
    }

    /// The settings/IPC spelling: `main`, `test`, or `regtest`.
    pub fn as_str(self) -> &'static str {
        match self {
            WalletNetwork::Main => "main",
            WalletNetwork::Test => "test",
            WalletNetwork::Regtest(_) => "regtest",
        }
    }

    /// The consensus parameters for this network (used by sync/address logic).
    pub fn params(self) -> WalletParams {
        match self {
            WalletNetwork::Test => WalletParams::Consensus(Network::TestNetwork),
            WalletNetwork::Main => WalletParams::Consensus(Network::MainNetwork),
            WalletNetwork::Regtest(level) => WalletParams::Local(level.local_network()),
        }
    }

    /// The address/key encoding network type.
    pub fn network_type(self) -> NetworkType {
        match self {
            WalletNetwork::Test => NetworkType::Test,
            WalletNetwork::Main => NetworkType::Main,
            WalletNetwork::Regtest(_) => NetworkType::Regtest,
        }
    }

    /// A sensible default lightwalletd endpoint for this network. For regtest
    /// this is lightwalletd's conventional local port; a devnet that picks its
    /// own port (thus-spoke-zakura) is found with devnet detection instead.
    pub fn default_lightwalletd(self) -> &'static str {
        match self {
            WalletNetwork::Test => "https://testnet.zec.rocks:443",
            WalletNetwork::Main => "https://zec.rocks:443",
            WalletNetwork::Regtest(_) => "http://127.0.0.1:9067",
        }
    }

    /// On-disk directory name for this network's wallet data. Each network
    /// keeps entirely separate databases, blocks caches, and pending
    /// transactions, so switching networks never shows one network's balance
    /// while pointed at the other's chain (and testnet or devnet data can't
    /// corrupt a mainnet db, or vice versa).
    ///
    /// All regtest upgrade levels share one directory: the level describes the
    /// node's rules, not a different chain.
    pub fn dir_name(self) -> &'static str {
        match self {
            WalletNetwork::Test => "testnet",
            WalletNetwork::Main => "mainnet",
            WalletNetwork::Regtest(_) => "regtest",
        }
    }
}

/// Chain info reported by a lightwalletd server (a connectivity probe).
#[derive(Debug, Clone, Serialize)]
pub struct LightwalletdInfo {
    pub chain_name: String,
    pub block_height: u64,
    pub estimated_height: u64,
    pub vendor: String,
    pub version: String,
    /// The consensus branch id the node currently expects, as lowercase hex
    /// (e.g. `5437f330`). A transaction built for a different branch is rejected
    /// with "incorrect consensus branch id".
    pub consensus_branch_id: String,
    /// The consensus branch id this wallet build would produce at the node's
    /// current height, lowercase hex. Filled in by the command layer (which
    /// knows the configured network). `None` when not computed.
    #[serde(default)]
    pub wallet_branch_id: Option<String>,
    /// True when `wallet_branch_id` matches `consensus_branch_id` — i.e. this
    /// build can create transactions the node will accept. `None` when unknown.
    #[serde(default)]
    pub branch_supported: Option<bool>,
}

/// The consensus branch id this build would use at `height` on `network`, as
/// lowercase 8-digit hex. Compared against the node's expected branch id to
/// detect a network-upgrade (e.g. Ironwood/NU7) mismatch before building a tx.
pub fn branch_id_for_height(network: WalletNetwork, height: u64) -> String {
    use zcash_protocol::consensus::{BlockHeight, BranchId};
    let params = network.params();
    let bid = BranchId::for_height(&params, BlockHeight::from_u32(height as u32));
    format!("{:08x}", u32::from(bid))
}

/// The NU6.3 / Ironwood consensus branch id (little-endian u32, printed as
/// 8-digit hex). Orchard actions mined under this upgrade prove against the
/// `PostNu6_3` circuit (the fixed circuit plus the `disableCrossAddress`
/// constraint). Activated on testnet at 4,134,000 and on mainnet at 3,428,143
/// (see `zcash_protocol` activation tables), so both live networks are past it.
const NU6_3_BRANCH_ID: &str = "37a5165b";

/// The Orchard proving/verifying circuit version to use for a transaction mined
/// at `height` on `network`. This MUST match the consensus branch active at that
/// height, or the proof is rejected by the network:
///
/// - Post-NU6.3 (Ironwood; both testnet and mainnet are past it) → `PostNu6_3`.
/// - Post-NU6.2 but pre-NU6.3 → `FixedPostNu6_2`.
///
/// Deriving it from the live branch id — rather than hardcoding one network's
/// value — is what lets the same build produce valid transactions on both
/// networks, each picking the circuit for the upgrade active at that height.
/// `FixedPostNu6_2` is the pre-Ironwood floor; the historical `InsecurePreNu6_2`
/// circuit is never used for new sends.
#[cfg(feature = "wallet")]
fn orchard_circuit_version_for_height(
    network: WalletNetwork,
    height: u64,
) -> orchard::circuit::OrchardCircuitVersion {
    use orchard::circuit::OrchardCircuitVersion;
    if branch_id_for_height(network, height) == NU6_3_BRANCH_ID {
        OrchardCircuitVersion::PostNu6_3
    } else {
        OrchardCircuitVersion::FixedPostNu6_2
    }
}

/// Normalize an endpoint: a bare `host:port` (e.g. `tz.ombie.cash:443`) is
/// assumed to be TLS and gets an `https://` scheme.
fn normalize_endpoint(url: &str) -> String {
    let url = url.trim();
    if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

/// True when the host component of a normalized URL is a loopback address —
/// plaintext gRPC is only tolerated against a local node (regtest/dev), never
/// against a remote lightwalletd where the traffic would cross the network.
pub fn is_loopback_host(normalized_url: &str) -> bool {
    let after_scheme = normalized_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(normalized_url);
    let host = after_scheme
        .split(['/', ':'])
        .next()
        .unwrap_or("")
        .trim_end_matches('.');
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]"
}

/// Reject a lightwalletd endpoint that would send wallet traffic in cleartext.
/// `http://` is permitted only for loopback hosts (local regtest/dev); any
/// remote plaintext endpoint is refused so compact-block sync, balances, and
/// broadcasts are never exposed on the wire.
pub fn validate_endpoint_security(url: &str) -> Result<(), CoreError> {
    let normalized = normalize_endpoint(url);
    if normalized.starts_with("http://") && !is_loopback_host(&normalized) {
        return Err(CoreError::Connection(format!(
            "refusing plaintext (http://) lightwalletd endpoint '{}': \
             wallet traffic would be unencrypted. Use https:// (or a \
             127.0.0.1 endpoint for local testing).",
            url.trim()
        )));
    }
    Ok(())
}

/// Connect a gRPC client to a lightwalletd endpoint (TLS for `https://`).
async fn connect(url: &str) -> Result<CompactTxStreamerClient<Channel>, CoreError> {
    validate_endpoint_security(url)?;
    let url = normalize_endpoint(url);
    let mut endpoint = Channel::from_shared(url.clone())
        .map_err(|e| CoreError::Connection(format!("invalid lightwalletd URL: {e}")))?
        // Syncing streams compact blocks for minutes at a time. Without
        // keep-alive, a connection that dies silently — a NAT idle timeout, a
        // server restart, a dropped VPN — leaves the stream waiting forever with
        // no error and no progress. HTTP/2 pings detect the dead peer and fail
        // the request so the caller can retry. Note there is deliberately no
        // request `timeout()`: that would abort healthy long block streams.
        .connect_timeout(std::time::Duration::from_secs(15))
        .tcp_keepalive(Some(std::time::Duration::from_secs(30)))
        .http2_keep_alive_interval(std::time::Duration::from_secs(20))
        .keep_alive_timeout(std::time::Duration::from_secs(20))
        .keep_alive_while_idle(true);
    if url.starts_with("https://") {
        endpoint = endpoint
            .tls_config(ClientTlsConfig::new().with_webpki_roots())
            .map_err(|e| CoreError::Connection(format!("TLS config: {e}")))?;
    }
    // tonic renders a failed connect as the bare string "transport error"; the
    // DNS/refused/TLS cause is only reachable through the source chain.
    let channel = endpoint
        .connect()
        .await
        .map_err(|e| crate::neterr::connection_error("connecting to lightwalletd", &url, &e))?;
    Ok(CompactTxStreamerClient::new(channel))
}

/// Fetch chain info from a lightwalletd endpoint — used to verify reachability
/// and show the current chain height before syncing.
pub async fn lightwalletd_info(url: &str) -> Result<LightwalletdInfo, CoreError> {
    let mut client = connect(url).await?;
    let info = client
        .get_lightd_info(Empty {})
        .await
        .map_err(|e| CoreError::Connection(format!("get_lightd_info: {e}")))?
        .into_inner();
    Ok(LightwalletdInfo {
        chain_name: info.chain_name,
        block_height: info.block_height,
        estimated_height: info.estimated_height,
        vendor: info.vendor,
        version: info.version,
        consensus_branch_id: info.consensus_branch_id.trim().to_lowercase(),
        wallet_branch_id: None,
        branch_supported: None,
    })
}

// ---------------------------------------------------------------------------
// Per-group wallet: sqlite-backed account, sync, and balance.
//
// Each FROST group is one view-only Orchard account, stored in its own sqlite
// wallet under `<data_dir>/wallets/<group_id>/`. The group's UFVK (derived from
// its `ak`) is imported as a watch-only account; sync trial-decrypts compact
// blocks locally; balance is read from the wallet db.
// ---------------------------------------------------------------------------

use std::path::{Path, PathBuf};

use rand::rngs::OsRng;
use zcash_client_backend::data_api::chain::error::Error as ChainError;
use zcash_client_backend::data_api::chain::{
    BlockSource, ChainState, CommitmentTreeRoot,
};
use zcash_client_backend::data_api::scanning::{ScanPriority, ScanRange};
use zcash_client_backend::data_api::wallet::{
    create_pczt_from_proposal, propose_standard_transfer_to_address, ConfirmationsPolicy,
};
use zcash_client_backend::data_api::{
    AccountBirthday, AccountPurpose, WalletCommitmentTrees, WalletRead, WalletWrite,
};
use zcash_client_backend::fees::StandardFeeRule;
use zcash_client_backend::wallet::OvkPolicy;
use zcash_client_backend::proto::compact_formats::CompactBlock;
use zcash_client_backend::proto::service::{BlockId, ChainSpec};
use zcash_client_sqlite::util::SystemClock;
use zcash_client_sqlite::wallet::init::init_wallet_db;
use zcash_client_sqlite::WalletDb;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_protocol::memo::{Memo, MemoBytes};

type GroupDb = WalletDb<rusqlite::Connection, WalletParams, SystemClock, OsRng>;

/// `(wallet.sqlite path, fsblockdb dir)` for a group on a given network.
/// Scoped by network (`.../<group_id>/<network>/...`) so testnet and mainnet
/// each keep their own db, blocks cache, and balance — they never share state.
fn wallet_paths(data_dir: &Path, group_id: &str, network: WalletNetwork) -> (PathBuf, PathBuf) {
    let base = data_dir
        .join("wallets")
        .join(group_id)
        .join(network.dir_name());
    (base.join("wallet.sqlite"), base.join("blocks"))
}

/// Path of the on-disk record for a fully-signed transaction awaiting broadcast.
/// Keeping the signed PCZT lets a failed broadcast be retried without repeating
/// the whole FROST signing ceremony. Network-scoped like [`wallet_paths`].
fn pending_tx_path(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    ceremony_id: &str,
) -> PathBuf {
    data_dir
        .join("wallets")
        .join(group_id)
        .join(network.dir_name())
        .join("pending")
        .join(format!("{ceremony_id}.pczt.hex"))
}

/// Persist a signed-but-not-broadcast PCZT so it can be re-broadcast later.
pub fn save_pending_tx(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    ceremony_id: &str,
    signed_pczt_hex: &str,
) -> Result<PathBuf, CoreError> {
    let path = pending_tx_path(data_dir, group_id, network, ceremony_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        let _ = crate::keystore::restrict_dir_to_owner(parent);
    }
    std::fs::write(&path, signed_pczt_hex.as_bytes())?;
    let _ = crate::keystore::restrict_to_owner(&path);
    Ok(path)
}

/// Load a previously-saved signed PCZT for re-broadcast.
pub fn load_pending_tx(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    ceremony_id: &str,
) -> Result<String, CoreError> {
    let path = pending_tx_path(data_dir, group_id, network, ceremony_id);
    let hex = std::fs::read_to_string(&path).map_err(|e| {
        CoreError::Config(format!("no pending transaction for {ceremony_id}: {e}"))
    })?;
    Ok(hex.trim().to_string())
}

/// Remove a pending transaction record once it has been broadcast.
pub fn clear_pending_tx(data_dir: &Path, group_id: &str, network: WalletNetwork, ceremony_id: &str) {
    let path = pending_tx_path(data_dir, group_id, network, ceremony_id);
    let _ = std::fs::remove_file(path);
}

/// The `PRAGMA key` statement for a raw 32-byte SQLCipher key. Using the
/// `x'<hex>'` blob form supplies the key material directly (no passphrase KDF).
fn key_pragma(db_key: &[u8]) -> String {
    format!("PRAGMA key = \"x'{}'\";", hex::encode(db_key))
}

/// How long a connection waits for a competing lock before giving up with
/// `SQLITE_BUSY` ("database is locked").
///
/// SQLite defaults this to zero, so the sync writer and the UI's periodic read
/// queries (balances, notes, history) fail *immediately* the moment they
/// overlap, rather than waiting out each other's short-lived locks. Every
/// connection to a group wallet must set this.
const DB_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Put a group wallet into WAL mode. Must run *after* the SQLCipher key pragma.
///
/// In SQLite's default rollback-journal mode, every reader holds a SHARED lock,
/// and a writer can only commit once it upgrades to EXCLUSIVE — which requires
/// that no SHARED locks are held. The UI polls this db while a sync runs (scan
/// progress every 2s, notes every 15s, history every 35s), so a long catch-up
/// sync is starved out of the EXCLUSIVE lock, blows past `DB_BUSY_TIMEOUT`, and
/// dies with "database is locked" mid-way through a commitment-tree write.
///
/// WAL lets readers and the single writer proceed concurrently, so the sync
/// commits regardless of how often the UI reads. The mode is persisted in the
/// database header, so it only has to be set once per file.
///
/// `journal_mode` reports the mode actually in effect: a filesystem that cannot
/// support WAL (some network mounts) keeps the old mode rather than failing.
/// That is degraded but still correct, so don't turn it into a hard error.
fn set_wal_mode(conn: &rusqlite::Connection) -> Result<(), CoreError> {
    let _mode: String = conn
        .query_row("PRAGMA journal_mode = WAL;", [], |r| r.get(0))
        .map_err(|e| CoreError::Crypto(format!("set journal mode: {e}")))?;
    // Safe under WAL: a crash can lose the last commits but cannot corrupt the
    // db, and it keeps the block-scan write path from fsyncing on every batch.
    conn.execute_batch("PRAGMA synchronous = NORMAL;")
        .map_err(|e| CoreError::Crypto(format!("set synchronous: {e}")))?;
    Ok(())
}

/// Open a connection used only for reads (balances, notes, history, scan
/// progress), unlocking it with the SQLCipher key when the file is encrypted. A
/// pre-encryption plaintext db (not yet migrated by [`open_db`]) is read as-is.
///
/// Read-only by *convention*, not by open flag: this deliberately does NOT pass
/// `SQLITE_OPEN_READ_ONLY`. Under WAL (see [`set_wal_mode`]) a reader needs the
/// `-shm` shared-memory index, and a strictly read-only handle cannot create it
/// — so if the last connection closed cleanly (which removes `-wal`/`-shm`), a
/// read-only open of a WAL db fails outright with `SQLITE_CANTOPEN`. A writable
/// handle can materialize the index; callers here still only issue SELECTs, and
/// a read that takes no write lock cannot block the sync writer under WAL.
fn open_readonly_connection(
    db_path: &Path,
    db_key: &[u8],
) -> Result<rusqlite::Connection, CoreError> {
    let plaintext = is_plaintext_sqlite(db_path)?;
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| CoreError::Crypto(format!("open wallet db: {e}")))?;
    conn.busy_timeout(DB_BUSY_TIMEOUT)
        .map_err(|e| CoreError::Crypto(format!("set busy timeout: {e}")))?;
    if !plaintext {
        conn.execute_batch(&key_pragma(db_key))
            .map_err(|e| CoreError::Crypto(format!("unlock wallet db: {e}")))?;
    }
    // Same `WHERE x IN rarray(?)` support the writable connection registers;
    // zcash_client_sqlite's read queries rely on it too.
    rusqlite::vtab::array::load_module(&conn)
        .map_err(|e| CoreError::Crypto(format!("load rarray module: {e}")))?;
    Ok(conn)
}

/// Scan progress as `(fully_scanned_height, chain_tip_height)`.
///
/// Deliberately opens a *read-only* connection rather than reusing
/// [`group_status`], which takes the writable one: this is polled while
/// `sync_group` holds the writer for the whole catch-up, and a second writer
/// would simply block. Each scanned batch is committed, so the height observed
/// here advances during a sync that would otherwise look frozen.
pub fn sync_progress(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    db_key: &[u8],
) -> Result<(u64, u64), CoreError> {
    let (db_path, _) = wallet_paths(data_dir, group_id, network);
    if !db_path.exists() {
        return Ok((0, 0));
    }
    let conn = open_readonly_connection(&db_path, db_key)?;
    let db = WalletDb::from_connection(&conn, network.params(), SystemClock, OsRng);
    let summary = db
        .get_wallet_summary(ConfirmationsPolicy::default())
        .map_err(|e| CoreError::Crypto(format!("wallet summary: {e}")))?;
    Ok(summary.map_or((0, 0), |s| {
        (
            u64::from(s.fully_scanned_height()),
            u64::from(s.chain_tip_height()),
        )
    }))
}

/// Open a rusqlite connection and unlock it with the SQLCipher key, verifying
/// that the key actually decrypts the database (a wrong key or a plaintext file
/// fails here with `SQLITE_NOTADB`).
fn open_keyed_connection(
    db_path: &Path,
    db_key: &[u8],
) -> Result<rusqlite::Connection, CoreError> {
    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| CoreError::Crypto(format!("open wallet db: {e}")))?;
    // Sync writes while the UI reads on its refresh timers; wait out the other
    // connection's lock instead of failing with "database is locked".
    conn.busy_timeout(DB_BUSY_TIMEOUT)
        .map_err(|e| CoreError::Crypto(format!("set busy timeout: {e}")))?;
    conn.execute_batch(&key_pragma(db_key))
        .map_err(|e| CoreError::Crypto(format!("set wallet db key: {e}")))?;
    // Force the cipher to engage; fails cleanly if the key is wrong.
    conn.execute_batch("SELECT count(*) FROM sqlite_master;")
        .map_err(|e| CoreError::Crypto(format!("unlock wallet db: {e}")))?;
    set_wal_mode(&conn)?;
    // WalletDb::for_path normally registers this virtual table for `WHERE x IN
    // rarray(?)` queries used internally by zcash_client_sqlite/backend; since
    // we build the connection ourselves (to key it first), register it here too.
    rusqlite::vtab::array::load_module(&conn)
        .map_err(|e| CoreError::Crypto(format!("load rarray module: {e}")))?;
    Ok(conn)
}

/// True when the file begins with the standard plaintext SQLite header, i.e. it
/// is an unencrypted database (an encrypted SQLCipher file has no such header).
fn is_plaintext_sqlite(db_path: &Path) -> Result<bool, CoreError> {
    use std::io::Read;
    let mut f = std::fs::File::open(db_path)?;
    let mut magic = [0u8; 16];
    match f.read(&mut magic) {
        Ok(16) => Ok(&magic == b"SQLite format 3\0"),
        _ => Ok(false),
    }
}

/// One-time migration of a legacy plaintext wallet database to an encrypted
/// SQLCipher database keyed by `db_key`, preserving all data. Exports the
/// plaintext db into an attached encrypted copy, then atomically replaces the
/// original. See <https://www.zetetic.net/sqlcipher/sqlcipher-api/#sqlcipher_export>.
fn migrate_plaintext_to_encrypted(db_path: &Path, db_key: &[u8]) -> Result<(), CoreError> {
    let tmp = db_path.with_extension("sqlite.enc-tmp");
    let _ = std::fs::remove_file(&tmp);
    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| CoreError::Crypto(format!("open plaintext wallet db: {e}")))?;
    conn.execute_batch(&format!(
        "ATTACH DATABASE '{}' AS encrypted KEY \"x'{}'\";\
         SELECT sqlcipher_export('encrypted');\
         DETACH DATABASE encrypted;",
        tmp.to_string_lossy().replace('\'', "''"),
        hex::encode(db_key),
    ))
    .map_err(|e| CoreError::Crypto(format!("encrypt wallet db: {e}")))?;
    drop(conn);
    std::fs::rename(&tmp, db_path)?;
    let _ = crate::keystore::restrict_to_owner(db_path);
    Ok(())
}

fn open_db(db_path: &Path, network: WalletNetwork, db_key: &[u8]) -> Result<GroupDb, CoreError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
        // The wallet db holds the group's UFVK and transaction history, so lock
        // the directory to the owner as defence in depth alongside encryption.
        let _ = crate::keystore::restrict_dir_to_owner(parent);
    }
    // Transparently upgrade a pre-encryption plaintext db to SQLCipher so no
    // history is lost when this version is first run.
    if db_path.exists() && is_plaintext_sqlite(db_path)? {
        migrate_plaintext_to_encrypted(db_path, db_key)?;
    }
    let conn = open_keyed_connection(db_path, db_key)?;
    let mut db = WalletDb::from_connection(conn, network.params(), SystemClock, OsRng);
    init_wallet_db(&mut db, None)
        .map_err(|e| CoreError::Crypto(format!("init wallet db: {e}")))?;
    // Restrict the sqlite file itself to owner-only.
    if db_path.exists() {
        let _ = crate::keystore::restrict_to_owner(db_path);
    }
    Ok(db)
}

/// Open a group wallet for *reading only* (balances, account ids): a `WalletDb`
/// over a keyed connection, but WITHOUT running `init_wallet_db`.
///
/// `init_wallet_db` opens a write transaction to check/apply schema migrations,
/// so calling it on every read takes a write lock. The balance panel polls
/// `group_status` every few seconds, so under `open_db` each poll fought the sync
/// writer for the single WAL write lock and intermittently lost with "database is
/// locked" mid commitment-tree write. Migrations already ran when the account was
/// created and run again on every sync, so a read never needs to migrate — a
/// SELECT under WAL takes only a shared lock and cannot block the writer.
fn open_db_read(db_path: &Path, network: WalletNetwork, db_key: &[u8]) -> Result<GroupDb, CoreError> {
    let conn = open_keyed_connection(db_path, db_key)?;
    Ok(WalletDb::from_connection(conn, network.params(), SystemClock, OsRng))
}

/// A single shielded/transparent pool's balance, broken into spendable now,
/// pending (maturing or unconfirmed), and total.
#[derive(Debug, Clone, Serialize, Default)]
pub struct PoolBalance {
    /// Confirmed and spendable right now.
    pub spendable_zatoshis: u64,
    /// Received but not yet spendable (awaiting confirmations / maturity).
    pub pending_zatoshis: u64,
    /// spendable + pending.
    pub total_zatoshis: u64,
}

/// Balance + sync status for a group's wallet.
#[derive(Debug, Clone, Serialize)]
pub struct WalletStatus {
    /// Whether the view-only account has been imported yet.
    pub initialized: bool,
    /// Receiving unified address (from the UFVK), for the configured network.
    pub address: Option<String>,
    /// Aggregate totals across *all* pools the account holds. Post-Ironwood a
    /// group's funds are split between the sealed `orchard` pool and the new
    /// `ironwood` pool (a turnstile send moves value from the former to the
    /// latter), so this is the sum of both — not the Orchard pool alone.
    pub total_zatoshis: u64,
    pub spendable_zatoshis: u64,
    /// Per-pool breakdown. `orchard` is the sealed legacy pool (can be spent but
    /// never received into again); `ironwood` is the pool all new shielded value
    /// lands in post-NU6.3. `sapling` and `transparent` are zero for an
    /// Orchard/Ironwood group UFVK.
    pub orchard: PoolBalance,
    pub ironwood: PoolBalance,
    pub sapling: PoolBalance,
    pub transparent: PoolBalance,
    /// Highest fully-scanned block, and the chain tip the wallet knows about.
    pub synced_height: u64,
    pub chain_tip_height: u64,
}

/// Read a group's wallet status from its local db (no network).
pub fn group_status(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    ufvk: &str,
    db_key: &[u8],
) -> Result<WalletStatus, CoreError> {
    let (db_path, _) = wallet_paths(data_dir, group_id, network);
    let address = ufvk_default_address(network, ufvk).ok();
    if !db_path.exists() {
        return Ok(WalletStatus {
            initialized: false,
            address,
            total_zatoshis: 0,
            spendable_zatoshis: 0,
            orchard: PoolBalance::default(),
            ironwood: PoolBalance::default(),
            sapling: PoolBalance::default(),
            transparent: PoolBalance::default(),
            synced_height: 0,
            chain_tip_height: 0,
        });
    }
    // Read-only: no migration, so this poll never takes a write lock (see
    // open_db_read). The account was already created and migrated by
    // init_group_account before any status read can happen.
    let db = open_db_read(&db_path, network, db_key)?;
    let account_ids = db
        .get_account_ids()
        .map_err(|e| CoreError::Crypto(format!("wallet accounts: {e}")))?;
    if account_ids.is_empty() {
        return Ok(WalletStatus {
            initialized: false,
            address,
            total_zatoshis: 0,
            spendable_zatoshis: 0,
            orchard: PoolBalance::default(),
            ironwood: PoolBalance::default(),
            sapling: PoolBalance::default(),
            transparent: PoolBalance::default(),
            synced_height: 0,
            chain_tip_height: 0,
        });
    }
    let summary = db
        .get_wallet_summary(ConfirmationsPolicy::default())
        .map_err(|e| CoreError::Crypto(format!("wallet summary: {e}")))?;
    let (total, spendable, orchard, ironwood, sapling, transparent, synced, tip) = match summary {
        Some(s) => {
            let bal = s.account_balances().values().next();
            let total = bal.map(|b| u64::from(b.total())).unwrap_or(0);
            let spendable = bal.map(|b| u64::from(b.spendable_value())).unwrap_or(0);
            // Per-pool breakdown. Orchard (sealed) and Ironwood (post-NU6.3) are
            // the pools the group holds; sapling/transparent read 0 with an
            // Orchard/Ironwood-only UFVK.
            let orchard = bal.map(|b| pool_balance(b.orchard_balance())).unwrap_or_default();
            let ironwood = bal.map(|b| pool_balance(b.ironwood_balance())).unwrap_or_default();
            let sapling = bal.map(|b| pool_balance(b.sapling_balance())).unwrap_or_default();
            let transparent = bal
                .map(|b| {
                    // Transparent (unshielded) has no maturity concept; treat the
                    // whole unshielded balance as spendable/total.
                    let t = u64::from(b.unshielded_balance().total());
                    PoolBalance {
                        spendable_zatoshis: t,
                        pending_zatoshis: 0,
                        total_zatoshis: t,
                    }
                })
                .unwrap_or_default();
            (
                total,
                spendable,
                orchard,
                ironwood,
                sapling,
                transparent,
                u64::from(s.fully_scanned_height()),
                u64::from(s.chain_tip_height()),
            )
        }
        None => (
            0,
            0,
            PoolBalance::default(),
            PoolBalance::default(),
            PoolBalance::default(),
            PoolBalance::default(),
            0,
            0,
        ),
    };
    Ok(WalletStatus {
        initialized: true,
        address,
        total_zatoshis: total,
        spendable_zatoshis: spendable,
        orchard,
        ironwood,
        sapling,
        transparent,
        synced_height: synced,
        chain_tip_height: tip,
    })
}

/// Convert a zcash_client_backend shielded-pool `Balance` into our `PoolBalance`.
/// Pending = value awaiting spendability + change awaiting confirmation.
fn pool_balance(b: &zcash_client_backend::data_api::Balance) -> PoolBalance {
    let spendable = u64::from(b.spendable_value());
    let pending = u64::from(b.value_pending_spendability())
        + u64::from(b.change_pending_confirmation());
    PoolBalance {
        spendable_zatoshis: spendable,
        pending_zatoshis: pending,
        total_zatoshis: u64::from(b.total()),
    }
}

/// A deep-rescan floor for testnet: sits comfortably before the NU6.3/Ironwood
/// activation (4,134,000), so scanning from here finds a group funded any time
/// during Ironwood testing.
///
/// This is NOT applied automatically. A brand-new group has no prior funds, so
/// it starts at the chain tip and syncs in seconds; scanning ~350k blocks of
/// pre-creation history was the single largest source of slow first syncs.
/// Recovery of a wiped wallet uses the birthday persisted in settings instead
/// (see the command layer). This constant is only for an *explicit* deep rescan
/// — a user who lost both the wallet db and the recorded birthday and wants to
/// re-discover funds — supplied through `birthday_height`, never as a default.
pub const DEFAULT_TESTNET_BIRTHDAY: u64 = 3_800_000;

/// The birthday a brand-new wallet starts from when the caller gives no height
/// and none was recorded for the group: the chain tip, on both networks.
///
/// A newly created group cannot hold funds mined before it existed, so there is
/// nothing to find below the tip — scanning earlier is pure cost. Returning
/// `None` lets [`resolve_scan_from`] resolve the start to the current tip. To
/// recover funds that predate a rebuilt wallet, pass an explicit
/// `birthday_height` (the command layer supplies the persisted one); for a
/// from-scratch testnet rescan, pass [`DEFAULT_TESTNET_BIRTHDAY`].
///
/// A local regtest devnet is the exception: its whole chain is a few hundred
/// blocks, and a developer typically funds the group from the devnet faucet
/// *before* opening its wallet. Scanning from [`REGTEST_BIRTHDAY`] costs
/// nothing and finds those funds.
pub fn default_birthday_height(network: WalletNetwork) -> Option<u64> {
    match network {
        WalletNetwork::Regtest(_) => Some(REGTEST_BIRTHDAY),
        WalletNetwork::Test | WalletNetwork::Main => None,
    }
}

/// First block a regtest wallet scans. lightwalletd cannot serve a devnet's
/// block 1 (thus-spoke-zakura documents scanning from block 2), and block 1
/// only carries the upgrade-activation coinbase, so nothing is lost.
pub const REGTEST_BIRTHDAY: u64 = 2;

/// Pick the first block to scan: the requested birthday held inside
/// `[nu5, tip]`, or the tip when nothing was requested.
///
/// Both bounds matter. Below NU5 there can be no Orchard notes, so scanning
/// there is pure cost. Above the tip there is no treestate to anchor the
/// birthday to, and the wallet would scan nothing at all — which is how a
/// testnet-shaped default silently breaks a mainnet wallet.
fn resolve_scan_from(requested: Option<u64>, nu5: u64, tip: u64) -> u64 {
    match requested {
        Some(h) => h.max(nu5).min(tip),
        None => tip,
    }
}

/// Import the group's UFVK as a view-only account and return the height its
/// scanning starts from. Idempotent: returns 0 if the account already exists.
/// Touches the network (fetches the tip and a treestate).
///
/// `birthday_height` is the first block to scan. Pass `None` for a brand-new
/// group: it holds no prior funds, so it starts at the chain tip and syncs in
/// seconds. Pass `Some(h)` to recover a group whose funds arrived at or after
/// `h` — after rebuilding a wiped wallet database (the command layer supplies
/// the persisted birthday), or [`DEFAULT_TESTNET_BIRTHDAY`] for a full rescan.
///
/// Blocks before the birthday are never scanned, so a birthday set too late
/// makes existing funds invisible. The height is clamped into
/// `[NU5 activation, chain tip]`: Orchard notes cannot exist below NU5, and a
/// birthday above the tip has no treestate to anchor to.
pub async fn init_group_account(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    ufvk_str: &str,
    lightwalletd_url: &str,
    db_key: &[u8],
    birthday_height: Option<u64>,
) -> Result<u64, CoreError> {
    use zcash_protocol::consensus::{NetworkUpgrade, Parameters};

    let (db_path, _) = wallet_paths(data_dir, group_id, network);
    let mut db = open_db(&db_path, network, db_key)?;
    if !db
        .get_account_ids()
        .map_err(|e| CoreError::Crypto(format!("wallet accounts: {e}")))?
        .is_empty()
    {
        tracing::debug!(group = %group_id, "wallet setup: account already imported; nothing to do");
        return Ok(0); // already imported
    }

    let params = network.params();
    let ufvk = UnifiedFullViewingKey::decode(&params, ufvk_str)
        .map_err(|e| CoreError::Crypto(format!("invalid UFVK: {e}")))?;

    // These are single unary RPCs, not the long block stream, so bound them. A
    // server that accepts the TCP connection but never answers (a misconfigured
    // proxy, a stalled `get_tree_state` for a deep birthday) would otherwise leave
    // wallet setup spinning forever with no error and no log line.
    let rpc_timeout = std::time::Duration::from_secs(30);

    tracing::info!(group = %group_id, url = %lightwalletd_url, "wallet setup: connecting to lightwalletd");
    let mut client = connect(lightwalletd_url).await?;
    let tip = tokio::time::timeout(rpc_timeout, client.get_latest_block(ChainSpec {}))
        .await
        .map_err(|_| {
            CoreError::Connection(
                "get_latest_block timed out — lightwalletd accepted the connection but did not respond".into(),
            )
        })?
        .map_err(|e| CoreError::Connection(format!("get_latest_block: {e}")))?
        .into_inner()
        .height;
    tracing::info!(group = %group_id, tip, "wallet setup: connected; got chain tip");

    let nu5 = params
        .activation_height(NetworkUpgrade::Nu5)
        .map_or(0, |a| u64::from(u32::from(a)));
    // No requested birthday means a brand-new group: start at the tip. Recovery
    // passes an explicit height (persisted birthday or a deep-rescan floor).
    let scan_from = resolve_scan_from(
        birthday_height.or_else(|| default_birthday_height(network)),
        nu5,
        tip,
    );

    // `AccountBirthday::height()` is `prior_chain_state.block_height() + 1`, so
    // request the frontier as of the block *before* the first one to scan.
    // Fetching the treestate at `scan_from` itself would skip that block — and
    // with it the transaction that funded the group.
    tracing::info!(group = %group_id, scan_from, "wallet setup: fetching tree state for the account birthday");
    let treestate = tokio::time::timeout(
        rpc_timeout,
        client.get_tree_state(BlockId {
            height: scan_from.saturating_sub(1),
            hash: vec![],
        }),
    )
    .await
    .map_err(|_| {
        CoreError::Connection(
            "get_tree_state timed out — lightwalletd did not return the birthday tree state".into(),
        )
    })?
    .map_err(|e| CoreError::Connection(format!("get_tree_state: {e}")))?
    .into_inner();
    let birthday = AccountBirthday::from_treestate(treestate, None)
        .map_err(|_| CoreError::Crypto("could not derive account birthday from treestate".into()))?;

    db.import_account_ufvk(group_id, &ufvk, &birthday, AccountPurpose::ViewOnly, None)
        .map_err(|e| CoreError::Crypto(format!("import account: {e}")))?;
    tracing::info!(group = %group_id, scan_from, "wallet setup: view-only account imported; will scan from this height");
    Ok(scan_from)
}

/// How many blocks each sync batch downloads and scans at once when the caller
/// gives no override. Larger batches amortize the per-batch gRPC round-trip and
/// database-transaction overhead across more blocks, which is the dominant cost
/// once trial decryption finds nothing (the common case for a wallet catching up
/// over empty history). Compact blocks are small, so a few thousand per batch is
/// comfortable in memory. Tunable per-install via settings; see [`sync_group`].
pub const DEFAULT_SYNC_BATCH_SIZE: u32 = 5_000;

/// Clamp bounds for a caller-supplied batch size. Below the floor the round-trip
/// overhead dominates; above the ceiling a batch's worth of compact blocks can
/// spike memory (they are all held in a `Vec` while the batch is scanned).
pub const MIN_SYNC_BATCH_SIZE: u32 = 500;
pub const MAX_SYNC_BATCH_SIZE: u32 = 25_000;

/// Options controlling how a sync runs.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncOptions {
    /// Blocks to download and scan per batch; `None` uses
    /// [`DEFAULT_SYNC_BATCH_SIZE`], clamped into `[MIN, MAX]_SYNC_BATCH_SIZE`.
    pub batch_size: Option<u32>,
}

/// Sync the group's wallet: download and trial-decrypt compact blocks from
/// lightwalletd into the local db. Long-running; touches the network.
///
/// Driven by the pipelined [`run_pipelined`] driver, which overlaps block
/// download with CPU trial-decryption and streams blocks straight from the
/// network to the scanner (no on-disk block cache).
pub async fn sync_group(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    lightwalletd_url: &str,
    db_key: &[u8],
    opts: SyncOptions,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(), CoreError> {
    let batch_size = opts
        .batch_size
        .unwrap_or(DEFAULT_SYNC_BATCH_SIZE)
        .clamp(MIN_SYNC_BATCH_SIZE, MAX_SYNC_BATCH_SIZE);
    let (db_path, _) = wallet_paths(data_dir, group_id, network);
    let mut db = open_db(&db_path, network, db_key)?;

    let mut client = connect(lightwalletd_url).await?;
    let params = network.params();
    // The driver scans in transactional batches, so dropping the future between
    // batches leaves the db consistent (just short of the tip). That makes it
    // safe to race against a cancellation token: "Sync Now" trips the token to
    // abandon a stalled run, and a fresh sync resumes from where this one left
    // off. Without this, a stuck stream would keep the sync pending forever.
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(CoreError::Cancelled),
        res = run_pipelined(&mut client, &params, &mut db, batch_size) => res,
    };
    // Turn known, actionable failures into a message that says what to do, while
    // keeping the raw server error appended for diagnosis.
    result.map_err(annotate_sync_error)
}

/// Rewrite a raw sync failure into an actionable message when it matches a known
/// cause, preserving the original text after an em-dash for debugging. Applies to
/// both sync drivers (both surface a `CoreError::Connection` carrying the raw
/// lightwalletd/tonic error string).
/// True when a connection error is a lightwalletd rejecting a shielded pool it
/// doesn't recognize — Ironwood (NU6.3) on a server that predates it, which
/// answers `get_subtree_roots` for that pool with gRPC InvalidArgument
/// "invalid shielded protocol value".
fn is_invalid_shielded_protocol(e: &CoreError) -> bool {
    let lower = match e {
        CoreError::Connection(m) => m.to_lowercase(),
        _ => return false,
    };
    lower.contains("invalid shielded protocol")
        || (lower.contains("shielded protocol") && lower.contains("invalid"))
}

fn annotate_sync_error(e: CoreError) -> CoreError {
    // A lightwalletd that predates Ironwood (NU6.3) doesn't know the Ironwood
    // shielded protocol, so the very first sync step — fetching subtree roots for
    // all pools, including Ironwood — is rejected with "invalid shielded protocol
    // value". The whole sync then aborts. This is a server-capability problem, not
    // a wallet bug, and the fix is to point at an Ironwood-capable server. (The
    // pipelined driver skips Ironwood roots on this error instead; this friendly
    // message is for the stock driver, which can't.)
    if is_invalid_shielded_protocol(&e) {
        let raw = match &e {
            CoreError::Connection(m) => m.clone(),
            _ => return e,
        };
        return CoreError::Connection(format!(
            "This lightwalletd server doesn't support Ironwood (NU6.3). Syncing has \
             to fetch the Ironwood note-commitment tree, and the server rejected \
             that request (\"invalid shielded protocol value\"). Switch to an \
             Ironwood-capable lightwalletd in the wallet's network settings, then \
             sync again. — {raw}"
        ));
    }

    e
}

/// An in-memory [`BlockSource`] over one batch of already-downloaded compact
/// blocks. The pipelined driver hands each batch straight from the network to
/// the scanner through this, so a sync never touches an on-disk block cache (no
/// file writes, no cache mutex contention between the download-ahead producer and
/// the scanning consumer). Scanning is fully transactional via `put_blocks`, so an
/// interrupted batch leaves the db consistent.
struct MemBlockSource(Vec<CompactBlock>);

impl BlockSource for MemBlockSource {
    // Reading from an owned `Vec` can't fail.
    type Error = std::convert::Infallible;

    fn with_blocks<F, WalletErrT>(
        &self,
        from_height: Option<BlockHeight>,
        limit: Option<usize>,
        mut with_block: F,
    ) -> Result<(), ChainError<WalletErrT, Self::Error>>
    where
        F: FnMut(CompactBlock) -> Result<(), ChainError<WalletErrT, Self::Error>>,
    {
        let start = from_height.map(u32::from);
        let mut remaining = limit.unwrap_or(usize::MAX);
        for cb in &self.0 {
            if remaining == 0 {
                break;
            }
            // The producer downloads exactly the requested range, but honour
            // `from_height`/`limit` defensively so this matches the disk cache's
            // contract (ascending, contiguous from `from_height`).
            if let Some(s) = start {
                if (cb.height as u32) < s {
                    continue;
                }
            }
            with_block(cb.clone())?;
            remaining -= 1;
        }
        Ok(())
    }
}

/// One prefetched batch handed from the download producer to the scan consumer:
/// the range it covers, its compact blocks, and the chain-state anchor immediately
/// before the range (needed by `scan_cached_blocks`).
type PrefetchedBatch = (ScanRange, Vec<CompactBlock>, ChainState);

/// Split a suggested scan range into `batch_size`-block sub-ranges, preserving
/// priority. Ported verbatim from the upstream `sync::running` step-7 splitter so
/// the pipelined driver scans in the exact same units as the stock driver.
fn split_scan_range(range: ScanRange, batch_size: u32) -> Vec<ScanRange> {
    let mut acc = range;
    let mut out = Vec::new();
    loop {
        if acc.is_empty() {
            break;
        }
        match acc.split_at(acc.block_range().start + batch_size) {
            Some((cur, next)) => {
                out.push(cur);
                acc = next;
            }
            None => {
                out.push(acc);
                break;
            }
        }
    }
    out
}

/// Custom pipelined sync driver: same control flow as
/// `zcash_client_backend::sync::run`, but the historic-range scan overlaps block
/// download with trial-decryption. Correctness-critical logic (subtree roots,
/// chain-tip update, verify pass, reorg/continuity rewind, priority re-ordering)
/// is ported faithfully from the upstream `sync.rs`; only the download/scan
/// overlap in step 7 is new. Produces the same wallet state as the stock driver.
///
/// Note: the transparent-UTXO refresh in upstream `running` is gated on the
/// `transparent-inputs` feature, which our `zcash_client_backend` build does not
/// enable (group accounts are Orchard-only view keys), so the stock driver we run
/// today does not perform it either. Omitting it here keeps the two byte-identical.
async fn run_pipelined(
    client: &mut CompactTxStreamerClient<Channel>,
    params: &WalletParams,
    db: &mut GroupDb,
    batch_size: u32,
) -> Result<(), CoreError> {
    // 1) & 2) Download note-commitment subtree roots and hand them to the db, so
    //    the trees are initialized without replaying all history. One-time; no
    //    pipelining benefit, so it stays serial.
    update_subtree_roots_pipelined(client, db).await?;

    // Re-run the per-session loop until the wallet's view of the chain tip is
    // valid (mirrors `while running(..).await? {}` upstream).
    while running_pipelined(client, params, db, batch_size).await? {}

    Ok(())
}

/// One pass of the pipelined sync loop. Returns `true` when the suggested scan
/// ranges changed underneath us (continuity error, or a newly higher-priority
/// range) and the caller should restart from a fresh `suggest_scan_ranges`.
async fn running_pipelined(
    client: &mut CompactTxStreamerClient<Channel>,
    params: &WalletParams,
    db: &mut GroupDb,
    batch_size: u32,
) -> Result<bool, CoreError> {
    // 3) & 4) Refresh the chain tip so `suggest_scan_ranges` reflects new blocks.
    update_chain_tip_pipelined(client, db).await?;

    // 6) Verify pass. Any `Verify`-priority range is always first; it is small
    //    (a short reorg-check window), so we scan it serially — pipelining it buys
    //    nothing and the loop may re-request ranges after each one.
    loop {
        let scan_ranges = db
            .suggest_scan_ranges()
            .map_err(|e| CoreError::Crypto(format!("suggest_scan_ranges: {e}")))?;
        match scan_ranges.first() {
            Some(sr) if sr.priority() == ScanPriority::Verify => {
                let sr = sr.clone();
                let blocks = download_blocks_pipelined(client, &sr).await?;
                let chain_state =
                    download_chain_state_pipelined(client, sr.block_range().start - 1).await?;
                let src = MemBlockSource(blocks);
                if scan_batch(params, &src, db, &chain_state, &sr)? {
                    // Ranges changed; re-request and re-check for a Verify range.
                    continue;
                }
                // Cache and scanned data are locally consistent; done verifying.
                break;
            }
            _ => break,
        }
    }

    // 7) Historic ranges, pipelined. Snapshot the suggested ranges, split them
    //    into batches, and download-ahead while scanning.
    let scan_ranges = db
        .suggest_scan_ranges()
        .map_err(|e| CoreError::Crypto(format!("suggest_scan_ranges: {e}")))?;
    let batches: Vec<ScanRange> = scan_ranges
        .into_iter()
        .flat_map(|r| split_scan_range(r, batch_size))
        .collect();
    if batches.is_empty() {
        return Ok(false);
    }

    // Producer: download each batch's blocks + chain-state anchor and hand them
    // over a bounded channel (capacity 2) so download runs up to two batches
    // ahead of scanning. A cloned tonic client shares the underlying HTTP/2
    // connection, so this adds no new socket.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<PrefetchedBatch, CoreError>>(2);
    let mut producer_client = client.clone();
    let producer = tokio::spawn(async move {
        for sr in batches {
            let dl_start = std::time::Instant::now();
            let blocks = match download_blocks_pipelined(&mut producer_client, &sr).await {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            };
            tracing::debug!(
                "pipelined download: {} blocks for {} in {} ms",
                blocks.len(),
                sr,
                dl_start.elapsed().as_millis()
            );
            let chain_state = match download_chain_state_pipelined(
                &mut producer_client,
                sr.block_range().start - 1,
            )
            .await
            {
                Ok(cs) => cs,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            };
            // If the consumer has hung up (ranges changed, or an error broke the
            // loop) stop downloading.
            if tx.send(Ok((sr, blocks, chain_state))).await.is_err() {
                return;
            }
        }
    });

    // Consumer: scan each prefetched batch in order. `scan_batch` is CPU-bound
    // and synchronous; on a multi-threaded runtime the producer keeps downloading
    // the next batches on other worker threads while this one scans, which is the
    // whole point. Scanning is transactional per batch, so bailing out early (or
    // being dropped on cancellation) leaves the db consistent at a batch boundary.
    let mut result = Ok(false);
    let scan_run_start = std::time::Instant::now();
    let mut scanned_blocks: u64 = 0;
    while let Some(item) = rx.recv().await {
        let (sr, blocks, chain_state) = match item {
            Ok(v) => v,
            Err(e) => {
                result = Err(e);
                break;
            }
        };
        let n = blocks.len() as u64;
        let src = MemBlockSource(blocks);
        let scan_start = std::time::Instant::now();
        let outcome = scan_batch(params, &src, db, &chain_state, &sr);
        // Per-batch scan cost and cumulative throughput. This is the CPU-bound leg
        // (trial decryption + note-commitment tree updates); logging it here makes
        // the download-vs-scan split visible when diagnosing slow syncs.
        scanned_blocks += n;
        let secs = scan_run_start.elapsed().as_secs_f64();
        tracing::info!(
            "pipelined scan: {} blocks for {} in {} ms ({:.0} blocks/s cumulative over {} blocks)",
            n,
            sr,
            scan_start.elapsed().as_millis(),
            if secs > 0.0 { scanned_blocks as f64 / secs } else { 0.0 },
            scanned_blocks
        );
        match outcome {
            Ok(true) => {
                // Ranges changed (continuity error or a new higher-priority
                // range); restart the whole pass from fresh suggestions.
                result = Ok(true);
                break;
            }
            Ok(false) => {}
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }

    // Stop the producer: either it already finished, or we broke early and it
    // should abandon any in-flight download.
    producer.abort();
    result
}

/// Scan one batch and interpret the outcome, mirroring the upstream `scan_blocks`
/// helper: on a continuity error, rewind the db and signal a restart; otherwise
/// signal a restart if scanning surfaced a higher-priority range. The in-memory
/// source needs no cache truncation on rewind (each batch is downloaded fresh).
fn scan_batch(
    params: &WalletParams,
    src: &MemBlockSource,
    db: &mut GroupDb,
    chain_state: &ChainState,
    scan_range: &ScanRange,
) -> Result<bool, CoreError> {
    use zcash_client_backend::data_api::chain::scan_cached_blocks;

    let scan_result = scan_cached_blocks(
        params,
        src,
        db,
        scan_range.block_range().start,
        chain_state,
        scan_range.len(),
    );

    match scan_result {
        Err(ChainError::Scan(err)) if err.is_continuity_error() => {
            // Rewind to at least one block before the error height, matching the
            // upstream heuristic (10 blocks of slack).
            let rewind_height = err.at_height().saturating_sub(10);
            tracing::info!(
                "chain reorg detected at {}, rewinding to {}",
                err.at_height(),
                rewind_height
            );
            db.truncate_to_height(rewind_height)
                .map_err(|e| CoreError::Crypto(format!("truncate on reorg: {e}")))?;
            Ok(true)
        }
        Ok(_) => {
            // If scanning added a range of higher priority than the one we just
            // scanned, invalidate the current ordering and restart.
            let latest = db
                .suggest_scan_ranges()
                .map_err(|e| CoreError::Crypto(format!("suggest_scan_ranges: {e}")))?;
            Ok(latest
                .first()
                .map(|r| r.priority() > scan_range.priority())
                .unwrap_or(false))
        }
        Err(e) => Err(CoreError::Crypto(format!("scan: {e}"))),
    }
}

/// Download the subtree roots for all three shielded pools and store them, so the
/// note-commitment trees are initialized without replaying history. Ported from
/// the upstream `update_subtree_roots` (Sapling + Orchard + Ironwood).
async fn update_subtree_roots_pipelined(
    client: &mut CompactTxStreamerClient<Channel>,
    db: &mut GroupDb,
) -> Result<(), CoreError> {
    use zcash_client_backend::proto::service::ShieldedProtocol;

    // The concrete root-hash types (`sapling::Node`, `MerkleHashOrchard`) are
    // inferred from the `put_*` calls below, so this compiles without naming the
    // Sapling crate (not a direct dependency of this crate).
    let sapling_roots = download_subtree_roots(client, ShieldedProtocol::Sapling).await?;
    db.put_sapling_subtree_roots(0, &sapling_roots)
        .map_err(|e| CoreError::Crypto(format!("put sapling subtree roots: {e}")))?;

    let orchard_roots = download_subtree_roots(client, ShieldedProtocol::Orchard).await?;
    db.put_orchard_subtree_roots(0, &orchard_roots)
        .map_err(|e| CoreError::Crypto(format!("put orchard subtree roots: {e}")))?;

    // Ironwood (NU6.3) is newer than some lightwalletd deployments — notably on
    // testnet. Such a server rejects the Ironwood subtree-roots request with
    // "invalid shielded protocol value". A server that doesn't know the pool has
    // no Ironwood notes to report, so treat that one rejection as "no Ironwood
    // roots yet" and keep syncing the other pools, rather than aborting the whole
    // sync. (Ironwood value won't be tracked until an Ironwood-capable server is
    // used, but on such a server there is none to miss.) Any other error still
    // fails the sync.
    match download_subtree_roots(client, ShieldedProtocol::Ironwood).await {
        Ok(ironwood_roots) => {
            db.put_ironwood_subtree_roots(0, &ironwood_roots)
                .map_err(|e| CoreError::Crypto(format!("put ironwood subtree roots: {e}")))?;
        }
        Err(e) if is_invalid_shielded_protocol(&e) => {
            tracing::warn!(
                "lightwalletd rejected the Ironwood subtree-roots request ({e}); \
                 continuing without Ironwood tree state — this server predates \
                 Ironwood (NU6.3). Ironwood funds won't be tracked until you use an \
                 Ironwood-capable server."
            );
        }
        Err(e) => return Err(e),
    }

    Ok(())
}

/// Stream the subtree roots for one shielded pool from lightwalletd. Ported from
/// the upstream `download_subtree_roots`.
async fn download_subtree_roots<H>(
    client: &mut CompactTxStreamerClient<Channel>,
    protocol: zcash_client_backend::proto::service::ShieldedProtocol,
) -> Result<Vec<CommitmentTreeRoot<H>>, CoreError>
where
    H: zcash_primitives::merkle_tree::HashSer,
{
    use zcash_client_backend::proto::service::GetSubtreeRootsArg;

    let request = GetSubtreeRootsArg {
        start_index: 0,
        shielded_protocol: protocol as i32,
        max_entries: 0,
    };

    let mut stream = client
        .get_subtree_roots(request)
        .await
        .map_err(|e| CoreError::Connection(format!("get_subtree_roots: {e}")))?
        .into_inner();

    let mut roots = Vec::new();
    while let Some(root) = stream
        .message()
        .await
        .map_err(|e| CoreError::Connection(format!("subtree root stream: {e}")))?
    {
        let root_hash = H::read(&root.root_hash[..])
            .map_err(|e| CoreError::Crypto(format!("subtree root hash: {e}")))?;
        roots.push(CommitmentTreeRoot::from_parts(
            BlockHeight::from_u32(root.completing_block_height as u32),
            root_hash,
        ));
    }
    Ok(roots)
}

/// Fetch the current chain tip and record it, so `suggest_scan_ranges` accounts
/// for newly mined blocks. Ported from the upstream `update_chain_tip`.
async fn update_chain_tip_pipelined(
    client: &mut CompactTxStreamerClient<Channel>,
    db: &mut GroupDb,
) -> Result<(), CoreError> {
    let tip_height: BlockHeight = client
        .get_latest_block(ChainSpec::default())
        .await
        .map_err(|e| CoreError::Connection(format!("get_latest_block: {e}")))?
        .get_ref()
        .height
        .try_into()
        .map_err(|_| CoreError::Crypto("lightwalletd returned an invalid tip height".into()))?;
    db.update_chain_tip(tip_height)
        .map_err(|e| CoreError::Crypto(format!("update chain tip: {e}")))?;
    Ok(())
}

/// Download the compact blocks in `scan_range` into memory. Ported from the
/// upstream `download_blocks`, but returns the blocks instead of writing them to
/// a disk cache, so the producer can hand them straight to the scanner.
async fn download_blocks_pipelined(
    client: &mut CompactTxStreamerClient<Channel>,
    scan_range: &ScanRange,
) -> Result<Vec<CompactBlock>, CoreError> {
    use zcash_client_backend::proto::service::BlockRange;

    let start = BlockId {
        height: scan_range.block_range().start.into(),
        hash: vec![],
    };
    let end = BlockId {
        height: (scan_range.block_range().end - 1).into(),
        hash: vec![],
    };
    let range = BlockRange {
        start: Some(start),
        end: Some(end),
        pool_types: vec![],
    };
    let mut stream = client
        .get_block_range(range)
        .await
        .map_err(|e| CoreError::Connection(format!("get_block_range: {e}")))?
        .into_inner();

    let mut blocks = Vec::new();
    while let Some(cb) = stream
        .message()
        .await
        .map_err(|e| CoreError::Connection(format!("block stream: {e}")))?
    {
        blocks.push(cb);
    }
    Ok(blocks)
}

/// Fetch the chain-state anchor at `block_height` (the tree state just before a
/// range's first block). Ported from the upstream `download_chain_state`.
async fn download_chain_state_pipelined(
    client: &mut CompactTxStreamerClient<Channel>,
    block_height: BlockHeight,
) -> Result<ChainState, CoreError> {
    client
        .get_tree_state(BlockId {
            height: block_height.into(),
            hash: vec![],
        })
        .await
        .map_err(|e| CoreError::Connection(format!("get_tree_state: {e}")))?
        .into_inner()
        .to_chain_state()
        .map_err(|e| CoreError::Crypto(format!("chain state: {e}")))
}

/// Which shielded pool an action belongs to. Post-NU6.3 a single transaction can
/// carry both bundles at once — e.g. a turnstile send spends Orchard notes (an
/// Orchard-bundle action) while delivering the payment through the Ironwood
/// bundle — so every spend must record which bundle holds it. The two bundles
/// use the same `orchard::pczt` types, so signing differs only in which
/// low-level-signer method reaches the bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpendPool {
    Orchard,
    Ironwood,
}

/// One shielded spend the group must FROST-sign: which pool's bundle holds it,
/// its action index within that bundle, and the per-spend re-randomization value
/// α (hex of the canonical scalar encoding), which becomes the FROST
/// coordinator's `randomizer` for that signature.
#[derive(Debug, Clone, Serialize)]
pub struct SpendToSign {
    pub pool: SpendPool,
    pub index: usize,
    pub alpha_hex: String,
}

/// A draft transaction: a built, unsigned PCZT plus the data the FROST signing
/// step needs (the shielded sighash to sign, and each spend's α). Building
/// moves no funds.
#[derive(Debug, Clone, Serialize)]
pub struct DraftTransaction {
    /// Hex of the serialized PCZT, carried into the signing/broadcast step.
    pub pczt_hex: String,
    /// The shielded sighash the group must FROST-sign (hex).
    pub sighash_hex: String,
    /// The Orchard spends to authorize (each FROST-signed with its own α).
    pub spends: Vec<SpendToSign>,
    pub fee_zatoshis: u64,
    pub amount_zatoshis: u64,
    pub recipient: String,
    /// True when the recipient is a transparent address, i.e. this transfer
    /// moves funds out of the group's shielded Orchard pool into the
    /// transparent pool (an "unshield"). The group's Orchard spend is still
    /// FROST-signed exactly as a normal shielded send; only the output differs.
    pub is_unshield: bool,
    /// Optional memo attached to the recipient's shielded output. Encrypted
    /// on-chain; only the recipient's viewing key can decrypt it. Always None
    /// for unshield transfers (transparent outputs carry no memo).
    pub memo: Option<String>,
}

/// Build an unsigned Orchard transfer as a PCZT and return its sighash. Uses
/// the standard ZIP-317 fee and greedy input selection. No signing, no
/// broadcast — this only constructs the transaction.
///
/// Before building, the wallet's chain tip is refreshed from lightwalletd so the
/// transaction's expiry height is anchored to the *current* tip. Otherwise a
/// stale tip yields an expiry that may already be in the past by broadcast time,
/// and the node rejects the tx ("must not be mined at a block height greater
/// than its expiry"). The signing ceremony must still complete within the
/// ~40-block expiry window (≈50 min on testnet) of the build.
pub async fn prepare_send(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    recipient: &str,
    amount_zatoshis: u64,
    memo: Option<String>,
    lightwalletd_url: &str,
    db_key: &[u8],
) -> Result<DraftTransaction, CoreError> {
    use zcash_keys::address::Address;
    use zcash_protocol::value::Zatoshis;
    use zcash_protocol::ShieldedPool;

    let params = network.params();
    let (db_path, _) = wallet_paths(data_dir, group_id, network);
    let mut db = open_db(&db_path, network, db_key)?;

    // Anchor the expiry to the live chain tip, not whatever sync last recorded.
    let mut client = connect(lightwalletd_url).await?;
    let tip_height = client
        .get_latest_block(ChainSpec {})
        .await
        .map_err(|e| CoreError::Connection(format!("get_latest_block: {e}")))?
        .into_inner()
        .height;
    db.update_chain_tip(BlockHeight::from_u32(tip_height as u32))
        .map_err(|e| CoreError::Crypto(format!("update chain tip: {e}")))?;

    let account_id = *db
        .get_account_ids()
        .map_err(|e| CoreError::Crypto(format!("wallet accounts: {e}")))?
        .first()
        .ok_or_else(|| CoreError::Crypto("wallet not initialized".into()))?;

    let r = recipient.trim();
    let to = Address::decode(&params, r).ok_or_else(|| {
        // Give a network-mismatch hint when the address prefix clearly belongs
        // to the other network — saves a confusing round-trip for the user.
        let looks_regtest = r.starts_with("uregtest") || r.starts_with("zregtestsapling");
        let looks_test = r.starts_with("utest") || r.starts_with("ztestsapling");
        let looks_main = (r.starts_with("u1") || r.starts_with("zs1") || r.starts_with("t1"))
            && !looks_test
            && !looks_regtest;
        let hint = match network {
            WalletNetwork::Main if looks_test => " — this looks like a testnet address but you are on mainnet",
            WalletNetwork::Main if looks_regtest => " — this looks like a devnet (regtest) address but you are on mainnet",
            WalletNetwork::Test if looks_main => " — this looks like a mainnet address but you are on testnet",
            WalletNetwork::Test if looks_regtest => " — this looks like a devnet (regtest) address but you are on testnet",
            WalletNetwork::Regtest(_) if looks_main => " — this looks like a mainnet address but you are on the local devnet",
            WalletNetwork::Regtest(_) if looks_test => " — this looks like a testnet address but you are on the local devnet",
            _ => "",
        };
        CoreError::Crypto(format!("invalid recipient address{hint}"))
    })?;
    // A transparent recipient means this is an unshield (Orchard → transparent).
    let is_unshield = matches!(to, Address::Transparent(_));
    let amount =
        Zatoshis::from_u64(amount_zatoshis).map_err(|e| CoreError::Crypto(format!("amount: {e}")))?;

    // Memos are only valid for shielded (Orchard) outputs; transparent outputs
    // carry no memo. Silently drop any memo supplied for an unshield.
    let memo_bytes: Option<MemoBytes> = if is_unshield {
        None
    } else {
        memo.as_deref().filter(|s| !s.is_empty()).map(|s| {
            s.parse::<Memo>()
                .map(|m| m.encode())
                .unwrap_or_else(|_| MemoBytes::empty())
        })
    };

    // Let the backend choose the transaction version from the target height:
    // version 6 (carrying an Ironwood bundle) once NU6.3 is active, version 5
    // before it. Passing `None` is what enables Ironwood sends — post-NU6.3 the
    // Orchard pool is sealed, so a shielded payment to a unified address is
    // delivered through the Ironwood bundle of a V6 transaction. `lock_inputs`
    // is None: Cyze builds and signs one transaction at a time, so there is no
    // need to reserve inputs across concurrent proposals.
    let proposal = propose_standard_transfer_to_address::<_, _, std::convert::Infallible>(
        &mut db,
        &params,
        StandardFeeRule::Zip317,
        account_id,
        ConfirmationsPolicy::default(),
        &to,
        amount,
        memo_bytes,
        None, // change memo
        ShieldedPool::Orchard,
        None, // lock_inputs
        None, // proposed_version: derive V5/V6 from the target height
    )
    .map_err(|e| CoreError::Ceremony(format!("propose transfer: {e:?}")))?;

    let fee = u64::from(proposal.steps().last().balance().fee_required());

    let pczt = create_pczt_from_proposal::<_, _, std::convert::Infallible, _, std::convert::Infallible, _>(
        &mut db,
        &params,
        account_id,
        OvkPolicy::Sender,
        &proposal,
        None, // expiry_height: use the proposal's default expiry
        // Orchard-bundle padding; the Ironwood bundle derives its own from the
        // proposal so it matches the action count the fee was computed from.
        zcash_primitives::transaction::builder::BundlePadding::DEFAULT,
    )
    .map_err(|e| CoreError::Ceremony(format!("create pczt: {e:?}")))?;

    // Ironwood cohort: Pczt::serialize now consumes self and returns Result
    // (postcard EncodingError). Serialize a clone since `pczt` is still needed
    // below for the sighash and spend extraction.
    let pczt_hex = hex::encode(
        pczt.clone()
            .serialize()
            .map_err(|e| CoreError::Ceremony(format!("serialize pczt: {e:?}")))?,
    );

    let sighash = pczt::roles::signer::Signer::new(pczt.clone())
        .map_err(|e| CoreError::Ceremony(format!("signer: {e:?}")))?
        .shielded_sighash();

    // Read each real spend's α (the re-randomization the FROST signers must
    // use), across both the Orchard and Ironwood bundles.
    let spends = spends_to_sign(pczt)?;

    Ok(DraftTransaction {
        pczt_hex,
        sighash_hex: hex::encode(sighash),
        spends,
        fee_zatoshis: fee,
        amount_zatoshis,
        recipient: recipient.to_string(),
        is_unshield,
        memo: if is_unshield { None } else { memo.filter(|s| !s.is_empty()) },
    })
}

/// Build an unsigned coinholder-poll **vote** as a draft transaction: a shielded
/// payment carrying the encoded Vote Cast Memo to the poll's reception Z-address.
/// Returns the same [`DraftTransaction`] a normal send does, so the existing
/// FROST signing + broadcast path casts the vote unchanged.
///
/// `ballot_definition` is the poll's ballot bytes *exactly as published* (hashed
/// for the memo's `poll-hash`); `question_shapes` describes each question so the
/// votes can be validated; `votes` is one [`VoteEntry`] per question. The vote is
/// validated before anything is built — an invalid memo is counted as all-abstain
/// on-chain, so it must never be cast.
///
/// `amount_zatoshis` is the value delivered to the reception address alongside
/// the memo. Vote **weight** is derived by the poll's tally from a shielded
/// balance snapshot, *not* from this amount, so callers pass the minimum the poll
/// requires (often a dust amount) rather than staking real value here. The group
/// must simply hold its balance through the poll's snapshot window to be counted.
#[allow(clippy::too_many_arguments)]
pub async fn prepare_vote(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    reception_address: &str,
    ballot_definition: &[u8],
    question_shapes: &[crate::voting::QuestionShape],
    votes: &[crate::voting::VoteEntry],
    amount_zatoshis: u64,
    lightwalletd_url: &str,
    db_key: &[u8],
) -> Result<DraftTransaction, CoreError> {
    // Validate + encode before touching the wallet or network: a malformed vote
    // must never be broadcast (it would be tallied as all-abstain).
    crate::voting::validate_votes(votes, question_shapes)?;
    let poll_hash = crate::voting::poll_hash(ballot_definition);
    let memo = crate::voting::encode_vote_memo(&poll_hash, votes)?;

    prepare_send(
        data_dir,
        group_id,
        network,
        reception_address,
        amount_zatoshis,
        Some(memo),
        lightwalletd_url,
        db_key,
    )
    .await
}

/// Collect every spend the group must FROST-sign, across both the Orchard and
/// Ironwood bundles, tagged with its pool and α. Requires orchard's
/// `unstable-frost` feature (which exposes `spend().alpha()`). Orchard spends are
/// listed first, then Ironwood; the order only needs to be stable, since each
/// spend carries its own pool + index for application.
///
/// A spend needs the group's signature iff it still lacks a `spend_auth_sig`:
/// this runs after `create_pczt_from_proposal`, whose IO Finalizer has already
/// signed every dummy padding spend from its `dummy_sk`, so the only spends left
/// unsigned are the wallet's own notes. Each carries an α to sign with. The old
/// heuristic keyed on `value != 0`, which wrongly skipped the **zero-value real
/// spend** the turnstile-out construction adds to the Orchard bundle — leaving it
/// unauthorized and failing extraction with `MissingSpendAuthSig`.
fn spends_to_sign(pczt: pczt::Pczt) -> Result<Vec<SpendToSign>, CoreError> {
    use ff::PrimeField;
    use pczt::roles::low_level_signer::{OrchardParseError, Signer};

    // Append every not-yet-signed spend of one already-parsed bundle, tagging
    // its pool. `spend_auth_sig().is_none()` selects exactly the spends the group
    // must authorize (dummies were already signed by the IO Finalizer); `alpha`
    // is the re-randomization those signatures must use.
    fn collect(bundle: &orchard::pczt::Bundle, pool: SpendPool, out: &mut Vec<SpendToSign>) {
        for (index, action) in bundle.actions().iter().enumerate() {
            let spend = action.spend();
            if spend.spend_auth_sig().is_none() {
                if let Some(alpha) = spend.alpha() {
                    out.push(SpendToSign {
                        pool,
                        index,
                        alpha_hex: hex::encode(alpha.to_repr()),
                    });
                }
            }
        }
    }

    let mut spends = Vec::new();
    let signer = Signer::new(pczt)
        .sign_orchard_with(|_pczt, bundle, _| {
            collect(bundle, SpendPool::Orchard, &mut spends);
            Ok::<_, OrchardParseError>(())
        })
        .map_err(|e: OrchardParseError| CoreError::Ceremony(format!("read orchard spends: {e:?}")))?;
    signer
        .sign_ironwood_with(|_pczt, bundle, _| {
            collect(bundle, SpendPool::Ironwood, &mut spends);
            Ok::<_, OrchardParseError>(())
        })
        .map_err(|e: OrchardParseError| CoreError::Ceremony(format!("read ironwood spends: {e:?}")))?;
    Ok(spends)
}

/// Whether the PCZT carries any actions in each pool's bundle, as
/// `(orchard, ironwood)`. Used to prove only the bundles a transaction actually
/// has — proving an empty bundle fails its anchor check.
fn bundle_presence(pczt: &pczt::Pczt) -> Result<(bool, bool), CoreError> {
    use pczt::roles::low_level_signer::{OrchardParseError, Signer};

    let mut has_orchard = false;
    let mut has_ironwood = false;
    let signer = Signer::new(pczt.clone())
        .sign_orchard_with(|_pczt, bundle, _| {
            has_orchard = !bundle.actions().is_empty();
            Ok::<_, OrchardParseError>(())
        })
        .map_err(|e: OrchardParseError| CoreError::Ceremony(format!("read orchard bundle: {e:?}")))?;
    signer
        .sign_ironwood_with(|_pczt, bundle, _| {
            has_ironwood = !bundle.actions().is_empty();
            Ok::<_, OrchardParseError>(())
        })
        .map_err(|e: OrchardParseError| CoreError::Ceremony(format!("read ironwood bundle: {e:?}")))?;
    Ok((has_orchard, has_ironwood))
}

/// Find every shielded spend that still lacks a spend-authorization signature,
/// across both Orchard-protocol bundles, described precisely.
///
/// The `TransactionExtractor` requires a `spend_auth_sig` on *every* action and
/// otherwise fails late and opaquely with `MissingSpendAuthSig`. By the time we
/// reach it, dummy padding spends have been signed by the IO Finalizer (it
/// consumes their `dummy_sk`) and real spends have been FROST-signed and applied
/// ([`apply_signatures`]). Anything still unsigned is a bug in *this* signing
/// path — most likely a real spend that [`spends_to_sign`] failed to enumerate,
/// or a dummy the IO Finalizer skipped — and we want to surface it here, before
/// the expensive proving/broadcast, with enough detail to fix it. The returned
/// strings distinguish the two cases (a real spend still carrying `value`/`alpha`
/// vs. a dummy whose `dummy_sk` was never consumed).
fn find_unsigned_spends(pczt: &pczt::Pczt) -> Result<Vec<String>, CoreError> {
    use orchard::value::NoteValue;
    use pczt::roles::low_level_signer::{OrchardParseError, Signer};

    fn scan(bundle: &orchard::pczt::Bundle, pool: &str, out: &mut Vec<String>) {
        for (index, action) in bundle.actions().iter().enumerate() {
            let spend = action.spend();
            if spend.spend_auth_sig().is_some() {
                continue; // already authorized
            }
            let is_real = spend.value().is_some_and(|v| v != NoteValue::default());
            let has_alpha = spend.alpha().is_some();
            let has_dummy_sk = spend.dummy_sk().is_some();
            let kind = if is_real {
                "REAL spend never FROST-signed (spends_to_sign missed it?)"
            } else if has_dummy_sk {
                "dummy with unconsumed dummy_sk (IO Finalizer skipped it?)"
            } else {
                "zero-value spend left unsigned"
            };
            out.push(format!(
                "{pool} bundle action {index}: {kind} [alpha_present={has_alpha}]"
            ));
        }
    }

    let mut unsigned = Vec::new();
    let signer = Signer::new(pczt.clone())
        .sign_orchard_with(|_pczt, bundle, _| {
            scan(bundle, "orchard", &mut unsigned);
            Ok::<_, OrchardParseError>(())
        })
        .map_err(|e: OrchardParseError| CoreError::Ceremony(format!("read orchard bundle: {e:?}")))?;
    signer
        .sign_ironwood_with(|_pczt, bundle, _| {
            scan(bundle, "ironwood", &mut unsigned);
            Ok::<_, OrchardParseError>(())
        })
        .map_err(|e: OrchardParseError| CoreError::Ceremony(format!("read ironwood bundle: {e:?}")))?;
    Ok(unsigned)
}

/// Apply FROST-produced spend-auth signatures to a draft PCZT, returning the
/// signed PCZT (hex). Each entry is `(pool, action index, 64-byte sig hex)`; the
/// pool selects which bundle the signature is applied to, so a turnstile
/// transaction that spends from both pools is signed correctly.
pub fn apply_signatures(
    pczt_hex: &str,
    sighash_hex: &str,
    signatures: Vec<(SpendPool, usize, String)>,
) -> Result<String, CoreError> {
    use orchard::primitives::redpallas::{Signature, SpendAuth};
    use pczt::roles::low_level_signer::{OrchardParseError, Signer};

    let pczt = pczt::Pczt::parse(
        &hex::decode(pczt_hex.trim()).map_err(|e| CoreError::Ceremony(format!("pczt hex: {e}")))?,
    )
    .map_err(|e| CoreError::Ceremony(format!("parse pczt: {e:?}")))?;
    let sighash: [u8; 32] = hex::decode(sighash_hex.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| CoreError::Ceremony("sighash must be 32 bytes hex".into()))?;

    // Decode and split the signatures by pool.
    let mut orchard_sigs: Vec<(usize, Signature<SpendAuth>)> = Vec::new();
    let mut ironwood_sigs: Vec<(usize, Signature<SpendAuth>)> = Vec::new();
    for (pool, idx, sig_hex) in signatures {
        let bytes: [u8; 64] = hex::decode(sig_hex.trim())
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| CoreError::Ceremony("signature must be 64 bytes hex".into()))?;
        let sig = Signature::<SpendAuth>::from(bytes);
        match pool {
            SpendPool::Orchard => orchard_sigs.push((idx, sig)),
            SpendPool::Ironwood => ironwood_sigs.push((idx, sig)),
        }
    }

    // Applying a signature to the wrong action index is a hard error rather than
    // a bad-signature rejection at broadcast, so surface it precisely.
    let mut apply_err: Option<String> = None;
    let apply = |bundle: &mut orchard::pczt::Bundle,
                 sigs: &[(usize, Signature<SpendAuth>)],
                 pool: &str,
                 err: &mut Option<String>| {
        for (idx, sig) in sigs {
            if let Err(e) = bundle.actions_mut()[*idx].apply_signature(sighash, sig.clone()) {
                *err = Some(format!("{pool} spend {idx}: {e:?}"));
                break;
            }
        }
    };

    let signer = Signer::new(pczt)
        .sign_orchard_with(|_pczt, bundle, _| {
            apply(bundle, &orchard_sigs, "orchard", &mut apply_err);
            Ok::<_, OrchardParseError>(())
        })
        .map_err(|e: OrchardParseError| CoreError::Ceremony(format!("apply orchard: {e:?}")))?;
    let signer = signer
        .sign_ironwood_with(|_pczt, bundle, _| {
            apply(bundle, &ironwood_sigs, "ironwood", &mut apply_err);
            Ok::<_, OrchardParseError>(())
        })
        .map_err(|e: OrchardParseError| CoreError::Ceremony(format!("apply ironwood: {e:?}")))?;
    if let Some(e) = apply_err {
        return Err(CoreError::Ceremony(format!("invalid signature for {e}")));
    }
    let signed = signer.finish();

    // Completeness guard: every shielded spend must now be authorized, or the
    // TransactionExtractor fails late and opaquely with `MissingSpendAuthSig`
    // (and only after the expensive proving step). Catch it here instead, naming
    // the exact unsigned action(s), so a signing-path gap surfaces early and
    // actionably rather than as a doomed broadcast. No transaction is built.
    let unsigned = find_unsigned_spends(&signed)?;
    if !unsigned.is_empty() {
        return Err(CoreError::Ceremony(format!(
            "not all spends were authorized, so this transaction would be rejected at \
             broadcast (MissingSpendAuthSig). Unsigned: {}. No transaction was built or \
             sent. This is a bug in the pool-aware signing path — please report it with \
             this message.",
            unsigned.join("; ")
        )));
    }

    // Pczt::serialize consumes self and returns Result (postcard EncodingError).
    Ok(hex::encode(signed.serialize().map_err(|e| {
        CoreError::Ceremony(format!("serialize pczt: {e:?}"))
    })?))
}

/// Prove, finalize, and broadcast a fully spend-auth-signed PCZT, returning the
/// transaction id. The Orchard proof step is CPU-heavy (building the proving
/// key takes several seconds), so it runs on a blocking thread.
///
/// This is the final leg of the send pipeline: the group has already applied
/// its threshold signature to every spend ([`apply_signatures`]); here
/// we attach the zero-knowledge proof, finalize, extract the transaction (which
/// creates the binding signature), and submit it to lightwalletd.
pub async fn broadcast_signed(
    signed_pczt_hex: &str,
    network: WalletNetwork,
    url: &str,
) -> Result<String, CoreError> {
    let pczt = pczt::Pczt::parse(
        &hex::decode(signed_pczt_hex.trim())
            .map_err(|e| CoreError::Ceremony(format!("pczt hex: {e}")))?,
    )
    .map_err(|e| CoreError::Ceremony(format!("parse pczt: {e:?}")))?;

    // Prove only the bundles this transaction actually carries: a turnstile send
    // has both an Orchard bundle (the spends) and an Ironwood bundle (the
    // outputs); a pure Ironwood send has only Ironwood; a legacy unshield only
    // Orchard. Proving an empty bundle fails its anchor check, so gate on it.
    let (has_orchard, has_ironwood) = bundle_presence(&pczt)?;

    // Select the Orchard circuit from the consensus branch active at the live
    // chain tip, so the proof matches what THIS network's validators expect
    // (PostNu6_3 on Ironwood, FixedPostNu6_2 pre-NU6.3). A stale or hardcoded
    // circuit produces a proof the network rejects. This runs seconds before
    // broadcast, so the tip is effectively the tx's mined branch. Both the
    // Orchard and Ironwood pools share this circuit, so one key serves both.
    let mut client = connect(url).await?;
    let tip_height = client
        .get_latest_block(ChainSpec {})
        .await
        .map_err(|e| CoreError::Connection(format!("get_latest_block: {e}")))?
        .into_inner()
        .height;
    let circuit_version = orchard_circuit_version_for_height(network, tip_height);

    // Proving + finalize + extract is synchronous, CPU-bound work; keep it off
    // the async runtime so progress events and other tasks stay responsive.
    let (raw, txid) = tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, String), CoreError> {
        use orchard::circuit::{ProvingKey, VerifyingKey};
        use pczt::roles::{
            prover::Prover, spend_finalizer::SpendFinalizer, tx_extractor::TransactionExtractor,
        };

        // `circuit_version` is chosen from the live chain tip above and captured
        // by this move closure; one proving/verifying key serves both the
        // Orchard and Ironwood bundles, which share that circuit.
        let pk = ProvingKey::build(circuit_version);

        // 1. Zero-knowledge proof for each present bundle.
        let mut prover = Prover::new(pczt);
        if has_orchard {
            prover = prover
                .create_orchard_proof(&pk)
                .map_err(|e| CoreError::Ceremony(format!("orchard proof: {e:?}")))?;
        }
        if has_ironwood {
            prover = prover
                .create_ironwood_proof(&pk)
                .map_err(|e| CoreError::Ceremony(format!("ironwood proof: {e:?}")))?;
        }
        let pczt = prover.finish();

        // 2. Finalize spends (spend-auth signatures are already applied).
        let pczt = SpendFinalizer::new(pczt)
            .finalize_spends()
            .map_err(|e| CoreError::Ceremony(format!("finalize spends: {e:?}")))?;

        // 3. Extract the final transaction (creates the binding signature). The
        // extractor verifies both the Orchard and Ironwood bundles with this one
        // vk, since they share the PostNu6_3 circuit.
        let vk = VerifyingKey::build(circuit_version);
        let tx = TransactionExtractor::new(pczt)
            .with_orchard(&vk)
            .extract()
            .map_err(|e| CoreError::Ceremony(format!("extract transaction: {e:?}")))?;

        let txid = format!("{}", tx.txid());
        let mut raw = Vec::new();
        tx.write(&mut raw)
            .map_err(|e| CoreError::Ceremony(format!("serialize transaction: {e}")))?;
        Ok((raw, txid))
    })
    .await
    .map_err(|e| CoreError::Ceremony(format!("proving task panicked: {e}")))??;

    // 4. Submit to lightwalletd (reusing the connection opened above).
    let resp = client
        .send_transaction(zcash_client_backend::proto::service::RawTransaction { data: raw, height: 0 })
        .await
        .map_err(|e| CoreError::Connection(format!("send_transaction: {e}")))?
        .into_inner();
    if resp.error_code != 0 {
        return Err(CoreError::Connection(format!(
            "lightwalletd rejected the transaction (code {}): {}",
            resp.error_code, resp.error_message
        )));
    }
    Ok(txid)
}

/// A single transaction as seen from this wallet's perspective.
#[derive(Debug, Clone, Serialize)]
pub struct TxRecord {
    /// Transaction ID, hex, in display order (bytes reversed vs. on-disk storage).
    pub txid: String,
    /// Block height when mined; `None` for pending/unconfirmed.
    pub block_height: Option<u64>,
    /// Unix timestamp (seconds since epoch) from the mined block; `None` when unconfirmed.
    pub timestamp: Option<i64>,
    /// `"receive"` or `"send"`.
    pub direction: String,
    /// Value in zatoshis (always positive; for sends this is the total value
    /// of the output(s) created, not including change returned to the wallet).
    pub amount_zatoshis: u64,
    /// Network fee paid, if known (only present for sends created by this wallet).
    pub fee_zatoshis: Option<u64>,
    /// Decoded memo text, if one was attached to this transaction.
    pub memo: Option<String>,
    /// Recipient unified address for sends; `None` for self-transfers (note consolidation).
    pub recipient: Option<String>,
}

/// Read on-chain transaction history for a group's wallet — received funds and
/// sent transactions, newest confirmed first.
///
/// Uses direct SQLite queries because `zcash_client_backend 0.23` exposes no
/// clean transaction-list API on `WalletRead`. The tables queried are stable
/// parts of `zcash_client_sqlite`'s schema: `transactions`, `accounts`,
/// `orchard_received_notes`, and `sent_notes`.
/// Total number of shielded notes this group has ever received, across both the
/// Orchard and Ironwood pools. Used as a coarse "wallet activity" signal to
/// decide when to rotate the receive address (#3): once the count grows, the
/// currently-shown address may have been paid, so the next view hands out a
/// fresh diversifier. Returns 0 when no wallet db exists.
///
/// Counting Ironwood too matters post-NU6.3: a payment to the group's unified
/// address now lands in the Ironwood pool, so an Orchard-only count would never
/// grow on a new receive and the address would never rotate — silently reusing
/// one address across payments.
pub fn count_received_notes(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    db_key: &[u8],
) -> Result<u64, CoreError> {
    let (db_path, _) = wallet_paths(data_dir, group_id, network);
    if !db_path.exists() {
        return Ok(0);
    }
    let conn = open_readonly_connection(&db_path, db_key)?;
    let count: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM orchard_received_notes) \
                  + (SELECT COUNT(*) FROM ironwood_received_notes)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| CoreError::Crypto(format!("count received notes: {e}")))?;
    Ok(count.max(0) as u64)
}

/// A single unspent Orchard note that makes up part of the group's balance.
#[derive(Debug, Clone, Serialize)]
pub struct NoteRecord {
    /// Receiving transaction id (hex, display order) the note arrived in.
    pub received_txid: String,
    /// Note value in zatoshis.
    pub value_zatoshis: u64,
    /// `"spendable"` (confirmed, unspent), `"pending"` (unconfirmed incoming),
    /// or `"spending"` (a broadcast-but-unmined send is consuming it).
    pub status: String,
    /// Block height the note was received at; `None` while unconfirmed.
    pub received_height: Option<u64>,
    /// Confirmations so far (chain tip − received height + 1); 0 if unconfirmed.
    pub confirmations: u64,
    /// True when this note is change returned to the group by one of its sends.
    pub is_change: bool,
    /// Decoded memo, if any.
    pub memo: Option<String>,
}

/// List the unspent Orchard notes that comprise the group's balance, newest/
/// largest first. Notes already spent in a mined transaction are excluded.
/// Powers the "Review Notes" view: each note is one spend authorization, so the
/// count is also the number of FROST signing rounds a full-balance send needs.
pub fn wallet_notes(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    db_key: &[u8],
) -> Result<Vec<NoteRecord>, CoreError> {
    let (db_path, _) = wallet_paths(data_dir, group_id, network);
    if !db_path.exists() {
        return Ok(vec![]);
    }
    let conn = open_readonly_connection(&db_path, db_key)?;

    use rusqlite::OptionalExtension;
    let account_id: Option<i64> = conn
        .query_row("SELECT id FROM accounts LIMIT 1", [], |row| row.get(0))
        .optional()
        .map_err(|e| CoreError::Crypto(format!("get account id: {e}")))?;
    let Some(account_id) = account_id else {
        return Ok(vec![]);
    };
    let tip: Option<i64> = conn
        .query_row("SELECT MAX(height) FROM blocks", [], |row| row.get(0))
        .optional()
        .map_err(|e| CoreError::Crypto(format!("chain tip: {e}")))?
        .flatten();
    let tip = tip.unwrap_or(0);

    // Post-NU6.3 a group holds notes in two pools: the sealed Orchard pool and
    // the Ironwood pool that all new value (and migrated funds) land in. The two
    // tables are structurally identical, so query each with the same shape and
    // merge. Table/column names are compile-time constants, not user input, so
    // interpolating them into the SQL is safe.
    let mut notes = Vec::new();
    for (notes_table, spends_table, fk_col) in [
        ("orchard_received_notes", "orchard_received_note_spends", "orchard_received_note_id"),
        ("ironwood_received_notes", "ironwood_received_note_spends", "ironwood_received_note_id"),
    ] {
        let sql = format!(
            "SELECT t.txid, orn.value, orn.is_change, orn.memo, t.mined_height, \
             MAX(CASE WHEN spend_t.mined_height IS NOT NULL THEN 1 ELSE 0 END) AS spent_mined, \
             MAX(CASE WHEN s.{fk_col} IS NOT NULL THEN 1 ELSE 0 END) AS has_spend \
             FROM {notes_table} orn \
             JOIN transactions t ON orn.transaction_id = t.id_tx \
             LEFT JOIN {spends_table} s ON s.{fk_col} = orn.id \
             LEFT JOIN transactions spend_t ON spend_t.id_tx = s.transaction_id \
             WHERE orn.account_id = ?1 \
             GROUP BY orn.id"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| CoreError::Crypto(format!("prepare notes query: {e}")))?;

        let rows = stmt
            .query_map([account_id], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                    row.get::<_, Option<u64>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })
            .map_err(|e| CoreError::Crypto(format!("execute notes query: {e}")))?;

        for row in rows {
            let (mut txid_bytes, value, is_change, memo_bytes, received_height, spent_mined, has_spend) =
                row.map_err(|e| CoreError::Crypto(format!("note row: {e}")))?;
            // A note spent in a mined transaction is gone — not part of the balance.
            if spent_mined == 1 {
                continue;
            }
            txid_bytes.reverse();
            let confirmations = match received_height {
                Some(h) if tip as u64 >= h => tip as u64 - h + 1,
                _ => 0,
            };
            let status = if received_height.is_none() {
                "pending" // incoming, not yet mined
            } else if has_spend == 1 {
                "spending" // a broadcast-but-unmined send is consuming it
            } else {
                "spendable"
            };
            notes.push(NoteRecord {
                received_txid: hex::encode(&txid_bytes),
                value_zatoshis: value,
                status: status.to_string(),
                received_height,
                confirmations,
                is_change: is_change != 0,
                memo: memo_bytes.as_deref().and_then(decode_zcash_memo),
            });
        }
    }
    // Largest notes first, across both pools.
    notes.sort_by(|a, b| b.value_zatoshis.cmp(&a.value_zatoshis));
    Ok(notes)
}

pub fn wallet_history(
    data_dir: &Path,
    group_id: &str,
    network: WalletNetwork,
    db_key: &[u8],
) -> Result<Vec<TxRecord>, CoreError> {
    let (db_path, _) = wallet_paths(data_dir, group_id, network);
    if !db_path.exists() {
        return Ok(vec![]);
    }

    let conn = open_readonly_connection(&db_path, db_key)?;

    // There is at most one account per group wallet.
    use rusqlite::OptionalExtension;
    let account_id: Option<i64> = conn
        .query_row("SELECT id FROM accounts LIMIT 1", [], |row| row.get(0))
        .optional()
        .map_err(|e| CoreError::Crypto(format!("get account id: {e}")))?;
    let Some(account_id) = account_id else {
        return Ok(vec![]);
    };

    let mut records: Vec<TxRecord> = Vec::new();

    // ── Received ────────────────────────────────────────────────────────────
    // Shielded notes for our account that are not change (is_change = 0 means
    // this note arrived in a transaction that we did NOT also spend from —
    // i.e., someone else sent us funds). Post-NU6.3 a receive lands in the
    // Ironwood pool while legacy receipts are in Orchard, so union both note
    // tables (identical columns) before grouping — one tx = one history entry,
    // even if a tx somehow deposited into both pools. Sum the note values, pick
    // the first real memo.
    {
        // A derived table unioning both pools' non-dummy columns. Table names are
        // compile-time constants, so this static SQL carries no injection risk.
        const RECEIVED_NOTES_UNION: &str = "( \
            SELECT transaction_id, account_id, value, is_change, memo FROM orchard_received_notes \
            UNION ALL \
            SELECT transaction_id, account_id, value, is_change, memo FROM ironwood_received_notes )";
        let sql = format!(
            "SELECT t.txid, t.mined_height, b.time, SUM(rn.value), \
             ( SELECT rn2.memo \
               FROM {RECEIVED_NOTES_UNION} rn2 \
               WHERE rn2.transaction_id = t.id_tx \
                 AND rn2.account_id = ?1 \
                 AND rn2.is_change = 0 \
                 AND rn2.memo IS NOT NULL \
               LIMIT 1 ), \
             ( SELECT vt.fee_paid FROM v_transactions vt WHERE vt.txid = t.txid LIMIT 1 ) \
             FROM {RECEIVED_NOTES_UNION} rn \
             JOIN transactions t ON rn.transaction_id = t.id_tx \
             LEFT JOIN blocks b ON b.height = t.mined_height \
             WHERE rn.account_id = ?1 AND rn.is_change = 0 \
             GROUP BY t.id_tx \
             HAVING SUM(rn.value) > 0 \
             ORDER BY t.mined_height DESC NULLS LAST"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| CoreError::Crypto(format!("prepare receive query: {e}")))?;

        let rows = stmt
            .query_map([account_id], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Option<u64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            })
            .map_err(|e| CoreError::Crypto(format!("execute receive query: {e}")))?;

        for row in rows {
            let (mut txid_bytes, block_height, timestamp, amount, memo_bytes, fee_paid) =
                row.map_err(|e| CoreError::Crypto(format!("receive row: {e}")))?;
            // zcash_client_sqlite stores txid in internal byte order; the
            // conventional display representation (block explorers, CLI) is
            // byte-reversed.
            txid_bytes.reverse();
            records.push(TxRecord {
                txid: hex::encode(&txid_bytes),
                block_height,
                timestamp,
                direction: "receive".to_string(),
                amount_zatoshis: amount,
                // The fee is on-chain and public. The wallet knows it whenever it
                // has the full transaction (v_transactions.fee_paid); it stays
                // None only when the sender's inputs were never fetched, in which
                // case the UI says the sender paid it rather than showing nothing.
                fee_zatoshis: fee_paid.map(|f| f.max(0) as u64),
                memo: memo_bytes.as_deref().and_then(decode_zcash_memo),
                recipient: None,
            });
        }
    }

    // ── Sent ────────────────────────────────────────────────────────────────
    // `sent_notes` only gets a row for the external recipient's output if this
    // wallet's *outgoing viewing key* successfully re-decrypts it during chain
    // scanning — that's best-effort and not always true (it depends on when the
    // account was imported/rescanned). Deriving the sent amount by summing
    // `sent_notes` therefore silently falls back to just the change value when
    // that recovery fails, which understates or (worse) overstates the amount.
    //
    // `v_transactions.account_balance_delta` has no such gap: it is the net
    // change in this account's balance for the tx, built purely from notes we
    // know we *spent* (nullifier-based) and notes we *received* as change
    // (our own IVK) — both always reliable regardless of OVK recovery. Per
    // that view's own documented contract (see the module doc comment in
    // zcash_client_sqlite for `v_transactions`), for a single-account wallet
    // the amount sent to addresses outside the wallet is
    // `-(account_balance_delta) - fee_paid` when `account_balance_delta < 0`.
    // The recipient address/memo are still opportunistically read from
    // `sent_notes` when that OVK recovery *did* succeed; otherwise they're
    // simply absent (shown as "-" by the UI) rather than causing a wrong amount.
    {
        let mut stmt = conn
            .prepare(
                "SELECT vt.txid, vt.mined_height, vt.block_time, vt.fee_paid, vt.account_balance_delta, \
                 MAX(sn.to_address) AS ext_address, \
                 ( SELECT sn2.memo \
                   FROM sent_notes sn2 \
                   WHERE sn2.transaction_id = t.id_tx \
                     AND sn2.from_account_id = ?1 \
                     AND sn2.to_account_id IS NULL \
                     AND sn2.memo IS NOT NULL \
                   LIMIT 1 ) \
                 FROM v_transactions vt \
                 JOIN transactions t ON t.txid = vt.txid \
                 LEFT JOIN sent_notes sn \
                   ON sn.transaction_id = t.id_tx \
                   AND sn.from_account_id = ?1 \
                   AND sn.to_account_id IS NULL \
                 WHERE vt.account_uuid = (SELECT uuid FROM accounts WHERE id = ?1) \
                   AND vt.account_balance_delta < 0 \
                 GROUP BY t.id_tx \
                 ORDER BY vt.mined_height DESC NULLS LAST",
            )
            .map_err(|e| CoreError::Crypto(format!("prepare send query: {e}")))?;

        let rows = stmt
            .query_map([account_id], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Option<u64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<Vec<u8>>>(6)?,
                ))
            })
            .map_err(|e| CoreError::Crypto(format!("execute send query: {e}")))?;

        for row in rows {
            let (mut txid_bytes, block_height, timestamp, fee_paid, account_balance_delta, ext_address, memo_bytes) =
                row.map_err(|e| CoreError::Crypto(format!("send row: {e}")))?;
            txid_bytes.reverse();
            // account_balance_delta < 0 (guaranteed by the WHERE clause) is the
            // total decrease in our balance: amount sent externally + fee.
            let debit = account_balance_delta.unsigned_abs();
            let fee = fee_paid.map(|f| f.max(0) as u64).unwrap_or(0);
            let amount = debit.saturating_sub(fee);
            records.push(TxRecord {
                txid: hex::encode(&txid_bytes),
                block_height,
                timestamp,
                direction: "send".to_string(),
                amount_zatoshis: amount,
                fee_zatoshis: fee_paid.map(|f| f.max(0) as u64),
                memo: memo_bytes.as_deref().and_then(decode_zcash_memo),
                recipient: ext_address,
            });
        }
    }

    // Merge and sort: confirmed newest first, then pending (no block).
    records.sort_by(|a, b| match (b.block_height, a.block_height) {
        (Some(bh), Some(ah)) => bh.cmp(&ah),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    Ok(records)
}

/// Decode a raw Zcash memo blob (up to 512 bytes) to a UTF-8 string.
/// The 0xF6 sentinel byte signals an explicitly empty memo; all-zero padding
/// is also treated as absent. Returns `None` for either case.
fn decode_zcash_memo(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes[0] == 0xF6 {
        return None;
    }
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim_end_matches('\0').trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

/// The receiving unified address for a UFVK string, encoded for `network`.
/// This is what the wallet's account would expose for receiving funds.
pub fn ufvk_default_address(network: WalletNetwork, ufvk: &str) -> Result<String, CoreError> {
    use zcash_keys::keys::{UnifiedAddressRequest, UnifiedFullViewingKey};
    let params = network.params();
    let ufvk = UnifiedFullViewingKey::decode(&params, ufvk)
        .map_err(|e| CoreError::Crypto(format!("invalid UFVK: {e}")))?;
    let (address, _) = ufvk
        .default_address(UnifiedAddressRequest::AllAvailableKeys)
        .map_err(|e| CoreError::Crypto(format!("address generation: {e}")))?;
    Ok(address.encode(&params))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_params_and_defaults() {
        assert_eq!(WalletNetwork::Test.params(), WalletParams::Consensus(Network::TestNetwork));
        assert_eq!(WalletNetwork::Main.params(), WalletParams::Consensus(Network::MainNetwork));
        assert!(WalletNetwork::Test.default_lightwalletd().starts_with("https://"));
        assert!(WalletNetwork::Main.default_lightwalletd().starts_with("https://"));

        // Regtest: local params, regtest encoding, its own data dir, and a
        // loopback plaintext default that the endpoint guard accepts.
        let dev = WalletNetwork::Regtest(RegtestUpgrade::Nu6);
        assert!(matches!(dev.params(), WalletParams::Local(_)));
        assert_eq!(dev.network_type(), NetworkType::Regtest);
        assert_eq!(dev.dir_name(), "regtest");
        assert!(validate_endpoint_security(dev.default_lightwalletd()).is_ok());
        assert_eq!(WalletNetwork::from_str("regtest"), dev);
        assert_eq!(WalletNetwork::from_str(dev.as_str()), dev);
    }

    #[test]
    fn regtest_upgrade_from_branch_id() {
        // The branch ids a node reports for each upgrade we can build for.
        assert_eq!(RegtestUpgrade::from_branch_id("c8e71055"), Some(RegtestUpgrade::Nu6));
        assert_eq!(RegtestUpgrade::from_branch_id("4dec4df0"), Some(RegtestUpgrade::Nu6_1));
        assert_eq!(RegtestUpgrade::from_branch_id("5437F330"), Some(RegtestUpgrade::Nu6_2));
        assert_eq!(RegtestUpgrade::from_branch_id(NU6_3_BRANCH_ID), Some(RegtestUpgrade::Nu6_3));
        // NU5 (and anything unknown) is not a level we model.
        assert_eq!(RegtestUpgrade::from_branch_id("c2d6d0b4"), None);
        assert_eq!(RegtestUpgrade::from_branch_id(""), None);
        for u in RegtestUpgrade::ALL {
            assert_eq!(RegtestUpgrade::parse(u.as_str()), Some(u));
        }
    }

    #[test]
    fn regtest_params_activate_only_up_to_level() {
        // Each level must make the wallet agree with the node on the branch id
        // at the tip: that is what picks the tx version and Orchard circuit.
        for (level, branch) in [
            (RegtestUpgrade::Nu6, "c8e71055"),
            (RegtestUpgrade::Nu6_1, "4dec4df0"),
            (RegtestUpgrade::Nu6_2, "5437f330"),
            (RegtestUpgrade::Nu6_3, NU6_3_BRANCH_ID),
        ] {
            let net = WalletNetwork::Regtest(level);
            assert_eq!(branch_id_for_height(net, 500), branch, "{level:?}");
            assert_eq!(RegtestUpgrade::from_branch_id(branch), Some(level));
        }
        let nu6 = RegtestUpgrade::Nu6.local_network();
        assert_eq!(nu6.activation_height(NetworkUpgrade::Nu5), Some(BlockHeight::from_u32(1)));
        assert_eq!(nu6.activation_height(NetworkUpgrade::Nu6_1), None);
        assert_eq!(nu6.activation_height(NetworkUpgrade::Nu6_3), None);
    }

    #[test]
    fn annotate_sync_error_flags_non_ironwood_server() {
        // The exact string a pre-Ironwood lightwalletd returns, as wrapped by the
        // stock driver.
        let raw = "sync: Error while communicating with lightwalletd server: \
                   status: InvalidArgument, message: \"Error: Invalid shielded \
                   protocol value.\"";
        let out = annotate_sync_error(CoreError::Connection(raw.to_string()));
        match out {
            CoreError::Connection(m) => {
                assert!(m.contains("doesn't support Ironwood"), "friendly headline: {m}");
                assert!(m.contains("network settings"), "actionable guidance: {m}");
                // Raw detail is preserved after the em-dash separator.
                assert!(m.contains(" — "), "keeps raw detail: {m}");
                assert!(m.contains("Invalid shielded protocol value"), "raw text: {m}");
            }
            other => panic!("expected Connection error, got {other:?}"),
        }
    }

    #[test]
    fn annotate_sync_error_passes_through_unrelated() {
        // An unrelated connection error is returned unchanged (no false headline).
        let raw = "sync: Error while communicating with lightwalletd server: \
                   transport error";
        match annotate_sync_error(CoreError::Connection(raw.to_string())) {
            CoreError::Connection(m) => {
                assert_eq!(m, raw);
                assert!(!m.contains("Ironwood"));
            }
            other => panic!("expected Connection error, got {other:?}"),
        }
        // Cancellation is untouched.
        assert!(matches!(
            annotate_sync_error(CoreError::Cancelled),
            CoreError::Cancelled
        ));
    }

    #[test]
    fn detects_unsupported_ironwood_pool_for_graceful_skip() {
        // The rejection the pipelined driver skips Ironwood roots on.
        let raw = "get_subtree_roots: status: InvalidArgument, message: \
                   \"Error: Invalid shielded protocol value.\"";
        assert!(is_invalid_shielded_protocol(&CoreError::Connection(raw.into())));
        // Unrelated errors and non-connection variants are not mistaken for it.
        assert!(!is_invalid_shielded_protocol(&CoreError::Connection(
            "get_subtree_roots: transport error".into()
        )));
        assert!(!is_invalid_shielded_protocol(&CoreError::Cancelled));
    }

    /// The pipelined driver must scan in the exact same batch units as the stock
    /// driver, or its result could diverge. This locks the splitter's behaviour to
    /// the upstream `sync::running` step-7 semantics: contiguous, priority-
    /// preserving, `batch_size`-block sub-ranges that exactly cover the input and
    /// never produce an empty range.
    #[test]
    fn split_scan_range_matches_upstream_batching() {
        let h = BlockHeight::from_u32;
        let range = ScanRange::from_parts(h(100)..h(1050), ScanPriority::Historic);

        // An evenly-plus-remainder range → full batches then a short tail.
        let batches = split_scan_range(range.clone(), 400);
        assert_eq!(batches.len(), 3);
        assert_eq!(*batches[0].block_range(), h(100)..h(500));
        assert_eq!(*batches[1].block_range(), h(500)..h(900));
        assert_eq!(*batches[2].block_range(), h(900)..h(1050));
        // Priority is preserved on every sub-range.
        assert!(batches.iter().all(|b| b.priority() == ScanPriority::Historic));
        // Contiguous cover: no gaps, no overlaps, no empty ranges.
        assert!(batches.iter().all(|b| !b.is_empty()));
        for w in batches.windows(2) {
            assert_eq!(w[0].block_range().end, w[1].block_range().start);
        }
        assert_eq!(batches.first().unwrap().block_range().start, h(100));
        assert_eq!(batches.last().unwrap().block_range().end, h(1050));

        // A range smaller than one batch → a single batch equal to the input.
        let small = ScanRange::from_parts(h(10)..h(30), ScanPriority::ChainTip);
        let one = split_scan_range(small.clone(), 5000);
        assert_eq!(one.len(), 1);
        assert_eq!(*one[0].block_range(), h(10)..h(30));

        // A range that is an exact multiple of the batch size → no empty tail.
        let exact = ScanRange::from_parts(h(0)..h(1000), ScanPriority::Historic);
        let even = split_scan_range(exact, 500);
        assert_eq!(even.len(), 2);
        assert_eq!(*even[1].block_range(), h(500)..h(1000));
    }

    /// The receive address the wallet's key crate (`zcash_keys`) derives from
    /// our group UFVK must equal the address our derivation produced — proving
    /// our deterministically-derived keys are standard, wallet-usable Orchard
    /// keys, on both networks.
    #[test]
    fn ufvk_round_trips_to_our_address() {
        use orchard::keys::{FullViewingKey, SpendingKey};
        let sk = Option::<SpendingKey>::from(SpendingKey::from_bytes([9u8; 32])).unwrap();
        let ak: [u8; 32] = FullViewingKey::from(&sk).to_bytes()[..32].try_into().unwrap();

        for net in [
            WalletNetwork::Test,
            WalletNetwork::Main,
            WalletNetwork::Regtest(RegtestUpgrade::Nu6),
        ] {
            let keys = crate::zcash::derive_orchard_keys(&ak, net.network_type()).unwrap();
            let addr = ufvk_default_address(net, &keys.ufvk).unwrap();
            assert_eq!(addr, keys.address, "zcash_keys must agree on {net:?}");
        }
    }

    /// Mainnet activation heights, for readability in the tests below.
    const MAIN_NU5: u64 = 1_687_104;
    const TEST_NU5: u64 = 1_842_420;

    #[test]
    fn scan_from_defaults_to_the_tip_when_nothing_is_requested() {
        assert_eq!(resolve_scan_from(None, MAIN_NU5, 3_400_000), 3_400_000);
    }

    #[test]
    fn scan_from_honours_a_requested_birthday_inside_the_range() {
        assert_eq!(
            resolve_scan_from(Some(3_800_000), TEST_NU5, 4_200_000),
            3_800_000
        );
    }

    #[test]
    fn scan_from_never_precedes_nu5() {
        // Orchard notes cannot exist below NU5, so an earlier birthday is lifted.
        assert_eq!(resolve_scan_from(Some(1000), MAIN_NU5, 3_400_000), MAIN_NU5);
    }

    #[test]
    fn scan_from_never_exceeds_the_tip() {
        // The testnet default (3.8M) sits above mainnet's tip (~3.4M). Without
        // the clamp the treestate fetch would fail and the wallet would scan
        // nothing; instead a mainnet wallet quietly starts at the tip.
        let mainnet_tip = 3_400_000;
        assert_eq!(
            resolve_scan_from(Some(DEFAULT_TESTNET_BIRTHDAY), MAIN_NU5, mainnet_tip),
            mainnet_tip
        );
    }

    #[test]
    fn new_wallets_start_at_the_tip_on_both_networks() {
        // A brand-new group holds no pre-creation funds, so with no requested or
        // recorded birthday it starts at the chain tip (None -> tip) rather than
        // scanning hundreds of thousands of blocks of empty history. The testnet
        // deep-rescan floor is opt-in via `birthday_height`, never a default.
        assert_eq!(default_birthday_height(WalletNetwork::Test), None);
        assert_eq!(default_birthday_height(WalletNetwork::Main), None);
        assert_eq!(resolve_scan_from(None, TEST_NU5, 4_200_000), 4_200_000);
    }

    #[test]
    fn regtest_wallets_scan_the_whole_devnet_chain() {
        // A devnet group is usually funded from the faucet before its wallet is
        // opened, so regtest scans from block 2 rather than the tip.
        for level in RegtestUpgrade::ALL {
            let net = WalletNetwork::Regtest(level);
            assert_eq!(default_birthday_height(net), Some(REGTEST_BIRTHDAY));
            assert_eq!(resolve_scan_from(default_birthday_height(net), 1, 350), 2);
        }
        // A brand-new devnet whose tip is below the birthday still anchors.
        assert_eq!(resolve_scan_from(Some(REGTEST_BIRTHDAY), 1, 1), 1);
    }

    #[test]
    fn explicit_deep_rescan_floor_is_honoured_when_requested() {
        // Passing DEFAULT_TESTNET_BIRTHDAY explicitly (an opt-in deep rescan)
        // still scans from that floor when it sits inside [NU5, tip].
        assert_eq!(
            resolve_scan_from(Some(DEFAULT_TESTNET_BIRTHDAY), TEST_NU5, 4_200_000),
            DEFAULT_TESTNET_BIRTHDAY
        );
    }
}
