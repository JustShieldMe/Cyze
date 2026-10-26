//! Repairing a member's lost share (see `frost_app_core::repair`): starting a
//! repair and helping with one from a group, and receiving one on the new
//! device of the member who lost theirs.
//!
//! Once the member's app has the share back, each helper's app switches the
//! group to the member's new communication key and vouches for it in the
//! group's records (`RecordBody::MemberKey`); the other members' apps switch
//! once as many members vouch for it as a payment needs approvals
//! ([`apply_vouched_keys`]).

use std::collections::BTreeMap;

use frost_app_core::polls::RecordBody;
use frost_app_core::repair::{self, Connection, GroupData, HelperParams, RepairCode, RepairEvent, Start};
use frost_client::api::PublicKey;
use frost_client::cli::config::{Group, Participant};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::commands::polls::write;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, ApprovalGate, CeremonyHandle};

/// This device's connection to a group's coordination server.
async fn connection(state: &AppState, server_url: String) -> AppResult<Connection> {
    let trust = crate::commands::server::trust_for(state, &server_url).await;
    state
        .with_config(|config| {
            let comm = config
                .communication_key
                .as_ref()
                .ok_or_else(|| AppError::new("config", "keystore has no communication key"))?;
            Ok(Connection { server_url, trust, comm_privkey: comm.privkey.clone(), comm_pubkey: comm.pubkey.clone() })
        })
        .await
}

/// The group as this device keeps it, for a repaired member, and its key package.
async fn group_data(state: &AppState, group_id: &str) -> AppResult<(GroupData, Vec<u8>)> {
    let seed = state.viewing_seed(group_id).await?.map(|s| hex::encode(*s));
    let birthdays: BTreeMap<String, u64> = state
        .load_private()
        .await?
        .network_birthdays
        .into_iter()
        .filter(|(key, _)| key.ends_with(&format!("/{group_id}")))
        .collect();
    state
        .with_config(|config| {
            let group = config.group.get(group_id).ok_or_else(|| AppError::new("config", "group not found"))?;
            let summary = frost_app_core::config::summarize_group(group_id, group)?;
            let data = GroupData {
                group_id: group_id.to_string(),
                description: group.description.clone(),
                ciphersuite: group.ciphersuite.clone(),
                threshold: summary.threshold as u16,
                public_key_package: hex::encode(&group.public_key_package),
                server_url: group.server_url.clone(),
                members: group.participant.iter().map(|(id, p)| (id.clone(), hex::encode(&p.pubkey.0))).collect(),
                viewing_seed: seed,
                birthdays,
            };
            Ok((data, group.key_package.clone()))
        })
        .await
}

fn emit_progress<R: Runtime>(app: &AppHandle<R>, ceremony_id: Uuid, event: &RepairEvent) {
    let mut value = serde_json::json!(event);
    // The new key's safety code, for the helper to compare with its owner.
    if let RepairEvent::AwaitingApproval { new_key, .. } = event {
        if let Ok(bytes) = hex::decode(new_key) {
            value["new_key_safety_code"] = serde_json::json!(frost_app_core::safety::safety_code(&bytes));
        }
    }
    let _ = app.emit("repair:progress", serde_json::json!({ "ceremony_id": ceremony_id, "event": value }));
}

fn forward_events<R: Runtime>(app: &AppHandle<R>, ceremony_id: Uuid) -> mpsc::Sender<RepairEvent> {
    let (tx, mut rx) = mpsc::channel(32);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            emit_progress(&app, ceremony_id, &event);
        }
    });
    tx
}

/// A repair this member started, and the code the member being repaired
/// pastes into their new app.
#[derive(Serialize)]
pub struct StartedRepair {
    pub ceremony_id: Uuid,
    pub code: String,
}

