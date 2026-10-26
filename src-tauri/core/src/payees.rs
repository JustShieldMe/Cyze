//! A group's shared payees and what its payments were for, worked out from the
//! records its members send each other ([`crate::polls`]).
//!
//! A payee belongs to the member who added it: only their records can change
//! or remove it, so one member can't quietly swap the address behind a name
//! another member saved. Anyone can add their own. Apps show who added each
//! one, and a payment's payee is looked up on each member's own device from
//! the address the payment verifiably pays, never taken from the request.
//!
//! A payee is *trusted* once as many members as a payment needs approvals
//! trust its address: the member who added it, and others who said so
//! ([`RecordBody::PayeeTrust`]). A trust is in an address, so changing a
//! payee's address leaves it trusted only by the member who changed it. A
//! group's rules can allow payments only to trusted payees ([`crate::rules`]).
//!
//! A payment reason is what a member says a transaction was for. Usually the
//! member who started the payment, as it's sent; any member can add theirs.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use crate::polls::{GroupLog, Record, RecordBody};

/// A payee as the interface shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Payee {
    pub payee_id: String,
    /// Hex communication key of the member who added it.
    pub author: String,
    pub name: String,
    pub address: String,
    pub added_at: u64,
    pub changed_at: u64,
    /// Members (hex communication keys) trusting its address, its author first.
    pub trusted_by: Vec<String>,
    /// As many members trust it as a payment needs approvals.
    pub trusted: bool,
}

/// What a member says a transaction was for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaymentReason {
    pub txid: String,
    /// Hex communication key of the member who said it.
    pub author: String,
    pub text: String,
    pub at: u64,
}

/// A member's word on a payee: trusting it at `address`, or not.
struct Trust<'a> {
    address: &'a str,
    trusted: bool,
}

/// The records of current members that pass [`Record::check`], by author, in
/// the order each wrote them.
fn current<'a>(log: &'a GroupLog, members: &[String]) -> impl Iterator<Item = (&'a String, Vec<&'a Record>)> {
    let members: HashSet<String> = members.iter().cloned().collect();
    log.records
        .iter()
        .filter(move |(author, _)| members.contains(*author))
        .map(|(author, records)| (author, records.values().filter(|r| r.check().is_ok()).collect()))
}

/// Every payee in the group, by name, with who trusts it; trusted once
/// `threshold` members do.
pub fn payees(log: &GroupLog, members: &[String], threshold: u32) -> Vec<Payee> {
    // Each member's latest word on each payee: (owner, payee id) → member →
    // (address, trusted).
    let mut trusts: BTreeMap<(&str, &str), BTreeMap<&str, Trust>> = BTreeMap::new();
    for (author, records) in current(log, members) {
        for record in records {
            if let RecordBody::PayeeTrust { owner, payee_id, address, trusted } = &record.body {
                trusts.entry((owner, payee_id)).or_default().insert(author, Trust { address, trusted: *trusted });
            }
        }
    }
    let mut out = Vec::new();
    for (author, records) in current(log, members) {
        let mut theirs: BTreeMap<&str, Option<Payee>> = BTreeMap::new();
        for record in records {
            match &record.body {
                RecordBody::Payee { payee_id, name, address } => {
                    let added_at = theirs.get(payee_id.as_str()).and_then(|p| p.as_ref()).map_or(record.at, |p| p.added_at);
                    let payee = Payee {
                        payee_id: payee_id.clone(),
                        author: author.clone(),
                        name: name.trim().to_string(),
                        address: address.clone(),
                        added_at,
                        changed_at: record.at,
                        trusted_by: Vec::new(),
                        trusted: false,
                    };
                    theirs.insert(payee_id, Some(payee));
                }
                RecordBody::PayeeRemoved { payee_id } => {
                    theirs.insert(payee_id, None);
                }
                _ => {}
            }
        }
        out.extend(theirs.into_values().flatten());
    }
    for payee in &mut out {
        let others = trusts.get(&(payee.author.as_str(), payee.payee_id.as_str()));
        payee.trusted_by = std::iter::once(payee.author.clone())
            .chain(
                others
                    .into_iter()
                    .flatten()
                    .filter(|(member, trust)| trust.trusted && trust.address == payee.address && **member != payee.author)
                    .map(|(member, _)| member.to_string()),
            )
            .collect();
        payee.trusted = threshold > 0 && payee.trusted_by.len() >= threshold as usize;
    }
    out.sort_by_key(|p| (p.name.to_lowercase(), p.added_at));
    out
}

