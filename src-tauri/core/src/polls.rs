//! Group polls: a member asks the rest of the group a question, every member
//! answers for themselves and can change their answer until the poll closes.
//! Like Snapshot, for people who already share a wallet: a way to signal and
//! coordinate, with nothing on chain and no funds involved.
//!
//! The same records carry the group's other shared state: its payees and what
//! its payments were for (worked out in [`crate::payees`]).
//!
//! ## How polls travel
//!
//! Members reach each other through the group's coordination server, as for
//! signing, and everything is end-to-end encrypted between members (Noise).
//! What travels are *records*: a poll, a vote, a poll closing early. Each
//! member numbers the records they write (`seq`, from 1) and only ever sends
//! their own, so the encrypted channel's sender authentication is what says
//! who wrote a record: no member can forge another's vote, and a server that
//! only relays sees ciphertext.
//!
//! A member who was offline catches up with a [`Message::SyncRequest`]: "this
//! is how far I've got with each of you". Each member who sees it resends
//! the records of theirs that are missing. Records are stored by author and
//! number, so receiving one twice changes nothing.
//!
//! One session carries one encrypted message to each recipient, holding every
//! record sent at once, so a recipient can always decrypt it on its own.
//!
//! ## Records from other versions
//!
//! Every numbered record a member sends is kept, including kinds this version
//! doesn't know ([`RecordBody::Unknown`]) and records it can't use (they fail
//! [`Record::check`], say a newer version allows more options). Only records
//! that pass the check count. Keeping the rest fills their number, so the
//! member's later records aren't stuck behind a gap that catching up can never
//! fill, and a later version reads them from storage as they came.

use std::collections::{BTreeMap, HashSet};

use frost_client::api::{self, PublicKey, Uuid};
use frost_client::cipher::{Cipher, PrivateKey};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::transport::{FrostdClient, ServerTrust};

/// The `message_count` marking a session that carries group-poll messages
/// (see [`crate::viewing::NEW_VIEWING_KEY_SESSION`] for why sessions are
/// marked this way).
pub const GROUP_MESSAGE_SESSION: u8 = 0xC9;

pub const MAX_TITLE: usize = 200;
pub const MAX_DETAILS: usize = 2_000;
pub const MAX_OPTION: usize = 100;
pub const MAX_OPTIONS: usize = 10;
/// The most payments a poll proposes: as many as one transaction makes
/// (`wallet::MAX_PAYMENTS`).
pub const MAX_PROPOSED_PAYMENTS: usize = 20;
/// The options of a poll that proposes payments: agreeing on the first pays.
pub const PAYMENT_OPTIONS: [&str; 2] = ["Pay", "Don't pay"];
/// The options of a poll that proposes rules: agreeing on the first adopts them.
pub const RULES_OPTIONS: [&str; 2] = ["Adopt", "Don't adopt"];
pub const MAX_PAYEE_NAME: usize = 64;
pub const MAX_ADDRESS: usize = 1_024;
pub const MAX_REASON: usize = 500;
/// The largest record kept as it came (as JSON). A bigger one is kept without
/// its contents, which still fills its number.
pub const MAX_RECORD_BYTES: usize = 16 * 1024;

/// What one member sends the others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cyze_poll", rename_all = "snake_case")]
pub enum Message {
    /// A record its sender wrote.
    Record(Record),
    /// The highest record number the sender has, with none missing below it,
    /// from each member (hex communication key); members resend theirs after
    /// that.
    SyncRequest { group_id: String, have: BTreeMap<String, u64> },
}

