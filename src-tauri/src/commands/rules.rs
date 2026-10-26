//! A group's rules for its payments (see `frost_app_core::rules`): which are in
//! effect, and checking a payment against them. Rules are adopted through
//! group polls (`commands::polls::create_poll` with `rules`); the sender's app
//! and each approver's app check every payment against the rules in effect as
//! they see them.

use frost_app_core::polls;
use frost_app_core::rules::{Broken, GroupRules};
use frost_app_core::wallet::verify::VerifiedTransaction;
use serde::Serialize;
use tauri::State;

use crate::commands::payees::payees_in;
use crate::commands::polls::{group, list_polls_in};
use crate::error::AppResult;
use crate::state::AppState;

/// The rules in effect in a group, as this device sees them.
#[derive(Serialize)]
pub struct RulesInEffect {
    pub rules: GroupRules,
    /// The poll that adopted them; None when the group never adopted any.
    pub poll_id: Option<String>,
    pub title: Option<String>,
    /// The group's threshold and size, which the rules are read against.
    pub threshold: u32,
    pub members: u32,
}

pub(crate) async fn in_effect(state: &AppState, group_id: &str) -> AppResult<RulesInEffect> {
    let views = list_polls_in(state, group_id).await?;
    let g = group(state, group_id).await?;
    let poll = polls::rules_in_effect(&views);
    Ok(RulesInEffect {
        rules: poll.and_then(|p| p.rules.clone()).unwrap_or_default(),
        poll_id: poll.map(|p| p.poll_id.clone()),
        title: poll.map(|p| p.title.clone()),
        threshold: g.threshold,
        members: g.members.len() as u32,
    })
}

/// The rules in effect in a group.
#[tauri::command]
pub async fn group_rules(state: State<'_, AppState>, group_id: String) -> AppResult<RulesInEffect> {
    in_effect(&state, &group_id).await
}

/// Check the payments a verified transaction makes, asked of `asked` members,
/// against the rules in effect. A transfer within the group makes none.
pub(crate) async fn check(
    state: &AppState,
    group_id: &str,
    tx: &VerifiedTransaction,
    asked: u32,
) -> AppResult<(RulesInEffect, Result<(), Broken>)> {
    let rules = in_effect(state, group_id).await?;
    let trusted: Vec<Vec<String>> =
        payees_in(state, group_id).await?.into_iter().filter(|p| p.payee.trusted).map(|p| p.forms).collect();
    let payments: Vec<(&str, u64)> = tx.payments.iter().map(|p| (p.address.as_str(), p.value_zatoshis)).collect();
    let verdict = rules
        .rules
        .check(&payments, asked, rules.threshold, |address| trusted.iter().any(|forms| forms.iter().any(|f| f == address)));
    Ok((rules, verdict))
}
