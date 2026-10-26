use frost_app_core::ciphersuite::Suite;
use frost_app_core::events::{ParticipantEvent, SigningContext};
use frost_app_core::signing::{
    run_coordinator, run_participant, CoordinatorParams, ParticipantParams,
};
use frost_app_core::wallet::verify::{self, VerifiedTransaction};
use frost_app_core::wallet::ApprovalWindow;
use frost_client::api::{self, PublicKey};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::{AppState, ApprovalGate, CeremonyHandle};

/// How long an approved transaction's other per-note rounds are approved
/// automatically: longer than a send's 35-minute signing budget.
const APPROVAL_MEMORY: std::time::Duration = std::time::Duration::from_secs(40 * 60);

/// Resolve a group from the keystore plus the server/trust to use for it.
pub(crate) async fn group_context(
    state: &AppState,
    group_id: &str,
    server_override: Option<String>,
) -> AppResult<(frost_client::cli::config::Group, String)> {
    let group = state
        .with_config(|config| {
            config
                .group
                .get(group_id)
                .cloned()
                .ok_or_else(|| AppError::new("config", "group not found"))
        })
        .await?;
    // If the local sidecar is running and no explicit per-group URL was set,
    // connect to it directly instead of routing through the Cloudflare tunnel
    // URL that may be saved in settings for external participants to use.
    let local_sidecar_url = {
        let guard = state.sidecar.lock().await;
        guard.as_ref().map(|h| format!("127.0.0.1:{}", h.port))
    };
    // The group records the server it was created on. That is the right default
    // for a *stable* address, but a Cloudflare quick tunnel is ephemeral: its
    // hostname is regenerated whenever the coordinator restarts the tunnel, so a
    // stored one is dead the moment that happens. Left ahead of the current
    // setting it would silently pin the group to a permanently-unresolvable host
    // — you could still *see* a session (the inbox polls the configured server)
    // but never join it. So a stored ephemeral URL yields to the live setting,
    // and is only used as a last resort when nothing else is configured.
    let (stable_group_url, ephemeral_group_url) = match group.server_url.clone() {
        Some(u) if frost_app_core::neterr::is_ephemeral_server(&u) => (None, Some(u)),
        other => (other, None),
    };
    let server_url = server_override
        .or(stable_group_url)
        .or(local_sidecar_url)
        .or_else(|| state.load_settings().server_url)
        .or(ephemeral_group_url)
        .ok_or_else(|| {
            AppError::new(
                "config",
                "no server is set up for this group to meet on. Choose one under Settings → Connection",
            )
        })?;
    let server_url = server_url
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string();
    Ok((group, server_url))
}

#[derive(Deserialize)]
pub struct CreateSigningSessionArgs {
    pub group_id: String,
    /// Hex-encoded message to sign.
    pub message_hex: String,
    /// Hex comm pubkeys of the signers (must be group participants).
    pub signers: Vec<String>,
    pub server_url: Option<String>,
}

