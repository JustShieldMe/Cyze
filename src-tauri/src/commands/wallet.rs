//! Zcash light-wallet commands: lightwalletd config + connectivity. Network and
//! endpoint are user-configurable; mainnet is the default, with testnet a click
//! away in settings.

use frost_app_core::ciphersuite::Suite;
use frost_app_core::signing::{run_coordinator, CoordinatorParams};
use frost_app_core::wallet::{self, LightwalletdInfo, RegtestUpgrade, WalletNetwork, WalletStatus};
use frost_client::api::PublicKey;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::{AppState, CeremonyHandle, Settings, WalletKind};

/// The wallet network the app is configured for. For a local devnet this
/// includes its upgrade level: the user's override if set, otherwise the level
/// last detected from the node, otherwise NU6 (thus-spoke-zakura's default).
/// Every command resolves the network through here so they all agree.
pub(crate) fn configured_network(settings: &Settings) -> WalletNetwork {
    match settings.wallet_network.as_deref().unwrap_or("main") {
        "regtest" => WalletNetwork::Regtest(
            settings
                .regtest_upgrade
                .as_deref()
                .or(settings.regtest_detected.as_deref())
                .and_then(RegtestUpgrade::parse)
                .unwrap_or_default(),
        ),
        other => WalletNetwork::from_str(other),
    }
}

#[derive(Serialize)]
pub struct WalletConfig {
    /// "main", "test", or "regtest".
    pub network: String,
    pub lightwalletd_url: String,
    /// Regtest: the forced upgrade level, or `None` for auto.
    pub regtest_upgrade: Option<String>,
    /// Regtest: the upgrade level last detected from the node, if any.
    pub regtest_detected: Option<String>,
    /// Regtest: the level actually in use (override, else detected, else NU6).
    pub regtest_effective: Option<String>,
    /// No server was chosen: the app uses the fastest known one (`pick_server`).
    pub automatic: bool,
    /// How quickly the automatically chosen server answered, once one was.
    pub auto_latency_ms: Option<u64>,
}

/// Resolve the effective wallet config. With no lightwalletd endpoint saved,
/// that's the server picked automatically (`pick_server`), or the network's
/// default until one is.
pub(crate) fn resolve_config(state: &AppState) -> WalletConfig {
    let s = state.load_settings();
    // Mainnet is the default: it is where the wallet is actually used, and the
    // mainnet guardrails (badge, confirmation modal, address checks) make the
    // active network unmissable. Testnet remains one click away in settings.
    let net = configured_network(&s);
    let saved = s.lightwalletd_url.clone().filter(|u| !u.trim().is_empty());
    let automatic = saved.is_none() && !net.known_lightwalletd().is_empty();
    let auto = automatic
        .then(|| state.auto_lightwalletd.lock().ok()?.get(net.as_str()).cloned())
        .flatten();
    let auto_latency_ms = auto.as_ref().map(|p| p.latency_ms);
    let lightwalletd_url = saved
        .or(auto.map(|p| p.url))
        .unwrap_or_else(|| net.default_lightwalletd().to_string());
    let regtest_effective = match net {
        WalletNetwork::Regtest(level) => Some(level.as_str().to_string()),
        _ => None,
    };
    WalletConfig {
        network: net.as_str().to_string(),
        lightwalletd_url,
        regtest_upgrade: s.regtest_upgrade.clone(),
        regtest_detected: s.regtest_detected.clone(),
        regtest_effective,
        automatic,
        auto_latency_ms,
    }
}

/// With no server chosen, pick the fastest of the known ones on the network
/// (see `wallet::fastest_lightwalletd`): once a session, and again with
/// `again` after the one in use stopped answering. Returns whether the server
/// in use changed.
pub(crate) async fn pick_server(state: &AppState, again: bool) -> bool {
    let cfg = resolve_config(state);
    if !cfg.automatic || (cfg.auto_latency_ms.is_some() && !again) {
        return false;
    }
    let net = configured_network(&state.load_settings());
    // Each probe through Tor builds a circuit first.
    let timeout = std::time::Duration::from_secs(if wallet::tor::is_on() { 30 } else { 5 });
    let Some(best) = wallet::fastest_lightwalletd(net, net.known_lightwalletd(), timeout).await else {
        tracing::warn!("none of the known sync servers answered");
        return false;
    };
    tracing::info!(url = %best.url, latency_ms = best.latency_ms, "using the fastest sync server");
    let changed = best.url != cfg.lightwalletd_url;
    if let Ok(mut picks) = state.auto_lightwalletd.lock() {
        picks.insert(net.as_str(), best);
    }
    changed
}

#[tauri::command]
pub async fn get_wallet_config(state: State<'_, AppState>) -> AppResult<WalletConfig> {
    Ok(resolve_config(&state))
}

/// Save the wallet network and endpoint. An empty URL clears the override,
/// reverting to the network's default endpoint. `regtest_upgrade` forces a
/// devnet upgrade level (`"nu6"` … `"nu7"`); omitted or empty means auto.
#[tauri::command]
pub async fn set_wallet_config(
    state: State<'_, AppState>,
    network: String,
    lightwalletd_url: String,
    regtest_upgrade: Option<String>,
) -> AppResult<WalletConfig> {
    let mut settings = state.load_settings();
    settings.wallet_network = Some(WalletNetwork::from_str(&network).as_str().to_string());
    settings.regtest_upgrade = match regtest_upgrade.as_deref().map(str::trim) {
        None | Some("") | Some("auto") => None,
        Some(level) => Some(
            RegtestUpgrade::parse(level)
                .ok_or_else(|| AppError::new("config", format!("unknown devnet upgrade level '{level}'")))?
                .as_str()
                .to_string(),
        ),
    };
    let url = lightwalletd_url.trim();
    // Refuse to persist a plaintext (non-loopback http://) endpoint so wallet
    // traffic is never silently downgraded off TLS.
    if !url.is_empty() {
        wallet::validate_endpoint_security(url)?;
    }
    settings.lightwalletd_url = (!url.is_empty()).then(|| url.to_string());
    state.save_settings(&settings)?;
    // The wallet now lives on a different network or server: resync from it.
    state.sync_service.wake(true);
    Ok(resolve_config(&state))
}

