//! Group polls (see `frost_app_core::polls`): this device's records, kept
//! encrypted at rest, and the commands and background delivery around them.
//!
//! Records this device writes go into its outbox and out to the other
//! members straight away; anything that couldn't be sent (the coordination
//! server was down) goes out on the next pass of [`spawn`], which also asks
//! the others for anything this device missed while it was offline.

use std::collections::HashMap;
use std::time::Duration;

use frost_app_core::community_vote::{self, CommunityVotes};
use frost_app_core::polls::{self, Channel, GroupLog, Message, PollView, Record, RecordBody};
use frost_client::api::PublicKey;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use uuid::Uuid;

use crate::commands::wallet::unix_now;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const FILE: &str = "group-polls.enc";
const KEY_INFO: &[u8] = b"group-polls-v1";
const LABEL: &[u8] = b"cyze-group-polls-v1";

/// Every group's poll records on this device, by group id.
type Logs = HashMap<String, GroupLog>;

impl AppState {
    async fn polls_key(&self) -> AppResult<zeroize::Zeroizing<[u8; 32]>> {
        let guard = self.unlocked.read().await;
        let unlocked = guard.as_ref().ok_or_else(AppError::locked)?;
        Ok(unlocked.file.derive_subkey(KEY_INFO))
    }

    async fn load_polls(&self) -> AppResult<Logs> {
        let key = self.polls_key().await?;
        let Ok(sealed) = std::fs::read(self.data_dir.join(FILE)) else {
            return Ok(Logs::default());
        };
        let plain = frost_app_core::sealed::open(&key, LABEL, &sealed)?;
        serde_json::from_slice(&plain).map_err(|e| AppError::new("config", format!("group polls: {e}")))
    }

    /// Change one group's records, serialized with other changes.
    async fn update_polls<T>(&self, group_id: &str, change: impl FnOnce(&mut GroupLog) -> T) -> AppResult<T> {
        let _serialized = self.polls_lock.lock().await;
        let mut logs = self.load_polls().await?;
        let out = change(logs.entry(group_id.to_string()).or_default());
        let plain = zeroize::Zeroizing::new(
            serde_json::to_vec(&logs).map_err(|e| AppError::new("config", e.to_string()))?,
        );
        let sealed = frost_app_core::sealed::seal(&*self.polls_key().await?, LABEL, &plain)?;
        frost_app_core::keystore::write_atomic(&self.data_dir.join(FILE), &sealed)?;
        Ok(out)
    }
}

/// A group's members and threshold, and who this device is.
struct Group {
    members: Vec<PublicKey>,
    threshold: u32,
    me: PublicKey,
}

impl Group {
    fn others(&self) -> Vec<PublicKey> {
        self.members.iter().filter(|p| **p != self.me).cloned().collect()
    }
    fn member_hexes(&self) -> Vec<String> {
        self.members.iter().map(|p| hex::encode(&p.0)).collect()
    }
}

async fn group(state: &AppState, group_id: &str) -> AppResult<Group> {
    state
        .with_config(|config| {
            let group = config.group.get(group_id).ok_or_else(|| AppError::new("config", "group not found"))?;
            let summary = frost_app_core::config::summarize_group(group_id, group)?;
            let me = config
                .communication_key
                .as_ref()
                .ok_or_else(|| AppError::new("config", "keystore has no communication key"))?
                .pubkey
                .clone();
            Ok(Group {
                members: group.participant.values().map(|p| p.pubkey.clone()).collect(),
                threshold: summary.threshold as u32,
                me,
            })
        })
        .await
}

async fn channel(state: &AppState, group_id: &str) -> AppResult<Channel> {
    let (_, server_url) = crate::commands::signing::group_context(state, group_id, None).await?;
    let trust = crate::commands::server::trust_for(state, &server_url).await;
    state
        .with_config(|config| {
            let comm = config
                .communication_key
                .as_ref()
                .ok_or_else(|| AppError::new("config", "keystore has no communication key"))?;
            Ok(Channel { server_url, trust, comm_privkey: comm.privkey.clone(), comm_pubkey: comm.pubkey.clone() })
        })
        .await
}

fn updated<R: Runtime>(app: &AppHandle<R>, group_id: &str, new_polls: usize) {
    let _ = app.emit("polls:updated", serde_json::json!({ "group_id": group_id, "new_polls": new_polls }));
}