#[tauri::command]
pub async fn create_signing_session<R: tauri::Runtime>(
    app: AppHandle<R>,
    args: CreateSigningSessionArgs,
) -> AppResult<Uuid> {
    let state = app.state::<AppState>();
    let (group, server_url) = group_context(&state, &args.group_id, args.server_url).await?;
    let suite = Suite::from_id(&group.ciphersuite).map_err(AppError::from)?;
    let trust = crate::commands::server::trust_for(&state, &server_url).await;

    let message = hex::decode(args.message_hex.trim())
        .map_err(|e| AppError::new("config", format!("message must be hex: {e}")))?;
    if message.is_empty() {
        return Err(AppError::new("config", "message is empty"));
    }

    // Map selected signer pubkeys to their group identifiers.
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

    let params = CoordinatorParams {
        server_url,
        trust,
        comm_privkey,
        comm_pubkey,
        public_key_package: group.public_key_package.clone(),
        message,
        signers,
        // The local user's own share, used only if they are among the signers
        // so the coordinator can contribute its commitment/share locally.
        self_key_package: group.key_package.clone(),
        // Standalone signing generates a fresh randomizer; the Orchard
        // transaction flow (phase 5) supplies the spend's α here instead.
        randomizer: None,
        // Standalone signing has no wallet transaction context to show.
        send_context: Vec::new(),
    };

    let ceremony_id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    state.ceremonies.lock().await.insert(
        ceremony_id,
        CeremonyHandle {
            cancel: cancel.clone(),
            approval: None,
            gate: ApprovalGate::Open,
        },
    );

    let (tx, mut rx) = mpsc::channel(64);
    let event_app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = event_app.emit(
                "signing:progress",
                serde_json::json!({ "ceremony_id": ceremony_id, "event": event }),
            );
        }
    });

    let task_app = app.clone();
    tauri::async_runtime::spawn(async move {
        // 35-minute timeout: gives signers ample time to approve while keeping
        // the session from hanging indefinitely when someone goes offline.
        const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(35 * 60);
        let result = match tokio::time::timeout(TIMEOUT, run_coordinator(suite, params, tx, cancel)).await {
            Ok(r) => r,
            Err(_) => Err(frost_app_core::CoreError::Ceremony(
                "Signing session timed out after 35 minutes. \
                 Start a new session when all participants are available."
                .to_string(),
            )),
        };
        let state = task_app.state::<AppState>();
        state.ceremonies.lock().await.remove(&ceremony_id);
        match result {
            Ok(output) => {
                let _ = task_app.emit(
                    "signing:complete",
                    serde_json::json!({
                        "ceremony_id": ceremony_id,
                        "signature_hex": hex::encode(&output.signature),
                    }),
                );
            }
            Err(e) => {
                let _ = task_app.emit(
                    "signing:failed",
                    serde_json::json!({ "ceremony_id": ceremony_id, "error": e.to_string() }),
                );
            }
        }
    });

    Ok(ceremony_id)
}

#[derive(Deserialize)]
pub struct JoinSigningSessionArgs {
    pub group_id: String,
    pub session_id: Uuid,
    pub server_url: Option<String>,
}

#[tauri::command]
pub async fn join_signing_session<R: tauri::Runtime>(
    app: AppHandle<R>,
    args: JoinSigningSessionArgs,
) -> AppResult<Uuid> {
    let state = app.state::<AppState>();
    let (group, server_url) = group_context(&state, &args.group_id, args.server_url).await?;
    let suite = Suite::from_id(&group.ciphersuite).map_err(AppError::from)?;
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

    let params = ParticipantParams {
        server_url,
        trust,
        comm_privkey,
        comm_pubkey,
        key_package: group.key_package.clone(),
        session_id: args.session_id,
        group_pubkeys: group.participant.values().map(|p| p.pubkey.clone()).collect(),
    };

    let ceremony_id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    let (approve_tx, approve_rx) = oneshot::channel();
    // A RedPallas key spends the group's funds: nothing is signed with it until
    // the request's transaction has been verified (see `gate_wallet_request`).
    let wallet_group = suite == Suite::RedPallas;
    state.ceremonies.lock().await.insert(
        ceremony_id,
        CeremonyHandle {
            cancel: cancel.clone(),
            approval: Some(approve_tx),
            gate: if wallet_group { ApprovalGate::Pending } else { ApprovalGate::Open },
        },
    );

    let (tx, mut rx) = mpsc::channel(64);
    let event_app = app.clone();
    let gate_group = args.group_id.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            let mut payload = serde_json::json!({ "ceremony_id": ceremony_id, "event": event });
            if let (true, ParticipantEvent::AwaitingApproval { message_hex, context, randomizer_hex }) =
                (wallet_group, &event)
            {
                gate_wallet_request(
                    &event_app,
                    ceremony_id,
                    &gate_group,
                    message_hex,
                    context.as_deref(),
                    randomizer_hex.as_deref(),
                )
                .await
                .annotate(&mut payload["event"]);
            }
            let _ = event_app.emit("signing:progress", payload);
        }
    });

    let task_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = run_participant(suite, params, approve_rx, tx, cancel).await;
        let state = task_app.state::<AppState>();
        let handle = state.ceremonies.lock().await.remove(&ceremony_id);
        let result = match (result, handle.map(|h| h.gate)) {
            (Err(_), Some(ApprovalGate::Refused(reason))) => Err(reason),
            (result, _) => result.map_err(|e| e.to_string()),
        };
        match result {
            Ok(()) => {
                let _ = task_app.emit(
                    "signing:complete",
                    serde_json::json!({ "ceremony_id": ceremony_id, "signature_hex": null }),
                );
            }
            Err(e) => {
                let _ = task_app.emit(
                    "signing:failed",
                    serde_json::json!({ "ceremony_id": ceremony_id, "error": e }),
                );
            }
        }
    });

    Ok(ceremony_id)
}

