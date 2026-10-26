//! Payments a member schedules to repeat (see `frost_app_core::schedules`).
//! Schedules travel in the group's records alongside its polls, like payees.
//! When one is due, the author's app asks its approvers like any other payment
//! (the interface's scheduler, through `wallet_send`) and records the
//! transaction it went in, so the others see it was sent.

use frost_app_core::polls::{ProposedPayment, RecordBody};
use frost_app_core::schedules::{self, Every, Schedule};
use frost_app_core::wallet::verify::address_forms;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, State};
use uuid::Uuid;

use crate::commands::payees::payment_reason;
use crate::commands::polls::{group, proposed_payments, write};
use crate::commands::rules::check_payments;
use crate::commands::wallet::{configured_network, unix_now};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// How far ahead a schedule's first payment can be.
const MAX_FIRST_DUE_SECS: u64 = 366 * 86_400;

/// A schedule as the interface shows it.
#[derive(Serialize)]
pub struct ScheduleDto {
    pub group_id: String,
    #[serde(flatten)]
    pub schedule: Schedule,
    /// This device's member made it, so its app starts the payments and it
    /// can stop it.
    pub mine: bool,
}

async fn schedules_in(state: &AppState, group_id: &str) -> AppResult<Vec<ScheduleDto>> {
    let network = configured_network(&state.load_settings());
    let logs = state.load_polls().await?;
    let Some(log) = logs.get(group_id) else {
        return Ok(Vec::new());
    };
    let g = group(state, group_id).await?;
    let me = hex::encode(&g.me.0);
    // A schedule paying addresses on another network isn't one here.
    Ok(schedules::schedules(log, &g.member_hexes(), unix_now())
        .into_iter()
        .filter(|s| s.payments.iter().all(|p| address_forms(network, &p.recipient).is_ok()))
        .map(|schedule| ScheduleDto { group_id: group_id.to_string(), mine: schedule.author == me, schedule })
        .collect())
}

async fn mine(state: &AppState, group_id: &str, schedule_id: &str) -> AppResult<ScheduleDto> {
    schedules_in(state, group_id)
        .await?
        .into_iter()
        .find(|s| s.mine && s.schedule.schedule_id == schedule_id)
        .ok_or_else(|| AppError::new("config", "only the member who scheduled a payment can change it"))
}

/// The group's scheduled payments on the configured network.
#[tauri::command]
pub async fn list_schedules(state: State<'_, AppState>, group_id: String) -> AppResult<Vec<ScheduleDto>> {
    schedules_in(&state, &group_id).await
}

/// This member's scheduled payments that are due now, in every group: the
/// payments this app starts.
#[tauri::command]
pub async fn due_schedules(state: State<'_, AppState>) -> AppResult<Vec<ScheduleDto>> {
    let group_ids: Vec<String> = state.with_config(|c| Ok(c.group.keys().cloned().collect())).await?;
    let mut due = Vec::new();
    for group_id in group_ids {
        match schedules_in(&state, &group_id).await {
            Ok(schedules) => due.extend(schedules.into_iter().filter(|s| s.mine && s.schedule.due_now.is_some())),
            Err(e) => tracing::info!("couldn't read a group's schedules: {}", e.message),
        }
    }
    Ok(due)
}

#[derive(Deserialize)]
pub struct CreateScheduleArgs {
    pub group_id: String,
    pub payments: Vec<ProposedPayment>,
    #[serde(default)]
    pub reason: String,
    pub every: Every,
    /// The first payment's date (unix seconds).
    pub first_due: u64,
    /// The members asked to approve each payment, this one among them.
    pub signers: Vec<String>,
}

/// Schedule payments to repeat. Returns the schedule's id. Refused when the
/// group's rules, as they are now, would refuse the payments.
#[tauri::command]
pub async fn create_schedule<R: Runtime>(app: AppHandle<R>, args: CreateScheduleArgs) -> AppResult<String> {
    let state = app.state::<AppState>();
    let payments = proposed_payments(&state, args.payments)?;
    if payments.is_empty() {
        return Err(AppError::new("config", "add a payment to schedule"));
    }
    let reason = payment_reason(&args.reason)?.unwrap_or_default();
    if !args.every.is_valid() {
        return Err(AppError::new("config", "choose how often the payment repeats"));
    }
    let now = unix_now();
    if args.first_due + 86_400 < now || args.first_due > now + MAX_FIRST_DUE_SECS {
        return Err(AppError::new("config", "the first payment must be between today and a year from now"));
    }
    let g = group(&state, &args.group_id).await?;
    let me = hex::encode(&g.me.0);
    let members = g.member_hexes();
    let mut signers: Vec<String> = Vec::new();
    for s in args.signers.iter().map(|s| s.to_lowercase()) {
        if !members.contains(&s) {
            return Err(AppError::new("config", "everyone asked to approve must be in the group"));
        }
        if !signers.contains(&s) {
            signers.push(s);
        }
    }
    if !signers.contains(&me) {
        return Err(AppError::new("config", "you approve the payments you schedule"));
    }
    if signers.len() < g.threshold as usize {
        return Err(AppError::new("config", format!("ask at least {} members to approve", g.threshold)));
    }
    let network = configured_network(&state.load_settings());
    let read_as: Vec<String> = payments
        .iter()
        .map(|p| address_forms(network, &p.recipient).map(|f| f.last().cloned().unwrap_or_default()))
        .collect::<Result<_, _>>()?;
    let pairs: Vec<(&str, u64)> = read_as.iter().map(String::as_str).zip(payments.iter().map(|p| p.amount_zatoshis)).collect();
    if let (_, Err(broken)) = check_payments(&state, &args.group_id, &pairs, signers.len() as u32, false).await? {
        return Err(AppError::new("rules", broken.to_string()));
    }
    let schedule_id = Uuid::new_v4().to_string();
    let body =
        RecordBody::Schedule { schedule_id: schedule_id.clone(), payments, reason, every: args.every, first_due: args.first_due, signers };
    write(&app, &args.group_id, body).await?;
    Ok(schedule_id)
}

/// Stop a schedule this member made: no more of its payments are started.
#[tauri::command]
pub async fn stop_schedule<R: Runtime>(app: AppHandle<R>, group_id: String, schedule_id: String) -> AppResult<()> {
    let state = app.state::<AppState>();
    if mine(&state, &group_id, &schedule_id).await?.schedule.stopped {
        return Ok(());
    }
    write(&app, &group_id, RecordBody::ScheduleStopped { schedule_id }).await
}

/// Tell the group a scheduled payment was sent, so it isn't started again.
#[tauri::command]
pub async fn record_schedule_run<R: Runtime>(
    app: AppHandle<R>,
    group_id: String,
    schedule_id: String,
    due: u64,
    txid: String,
) -> AppResult<()> {
    let state = app.state::<AppState>();
    let txid = txid.trim().to_lowercase();
    let s = mine(&state, &group_id, &schedule_id).await?.schedule;
    if !s.every.falls_on(s.first_due, due) || due > unix_now() {
        return Err(AppError::new("config", "that isn't a date this payment was due"));
    }
    match s.runs.get(&due) {
        Some(sent) if *sent == txid => Ok(()),
        Some(_) => Err(AppError::new("config", "this payment was already sent")),
        None if txid.len() == 64 && txid.bytes().all(|b| b.is_ascii_hexdigit()) => {
            write(&app, &group_id, RecordBody::ScheduleRun { schedule_id, due, txid }).await
        }
        None => Err(AppError::new("config", "not a transaction id")),
    }
}
