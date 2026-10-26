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

use serde::{Deserialize, Serialize};

/// The most approvals a rule can ask for.
pub const MAX_APPROVALS: u32 = 100;
/// The shortest and longest delay a rule can set before a large payment.
pub const MIN_DELAY_SECS: u64 = 60 * 60;
pub const MAX_DELAY_SECS: u64 = 30 * 24 * 60 * 60;

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
        let rules = GroupRules { trusted_payees_only: true, large: None, delay: None };
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
    }
}