/// The polls in one group, or in every group.
#[tauri::command]
pub async fn list_polls(state: State<'_, AppState>, group_id: Option<String>) -> AppResult<Vec<PollView>> {
    let logs = state.load_polls().await?;
    let ids: Vec<String> = match group_id {
        Some(id) => vec![id],
        None => state.with_config(|c| Ok(c.group.keys().cloned().collect())).await?,
    };
    let now = unix_now();
    let mut out = Vec::new();
    for id in ids {
        let Some(log) = logs.get(&id) else { continue };
        let g = group(&state, &id).await?;
        out.extend(polls::polls(&id, log, &g.member_hexes(), g.threshold, &hex::encode(&g.me.0), now));
    }
    out.sort_by_key(|p| (p.closed, std::cmp::Reverse(p.created_at)));
    Ok(out)
}

#[derive(Deserialize)]
pub struct CreatePollArgs {
    pub group_id: String,
    pub title: String,
    #[serde(default)]
    pub details: String,
    pub options: Vec<String>,
    /// How long it stays open.
    pub open_for_secs: u64,
}

/// Ask the group a question. Returns the poll's id.
#[tauri::command]
pub async fn create_poll<R: Runtime>(app: AppHandle<R>, args: CreatePollArgs) -> AppResult<String> {
    let poll_id = Uuid::new_v4().to_string();
    let open_for = args.open_for_secs.clamp(60 * 60, 90 * 24 * 60 * 60);
    let body = RecordBody::Poll {
        poll_id: poll_id.clone(),
        title: args.title.trim().to_string(),
        details: args.details.trim().to_string(),
        options: args.options.iter().map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect(),
        closes_at: unix_now() + open_for,
        community: None,
    };
    write(&app, &args.group_id, body).await?;
    Ok(poll_id)
}

/// Zcash community votes on the configured network: open rounds, and the
/// latest finished ones with results. Read at most every few minutes.
#[tauri::command]
pub async fn community_votes(state: State<'_, AppState>) -> AppResult<CommunityVotes> {
    let network = crate::commands::wallet::configured_network(&state.load_settings()).as_str().to_string();
    if let Some((at, net, votes)) = state.community_votes.lock().await.as_ref() {
        if *net == network && at.elapsed() < COMMUNITY_REFRESH {
            return Ok(votes.clone());
        }
    }
    let votes = community_vote::fetch(&network).await?;
    *state.community_votes.lock().await = Some((std::time::Instant::now(), network, votes.clone()));
    Ok(votes)
}

/// How long community votes are reused before being read again.
const COMMUNITY_REFRESH: Duration = Duration::from_secs(10 * 60);

/// Ask the group about proposals in an open community vote: one poll per
/// proposal, with the proposal's options, closing an hour before the round
/// does. Returns how many polls it created.
#[tauri::command]
pub async fn poll_community_round<R: Runtime>(
    app: AppHandle<R>,
    group_id: String,
    round_id: String,
    proposal_ids: Vec<u32>,
) -> AppResult<u32> {
    let votes = community_votes(app.state::<AppState>()).await?;
    let round = votes
        .open
        .into_iter()
        .find(|r| r.id == round_id)
        .ok_or_else(|| AppError::new("config", "that community vote isn't open"))?;
    let closes_at = round.vote_end_time.saturating_sub(60 * 60).max(unix_now() + 60 * 60);
    let mut created = 0;
    for proposal in round.proposals.iter().filter(|p| proposal_ids.contains(&p.id)) {
        let details: String = format!("Zcash community vote: {}.\n\n{}", round.title, proposal.description)
            .chars()
            .take(polls::MAX_DETAILS)
            .collect();
        let body = RecordBody::Poll {
            poll_id: Uuid::new_v4().to_string(),
            title: proposal.title.chars().take(polls::MAX_TITLE).collect(),
            details,
            options: proposal.options.iter().take(polls::MAX_OPTIONS).map(|o| o.chars().take(polls::MAX_OPTION).collect()).collect(),
            closes_at,
            community: Some(polls::CommunityRef { round_id: round.id.clone(), proposal_id: proposal.id }),
        };
        write(&app, &group_id, body).await?;
        created += 1;
    }
    Ok(created)
}