/// What the approval gate decided about a wallet signing request.
enum Verdict {
    /// The transaction verified; the user decides, unless it is another round
    /// of a transaction they already approved.
    Verified { tx: VerifiedTransaction, auto_approved: bool, window: Option<ApprovalWindow> },
    /// No transaction attached, and raw signing is allowed (advanced options).
    Raw,
    /// Refused automatically, for this reason.
    Refused(String),
}

impl Verdict {
    /// Add the decision to the `awaiting_approval` event the UI receives.
    fn annotate(&self, event: &mut serde_json::Value) {
        match self {
            Verdict::Verified { tx, auto_approved, window } => {
                event["verified"] = serde_json::json!(tx);
                event["auto_approved"] = serde_json::json!(auto_approved);
                event["window"] = serde_json::json!(window);
            }
            Verdict::Raw => event["unverified"] = serde_json::json!(true),
            Verdict::Refused(reason) => event["refused"] = serde_json::json!(reason),
        }
    }
}

/// Verify a wallet signing request before its approval can be given (see
/// `frost_app_core::wallet::verify`), and settle or refuse the ceremony's gate
/// accordingly.
async fn gate_wallet_request<R: tauri::Runtime>(
    app: &AppHandle<R>,
    ceremony_id: Uuid,
    group_id: &str,
    message_hex: &str,
    context: Option<&SigningContext>,
    randomizer_hex: Option<&str>,
) -> Verdict {
    let state = app.state::<AppState>();
    let settings = state.load_settings();
    let network = crate::commands::wallet::configured_network(&settings);
    let seed = match state.viewing_seed(group_id).await {
        Ok(seed) => seed,
        Err(e) => return Verdict::Refused(format!("refused to sign: can't read the group's viewing key ({})", e.message)),
    };
    let verdict = match context.and_then(|c| c.pczt.as_deref().map(|pczt| (c, pczt))) {
        Some((claimed, pczt)) => verify::verify_signing_request(network, group_id, seed.as_deref(), pczt, message_hex, randomizer_hex)
            .and_then(|tx| verify::check_coordinator_claims(network, group_id, seed.as_deref(), &tx, claimed).map(|()| tx))
            .map_or_else(|e| Verdict::Refused(e.to_string()), |tx| Verdict::Verified { tx, auto_approved: false, window: None }),
        None if settings.advanced_mode.unwrap_or(false) => Verdict::Raw,
        None => Verdict::Refused(
            "refused to sign: the request has no transaction attached, so there's no way to check \
             what it would authorize. Signing raw messages with a wallet key is an advanced option."
                .into(),
        ),
    };

    let verdict = match verdict {
        Verdict::Verified { tx, auto_approved, .. } => {
            let window = approval_window(&state, group_id, tx.expiry_height).await;
            Verdict::Verified { tx, auto_approved, window }
        }
        other => other,
    };

    let mut ceremonies = state.ceremonies.lock().await;
    let Some(handle) = ceremonies.get_mut(&ceremony_id) else {
        return verdict;
    };
    match verdict {
        Verdict::Verified { tx, window, .. } => {
            let approved_before = state
                .approved_sighashes
                .lock()
                .await
                .get(&tx.sighash_hex)
                .is_some_and(|at| at.elapsed() < APPROVAL_MEMORY);
            handle.gate = ApprovalGate::Verified { sighash_hex: tx.sighash_hex.clone() };
            let auto_approved = approved_before
                && handle.approval.take().is_some_and(|approval| approval.send(true).is_ok());
            Verdict::Verified { tx, auto_approved, window }
        }
        Verdict::Raw => {
            handle.gate = ApprovalGate::Open;
            Verdict::Raw
        }
        Verdict::Refused(reason) => {
            tracing::warn!("signing request refused: {reason}");
            handle.gate = ApprovalGate::Refused(reason.clone());
            if let Some(approval) = handle.approval.take() {
                let _ = approval.send(false);
            }
            Verdict::Refused(reason)
        }
    }
}

