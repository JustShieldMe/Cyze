//! Rules a group adopts for its payments, which each member's app applies when
//! deciding whether to sign.
//!
//! Zcash has no contracts for shielded funds, so nothing on chain enforces
//! these. An app refuses to sign a payment that breaks them, so one goes
//! through only if as many members as a payment needs all run apps that skip
//! the rules: the same trust the wallet itself rests on. They guard against a
//! careless approval, a swapped address or a single compromised device, not
//! against that many members acting together.
//!
//! The group adopts rules the way it agrees on anything: a poll proposes a
//! complete set ([`crate::polls::RecordBody::Poll`] with `rules`), and the
//! latest one the group agreed to adopt is in effect. Changing the rules takes
//! as many members as spending does.
//!
//! A delay with veto works through payment polls: a large payment has to be
//! announced in one, and can be sent once the delay has passed with no member
//! choosing "Don't pay". Each member's app measures the delay from when it
//! received the announcement, so a backdated one gains nothing and everyone
//! has the whole delay to object.
//!
//! A spending limit lets members have their apps approve routine payments by
//! themselves: to trusted payees, up to an amount in a period, counting
//! everything the wallet sent in it ([`GroupRules::may_approve_automatically`]).
//! Each member turns it on for their own app; the others still approve by
//! hand. An app that approves automatically still verifies the transaction
//! and checks every other rule first.

use serde::{Deserialize, Serialize};

/// The most approvals a rule can ask for.
pub const MAX_APPROVALS: u32 = 100;
/// The shortest and longest delay a rule can set before a large payment.
pub const MIN_DELAY_SECS: u64 = 60 * 60;
pub const MAX_DELAY_SECS: u64 = 30 * 24 * 60 * 60;
/// The shortest and longest period a spending limit can count over.
pub const MIN_LIMIT_PERIOD_SECS: u64 = 60 * 60;
pub const MAX_LIMIT_PERIOD_SECS: u64 = 90 * 24 * 60 * 60;

/// A group's rules for its payments. The default is none: any payment as many
/// members approve as the group needs goes through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRules {
    /// Pay only trusted payees: those as many members trust as a payment
    /// needs approvals ([`crate::payees`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trusted_payees_only: bool,
    /// Larger payments need more approvals than the group's threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub large: Option<LargePayments>,
    /// Larger payments are announced first, and wait for vetoes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<Delay>,
    /// Members' apps may approve routine payments automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<SpendingLimit>,
}

/// "A member's app may approve payments to trusted payees by itself while the
/// wallet has sent less than `up_to_zatoshis` in the last `per_secs`, this
/// payment and its fee included."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendingLimit {
    pub up_to_zatoshis: u64,
    pub per_secs: u64,
}

/// Why an app doesn't approve a payment automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotAutomatic {
    /// The group has no spending limit.
    NoLimit,
    /// It makes no payments (a transfer within the group).
    NoPayments,
    /// It pays an address that isn't a trusted payee.
    Untrusted,
    /// It's large enough that the rules need more approvals, or a wait.
    Large,
    /// Its fee is higher than a transaction like it needs.
    Fee,
    /// It would take the wallet's spending in the period over the limit.
    OverLimit { spent: u64, cost: u64, limit: SpendingLimit },
}

impl std::fmt::Display for NotAutomatic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotAutomatic::NoLimit => write!(f, "the group has no spending limit"),
            NotAutomatic::NoPayments => write!(f, "it makes no payments"),
            NotAutomatic::Untrusted => write!(f, "it pays an address that isn't a trusted payee"),
            NotAutomatic::Large => write!(f, "it's large enough that the rules need more approvals or a wait"),
            NotAutomatic::Fee => write!(f, "its fee is higher than usual"),
            NotAutomatic::OverLimit { spent, cost, limit } => write!(
                f,
                "the wallet has sent {} in the last {}, and this would take it to {}, over the limit of {}",
                coins(*spent),
                duration(limit.per_secs),
                coins(spent + cost),
                coins(limit.up_to_zatoshis)
            ),
        }
    }
}

/// The most a transaction making `payments` payments should pay in fees for
/// an app to approve it by itself: ZIP 317 charges 5,000 zatoshis per action,
/// so this allows a few notes spent per payment.
pub fn usual_fee(payments: usize) -> u64 {
    50_000 + 10_000 * (payments as u64 + 1)
}

/// "Payments of at least `from_zatoshis` together are announced in a payment
/// poll at least `secs` before they're sent, and any member can veto them by
/// choosing Don't pay."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delay {
    pub from_zatoshis: u64,
    pub secs: u64,
}