/// Start repairing a member's share: `identifier` (hex) is the member,
/// `new_contact` their new contact code, and `helpers` the members helping
/// (hex communication keys, this one included, at least the threshold).
#[tauri::command]
pub async fn start_repair<R: Runtime>(
    app: AppHandle<R>,
    group_id: String,
    identifier: String,
    new_contact: String,
    helpers: Vec<String>,
) -> AppResult<StartedRepair> {
    let state = app.state::<AppState>();
    let new_key = frost_app_core::config::contact_from_text(&new_contact)?.pubkey;
    let (data, key_package) = group_data(&state, &group_id).await?;
    let me = state
        .with_config(|c| Ok(c.communication_key.as_ref().map(|k| hex::encode(&k.pubkey.0)).unwrap_or_default()))
        .await?;
    let member_keys: Vec<&String> = data.members.values().collect();
    let old_key = data.members.get(&identifier).ok_or_else(|| AppError::new("config", "that isn't a member of this group"))?;
    if helpers.contains(old_key) {
        return Err(AppError::new("config", "the member being repaired can't help repair their own share"));
    }
    if !helpers.contains(&me) {
        return Err(AppError::new("config", "you have to be one of the helpers"));
    }
    if !helpers.iter().all(|h| member_keys.contains(&h)) || helpers.len() < usize::from(data.threshold) {
        return Err(AppError::new("config", format!("choose at least {} helpers from the group", data.threshold)));
    }
    if member_keys.contains(&&hex::encode(&new_key.0)) {
        return Err(AppError::new("config", "that contact code is already a member's: ask for the member's new one"));
    }
    let helper_keys: Vec<PublicKey> =
        helpers.iter().map(|h| hex::decode(h).map(PublicKey)).collect::<Result<_, _>>().map_err(|_| AppError::new("config", "a malformed helper key"))?;

    let (_, server_url) = crate::commands::signing::group_context(&state, &group_id, None).await?;
    let conn = connection(&state, server_url.clone()).await?;
    let session_id = repair::create_session(&conn, &helper_keys, &new_key).await?;
    let member_name = crate::commands::contacts::list_contacts(state.clone())
        .await?
        .into_iter()
        .find(|c| c.pubkey == *old_key)
        .map(|c| c.alias.unwrap_or(c.name))
        .unwrap_or_else(|| "the member".into());
    let code = RepairCode {
        server: server_url.clone(),
        cert_pem: state.load_settings().trusted_certs.get(&server_url).cloned(),
        session_id,
        group: data.description.clone(),
        member: member_name,
    }
    .encode()?;

    let ceremony_id = Uuid::new_v4();
    let (approve, approval) = oneshot::channel();
    let _ = approve.send(true);
    let start = Some(Start { identifier, new_key, helpers: helper_keys });
    run_helper(&app, ceremony_id, group_id, HelperParams { conn, session_id, group: data, key_package, start }, approval).await;
    Ok(StartedRepair { ceremony_id, code })
}

/// Help with a repair another member started: the app shows the request and
/// goes on once this member approves (`respond_to_signing`).
#[tauri::command]
pub async fn join_repair<R: Runtime>(app: AppHandle<R>, group_id: String, session_id: Uuid) -> AppResult<Uuid> {
    let state = app.state::<AppState>();
    let (data, key_package) = group_data(&state, &group_id).await?;
    let (_, server_url) = crate::commands::signing::group_context(&state, &group_id, None).await?;
    let conn = connection(&state, server_url).await?;
    let ceremony_id = Uuid::new_v4();
    let (approve, approval) = oneshot::channel();
    state.ceremonies.lock().await.insert(
        ceremony_id,
        CeremonyHandle { cancel: CancellationToken::new(), approval: Some(approve), gate: ApprovalGate::Open },
    );
    run_helper(&app, ceremony_id, group_id, HelperParams { conn, session_id, group: data, key_package, start: None }, approval).await;
    Ok(ceremony_id)
}