/// How long is left to approve a transaction expiring at `expiry_height`, as
/// the sync server sees the chain. None if it can't say within a few seconds;
/// the request is shown without a deadline then.
async fn approval_window(state: &AppState, group_id: &str, expiry_height: u32) -> Option<ApprovalWindow> {
    use crate::commands::wallet::{group_wallet_ctx, shared_lightwalletd};
    let (network, url, _) = group_wallet_ctx(state, group_id).await.ok()?;
    let tip = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        shared_lightwalletd(state, &url).await.ok()?.latest_height().await.ok()
    })
    .await
    .ok()??;
    ApprovalWindow::at(network, expiry_height, tip, frost_app_core::wallet::now_ms())
}

/// Resolve the approval gate for a participant ceremony.
#[tauri::command]
pub async fn respond_to_signing(
    state: State<'_, AppState>,
    ceremony_id: Uuid,
    approve: bool,
) -> AppResult<()> {
    let mut ceremonies = state.ceremonies.lock().await;
    let handle = ceremonies
        .get_mut(&ceremony_id)
        .ok_or_else(|| AppError::new("ceremony", "ceremony not found"))?;
    if approve {
        match &handle.gate {
            ApprovalGate::Open => {}
            ApprovalGate::Verified { sighash_hex } => {
                let mut approved = state.approved_sighashes.lock().await;
                approved.retain(|_, at| at.elapsed() < APPROVAL_MEMORY);
                approved.insert(sighash_hex.clone(), std::time::Instant::now());
            }
            ApprovalGate::Pending => {
                return Err(AppError::new("verification_failed", "this request is still being verified"));
            }
            ApprovalGate::Refused(reason) => {
                return Err(AppError::new("verification_failed", reason.clone()));
            }
        }
    }
    let approval = handle
        .approval
        .take()
        .ok_or_else(|| AppError::new("ceremony", "ceremony is not awaiting approval"))?;
    approval
        .send(approve)
        .map_err(|_| AppError::new("ceremony", "ceremony is no longer running"))?;
    Ok(())
}

#[derive(Serialize)]
pub struct PendingSession {
    pub session_id: Uuid,
    /// `"signing"`; `"viewing_key"` for agreeing on a group's private viewing
    /// key (`commands::viewing`); or `"group_message"` for group polls
    /// (`commands::polls`). The app joins the last two by itself.
    pub kind: &'static str,
    /// Contact name of the coordinator if known.
    pub coordinator: Option<String>,
    pub coordinator_pubkey: String,
    /// Group IDs in the keystore whose participants include the coordinator.
    pub matching_groups: Vec<String>,
}

