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
            if let (true, ParticipantEvent::AwaitingApproval { message_hex, context, randomizer_hex, coordinator, signers }) =
                (wallet_group, &event)
            {
                let request = WalletRequest {
                    message_hex,
                    context: context.as_deref(),
                    randomizer_hex: randomizer_hex.as_deref(),
                    coordinator,
                    signers: *signers,
                };
                gate_wallet_request(&event_app, ceremony_id, &gate_group, request)
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
    Verified {
        tx: VerifiedTransaction,
        auto_approved: bool,
        window: Option<ApprovalWindow>,
        poll: Option<Box<PollCheck>>,
        rules: Option<RulesCheck>,
        /// This device's decision on approving it within the group's spending
        /// limit, when it approves automatically there.
        auto: Option<Box<crate::commands::rules::AutoDecision>>,
    },
    /// No transaction attached, and raw signing is allowed (advanced options).
    Raw,
    /// Refused automatically, for this reason.
    Refused(String),
}

impl Verdict {
    /// Add the decision to the `awaiting_approval` event the UI receives.
    fn annotate(&self, event: &mut serde_json::Value) {
        match self {
            Verdict::Verified { tx, auto_approved, window, poll, rules, auto } => {
                event["verified"] = serde_json::json!(tx);
                event["auto_approved"] = serde_json::json!(auto_approved);
                event["window"] = serde_json::json!(window);
                if let Some(poll) = poll {
                    event["poll"] = serde_json::json!(poll);
                }
                if let Some(rules) = rules {
                    event["rules"] = serde_json::json!(rules);
                }
                if let Some(auto) = auto {
                    event["auto_limit"] = serde_json::json!(auto);
                }
            }
            Verdict::Raw => event["unverified"] = serde_json::json!(true),
            Verdict::Refused(reason) => event["refused"] = serde_json::json!(reason),
        }
    }
}

/// What an approver's app found about the group poll a request says it sends
/// the agreed payments of, from its own copy of the poll.
#[derive(serde::Serialize)]
struct PollCheck {
    poll_id: String,
    /// This device doesn't have the poll (yet): nothing to check it against.
    missing: bool,
    title: String,
    /// The group agreed to pay.
    agreed: bool,
    /// The group agreed not to.
    declined: bool,
    /// How many members chose "Pay", and how many it takes.
    chose_pay: u32,
    threshold: u32,
    /// Under a delay rule: how long the payment waits after it's announced,
    /// and when this device learned of it (unix seconds). The gate already
    /// refused it if that hasn't passed, or a member vetoed it.
    #[serde(skip_serializing_if = "Option::is_none")]
    delay_secs: Option<u64>,
    announced_at: u64,
}

/// What a wallet signing request carries, as the approval event gives it.
struct WalletRequest<'a> {
    message_hex: &'a str,
    context: Option<&'a SigningContext>,
    randomizer_hex: Option<&'a str>,
    /// Hex communication key of the member who asked.
    coordinator: &'a str,
    /// How many members' approvals it takes.
    signers: u32,
}

/// The group's rules a request met, for the approver to see. Only for a group
/// that adopted rules.
#[derive(serde::Serialize)]
struct RulesCheck {
    trusted_payees_only: bool,
    /// Approvals the rules need for this payment, and how many it asks.
    needed: u32,
    asked: u32,
}

/// Check a request that says it sends a poll's agreed payments against this
/// device's copy of the poll. Err (refuse) if it was proposed by another
/// member, was already paid, or the transaction doesn't make exactly the
/// payments it proposed.
async fn check_poll(
    state: &AppState,
    network: frost_app_core::wallet::WalletNetwork,
    group_id: &str,
    poll_id: &str,
    coordinator: &str,
    tx: &VerifiedTransaction,
) -> Result<PollCheck, String> {
    let Ok(poll) = crate::commands::polls::find(state, group_id, poll_id).await else {
        return Ok(PollCheck {
            poll_id: poll_id.to_string(),
            missing: true,
            title: String::new(),
            agreed: false,
            declined: false,
            chose_pay: 0,
            threshold: 0,
            delay_secs: None,
            announced_at: 0,
        });
    };
    if poll.payments.is_empty() {
        return Err("refused to sign: the request names a poll that doesn't propose a payment".into());
    }
    if poll.author != coordinator {
        return Err("refused to sign: only the member who proposed a payment can send it, and another member asked".into());
    }
    if let Some(txid) = &poll.paid_txid {
        return Err(format!("refused to sign: the poll's payment was already sent, in {txid}"));
    }
    if !crate::commands::polls::pays_what_was_proposed(network, &poll.payments, tx) {
        return Err("refused to sign: the transaction doesn't make the payments the poll proposed".into());
    }
    // Under a delay rule, an announcement goes only once its delay has passed
    // since this device learned of it, with no veto.
    let rules = crate::commands::rules::in_effect(state, group_id)
        .await
        .map_err(|e| format!("refused to sign: couldn't read the group's rules ({})", e.message))?
        .rules;
    let total = poll.payments.iter().map(|p| p.amount_zatoshis).sum();
    let delay = rules.delay_for(total);
    if delay.is_some() {
        crate::commands::rules::poll_permits(&rules, &poll, crate::commands::wallet::unix_now())
            .map_err(|e| format!("refused to sign: {e}"))?;
    }
    Ok(PollCheck {
        poll_id: poll.poll_id,
        missing: false,
        title: poll.title,
        agreed: poll.agreed == Some(0),
        declined: poll.agreed == Some(1),
        chose_pay: poll.tally.first().copied().unwrap_or(0),
        threshold: poll.threshold,
        delay_secs: delay.map(|d| d.secs),
        announced_at: poll.announced_at,
    })
}