/// "Payments of at least `from_zatoshis` together need `approvals` approvals."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LargePayments {
    pub from_zatoshis: u64,
    pub approvals: u32,
}

/// How a payment breaks a group's rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Broken {
    /// It pays an address that isn't a trusted payee.
    Untrusted { address: String },
    /// It's large, and asks fewer members to approve than the rules need.
    TooFewApprovals { needed: u32, asked: u32, from_zatoshis: u64 },
    /// It's large, and wasn't announced (in a payment poll whose delay has
    /// passed without a veto).
    NotAnnounced { delay: Delay },
}

/// Why an announced payment can't be sent yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotYet {
    /// Its delay hasn't passed (on this device's clock): it can go at `from`.
    TooSoon { from: u64 },
    /// A member chose Don't pay.
    Vetoed,
}

impl std::fmt::Display for NotYet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotYet::TooSoon { from } => write!(f, "its announcement's delay hasn't passed: it can go from {from} (unix time)"),
            NotYet::Vetoed => write!(f, "a member vetoed it by choosing Don't pay"),
        }
    }
}

/// Whether a payment announced at `announced_at` (as this device saw it), with
/// `vetoes` members currently choosing Don't pay, can be sent at `now`.
pub fn announcement_ready(announced_at: u64, vetoes: u32, delay: Delay, now: u64) -> Result<(), NotYet> {
    if vetoes > 0 {
        return Err(NotYet::Vetoed);
    }
    let from = announced_at.saturating_add(delay.secs);
    if now < from {
        return Err(NotYet::TooSoon { from });
    }
    Ok(())
}

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Broken::Untrusted { address } => {
                write!(f, "the group's rules allow payments only to trusted payees, and this pays {address}")
            }
            Broken::TooFewApprovals { needed, asked, from_zatoshis } => write!(
                f,
                "the group's rules need {needed} approvals for payments of {} or more, and this asks {asked}",
                coins(*from_zatoshis)
            ),
            Broken::NotAnnounced { delay } => write!(
                f,
                "the group's rules need payments of {} or more announced in a payment poll {} before they're sent",
                coins(delay.from_zatoshis),
                duration(delay.secs)
            ),
        }
    }
}

impl GroupRules {
    /// Whether these are the default: no rules.
    pub fn is_empty(&self) -> bool {
        *self == GroupRules::default()
    }

    /// How many approvals payments totalling `total_zatoshis` need, with the
    /// group needing `threshold`.
    pub fn approvals_needed(&self, total_zatoshis: u64, threshold: u32) -> u32 {
        match self.large {
            Some(large) if total_zatoshis >= large.from_zatoshis => large.approvals.max(threshold),
            _ => threshold,
        }
    }

    /// Whether a member's app may approve a transaction by itself: its
    /// `payments` (address, amount) all to trusted payees, routine (no extra
    /// approvals or wait), a usual `fee`, and with the wallet having sent
    /// `spent_in_period` in the limit's period, within the limit. Returns what
    /// it costs against the limit (payments and fee).
    pub fn may_approve_automatically(
        &self,
        payments: &[(&str, u64)],
        fee: u64,
        threshold: u32,
        is_trusted: impl Fn(&str) -> bool,
        spent_in_period: u64,
    ) -> Result<u64, NotAutomatic> {
        let limit = self.limit.ok_or(NotAutomatic::NoLimit)?;
        if payments.is_empty() {
            return Err(NotAutomatic::NoPayments);
        }
        if !payments.iter().all(|(address, _)| is_trusted(address)) {
            return Err(NotAutomatic::Untrusted);
        }
        let total: u64 = payments.iter().map(|(_, amount)| amount).sum();
        if self.approvals_needed(total, threshold) > threshold || self.delay_for(total).is_some() {
            return Err(NotAutomatic::Large);
        }
        if fee > usual_fee(payments.len()) {
            return Err(NotAutomatic::Fee);
        }
        let cost = total.saturating_add(fee);
        if spent_in_period.saturating_add(cost) > limit.up_to_zatoshis {
            return Err(NotAutomatic::OverLimit { spent: spent_in_period, cost, limit });
        }
        Ok(cost)
    }

    /// The delay payments totalling `total_zatoshis` wait for, if any.
    pub fn delay_for(&self, total_zatoshis: u64) -> Option<Delay> {
        self.delay.filter(|d| total_zatoshis >= d.from_zatoshis)
    }