/// One-shot poll of the configured server for sessions involving us.
/// The frontend inbox calls this periodically while unlocked.
#[tauri::command]
pub async fn list_pending_sessions(
    state: State<'_, AppState>,
    server_url: Option<String>,
) -> AppResult<Vec<PendingSession>> {
    let server_url = server_url
        .or_else(|| state.load_settings().server_url)
        .ok_or_else(|| AppError::new("config", "no server is set up for your groups to meet on. Choose one under Settings → Connection"))?;
    let server_url = server_url
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string();

    let (comm_privkey, comm_pubkey, contacts, groups) = state
        .with_config(|config| {
            let comm = config
                .communication_key
                .as_ref()
                .ok_or_else(|| AppError::new("config", "keystore has no communication key"))?;
            let contacts: Vec<(String, Vec<u8>)> = config
                .contact
                .values()
                .map(|c| (c.name.clone(), c.pubkey.0.clone()))
                .collect();
            let groups: Vec<(String, Vec<Vec<u8>>)> = config
                .group
                .iter()
                .map(|(id, g)| {
                    (
                        id.clone(),
                        g.participant.values().map(|p| p.pubkey.0.clone()).collect(),
                    )
                })
                .collect();
            Ok((comm.privkey.clone(), comm.pubkey.clone(), contacts, groups))
        })
        .await?;

    let trust = crate::commands::server::trust_for(&state, &server_url).await;
    let mut client =
        frost_app_core::transport::FrostdClient::new(format!("https://{server_url}"), &trust)?;
    client.login(&comm_privkey, &comm_pubkey).await?;

    let dkg_sessions = state.dkg_sessions.lock().map(|s| s.clone()).unwrap_or_default();
    let viewing_key_sessions = state.viewing_key_sessions.lock().map(|s| s.clone()).unwrap_or_default();
    let group_message_sessions = state.group_message_sessions.lock().map(|s| s.clone()).unwrap_or_default();
    let mut pending = Vec::new();
    for session_id in client.list_sessions().await?.session_ids {
        // A key-generation or viewing-key session the organizer hasn't closed yet.
        if dkg_sessions.contains(&session_id)
            || viewing_key_sessions.contains(&session_id)
            || group_message_sessions.contains(&session_id)
        {
            continue;
        }
        let info = client
            .get_session_info(&api::GetSessionInfoArgs { session_id })
            .await?;
        // Skip sessions we coordinate ourselves.
        if info.coordinator_pubkey == comm_pubkey {
            continue;
        }
        let coordinator = contacts
            .iter()
            .find(|(_, pk)| *pk == info.coordinator_pubkey.0)
            .map(|(name, _)| name.clone());
        // Which of our groups is this session for? frostd is group-agnostic — a
        // session carries no group id — so the *signer set* is what identifies it.
        // Matching on the coordinator alone lights the session up under every
        // group that merely shares that coordinator, which is the common case
        // (same people, several groups). Require instead that every pubkey in the
        // session is a member of the group: a threshold subset of group A is not
        // a subset of group B unless B contains all of them.
        let session_pubkeys: Vec<&Vec<u8>> = info.pubkeys.iter().map(|p| &p.0).collect();
        let matching_groups = groups
            .iter()
            .filter(|(_, members)| {
                members.contains(&info.coordinator_pubkey.0)
                    && session_pubkeys.iter().all(|pk| members.contains(pk))
            })
            .map(|(id, _)| id.clone())
            .collect();
        pending.push(PendingSession {
            session_id,
            kind: match info.message_count {
                frost_app_core::viewing::NEW_VIEWING_KEY_SESSION => "viewing_key",
                frost_app_core::polls::GROUP_MESSAGE_SESSION => "group_message",
                _ => "signing",
            },
            coordinator,
            coordinator_pubkey: hex::encode(&info.coordinator_pubkey.0),
            matching_groups,
        });
    }
    let _ = client.logout().await;
    Ok(pending)
}