/// Something a member wrote, numbered in the order they wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub group_id: String,
    pub seq: u64,
    /// When the author wrote it (unix seconds, their clock).
    pub at: u64,
    #[serde(flatten)]
    pub body: RecordBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordBody {
    Poll {
        poll_id: String,
        title: String,
        #[serde(default)]
        details: String,
        options: Vec<String>,
        closes_at: u64,
        /// The Zcash community vote proposal it asks about, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        community: Option<CommunityRef>,
        /// The payments it proposes, if any: once the group agrees to pay,
        /// its author can send exactly these.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        payments: Vec<ProposedPayment>,
        /// The rules it proposes, if any, replacing the group's: in effect
        /// once the group agrees to adopt them (see [`rules_in_effect`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rules: Option<crate::rules::GroupRules>,
    },
    /// The author trusting (or no longer trusting) a payee at this address,
    /// which `owner` (hex communication key) added. A trust counts only while
    /// the payee's address is still this one.
    PayeeTrust { owner: String, payee_id: String, address: String, trusted: bool },
    /// The poll's author recording that they sent its payments, in `txid`.
    PollPaid { poll_id: String, txid: String },
    /// The author's answer: the latest one counts.
    Vote { poll_id: String, choice: u32 },
    /// The poll's author closing it early.
    Close { poll_id: String },
    /// A payee the author adds to the group's address book, or changes: their
    /// latest record for the id counts.
    Payee { payee_id: String, name: String, address: String },
    /// The author removing a payee they added.
    PayeeRemoved { payee_id: String },
    /// What a payment was for, as the author tells the group: their latest
    /// for the transaction counts, and an empty one takes theirs back. Never
    /// on chain.
    PaymentReason { txid: String, text: String },
    /// A record of a kind this version doesn't know (from a newer one), or a
    /// known kind it can't read, kept as it came. Never counts.
    #[serde(untagged)]
    Unknown(serde_json::Map<String, serde_json::Value>),
}

impl Record {
    /// Whether the record is well formed, so a malformed one is refused rather
    /// than stored.
    pub fn check(&self) -> Result<(), CoreError> {
        let bad = |m: &str| Err(CoreError::Ceremony(format!("a malformed poll record: {m}")));
        if self.seq == 0 {
            return bad("records are numbered from 1");
        }
        match &self.body {
            RecordBody::Poll { poll_id, title, details, options, closes_at, payments, rules, .. } => {
                if poll_id.is_empty() || poll_id.len() > 64 {
                    return bad("poll id");
                }
                if title.trim().is_empty() || title.chars().count() > MAX_TITLE {
                    return bad("title");
                }
                if details.chars().count() > MAX_DETAILS {
                    return bad("details");
                }
                if !(2..=MAX_OPTIONS).contains(&options.len())
                    || options.iter().any(|o| o.trim().is_empty() || o.chars().count() > MAX_OPTION)
                {
                    return bad("options");
                }
                if *closes_at <= self.at {
                    return bad("it closes before it opens");
                }
                if let Some(rules) = rules {
                    if options.len() != RULES_OPTIONS.len() || !payments.is_empty() || !rules.is_valid() {
                        return bad("proposed rules");
                    }
                }
                if !payments.is_empty() {
                    if options.len() != PAYMENT_OPTIONS.len() || payments.len() > MAX_PROPOSED_PAYMENTS {
                        return bad("proposed payments");
                    }
                    for p in payments {
                        let address = &p.recipient;
                        if address.is_empty() || address.len() > MAX_ADDRESS || !address.chars().all(|c| c.is_ascii_alphanumeric()) {
                            return bad("a proposed payment's address");
                        }
                        if p.amount_zatoshis == 0 || p.amount_zatoshis > MAX_ZATOSHIS {
                            return bad("a proposed payment's amount");
                        }
                        if p.memo.as_ref().is_some_and(|m| m.len() > 512) {
                            return bad("a proposed payment's memo");
                        }
                    }
                }
            }
            RecordBody::PollPaid { poll_id, txid } => {
                if poll_id.is_empty() || poll_id.len() > 64 {
                    return bad("poll id");
                }
                if txid.len() != 64 || !txid.chars().all(|c| c.is_ascii_hexdigit()) {
                    return bad("transaction id");
                }
            }
            RecordBody::Vote { poll_id, .. } | RecordBody::Close { poll_id } => {
                if poll_id.is_empty() || poll_id.len() > 64 {
                    return bad("poll id");
                }
            }
            RecordBody::Payee { payee_id, name, address } => {
                if payee_id.is_empty() || payee_id.len() > 64 {
                    return bad("payee id");
                }
                if name.trim().is_empty() || name.chars().count() > MAX_PAYEE_NAME {
                    return bad("payee name");
                }
                if address.is_empty() || address.len() > MAX_ADDRESS || !address.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return bad("payee address");
                }
            }
            RecordBody::PayeeTrust { owner, payee_id, address, .. } => {
                if owner.is_empty() || owner.len() > 128 || !owner.chars().all(|c| c.is_ascii_hexdigit()) {
                    return bad("payee owner");
                }
                if payee_id.is_empty() || payee_id.len() > 64 {
                    return bad("payee id");
                }
                if address.is_empty() || address.len() > MAX_ADDRESS || !address.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return bad("payee address");
                }
            }
            RecordBody::PayeeRemoved { payee_id } => {
                if payee_id.is_empty() || payee_id.len() > 64 {
                    return bad("payee id");
                }
            }
            RecordBody::PaymentReason { txid, text } => {
                if txid.len() != 64 || !txid.chars().all(|c| c.is_ascii_hexdigit()) {
                    return bad("transaction id");
                }
                if text.chars().count() > MAX_REASON {
                    return bad("reason");
                }
            }
            RecordBody::Unknown(_) => return bad("a kind this version of Cyze doesn't know"),
        }
        Ok(())
    }

    /// The record as this device keeps it: as it came, unless it's larger
    /// than [`MAX_RECORD_BYTES`].
    pub fn bounded(self) -> Self {
        let size = serde_json::to_vec(&self).map_or(usize::MAX, |json| json.len());
        if size <= MAX_RECORD_BYTES {
            return self;
        }
        Record { body: RecordBody::Unknown(serde_json::Map::new()), ..self }
    }
}

