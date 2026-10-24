//! The viewing seed a group's members agree on, from which its Orchard viewing
//! key is derived (`zcash::orchard_fvk_bytes`). Kept apart from the `zcash`
//! feature because key generation runs without it.
//!
//! New groups agree on it during key generation (`dkg::agree_viewing_seed`).
//! A group created before that derived its viewing key from its id alone, so
//! anyone who learned the id could see its balance and payments. Its members
//! can agree on a seed later, over a session of their own
//! ([`agree_new_viewing_key`]); the group then gets a new, private address,
//! and moves its funds there with an ordinary signed payment.

use std::collections::{HashMap, HashSet, VecDeque};

use frost_client::api::{self, PublicKey, Uuid};
use frost_client::cipher::{Cipher, PrivateKey};
use frost_core::Identifier;
use reddsa::frost::redpallas::PallasBlake2b512;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::CoreError;
use crate::transport::{FrostdClient, ServerTrust};

const SEED_DOMAIN: &[u8] = b"Cyze:OrchardFVK:seed:v2";
const CONFIRM_DOMAIN: &[u8] = b"Cyze:OrchardFVK:confirm:v2";

/// The secret a group's members agree on to derive its viewing key.
pub type ViewingSeed = [u8; 32];

/// The seed for group key `ak` from every member's contribution, keyed by
/// their FROST identifier (serialized). Order-independent: members receive
/// the contributions in any order.
pub fn combine(ak: &[u8], contributions: &[(Vec<u8>, [u8; 32])]) -> ViewingSeed {
    let mut sorted: Vec<&(Vec<u8>, [u8; 32])> = contributions.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    h.update(SEED_DOMAIN);
    h.update((ak.len() as u64).to_le_bytes());
    h.update(ak);
    for (identifier, contribution) in sorted {
        h.update((identifier.len() as u64).to_le_bytes());
        h.update(identifier);
        h.update(contribution);
    }
    h.finalize().into()
}

/// What members compare at the end of the agreement: equal digests mean equal
/// seeds, and so equal viewing keys. Reveals nothing about the seed.
pub fn digest(ak: &[u8], seed: &ViewingSeed) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(CONFIRM_DOMAIN);
    h.update((ak.len() as u64).to_le_bytes());
    h.update(ak);
    h.update(seed);
    h.finalize().into()
}

/// The `message_count` of a session that agrees on a new viewing key for an
/// existing group. frostd sessions carry no type, and signing and key
/// generation always ask for 1 message, so this value is how members' apps
/// tell such a session from a signing request.
pub const NEW_VIEWING_KEY_SESSION: u8 = 0xC7;

/// Agreeing on a new viewing key for an existing group.
pub struct NewViewingKeyParams {
    /// `host:port` of the frostd server.
    pub server_url: String,
    pub trust: ServerTrust,
    pub comm_privkey: PrivateKey,
    pub comm_pubkey: PublicKey,
    /// The group's verifying key (`ak`), hex: its id.
    pub group_id: String,
    /// Every member's communication key, this device's included. Every one
    /// must take part: a member left out couldn't see the group's funds.
    pub members: Vec<PublicKey>,
    /// The session to join, or `None` to start one.
    pub session_id: Option<Uuid>,
}

/// Progress of [`agree_new_viewing_key`].
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ViewingKeyEvent {
    Connecting,
    /// In the session; members are sending their contributions.
    SessionReady { session_id: Uuid },
    /// These members (hex communication keys) haven't sent theirs yet.
    Waiting { members: Vec<String> },
}