    /// Check a transaction's `payments` (address, amount), asked of `asked`
    /// members, against these rules. `is_trusted` says whether an address is a
    /// trusted payee's; `announced`, whether it carries out a payment poll
    /// whose delay has passed without a veto ([`announcement_ready`]).
    pub fn check(
        &self,
        payments: &[(&str, u64)],
        asked: u32,
        threshold: u32,
        is_trusted: impl Fn(&str) -> bool,
        announced: bool,
    ) -> Result<(), Broken> {
        if self.trusted_payees_only {
            if let Some((address, _)) = payments.iter().find(|(address, _)| !is_trusted(address)) {
                return Err(Broken::Untrusted { address: address.to_string() });
            }
        }
        let total = payments.iter().map(|(_, amount)| amount).sum();
        if let Some(delay) = self.delay_for(total) {
            if !announced {
                return Err(Broken::NotAnnounced { delay });
            }
        }
        let needed = self.approvals_needed(total, threshold);
        if asked < needed {
            let from_zatoshis = self.large.map_or(0, |l| l.from_zatoshis);
            return Err(Broken::TooFewApprovals { needed, asked, from_zatoshis });
        }
        Ok(())
    }

    /// Whether the rules are well formed, so malformed ones are never adopted.
    pub fn is_valid(&self) -> bool {
        self.large.is_none_or(|l| l.from_zatoshis > 0 && (2..=MAX_APPROVALS).contains(&l.approvals))
            && self.delay.is_none_or(|d| d.from_zatoshis > 0 && (MIN_DELAY_SECS..=MAX_DELAY_SECS).contains(&d.secs))
            && self.limit.is_none_or(|l| {
                l.up_to_zatoshis > 0 && (MIN_LIMIT_PERIOD_SECS..=MAX_LIMIT_PERIOD_SECS).contains(&l.per_secs)
            })
    }
}

