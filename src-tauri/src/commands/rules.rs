//! A group's rules for its payments (see `frost_app_core::rules`): which are in
//! effect, and checking a payment against them. Rules are adopted through
//! group polls (`commands::polls::create_poll` with `rules`); the sender's app
//! and each approver's app check every payment against the rules in effect as
//! they see them.

use frost_app_core::polls;
use frost_app_core::rules::{Broken, GroupRules, NotAutomatic, SpendingLimit};
use frost_app_core::wallet::{self, verify::VerifiedTransaction};
use serde::Serialize;
use tauri::State;

use crate::commands::payees::payees_in;
use crate::commands::polls::{group, list_polls_in};
use crate::commands::wallet::{group_wallet_ctx, unix_now};
use crate::error::{AppError, AppResult};
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

/// Whether a payment poll's payments can be sent now, as this device sees it:
/// under a delay rule, once the delay has passed since this device learned of
/// it and nobody chose Don't pay; otherwise once the group agreed to pay.
pub(crate) fn poll_permits(rules: &GroupRules, poll: &polls::PollView, now: u64) -> Result<(), String> {
    let total = poll.payments.iter().map(|p| p.amount_zatoshis).sum();
    match rules.delay_for(total) {
        Some(delay) => {
            let vetoes = poll.tally.get(1).copied().unwrap_or(0);
            frost_app_core::rules::announcement_ready(poll.announced_at, vetoes, delay, now).map_err(|e| match e {
                frost_app_core::rules::NotYet::Vetoed => "a member vetoed this payment by choosing Don't pay".to_string(),
                frost_app_core::rules::NotYet::TooSoon { from } => {
                    format!("the group's rules make this payment wait: it can be sent in {}", wait(from.saturating_sub(now)))
                }
            })
        }
        None if poll.agreed == Some(0) => Ok(()),
        None => Err("the group hasn't agreed to pay".into()),
    }
}