/// The messages in one decrypted batch. One that doesn't parse (a message
/// type from a newer version, or a malformed one) is skipped rather than
/// losing the rest.
pub fn decode_batch(bytes: &[u8]) -> Result<Vec<Message>, CoreError> {
    let batch: Vec<serde_json::Value> =
        serde_json::from_slice(bytes).map_err(|e| CoreError::Ceremony(format!("a malformed group message: {e}")))?;
    Ok(batch
        .into_iter()
        .filter_map(|message| {
            serde_json::from_value(message)
                .inspect_err(|e| tracing::debug!("skipped a group message this version can't read: {e}"))
                .ok()
        })
        .collect())
}

/// The most zatoshis there will ever be (21 million coins).
const MAX_ZATOSHIS: u64 = 21_000_000 * 100_000_000;

/// A payment a poll proposes, as the group would make it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedPayment {
    pub recipient: String,
    pub amount_zatoshis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// A Zcash community vote proposal (`crate::community_vote`) a poll asks the
/// group about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityRef {
    /// The round's id, hex.
    pub round_id: String,
    pub proposal_id: u32,
}

/// Every record this device has for one group, by author (hex communication
/// key) and number, plus its own numbering and what it hasn't delivered yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupLog {
    #[serde(default)]
    pub records: BTreeMap<String, BTreeMap<u64, Record>>,
    /// Numbers of this device's own records not yet sent to every member.
    #[serde(default)]
    pub outbox: Vec<u64>,
}

impl GroupLog {
    /// Store a record from `author`. Returns whether it was new.
    pub fn insert(&mut self, author: &str, record: Record) -> bool {
        let by_author = self.records.entry(author.to_string()).or_default();
        if by_author.contains_key(&record.seq) {
            return false;
        }
        by_author.insert(record.seq, record);
        true
    }

    /// The next number for a record `me` writes.
    pub fn next_seq(&self, me: &str) -> u64 {
        self.records.get(me).and_then(|r| r.keys().next_back()).map_or(1, |n| n + 1)
    }

    /// The highest number from each author with none missing below it.
    pub fn have(&self) -> BTreeMap<String, u64> {
        self.records
            .iter()
            .map(|(author, records)| {
                let mut n = 0;
                while records.contains_key(&(n + 1)) {
                    n += 1;
                }
                (author.clone(), n)
            })
            .collect()
    }

    /// `me`'s records after number `after`, for a member catching up.
    pub fn own_after(&self, me: &str, after: u64) -> Vec<Record> {
        self.records
            .get(me)
            .map(|r| r.range(after + 1..).map(|(_, rec)| rec.clone()).collect())
            .unwrap_or_default()
    }
}

