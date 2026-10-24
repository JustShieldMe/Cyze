//! Making an existing group's viewing key private, and moving its funds off
//! the old address.
//!
//! A group created before Cyze 0.3 derives its viewing key from its id, so
//! anyone who learns the id can see its balance and payments. Its members agree
//! on a new viewing seed (`frost_app_core::viewing::agree_new_viewing_key`) over
//! a session one of them starts and the others' apps join by themselves. Each
//! app then records the seed, keeps the old wallet (`WalletKind::Old`) and
//! starts a private one. Moving the funds is an ordinary payment from the old
//! wallet to the new address, approved like any other (`wallet_send` with
//! `from_old_address`); co-signers' apps check it like any other too.

use frost_app_core::viewing::{agree_new_viewing_key, NewViewingKeyParams, ViewingKeyEvent, ViewingSeed};
use frost_app_core::wallet::{self, DraftTransaction};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::commands::wallet::{configured_network, local_status, resolve_config, wallet_ctx};
use crate::error::{AppError, AppResult};
use crate::state::{AppState, WalletKind};

/// A run of the agreement, as the UI shows it.
#[derive(Clone, Debug, Serialize)]
pub struct ViewingKeyProgress {
    pub session_id: Option<Uuid>,
    /// Hex communication key of the member who started it; `None` when this
    /// device did.
    pub started_by: Option<String>,
    /// `"connecting"`, `"waiting"` or `"failed"`.
    pub state: &'static str,
    /// Members (hex communication keys) whose apps haven't joined yet.
    pub waiting_for: Vec<String>,
    pub error: Option<String>,
}

/// A run in progress (or just failed), with what stops it.
pub struct ViewingKeyRun {
    pub cancel: CancellationToken,
    pub progress: ViewingKeyProgress,
}

/// Funds still at a group's old address.
#[derive(Serialize)]
pub struct OldAddress {
    pub total_zatoshis: u64,
    pub spendable_zatoshis: u64,
    /// Its wallet has scanned close enough to the chain tip to move funds.
    pub synced: bool,
}

#[derive(Serialize)]
pub struct ViewingKeyStatus {
    /// The group's viewing key is private: its members agreed on it, rather
    /// than it being derived from the group's id.
    pub private: bool,
    /// For a group that made its viewing key private later: what's still at
    /// the old address.
    pub old_address: Option<OldAddress>,
    /// A run of the agreement in progress, or one that just failed.
    pub progress: Option<ViewingKeyProgress>,
}

/// Tell the UI to re-read a group's viewing-key status.
fn updated<R: Runtime>(app: &AppHandle<R>, group_id: &str) {
    let _ = app.emit("viewing_key:updated", serde_json::json!({ "group_id": group_id }));
}

#[tauri::command]
pub async fn viewing_key_status(state: State<'_, AppState>, group_id: String) -> AppResult<ViewingKeyStatus> {
    let private = state.viewing_seed(&group_id).await?.is_some();
    let old_address = if state.has_old_viewing_key(&group_id).await? {
        let s = local_status(&state, &group_id, WalletKind::Old).await?;
        Some(OldAddress {
            total_zatoshis: s.total_zatoshis,
            spendable_zatoshis: s.spendable_zatoshis,
            synced: s.initialized && s.chain_tip_height > 0 && wallet::is_caught_up(s.synced_height, s.chain_tip_height),
        })
    } else {
        None
    };
    let progress = state
        .viewing_key_runs
        .lock()
        .ok()
        .and_then(|runs| runs.get(&group_id).map(|r| r.progress.clone()));
    Ok(ViewingKeyStatus { private, old_address, progress })
}

/// Start agreeing on a private viewing key with the rest of the group. Every
/// member's app has to be open: theirs join by themselves.
#[tauri::command]
pub async fn make_viewing_key_private<R: Runtime>(app: AppHandle<R>, group_id: String) -> AppResult<()> {
    begin(&app, &group_id, None, None).await
}

/// Join a session another member started (the app does this by itself when
/// it sees one; see `list_pending_sessions`).
#[tauri::command]
pub async fn join_viewing_key_session<R: Runtime>(
    app: AppHandle<R>,
    group_id: String,
    session_id: Uuid,
    started_by: Option<String>,
) -> AppResult<()> {
    begin(&app, &group_id, Some(session_id), started_by).await
}

