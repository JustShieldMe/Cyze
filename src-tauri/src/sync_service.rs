//! Background wallet sync.
//!
//! The wallet used to sync only while its page was open: the page started a
//! sync every 10 s and polled the database for progress, so leaving the page
//! stopped syncing, and coming back meant waiting out a catch-up before sending.
//! This service runs in the backend from unlock to lock and keeps the active
//! wallet current wherever the user is in the app.
//!
//! Each pass: pick the active RedPallas group, set up its view-only account if
//! needed, then check the chain tip (one small RPC on the shared connection)
//! and sync only when there is something new. Failures back off exponentially.
//! "Sync Now", a wallet switch, or a network change wakes it immediately.
//! Progress reaches the UI as `wallet:sync` events (a [`SyncStatus`]);
//! `wallet:updated` tells it to re-read balances, notes and history.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::Notify;

use crate::commands::wallet::{
    group_wallet_ctx, init_account, local_status, rebuild_after_reorg, shared_lightwalletd, sync_once,
};
use crate::error::AppError;
use crate::state::{wallet_session_key, AppState};

/// How often a caught-up wallet checks for a new block. Blocks arrive about
/// every 75 s, so a new one shows up within ~15 s.
const TIP_POLL: Duration = Duration::from_secs(15);
/// How often progress is re-read and reported while a sync runs.
const PROGRESS_TICK: Duration = Duration::from_secs(1);
/// First retry delay after a failure; doubles up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(10);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// With no wallet to sync, re-check this often (a group may have been created).
const IDLE_RECHECK: Duration = Duration::from_secs(30);

/// What the UI shows about a wallet's sync. Sent as the `wallet:sync` event.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct SyncStatus {
    pub group_id: String,
    /// `"setting_up"`, `"syncing"`, `"idle"` (up to date) or `"error"`.
    pub state: &'static str,
    /// Highest fully scanned block and the chain tip, as last known.
    pub scanned_height: u64,
    pub tip_height: u64,
    /// When a sync last completed (epoch milliseconds).
    pub last_synced_ms: Option<i64>,
    /// The last failure, while `state` is `"error"`.
    pub error: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Default)]
pub struct SyncService {
    wake: Notify,
    /// The next pass must sync even if the tip hasn't moved.
    force: AtomicBool,
    statuses: std::sync::Mutex<HashMap<String, SyncStatus>>,
}

impl SyncService {
    /// Run a pass now instead of waiting for the next poll. `force` makes it
    /// sync even if no new block has arrived.
    pub fn wake(&self, force: bool) {
        if force {
            self.force.store(true, Ordering::SeqCst);
        }
        self.wake.notify_one();
    }

    /// The latest status for a group, if the service has reported one.
    pub fn status(&self, group_id: &str) -> Option<SyncStatus> {
        self.statuses.lock().ok()?.get(group_id).cloned()
    }

    fn last_synced(&self, group_id: &str) -> Option<i64> {
        self.status(group_id).and_then(|s| s.last_synced_ms)
    }

    /// Record a status and send it to the UI, unless nothing changed.
    fn report<R: Runtime>(&self, app: &AppHandle<R>, status: SyncStatus) {
        let changed = match self.statuses.lock() {
            Ok(mut map) => map.insert(status.group_id.clone(), status.clone()).as_ref() != Some(&status),
            Err(_) => true,
        };
        if changed {
            let _ = app.emit("wallet:sync", &status);
        }
    }
}

/// The wallet the service keeps in sync: the active wallet if it is a RedPallas
/// group, or the only RedPallas group when none has been chosen yet.
pub async fn active_wallet(state: &AppState) -> Option<String> {
    let pallas: Vec<String> = state
        .with_config(|config| {
            Ok(config
                .group
                .iter()
                .filter(|(_, g)| g.ciphersuite.contains("Pallas"))
                .map(|(id, _)| id.clone())
                .collect())
        })
        .await
        .ok()?;
    match state.load_settings().active_group_id {
        Some(id) if pallas.contains(&id) => Some(id),
        _ if pallas.len() == 1 => pallas.into_iter().next(),
        _ => None,
    }
}

/// Start the service. It lives for the whole process and idles while locked.
pub fn spawn<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(run(app));
}

