//! A group's shared payees and payment reasons (see `frost_app_core::payees`).
//! They travel in the group's records alongside its polls, so storage, delivery
//! and catching up are `commands::polls`'.

use frost_app_core::payees::{self, PaymentReason};
use frost_app_core::polls::{self, RecordBody};
use frost_app_core::wallet::verify::address_forms;
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State};
use uuid::Uuid;

use crate::commands::polls::{group, write};
use crate::commands::wallet::configured_network;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// A payee as the interface shows it.
#[derive(Serialize)]
pub struct PayeeDto {
    #[serde(flatten)]
    pub payee: payees::Payee,
    /// The address as given and as a verified payment to it reads
    /// ([`address_forms`]): a payment to any of them is to this payee.
    pub forms: Vec<String>,
    /// This device's member added it, so can change or remove it.
    pub mine: bool,
}

pub(crate) async fn payees_in(state: &AppState, group_id: &str) -> AppResult<Vec<PayeeDto>> {
    let network = configured_network(&state.load_settings());
    let logs = state.load_polls().await?;
    let Some(log) = logs.get(group_id) else { return Ok(Vec::new()) };
    let g = group(state, group_id).await?;
    let me = hex::encode(&g.me.0);
    // A payee saved on another network isn't one here.
    Ok(payees::payees(log, &g.member_hexes(), g.threshold)
        .into_iter()
        .filter_map(|payee| {
            let forms = address_forms(network, &payee.address).ok()?;
            Some(PayeeDto { mine: payee.author == me, payee, forms })
        })
        .collect())
}

/// The group's payees on the configured network, by name.
#[tauri::command]
pub async fn list_payees(state: State<'_, AppState>, group_id: String) -> AppResult<Vec<PayeeDto>> {
    payees_in(&state, &group_id).await
}

/// Add a payee to the group, or change one this member added. Returns its id.
#[tauri::command]
pub async fn save_payee<R: Runtime>(
    app: AppHandle<R>,
    group_id: String,
    payee_id: Option<String>,
    name: String,
    address: String,
) -> AppResult<String> {
    let state = app.state::<AppState>();
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::new("config", "give the payee a name"));
    }
    if name.chars().count() > polls::MAX_PAYEE_NAME {
        return Err(AppError::new("config", format!("a payee's name can be up to {} characters", polls::MAX_PAYEE_NAME)));
    }
    let network = configured_network(&state.load_settings());
    let forms = address_forms(network, &address)?;
    let existing = payees_in(&state, &group_id).await?;
    if let Some(id) = &payee_id {
        if !existing.iter().any(|p| p.payee.payee_id == *id && p.mine) {
            return Err(AppError::new("config", "only the member who added a payee can change it"));
        }
    }
    // One name per address and one address per name, so a name an approver
    // sees can't stand for two addresses.
    for other in existing.iter().filter(|p| !(p.mine && Some(&p.payee.payee_id) == payee_id.as_ref())) {
        if other.forms.iter().any(|f| forms.contains(f)) {
            return Err(AppError::new("config", format!("this address is already a payee: {}", other.payee.name)));
        }
        if other.payee.name.to_lowercase() == name.to_lowercase() {
            return Err(AppError::new("config", format!("there's already a payee called {}", other.payee.name)));
        }
    }
    let payee_id = payee_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let address = forms[0].clone();
    write(&app, &group_id, RecordBody::Payee { payee_id: payee_id.clone(), name, address }).await?;
    Ok(payee_id)
}

/// Remove a payee this member added.
#[tauri::command]
pub async fn remove_payee<R: Runtime>(app: AppHandle<R>, group_id: String, payee_id: String) -> AppResult<()> {
    let state = app.state::<AppState>();
    if !payees_in(&state, &group_id).await?.iter().any(|p| p.payee.payee_id == payee_id && p.mine) {
        return Err(AppError::new("config", "only the member who added a payee can remove it"));
    }
    write(&app, &group_id, RecordBody::PayeeRemoved { payee_id }).await
}

/// Trust a payee another member added, at its current address, or stop
/// trusting it. A payee is trusted once as many members trust it as a payment
/// needs approvals.
#[tauri::command]
pub async fn trust_payee<R: Runtime>(
    app: AppHandle<R>,
    group_id: String,
    owner: String,
    payee_id: String,
    trusted: bool,
) -> AppResult<()> {
    let state = app.state::<AppState>();
    let payee = payees_in(&state, &group_id)
        .await?
        .into_iter()
        .find(|p| p.payee.author == owner && p.payee.payee_id == payee_id)
        .ok_or_else(|| AppError::new("config", "payee not found"))?;
    if payee.mine {
        return Err(AppError::new("config", "you trust the payees you add"));
    }
    let address = payee.payee.address;
    write(&app, &group_id, RecordBody::PayeeTrust { owner, payee_id, address, trusted }).await
}

/// What the group's members say its transactions were for.
#[tauri::command]
pub async fn list_payment_reasons(state: State<'_, AppState>, group_id: String) -> AppResult<Vec<PaymentReason>> {
    let logs = state.load_polls().await?;
    let Some(log) = logs.get(&group_id) else { return Ok(Vec::new()) };
    let g = group(&state, &group_id).await?;
    Ok(payees::reasons(log, &g.member_hexes()))
}

/// Tell the group what a transaction was for, or (with empty text) take back
/// what this member said.
#[tauri::command]
pub async fn set_payment_reason<R: Runtime>(app: AppHandle<R>, group_id: String, txid: String, text: String) -> AppResult<()> {
    write(&app, &group_id, RecordBody::PaymentReason { txid: txid.trim().to_lowercase(), text: payment_reason(&text)?.unwrap_or_default() })
        .await
}

/// A payment reason as it's kept: trimmed, None if empty. Err if too long.
pub(crate) fn payment_reason(text: &str) -> AppResult<Option<String>> {
    let text = text.trim();
    if text.chars().count() > polls::MAX_REASON {
        return Err(AppError::new("config", format!("a reason can be up to {} characters", polls::MAX_REASON)));
    }
    Ok((!text.is_empty()).then(|| text.to_string()))
}