/// A duration in seconds, in the largest whole unit: "2 days", "36 hours".
fn duration(secs: u64) -> String {
    let (n, unit) = if secs.is_multiple_of(86_400) { (secs / 86_400, "day") } else { (secs.div_ceil(3_600), "hour") };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// An amount in zatoshis as coins, trailing zeros trimmed.
fn coins(zatoshis: u64) -> String {
    let whole = format!("{}.{:08}", zatoshis / 100_000_000, zatoshis % 100_000_000);
    whole.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZEC: u64 = 100_000_000;

    #[test]
    fn no_rules_need_only_the_threshold() {
        let rules = GroupRules::default();
        assert!(rules.is_empty());
        assert_eq!(rules.check(&[("u1anyone", 1_000 * ZEC)], 2, 2, |_| false, false), Ok(()));
        assert_eq!(
            rules.check(&[("u1anyone", ZEC)], 1, 2, |_| false, false),
            Err(Broken::TooFewApprovals { needed: 2, asked: 1, from_zatoshis: 0 })
        );
    }

    #[test]
    fn only_trusted_payees_are_paid_when_the_rules_say_so() {
        let rules = GroupRules { trusted_payees_only: true, ..GroupRules::default() };
        let trusted = |a: &str| a == "u1auditor";
        assert_eq!(rules.check(&[("u1auditor", ZEC)], 2, 2, trusted, false), Ok(()));
        assert_eq!(
            rules.check(&[("u1auditor", ZEC), ("u1stranger", 1)], 2, 2, trusted, false),
            Err(Broken::Untrusted { address: "u1stranger".into() })
        );
    }

    #[test]
    fn large_payments_together_need_more_approvals() {
        let rules = GroupRules { large: Some(LargePayments { from_zatoshis: 100 * ZEC, approvals: 4 }), ..GroupRules::default() };
        assert_eq!(rules.approvals_needed(99 * ZEC, 3), 3);
        assert_eq!(rules.approvals_needed(100 * ZEC, 3), 4);
        // Several smaller payments in one transaction count together.
        let split = [("u1a", 60 * ZEC), ("u1b", 40 * ZEC)];
        assert_eq!(
            rules.check(&split, 3, 3, |_| true, false),
            Err(Broken::TooFewApprovals { needed: 4, asked: 3, from_zatoshis: 100 * ZEC })
        );
        assert_eq!(rules.check(&split, 4, 3, |_| true, false), Ok(()));
        let broken = rules.check(&split, 3, 3, |_| true, false).unwrap_err().to_string();
        assert_eq!(broken, "the group's rules need 4 approvals for payments of 100 or more, and this asks 3");
    }

    #[test]
    fn large_payments_wait_for_their_announcement_and_any_veto_stops_them() {
        let delay = Delay { from_zatoshis: 50 * ZEC, secs: 48 * 3_600 };
        let rules = GroupRules { delay: Some(delay), ..GroupRules::default() };
        assert_eq!(rules.check(&[("u1a", 49 * ZEC)], 2, 2, |_| true, false), Ok(()), "below it, no announcement");
        assert_eq!(rules.check(&[("u1a", 50 * ZEC)], 2, 2, |_| true, false), Err(Broken::NotAnnounced { delay }));
        assert_eq!(rules.check(&[("u1a", 50 * ZEC)], 2, 2, |_| true, true), Ok(()));
        let not_announced = rules.check(&[("u1a", 50 * ZEC)], 2, 2, |_| true, false).unwrap_err().to_string();
        assert_eq!(not_announced, "the group's rules need payments of 50 or more announced in a payment poll 2 days before they're sent");

        let announced_at = 1_000;
        assert_eq!(announcement_ready(announced_at, 0, delay, announced_at + delay.secs - 1), Err(NotYet::TooSoon { from: announced_at + delay.secs }));
        assert_eq!(announcement_ready(announced_at, 0, delay, announced_at + delay.secs), Ok(()));
        assert_eq!(announcement_ready(announced_at, 1, delay, announced_at + 10 * delay.secs), Err(NotYet::Vetoed), "a veto stops it however long it waits");
    }

    #[test]
    fn an_app_approves_routine_payments_to_trusted_payees_by_itself_within_the_limit() {
        let week = 7 * 86_400;
        let limit = SpendingLimit { up_to_zatoshis: 10 * ZEC, per_secs: week };
        let rules = GroupRules {
            limit: Some(limit),
            large: Some(LargePayments { from_zatoshis: 5 * ZEC, approvals: 3 }),
            ..GroupRules::default()
        };
        let trusted = |a: &str| a != "u1stranger";
        let auto = |payments: &[(&str, u64)], fee, spent| rules.may_approve_automatically(payments, fee, 2, trusted, spent);
        assert_eq!(auto(&[("u1a", ZEC)], 15_000, 3 * ZEC), Ok(ZEC + 15_000));
        assert_eq!(auto(&[], 15_000, 0), Err(NotAutomatic::NoPayments));
        assert_eq!(auto(&[("u1a", ZEC), ("u1stranger", 1)], 15_000, 0), Err(NotAutomatic::Untrusted));
        assert_eq!(auto(&[("u1a", 5 * ZEC)], 15_000, 0), Err(NotAutomatic::Large), "needs more approvals anyway");
        assert_eq!(auto(&[("u1a", ZEC)], usual_fee(1) + 1, 0), Err(NotAutomatic::Fee));
        // Up to the limit, the fee included, and not a zatoshi more.
        assert_eq!(auto(&[("u1a", ZEC - 15_000)], 15_000, 9 * ZEC), Ok(ZEC));
        let over = auto(&[("u1a", ZEC)], 15_000, 9 * ZEC);
        assert_eq!(over, Err(NotAutomatic::OverLimit { spent: 9 * ZEC, cost: ZEC + 15_000, limit }));
        assert_eq!(
            over.unwrap_err().to_string(),
            "the wallet has sent 9 in the last 7 days, and this would take it to 10.00015, over the limit of 10"
        );
        assert_eq!(GroupRules::default().may_approve_automatically(&[("u1a", 1)], 0, 2, trusted, 0), Err(NotAutomatic::NoLimit));
    }

    #[test]
    fn malformed_rules_are_never_valid() {
        assert!(GroupRules::default().is_valid());
        let large = |from_zatoshis, approvals| GroupRules { large: Some(LargePayments { from_zatoshis, approvals }), ..GroupRules::default() };
        assert!(large(ZEC, 3).is_valid());
        assert!(!large(0, 3).is_valid());
        assert!(!large(ZEC, 1).is_valid());
        assert!(!large(ZEC, MAX_APPROVALS + 1).is_valid());
        let delay = |from_zatoshis, secs| GroupRules { delay: Some(Delay { from_zatoshis, secs }), ..GroupRules::default() };
        assert!(delay(ZEC, MIN_DELAY_SECS).is_valid());
        assert!(!delay(ZEC, MIN_DELAY_SECS - 1).is_valid());
        assert!(!delay(ZEC, MAX_DELAY_SECS + 1).is_valid());
        assert!(!delay(0, MIN_DELAY_SECS).is_valid());
        let limit = |up_to_zatoshis, per_secs| GroupRules { limit: Some(SpendingLimit { up_to_zatoshis, per_secs }), ..GroupRules::default() };
        assert!(limit(ZEC, MIN_LIMIT_PERIOD_SECS).is_valid());
        assert!(!limit(0, MIN_LIMIT_PERIOD_SECS).is_valid());
        assert!(!limit(ZEC, MAX_LIMIT_PERIOD_SECS + 1).is_valid());
    }
}