/// Probe a lightwalletd endpoint and return its chain info (connectivity test).
/// Uses the configured endpoint when `url` is omitted.
#[tauri::command]
pub async fn lightwalletd_info(
    state: State<'_, AppState>,
    url: Option<String>,
) -> AppResult<LightwalletdInfo> {
    let url = url
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| resolve_config(&state).lightwalletd_url);
    let mut info = wallet::lightwalletd_info(&url).await?;
    record_regtest_level(&state, &info)?;
    // Compute the consensus branch id this build would use at the node's height
    // and flag a mismatch (e.g. after a network upgrade like Ironwood the node
    // expects a branch id this build doesn't yet know), so the UI can warn
    // before a whole signing ceremony is spent on a tx the node will reject.
    let network = configured_network(&state.load_settings());
    let wallet_branch = wallet::branch_id_for_height(network, info.block_height);
    info.branch_supported = Some(!info.consensus_branch_id.is_empty()
        && info.consensus_branch_id == wallet_branch);
    info.wallet_branch_id = Some(wallet_branch);
    Ok(info)
}

/// What "Detect local devnet" found.
#[derive(Serialize)]
pub struct DevnetStatus {
    /// The thus-spoke-zakura CLI is installed (Linux/macOS only).
    pub installed: bool,
    /// The devnet's lightwalletd answered and reports a regtest chain.
    pub running: bool,
    /// The devnet's lightwalletd URL (loopback, plaintext), when known.
    pub lightwalletd: Option<String>,
    /// The devnet's browser dashboard (faucet, mining), when known.
    pub dashboard: Option<String>,
    /// Chain height reported by the devnet's lightwalletd, when running.
    pub block_height: Option<u64>,
    /// The upgrade level read from the node (`nu6` … `nu7`), when running.
    pub detected_upgrade: Option<String>,
    /// Why the devnet isn't usable yet, phrased as what to do next.
    pub detail: Option<String>,
}

/// Find a local thus-spoke-zakura devnet and check that it's up. `instance` is
/// the launcher's `--name` (default `"default"`). Never changes the saved
/// endpoint — the UI fills the field and the user saves — but it does record
/// the devnet's upgrade level, like a connection test.
#[tauri::command]
pub async fn detect_local_devnet(
    state: State<'_, AppState>,
    instance: Option<String>,
) -> AppResult<DevnetStatus> {
    use crate::devnet::{lookup, Lookup};
    let instance = instance
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".into());
    let mut status = DevnetStatus {
        installed: false,
        running: false,
        lightwalletd: None,
        dashboard: None,
        block_height: None,
        detected_upgrade: None,
        detail: None,
    };
    let endpoints = match lookup(&instance).await? {
        Lookup::NotInstalled => {
            status.detail = Some(if cfg!(target_os = "windows") {
                "thus-spoke-zakura runs on Linux and macOS. On Windows, run it in WSL2 and enter its lightwalletd URL by hand.".into()
            } else {
                "thus-spoke-zakura isn't installed (looked on PATH and in ~/.local/bin).".into()
            });
            return Ok(status);
        }
        Lookup::NoInstance(reason) => {
            status.installed = true;
            status.detail = Some(format!(
                "No devnet named '{instance}' is set up ({reason}). Start one with `thus-spoke-zakura`."
            ));
            return Ok(status);
        }
        Lookup::Found(e) => e,
    };
    status.installed = true;
    status.dashboard = Some(endpoints.dashboard.clone());
    // The devnet binds to loopback only. Anything else isn't the devnet this
    // detector is for, and must not be offered as a plaintext endpoint.
    if !wallet::is_loopback_host(&endpoints.lightwalletd) {
        status.detail = Some(format!(
            "the devnet reported a non-local lightwalletd ({}); enter the endpoint by hand",
            endpoints.lightwalletd
        ));
        return Ok(status);
    }
    status.lightwalletd = Some(endpoints.lightwalletd.clone());
    // `endpoints` reads a file that outlives the devnet, so probe it.
    match wallet::lightwalletd_info(&endpoints.lightwalletd).await {
        Ok(info) if info.chain_name == "regtest" => {
            record_regtest_level(&state, &info)?;
            status.running = true;
            status.block_height = Some(info.block_height);
            status.detected_upgrade = RegtestUpgrade::from_branch_id(&info.consensus_branch_id)
                .map(|u| u.as_str().to_string());
        }
        Ok(info) => {
            status.detail = Some(format!(
                "{} is a '{}' node, not a regtest devnet",
                endpoints.lightwalletd, info.chain_name
            ));
        }
        Err(e) => {
            tracing::debug!(error = %e, url = %endpoints.lightwalletd, "devnet: probe failed");
            status.detail = Some(format!(
                "Found devnet '{instance}', but it isn't running. Start it with `thus-spoke-zakura`."
            ));
        }
    }
    Ok(status)
}

/// When a probed node is a regtest devnet, remember which upgrade it runs (read
/// from its consensus branch id) so "auto" builds for the same rules. Keyed on
/// what the node reports, not the saved network, so testing a devnet endpoint
/// before saving it still records the level.
pub(crate) fn record_regtest_level(state: &AppState, info: &LightwalletdInfo) -> AppResult<()> {
    if info.chain_name != "regtest" {
        return Ok(());
    }
    let detected = RegtestUpgrade::from_branch_id(&info.consensus_branch_id).map(|u| u.as_str().to_string());
    let mut settings = state.load_settings();
    if settings.regtest_detected != detected {
        tracing::info!(branch = %info.consensus_branch_id, level = ?detected, "devnet: detected upgrade level");
        settings.regtest_detected = detected;
        state.save_settings(&settings)?;
    }
    Ok(())
}

/// Resolve the wallet context for a RedPallas group: (network, lightwalletd
/// URL, derived UFVK string) of its current wallet.
pub(crate) async fn group_wallet_ctx(
    state: &AppState,
    group_id: &str,
) -> AppResult<(WalletNetwork, String, String)> {
    let ctx = wallet_ctx(state, group_id, WalletKind::Current).await?;
    Ok((ctx.network, ctx.url, ctx.ufvk))
}

/// One of a group's wallets: what it watches and where it lives.
pub(crate) struct WalletCtx {
    pub network: WalletNetwork,
    pub url: String,
    pub ufvk: String,
    /// Its folder under `wallets/` (opaque; see `AppState::wallet_dir_id_of`).
    pub dir: String,
    /// What its start height and subtree-root refresh are recorded under.
    pub key: String,
}