/// How long to wait for every member before giving up.
pub const NEW_VIEWING_KEY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Agree on a new viewing seed with the rest of an existing group, the way
/// members of a new group do during key generation: everyone contributes, so
/// no one member picks it, and everyone confirms they got the same one.
///
/// The member who starts creates the session with every member in it, marked
/// as [`NEW_VIEWING_KEY_SESSION`]; the others join it by id. Joining checks
/// that the session is that kind and holds exactly the group's members, so a
/// session that includes an outsider, or leaves a member out, is refused.
pub async fn agree_new_viewing_key(
    params: NewViewingKeyParams,
    events: mpsc::Sender<ViewingKeyEvent>,
    cancel: CancellationToken,
) -> Result<ViewingSeed, CoreError> {
    let group_key: [u8; 32] = hex::decode(params.group_id.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| CoreError::Config("the group id is not 32 bytes of hex".into()))?;
    let members: HashSet<&PublicKey> = params.members.iter().collect();
    if !members.contains(&params.comm_pubkey) {
        return Err(CoreError::Config("you aren't a member of this group".into()));
    }
    if members.len() < 2 {
        return Err(CoreError::Config("a group needs at least two members".into()));
    }

    let mut client = FrostdClient::new(format!("https://{}", params.server_url), &params.trust)?;
    let _ = events.send(ViewingKeyEvent::Connecting).await;
    client.login(&params.comm_privkey, &params.comm_pubkey).await?;
    let starting = params.session_id.is_none();
    let session_id = match params.session_id {
        Some(id) => id,
        None => {
            client
                .create_new_session(&api::CreateNewSessionArgs {
                    pubkeys: params.members.clone(),
                    message_count: NEW_VIEWING_KEY_SESSION,
                })
                .await?
                .session_id
        }
    };

    let result = async {
        let info = client.get_session_info(&api::GetSessionInfoArgs { session_id }).await?;
        if info.message_count != NEW_VIEWING_KEY_SESSION {
            return Err(CoreError::Ceremony("this session isn't for agreeing on a viewing key".into()));
        }
        let in_session: HashSet<&PublicKey> = info.pubkeys.iter().collect();
        if in_session != members || !members.contains(&info.coordinator_pubkey) {
            return Err(CoreError::Ceremony(
                "the session's members aren't exactly this group's members, so it was refused".into(),
            ));
        }
        let _ = events.send(ViewingKeyEvent::SessionReady { session_id }).await;

        let identifiers = params
            .members
            .iter()
            .map(|p| {
                Identifier::<PallasBlake2b512>::derive(&[session_id.as_bytes(), &p.0[..]].concat())
                    .map(|id| (p.clone(), id))
                    .map_err(|e| CoreError::Ceremony(e.to_string()))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let me = identifiers[&params.comm_pubkey];
        let mut cipher = Cipher::new(params.comm_privkey.clone(), params.members.clone())
            .map_err(|e| CoreError::Crypto(e.to_string()))?;
        let mut pending = VecDeque::new();
        let mut io = crate::dkg::SessionIo {
            client: &client,
            session_id,
            cipher: &mut cipher,
            cancel: &cancel,
            pending: &mut pending,
        };
        let mut last_waiting = Vec::new();
        let mut waiting = |missing: &[PublicKey]| {
            let mut missing: Vec<String> = missing.iter().map(|p| hex::encode(&p.0)).collect();
            missing.sort();
            if missing != last_waiting {
                last_waiting.clone_from(&missing);
                let _ = events.try_send(ViewingKeyEvent::Waiting { members: missing });
            }
        };
        crate::dkg::agree_viewing_seed(&mut io, &identifiers, &params.comm_pubkey, me, &group_key, &mut waiting).await
    };
    let result = tokio::select! {
        r = result => r,
        _ = tokio::time::sleep(NEW_VIEWING_KEY_TIMEOUT) => Err(CoreError::Ceremony(
            "not every member joined within 30 minutes. Try again when everyone has the app open.".into(),
        )),
    };

    // Like key generation, the member who started keeps the session open a
    // little after finishing, so the others can still collect the last
    // confirmations; on failure it closes it so they stop waiting.
    match (&result, starting) {
        (Ok(_), true) => {
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                let _ = client.close_session(&api::CloseSessionArgs { session_id }).await;
                let _ = client.logout().await;
            });
        }
        (Err(_), true) => {
            let _ = client.close_session(&api::CloseSessionArgs { session_id }).await;
            let _ = client.logout().await;
        }
        (_, false) => {
            let _ = client.logout().await;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_ignores_arrival_order_but_not_contributions() {
        let ak = [1u8; 32];
        let a = (vec![1u8; 32], [1u8; 32]);
        let b = (vec![2u8; 32], [2u8; 32]);
        let c = (vec![3u8; 32], [3u8; 32]);
        let seed = combine(&ak, &[a.clone(), b.clone(), c.clone()]);
        assert_eq!(seed, combine(&ak, &[c.clone(), a.clone(), b.clone()]));
        assert_ne!(seed, combine(&ak, &[a.clone(), b.clone(), (c.0.clone(), [4u8; 32])]));
        assert_ne!(seed, combine(&ak, &[a.clone(), b.clone()]));
        assert_ne!(seed, combine(&[2u8; 32], &[a, b, c]));
        assert_eq!(digest(&ak, &seed), digest(&ak, &seed));
        assert_ne!(digest(&ak, &seed), digest(&ak, &[0u8; 32]));
    }
}