/// A poll as the interface shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PollView {
    pub group_id: String,
    pub poll_id: String,
    /// Hex communication key of the member who asked.
    pub author: String,
    pub title: String,
    pub details: String,
    pub options: Vec<String>,
    pub created_at: u64,
    pub closes_at: u64,
    /// Closed by time or by its author.
    pub closed: bool,
    /// Each member's current answer (hex communication key → option).
    pub votes: BTreeMap<String, u32>,
    /// How many members chose each option.
    pub tally: Vec<u32>,
    /// This device's member's answer.
    pub my_vote: Option<u32>,
    pub members: u32,
    /// How many members must agree for an option to count as agreed: the
    /// group's signing threshold.
    pub threshold: u32,
    /// The option at least `threshold` members chose, if any.
    pub agreed: Option<u32>,
    /// The Zcash community vote proposal it asks about, if any.
    pub community: Option<CommunityRef>,
    /// The payments it proposes, if any (agreeing on option 0 pays them).
    pub payments: Vec<ProposedPayment>,
    /// The rules it proposes, if any (agreeing on option 0 adopts them).
    pub rules: Option<crate::rules::GroupRules>,
    /// The transaction its author sent them in, once they did.
    pub paid_txid: Option<String>,
}

/// Work out every poll in a group from its records. Only current members'
/// records that pass [`Record::check`] count; a vote counts if it's for an
/// existing option, written before the poll closed, and the latest of that
/// member's votes on it.
pub fn polls(group_id: &str, log: &GroupLog, members: &[String], threshold: u32, me: &str, now: u64) -> Vec<PollView> {
    let members: HashSet<&str> = members.iter().map(String::as_str).collect();
    let current = |author: &String| members.contains(author.as_str());
    let valid = |r: &&Record| r.check().is_ok();
    let mut out = Vec::new();
    for (author, records) in log.records.iter().filter(|(a, _)| current(a)) {
        for record in records.values().filter(valid) {
            let RecordBody::Poll { poll_id, title, details, options, closes_at, community, payments, rules } = &record.body else {
                continue;
            };
            // Paid: the author's first record of sending it.
            let paid_txid = records.values().filter(valid).find_map(|r| match &r.body {
                RecordBody::PollPaid { poll_id: p, txid } if p == poll_id => Some(txid.clone()),
                _ => None,
            });
            // Closed early: the author's first Close for it.
            let closed_at = records
                .values()
                .filter(valid)
                .filter(|r| matches!(&r.body, RecordBody::Close { poll_id: p } if p == poll_id))
                .map(|r| r.at)
                .min();
            let ends = closed_at.map_or(*closes_at, |c| c.min(*closes_at));
            let mut votes = BTreeMap::new();
            for (voter, theirs) in log.records.iter().filter(|(a, _)| current(a)) {
                let latest = theirs
                    .values()
                    .rev()
                    .filter(valid)
                    .filter(|r| r.at <= ends)
                    .find_map(|r| match &r.body {
                        RecordBody::Vote { poll_id: p, choice } if p == poll_id && (*choice as usize) < options.len() => {
                            Some(*choice)
                        }
                        _ => None,
                    });
                if let Some(choice) = latest {
                    votes.insert(voter.clone(), choice);
                }
            }
            let mut tally = vec![0u32; options.len()];
            for choice in votes.values() {
                tally[*choice as usize] += 1;
            }
            let agreed = (threshold > 0)
                .then(|| tally.iter().position(|n| *n >= threshold).map(|i| i as u32))
                .flatten();
            out.push(PollView {
                group_id: group_id.to_string(),
                poll_id: poll_id.clone(),
                author: author.clone(),
                title: title.clone(),
                details: details.clone(),
                options: options.clone(),
                created_at: record.at,
                closes_at: *closes_at,
                closed: closed_at.is_some() || now >= *closes_at,
                my_vote: votes.get(me).copied(),
                votes,
                tally,
                members: members.len() as u32,
                threshold,
                agreed,
                community: community.clone(),
                payments: payments.clone(),
                rules: rules.clone(),
                paid_txid,
            });
        }
    }
    // Open polls first, newest first within each.
    out.sort_by_key(|p| (p.closed, std::cmp::Reverse(p.created_at)));
    out
}

/// The poll whose rules are in effect: the latest one the group agreed to
/// adopt. None if the group never adopted any, so it has none.
pub fn rules_in_effect(polls: &[PollView]) -> Option<&PollView> {
    polls
        .iter()
        .filter(|p| p.rules.is_some() && p.agreed == Some(0))
        .max_by(|a, b| (a.created_at, &a.poll_id).cmp(&(b.created_at, &b.poll_id)))
}