async fn run<R: Runtime>(app: AppHandle<R>) {
    let state = app.state::<AppState>();
    let mut unlocked = state.watch_unlocked();
    // Wallets synced at least once since unlock. The first pass for each always
    // syncs, even at the tip, so a reorg or pending verify range is picked up.
    let mut synced_since_unlock: HashSet<String> = HashSet::new();
    let mut backoff: Option<Duration> = None;

    loop {
        if !*unlocked.borrow_and_update() {
            synced_since_unlock.clear();
            backoff = None;
            if unlocked.changed().await.is_err() {
                return;
            }
            continue;
        }

        let Some(group_id) = active_wallet(&state).await else {
            wait(&state, &mut unlocked, IDLE_RECHECK).await;
            continue;
        };
        let force = state.sync_service.force.swap(false, Ordering::SeqCst);

        // A lock mid-pass cancels the sync: its key material is going away.
        let outcome = tokio::select! {
            outcome = pass(&app, &state, &group_id, force, &mut synced_since_unlock) => Some(outcome),
            _ = until_locked(&mut unlocked) => None,
        };
        let Some(outcome) = outcome else {
            if let Some(token) = state.sync_cancels.lock().await.get(&group_id) {
                token.cancel();
            }
            continue;
        };

        let delay = match outcome {
            Ok(()) => {
                backoff = None;
                TIP_POLL
            }
            // Cancelled by a restart, a wallet switch or a network change; the
            // wake that caused it is already pending, so go straight round.
            Err(e) if e.code == "cancelled" => Duration::ZERO,
            // A reorg below the wallet's start: rebuild it from an earlier
            // height and sync again straight away.
            Err(e) if e.code == "reorg" => match rebuild_after_reorg(&state, &group_id).await {
                Ok(()) => {
                    tracing::warn!("rebuilt the wallet after a chain reorganization: {}", e.message);
                    synced_since_unlock.clear();
                    Duration::ZERO
                }
                Err(rebuild) => {
                    tracing::warn!("couldn't rebuild the wallet after a reorg: {}", rebuild.message);
                    backoff = Some(backoff.map_or(FIRST_BACKOFF, |b| (b * 2).min(MAX_BACKOFF)));
                    backoff.unwrap_or(FIRST_BACKOFF)
                }
            },
            Err(e) => {
                tracing::warn!("wallet sync failed: {}", e.message);
                let mut status = state.sync_service.status(&group_id).unwrap_or_default();
                status.group_id = group_id.clone();
                status.state = "error";
                status.error = Some(e.message);
                status.error_code = Some(e.code.to_string());
                state.sync_service.report(&app, status);
                let next = backoff.map_or(FIRST_BACKOFF, |b| (b * 2).min(MAX_BACKOFF));
                backoff = Some(next);
                next
            }
        };
        wait(&state, &mut unlocked, delay).await;
    }
}

/// One pass for `group_id`: set up, check the tip, sync if needed.
async fn pass<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    group_id: &str,
    force: bool,
    synced_since_unlock: &mut HashSet<String>,
) -> Result<(), AppError> {
    let service = &state.sync_service;
    let (network, url, _ufvk) = group_wallet_ctx(state, group_id).await?;
    let session_key = wallet_session_key(group_id, network.dir_name());
    let base = |state: &'static str, scanned: u64, tip: u64| SyncStatus {
        group_id: group_id.to_string(),
        state,
        scanned_height: scanned,
        tip_height: tip,
        last_synced_ms: service.last_synced(group_id),
        error: None,
        error_code: None,
    };

    let mut local = local_status(state, group_id).await?;
    if !local.initialized {
        service.report(app, base("setting_up", 0, 0));
        init_account(state, group_id, None).await?;
        local = local_status(state, group_id).await?;
    }

    let tip = shared_lightwalletd(state, &url).await?.latest_height().await?;
    let needed = force || !synced_since_unlock.contains(&session_key) || tip > local.synced_height;
    if !needed {
        service.report(app, base("idle", local.synced_height, tip));
        return Ok(());
    }

    service.report(app, base("syncing", local.synced_height, tip.max(local.chain_tip_height)));
    let sync = sync_once(state, group_id);
    tokio::pin!(sync);
    let mut ticker = tokio::time::interval(PROGRESS_TICK);
    ticker.tick().await; // the first tick is immediate
    let (status, stats) = loop {
        tokio::select! {
            result = &mut sync => break result?,
            _ = ticker.tick() => {
                // A just-imported wallet reads 0/0 until its first batch lands;
                // keep showing the last known heights rather than zeros.
                if let Ok((scanned, tip)) = progress(state, group_id).await {
                    if tip > 0 {
                        service.report(app, base("syncing", scanned, tip));
                    }
                }
            }
        }
    };

    let first = synced_since_unlock.insert(session_key);
    service.report(
        app,
        SyncStatus {
            last_synced_ms: Some(now_ms()),
            ..base("idle", status.synced_height, status.chain_tip_height)
        },
    );
    if stats.blocks > 0 || first {
        let _ = app.emit("wallet:updated", serde_json::json!({ "group_id": group_id }));
    }
    Ok(())
}

async fn progress(state: &AppState, group_id: &str) -> Result<(u64, u64), AppError> {
    let (network, _url, _ufvk) = group_wallet_ctx(state, group_id).await?;
    let db_key = state.wallet_db_key(group_id).await?;
    Ok(frost_app_core::wallet::sync_progress(
        &state.data_dir,
        group_id,
        network,
        db_key.as_ref(),
    )?)
}

/// Sleep for `delay`, returning early on a wake-up or a lock change.
async fn wait(state: &AppState, unlocked: &mut tokio::sync::watch::Receiver<bool>, delay: Duration) {
    tokio::select! {
        _ = state.sync_service.wake.notified() => {}
        _ = tokio::time::sleep(delay) => {}
        _ = unlocked.changed() => {}
    }
}

/// Resolve once the keystore is locked.
async fn until_locked(unlocked: &mut tokio::sync::watch::Receiver<bool>) {
    while *unlocked.borrow_and_update() {
        if unlocked.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