/// Stop this device's part in a run, or clear a failed one from view.
#[tauri::command]
pub async fn cancel_viewing_key_session<R: Runtime>(app: AppHandle<R>, group_id: String) -> AppResult<()> {
    let state = app.state::<AppState>();
    if let Some(run) = state.viewing_key_runs.lock().map_err(poisoned)?.remove(&group_id) {
        run.cancel.cancel();
    }
    updated(&app, &group_id);
    Ok(())
}

/// Build the payment that moves everything spendable at a group's old address
/// to its private one, for review. Nothing is signed or sent.
#[tauri::command]
pub async fn wallet_prepare_move(state: State<'_, AppState>, group_id: String) -> AppResult<DraftTransaction> {
    prepare_move(&state, &group_id).await
}

pub(crate) async fn prepare_move(state: &AppState, group_id: &str) -> AppResult<DraftTransaction> {
    let old = wallet_ctx(state, group_id, WalletKind::Old).await?;
    let current = wallet_ctx(state, group_id, WalletKind::Current).await?;
    let to = wallet::ufvk_default_address(current.network, &current.ufvk)?;
    let db_key = state.wallet_db_key(group_id).await?;
    Ok(wallet::prepare_sweep(&state.data_dir, &old.dir, old.network, &to, &old.url, db_key.as_ref()).await?)
}

fn poisoned<T>(_: T) -> AppError {
    AppError::new("internal", "state lock poisoned")
}

/// Whether this device can take part in agreeing on a new viewing key for the
/// group now.
async fn check_can_agree(state: &AppState, group_id: &str) -> AppResult<()> {
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
        return Err(AppError::new("config", "only Zcash groups have a viewing key"));
    }
    if state.viewing_seed(group_id).await?.is_none() {
        return Ok(());
    }
    if !state.has_old_viewing_key(group_id).await? {
        return Err(AppError::new("config", "this group's viewing key is already private"));
    }
    // Agreeing again replaces the private key. That's how a member whose app
    // missed the end of the last run catches up with the others, and it's only
    // safe while nothing has arrived under the key it replaces.
    let ctx = wallet_ctx(state, group_id, WalletKind::Current).await?;
    let db_key = state.wallet_db_key(group_id).await?;
    let received = match wallet::count_received_notes(&state.data_dir, &ctx.dir, ctx.network, db_key.as_ref()) {
        Ok(n) => n,
        Err(_) => local_status(state, group_id, WalletKind::Current).await?.total_zatoshis,
    };
    if received > 0 {
        return Err(AppError::new(
            "config",
            "this group's viewing key is already private, and funds have arrived under it",
        ));
    }
    Ok(())
}