/// Answer a poll, or change this member's answer while it's open.
#[tauri::command]
pub async fn vote_poll<R: Runtime>(app: AppHandle<R>, group_id: String, poll_id: String, choice: u32) -> AppResult<()> {
    let poll = find(&app.state::<AppState>(), &group_id, &poll_id).await?;
    if poll.closed {
        return Err(AppError::new("config", "this poll has closed"));
    }
    if choice as usize >= poll.options.len() {
        return Err(AppError::new("config", "that isn't one of the poll's options"));
    }
    write(&app, &group_id, RecordBody::Vote { poll_id, choice }).await
}

/// Close a poll this member asked, before its time is up.
#[tauri::command]
pub async fn close_poll<R: Runtime>(app: AppHandle<R>, group_id: String, poll_id: String) -> AppResult<()> {
    let state = app.state::<AppState>();
    let poll = find(&state, &group_id, &poll_id).await?;
    let me = hex::encode(&group(&state, &group_id).await?.me.0);
    if poll.author != me {
        return Err(AppError::new("config", "only the member who asked can close a poll early"));
    }
    write(&app, &group_id, RecordBody::Close { poll_id }).await
}

async fn find(state: &AppState, group_id: &str, poll_id: &str) -> AppResult<PollView> {
    list_polls_in(state, group_id)
        .await?
        .into_iter()
        .find(|p| p.poll_id == poll_id)
        .ok_or_else(|| AppError::new("config", "poll not found"))
}

async fn list_polls_in(state: &AppState, group_id: &str) -> AppResult<Vec<PollView>> {
    let logs = state.load_polls().await?;
    let g = group(state, group_id).await?;
    Ok(logs
        .get(group_id)
        .map(|log| polls::polls(group_id, log, &g.member_hexes(), g.threshold, &hex::encode(&g.me.0), unix_now()))
        .unwrap_or_default())
}

/// Store a record this member wrote and send it to the others.
async fn write<R: Runtime>(app: &AppHandle<R>, group_id: &str, body: RecordBody) -> AppResult<()> {
    let state = app.state::<AppState>();
    let me = hex::encode(&group(&state, group_id).await?.me.0);
    let record = state
        .update_polls(group_id, |log| {
            let record = Record { group_id: group_id.to_string(), seq: log.next_seq(&me), at: unix_now(), body };
            record.check()?;
            log.insert(&me, record.clone());
            log.outbox.push(record.seq);
            Ok::<_, frost_app_core::CoreError>(record)
        })
        .await??;
    updated(app, group_id, 0);
    tracing::debug!(seq = record.seq, "wrote a group poll record");
    let app = app.clone();
    let group_id = group_id.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = deliver(&app, &group_id).await {
            tracing::info!("couldn't send poll records yet; they'll go out later: {}", e.message);
        }
    });
    Ok(())
}

/// Send this device's undelivered records to the other members.
async fn deliver<R: Runtime>(app: &AppHandle<R>, group_id: &str) -> AppResult<()> {
    let state = app.state::<AppState>();
    let g = group(&state, group_id).await?;
    let me = hex::encode(&g.me.0);
    let logs = state.load_polls().await?;
    let Some(log) = logs.get(group_id) else { return Ok(()) };
    let pending: Vec<u64> = log.outbox.clone();
    let records: Vec<Message> = pending
        .iter()
        .filter_map(|seq| log.records.get(&me)?.get(seq).cloned())
        .map(Message::Record)
        .collect();
    if records.is_empty() || g.others().is_empty() {
        return Ok(());
    }
    polls::send(&channel(&state, group_id).await?, &g.others(), &records).await?;
    state.update_polls(group_id, |log| log.outbox.retain(|seq| !pending.contains(seq))).await?;
    Ok(())
}

/// Ask the other members for anything this device is missing.
async fn request_sync<R: Runtime>(app: &AppHandle<R>, group_id: &str) -> AppResult<()> {
    let state = app.state::<AppState>();
    let g = group(&state, group_id).await?;
    if g.others().is_empty() {
        return Ok(());
    }
    let have = state.load_polls().await?.get(group_id).map(GroupLog::have).unwrap_or_default();
    let request = Message::SyncRequest { group_id: group_id.to_string(), have };
    polls::send(&channel(&state, group_id).await?, &g.others(), &[request]).await?;
    Ok(())
}