/// Who a device sends as, and through which server.
pub struct Channel {
    pub server_url: String,
    pub trust: ServerTrust,
    pub comm_privkey: PrivateKey,
    pub comm_pubkey: PublicKey,
}

/// Send `messages` to each of `to` in one new session, one encrypted message
/// each. The session is left open so members who are offline can still
/// collect them while the server keeps it (a day after its last activity).
pub async fn send(channel: &Channel, to: &[PublicKey], messages: &[Message]) -> Result<Uuid, CoreError> {
    if to.is_empty() {
        return Err(CoreError::Config("nobody to send to".into()));
    }
    let bytes = serde_json::to_vec(messages).map_err(|e| CoreError::Ceremony(e.to_string()))?;
    let mut client = FrostdClient::new(format!("https://{}", channel.server_url), &channel.trust)?;
    client.login(&channel.comm_privkey, &channel.comm_pubkey).await?;
    let result = async {
        let session_id = client
            .create_new_session(&api::CreateNewSessionArgs {
                pubkeys: to.to_vec(),
                message_count: GROUP_MESSAGE_SESSION,
            })
            .await?
            .session_id;
        let mut cipher =
            Cipher::new(channel.comm_privkey.clone(), to.to_vec()).map_err(|e| CoreError::Crypto(e.to_string()))?;
        for pk in to {
            let msg = cipher.encrypt(Some(pk), bytes.clone()).map_err(|e| CoreError::Crypto(e.to_string()))?;
            client.send(&api::SendArgs { session_id, recipients: vec![pk.clone()], msg }).await?;
        }
        Ok(session_id)
    }
    .await;
    let _ = client.logout().await;
    result
}