/// Resolve one of a RedPallas group's wallets (see [`WalletKind`]).
pub(crate) async fn wallet_ctx(state: &AppState, group_id: &str, kind: WalletKind) -> AppResult<WalletCtx> {
    let ciphersuite = state
        .with_config(|config| {
            config
                .group
                .get(group_id)
                .map(|g| g.ciphersuite.clone())
                .ok_or_else(|| AppError::new("config", "group not found"))
        })
        .await?;
    if !ciphersuite.contains("Pallas") {
        return Err(AppError::new(
            "config",
            "the wallet is only available for RedPallas (Zcash) groups",
        ));
    }
    let cfg = resolve_config(state);
    let network = configured_network(&state.load_settings());
    let has_old = state.has_old_viewing_key(group_id).await?;
    if kind == WalletKind::Old && !has_old {
        return Err(AppError::new("config", "this group has no old address"));
    }
    // The group id is the hex of its verifying key (the Orchard ak). The old
    // wallet watches the key derived from it alone.
    let seed = match kind {
        WalletKind::Current => state.viewing_seed(group_id).await?,
        WalletKind::Old => None,
    };
    let ufvk = frost_app_core::zcash::derive_orchard_keys_hex(group_id, seed.as_deref(), network.network_type())?.ufvk;
    let base = crate::state::wallet_session_key(group_id, network.dir_name());
    Ok(WalletCtx {
        network,
        url: cfg.lightwalletd_url,
        ufvk,
        dir: state.wallet_dir_id_of(group_id, kind).await?,
        key: if kind == WalletKind::Current && has_old { format!("{base}/private") } else { base },
    })
}

/// Wallet balance + sync status for a group (reads the local db; no network).
#[tauri::command]
pub async fn wallet_group_status(
    state: State<'_, AppState>,
    group_id: String,
) -> AppResult<WalletStatus> {
    let (network, _url, ufvk) = group_wallet_ctx(&state, &group_id).await?;
    let db_key = state.wallet_db_key(&group_id).await?;
    let wid = state.wallet_dir_id(&group_id).await?;
    Ok(wallet::group_status(
        &state.data_dir,
        &wid,
        network,
        &ufvk,
        db_key.as_ref(),
    )?)
}

/// Import the group's UFVK as a view-only account. Idempotent. Touches the
/// network. Returns the first block the wallet will scan (0 if already imported).
///
/// `birthday_height` selects that first block. When omitted, a birthday recorded
/// from a previous import is reused — this is what lets a rebuilt wallet
/// database recover its funds instead of starting at the chain tip — and
/// otherwise the account starts at the tip, correct for a brand-new group.
#[tauri::command]
pub async fn wallet_init_account(
    state: State<'_, AppState>,
    group_id: String,
    birthday_height: Option<u64>,
) -> AppResult<u64> {
    init_account(&state, &group_id, birthday_height).await
}

/// Slack added to a group's age when estimating its birthday from its creation
/// time, covering clock skew and block-time variance. A day of blocks scans in
/// a few seconds; starting late would hide funds.
const ESTIMATED_BIRTHDAY_MARGIN_SECS: u64 = 24 * 60 * 60;

/// Set up a group's view-only account (see [`wallet_init_account`]). Shared by
/// the command and the background sync service.
///
/// The first block to scan comes from, in order: `birthday_height`; the height
/// recorded for this group on this network (at key generation, or by an
/// earlier import); the legacy per-group record; an estimate from when the
/// group was created; otherwise the chain tip.
pub(crate) async fn init_account(
    state: &AppState,
    group_id: &str,
    birthday_height: Option<u64>,
) -> AppResult<u64> {
    init_account_of(state, group_id, WalletKind::Current, birthday_height).await
}

/// [`init_account`] for one of a group's wallets.
pub(crate) async fn init_account_of(
    state: &AppState,
    group_id: &str,
    kind: WalletKind,
    birthday_height: Option<u64>,
) -> AppResult<u64> {
    let WalletCtx { network, url, ufvk, dir: wid, key } = wallet_ctx(state, group_id, kind).await?;
    let db_key = state.wallet_db_key(group_id).await?;

    // A devnet always scans its whole (tiny) chain (see
    // `default_birthday_height`) and is reset often, so it neither reuses a
    // recorded height (a public-chain ~4M would hide every devnet note) nor
    // records its own.
    let is_devnet = matches!(network, WalletNetwork::Regtest(_));
    let private = state.load_private().await?;
    let recorded = if is_devnet {
        None
    } else {
        private
            .network_birthdays
            .get(&key)
            .or_else(|| private.wallet_birthdays.get(group_id))
            .copied()
    };
    let estimated = match (recorded, private.group_created_at.get(group_id)) {
        (None, Some(&created)) if !is_devnet && birthday_height.is_none() => {
            let tip = shared_lightwalletd(state, &url).await?.latest_height().await?;
            let age = unix_now().saturating_sub(created);
            Some(wallet::height_before(network, tip, age + ESTIMATED_BIRTHDAY_MARGIN_SECS))
        }
        _ => None,
    };
    let requested = birthday_height.or(recorded).or(estimated);

    let scan_from = wallet::init_group_account(
        &state.data_dir,
        &wid,
        network,
        &ufvk,
        &url,
        db_key.as_ref(),
        requested,
    )
    .await?;

    // A freshly imported account lives in a new database that has no subtree
    // roots yet, so its first sync must fetch them even if an earlier database
    // for this group did so this session.
    if scan_from > 0 {
        state.subtree_roots_fetched.lock().await.remove(&key);
    }

    // Remember where this wallet starts, so a later rebuild of the (deleted)
    // wallet database scans from here again rather than from the chain tip.
    // `scan_from == 0` means the account already existed; nothing was imported.
    if scan_from > 0 && !is_devnet && recorded != Some(scan_from) {
        state.update_private(|p| p.network_birthdays.insert(key, scan_from)).await?;
    }
    Ok(scan_from)
}

/// How far before its old start a wallet rebuilt after a deep reorg rescans.
/// Reorgs are a few blocks deep in practice; a thousand scan in about a second.
const REORG_REBUILD_MARGIN: u64 = 1_000;

