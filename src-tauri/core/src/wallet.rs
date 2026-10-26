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

pub mod tor;
pub mod verify;

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
    /// NU7 — what thus-spoke-zakura 0.4 starts new chains at.
    Nu7,
}

impl RegtestUpgrade {
    pub const ALL: [RegtestUpgrade; 5] = [
        RegtestUpgrade::Nu6,
        RegtestUpgrade::Nu6_1,
        RegtestUpgrade::Nu6_2,
        RegtestUpgrade::Nu6_3,
        RegtestUpgrade::Nu7,
    ];

    /// The settings/IPC spelling (`nu6`, `nu6_1`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            RegtestUpgrade::Nu6 => "nu6",
            RegtestUpgrade::Nu6_1 => "nu6_1",
            RegtestUpgrade::Nu6_2 => "nu6_2",
            RegtestUpgrade::Nu6_3 => "nu6_3",
            RegtestUpgrade::Nu7 => "nu7",
        }
    }

    fn branch_id(self) -> zcash_protocol::consensus::BranchId {
        use zcash_protocol::consensus::BranchId;
        match self {
            RegtestUpgrade::Nu6 => BranchId::Nu6,
            RegtestUpgrade::Nu6_1 => BranchId::Nu6_1,
            RegtestUpgrade::Nu6_2 => BranchId::Nu6_2,
            RegtestUpgrade::Nu6_3 => BranchId::Nu6_3,
            RegtestUpgrade::Nu7 => BranchId::Nu7,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|u| u.as_str() == s)
    }

    /// The upgrade whose consensus branch id a node reports (lowercase hex, as
    /// in [`LightwalletdInfo::consensus_branch_id`]). `None` for a branch older
    /// than NU6 or one this build doesn't know.
    pub fn from_branch_id(branch_hex: &str) -> Option<Self> {
        let wanted = branch_hex.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|u| format!("{:08x}", u32::from(u.branch_id())) == wanted)
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
            nu7: upto(RegtestUpgrade::Nu7),
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
    // Not `FromStr`: this never fails (unknown spellings mean testnet).
    #[allow(clippy::should_implement_trait)]
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

    /// The coin's ticker on this network, as the interface shows it.
    pub fn ticker(self) -> &'static str {
        match self {
            WalletNetwork::Main => "ZEC",
            WalletNetwork::Test => "TAZ",
            WalletNetwork::Regtest(_) => "rZEC",
        }
    }

    /// The servers the app picks from by itself when none is chosen: lightwalletd
    /// endpoints known to serve Ironwood, all run by zec.rocks, one per region.
    pub fn known_lightwalletd(self) -> &'static [&'static str] {
        match self {
            WalletNetwork::Main => &[
                "https://zec.rocks:443",
                "https://na.zec.rocks:443",
                "https://eu.zec.rocks:443",
                "https://ap.zec.rocks:443",
                "https://sa.zec.rocks:443",
            ],
            WalletNetwork::Test => &["https://testnet.zec.rocks:443"],
            WalletNetwork::Regtest(_) => &[],
        }
    }

    /// The chain name lightwalletd reports for this network.
    fn chain_name(self) -> &'static str {
        match self {
            WalletNetwork::Main => "main",
            WalletNetwork::Test => "test",
            WalletNetwork::Regtest(_) => "regtest",
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
/// 8-digit hex).
#[cfg(test)]
const NU6_3_BRANCH_ID: &str = "37a5165b";

/// The Orchard proving/verifying circuit version to use for a transaction mined
/// at `height` on `network`. This MUST match the circuit consensus requires at
/// that height, or the proof is rejected by the network:
///
/// - From NU6.3 (Ironwood) on, NU7 included → `PostNu6_3`. Both live networks
///   are past NU6.3 (testnet 4,134,000; mainnet 3,428,143).
/// - Before NU6.3 → `FixedPostNu6_2`, which can verify but no longer prove.
///
/// Decided by whether NU6.3 is active, not by matching its branch id: from
/// NU7 the branch changes but the circuit doesn't.
#[cfg(feature = "wallet")]
fn orchard_circuit_version_for_height(
    network: WalletNetwork,
    height: u64,
) -> orchard::circuit::OrchardCircuitVersion {
    use orchard::circuit::OrchardCircuitVersion;
    use zcash_protocol::consensus::{BlockHeight, NetworkUpgrade, Parameters};
    if network.params().is_nu_active(NetworkUpgrade::Nu6_3, BlockHeight::from_u32(height as u32)) {
        OrchardCircuitVersion::PostNu6_3
    } else {
        OrchardCircuitVersion::FixedPostNu6_2
    }
}

/// The height `secs` seconds of block production before `tip`: 75-second
/// blocks before NU7 activates and 25-second blocks from it (ZIP 218).
pub fn height_before(network: WalletNetwork, tip: u64, secs: u64) -> u64 {
    use zcash_protocol::consensus::{NetworkUpgrade, Parameters, NU7_POW_TARGET_SPACING_RATIO, SECONDS_PER_BLOCK};
    let before_nu7 = u64::from(SECONDS_PER_BLOCK);
    let from_nu7 = before_nu7 / u64::from(NU7_POW_TARGET_SPACING_RATIO);
    match network.params().activation_height(NetworkUpgrade::Nu7).map(u64::from) {
        Some(nu7) if tip > nu7 => {
            let covered = (tip - nu7) * from_nu7;
            if secs <= covered {
                tip - secs / from_nu7
            } else {
                nu7.saturating_sub((secs - covered) / before_nu7)
            }
        }
        _ => tip.saturating_sub(secs / before_nu7),
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
    if let Some(tor) = tor::route(&url)? {
        let uri = url
            .parse::<tonic::transport::Uri>()
            .map_err(|e| CoreError::Connection(format!("invalid lightwalletd URL: {e}")))?;
        return tor
            .connect_to_lightwalletd(uri, false)
            .await
            .map_err(|e| CoreError::Connection(format!("connecting to {url} through Tor: {e}")));
    }
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

/// A lightwalletd connection that outlives a single sync.
///
/// Once caught up, the wallet re-syncs every few seconds, and connecting fresh
/// each time paid a TCP + TLS handshake per cycle. Keep one of these and pass it
/// to [`sync_group`] instead. Clones are cheap and share the one HTTP/2
/// connection; if it drops, the next request fails and the caller should
/// reconnect.
#[derive(Clone)]
pub struct Lightwalletd {
    url: String,
    client: CompactTxStreamerClient<Channel>,
}

impl Lightwalletd {
    /// Connect to `url`, under the same endpoint rules as every wallet call.
    pub async fn connect(url: &str) -> Result<Self, CoreError> {
        Ok(Self {
            url: url.trim().to_string(),
            client: connect(url).await?,
        })
    }

    /// The endpoint this connection was made to.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The server's current chain tip height. One small RPC, so a caught-up
    /// wallet can check for new blocks without running a sync.
    pub async fn latest_height(&self) -> Result<u64, CoreError> {
        let mut client = self.client.clone();
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            client.get_latest_block(ChainSpec::default()),
        )
        .await
        .map_err(|_| CoreError::Connection("get_latest_block timed out".into()))?
        .map(|r| r.into_inner().height)
        .map_err(|e| CoreError::Connection(format!("get_latest_block: {e}")))
    }
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

/// A server that answered [`fastest_lightwalletd`]'s probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerProbe {
    pub url: String,
    /// Connecting plus one round trip, in milliseconds.
    pub latency_ms: u64,
    pub height: u64,
}

/// How far behind the highest tip a probed server may be and still be chosen.
const PROBE_MAX_LAG: u64 = 3;

/// Probe `urls` at once and return the fastest that's on `network`'s chain and
/// within a few blocks of the highest tip any of them reports, so a server
/// that's lagging or on the wrong chain is never chosen for being quick. One
/// that doesn't answer within `timeout` is skipped. `None` if none qualifies.
pub async fn fastest_lightwalletd(
    network: WalletNetwork,
    urls: &[&str],
    timeout: std::time::Duration,
) -> Option<ServerProbe> {
    let mut probes = tokio::task::JoinSet::new();
    for url in urls {
        let url = url.to_string();
        probes.spawn(async move {
            let started = std::time::Instant::now();
            let info = tokio::time::timeout(timeout, lightwalletd_info(&url)).await.ok()?.ok()?;
            Some((info, ServerProbe { url, latency_ms: elapsed_ms(started), height: 0 }))
        });
    }
    let mut answered = Vec::new();
    while let Some(probe) = probes.join_next().await {
        if let Ok(Some((info, mut probe))) = probe {
            if info.chain_name == network.chain_name() {
                probe.height = info.block_height;
                answered.push(probe);
            }
        }
    }
    choose_server(answered)
}

/// The quickest of the servers near the highest tip (see [`fastest_lightwalletd`]).
fn choose_server(answered: Vec<ServerProbe>) -> Option<ServerProbe> {
    let tip = answered.iter().map(|p| p.height).max()?;
    answered
        .into_iter()
        .filter(|p| p.height + PROBE_MAX_LAG >= tip)
        .min_by_key(|p| p.latency_ms)
}

// ---------------------------------------------------------------------------
// Per-group wallet: sqlite-backed account, sync, and balance.
//
// Each FROST group is one view-only Orchard account, stored in its own sqlite
// wallet under `<data_dir>/wallets/<wallet_id>/`. The caller picks `wallet_id`;
// the app uses an opaque id rather than the group id, which is enough to view
// the wallet. The group's UFVK (derived from its `ak`) is imported as a
// watch-only account; sync trial-decrypts compact blocks locally; balance is
// read from the wallet db.
// ---------------------------------------------------------------------------

use std::path::{Path, PathBuf};

/// The randomness source wallet databases use: the OS generator, in the
/// `rand` 0.10 traits the Zakura wallet crates expect. An OS RNG failure is
/// practically impossible and unrecoverable, so it panics rather than adding
/// an error path to every database call.
type WalletRng = rand10::rand_core::UnwrapErr<rand10::rngs::SysRng>;

fn wallet_rng() -> WalletRng {
    rand10::rand_core::UnwrapErr(rand10::rngs::SysRng)
}
use zcash_client_backend::data_api::chain::error::Error as ChainError;
use zcash_client_backend::data_api::chain::{
    BlockSource, ChainState, CommitmentTreeRoot,
};
use zcash_client_backend::data_api::scanning::{ScanPriority, ScanRange};
use zcash_client_backend::data_api::wallet::{
    create_pczt_from_proposal, propose_send_max_transfer, propose_transfer, ConfirmationsPolicy,
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

type GroupDb = WalletDb<rusqlite::Connection, WalletParams, SystemClock, WalletRng>;

/// The wallet database file. `-v2` is the Zakura-stack database. Earlier
/// versions kept `wallet.sqlite`, built by upstream `zcash_client_sqlite`,
/// whose schema the Zakura fork doesn't promise to migrate, plus a `blocks`
/// cache the in-memory sync no longer uses. A wallet database is a rebuildable
/// cache of the chain, so those are deleted ([`remove_legacy_wallet`]) and the
/// wallet resyncs from its recorded start height.
const WALLET_DB: &str = "wallet-v2.sqlite";

/// The wallet database for a group on a given network. Scoped by network
/// (`.../<wallet_id>/<network>/...`) so testnet and mainnet each keep their own
/// db and balance and never share state.
fn wallet_db_path(data_dir: &Path, wallet_id: &str, network: WalletNetwork) -> PathBuf {
    data_dir
        .join("wallets")
        .join(wallet_id)
        .join(network.dir_name())
        .join(WALLET_DB)
}

/// Delete what earlier versions kept in a wallet directory (see [`WALLET_DB`]).
fn remove_legacy_wallet(dir: &Path) {
    for name in ["wallet.sqlite", "wallet.sqlite-wal", "wallet.sqlite-shm", "wallet.sqlite.enc-tmp"] {
        let _ = std::fs::remove_file(dir.join(name));
    }
    let _ = std::fs::remove_dir_all(dir.join("blocks"));
}

const PENDING_TX_LABEL: &[u8] = b"cyze-pending-tx-v1";

/// Path of the on-disk record for a fully-signed transaction awaiting broadcast.
/// Keeping the signed PCZT lets a failed broadcast be retried without repeating
/// the whole FROST signing ceremony. Network-scoped like [`wallet_db_path`].
fn pending_tx_path(
    data_dir: &Path,
    wallet_id: &str,
    network: WalletNetwork,
    ceremony_id: &str,
) -> PathBuf {
    data_dir
        .join("wallets")
        .join(wallet_id)
        .join(network.dir_name())
        .join("pending")
        .join(format!("{ceremony_id}.pczt.hex"))
}

/// Persist a signed-but-not-broadcast PCZT so it can be re-broadcast later.
pub fn save_pending_tx(
    data_dir: &Path,
    wallet_id: &str,
    network: WalletNetwork,
    ceremony_id: &str,
    signed_pczt_hex: &str,
    key: &[u8; 32],
) -> Result<PathBuf, CoreError> {
    let path = pending_tx_path(data_dir, wallet_id, network, ceremony_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        let _ = crate::keystore::restrict_dir_to_owner(parent);
    }
    // Encrypted: the PCZT names the recipient and amount for signers to verify.
    let sealed = crate::sealed::seal(key, PENDING_TX_LABEL, signed_pczt_hex.as_bytes())?;
    crate::keystore::write_atomic(&path, &sealed)?;
    let _ = crate::keystore::restrict_to_owner(&path);
    Ok(path)
}

/// Load a previously-saved signed PCZT for re-broadcast.
pub fn load_pending_tx(
    data_dir: &Path,
    wallet_id: &str,
    network: WalletNetwork,
    ceremony_id: &str,
    key: &[u8; 32],
) -> Result<String, CoreError> {
    let path = pending_tx_path(data_dir, wallet_id, network, ceremony_id);
    let bytes = std::fs::read(&path).map_err(|e| {
        CoreError::Config(format!("no pending transaction for {ceremony_id}: {e}"))
    })?;
    let plain = match crate::sealed::open(key, PENDING_TX_LABEL, &bytes) {
        Ok(plain) => plain.to_vec(),
        // Saved by an earlier version, unencrypted.
        Err(_) if bytes.iter().all(|b| b.is_ascii_hexdigit() || b.is_ascii_whitespace()) => bytes,
        Err(e) => return Err(e),
    };
    let hex = String::from_utf8(plain).map_err(|e| CoreError::Config(format!("pending transaction: {e}")))?;
    Ok(hex.trim().to_string())
}

/// Remove a pending transaction record once it has been broadcast.
pub fn clear_pending_tx(data_dir: &Path, wallet_id: &str, network: WalletNetwork, ceremony_id: &str) {
    let path = pending_tx_path(data_dir, wallet_id, network, ceremony_id);
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
/// progress), unlocked with the SQLCipher key.
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
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| CoreError::Crypto(format!("open wallet db: {e}")))?;
    conn.busy_timeout(DB_BUSY_TIMEOUT)
        .map_err(|e| CoreError::Crypto(format!("set busy timeout: {e}")))?;
    conn.execute_batch(&key_pragma(db_key))
        .map_err(|e| CoreError::Crypto(format!("unlock wallet db: {e}")))?;
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
    wallet_id: &str,
    network: WalletNetwork,
    db_key: &[u8],
) -> Result<(u64, u64), CoreError> {
    let db_path = wallet_db_path(data_dir, wallet_id, network);
    if !db_path.exists() {
        return Ok((0, 0));
    }
    let conn = open_readonly_connection(&db_path, db_key)?;
    let db = WalletDb::from_connection(&conn, network.params(), SystemClock, wallet_rng());
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

fn open_db(db_path: &Path, network: WalletNetwork, db_key: &[u8]) -> Result<GroupDb, CoreError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
        // The wallet db holds the group's UFVK and transaction history, so lock
        // the directory to the owner as defence in depth alongside encryption.
        let _ = crate::keystore::restrict_dir_to_owner(parent);
        remove_legacy_wallet(parent);
    }
    let conn = open_keyed_connection(db_path, db_key)?;
    let mut db = WalletDb::from_connection(conn, network.params(), SystemClock, wallet_rng());
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
    Ok(WalletDb::from_connection(conn, network.params(), SystemClock, wallet_rng()))
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
    wallet_id: &str,
    network: WalletNetwork,
    ufvk: &str,
    db_key: &[u8],
) -> Result<WalletStatus, CoreError> {
    let db_path = wallet_db_path(data_dir, wallet_id, network);
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
    wallet_id: &str,
    network: WalletNetwork,
    ufvk_str: &str,
    lightwalletd_url: &str,
    db_key: &[u8],
    birthday_height: Option<u64>,
) -> Result<u64, CoreError> {
    use zcash_protocol::consensus::{NetworkUpgrade, Parameters};

    let tag = crate::redact::log_tag(wallet_id);
    let db_path = wallet_db_path(data_dir, wallet_id, network);
    let mut db = open_db(&db_path, network, db_key)?;
    if !db
        .get_account_ids()
        .map_err(|e| CoreError::Crypto(format!("wallet accounts: {e}")))?
        .is_empty()
    {
        tracing::debug!(group = %tag, "wallet setup: account already imported; nothing to do");
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

    tracing::info!(group = %tag, url = %lightwalletd_url, "wallet setup: connecting to lightwalletd");
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
    tracing::info!(group = %tag, tip, "wallet setup: connected; got chain tip");

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
    tracing::info!(group = %tag, scan_from, "wallet setup: fetching tree state for the account birthday");
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

    db.import_account_ufvk(wallet_id, &ufvk, &birthday, AccountPurpose::ViewOnly, None)
        .map_err(|e| CoreError::Crypto(format!("import account: {e}")))?;
    tracing::info!(group = %tag, scan_from, "wallet setup: view-only account imported; will scan from this height");
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
    /// Skip fetching the note-commitment subtree roots. A pool gains a new root
    /// only every 2^16 notes, so a caller that fetched them earlier in the
    /// session can skip the three RPC streams and db writes on each re-sync.
    /// `false` (the default) fetches them, as upstream `sync::run` does.
    pub skip_subtree_roots: bool,
}

/// Where a sync spent its time. Whichever of `download_ms` and `scan_ms` is
/// larger is the bottleneck. The wait figures show how well the two legs
/// overlapped.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct SyncStats {
    /// Historic-range batches scanned (the verify pass is not counted).
    pub batches: u32,
    /// Blocks scanned across those batches.
    pub blocks: u64,
    /// Fetching and storing subtree roots; 0 when skipped.
    pub subtree_roots_ms: u64,
    /// Downloading blocks and their chain-state anchors.
    pub download_ms: u64,
    /// Scanning: trial decryption plus the per-batch db commit.
    pub scan_ms: u64,
    /// The scanner sat idle waiting for the next download (network-bound).
    pub scan_wait_ms: u64,
    /// The downloader sat idle because the scanner was behind (CPU-bound).
    pub download_wait_ms: u64,
    /// Wall-clock time for the whole sync.
    pub total_ms: u64,
}

impl SyncStats {
    /// Blocks scanned per second of wall-clock time.
    pub fn blocks_per_sec(&self) -> f64 {
        if self.total_ms == 0 {
            0.0
        } else {
            self.blocks as f64 * 1000.0 / self.total_ms as f64
        }
    }
}

fn elapsed_ms(since: std::time::Instant) -> u64 {
    since.elapsed().as_millis() as u64
}

/// Sync the group's wallet: download and trial-decrypt compact blocks from
/// lightwalletd into the local db. Long-running; touches the network.
///
/// Driven by the pipelined [`run_pipelined`] driver, which overlaps block
/// download with CPU trial-decryption and streams blocks straight from the
/// network to the scanner (no on-disk block cache).
pub async fn sync_group(
    data_dir: &Path,
    wallet_id: &str,
    network: WalletNetwork,
    lightwalletd: &Lightwalletd,
    db_key: &[u8],
    opts: SyncOptions,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<SyncStats, CoreError> {
    let started = std::time::Instant::now();
    let batch_size = opts
        .batch_size
        .unwrap_or(DEFAULT_SYNC_BATCH_SIZE)
        .clamp(MIN_SYNC_BATCH_SIZE, MAX_SYNC_BATCH_SIZE);
    let db_path = wallet_db_path(data_dir, wallet_id, network);
    let mut db = open_db(&db_path, network, db_key)?;

    let mut client = lightwalletd.client.clone();
    let params = network.params();
    let mut stats = SyncStats::default();
    // The driver scans in transactional batches, so dropping the future between
    // batches leaves the db consistent (just short of the tip). That makes it
    // safe to race against a cancellation token: "Sync Now" trips the token to
    // abandon a stalled run, and a fresh sync resumes from where this one left
    // off. Without this, a stuck stream would keep the sync pending forever.
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(CoreError::Cancelled),
        res = run_pipelined(&mut client, &params, &mut db, batch_size, opts.skip_subtree_roots, &mut stats) => res,
    };
    stats.total_ms = elapsed_ms(started);
    if result.is_ok() {
        let summary = format!(
            "sync finished: {} blocks in {} batches, {} ms ({:.0} blocks/s); \
             subtree roots {} ms, download {} ms, scan {} ms, scanner waited {} ms \
             on the network, downloader waited {} ms on the scanner",
            stats.blocks,
            stats.batches,
            stats.total_ms,
            stats.blocks_per_sec(),
            stats.subtree_roots_ms,
            stats.download_ms,
            stats.scan_ms,
            stats.scan_wait_ms,
            stats.download_wait_ms,
        );
        // An up-to-date wallet re-syncs every few seconds; only a sync that
        // actually scanned something is worth an info line.
        if stats.blocks > 0 {
            tracing::info!("{summary}");
        } else {
            tracing::debug!("{summary}");
        }
    }
    // Turn known, actionable failures into a message that says what to do, while
    // keeping the raw server error appended for diagnosis.
    result.map(|()| stats).map_err(annotate_sync_error)
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

/// Mainnet's 2022–23 spam: blocks so full of outputs that a normal batch of
/// them would take gigabytes of memory. ECC's mobile SDK and Vizor scan them
/// [`SPAM_BATCH_SIZE`] blocks at a time. Only deep rescans reach back this far.
const MAINNET_SPAM: std::ops::Range<u32> = 1_710_000..2_050_000;
const SPAM_BATCH_SIZE: u32 = 100;

/// The spam range on `params`' network, if it has one.
fn spam_range(params: &WalletParams) -> Option<std::ops::Range<u32>> {
    (*params == WalletParams::Consensus(Network::MainNetwork)).then_some(MAINNET_SPAM)
}

/// Split a suggested scan range into `batch_size`-block sub-ranges, preserving
/// priority, as the upstream `sync::running` step-7 splitter does, except
/// inside `spam` (see [`MAINNET_SPAM`]): batches stop where it starts and are
/// [`SPAM_BATCH_SIZE`] blocks within it.
fn split_scan_range(range: ScanRange, batch_size: u32, spam: Option<std::ops::Range<u32>>) -> Vec<ScanRange> {
    let mut acc = range;
    let mut out = Vec::new();
    loop {
        if acc.is_empty() {
            break;
        }
        let start = u32::from(acc.block_range().start);
        let len = match &spam {
            Some(r) if r.contains(&start) => SPAM_BATCH_SIZE.min(r.end - start),
            Some(r) if start < r.start => batch_size.min(r.start - start),
            _ => batch_size,
        };
        match acc.split_at(acc.block_range().start + len) {
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
/// Note: upstream `running` also refreshes transparent UTXOs when the
/// `transparent-inputs` feature is on. It is on in our build (the `pczt` feature
/// enables it), but a group account is an Orchard-only view key with no
/// transparent receivers, so that refresh would have nothing to fetch. Omitting
/// it costs nothing and saves a round trip per sync.
async fn run_pipelined(
    client: &mut CompactTxStreamerClient<Channel>,
    params: &WalletParams,
    db: &mut GroupDb,
    batch_size: u32,
    skip_subtree_roots: bool,
    stats: &mut SyncStats,
) -> Result<(), CoreError> {
    // 1) & 2) Download note-commitment subtree roots and hand them to the db, so
    //    the trees are initialized without replaying all history. No pipelining
    //    benefit, so it stays serial; callers that fetched them earlier in the
    //    session skip it.
    if !skip_subtree_roots {
        let t = std::time::Instant::now();
        update_subtree_roots_pipelined(client, db).await?;
        stats.subtree_roots_ms = elapsed_ms(t);
    }

    // Re-run the per-session loop until the wallet's view of the chain tip is
    // valid (mirrors `while running(..).await? {}` upstream).
    while running_pipelined(client, params, db, batch_size, stats).await? {}

    Ok(())
}

/// Run synchronous, CPU-heavy work (scanning) from async code without stalling
/// the runtime.
///
/// Calling `scan_cached_blocks` directly on a Tokio worker undermines the
/// pipeline. When the scanner's `recv` frees a channel slot, it wakes the
/// waiting downloader on *this* worker, into a LIFO slot other workers cannot
/// steal. The download therefore can't resume until this worker is free again,
/// so download and scan stop overlapping. `block_in_place` first hands this
/// worker's queue to another thread, so the downloader keeps going. A
/// current-thread runtime (unit tests) has nothing to hand off and just runs
/// the closure.
fn run_blocking<T>(f: impl FnOnce() -> T) -> T {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current() {
        Ok(h) if h.runtime_flavor() == RuntimeFlavor::MultiThread => tokio::task::block_in_place(f),
        _ => f(),
    }
}

/// Downloader-side timings, shared with the spawned producer task (which may be
/// aborted, so it records as it goes rather than returning a value).
#[derive(Default)]
struct ProducerTimes {
    download_ms: std::sync::atomic::AtomicU64,
    send_wait_ms: std::sync::atomic::AtomicU64,
}

/// One pass of the pipelined sync loop. Returns `true` when the suggested scan
/// ranges changed underneath us (continuity error, or a newly higher-priority
/// range) and the caller should restart from a fresh `suggest_scan_ranges`.
async fn running_pipelined(
    client: &mut CompactTxStreamerClient<Channel>,
    params: &WalletParams,
    db: &mut GroupDb,
    batch_size: u32,
    stats: &mut SyncStats,
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
                let (blocks, chain_state) = download_batch(client, &sr).await?;
                let src = MemBlockSource(blocks);
                if run_blocking(|| scan_batch(params, &src, db, &chain_state, &sr))? {
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
        .flat_map(|r| split_scan_range(r, batch_size, spam_range(params)))
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
    let times = std::sync::Arc::new(ProducerTimes::default());
    let producer_times = times.clone();
    let producer = tokio::spawn(async move {
        use std::sync::atomic::Ordering::Relaxed;
        for sr in batches {
            let dl_start = std::time::Instant::now();
            let (blocks, chain_state) = match download_batch(&mut producer_client, &sr).await {
                Ok(batch) => batch,
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
            producer_times.download_ms.fetch_add(elapsed_ms(dl_start), Relaxed);
            // If the consumer has hung up (ranges changed, or an error broke the
            // loop) stop downloading. Time spent blocked here is time the
            // scanner was the bottleneck.
            let send_start = std::time::Instant::now();
            if tx.send(Ok((sr, blocks, chain_state))).await.is_err() {
                return;
            }
            producer_times.send_wait_ms.fetch_add(elapsed_ms(send_start), Relaxed);
        }
    });

    // Consumer: scan each prefetched batch in order. `scan_batch` is CPU-bound
    // and synchronous; `run_blocking` keeps it from pinning the producer, so the
    // next batches download while this one scans, which is the whole point.
    // Scanning is transactional per batch, so bailing out early (or being
    // dropped on cancellation) leaves the db consistent at a batch boundary.
    let mut result = Ok(false);
    let scan_run_start = std::time::Instant::now();
    let mut scanned_blocks: u64 = 0;
    loop {
        let wait_start = std::time::Instant::now();
        let Some(item) = rx.recv().await else { break };
        stats.scan_wait_ms += elapsed_ms(wait_start);
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
        let outcome = run_blocking(|| scan_batch(params, &src, db, &chain_state, &sr));
        stats.scan_ms += elapsed_ms(scan_start);
        stats.batches += 1;
        stats.blocks += n;
        // Per-batch scan cost and cumulative throughput. This is the CPU-bound leg
        // (trial decryption + note-commitment tree updates); logging it here makes
        // the download-vs-scan split visible when diagnosing slow syncs.
        scanned_blocks += n;
        let secs = scan_run_start.elapsed().as_secs_f64();
        tracing::debug!(
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
    {
        use std::sync::atomic::Ordering::Relaxed;
        stats.download_ms += times.download_ms.load(Relaxed);
        stats.download_wait_ms += times.send_wait_ms.load(Relaxed);
    }
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
            rewind_on_reorg(db, err.at_height(), rewind_height)?;
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

/// Rewind the wallet after a reorg detected at `at_height` (the first block
/// that didn't connect), preferably to `target` (a few blocks of slack below).
///
/// A rewind can only land on a tree checkpoint. A wallet keeps checkpoints
/// only from its start height on, and when a new wallet catches up it scans
/// the tip first and the blocks behind it afterwards, without checkpoints. So
/// near a wallet's start the backend refuses rewinds. Then try the smallest
/// rewind that still drops the block that didn't connect (`at_height - 2`): if
/// the fork goes deeper, the next scan finds the next mismatch lower down and
/// this steps back again. When even that is refused, the fork is below what
/// the wallet can rewind: [`CoreError::Reorg`], and the sync service rebuilds
/// the wallet from an earlier height. (Ported from the fix in 78e361b.)
fn rewind_on_reorg(db: &mut GroupDb, at_height: BlockHeight, target: BlockHeight) -> Result<(), CoreError> {
    use zcash_client_sqlite::error::SqliteClientError;
    let refused = match db.truncate_to_height(target) {
        Ok(_) => return Ok(()),
        Err(e @ SqliteClientError::RequestedRewindInvalid { .. }) => e,
        Err(e) => return Err(CoreError::Reorg(e.to_string())),
    };
    let minimal = at_height.saturating_sub(2);
    if minimal > target {
        match db.truncate_to_height(minimal) {
            Ok(actual) => {
                tracing::info!("reorg: rewound only to {actual}; the wallet can't go back to {target}");
                return Ok(());
            }
            Err(e) => tracing::info!("reorg: can't rewind to {minimal} either: {e}"),
        }
    }
    Err(CoreError::Reorg(refused.to_string()))
}

/// The height a group's wallet started scanning from (its account birthday),
/// read from its database. `None` if there is no wallet yet.
pub fn wallet_start_height(
    data_dir: &Path,
    wallet_id: &str,
    network: WalletNetwork,
    db_key: &[u8],
) -> Result<Option<u64>, CoreError> {
    let db_path = wallet_db_path(data_dir, wallet_id, network);
    if !db_path.exists() {
        return Ok(None);
    }
    let db = open_db_read(&db_path, network, db_key)?;
    let Some(account) = db
        .get_account_ids()
        .map_err(|e| CoreError::Crypto(format!("wallet accounts: {e}")))?
        .first()
        .copied()
    else {
        return Ok(None);
    };
    let birthday = db
        .get_account_birthday(account)
        .map_err(|e| CoreError::Crypto(format!("wallet birthday: {e}")))?;
    Ok(Some(u64::from(birthday)))
}

/// Delete a group's wallet database on a network (with its WAL/SHM files) so
/// the next setup rebuilds it. Pending signed transactions are kept. The wallet
/// is a cache of the chain: nothing is lost but scanning time.
pub fn delete_wallet_db(data_dir: &Path, wallet_id: &str, network: WalletNetwork) -> Result<(), CoreError> {
    let db = wallet_db_path(data_dir, wallet_id, network);
    for path in [db.clone(), PathBuf::from(format!("{}-wal", db.display())), PathBuf::from(format!("{}-shm", db.display()))] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(CoreError::Io(e)),
        }
    }
    Ok(())
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
/// Download a batch's blocks and the tree state just before them, checking that
/// the blocks follow it, as scanning from it assumes. They can disagree when
/// the chain reorganizes between the two requests, so a mismatch is fetched
/// once more; a second one means the server's data is inconsistent.
async fn download_batch(
    client: &mut CompactTxStreamerClient<Channel>,
    sr: &ScanRange,
) -> Result<(Vec<CompactBlock>, ChainState), CoreError> {
    for _ in 0..2 {
        let blocks = download_blocks_pipelined(client, sr).await?;
        let chain_state = download_chain_state_pipelined(client, sr.block_range().start - 1).await?;
        if blocks_follow(&chain_state, &blocks) {
            return Ok((blocks, chain_state));
        }
        tracing::info!("blocks for {sr} don't follow the tree state before them; fetching again");
    }
    Err(CoreError::Connection(
        "the sync server's blocks don't match its own record of the chain. Try again, or choose another \
         server in Settings → Wallet"
            .into(),
    ))
}

/// Whether `blocks` start right after the block `chain_state` describes.
fn blocks_follow(chain_state: &ChainState, blocks: &[CompactBlock]) -> bool {
    blocks.first().is_none_or(|b| b.prev_hash() == chain_state.block_hash())
}

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

/// One payment the group asks to make: to whom, how much, and an optional memo
/// (dropped for a transparent recipient, which can't receive one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentRequest {
    pub recipient: String,
    pub amount_zatoshis: u64,
    #[serde(default)]
    pub memo: Option<String>,
}

/// The most payments one transaction carries. Each is an output co-signers
/// verify from the attached transaction, which has to fit one frostd message
/// ([`crate::signing::signer_context_budget`], about 31 KiB for a few
/// signers): twenty take about 21 KiB, and a memo adds about 0.7 KiB.
pub const MAX_PAYMENTS: usize = 20;

/// One payment in a draft transaction, as built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftPayment {
    pub recipient: String,
    pub amount_zatoshis: u64,
    pub memo: Option<String>,
    /// To a transparent address: amount and recipient are public.
    pub transparent: bool,
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
    /// Every payment it makes. With more than one (a batch), `recipient` is
    /// empty, `amount_zatoshis` is their total, `memo` is None, and
    /// `is_unshield` says whether any is transparent.
    pub payments: Vec<DraftPayment>,
    /// How long the group has to approve it, as of when it was built.
    pub approval_window: Option<ApprovalWindow>,
}

/// Nodes refuse a transaction that would expire within this many blocks of
/// the next one (zcashd's `TX_EXPIRING_SOON_THRESHOLD`).
pub const EXPIRING_SOON_BLOCKS: u32 = 3;

/// How long a built transaction can wait for approvals before it can't be sent.
///
/// Transactions keep the standard expiry every wallet uses (about 50 minutes):
/// the expiry height is public, and a longer one would single out a group's
/// payments on-chain. So the window is shown rather than stretched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalWindow {
    /// The last block the transaction can be mined in.
    pub expiry_height: u32,
    /// Blocks still to come before it's too late to send; 0 once it is.
    pub blocks_left: u32,
    /// When that's expected (epoch ms), from the target block spacing. None on
    /// a local devnet, where blocks arrive when someone mines them.
    pub closes_at_ms: Option<i64>,
}

impl ApprovalWindow {
    /// The window of a transaction expiring at `expiry_height` with the chain
    /// at `tip_height`. None for one that never expires (expiry height 0).
    pub fn at(network: WalletNetwork, expiry_height: u32, tip_height: u64, now_ms: i64) -> Option<Self> {
        if expiry_height == 0 {
            return None;
        }
        // From a tip of `expiry - EXPIRING_SOON_BLOCKS`, the next block plus the
        // margin passes the expiry.
        let last = u64::from(expiry_height.saturating_sub(EXPIRING_SOON_BLOCKS));
        let blocks_left = u32::try_from(last.saturating_sub(tip_height)).unwrap_or(u32::MAX);
        let closes_at_ms = match network {
            WalletNetwork::Regtest(_) => None,
            _ => Some(now_ms + i64::from(blocks_left) * block_spacing_ms(network, tip_height)),
        };
        Some(Self { expiry_height, blocks_left, closes_at_ms })
    }

    /// Too late: a node would refuse the transaction now.
    pub fn is_closed(&self) -> bool {
        self.blocks_left == 0
    }
}

/// The target time between blocks after `tip_height`: 75 s, or 25 s once NU7
/// is active.
fn block_spacing_ms(network: WalletNetwork, tip_height: u64) -> i64 {
    use zcash_protocol::consensus::{NU7_POW_TARGET_SPACING_RATIO, SECONDS_PER_BLOCK};
    let next = BlockHeight::from_u32(u32::try_from(tip_height + 1).unwrap_or(u32::MAX));
    let seconds = if network.params().is_nu_active(NetworkUpgrade::Nu7, next) {
        SECONDS_PER_BLOCK / NU7_POW_TARGET_SPACING_RATIO
    } else {
        SECONDS_PER_BLOCK
    };
    i64::from(seconds) * 1000
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Build an unsigned Orchard transfer as a PCZT and return its sighash. Uses
/// the standard ZIP-317 fee and greedy input selection. No signing, no
/// broadcast — this only constructs the transaction.
///
/// Before building, the wallet's chain tip is refreshed from lightwalletd so the
/// transaction's expiry height is anchored to the *current* tip. Otherwise a
/// stale tip yields an expiry that may already be in the past by broadcast time,
/// and the node rejects the tx ("must not be mined at a block height greater
/// than its expiry"). The group must still approve it within the standard
/// expiry window (about 50 minutes; see [`ApprovalWindow`]).
#[allow(clippy::too_many_arguments)]
pub async fn prepare_send(
    data_dir: &Path,
    wallet_id: &str,
    network: WalletNetwork,
    recipient: &str,
    amount_zatoshis: u64,
    memo: Option<String>,
    lightwalletd_url: &str,
    db_key: &[u8],
) -> Result<DraftTransaction, CoreError> {
    let payment = PaymentRequest { recipient: recipient.to_string(), amount_zatoshis, memo };
    prepare_payments(data_dir, wallet_id, network, &[payment], lightwalletd_url, db_key).await
}

/// Build one unsigned transaction making several payments (a batch), as
/// [`prepare_send`] does for one. The group approves it once.
pub async fn prepare_payments(
    data_dir: &Path,
    wallet_id: &str,
    network: WalletNetwork,
    payments: &[PaymentRequest],
    lightwalletd_url: &str,
    db_key: &[u8],
) -> Result<DraftTransaction, CoreError> {
    use zcash_client_backend::data_api::wallet::input_selection::{GreedyInputSelector, SpendPolicy};
    use zcash_client_backend::fees::{standard::SingleOutputChangeStrategy, DustOutputPolicy};
    use zcash_keys::address::Address;
    use zcash_protocol::value::Zatoshis;
    use zcash_protocol::ShieldedPool;

    if payments.is_empty() || payments.len() > MAX_PAYMENTS {
        return Err(CoreError::Config(format!("a transaction makes 1 to {MAX_PAYMENTS} payments")));
    }
    let params = network.params();
    let mut built = Vec::with_capacity(payments.len());
    let mut requested = Vec::with_capacity(payments.len());
    let mut decoded = Vec::with_capacity(payments.len());
    for (i, p) in payments.iter().enumerate() {
        let which = if payments.len() > 1 { format!("payment {}: ", i + 1) } else { String::new() };
        let to = decode_recipient(network, &p.recipient).map_err(|e| CoreError::Crypto(format!("{which}{e}")))?;
        // A transparent recipient means an unshield (Orchard → transparent).
        // Memos are only valid for shielded outputs: one given for a
        // transparent recipient is dropped. Trimmed, as the memo reads back.
        let transparent = matches!(to, Address::Transparent(_));
        let memo = p.memo.as_deref().map(str::trim).filter(|m| !transparent && !m.is_empty()).map(str::to_string);
        let memo_bytes = memo
            .as_deref()
            .map(|m| {
                m.parse::<Memo>()
                    .map(|m| m.encode())
                    .map_err(|_| CoreError::Config(format!("{which}the memo is longer than 512 bytes")))
            })
            .transpose()?;
        let amount = Zatoshis::from_u64(p.amount_zatoshis)
            .ok()
            .filter(|a| !a.is_zero())
            .ok_or_else(|| CoreError::Config(format!("{which}the amount must be more than zero")))?;
        requested.push(
            zip321::Payment::new(to.to_zcash_address(&params), Some(amount), memo_bytes, None, None, vec![])
                .map_err(|e| CoreError::Config(format!("{which}{e:?}")))?,
        );
        decoded.push(to);
        built.push(DraftPayment {
            recipient: p.recipient.trim().to_string(),
            amount_zatoshis: p.amount_zatoshis,
            memo,
            transparent,
        });
    }
    let request = zip321::TransactionRequest::new(requested)
        .map_err(|e| CoreError::Config(format!("couldn't make the payment request ({e:?})")))?;

    let (mut db, account_id, tip_height) = open_for_send(data_dir, wallet_id, network, lightwalletd_url, db_key).await?;
    if payments.len() > 1 {
        // Among other payments, one to the group itself reads as change to
        // co-signers, who'd refuse the batch: say so now.
        use zcash_client_backend::data_api::Account as _;
        let own = db.get_account(account_id).ok().flatten().and_then(|a| a.ufvk().and_then(|k| k.orchard().cloned()));
        for (i, to) in decoded.iter().enumerate() {
            let ours = match (to, &own) {
                (Address::Unified(ua), Some(fvk)) => ua.orchard().is_some_and(|a| fvk.scope_for_address(a).is_some()),
                _ => false,
            };
            if ours {
                return Err(CoreError::Config(format!("payment {}: that's this wallet's own address", i + 1)));
            }
        }
    }
    // Let the backend choose the transaction version from the target height:
    // version 6 (carrying an Ironwood bundle) once NU6.3 is active, version 5
    // before it. Passing `None` is what enables Ironwood sends — post-NU6.3 the
    // Orchard pool is sealed, so a shielded payment to a unified address is
    // delivered through the Ironwood bundle of a V6 transaction. `lock_inputs`
    // is None: Cyze builds and signs one transaction at a time, so there is no
    // need to reserve inputs across concurrent proposals.
    let change_strategy = SingleOutputChangeStrategy::<GroupDb>::new(
        StandardFeeRule::Zip317,
        None, // change memo
        ShieldedPool::Orchard,
        DustOutputPolicy::default(),
    );
    let proposal = propose_transfer::<_, _, _, _, std::convert::Infallible>(
        &mut db,
        &params,
        account_id,
        &GreedyInputSelector::<GroupDb>::new(),
        &change_strategy,
        request,
        ConfirmationsPolicy::default(),
        &SpendPolicy::default(),
        None, // lock_inputs
        None, // proposed_version: derive V5/V6 from the target height
    )
    .map_err(|e| match e {
        zcash_client_backend::data_api::error::Error::InsufficientFunds { available, required } => {
            let what = if payments.len() > 1 { "these payments need" } else { "this payment needs" };
            CoreError::Ceremony(format!(
                "not enough spendable funds: {} {} can be spent now, and {what} {} with the fee. \
                 Funds received recently need 10 confirmations first",
                zec(available.into()),
                network.ticker(),
                zec(required.into()),
            ))
        }
        e => CoreError::Ceremony(format!("couldn't build the payment ({e:?})")),
    })?;

    draft_from_proposal(&mut db, network, account_id, tip_height, &proposal, built)
}

/// Build an unsigned transaction that pays everything spendable in a wallet to
/// `recipient` and leaves nothing behind as change: "send all", and how a
/// group that made its viewing key private empties its old address. The fee
/// comes out of the amount. Notes still waiting for confirmations stay. A memo
/// is dropped for a transparent recipient, which can't receive one.
pub async fn prepare_send_max(
    data_dir: &Path,
    wallet_id: &str,
    network: WalletNetwork,
    recipient: &str,
    memo: Option<String>,
    lightwalletd_url: &str,
    db_key: &[u8],
) -> Result<DraftTransaction, CoreError> {
    use zcash_client_backend::data_api::wallet::input_selection::LockedInputPolicy;
    use zcash_client_backend::data_api::MaxSpendMode;
    use zcash_keys::address::Address;
    use zcash_protocol::ShieldedPool;

    let params = network.params();
    let (mut db, account_id, tip_height) = open_for_send(data_dir, wallet_id, network, lightwalletd_url, db_key).await?;
    let to = decode_recipient(network, recipient)?;
    let is_unshield = matches!(to, Address::Transparent(_));
    let memo = if is_unshield { None } else { memo.filter(|m| !m.is_empty()) };
    let memo_bytes = memo
        .as_deref()
        .map(|m| m.parse::<Memo>().map(|m| m.encode()).unwrap_or_else(|_| MemoBytes::empty()));
    let proposal = propose_send_max_transfer::<_, _, _, std::convert::Infallible>(
        &mut db,
        &params,
        account_id,
        &[ShieldedPool::Orchard, ShieldedPool::Ironwood],
        &StandardFeeRule::Zip317,
        to.to_zcash_address(&params),
        memo_bytes,
        MaxSpendMode::MaxSpendable,
        ConfirmationsPolicy::default(),
        &LockedInputPolicy::Exclude,
        None, // lock_inputs
    )
    .map_err(|e| match format!("{e:?}") {
        m if m.contains("InsufficientFunds") => {
            CoreError::Ceremony("there's too little spendable to cover the network fee".into())
        }
        m => CoreError::Ceremony(format!("couldn't build the payment ({m})")),
    })?;
    let amount = proposal
        .steps()
        .last()
        .transaction_request()
        .total()
        .ok()
        .flatten()
        .map_or(0, u64::from);
    let payment = DraftPayment { recipient: recipient.trim().to_string(), amount_zatoshis: amount, memo, transparent: is_unshield };
    draft_from_proposal(&mut db, network, account_id, tip_height, &proposal, vec![payment])
}

/// Open a wallet to build a transaction: its chain tip refreshed from
/// lightwalletd, so the expiry is anchored to the live tip rather than whatever
/// sync last recorded, and scanned close enough to that tip to spend from.
/// Returns the tip too.
async fn open_for_send(
    data_dir: &Path,
    wallet_id: &str,
    network: WalletNetwork,
    lightwalletd_url: &str,
    db_key: &[u8],
) -> Result<(GroupDb, <GroupDb as WalletRead>::AccountId, u64), CoreError> {
    if let WalletNetwork::Regtest(level) = network {
        if level < RegtestUpgrade::Nu6_3 {
            return Err(CoreError::Ceremony(format!(
                "sending on a local devnet needs NU6.3 (Ironwood), and this devnet runs {}. \
                 Cyze builds only NU6.3 transactions; syncing and receiving work at any level",
                level.as_str()
            )));
        }
    }
    let db_path = wallet_db_path(data_dir, wallet_id, network);
    let mut db = open_db(&db_path, network, db_key)?;

    let mut client = connect(lightwalletd_url).await?;
    let tip_height = client
        .get_latest_block(ChainSpec {})
        .await
        .map_err(|e| CoreError::Connection(format!("get_latest_block: {e}")))?
        .into_inner()
        .height;
    db.update_chain_tip(BlockHeight::from_u32(tip_height as u32))
        .map_err(|e| CoreError::Crypto(format!("update chain tip: {e}")))?;
    let scanned = db
        .get_wallet_summary(ConfirmationsPolicy::default())
        .map_err(|e| CoreError::Crypto(format!("wallet summary: {e}")))?
        .map_or(0, |s| u64::from(s.fully_scanned_height()));
    ensure_caught_up(scanned, tip_height)?;

    let account_id = *db
        .get_account_ids()
        .map_err(|e| CoreError::Crypto(format!("wallet accounts: {e}")))?
        .first()
        .ok_or_else(|| CoreError::Crypto("wallet not initialized".into()))?;
    Ok((db, account_id, tip_height))
}

/// Whether `address` is a transparent address on `network` (a payment to it
/// is public and carries no memo).
pub fn is_transparent_address(network: WalletNetwork, address: &str) -> bool {
    matches!(decode_recipient(network, address), Ok(zcash_keys::address::Address::Transparent(_)))
}

/// Parse a recipient address for `network`, explaining a mismatch when the
/// address clearly belongs to another network.
pub(crate) fn decode_recipient(network: WalletNetwork, recipient: &str) -> Result<zcash_keys::address::Address, CoreError> {
    let r = recipient.trim();
    zcash_keys::address::Address::decode(&network.params(), r).ok_or_else(|| {
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
    })
}

/// Turn a proposal into the draft the group signs: the PCZT, its sighash, and
/// the α of each spend to sign.
fn draft_from_proposal(
    db: &mut GroupDb,
    network: WalletNetwork,
    account_id: <GroupDb as WalletRead>::AccountId,
    tip_height: u64,
    proposal: &zcash_client_backend::proposal::Proposal<StandardFeeRule, zcash_client_sqlite::ReceivedNoteId>,
    payments: Vec<DraftPayment>,
) -> Result<DraftTransaction, CoreError> {
    let fee = u64::from(proposal.steps().last().balance().fee_required());

    let pczt = create_pczt_from_proposal::<_, _, std::convert::Infallible, _, std::convert::Infallible, _>(
        db,
        &network.params(),
        account_id,
        OvkPolicy::Sender,
        proposal,
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
    let approval_window = ApprovalWindow::at(network, *pczt.global().expiry_height(), tip_height, now_ms());

    // Read each real spend's α (the re-randomization the FROST signers must
    // use), across both the Orchard and Ironwood bundles.
    let spends = spends_to_sign(pczt)?;

    let (recipient, memo) = match payments.as_slice() {
        [one] => (one.recipient.clone(), one.memo.clone()),
        _ => (String::new(), None),
    };
    Ok(DraftTransaction {
        pczt_hex,
        sighash_hex: hex::encode(sighash),
        spends,
        fee_zatoshis: fee,
        amount_zatoshis: payments.iter().map(|p| p.amount_zatoshis).sum(),
        recipient,
        is_unshield: payments.iter().any(|p| p.transparent),
        memo,
        payments,
        approval_window,
    })
}

/// How many blocks the wallet's scan may trail the live chain tip and still
/// build a transaction. Auto-sync keeps an open wallet within a block of the
/// tip; this leaves room for blocks mined since the last scan.
pub const SEND_MAX_SCAN_LAG: u64 = 3;

/// Refuse to build from a wallet that hasn't scanned up to the chain tip.
///
/// Any member can coordinate a send, so a note this wallet still lists as
/// unspent may have been spent in a block it hasn't scanned yet. Building on it
/// would run a whole signing ceremony for a transaction the network rejects as a
/// double spend.
fn ensure_caught_up(scanned: u64, tip: u64) -> Result<(), CoreError> {
    let behind = tip.saturating_sub(scanned);
    if behind > SEND_MAX_SCAN_LAG {
        return Err(CoreError::NotSynced { behind });
    }
    Ok(())
}

/// An amount in zatoshis as a decimal amount of the coin, trailing zeros trimmed.
fn zec(zatoshis: u64) -> String {
    let whole = format!("{}.{:08}", zatoshis / 100_000_000, zatoshis % 100_000_000);
    whole.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Whether a wallet scanned to `scanned` is close enough to `tip` to build a
/// transaction from (see [`ensure_caught_up`]).
pub fn is_caught_up(scanned: u64, tip: u64) -> bool {
    ensure_caught_up(scanned, tip).is_ok()
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
                        alpha_hex: hex::encode(alpha.to_bytes()),
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
        let pk = ProvingKey::build(circuit_version)
            .map_err(|e| CoreError::Ceremony(format!("orchard proving key: {e:?}")))?;

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
    /// Every payment a send made to addresses outside the wallet, as far as
    /// the wallet recovered them (see the send query): more than one for a
    /// batch. Empty for receipts.
    pub payments: Vec<TxPayment>,
}

/// One payment a send made, from the wallet's record of its outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TxPayment {
    pub recipient: Option<String>,
    pub amount_zatoshis: u64,
    pub memo: Option<String>,
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
    wallet_id: &str,
    network: WalletNetwork,
    db_key: &[u8],
) -> Result<u64, CoreError> {
    let db_path = wallet_db_path(data_dir, wallet_id, network);
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
    wallet_id: &str,
    network: WalletNetwork,
    db_key: &[u8],
) -> Result<Vec<NoteRecord>, CoreError> {
    let db_path = wallet_db_path(data_dir, wallet_id, network);
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
    notes.sort_by_key(|n| std::cmp::Reverse(n.value_zatoshis));
    Ok(notes)
}

pub fn wallet_history(
    data_dir: &Path,
    wallet_id: &str,
    network: WalletNetwork,
    db_key: &[u8],
) -> Result<Vec<TxRecord>, CoreError> {
    let db_path = wallet_db_path(data_dir, wallet_id, network);
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
                payments: Vec::new(),
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
    //
    // Each external output, for listing a batch's payments.
    let mut payments_by_tx: std::collections::HashMap<Vec<u8>, Vec<TxPayment>> = std::collections::HashMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT t.txid, sn.to_address, sn.value, sn.memo \
                 FROM sent_notes sn \
                 JOIN transactions t ON t.id_tx = sn.transaction_id \
                 WHERE sn.from_account_id = ?1 AND sn.to_account_id IS NULL \
                 ORDER BY sn.output_pool, sn.output_index",
            )
            .map_err(|e| CoreError::Crypto(format!("prepare payments query: {e}")))?;
        let rows = stmt
            .query_map([account_id], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                ))
            })
            .map_err(|e| CoreError::Crypto(format!("execute payments query: {e}")))?;
        for row in rows {
            let (txid, recipient, value, memo) = row.map_err(|e| CoreError::Crypto(format!("payment row: {e}")))?;
            payments_by_tx.entry(txid).or_default().push(TxPayment {
                recipient,
                amount_zatoshis: value.max(0) as u64,
                memo: memo.as_deref().and_then(decode_zcash_memo),
            });
        }
    }
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
            let payments = payments_by_tx.remove(&txid_bytes).unwrap_or_default();
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
                payments,
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
    fn pending_transactions_are_encrypted_and_older_plaintext_ones_still_load() {
        let dir = tempfile::tempdir().unwrap();
        let (key, other) = ([7u8; 32], [8u8; 32]);
        let path = save_pending_tx(dir.path(), "w1", WalletNetwork::Main, "c1", "deadbeef", &key).unwrap();
        assert!(!std::fs::read(&path).unwrap().windows(8).any(|w| w == b"deadbeef"));
        assert_eq!(load_pending_tx(dir.path(), "w1", WalletNetwork::Main, "c1", &key).unwrap(), "deadbeef");
        assert!(load_pending_tx(dir.path(), "w1", WalletNetwork::Main, "c1", &other).is_err());

        std::fs::write(pending_tx_path(dir.path(), "w1", WalletNetwork::Main, "c2"), "cafe01\n").unwrap();
        assert_eq!(load_pending_tx(dir.path(), "w1", WalletNetwork::Main, "c2", &key).unwrap(), "cafe01");
    }

    #[test]
    fn approvals_close_once_a_node_would_refuse_the_transaction() {
        let at = |tip| ApprovalWindow::at(WalletNetwork::Main, 1_040, tip, 0).unwrap();
        // At a tip of 1,036 the next block is 1,037, and 1,037 + 3 still fits
        // under the expiry; one block later it doesn't.
        assert_eq!(at(1_000).blocks_left, 37);
        assert_eq!(at(1_036).blocks_left, 1);
        assert!(!at(1_036).is_closed());
        assert!(at(1_037).is_closed());
        assert!(at(2_000).is_closed());
        assert_eq!(ApprovalWindow::at(WalletNetwork::Main, 0, 1_000, 0), None, "expiry 0 never expires");
    }

    #[test]
    fn the_approval_deadline_follows_the_block_spacing() {
        let nu7 = u64::from(u32::from(
            WalletNetwork::Main.params().activation_height(NetworkUpgrade::Nu7).unwrap(),
        ));
        let before = ApprovalWindow::at(WalletNetwork::Main, 2_000_040, 2_000_000, 1_000).unwrap();
        assert_eq!(before.closes_at_ms, Some(1_000 + 37 * 75_000));
        let after = ApprovalWindow::at(WalletNetwork::Main, (nu7 + 220) as u32, nu7 + 100, 1_000).unwrap();
        assert_eq!(after.closes_at_ms, Some(1_000 + 117 * 25_000));
        // A devnet mines on demand, so only the blocks count.
        let devnet = ApprovalWindow::at(WalletNetwork::Regtest(RegtestUpgrade::Nu7), 160, 100, 1_000).unwrap();
        assert_eq!((devnet.blocks_left, devnet.closes_at_ms), (57, None));
    }

    #[test]
    fn amounts_read_as_decimal_coins() {
        assert_eq!(zec(150_000_000), "1.5");
        assert_eq!(zec(100_000_000), "1");
        assert_eq!(zec(10_000), "0.0001");
        assert_eq!(zec(0), "0");
    }

    #[test]
    fn sends_need_a_wallet_scanned_near_the_tip() {
        assert!(ensure_caught_up(1_000, 1_000).is_ok());
        assert!(ensure_caught_up(1_000 - SEND_MAX_SCAN_LAG, 1_000).is_ok());
        let err = ensure_caught_up(1_000 - SEND_MAX_SCAN_LAG - 1, 1_000).unwrap_err();
        assert!(matches!(err, CoreError::NotSynced { behind: 4 }), "{err}");
        // A db whose recorded tip is ahead of the server's is not "behind".
        assert!(ensure_caught_up(1_010, 1_000).is_ok());
    }

    #[test]
    fn run_blocking_works_on_both_runtime_flavors() {
        // Outside any runtime, and on a current-thread runtime: runs inline.
        assert_eq!(run_blocking(|| 7), 7);
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert_eq!(rt.block_on(async { run_blocking(|| 7) }), 7);
        // On a worker of the multi-threaded runtime: via block_in_place.
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        let v = rt.block_on(async { tokio::spawn(async { run_blocking(|| 7) }).await.unwrap() });
        assert_eq!(v, 7);
    }

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
        assert_eq!(RegtestUpgrade::from_branch_id("77190ad9"), Some(RegtestUpgrade::Nu7));
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
            (RegtestUpgrade::Nu7, "77190ad9"),
        ] {
            let net = WalletNetwork::Regtest(level);
            assert_eq!(branch_id_for_height(net, 500), branch, "{level:?}");
            assert_eq!(RegtestUpgrade::from_branch_id(branch), Some(level));
        }
        let nu6 = RegtestUpgrade::Nu6.local_network();
        assert_eq!(nu6.activation_height(NetworkUpgrade::Nu5), Some(BlockHeight::from_u32(1)));
        assert_eq!(nu6.activation_height(NetworkUpgrade::Nu6_1), None);
        assert_eq!(nu6.activation_height(NetworkUpgrade::Nu6_3), None);
        assert_eq!(RegtestUpgrade::Nu6_3.local_network().activation_height(NetworkUpgrade::Nu7), None);
    }

    /// NU7 changes the branch id but not the Orchard circuit; picking the
    /// circuit by branch would have made every proof after mainnet's NU7
    /// activation (3,543,405) invalid.
    #[test]
    fn the_post_ironwood_circuit_is_used_across_nu7() {
        use orchard::circuit::OrchardCircuitVersion::{FixedPostNu6_2, PostNu6_3};
        assert_eq!(orchard_circuit_version_for_height(WalletNetwork::Main, 3_428_142), FixedPostNu6_2);
        assert_eq!(orchard_circuit_version_for_height(WalletNetwork::Main, 3_428_143), PostNu6_3);
        assert_eq!(orchard_circuit_version_for_height(WalletNetwork::Main, 3_543_405), PostNu6_3);
        assert_ne!(branch_id_for_height(WalletNetwork::Main, 3_543_405), NU6_3_BRANCH_ID);
        for level in [RegtestUpgrade::Nu6_3, RegtestUpgrade::Nu7] {
            assert_eq!(orchard_circuit_version_for_height(WalletNetwork::Regtest(level), 10), PostNu6_3);
        }
    }

    #[test]
    fn heights_from_ages_use_the_block_time_of_each_era() {
        let nu7 = 3_543_405;
        // Before NU7: 75-second blocks.
        assert_eq!(height_before(WalletNetwork::Main, 3_528_000, 75 * 100), 3_527_900);
        // After it: 25-second blocks, then 75-second ones before activation.
        assert_eq!(height_before(WalletNetwork::Main, nu7 + 300, 25 * 100), nu7 + 200);
        assert_eq!(height_before(WalletNetwork::Main, nu7 + 300, 25 * 300 + 75 * 10), nu7 - 10);
        assert_eq!(height_before(WalletNetwork::Main, 100, u64::MAX / 2), 0);
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
    fn the_quickest_server_near_the_tip_is_chosen() {
        let probe = |url: &str, latency_ms, height| ServerProbe { url: url.into(), latency_ms, height };
        // The fastest one is far behind; the next fastest that's current wins.
        let chosen = choose_server(vec![
            probe("lagging", 10, 1_000),
            probe("slow", 90, 1_010),
            probe("quick", 40, 1_008),
        ]);
        assert_eq!(chosen.map(|p| p.url), Some("quick".to_string()));
        assert_eq!(choose_server(vec![]), None);
        assert!(WalletNetwork::Main.known_lightwalletd().contains(&WalletNetwork::Main.default_lightwalletd()));
    }

    #[test]
    fn a_batch_must_start_right_after_its_tree_state() {
        use zcash_primitives::block::BlockHash;
        let state = ChainState::empty(BlockHeight::from_u32(99), BlockHash([7; 32]));
        let block = |prev: u8| CompactBlock { height: 100, prev_hash: vec![prev; 32], ..Default::default() };
        assert!(blocks_follow(&state, &[block(7)]));
        assert!(!blocks_follow(&state, &[block(8)]));
        assert!(blocks_follow(&state, &[]));
    }

    /// Inside mainnet's spam range, batches are small; a batch before it stops
    /// where it starts, and normal batches resume after it.
    #[test]
    fn spam_blocks_are_scanned_in_small_batches() {
        let h = BlockHeight::from_u32;
        let spam = Some(1_000..1_250);
        let range = ScanRange::from_parts(h(400)..h(1_800), ScanPriority::Historic);
        let lens: Vec<(u32, u32)> = split_scan_range(range, 500, spam)
            .iter()
            .map(|b| (u32::from(b.block_range().start), b.len() as u32))
            .collect();
        assert_eq!(
            lens,
            vec![(400, 500), (900, 100), (1_000, 100), (1_100, 100), (1_200, 50), (1_250, 500), (1_750, 50)]
        );
        assert_eq!(spam_range(&WalletNetwork::Main.params()), Some(MAINNET_SPAM));
        assert_eq!(spam_range(&WalletNetwork::Test.params()), None);
    }

    #[test]
    fn split_scan_range_matches_upstream_batching() {
        let h = BlockHeight::from_u32;
        let range = ScanRange::from_parts(h(100)..h(1050), ScanPriority::Historic);

        // An evenly-plus-remainder range → full batches then a short tail.
        let batches = split_scan_range(range.clone(), 400, None);
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
        let one = split_scan_range(small.clone(), 5000, None);
        assert_eq!(one.len(), 1);
        assert_eq!(*one[0].block_range(), h(10)..h(30));

        // A range that is an exact multiple of the batch size → no empty tail.
        let exact = ScanRange::from_parts(h(0)..h(1000), ScanPriority::Historic);
        let even = split_scan_range(exact, 500, None);
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
            let keys = crate::zcash::derive_orchard_keys(&ak, None, net.network_type()).unwrap();
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