/// Read what another member sent in `session_id`: who sent it (checked to be
/// one of `members`) and the messages. Reading empties this device's queue in
/// the session, so reading it again returns nothing.
pub async fn receive(
    channel: &Channel,
    session_id: Uuid,
    members: &[PublicKey],
) -> Result<(PublicKey, Vec<Message>), CoreError> {
    let mut client = FrostdClient::new(format!("https://{}", channel.server_url), &channel.trust)?;
    client.login(&channel.comm_privkey, &channel.comm_pubkey).await?;
    let result = async {
        let info = client.get_session_info(&api::GetSessionInfoArgs { session_id }).await?;
        if info.message_count != GROUP_MESSAGE_SESSION {
            return Err(CoreError::Ceremony("this session doesn't carry group messages".into()));
        }
        let sender = info.coordinator_pubkey;
        if !members.contains(&sender) || info.pubkeys.iter().any(|p| !members.contains(p)) {
            return Err(CoreError::Ceremony("the session isn't between this group's members".into()));
        }
        let mut cipher =
            Cipher::new(channel.comm_privkey.clone(), vec![sender.clone()]).map_err(|e| CoreError::Crypto(e.to_string()))?;
        let r = client.receive(&api::ReceiveArgs { session_id, as_coordinator: false }).await?;
        let mut messages = Vec::new();
        for msg in r.msgs {
            let msg = cipher.decrypt(msg).map_err(|e| CoreError::Crypto(e.to_string()))?;
            if msg.sender != sender {
                continue;
            }
            messages.extend(decode_batch(&msg.msg)?);
        }
        Ok((sender, messages))
    }
    .await;
    let _ = client.logout().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: &str = "group";

    fn rec(seq: u64, at: u64, body: RecordBody) -> Record {
        Record { group_id: G.into(), seq, at, body }
    }
    fn poll(id: &str, _at: u64, closes_at: u64) -> RecordBody {
        RecordBody::Poll {
            poll_id: id.into(),
            title: "Hire an auditor?".into(),
            details: String::new(),
            options: vec!["Yes".into(), "No".into()],
            closes_at,
            community: None,
            payments: Vec::new(),
            rules: None,
        }
    }
    fn vote(id: &str, choice: u32) -> RecordBody {
        RecordBody::Vote { poll_id: id.into(), choice }
    }
    fn members() -> Vec<String> {
        ["alice", "bob", "carol"].map(String::from).to_vec()
    }

    #[test]
    fn the_latest_vote_before_closing_counts_and_agreement_needs_the_threshold() {
        let mut log = GroupLog::default();
        log.insert("alice", rec(1, 100, poll("p", 100, 1_000)));
        log.insert("alice", rec(2, 110, vote("p", 0)));
        log.insert("bob", rec(1, 120, vote("p", 1)));
        log.insert("bob", rec(2, 130, vote("p", 0))); // changed their mind
        log.insert("carol", rec(1, 2_000, vote("p", 1))); // too late
        let p = &polls(G, &log, &members(), 2, "bob", 500)[0];
        assert_eq!(p.tally, vec![2, 0]);
        assert_eq!(p.my_vote, Some(0));
        assert_eq!(p.agreed, Some(0));
        assert!(!p.closed);
        assert!(polls(G, &log, &members(), 2, "bob", 1_000)[0].closed);
    }

    #[test]
    fn only_members_count_and_only_the_author_closes_early() {
        let mut log = GroupLog::default();
        log.insert("alice", rec(1, 100, poll("p", 100, 1_000)));
        log.insert("mallory", rec(1, 110, vote("p", 0)));
        log.insert("bob", rec(1, 120, RecordBody::Close { poll_id: "p".into() }));
        log.insert("carol", rec(1, 130, vote("p", 7))); // no such option
        let p = &polls(G, &log, &members(), 2, "alice", 500)[0];
        assert_eq!(p.tally, vec![0, 0]);
        assert!(!p.closed, "bob didn't ask, so bob can't close it");
        log.insert("alice", rec(2, 140, RecordBody::Close { poll_id: "p".into() }));
        log.insert("bob", rec(2, 150, vote("p", 0))); // after it closed
        let p = &polls(G, &log, &members(), 2, "alice", 500)[0];
        assert!(p.closed);
        assert_eq!(p.tally, vec![0, 0]);
    }

    #[test]
    fn catching_up_asks_for_what_is_missing() {
        let mut log = GroupLog::default();
        assert_eq!(log.next_seq("alice"), 1);
        log.insert("alice", rec(1, 1, vote("p", 0)));
        log.insert("alice", rec(3, 3, vote("p", 1)));
        assert!(!log.insert("alice", rec(1, 1, vote("p", 0))), "a duplicate changes nothing");
        assert_eq!(log.have().get("alice"), Some(&1), "number 2 is missing");
        assert_eq!(log.next_seq("alice"), 4);
        assert_eq!(log.own_after("alice", 1).iter().map(|r| r.seq).collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn malformed_records_are_refused() {
        assert!(rec(1, 10, poll("p", 10, 100)).check().is_ok());
        assert!(rec(0, 10, poll("p", 10, 100)).check().is_err());
        assert!(rec(1, 10, poll("p", 10, 5)).check().is_err());
        let one_option = RecordBody::Poll {
            poll_id: "p".into(),
            title: "t".into(),
            details: String::new(),
            options: vec!["only".into()],
            closes_at: 100,
            community: None,
            payments: Vec::new(),
            rules: None,
        };
        assert!(rec(1, 10, one_option).check().is_err());
        let message = Message::Record(rec(1, 10, vote("p", 0)));
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), message);
        assert!(json.contains(r#""cyze_poll":"record""#), "{json}");
    }

    fn payment_poll(id: &str, payments: Vec<ProposedPayment>) -> RecordBody {
        RecordBody::Poll {
            poll_id: id.into(),
            title: "Pay the auditor?".into(),
            details: String::new(),
            options: PAYMENT_OPTIONS.map(String::from).to_vec(),
            closes_at: 1_000,
            community: None,
            payments,
            rules: None,
        }
    }
    fn pay(recipient: &str, amount_zatoshis: u64) -> ProposedPayment {
        ProposedPayment { recipient: recipient.into(), amount_zatoshis, memo: None }
    }

    #[test]
    fn a_poll_proposes_payments_and_only_its_author_marks_it_paid() {
        let txid = "ab".repeat(32);
        let mut log = GroupLog::default();
        log.insert("alice", rec(1, 100, payment_poll("p", vec![pay("u1auditor", 250_000_000)])));
        log.insert("alice", rec(2, 110, vote("p", 0)));
        log.insert("bob", rec(1, 120, vote("p", 0)));
        log.insert("bob", rec(2, 130, RecordBody::PollPaid { poll_id: "p".into(), txid: "cd".repeat(32) }));
        let p = &polls(G, &log, &members(), 2, "bob", 500)[0];
        assert_eq!(p.payments, vec![pay("u1auditor", 250_000_000)]);
        assert_eq!(p.agreed, Some(0), "agreed to pay");
        assert_eq!(p.paid_txid, None, "Bob didn't propose it, so Bob can't mark it paid");
        log.insert("alice", rec(3, 140, RecordBody::PollPaid { poll_id: "p".into(), txid: txid.clone() }));
        assert_eq!(polls(G, &log, &members(), 2, "bob", 500)[0].paid_txid, Some(txid));
    }

    #[test]
    fn a_poll_proposing_payments_is_refused_unless_well_formed() {
        assert!(rec(1, 10, payment_poll("p", vec![pay("u1auditor", 1)])).check().is_ok());
        let mut three_options = payment_poll("p", vec![pay("u1auditor", 1)]);
        if let RecordBody::Poll { options, .. } = &mut three_options {
            options.push("Later".into());
        }
        assert!(rec(1, 10, three_options).check().is_err());
        assert!(rec(1, 10, payment_poll("p", vec![pay("u1auditor", 0)])).check().is_err());
        assert!(rec(1, 10, payment_poll("p", vec![pay("u1 auditor", 1)])).check().is_err());
        let too_many = (0..=MAX_PROPOSED_PAYMENTS).map(|_| pay("u1auditor", 1)).collect();
        assert!(rec(1, 10, payment_poll("p", too_many)).check().is_err());
        let memo = ProposedPayment { memo: Some("x".repeat(513)), ..pay("u1auditor", 1) };
        assert!(rec(1, 10, payment_poll("p", vec![memo])).check().is_err());
    }

    fn rules_poll(id: &str, rules: crate::rules::GroupRules) -> RecordBody {
        RecordBody::Poll {
            poll_id: id.into(),
            title: "New rules".into(),
            details: String::new(),
            options: RULES_OPTIONS.map(String::from).to_vec(),
            closes_at: 1_000,
            community: None,
            payments: Vec::new(),
            rules: Some(rules),
        }
    }

    #[test]
    fn the_latest_rules_the_group_agreed_to_are_in_effect() {
        use crate::rules::{GroupRules, LargePayments};
        let trusted_only = GroupRules { trusted_payees_only: true, large: None };
        let tiered = GroupRules { trusted_payees_only: false, large: Some(LargePayments { from_zatoshis: 1, approvals: 3 }) };
        let mut log = GroupLog::default();
        assert!(rules_in_effect(&polls(G, &log, &members(), 2, "bob", 500)).is_none(), "none adopted: no rules");
        log.insert("alice", rec(1, 100, rules_poll("r1", trusted_only.clone())));
        log.insert("alice", rec(2, 101, vote("r1", 0)));
        log.insert("bob", rec(1, 102, vote("r1", 0)));
        log.insert("bob", rec(2, 200, rules_poll("r2", tiered.clone())));
        log.insert("bob", rec(3, 201, vote("r2", 0)));
        let views = polls(G, &log, &members(), 2, "bob", 500);
        assert_eq!(rules_in_effect(&views).and_then(|p| p.rules.clone()), Some(trusted_only), "r2 isn't agreed yet");
        log.insert("carol", rec(1, 202, vote("r2", 0)));
        let views = polls(G, &log, &members(), 2, "bob", 500);
        assert_eq!(rules_in_effect(&views).and_then(|p| p.rules.clone()), Some(tiered), "the later one");
    }

    #[test]
    fn a_poll_proposing_rules_is_refused_unless_well_formed() {
        use crate::rules::{GroupRules, LargePayments};
        assert!(rec(1, 10, rules_poll("r", GroupRules::default())).check().is_ok(), "no rules at all is a valid proposal");
        let impossible = GroupRules { trusted_payees_only: false, large: Some(LargePayments { from_zatoshis: 0, approvals: 3 }) };
        assert!(rec(1, 10, rules_poll("r", impossible)).check().is_err());
        let mut with_payments = rules_poll("r", GroupRules::default());
        if let RecordBody::Poll { payments, .. } = &mut with_payments {
            payments.push(pay("u1auditor", 1));
        }
        assert!(rec(1, 10, with_payments).check().is_err(), "rules and payments don't mix");
    }

    /// What a newer version might send: a kind of record this one doesn't
    /// know, then a vote, plus a message type it doesn't know.
    const FROM_A_NEWER_VERSION: &str = r#"[
        {"cyze_poll":"record","group_id":"group","seq":1,"at":10,"kind":"from_the_future","what":"a kind no version uses"},
        {"cyze_poll":"record","group_id":"group","seq":2,"at":20,"kind":"vote","poll_id":"p","choice":0},
        {"cyze_poll":"something_new","group_id":"group"}
    ]"#;

    #[test]
    fn a_batch_from_a_newer_version_still_delivers_what_this_version_can_read() {
        let messages = decode_batch(FROM_A_NEWER_VERSION.as_bytes()).unwrap();
        assert_eq!(messages.len(), 2, "the unknown message type is skipped, not the batch");
        let [Message::Record(future), Message::Record(vote)] = messages.as_slice() else { panic!("{messages:?}") };
        assert!(matches!(future.body, RecordBody::Unknown(_)));
        assert_eq!(vote.body, RecordBody::Vote { poll_id: "p".into(), choice: 0 });
    }

    #[test]
    fn a_record_kind_this_version_doesnt_know_fills_its_number_and_never_counts() {
        let Message::Record(future) = decode_batch(FROM_A_NEWER_VERSION.as_bytes()).unwrap().remove(0) else {
            panic!()
        };
        assert!(future.check().is_err());
        let mut log = GroupLog::default();
        log.insert("alice", rec(2, 20, poll("p", 20, 1_000)));
        log.insert("alice", future.clone());
        assert_eq!(log.have().get("alice"), Some(&2), "no gap left to ask for again");
        assert_eq!(polls(G, &log, &members(), 2, "bob", 500).len(), 1);

        // Stored and read back (or resent after a downgrade), it's unchanged,
        // so a version that knows the kind reads it as it was written.
        let stored = serde_json::to_string(&log).unwrap();
        let back: GroupLog = serde_json::from_str(&stored).unwrap();
        assert_eq!(back, log);
        let original: serde_json::Value = serde_json::from_str(FROM_A_NEWER_VERSION).unwrap();
        let mut resent = serde_json::to_value(Message::Record(back.own_after("alice", 0)[0].clone())).unwrap();
        resent.as_object_mut().unwrap().remove("cyze_poll");
        let mut sent = original[0].clone();
        sent.as_object_mut().unwrap().remove("cyze_poll");
        assert_eq!(resent, sent);
    }

    #[test]
    fn a_record_this_version_cant_use_fills_its_number_and_never_counts() {
        let mut too_many = poll("q", 10, 1_000);
        if let RecordBody::Poll { options, .. } = &mut too_many {
            *options = (0..MAX_OPTIONS + 1).map(|i| i.to_string()).collect();
        }
        let bad_vote = r#"[{"cyze_poll":"record","group_id":"group","seq":3,"at":30,"kind":"vote","poll_id":"p","choice":"yes"}]"#;
        let Message::Record(bad_vote) = decode_batch(bad_vote.as_bytes()).unwrap().remove(0) else { panic!() };
        let mut log = GroupLog::default();
        log.insert("alice", rec(1, 10, poll("p", 10, 1_000)));
        log.insert("alice", rec(2, 20, too_many));
        log.insert("alice", bad_vote);
        log.insert("alice", rec(4, 40, vote("p", 1)));
        assert_eq!(log.have().get("alice"), Some(&4));
        let shown = polls(G, &log, &members(), 2, "alice", 500);
        assert_eq!(shown.iter().map(|p| p.poll_id.as_str()).collect::<Vec<_>>(), ["p"]);
        assert_eq!(shown[0].tally, vec![0, 1], "the unreadable vote doesn't count; the next one does");
    }

    #[test]
    fn an_oversized_record_is_kept_without_its_contents() {
        let mut big = serde_json::Map::new();
        big.insert("kind".into(), "from_the_future".into());
        big.insert("what".into(), "x".repeat(MAX_RECORD_BYTES).into());
        let kept = rec(5, 50, RecordBody::Unknown(big)).bounded();
        assert_eq!((kept.seq, kept.at), (5, 50));
        assert_eq!(kept.body, RecordBody::Unknown(serde_json::Map::new()));
        let back: Record = serde_json::from_str(&serde_json::to_string(&kept).unwrap()).unwrap();
        assert_eq!(back, kept);
        let small = rec(1, 10, vote("p", 0));
        assert_eq!(small.clone().bounded(), small);
    }
}