async fn begin<R: Runtime>(
    app: &AppHandle<R>,
    group_id: &str,
    session_id: Option<Uuid>,
    started_by: Option<String>,
) -> AppResult<()> {
    let state = app.state::<AppState>();
    check_can_agree(&state, group_id).await?;
    let (group, server_url) = crate::commands::signing::group_context(&state, group_id, None).await?;
    let trust = crate::commands::server::trust_for(&state, &server_url).await;
    let (comm_privkey, comm_pubkey) = state
        .with_config(|config| {
            let comm = config
                .communication_key
                .as_ref()
                .ok_or_else(|| AppError::new("config", "keystore has no communication key"))?;
            Ok((comm.privkey.clone(), comm.pubkey.clone()))
        })
        .await?;

    let cancel = CancellationToken::new();
    {
        let mut runs = state.viewing_key_runs.lock().map_err(poisoned)?;
        if runs.get(group_id).is_some_and(|r| r.progress.state != "failed") {
            return Err(AppError::new("ceremony", "this group is already agreeing on a viewing key"));
        }
        runs.insert(
            group_id.to_string(),
            ViewingKeyRun {
                cancel: cancel.clone(),
                progress: ViewingKeyProgress {
                    session_id,
                    started_by,
                    state: "connecting",
                    waiting_for: Vec::new(),
                    error: None,
                },
            },
        );
    }
    if let Some(id) = session_id {
        state.viewing_key_sessions.lock().map_err(poisoned)?.insert(id);
    }
    updated(app, group_id);

    let params = NewViewingKeyParams {
        server_url,
        trust,
        comm_privkey,
        comm_pubkey,
        group_id: group_id.to_string(),
        members: group.participant.values().map(|p| p.pubkey.clone()).collect(),
        session_id,
    };
    let (tx, mut rx) = mpsc::channel(32);
    let event_app = app.clone();
    let event_group = group_id.to_string();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            let state = event_app.state::<AppState>();
            if let ViewingKeyEvent::SessionReady { session_id } = &event {
                if let Ok(mut sessions) = state.viewing_key_sessions.lock() {
                    sessions.insert(*session_id);
                }
            }
            if let Ok(mut runs) = state.viewing_key_runs.lock() {
                if let Some(run) = runs.get_mut(&event_group) {
                    match event {
                        ViewingKeyEvent::Connecting => {}
                        ViewingKeyEvent::SessionReady { session_id } => {
                            run.progress.session_id = Some(session_id);
                            run.progress.state = "waiting";
                        }
                        ViewingKeyEvent::Waiting { members } => {
                            run.progress.state = "waiting";
                            run.progress.waiting_for = members;
                        }
                    }
                }
            }
            updated(&event_app, &event_group);
        }
    });

    let task_app = app.clone();
    let task_group = group_id.to_string();
    tauri::async_runtime::spawn(async move {
        let result = agree_new_viewing_key(params, tx, cancel.clone()).await;
        let state = task_app.state::<AppState>();
        let result = match result {
            Ok(seed) => save_seed(&state, &task_group, &seed).await,
            Err(e) => Err(AppError::from(e)),
        };
        if let Ok(mut runs) = state.viewing_key_runs.lock() {
            match &result {
                // A cancelled run was already removed; a newer one may have replaced it.
                Err(e) if e.code == "cancelled" => {}
                Err(e) => {
                    if let Some(run) = runs.get_mut(&task_group).filter(|_| !cancel.is_cancelled()) {
                        run.progress.state = "failed";
                        run.progress.error = Some(e.message.clone());
                    }
                }
                Ok(()) => {
                    runs.remove(&task_group);
                }
            }
        }
        match &result {
            Ok(()) => tracing::info!("the group's members agreed on a private viewing key"),
            Err(e) => tracing::warn!("agreeing on a private viewing key failed: {}", e.message),
        }
        updated(&task_app, &task_group);
        let _ = task_app.emit("wallet:updated", serde_json::json!({ "group_id": task_group }));
    });
    Ok(())
}

/// Record a group's new viewing seed. Its current wallet becomes a new,
/// private one, which starts at the chain tip (nothing can be under a key that
/// didn't exist before); the wallet it had so far is kept as its old wallet.
async fn save_seed(state: &AppState, group_id: &str, seed: &ViewingSeed) -> AppResult<()> {
    let network = configured_network(&state.load_settings());
    let mut tip = None;
    if !matches!(network, wallet::WalletNetwork::Regtest(_)) {
        let url = resolve_config(state).lightwalletd_url;
        let read = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            wallet::Lightwalletd::connect(&url).await?.latest_height().await
        });
        match read.await {
            Ok(Ok(height)) => tip = Some(height),
            // Without it the private wallet scans from the group's own start:
            // slower, never wrong.
            _ => tracing::warn!("couldn't read the chain tip for the private wallet's start"),
        }
    }
    // A wallet under a key being replaced held nothing (see check_can_agree).
    let replaced = match state.has_old_viewing_key(group_id).await? {
        true => Some(wallet_ctx(state, group_id, WalletKind::Current).await?),
        false => None,
    };
    state
        .mutate_keystore(|_, extras| {
            extras.viewing_seeds.insert(group_id.to_string(), hex::encode(seed));
            extras.old_viewing_keys.insert(group_id.to_string());
            Ok(())
        })
        .await?;
    if let Some(replaced) = replaced {
        let _ = wallet::delete_wallet_db(&state.data_dir, &replaced.dir, replaced.network);
    }
    let key = wallet_ctx(state, group_id, WalletKind::Current).await?.key;
    state
        .update_private(|p| {
            // Addresses now come from the new key; start its rotation afresh.
            p.receive_state.remove(group_id);
            match tip {
                Some(tip) => p.network_birthdays.insert(key.clone(), tip),
                None => p.network_birthdays.remove(&key),
            };
        })
        .await?;
    state.subtree_roots_fetched.lock().await.remove(&key);
    state.sync_service.wake(true);
    Ok(())
}