/// Recover from a reorg below a wallet's start (`CoreError::Reorg`): delete
/// its database and move its recorded start height back, so the next setup
/// rescans from before the fork. Nothing is lost: the funds are on-chain.
pub(crate) async fn rebuild_after_reorg(state: &AppState, group_id: &str, kind: WalletKind) -> AppResult<()> {
    let WalletCtx { network, dir: wid, key, .. } = wallet_ctx(state, group_id, kind).await?;
    // A devnet always rescans its whole chain, so it only needs the delete.
    if !matches!(network, WalletNetwork::Regtest(_)) {
        let db_key = state.wallet_db_key(group_id).await?;
        let from_db = wallet::wallet_start_height(&state.data_dir, &wid, network, db_key.as_ref())?;
        let private = state.load_private().await?;
        let recorded = private.network_birthdays.get(&key).or(private.wallet_birthdays.get(group_id)).copied();
        if let Some(start) = [from_db, recorded].into_iter().flatten().min() {
            let key = key.clone();
            state
                .update_private(|p| p.network_birthdays.insert(key, start.saturating_sub(REORG_REBUILD_MARGIN)))
                .await?;
        }
    }
    wallet::delete_wallet_db(&state.data_dir, &wid, network)?;
    state.subtree_roots_fetched.lock().await.remove(&key);
    Ok(())
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Record when a new group was created and, if lightwalletd answers, the chain
/// tip at that moment, as the group's wallet birthday on the configured
/// network. Called right after key generation, so every member's device starts
/// scanning where the group began, even if they first open the wallet weeks
/// later, after it has received funds. Best effort: a failure only means the
/// wallet falls back to estimating from the creation time.
pub(crate) async fn record_new_group_birthday(state: &AppState, group_id: &str) {
    let network = configured_network(&state.load_settings());
    let mut tip = None;
    if !matches!(network, WalletNetwork::Regtest(_)) {
        let url = resolve_config(state).lightwalletd_url;
        match tokio::time::timeout(std::time::Duration::from_secs(15), async {
            wallet::Lightwalletd::connect(&url).await?.latest_height().await
        })
        .await
        {
            Ok(Ok(height)) => tip = Some(height),
            Ok(Err(e)) => tracing::warn!("new group: couldn't read the chain tip for its birthday: {e}"),
            Err(_) => tracing::warn!("new group: timed out reading the chain tip for its birthday"),
        }
    }
    let key = crate::state::wallet_session_key(group_id, network.dir_name());
    let saved = state
        .update_private(|p| {
            p.group_created_at.entry(group_id.to_string()).or_insert_with(unix_now);
            if let Some(tip) = tip {
                p.network_birthdays.entry(key).or_insert(tip);
            }
        })
        .await;
    if let Err(e) = saved {
        tracing::warn!("new group: couldn't save its birthday: {}", e.message);
    }
}

/// Sync one group's wallet from lightwalletd and return its updated status.
/// Long-running; touches the network. Run by the background sync service; a
/// running sync is cancelled through its token in `AppState::sync_cancels`.
pub(crate) async fn sync_once(
    state: &AppState,
    group_id: &str,
    kind: WalletKind,
) -> AppResult<(WalletStatus, wallet::SyncStats)> {
    let group_id = group_id.to_string();
    let WalletCtx { network, url, ufvk, dir: wid, key: roots_key } = wallet_ctx(state, &group_id, kind).await?;
    let db_key = state.wallet_db_key(&group_id).await?;

    // Register a fresh cancellation token for this group, tripping (and replacing)
    // any prior one so at most one sync per group is ever live. "Sync Now" cancels
    // the previous run before this one starts, avoiding two writers on one db.
    let cancel = tokio_util::sync::CancellationToken::new();
    {
        let mut cancels = state.sync_cancels.lock().await;
        if let Some(prev) = cancels.insert(group_id.clone(), cancel.clone()) {
            prev.cancel();
        }
    }

    // Hold the app-wide sync gate for the whole run so only one wallet ever syncs
    // at a time (trial decryption is CPU-bound; concurrent group syncs only
    // thrash). We register+cancel the prior same-group token above first, so a
    // restart of this group releases the gate before we wait on it; an
    // active-wallet switch cancels the other group's sync, so this rarely blocks.
    // Holding it for the whole run also means a restarting sync can't race a
    // cancelled one's still-open db connection — the "database is locked" case.
    let _gate = state.sync_gate.lock().await;

    let lightwalletd = shared_lightwalletd(state, &url).await?;
    let roots_fresh = state
        .subtree_roots_fetched
        .lock()
        .await
        .get(&roots_key)
        .is_some_and(|at| at.elapsed() < crate::state::SUBTREE_ROOTS_REFRESH);
    let opts = wallet::SyncOptions {
        batch_size: state.load_settings().sync_batch_size,
        skip_subtree_roots: roots_fresh,
    };
    let result = wallet::sync_group(
        &state.data_dir, &wid, network, &lightwalletd, db_key.as_ref(), opts, &cancel,
    )
    .await;
    match &result {
        Ok(_) if !roots_fresh => {
            state
                .subtree_roots_fetched
                .lock()
                .await
                .insert(roots_key, std::time::Instant::now());
        }
        // The connection may be dead (server restart, network change), or the
        // user restarted a stalled sync. Drop it so the next sync starts on a
        // fresh connection. Both paths hold the sync gate, so this can't race
        // the next sync picking it up.
        Err(frost_app_core::CoreError::Connection(_) | frost_app_core::CoreError::Cancelled) => {
            state.lightwalletd.lock().await.take();
        }
        _ => {}
    }

    // Tidy the registry. If our token was cancelled, either a newer sync replaced
    // us (its token is now the registry entry — leave it) or a cancel request did
    // (the next sync's insert will clear it) — either way, don't touch the map. If
    // we finished uncancelled, no newer sync can have started (that would have
    // cancelled us), so the entry is ours to remove.
    if !cancel.is_cancelled() {
        state.sync_cancels.lock().await.remove(&group_id);
    }

    let stats = result?;
    let status = wallet::group_status(&state.data_dir, &wid, network, &ufvk, db_key.as_ref())?;
    Ok((status, stats))
}

/// The status of one of a group's wallets from its local database (no network).
pub(crate) async fn local_status(state: &AppState, group_id: &str, kind: WalletKind) -> AppResult<WalletStatus> {
    let ctx = wallet_ctx(state, group_id, kind).await?;
    let db_key = state.wallet_db_key(group_id).await?;
    Ok(wallet::group_status(&state.data_dir, &ctx.dir, ctx.network, &ctx.ufvk, db_key.as_ref())?)
}

/// Ask the background sync service to sync the active wallet now. With
/// `restart`, an in-flight sync is abandoned first (for one that has stalled).
#[tauri::command]
pub async fn wallet_sync_now(state: State<'_, AppState>, restart: bool) -> AppResult<()> {
    if restart {
        if let Some(group_id) = crate::sync_service::active_wallet(&state).await {
            if let Some(token) = state.sync_cancels.lock().await.get(&group_id) {
                token.cancel();
            }
        }
    }
    state.sync_service.wake(true);
    Ok(())
}

/// The background sync service's latest status for a group, so a screen that
/// opens mid-sync can show it before the next `wallet:sync` event.
#[tauri::command]
pub async fn wallet_sync_status(
    state: State<'_, AppState>,
    group_id: String,
) -> AppResult<Option<crate::sync_service::SyncStatus>> {
    Ok(state.sync_service.status(&group_id))
}

/// The app's lightwalletd connection for `url`: the open one if it points at
/// the same endpoint, otherwise a fresh connection that replaces it.
pub(crate) async fn shared_lightwalletd(state: &AppState, url: &str) -> AppResult<wallet::Lightwalletd> {
    let mut slot = state.lightwalletd.lock().await;
    if let Some(conn) = slot.as_ref().filter(|c| c.url() == url.trim()) {
        return Ok(conn.clone());
    }
    let conn = wallet::Lightwalletd::connect(url).await?;
    *slot = Some(conn.clone());
    Ok(conn)
}


/// On-chain transaction history for a group wallet: received funds and sent
/// transactions from the local wallet db, newest confirmed first. For a group
/// that made its viewing key private, its old wallet's history too.
#[tauri::command]
pub async fn wallet_history(
    state: State<'_, AppState>,
    group_id: String,
) -> AppResult<Vec<wallet::TxRecord>> {
    // Validate that `group_id` is a known RedPallas group before it is used as a
    // filesystem path component (see wallet_paths). This mirrors the guard every
    // other wallet command applies and prevents path traversal via the group id.
    let (network, _url, _ufvk) = group_wallet_ctx(&state, &group_id).await?;
    let db_key = state.wallet_db_key(&group_id).await?;
    let wid = state.wallet_dir_id(&group_id).await?;
    let current = wallet::wallet_history(&state.data_dir, &wid, network, db_key.as_ref())?;
    if !state.has_old_viewing_key(&group_id).await? {
        return Ok(current);
    }
    let old = wallet_ctx(&state, &group_id, WalletKind::Old).await?;
    let old = wallet::wallet_history(&state.data_dir, &old.dir, network, db_key.as_ref()).unwrap_or_default();
    Ok(merge_history(current, old))
}

/// One history from a group's current and old wallets. A move off the old
/// address shows in both, as a send from one and a receipt in the other; it
/// becomes one `"move"` entry.
fn merge_history(current: Vec<wallet::TxRecord>, old: Vec<wallet::TxRecord>) -> Vec<wallet::TxRecord> {
    let received: std::collections::HashSet<String> =
        current.iter().filter(|t| t.direction == "receive").map(|t| t.txid.clone()).collect();
    let moved: std::collections::HashSet<String> =
        old.iter().filter(|t| t.direction == "send" && received.contains(&t.txid)).map(|t| t.txid.clone()).collect();
    let mut all: Vec<wallet::TxRecord> = current
        .into_iter()
        .filter(|t| !moved.contains(&t.txid))
        .chain(old.into_iter().map(|mut t| {
            if moved.contains(&t.txid) {
                t.direction = "move".into();
                t.recipient = None;
            }
            t
        }))
        .collect();
    // Newest first, unconfirmed on top, as each wallet's own history is.
    all.sort_by_key(|t| std::cmp::Reverse(t.block_height.unwrap_or(u64::MAX)));
    all
}

/// The unspent Orchard notes that make up the group's balance (for the "Review
/// Notes" view). Each note is one spend authorization / FROST signing round.
#[tauri::command]
pub async fn wallet_notes(
    state: State<'_, AppState>,
    group_id: String,
) -> AppResult<Vec<wallet::NoteRecord>> {
    let (network, _url, _ufvk) = group_wallet_ctx(&state, &group_id).await?;
    let db_key = state.wallet_db_key(&group_id).await?;
    let wid = state.wallet_dir_id(&group_id).await?;
    Ok(wallet::wallet_notes(&state.data_dir, &wid, network, db_key.as_ref())?)
}

/// A rotating receive address plus the diversifier index it was derived at.
#[derive(Serialize)]
pub struct ReceiveAddress {
    pub address: String,
    pub index: u32,
}

/// Resolve the diversifier index to hand out for a group, rotating past the
/// current one when the wallet has received new notes since it was issued (#3),
/// then derive and return the corresponding Orchard receive address. When
/// `force_new` is set, always advance to a fresh index.
async fn resolve_receive_address(
    state: &AppState,
    group_id: &str,
    force_new: bool,
) -> AppResult<ReceiveAddress> {
    let (network, _url, _ufvk) = group_wallet_ctx(state, group_id).await?;
    let db_key = state.wallet_db_key(group_id).await?;
    let wid = state.wallet_dir_id(group_id).await?;
    let current_notes = wallet::count_received_notes(&state.data_dir, &wid, network, db_key.as_ref())?;
    let seed = state.viewing_seed(group_id).await?;

    let index = state
        .update_private(|p| {
            let entry = p.receive_state.entry(group_id.to_string()).or_default();
            // Rotate when explicitly asked, or when the address may have been
            // used (received-note count grew since it was issued).
            if force_new || current_notes > entry.baseline_notes {
                entry.index = entry.index.saturating_add(1);
                entry.baseline_notes = current_notes;
            }
            entry.index
        })
        .await?;

    let address =
        frost_app_core::zcash::derive_orchard_address_at(group_id, seed.as_deref(), network.network_type(), index)?;
    Ok(ReceiveAddress { address, index })
}

/// The group's current rotating receive address. Advances automatically if the
/// previous address has plausibly been paid since it was issued.
#[tauri::command]
pub async fn wallet_receive_address(
    state: State<'_, AppState>,
    group_id: String,
) -> AppResult<ReceiveAddress> {
    resolve_receive_address(&state, &group_id, false).await
}

/// Force a brand-new receive address for the group (manual rotation).
#[tauri::command]
pub async fn wallet_new_receive_address(
    state: State<'_, AppState>,
    group_id: String,
) -> AppResult<ReceiveAddress> {
    resolve_receive_address(&state, &group_id, true).await
}

/// Build (but do not sign or broadcast) an Orchard transfer, returning the
/// draft PCZT and the sighash the group must FROST-sign. Moves no funds.
#[tauri::command]
pub async fn wallet_prepare_send(
    state: State<'_, AppState>,
    group_id: String,
    recipient: String,
    amount_zatoshis: u64,
    memo: Option<String>,
) -> AppResult<wallet::DraftTransaction> {
    let (network, url, _ufvk) = group_wallet_ctx(&state, &group_id).await?;
    let db_key = state.wallet_db_key(&group_id).await?;
    let wid = state.wallet_dir_id(&group_id).await?;
    Ok(wallet::prepare_send(
        &state.data_dir,
        &wid,
        network,
        &recipient,
        amount_zatoshis,
        memo,
        &url,
        db_key.as_ref(),
    )
    .await?)
}

/// Like [`wallet_prepare_send`], but paying everything spendable, with the
/// fee coming out of it: what "Send all" reviews.
#[tauri::command]
pub async fn wallet_prepare_send_max(
    state: State<'_, AppState>,
    group_id: String,
    recipient: String,
    memo: Option<String>,
) -> AppResult<wallet::DraftTransaction> {
    let (network, url, _ufvk) = group_wallet_ctx(&state, &group_id).await?;
    let db_key = state.wallet_db_key(&group_id).await?;
    let wid = state.wallet_dir_id(&group_id).await?;
    Ok(wallet::prepare_send_max(&state.data_dir, &wid, network, &recipient, memo, &url, db_key.as_ref()).await?)
}

/// Resolve a ZcashNames (ZNS) name (e.g. `alice` or `alice.zcash`) to a unified
/// address via the public ZNS indexer, using the wallet's configured network.
/// Returns the registration (whose `address` is the send recipient) or `None`
/// when the name is unregistered.
///
/// The resolver is external infrastructure, so the UI MUST display the resolved
/// address for the user to confirm before sending — a name is a convenience, not
/// an authorization.
#[tauri::command]
pub async fn resolve_zns_name(
    state: State<'_, AppState>,
    name: String,
) -> AppResult<Option<frost_app_core::zns::ResolveResult>> {
    // Lookups go to the indexer over a plain HTTPS client, not through Tor.
    if wallet::tor::is_on() {
        return Err(AppError::new(
            "config",
            "name lookups don't go through Tor, so they're off while Tor is on. Paste the address instead",
        ));
    }
    let mainnet = match configured_network(&state.load_settings()) {
        WalletNetwork::Main => true,
        WalletNetwork::Test => false,
        // There is no ZNS indexer for a local chain. Say so, rather than
        // returning `None`, which the UI would show as "name not registered".
        WalletNetwork::Regtest(_) => {
            return Err(AppError::new(
                "config",
                "ZcashNames aren't available on the local devnet — paste a uregtest1… address instead",
            ))
        }
    };
    Ok(frost_app_core::zns::resolve_name(&name, mainnet).await?)
}

#[derive(Deserialize)]
pub struct WalletSendArgs {
    pub group_id: String,
    pub recipient: String,
    pub amount_zatoshis: u64,
    /// Hex comm pubkeys of the group members who will sign (>= threshold).
    pub signers: Vec<String>,
    /// Optional memo to attach to the recipient's shielded output.
    pub memo: Option<String>,
    /// Move everything spendable at the group's old address to its private
    /// one instead (see `commands::viewing`); `recipient`, `amount_zatoshis`
    /// and `memo` are then ignored.
    #[serde(default)]
    pub from_old_address: bool,
    /// Send everything spendable to `recipient`, the fee coming out of it
    /// ([`wallet_prepare_send_max`]); `amount_zatoshis` is then ignored.
    #[serde(default)]
    pub send_max: bool,
}

/// What `wallet_send` started: the ceremony, and the transaction as it was
/// built (a "send all" amount and the fee are only final now).
#[derive(Serialize)]
pub struct SendStarted {
    pub ceremony_id: Uuid,
    pub amount_zatoshis: u64,
    pub fee_zatoshis: u64,
    pub sighash_hex: String,
    /// When approvals are needed by (epoch ms, estimated from block spacing).
    pub closes_at_ms: i64,
}

/// The longest a send waits for approvals, whatever its expiry: one that
/// never expires, or a devnet where no one is mining, still ends.
const MAX_SIGNING_WAIT: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);