/// Send anything undelivered and ask the other members for anything missed,
/// now (the background pass does this every few hours).
#[tauri::command]
pub async fn refresh_polls<R: Runtime>(app: AppHandle<R>, group_id: String) -> AppResult<()> {
    deliver(&app, &group_id).await?;
    request_sync(&app, &group_id).await
}

/// Read a session of group messages another member started (the interface
/// does this by itself when it sees one; see `list_pending_sessions`).
#[tauri::command]
pub async fn receive_group_messages<R: Runtime>(app: AppHandle<R>, group_id: String, session_id: Uuid) -> AppResult<()> {
    let state = app.state::<AppState>();
    let g = group(&state, &group_id).await?;
    let (sender, messages) = polls::receive(&channel(&state, &group_id).await?, session_id, &g.members).await?;
    if let Ok(mut done) = state.group_message_sessions.lock() {
        done.insert(session_id);
    }
    let author = hex::encode(&sender.0);
    let me = hex::encode(&g.me.0);
    let mut records = Vec::new();
    let mut resend_after = None;
    for message in messages {
        match message {
            Message::Record(record) if record.group_id == group_id && record.check().is_ok() => records.push(record),
            Message::SyncRequest { group_id: g_id, have } if g_id == group_id => {
                resend_after = Some(have.get(&me).copied().unwrap_or(0));
            }
            _ => tracing::debug!("ignored a group message for another group or malformed"),
        }
    }
    let (new_polls, gap) = state
        .update_polls(&group_id, |log| {
            let mut new_polls = 0;
            for record in records {
                let is_poll = matches!(record.body, RecordBody::Poll { .. });
                if log.insert(&author, record) && is_poll {
                    new_polls += 1;
                }
            }
            let have = log.have().get(&author).copied().unwrap_or(0);
            let highest = log.records.get(&author).and_then(|r| r.keys().next_back().copied()).unwrap_or(0);
            (new_polls, highest > have)
        })
        .await?;
    updated(&app, &group_id, new_polls);

    // They're catching up: resend what they lack of ours, to them alone.
    if let Some(after) = resend_after {
        let own: Vec<Message> = state
            .load_polls()
            .await?
            .get(&group_id)
            .map(|log| log.own_after(&me, after))
            .unwrap_or_default()
            .into_iter()
            .map(Message::Record)
            .collect();
        if !own.is_empty() {
            polls::send(&channel(&state, &group_id).await?, std::slice::from_ref(&sender), &own).await?;
        }
    }
    // A gap in what they sent: ask everyone, which reaches them too.
    if gap {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = request_sync(&app, &group_id).await;
        });
    }
    Ok(())
}

/// How often, while unlocked, undelivered records go out again and the other
/// members are asked for anything missed.
const SYNC_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Deliver and catch up for every group: after each unlock (once the
/// coordination server has had a moment to come up), then every
/// [`SYNC_EVERY`].
pub fn spawn<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let mut unlocked = state.watch_unlocked();
        loop {
            if !*unlocked.borrow_and_update() {
                if unlocked.changed().await.is_err() {
                    return;
                }
                continue;
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
            let groups: Vec<String> =
                state.with_config(|c| Ok(c.group.keys().cloned().collect())).await.unwrap_or_default();
            for group_id in groups {
                let _ = deliver(&app, &group_id).await;
                if let Err(e) = request_sync(&app, &group_id).await {
                    tracing::debug!("couldn't ask the group for missed polls: {}", e.message);
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(SYNC_EVERY) => {}
                _ = unlocked.changed() => {}
            }
        }
    });
}

/// Forget a group's polls (when the group is removed).
pub(crate) async fn forget(state: &AppState, group_id: &str) -> AppResult<()> {
    let _serialized = state.polls_lock.lock().await;
    let mut logs = state.load_polls().await?;
    if logs.remove(group_id).is_some() {
        let plain = zeroize::Zeroizing::new(serde_json::to_vec(&logs).map_err(|e| AppError::new("config", e.to_string()))?);
        let sealed = frost_app_core::sealed::seal(&*state.polls_key().await?, LABEL, &plain)?;
        frost_app_core::keystore::write_atomic(&state.data_dir.join(FILE), &sealed)?;
    }
    Ok(())
}