async fn run_helper<R: Runtime>(
    app: &AppHandle<R>,
    ceremony_id: Uuid,
    group_id: String,
    params: HelperParams,
    approval: oneshot::Receiver<bool>,
) {
    let state = app.state::<AppState>();
    let cancel = {
        let mut ceremonies = state.ceremonies.lock().await;
        let handle = ceremonies
            .entry(ceremony_id)
            .or_insert_with(|| CeremonyHandle { cancel: CancellationToken::new(), approval: None, gate: ApprovalGate::Open });
        handle.cancel.clone()
    };
    let events = forward_events(app, ceremony_id);
    let task_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = repair::help(params, events, approval, cancel).await;
        let state = task_app.state::<AppState>();
        state.ceremonies.lock().await.remove(&ceremony_id);
        match result {
            Ok(outcome) => {
                // Vouch to the members as they are, before switching: an app
                // that doesn't know the new key yet refuses a session holding
                // it, and needs this to learn it. If it can't go now, the
                // others still get it by catching up.
                let vouch = RecordBody::MemberKey { identifier: outcome.identifier.clone(), pubkey: hex::encode(&outcome.new_key.0) };
                let vouched = write(&task_app, &group_id, vouch).await;
                if let Err(e) = crate::commands::polls::deliver(&task_app, &group_id).await {
                    tracing::info!("couldn't send the vouch for the new key yet: {}", e.message);
                }
                let switched = switch_member_key(&state, &group_id, &outcome.identifier, &outcome.new_key).await;
                if let Err(e) = vouched.and(switched) {
                    tracing::warn!("repaired, but couldn't switch to the member's new key: {}", e.message);
                }
                let _ = task_app.emit("repair:complete", serde_json::json!({ "ceremony_id": ceremony_id, "group_id": group_id }));
            }
            Err(e) => {
                let _ = task_app.emit("repair:failed", serde_json::json!({ "ceremony_id": ceremony_id, "error": e.to_string() }));
            }
        }
    });
}

/// Use `new_key` as member `identifier`'s communication key in a group.
async fn switch_member_key(state: &AppState, group_id: &str, identifier: &str, new_key: &PublicKey) -> AppResult<()> {
    state
        .mutate_config(|config| {
            let group = config.group.get_mut(group_id).ok_or_else(|| AppError::new("config", "group not found"))?;
            let member = group.participant.get_mut(identifier).ok_or_else(|| AppError::new("config", "member not found"))?;
            member.pubkey = new_key.clone();
            Ok(())
        })
        .await
}

/// Switch to members' new keys that enough members vouch for in the group's
/// records (after a repair this device didn't help with). Returns whether any
/// changed.
pub(crate) async fn apply_vouched_keys(state: &AppState, group_id: &str) -> AppResult<bool> {
    let g = crate::commands::polls::group(state, group_id).await?;
    let logs = state.load_polls().await?;
    let Some(log) = logs.get(group_id) else { return Ok(false) };
    let vouched = frost_app_core::polls::vouched_member_keys(log, &g.member_hexes(), g.threshold);
    if vouched.is_empty() {
        return Ok(false);
    }
    state
        .mutate_config(|config| {
            let Some(group) = config.group.get_mut(group_id) else { return Ok(false) };
            let mut changed = false;
            for (identifier, key) in vouched {
                let Ok(key) = hex::decode(&key) else { continue };
                if let Some(member) = group.participant.get_mut(&identifier) {
                    if member.pubkey.0 != key {
                        member.pubkey = PublicKey(key);
                        changed = true;
                    }
                }
            }
            Ok(changed)
        })
        .await
}

/// What a repair code says, to show before joining.
#[derive(Serialize)]
pub struct RepairCodeInfo {
    pub group: String,
    pub member: String,
    pub server: String,
}

#[tauri::command]
pub async fn read_repair_code(code: String) -> AppResult<RepairCodeInfo> {
    let code = RepairCode::decode(&code)?;
    Ok(RepairCodeInfo { group: code.group, member: code.member, server: code.server })
}

