//! Invite codes: everything a member needs to join a group's key generation,
//! in one string the organizer sends them.
//!
//! Joining used to mean typing the server address, trusting its certificate,
//! entering the threshold and ciphersuite the organizer chose, and importing a
//! contact for every other member. An invite carries all of that:
//! `cyze-invite-1:` followed by base64url (no padding) of a JSON [`Invite`].
//!
//! An invite is only as trustworthy as the channel it came over. Its member
//! keys decide who can hold a share, so the app shows which members are
//! already in the joiner's contacts and asks before adding any that aren't.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::ciphersuite::Suite;
use crate::error::CoreError;

pub const PREFIX: &str = "cyze-invite-1:";
/// Comfortably more than an invite for a large group with a certificate.
const MAX_LEN: usize = 32 * 1024;
const MAX_NAME: usize = 64;
const MAX_MEMBERS: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Member {
    /// The name the member chose for themselves (their contact name).
    pub name: String,
    /// Hex communication public key.
    pub pubkey: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invite {
    /// The server members connect to, as `host[:port]`.
    pub server: String,
    /// The server's certificate, when it isn't publicly trusted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_pem: Option<String>,
    /// The key-generation session to join.
    pub session_id: uuid::Uuid,
    pub suite: Suite,
    pub threshold: u16,
    /// What the organizer named the group (each device keeps its own name).
    pub name: String,
    /// Hex public key of the member who sent the invite.
    pub organizer: String,
    /// Everyone in the session, the organizer included.
    pub members: Vec<Member>,
}

fn invalid(why: impl Into<String>) -> CoreError {
    CoreError::Config(format!("this invite isn't valid: {}", why.into()))
}

fn check_name(what: &str, name: &str) -> Result<(), CoreError> {
    if name.trim().is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(invalid(format!("{what} must be 1 to {MAX_NAME} printable characters")));
    }
    Ok(())
}

impl Invite {
    /// Check that the invite describes a session that can actually run.
    pub fn validate(&self) -> Result<(), CoreError> {
        let server = self.server.trim();
        if server.is_empty() || server.contains(['/', ' ', '@']) {
            return Err(invalid("the server address is malformed"));
        }
        check_name("the group name", &self.name)?;
        let n = self.members.len();
        if !(2..=MAX_MEMBERS).contains(&n) {
            return Err(invalid(format!("a group needs 2 to {MAX_MEMBERS} members")));
        }
        if self.threshold < 2 || usize::from(self.threshold) > n {
            return Err(invalid(format!("{} of {n} isn't a valid threshold", self.threshold)));
        }
        let mut seen = std::collections::HashSet::new();
        for m in &self.members {
            check_name("a member's name", &m.name)?;
            match hex::decode(&m.pubkey) {
                Ok(key) if key.len() == 32 => {}
                _ => return Err(invalid(format!("{}'s key is malformed", m.name))),
            }
            if !seen.insert(m.pubkey.to_ascii_lowercase()) {
                return Err(invalid("a member is listed twice"));
            }
        }
        if !self.members.iter().any(|m| m.pubkey.eq_ignore_ascii_case(&self.organizer)) {
            return Err(invalid("the organizer isn't one of the members"));
        }
        if let Some(pem) = &self.cert_pem {
            crate::tls::cert_fingerprint(pem).map_err(|_| invalid("the server certificate is malformed"))?;
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<String, CoreError> {
        self.validate()?;
        let json = serde_json::to_vec(self).map_err(|e| CoreError::Config(e.to_string()))?;
        Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json)))
    }

    /// Read an invite as pasted: surrounding text and line breaks a chat app
    /// added are ignored.
    pub fn decode(text: &str) -> Result<Invite, CoreError> {
        if text.len() > MAX_LEN {
            return Err(invalid("it's too long"));
        }
        let start = text.find(PREFIX).ok_or_else(|| invalid(format!("it should start with {PREFIX}")))?;
        // A wrapped code arrives in whitespace-separated pieces, possibly
        // followed by more words; use the longest run of pieces that decodes.
        let rest = &text[start + PREFIX.len()..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c.is_whitespace()))
            .unwrap_or(rest.len());
        let pieces: Vec<&str> = rest[..end].split_whitespace().collect();
        let mut last_err = invalid("it was cut off or changed");
        for n in (1..=pieces.len()).rev() {
            let parsed = URL_SAFE_NO_PAD
                .decode(pieces[..n].concat())
                .ok()
                .and_then(|json| serde_json::from_slice::<Invite>(&json).ok());
            if let Some(invite) = parsed {
                match invite.validate() {
                    Ok(()) => return Ok(invite),
                    Err(e) => last_err = e,
                }
            }
        }
        Err(last_err)
    }

    pub fn includes(&self, pubkey_hex: &str) -> bool {
        self.members.iter().any(|m| m.pubkey.eq_ignore_ascii_case(pubkey_hex))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite() -> Invite {
        Invite {
            server: "calm-river-words.trycloudflare.com".into(),
            cert_pem: None,
            session_id: uuid::Uuid::from_u128(7),
            suite: Suite::RedPallas,
            threshold: 2,
            name: "Treasury".into(),
            organizer: "aa".repeat(32),
            members: vec![
                Member { name: "alice".into(), pubkey: "aa".repeat(32) },
                Member { name: "bob".into(), pubkey: "bb".repeat(32) },
                Member { name: "carol".into(), pubkey: "cc".repeat(32) },
            ],
        }
    }

    #[test]
    fn round_trips_even_when_a_chat_app_wraps_it() {
        let code = invite().encode().unwrap();
        assert!(code.starts_with(PREFIX));
        assert_eq!(Invite::decode(&code).unwrap(), invite());
        let (a, b) = code.split_at(40);
        let pasted = format!("Here's the invite:\n{a}\n{b}\nthanks!");
        assert_eq!(Invite::decode(&pasted).unwrap(), invite());
    }

    #[test]
    fn rejects_changed_or_truncated_codes() {
        let code = invite().encode().unwrap();
        assert!(Invite::decode(&code[..code.len() - 10]).is_err());
        assert!(Invite::decode("hello").is_err());
        assert!(Invite::decode(&format!("{PREFIX}not-base64!!")).is_err());
    }

    #[test]
    fn rejects_invites_that_cant_run() {
        let bad = [
            Invite { threshold: 1, ..invite() },
            Invite { threshold: 4, ..invite() },
            Invite { organizer: "dd".repeat(32), ..invite() },
            Invite { server: "https://x/y".into(), ..invite() },
            Invite { name: " ".into(), ..invite() },
            Invite { members: vec![invite().members[0].clone(), invite().members[0].clone()], ..invite() },
            Invite {
                members: vec![invite().members[0].clone(), Member { name: "eve".into(), pubkey: "ab".into() }],
                ..invite()
            },
            Invite { cert_pem: Some("not a certificate".into()), ..invite() },
        ];
        for b in bad {
            assert!(b.encode().is_err(), "{b:?}");
            let json = serde_json::to_vec(&b).unwrap();
            assert!(Invite::decode(&format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json))).is_err(), "{b:?}");
        }
    }

    #[test]
    fn knows_who_it_is_for() {
        assert!(invite().includes(&"BB".repeat(32)));
        assert!(!invite().includes(&"dd".repeat(32)));
    }
}