/// A wait in seconds as people say it: "3 hours", "2 days".
fn wait(secs: u64) -> String {
    let (n, unit) = match secs {
        s if s >= 2 * 86_400 => (s / 86_400, "day"),
        s if s >= 2 * 3_600 => (s / 3_600, "hour"),
        s => (s.div_ceil(60).max(1), "minute"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// The rules in effect in a group.
#[tauri::command]
pub async fn group_rules(state: State<'_, AppState>, group_id: String) -> AppResult<RulesInEffect> {
    in_effect(&state, &group_id).await
}

/// How long an automatic approval this device made counts on its own, before
/// the payment is in the wallet's history or has expired (transactions expire
/// within the hour).
const IN_FLIGHT_SECS: u64 = 70 * 60;

/// How recently the wallet must have synced for its history to count against
/// a spending limit.
const FRESH_SYNC_MS: i64 = 15 * 60 * 1000;

/// What the wallet sent in the last `per_secs`, payments and fees, as far as
/// this device can say: its history since the block that long ago (by block
/// spacing, not block times, which a chain needn't keep current), plus what it
/// approved automatically that may not be in it yet (counted twice once it
/// is, never too little). None when the wallet isn't synced recently enough
/// to say.
pub(crate) async fn spent_in_period(state: &AppState, group_id: &str, per_secs: u64) -> AppResult<Option<u64>> {
    let Some(tip) = state.sync_service.status(group_id).and_then(|s| {
        let fresh = s.last_synced_ms.is_some_and(|at| wallet::now_ms() - at < FRESH_SYNC_MS)
            && wallet::is_caught_up(s.scanned_height, s.tip_height);
        fresh.then_some(s.tip_height)
    }) else {
        return Ok(None);
    };
    let (network, _, _) = group_wallet_ctx(state, group_id).await?;
    let wid = state.wallet_dir_id(group_id).await?;
    let db_key = state.wallet_db_key(group_id).await?;
    let now = unix_now();
    let since = wallet::height_before(network, tip, per_secs);
    let sent: u64 = wallet::wallet_history(&state.data_dir, &wid, network, db_key.as_ref())?
        .iter()
        .filter(|t| t.direction == "send" && t.block_height.is_none_or(|h| h >= since))
        .map(|t| t.amount_zatoshis + t.fee_zatoshis.unwrap_or(0))
        .sum();
    let in_flight: u64 = state
        .auto_approvals
        .lock()
        .await
        .get(group_id)
        .into_iter()
        .flatten()
        .filter(|(at, _)| now.saturating_sub(*at) < IN_FLIGHT_SECS)
        .map(|(_, cost)| cost)
        .sum();
    Ok(Some(sent + in_flight))
}

/// Whether this device approves a verified transaction by itself, within the
/// group's spending limit. None when it doesn't approve automatically in this
/// group (the member didn't turn it on, or the group has no limit).
#[derive(Serialize)]
pub(crate) struct AutoDecision {
    pub approved: bool,
    /// Why not, when it isn't.
    pub reason: Option<String>,
    /// What the wallet sent in the period before this, what this costs, and
    /// the limit.
    pub spent: u64,
    pub cost: u64,
    pub limit: SpendingLimit,
}

pub(crate) async fn automatic_approval(state: &AppState, group_id: &str, tx: &VerifiedTransaction) -> Option<AutoDecision> {
    let on = state.load_private().await.ok()?.auto_approve.contains(group_id);
    let rules = in_effect(state, group_id).await.ok()?;
    let limit = rules.rules.limit.filter(|_| on)?;
    let cost_of = |tx: &VerifiedTransaction| tx.payments.iter().map(|p| p.value_zatoshis).sum::<u64>() + tx.fee_zatoshis;
    let Ok(Some(spent)) = spent_in_period(state, group_id, limit.per_secs).await else {
        let reason = "the wallet isn't synced, so its spending in the period isn't known".to_string();
        return Some(AutoDecision { approved: false, reason: Some(reason), spent: 0, cost: cost_of(tx), limit });
    };
    let trusted: Vec<Vec<String>> = payees_in(state, group_id)
        .await
        .ok()?
        .into_iter()
        .filter(|p| p.payee.trusted)
        .map(|p| p.forms)
        .collect();
    let is_trusted = |address: &str| trusted.iter().any(|forms| forms.iter().any(|f| f == address));
    let payments: Vec<(&str, u64)> = tx.payments.iter().map(|p| (p.address.as_str(), p.value_zatoshis)).collect();
    Some(match rules.rules.may_approve_automatically(&payments, tx.fee_zatoshis, rules.threshold, is_trusted, spent) {
        Ok(cost) => AutoDecision { approved: true, reason: None, spent, cost, limit },
        Err(why) => {
            let cost = match &why {
                NotAutomatic::OverLimit { cost, .. } => *cost,
                _ => cost_of(tx),
            };
            AutoDecision { approved: false, reason: Some(why.to_string()), spent, cost, limit }
        }
    })
}

/// Whether this device approves payments automatically within a group's
/// spending limit, what the wallet sent in the limit's period (None if it
/// can't say yet), and the limit.
#[derive(Serialize)]
pub struct AutoApprovalStatus {
    pub on: bool,
    pub spent: Option<u64>,
    pub limit: Option<SpendingLimit>,
}

#[tauri::command]
pub async fn auto_approval_status(state: State<'_, AppState>, group_id: String) -> AppResult<AutoApprovalStatus> {
    let on = state.load_private().await?.auto_approve.contains(&group_id);
    let limit = in_effect(&state, &group_id).await?.rules.limit;
    let spent = match limit {
        Some(l) => spent_in_period(&state, &group_id, l.per_secs).await.unwrap_or(None),
        None => None,
    };
    Ok(AutoApprovalStatus { on, spent, limit })
}

/// Turn this device's automatic approvals within a group's spending limit on
/// or off.
#[tauri::command]
pub async fn set_auto_approve(state: State<'_, AppState>, group_id: String, on: bool) -> AppResult<()> {
    if on && in_effect(&state, &group_id).await?.rules.limit.is_none() {
        return Err(AppError::new("rules", "the group has no spending limit to approve within"));
    }
    state
        .update_private(|p| {
            if on {
                p.auto_approve.insert(group_id.clone());
            } else {
                p.auto_approve.remove(&group_id);
            }
        })
        .await
}

/// Note that this device approved `cost` automatically in a group just now.
pub(crate) async fn record_automatic_approval(state: &AppState, group_id: &str, cost: u64) {
    let now = unix_now();
    let mut ledger = state.auto_approvals.lock().await;
    let entries = ledger.entry(group_id.to_string()).or_default();
    entries.retain(|(at, _)| now.saturating_sub(*at) < IN_FLIGHT_SECS);
    entries.push((now, cost));
}

/// Check the payments a verified transaction makes, asked of `asked` members,
/// against the rules in effect. A transfer within the group makes none.
/// `announced`: it carries out a payment poll whose delay has passed without a
/// veto, as this device sees it.
pub(crate) async fn check(
    state: &AppState,
    group_id: &str,
    tx: &VerifiedTransaction,
    asked: u32,
    announced: bool,
) -> AppResult<(RulesInEffect, Result<(), Broken>)> {
    let rules = in_effect(state, group_id).await?;
    let trusted: Vec<Vec<String>> =
        payees_in(state, group_id).await?.into_iter().filter(|p| p.payee.trusted).map(|p| p.forms).collect();
    let payments: Vec<(&str, u64)> = tx.payments.iter().map(|p| (p.address.as_str(), p.value_zatoshis)).collect();
    let is_trusted = |address: &str| trusted.iter().any(|forms| forms.iter().any(|f| f == address));
    let verdict = rules.rules.check(&payments, asked, rules.threshold, is_trusted, announced);
    Ok((rules, verdict))
}

#[cfg(test)]
mod tests {
    use super::*;
    use frost_app_core::polls::{PollView, ProposedPayment};
    use frost_app_core::rules::Delay;

    const ZEC: u64 = 100_000_000;

    fn poll(amount_zatoshis: u64, tally: [u32; 2], agreed: Option<u32>) -> PollView {
        PollView {
            group_id: "g".into(),
            poll_id: "p".into(),
            author: "aa".into(),
            title: "Pay the auditor?".into(),
            details: String::new(),
            options: vec!["Pay".into(), "Don't pay".into()],
            created_at: 1_000,
            announced_at: 1_000,
            closes_at: 1_000_000,
            closed: false,
            votes: Default::default(),
            tally: tally.to_vec(),
            my_vote: None,
            members: 3,
            threshold: 2,
            agreed,
            community: None,
            payments: vec![ProposedPayment { recipient: "u1auditor".into(), amount_zatoshis, memo: None }],
            rules: None,
            paid_txid: None,
        }
    }

    #[test]
    fn an_announced_payment_waits_out_its_delay_and_any_veto_stops_it() {
        let rules = GroupRules { delay: Some(Delay { from_zatoshis: 50 * ZEC, secs: 3_600 }), ..GroupRules::default() };
        let large = poll(60 * ZEC, [0, 0], None);
        let early = poll_permits(&rules, &large, 1_000 + 1_800).unwrap_err();
        assert_eq!(early, "the group's rules make this payment wait: it can be sent in 30 minutes");
        assert_eq!(poll_permits(&rules, &large, 1_000 + 3_600), Ok(()), "no agreement needed, only no veto");
        let vetoed = poll(60 * ZEC, [2, 1], Some(0));
        assert_eq!(poll_permits(&rules, &vetoed, 1_000_000).unwrap_err(), "a member vetoed this payment by choosing Don't pay");
        // Below the rule's amount, a payment poll needs the group to agree.
        assert_eq!(poll_permits(&rules, &poll(ZEC, [1, 0], None), 2_000).unwrap_err(), "the group hasn't agreed to pay");
        assert_eq!(poll_permits(&rules, &poll(ZEC, [2, 0], Some(0)), 2_000), Ok(()));
    }
}