/// How often a send waiting for approvals checks whether it has expired. The
/// sync service checks for new blocks as often.
const EXPIRY_POLL: std::time::Duration = std::time::Duration::from_secs(15);

/// Resolve once a transaction expiring at `expiry_height` can no longer be
/// sent, as the group's sync server sees the chain. A failed check waits for
/// the next one: an unreachable server can't say the window closed.
async fn until_expired<R: tauri::Runtime>(app: &AppHandle<R>, group_id: &str, expiry_height: u32) {
    loop {
        tokio::time::sleep(EXPIRY_POLL).await;
        let state = app.state::<AppState>();
        let Ok((network, url, _)) = group_wallet_ctx(&state, group_id).await else {
            continue;
        };
        let Ok(conn) = shared_lightwalletd(&state, &url).await else {
            continue;
        };
        if let Ok(tip) = conn.latest_height().await {
            if wallet::ApprovalWindow::at(network, expiry_height, tip, 0).is_some_and(|w| w.is_closed()) {
                return;
            }
        }
    }
}

/// Build, FROST-sign, and broadcast an Orchard transfer. Builds the PCZT, then
/// drives the coordinator ceremony over the transaction sighash using each
/// spend's α as the re-randomizer; on completion, applies the group signature
/// to the PCZT. Emits `send:progress` / `send:complete` / `send:failed`.
///
/// Approvals have until the transaction's standard expiry (see
/// [`wallet::ApprovalWindow`]): once the chain gets too close to it, the send
/// stops and `send:failed` carries `expired: true`.
#[tauri::command]
pub async fn wallet_send<R: tauri::Runtime>(
    app: AppHandle<R>,
    args: WalletSendArgs,
) -> AppResult<SendStarted> {
    let state = app.state::<AppState>();
    let (network, url, _ufvk) = group_wallet_ctx(&state, &args.group_id).await?;
    let db_key = state.wallet_db_key(&args.group_id).await?;

    // 1. Build the unsigned transaction (refreshes the chain tip so the expiry
    //    is anchored to the live tip). A signed move off the old address waits
    //    for broadcast with the group's other sends, in the current wallet.
    let wid = state.wallet_dir_id(&args.group_id).await?;
    let draft = if args.from_old_address {
        crate::commands::viewing::prepare_move(&state, &args.group_id).await?
    } else if args.send_max {
        wallet::prepare_send_max(&state.data_dir, &wid, network, &args.recipient, args.memo, &url, db_key.as_ref())
            .await?
    } else {
        wallet::prepare_send(
            &state.data_dir,
            &wid,
            network,
            &args.recipient,
            args.amount_zatoshis,
            args.memo,
            &url,
            db_key.as_ref(),
        )
        .await?
    };
    if draft.spends.is_empty() {
        return Err(AppError::new(
            "wallet",
            "the transaction has no shielded spends to sign",
        ));
    }
    let message = hex::decode(draft.sighash_hex.trim())
        .map_err(|e| AppError::new("wallet", format!("sighash: {e}")))?;
    // Co-signers verify the transaction itself, not our description of it
    // (wallet::verify). Prepared now so an oversized transaction fails before
    // anyone is asked to sign.
    let signer_pczt = wallet::verify::pczt_for_signers(&draft.pczt_hex)?;
    // Each Orchard spend is re-randomized with its own α, so each needs its own
    // re-randomized FROST signature over the (shared) sighash.
    let spends = draft
        .spends
        .iter()
        .map(|s| {
            let alpha = hex::decode(s.alpha_hex.trim())
                .map_err(|e| AppError::new("wallet", format!("alpha: {e}")))?;
            Ok((s.pool, s.index, alpha))
        })
        .collect::<AppResult<Vec<(wallet::SpendPool, usize, Vec<u8>)>>>()?;

    // 2. Shared coordinator inputs (everything but the per-spend randomizer).
    let (group, server_url) =
        crate::commands::signing::group_context(&state, &args.group_id, None).await?;
    let suite = Suite::from_id(&group.ciphersuite).map_err(AppError::from)?;
    let trust = crate::commands::server::trust_for(&state, &server_url).await;
    let signers = args
        .signers
        .iter()
        .map(|hex_pubkey| {
            let pubkey = PublicKey(
                hex::decode(hex_pubkey)
                    .map_err(|e| AppError::new("config", format!("bad signer pubkey: {e}")))?,
            );
            let participant = group
                .participant
                .values()
                .find(|p| p.pubkey == pubkey)
                .ok_or_else(|| AppError::new("config", "signer is not a group participant"))?;
            Ok((pubkey, participant.identifier.clone()))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let (comm_privkey, comm_pubkey) = state
        .with_config(|config| {
            let comm = config
                .communication_key
                .as_ref()
                .ok_or_else(|| AppError::new("config", "keystore has no communication key"))?;
            Ok((comm.privkey.clone(), comm.pubkey.clone()))
        })
        .await?;
    let public_key_package = group.public_key_package.clone();
    let self_key_package = group.key_package.clone();

    // 3. Spawn: one re-randomized ceremony per spend, then apply every signature
    //    to the PCZT and prove + broadcast.
    let ceremony_id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    state.ceremonies.lock().await.insert(
        ceremony_id,
        CeremonyHandle {
            cancel: cancel.clone(),
            approval: None,
            gate: crate::state::ApprovalGate::Open,
        },
    );

    let (tx, mut rx) = mpsc::channel(64);
    let event_app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = event_app.emit(
                "send:progress",
                serde_json::json!({ "ceremony_id": ceremony_id, "event": event }),
            );
        }
    });

    let started_at = wallet::now_ms();
    let max_wait_ms = MAX_SIGNING_WAIT.as_millis() as i64;
    let closes_at_ms = draft
        .approval_window
        .and_then(|w| w.closes_at_ms)
        .map_or(started_at + max_wait_ms, |at| at.min(started_at + max_wait_ms));
    let expiry_height = draft.approval_window.map(|w| w.expiry_height);
    let started = SendStarted {
        ceremony_id,
        amount_zatoshis: draft.amount_zatoshis,
        fee_zatoshis: draft.fee_zatoshis,
        sighash_hex: draft.sighash_hex.clone(),
        closes_at_ms,
    };

    let task_app = app.clone();
    let pczt_hex = draft.pczt_hex.clone();
    let sighash_hex = draft.sighash_hex.clone();
    let group_id = args.group_id.clone();
    let wallet_dir = wid.clone();
    let pending_key = state.pending_tx_key(&args.group_id).await?;
    // Transaction context fields sent to co-signers (C-1). For a multi-note
    // spend each note is signed in its own FROST session; every one carries the
    // same `plan_id` (the ceremony id) and the shared `tx_sighash` so a
    // participant can confirm the separate requests all belong to this one
    // transaction (#2). Built per-note below so `spend_index` can differ.
    let ctx_recipient = draft.recipient.clone();
    let ctx_amount = draft.amount_zatoshis;
    let ctx_fee = draft.fee_zatoshis;
    let ctx_memo = draft.memo.clone();
    let ctx_is_unshield = draft.is_unshield;
    let ctx_network = network.as_str().to_string();
    let plan_id = ceremony_id.to_string();
    tauri::async_runtime::spawn(async move {
        let fail = |error: String| {
            let _ = task_app.emit(
                "send:failed",
                serde_json::json!({ "ceremony_id": ceremony_id, "error": error }),
            );
        };

        let mut timed_out = false;
        let signing_result: Result<Vec<(wallet::SpendPool, usize, String)>, String> = {
            let total = spends.len();

            // Sign each note in its own re-randomized FROST session, one note at
            // a time. Each note is a distinct frostd session that every signer
            // must approve; running them sequentially means only one session is
            // live at a time, so the coordinator's shown session id always points
            // at the note currently being signed and external (e.g. CLI) signers
            // approve them in turn instead of having to find N simultaneous
            // sessions. Every session carries the same plan_id + tx_sighash so a
            // signer can confirm they all belong to this one transaction (#2).
            // Atomic: the first note to fail/reject returns Err before any
            // broadcast, so a partially-signed transaction is never produced.
            let signing_fut = async {
                let mut signatures: Vec<(wallet::SpendPool, usize, String)> =
                    Vec::with_capacity(total);
                for (i, (pool, index, alpha)) in spends.into_iter().enumerate() {
                    // Tell the UI which note is being signed; each is a separate
                    // approval in signers' inboxes.
                    let _ = task_app.emit(
                        "send:progress",
                        serde_json::json!({
                            "ceremony_id": ceremony_id,
                            "event": { "phase": "signing_spend", "spend": i + 1, "total": total },
                        }),
                    );
                    let send_context = serde_json::to_vec(&frost_app_core::events::SigningContext {
                        recipient: ctx_recipient.clone(),
                        amount_zatoshis: ctx_amount,
                        fee_zatoshis: ctx_fee,
                        memo: ctx_memo.clone(),
                        is_unshield: ctx_is_unshield,
                        network: ctx_network.clone(),
                        plan_id: plan_id.clone(),
                        spend_index: (i as u32) + 1,
                        spend_total: total as u32,
                        tx_sighash: sighash_hex.clone(),
                        pczt: Some(signer_pczt.clone()),
                    })
                    .unwrap_or_default();
                    let params = CoordinatorParams {
                        server_url: server_url.clone(),
                        trust: trust.clone(),
                        comm_privkey: comm_privkey.clone(),
                        comm_pubkey: comm_pubkey.clone(),
                        public_key_package: public_key_package.clone(),
                        message: message.clone(),
                        signers: signers.clone(),
                        self_key_package: self_key_package.clone(),
                        randomizer: Some(alpha), // this note's α
                        send_context,
                    };
                    match run_coordinator(suite, params, tx.clone(), cancel.clone()).await {
                        Ok(output) => {
                            signatures.push((pool, index, hex::encode(&output.signature)))
                        }
                        Err(e) => return Err(e.to_string()), // abort remaining notes
                    }
                }
                Ok(signatures)
            };
            // Multi-spend transactions share the one window across all spends.
            let expired = async {
                match expiry_height {
                    Some(height) => until_expired(&task_app, &group_id, height).await,
                    None => std::future::pending().await,
                }
            };
            tokio::pin!(signing_fut);
            let finished = tokio::select! {
                result = &mut signing_fut => Some(result),
                () = expired => None,
                () = tokio::time::sleep(MAX_SIGNING_WAIT) => None,
            };
            match finished {
                Some(result) => result,
                None => {
                    // Let the ceremony close its session, so the request leaves
                    // members' Approvals.
                    cancel.cancel();
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), &mut signing_fut).await;
                    timed_out = true;
                    Err("Not enough members approved in time, so the payment expired. Nothing was sent.".to_string())
                }
            }
        };

        drop(tx); // close the progress channel so the forwarding task ends
        task_app
            .state::<AppState>()
            .ceremonies
            .lock()
            .await
            .remove(&ceremony_id);

        let signatures = match signing_result {
            Ok(sigs) => sigs,
            Err(error) if timed_out => {
                let _ = task_app.emit(
                    "send:failed",
                    serde_json::json!({ "ceremony_id": ceremony_id, "error": error, "expired": true }),
                );
                return;
            }
            Err(e) => return fail(e),
        };

        let signed_pczt_hex =
            match wallet::apply_signatures(&pczt_hex, &sighash_hex, signatures) {
                Ok(hex) => hex,
                Err(e) => return fail(e.to_string()),
            };
        // Persist the fully-signed transaction before attempting broadcast. The
        // ceremony is the expensive, un-repeatable part; if broadcast fails
        // (endpoint down, transient network) the signed tx can be re-broadcast
        // via wallet_rebroadcast without gathering signatures again.
        let data_dir = task_app.state::<AppState>().data_dir.clone();
        let ceremony_str = ceremony_id.to_string();
        let _ = wallet::save_pending_tx(&data_dir, &wallet_dir, network, &ceremony_str, &signed_pczt_hex, &pending_key);

        // Prove + broadcast. Surfaced as its own phase since the proof build is
        // the slow part (several seconds).
        let _ = task_app.emit(
            "send:progress",
            serde_json::json!({
                "ceremony_id": ceremony_id,
                "event": { "phase": "proving" },
            }),
        );
        // Re-read the endpoint at broadcast time so a mid-ceremony settings
        // change (or the earlier captured URL going stale) does not send to the
        // wrong node.
        let broadcast_url = group_wallet_ctx(&task_app.state::<AppState>(), &group_id)
            .await
            .map(|(_, u, _)| u)
            .unwrap_or(url);
        match wallet::broadcast_signed(&signed_pczt_hex, network, &broadcast_url).await {
            Ok(txid) => {
                wallet::clear_pending_tx(&data_dir, &wallet_dir, network, &ceremony_str);
                let _ = task_app.emit(
                    "send:complete",
                    serde_json::json!({
                        "ceremony_id": ceremony_id,
                        "txid": txid,
                        "signed_pczt_hex": signed_pczt_hex,
                    }),
                );
            }
            Err(e) => {
                // Leave the pending record in place and tell the UI it can retry.
                let _ = task_app.emit(
                    "send:failed",
                    serde_json::json!({
                        "ceremony_id": ceremony_id,
                        "error": e.to_string(),
                        "retryable": true,
                    }),
                );
            }
        }
    });

    Ok(started)
}

/// Re-broadcast a previously-signed transaction whose first broadcast failed,
/// without re-running the FROST ceremony. Reads the signed PCZT persisted by
/// `wallet_send`, proves + broadcasts it against the current endpoint, and
/// clears the pending record on success. Returns the broadcast txid.
#[tauri::command]
pub async fn wallet_rebroadcast(
    state: State<'_, AppState>,
    group_id: String,
    ceremony_id: String,
) -> AppResult<String> {
    let (network, url, _ufvk) = group_wallet_ctx(&state, &group_id).await?;
    let wid = state.wallet_dir_id(&group_id).await?;
    let key = state.pending_tx_key(&group_id).await?;
    let signed_pczt_hex = wallet::load_pending_tx(&state.data_dir, &wid, network, &ceremony_id, &key)?;
    let txid = wallet::broadcast_signed(&signed_pczt_hex, network, &url).await?;
    wallet::clear_pending_tx(&state.data_dir, &wid, network, &ceremony_id);
    Ok(txid)
}