/// On the new device of the member being repaired: join the repair a repair
/// code points to, and once every helper's part checks out, keep the group.
#[tauri::command]
pub async fn receive_repair<R: Runtime>(app: AppHandle<R>, code: String) -> AppResult<Uuid> {
    let state = app.state::<AppState>();
    let code = RepairCode::decode(&code)?;
    // The server it runs on, trusted as the helper's app trusts it.
    let mut settings = state.load_settings();
    if let Some(pem) = &code.cert_pem {
        frost_app_core::tls::cert_fingerprint(pem)?;
        settings.trusted_certs.insert(code.server.clone(), pem.clone());
    }
    if settings.server_url.is_none() {
        settings.server_url = Some(code.server.clone());
    }
    state.save_settings(&settings)?;
    let conn = connection(&state, code.server.clone()).await?;

    let ceremony_id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    state
        .ceremonies
        .lock()
        .await
        .insert(ceremony_id, CeremonyHandle { cancel: cancel.clone(), approval: None, gate: ApprovalGate::Open });
    let events = forward_events(&app, ceremony_id);
    let task_app = app.clone();
    let session_id = code.session_id;
    tauri::async_runtime::spawn(async move {
        let result = repair::receive(conn, session_id, events, cancel).await;
        let state = task_app.state::<AppState>();
        state.ceremonies.lock().await.remove(&ceremony_id);
        let saved = match result {
            Ok(repaired) => keep_repaired_group(&state, repaired).await,
            Err(e) => Err(AppError::from(e)),
        };
        match saved {
            Ok(group_id) => {
                let _ = task_app.emit("repair:complete", serde_json::json!({ "ceremony_id": ceremony_id, "group_id": group_id }));
                let _ = task_app.emit("polls:updated", serde_json::json!({ "group_id": group_id, "new_polls": 0 }));
            }
            Err(e) => {
                let _ = task_app.emit("repair:failed", serde_json::json!({ "ceremony_id": ceremony_id, "error": e.message }));
            }
        }
    });
    Ok(ceremony_id)
}

/// Save a group a repair gave this device: the group with its key package,
/// its viewing seed, and where its wallet starts scanning.
async fn keep_repaired_group(state: &AppState, repaired: repair::Repaired) -> AppResult<String> {
    let data = repaired.group;
    let group_id = data.group_id.clone();
    let participant = data
        .members
        .iter()
        .map(|(id, key)| {
            let identifier = hex::decode(id).map_err(|_| AppError::new("repair", "a malformed member identifier"))?;
            let pubkey = PublicKey(hex::decode(key).map_err(|_| AppError::new("repair", "a malformed member key"))?);
            Ok((id.clone(), Participant { identifier, pubkey }))
        })
        .collect::<AppResult<BTreeMap<_, _>>>()?;
    let group = Group {
        description: data.description.clone(),
        ciphersuite: data.ciphersuite.clone(),
        public_key_package: hex::decode(&data.public_key_package).map_err(|_| AppError::new("repair", "a malformed public key package"))?,
        key_package: repaired.key_package,
        server_url: data.server_url.clone(),
        participant,
    };
    let seed = data.viewing_seed.clone();
    let id = group_id.clone();
    state
        .mutate_keystore(move |config, extras| {
            if config.group.contains_key(&id) {
                return Err(AppError::new("repair", "this device already has that group"));
            }
            if let Some(seed) = seed {
                extras.viewing_seeds.insert(id.clone(), seed);
            }
            config.group.insert(id, group);
            Ok(())
        })
        .await?;
    let birthdays = data.birthdays.clone();
    state
        .update_private(|p| {
            for (key, height) in birthdays {
                p.network_birthdays.entry(key).or_insert(height);
            }
        })
        .await?;
    state.sync_service.wake(false);
    Ok(group_id)
}