/// What members say each transaction was for: each member's latest for it,
/// oldest first.
pub fn reasons(log: &GroupLog, members: &[String]) -> Vec<PaymentReason> {
    let mut out = Vec::new();
    for (author, records) in current(log, members) {
        let mut theirs: BTreeMap<&str, PaymentReason> = BTreeMap::new();
        for record in records {
            if let RecordBody::PaymentReason { txid, text } = &record.body {
                if text.trim().is_empty() {
                    theirs.remove(txid.as_str());
                } else {
                    let reason =
                        PaymentReason { txid: txid.clone(), author: author.clone(), text: text.trim().to_string(), at: record.at };
                    theirs.insert(txid, reason);
                }
            }
        }
        out.extend(theirs.into_values());
    }
    out.sort_by_key(|r| r.at);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TXID: &str = "ab0000000000000000000000000000000000000000000000000000000000cdef";

    fn rec(seq: u64, at: u64, body: RecordBody) -> Record {
        Record { group_id: "group".into(), seq, at, body }
    }
    fn payee(id: &str, name: &str, address: &str) -> RecordBody {
        RecordBody::Payee { payee_id: id.into(), name: name.into(), address: address.into() }
    }
    fn reason(text: &str) -> RecordBody {
        RecordBody::PaymentReason { txid: TXID.into(), text: text.into() }
    }
    fn members() -> Vec<String> {
        ["alice", "bob"].map(String::from).to_vec()
    }

    #[test]
    fn a_payee_is_its_authors_latest_record_and_only_they_can_change_it() {
        let mut log = GroupLog::default();
        log.insert("alice", rec(1, 10, payee("p", "Acme", "u1acme")));
        log.insert("alice", rec(2, 20, payee("p", "Acme Ltd", "u1acmenew")));
        // Bob writing to Alice's payee id makes a payee of his own; it doesn't
        // touch hers.
        log.insert("bob", rec(1, 30, payee("p", "Acme Ltd", "u1bob")));
        log.insert("bob", rec(2, 40, RecordBody::PayeeRemoved { payee_id: "p".into() }));
        log.insert("mallory", rec(1, 50, payee("m", "Acme Ltd", "u1mallory")));
        let shown = payees(&log, &members(), 2);
        assert_eq!(shown.len(), 1, "{shown:?}");
        let p = &shown[0];
        assert_eq!((p.author.as_str(), p.name.as_str(), p.address.as_str()), ("alice", "Acme Ltd", "u1acmenew"));
        assert_eq!((p.added_at, p.changed_at), (10, 20));
    }

    #[test]
    fn a_removed_payee_is_gone_until_its_author_adds_it_again() {
        let mut log = GroupLog::default();
        log.insert("alice", rec(1, 10, payee("p", "Acme", "u1acme")));
        log.insert("alice", rec(2, 20, RecordBody::PayeeRemoved { payee_id: "p".into() }));
        assert!(payees(&log, &members(), 2).is_empty());
        log.insert("alice", rec(3, 30, payee("p", "Acme", "u1acme")));
        assert_eq!(payees(&log, &members(), 2)[0].added_at, 30);
    }

    #[test]
    fn payees_are_listed_by_name_and_malformed_ones_never_count() {
        let mut log = GroupLog::default();
        log.insert("alice", rec(1, 10, payee("z", "zebra fund", "u1z")));
        log.insert("bob", rec(1, 20, payee("a", "Auditor", "u1a")));
        log.insert("bob", rec(2, 30, payee("bad", "Spaces", "u1 with spaces")));
        log.insert("bob", rec(3, 40, payee("blank", "  ", "u1blank")));
        let names: Vec<_> = payees(&log, &members(), 2).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Auditor", "zebra fund"]);
    }

    fn trust(owner: &str, id: &str, address: &str, trusted: bool) -> RecordBody {
        RecordBody::PayeeTrust { owner: owner.into(), payee_id: id.into(), address: address.into(), trusted }
    }

    #[test]
    fn a_payee_is_trusted_once_enough_members_trust_its_address() {
        // Trust names the payee's owner by key, so these members have real ones.
        let (alice, bob, mallory) = ("aa".repeat(32), "bb".repeat(32), "cc".repeat(32));
        let members = vec![alice.clone(), bob.clone()];
        let mut log = GroupLog::default();
        log.insert(&alice, rec(1, 10, payee("p", "Auditor", "u1acme")));
        let first = &payees(&log, &members, 2)[0];
        assert_eq!((first.trusted_by.clone(), first.trusted), (vec![alice.clone()], false), "its author's trust alone");
        log.insert(&mallory, rec(1, 20, trust(&alice, "p", "u1acme", true)));
        assert!(!payees(&log, &members, 2)[0].trusted, "only members count");
        log.insert(&bob, rec(1, 30, trust(&alice, "p", "u1acme", true)));
        assert!(payees(&log, &members, 2)[0].trusted);

        // A new address isn't the one Bob trusted.
        log.insert(&alice, rec(2, 40, payee("p", "Auditor", "u1elsewhere")));
        let moved = &payees(&log, &members, 2)[0];
        assert_eq!((moved.trusted_by.clone(), moved.trusted), (vec![alice.clone()], false));
        log.insert(&bob, rec(2, 50, trust(&alice, "p", "u1elsewhere", true)));
        assert!(payees(&log, &members, 2)[0].trusted);
        log.insert(&bob, rec(3, 60, trust(&alice, "p", "u1elsewhere", false)));
        assert!(!payees(&log, &members, 2)[0].trusted, "Bob took it back");
    }

    #[test]
    fn each_members_latest_reason_counts_and_an_empty_one_takes_it_back() {
        let mut log = GroupLog::default();
        log.insert("alice", rec(1, 10, reason("Audit, invoice 12")));
        log.insert("alice", rec(2, 20, reason("Audit, invoice 12 (paid in full)")));
        log.insert("bob", rec(1, 30, reason("Approved at the March meeting")));
        log.insert("bob", rec(2, 40, reason("")));
        log.insert("mallory", rec(1, 50, reason("A gift")));
        let mut bad = reason("x");
        if let RecordBody::PaymentReason { txid, .. } = &mut bad {
            *txid = "not a txid".into();
        }
        log.insert("alice", rec(3, 60, bad));
        let shown = reasons(&log, &members());
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert_eq!((shown[0].author.as_str(), shown[0].text.as_str(), shown[0].at), ("alice", "Audit, invoice 12 (paid in full)", 20));
    }
}