/// Verify a wallet signing request before its approval can be given (see
/// `frost_app_core::wallet::verify`), and settle or refuse the ceremony's gate
/// accordingly.
async fn gate_wallet_request<R: tauri::Runtime>(
    app: &AppHandle<R>,
    ceremony_id: Uuid,
    group_id: &str,
    request: WalletRequest<'_>,
) -> Verdict {
    let WalletRequest { message_hex, context, randomizer_hex, coordinator, signers } = request;
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
            .map_or_else(
                |e| Verdict::Refused(e.to_string()),
                |tx| Verdict::Verified { tx, auto_approved: false, window: None, poll: None, rules: None, auto: None },
            ),
        None if settings.advanced_mode.unwrap_or(false) => Verdict::Raw,
        None => Verdict::Refused(
            "refused to sign: the request has no transaction attached, so there's no way to check \
             what it would authorize. Signing raw messages with a wallet key is an advanced option."
                .into(),
        ),
    };

    let verdict = match verdict {
        Verdict::Verified { tx, auto_approved, .. } => {
            let poll = match context.and_then(|c| c.poll_id.as_deref()) {
                Some(poll_id) => Some(check_poll(&state, network, group_id, poll_id, coordinator, &tx).await),
                None => None,
            };
            // The group's rules, as this device sees them. Refused if they
            // can't be read: they may forbid it.
            // It carries out an announcement this device has, ready to send.
            let announced = matches!(&poll, Some(Ok(check)) if !check.missing);
            let rules = match crate::commands::rules::check(&state, group_id, &tx, signers, announced).await {
                Ok((_, Err(broken))) => Err(format!("refused to sign: {broken}")),
                Ok((in_effect, Ok(()))) => Ok((!in_effect.rules.is_empty()).then(|| RulesCheck {
                    trusted_payees_only: in_effect.rules.trusted_payees_only,
                    needed: in_effect.rules.approvals_needed(tx.payments.iter().map(|p| p.value_zatoshis).sum(), in_effect.threshold),
                    asked: signers,
                })),
                Err(e) => Err(format!("refused to sign: couldn't read the group's rules ({})", e.message)),
            };
            match (poll.transpose(), rules) {
                (Err(reason), _) | (_, Err(reason)) => Verdict::Refused(reason),
                (Ok(poll), Ok(rules)) => {
                    let window = approval_window(&state, group_id, tx.expiry_height).await;
                    let auto = crate::commands::rules::automatic_approval(&state, group_id, &tx).await.map(Box::new);
                    let poll = poll.map(Box::new);
                    Verdict::Verified { tx, auto_approved, window, poll, rules, auto }
                }
            }
        }
        other => other,
    };

    let mut ceremonies = state.ceremonies.lock().await;
    let Some(handle) = ceremonies.get_mut(&ceremony_id) else {
        return verdict;
    };
    match verdict {
        Verdict::Verified { tx, window, poll, rules, auto, .. } => {
            let approved_before = state
                .approved_sighashes
                .lock()
                .await
                .get(&tx.sighash_hex)
                .is_some_and(|at| at.elapsed() < APPROVAL_MEMORY);
            handle.gate = ApprovalGate::Verified { sighash_hex: tx.sighash_hex.clone() };
            // Within the group's spending limit, approved by itself: once per
            // transaction, so its later rounds count as approved already.
            let by_limit = !approved_before && auto.as_ref().is_some_and(|a| a.approved);
            let auto_approved = (approved_before || by_limit)
                && handle.approval.take().is_some_and(|approval| approval.send(true).is_ok());
            if auto_approved && by_limit {
                let mut approved = state.approved_sighashes.lock().await;
                approved.retain(|_, at| at.elapsed() < APPROVAL_MEMORY);
                approved.insert(tx.sighash_hex.clone(), std::time::Instant::now());
                drop(approved);
                let cost = auto.as_ref().map_or(0, |a| a.cost);
                crate::commands::rules::record_automatic_approval(&state, group_id, cost).await;
                tracing::info!(cost, "approved a payment automatically, within the group's spending limit");
            }
            // A later round of a transaction approved by hand isn't an
            // automatic approval within the limit.
            let auto = auto.filter(|_| !approved_before);
            Verdict::Verified { tx, auto_approved, window, poll, rules, auto }
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
