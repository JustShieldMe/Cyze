//! Repairing a member's lost key share with the help of others, using FROST's
//! repairable threshold scheme (`frost_core::keys::repairable`): as many
//! helpers as the group's threshold rebuild the member's share for them,
//! without any of them learning it, and without moving the group's funds.
//!
//! A member who lost their device lost their communication key too, so they
//! come back with a new one, and the helpers accept it as theirs. That is the
//! trust decision: helpers should compare the new key's safety code with the
//! person first. As many members as can spend have to agree, the same trust
//! the wallet rests on.
//!
//! Repair rebuilds the *same* share. If a device may have been stolen rather
//! than lost, repairing doesn't shut the thief out: the group should move its
//! funds to a new group instead.
//!
//! The exchange, over a session marked [`REPAIR_SESSION`] holding the helpers
//! and the member's new key:
//! 1. The helper who starts sends everyone a [`RepairMsg::Request`]: which
//!    member, their new key, and the helpers.
//! 2. Each helper sends every other helper a [`RepairMsg::Delta`] (step 1).
//! 3. Each helper sums what it got and sends the member a
//!    [`RepairMsg::Share`] (step 2) with the group's data: what the member's
//!    app needs to rejoin, its viewing seed included.
//! 4. The member's app sums those (step 3), checks the share against the
//!    group's public verifying share for them, and answers [`RepairMsg::Done`].

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use frost_client::api::{self, PublicKey, Uuid};
use frost_client::cipher::{Cipher, PrivateKey};
use frost_core::keys::{repairable, KeyPackage, PublicKeyPackage, SecretShare, VerifiableSecretSharingCommitment, VerifyingShare};
use frost_core::{Ciphersuite, Identifier, Scalar};
use frost_ed25519::Ed25519Sha512;
use rand::rngs::OsRng;
use reddsa::frost::redpallas::PallasBlake2b512;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::error::CoreError;
use crate::transport::{FrostdClient, ServerTrust};

/// The `message_count` marking a session that repairs a member's share (see
/// [`crate::viewing::NEW_VIEWING_KEY_SESSION`] for why sessions are marked).
pub const REPAIR_SESSION: u8 = 0xCA;

/// How long a repair waits for everyone before giving up.
pub const REPAIR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// What the member being repaired pastes into their new app: where the
/// repair runs, and what it's for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairCode {
    /// `host:port` of the frostd server, and its certificate when it isn't
    /// publicly trusted.
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_pem: Option<String>,
    pub session_id: Uuid,
    /// The group's name and the member's, to show before joining.
    pub group: String,
    pub member: String,
}

const CODE_PREFIX: &str = "cyzerepair1";
const MAX_CODE_LEN: usize = 16 * 1024;

impl RepairCode {
    pub fn encode(&self) -> Result<String, CoreError> {
        use base64::Engine as _;
        let json = serde_json::to_vec(self).map_err(|e| CoreError::Config(e.to_string()))?;
        Ok(format!("{CODE_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)))
    }

    /// Read a repair code as pasted: surrounding text and line breaks a chat
    /// app added are ignored.
    pub fn decode(text: &str) -> Result<RepairCode, CoreError> {
        use base64::Engine as _;
        let invalid = || CoreError::Config(format!("this isn't a repair code: it should start with {CODE_PREFIX}"));
        if text.len() > MAX_CODE_LEN {
            return Err(invalid());
        }
        let start = text.find(CODE_PREFIX).ok_or_else(invalid)?;
        // A wrapped code arrives in whitespace-separated pieces, possibly
        // followed by more words; use the longest run of pieces that decodes.
        let rest = &text[start + CODE_PREFIX.len()..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c.is_whitespace()))
            .unwrap_or(rest.len());
        let pieces: Vec<&str> = rest[..end].split_whitespace().collect();
        let code = (1..=pieces.len())
            .rev()
            .find_map(|n| {
                let json = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(pieces[..n].concat()).ok()?;
                serde_json::from_slice::<RepairCode>(&json).ok()
            })
            .ok_or_else(invalid)?;
        if code.server.trim().is_empty() || code.server.contains(['/', ' ', '@']) {
            return Err(CoreError::Config("this repair code's server address is malformed".into()));
        }
        Ok(code)
    }
}

/// What a repaired member's app needs of the group besides their share: what
/// its other members keep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupData {
    pub group_id: String,
    pub description: String,
    pub ciphersuite: String,
    pub threshold: u16,
    /// The public key package, hex.
    pub public_key_package: String,
    pub server_url: Option<String>,
    /// The members: identifier (hex) to communication key (hex), the repaired
    /// member's new key included.
    pub members: BTreeMap<String, String>,
    /// The group's viewing seed (hex), for a wallet group that has one.
    #[serde(default)]
    pub viewing_seed: Option<String>,
    /// Where the group's wallet starts scanning, by network, so a repaired
    /// member's wallet doesn't miss what came before.
    #[serde(default)]
    pub birthdays: BTreeMap<String, u64>,
}

/// A message of a repair.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cyze_repair", rename_all = "snake_case")]
enum RepairMsg {
    /// What's being repaired, from the helper who started.
    Request {
        group_id: String,
        /// The member's identifier (hex), and their old and new keys (hex).
        identifier: String,
        member: String,
        new_key: String,
        /// Every helper's communication key (hex).
        helpers: Vec<String>,
    },
    /// Step 1, for one other helper.
    Delta { value: String },
    /// Step 2, for the member, with the group's data.
    Share { group: Box<GroupData>, sigma: String },
    /// The member's app checked the rebuilt share.
    Done { ok: bool },
}

/// Progress of a repair, for either side.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum RepairEvent {
    Connecting,
    SessionReady { session_id: Uuid },
    /// What a helper is asked to help with, awaiting their approval.
    AwaitingApproval { group_id: String, identifier: String, member: String, new_key: String },
    /// These (hex communication keys) haven't sent their part yet.
    Waiting { members: Vec<String> },
    /// This helper sent the member its part.
    ShareSent,
}

/// Step 1 for helper `key_package`: what to send each helper (this one
/// included) to help repair `participant`'s share.
pub fn deltas<C: Ciphersuite>(
    helpers: &[Identifier<C>],
    key_package: &KeyPackage<C>,
    participant: Identifier<C>,
) -> Result<BTreeMap<Identifier<C>, Scalar<C>>, CoreError> {
    // Step 1 reads only the share's identifier and value.
    let commitment = VerifiableSecretSharingCommitment::<C>::deserialize(Vec::<Vec<u8>>::new())
        .map_err(|e| CoreError::Ceremony(e.to_string()))?;
    let share = SecretShare::new(*key_package.identifier(), *key_package.signing_share(), commitment);
    repairable::repair_share_step_1(helpers, &share, &mut OsRng, participant).map_err(|e| CoreError::Ceremony(e.to_string()))
}

/// Step 3: the member's key package from every helper's step-2 sum, checked
/// against the group's public verifying share for them. Err if it doesn't
/// match, as it won't if any helper sent something wrong.
pub fn rebuild<C: Ciphersuite>(
    sigmas: &[Scalar<C>],
    identifier: Identifier<C>,
    public_key_package: &PublicKeyPackage<C>,
    threshold: u16,
) -> Result<KeyPackage<C>, CoreError> {
    let commitment = VerifiableSecretSharingCommitment::<C>::deserialize(Vec::<Vec<u8>>::new())
        .map_err(|e| CoreError::Ceremony(e.to_string()))?;
    let share = repairable::repair_share_step_3(sigmas, identifier, &commitment);
    let verifying_share = VerifyingShare::from(*share.signing_share());
    let expected = public_key_package
        .verifying_shares()
        .get(&identifier)
        .ok_or_else(|| CoreError::Ceremony("the group has no member with that identifier".into()))?;
    if verifying_share != *expected {
        return Err(CoreError::Ceremony(
            "the rebuilt share doesn't match the group's record of it, so it wasn't saved".into(),
        ));
    }
    Ok(KeyPackage::new(identifier, *share.signing_share(), verifying_share, *public_key_package.verifying_key(), threshold))
}

fn scalar_hex<C: Ciphersuite>(s: &Scalar<C>) -> String {
    hex::encode(<<C::Group as frost_core::Group>::Field as frost_core::Field>::serialize(s).as_ref())
}

fn scalar_from_hex<C: Ciphersuite>(text: &str) -> Result<Scalar<C>, CoreError> {
    let bad = || CoreError::Ceremony("a helper sent a malformed value".into());
    let bytes = hex::decode(text).map_err(|_| bad())?;
    let serialized = bytes.try_into().map_err(|_| bad())?;
    <<C::Group as frost_core::Group>::Field as frost_core::Field>::deserialize(&serialized).map_err(|_| bad())
}

fn identifier_from_hex<C: Ciphersuite>(text: &str) -> Result<Identifier<C>, CoreError> {
    let bytes = hex::decode(text).map_err(|_| CoreError::Ceremony("a malformed identifier".into()))?;
    Identifier::<C>::deserialize(&bytes).map_err(|e| CoreError::Ceremony(e.to_string()))
}

fn pubkey_hex(p: &PublicKey) -> String {
    hex::encode(&p.0)
}

/// Where a repair runs, and who this device is in it.
pub struct Connection {
    /// `host:port` of the frostd server.
    pub server_url: String,
    pub trust: ServerTrust,
    pub comm_privkey: PrivateKey,
    pub comm_pubkey: PublicKey,
}

/// Start a repair session: the helpers and the member's new key in it.
pub async fn create_session(conn: &Connection, helpers: &[PublicKey], new_key: &PublicKey) -> Result<Uuid, CoreError> {
    let mut client = FrostdClient::new(format!("https://{}", conn.server_url), &conn.trust)?;
    client.login(&conn.comm_privkey, &conn.comm_pubkey).await?;
    let mut pubkeys = helpers.to_vec();
    pubkeys.push(new_key.clone());
    let session_id =
        client.create_new_session(&api::CreateNewSessionArgs { pubkeys, message_count: REPAIR_SESSION }).await;
    let _ = client.logout().await;
    Ok(session_id?.session_id)
}

/// A helper's part in a repair.
pub struct HelperParams {
    pub conn: Connection,
    pub session_id: Uuid,
    /// This group as this helper keeps it, its key package included. For the
    /// helper who started the session, `start` says what to repair.
    pub group: GroupData,
    pub key_package: Vec<u8>,
    pub start: Option<Start>,
}

/// What the helper who starts a repair asks for.
pub struct Start {
    /// The member's identifier (hex) and new key.
    pub identifier: String,
    pub new_key: PublicKey,
    /// Every helper, this one included.
    pub helpers: Vec<PublicKey>,
}

/// How a repair went, for a helper: the member's identifier and new key, and
/// whether their app checked the rebuilt share.
#[derive(Debug, Clone)]
pub struct HelperOutcome {
    pub identifier: String,
    pub new_key: PublicKey,
}

/// Take a helper's part. A helper who didn't start the session is shown the
/// request (an [`RepairEvent::AwaitingApproval`]) and goes on only once
/// `approval` says yes. Returns once the member's app confirmed the share.
pub async fn help(
    params: HelperParams,
    events: mpsc::Sender<RepairEvent>,
    approval: oneshot::Receiver<bool>,
    cancel: CancellationToken,
) -> Result<HelperOutcome, CoreError> {
    match params.group.ciphersuite.as_str() {
        id if id == Ed25519Sha512::ID => help_generic::<Ed25519Sha512>(params, events, approval, cancel).await,
        id if id == PallasBlake2b512::ID => help_generic::<PallasBlake2b512>(params, events, approval, cancel).await,
        other => Err(CoreError::Config(format!("can't repair a {other} group"))),
    }
}

async fn help_generic<C: Ciphersuite>(
    params: HelperParams,
    events: mpsc::Sender<RepairEvent>,
    approval: oneshot::Receiver<bool>,
    cancel: CancellationToken,
) -> Result<HelperOutcome, CoreError> {
    let conn = &params.conn;
    let key_package = KeyPackage::<C>::deserialize(&params.key_package).map_err(|e| CoreError::Ceremony(e.to_string()))?;
    let members: HashMap<String, PublicKey> = params
        .group
        .members
        .iter()
        .map(|(id, key)| Ok((id.clone(), PublicKey(hex::decode(key).map_err(|_| CoreError::Config("a malformed member key".into()))?))))
        .collect::<Result<_, CoreError>>()?;
    let member_keys: HashSet<&PublicKey> = members.values().collect();

    let mut client = FrostdClient::new(format!("https://{}", conn.server_url), &conn.trust)?;
    let _ = events.send(RepairEvent::Connecting).await;
    client.login(&conn.comm_privkey, &conn.comm_pubkey).await?;
    let session_id = params.session_id;
    let result = async {
        let info = client.get_session_info(&api::GetSessionInfoArgs { session_id }).await?;
        if info.message_count != REPAIR_SESSION {
            return Err(CoreError::Ceremony("this session isn't for repairing a share".into()));
        }
        let _ = events.send(RepairEvent::SessionReady { session_id }).await;
        let mut cipher = Cipher::new(conn.comm_privkey.clone(), info.pubkeys.clone()).map_err(|e| CoreError::Crypto(e.to_string()))?;
        let mut pending = VecDeque::new();
        let mut io = crate::dkg::SessionIo { client: &client, session_id, cipher: &mut cipher, cancel: &cancel, pending: &mut pending };

        // What's being repaired: as this helper asked, or as the one who did.
        let (identifier, new_key, helpers) = match &params.start {
            Some(start) => {
                let request = RepairMsg::Request {
                    group_id: params.group.group_id.clone(),
                    identifier: start.identifier.clone(),
                    member: members.get(&start.identifier).map(pubkey_hex).unwrap_or_default(),
                    new_key: pubkey_hex(&start.new_key),
                    helpers: start.helpers.iter().map(pubkey_hex).collect(),
                };
                let others: Vec<PublicKey> = info.pubkeys.iter().filter(|p| **p != conn.comm_pubkey).cloned().collect();
                io.send_to(&others, &encode(&request)?).await?;
                (start.identifier.clone(), start.new_key.clone(), start.helpers.clone())
            }
            None => {
                let from = info.coordinator_pubkey.clone();
                let request = loop {
                    if let Some(msg) = take(io.pending, |sender, m| *sender == from && matches!(m, RepairMsg::Request { .. })) {
                        break msg;
                    }
                    io.receive_more().await?;
                };
                let RepairMsg::Request { group_id, identifier, member, new_key, helpers } = request else { unreachable!() };
                let helpers: Vec<PublicKey> = helpers
                    .iter()
                    .map(|h| hex::decode(h).map(PublicKey).map_err(|_| CoreError::Ceremony("a malformed helper key".into())))
                    .collect::<Result<_, _>>()?;
                let new_key = PublicKey(hex::decode(&new_key).map_err(|_| CoreError::Ceremony("a malformed new key".into()))?);
                // Exactly the named helpers and the new key, all helpers members,
                // at least the threshold of them, and the member not among them.
                let in_session: HashSet<&PublicKey> = info.pubkeys.iter().collect();
                let expected: HashSet<&PublicKey> = helpers.iter().chain(std::iter::once(&new_key)).collect();
                let old_key = members.get(&identifier);
                let ok = group_id == params.group.group_id
                    && in_session == expected
                    && helpers.iter().all(|h| member_keys.contains(h))
                    && helpers.contains(&conn.comm_pubkey)
                    && helpers.contains(&from)
                    && !member_keys.contains(&new_key)
                    && helpers.len() >= usize::from(params.group.threshold)
                    && old_key.is_some_and(|k| pubkey_hex(k) == member && !helpers.contains(k));
                if !ok {
                    return Err(CoreError::Ceremony("this repair request doesn't match the group, so it was refused".into()));
                }
                let _ = events
                    .send(RepairEvent::AwaitingApproval { group_id, identifier: identifier.clone(), member, new_key: pubkey_hex(&new_key) })
                    .await;
                (identifier, new_key, helpers)
            }
        };
        let approved = tokio::select! {
            _ = cancel.cancelled() => return Err(CoreError::Cancelled),
            r = approval => r.unwrap_or(false),
        };
        if !approved {
            return Err(CoreError::Ceremony("you declined to help with this repair".into()));
        }

        // Step 1: a value for every helper, this one's kept.
        let identifier_of = |key: &PublicKey| {
            members
                .iter()
                .find(|(_, k)| *k == key)
                .map(|(id, _)| id.clone())
                .ok_or_else(|| CoreError::Ceremony("a helper isn't a member".into()))
        };
        let helper_ids: Vec<Identifier<C>> =
            helpers.iter().map(|h| identifier_from_hex::<C>(&identifier_of(h)?)).collect::<Result<_, _>>()?;
        let participant = identifier_from_hex::<C>(&identifier)?;
        let mut deltas = deltas::<C>(&helper_ids, &key_package, participant)?;
        let own = deltas.remove(key_package.identifier()).ok_or_else(|| CoreError::Ceremony("no value for this helper".into()))?;
        for helper in helpers.iter().filter(|h| **h != conn.comm_pubkey) {
            let id = identifier_from_hex::<C>(&identifier_of(helper)?)?;
            let value = deltas.get(&id).ok_or_else(|| CoreError::Ceremony("no value for a helper".into()))?;
            io.send_to(std::slice::from_ref(helper), &encode(&RepairMsg::Delta { value: scalar_hex::<C>(value) })?).await?;
        }

        // Step 2: this helper's sum, for the member, with the group's data.
        let others: Vec<PublicKey> = helpers.iter().filter(|h| **h != conn.comm_pubkey).cloned().collect();
        let mut received: HashMap<PublicKey, Scalar<C>> = HashMap::new();
        while received.len() < others.len() {
            while let Some(msg) = take_from(io.pending, &others, |m| matches!(m, RepairMsg::Delta { .. })) {
                let (sender, RepairMsg::Delta { value }) = msg else { unreachable!() };
                received.insert(sender, scalar_from_hex::<C>(&value)?);
            }
            if received.len() < others.len() {
                let missing: Vec<String> = others.iter().filter(|p| !received.contains_key(*p)).map(pubkey_hex).collect();
                let _ = events.try_send(RepairEvent::Waiting { members: missing });
                io.receive_more().await?;
            }
        }
        let mut parts: Vec<Scalar<C>> = received.into_values().collect();
        parts.push(own);
        let sigma = repairable::repair_share_step_2::<C>(&parts);
        let mut group = params.group.clone();
        group.members.insert(identifier.clone(), pubkey_hex(&new_key));
        io.send_to(std::slice::from_ref(&new_key), &encode(&RepairMsg::Share { group: Box::new(group), sigma: scalar_hex::<C>(&sigma) })?)
            .await?;
        let _ = events.send(RepairEvent::ShareSent).await;

        // The member's app checks it.
        let done = loop {
            if let Some((_, RepairMsg::Done { ok })) = take_from(io.pending, std::slice::from_ref(&new_key), |m| matches!(m, RepairMsg::Done { .. })) {
                break ok;
            }
            io.receive_more().await?;
        };
        if !done {
            return Err(CoreError::Ceremony("the member's app couldn't rebuild the share, so nothing changed".into()));
        }
        Ok(HelperOutcome { identifier, new_key })
    };
    let result = tokio::select! {
        r = result => r,
        _ = tokio::time::sleep(REPAIR_TIMEOUT) => Err(CoreError::Ceremony(
            "the repair didn't finish within 30 minutes. Try again when everyone has the app open.".into(),
        )),
    };
    // The helper who started keeps the session open a while after finishing,
    // so the other helpers can still collect the member's confirmation; on
    // failure it closes it so they stop waiting.
    match (&result, params.start.is_some()) {
        (Ok(_), true) => {
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
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

/// What a repaired member's app ends up with: the group, and its key package.
#[derive(Debug, Clone)]
pub struct Repaired {
    pub group: GroupData,
    pub key_package: Vec<u8>,
}

/// The repaired member's part: join `session_id`, take every helper's part,
/// rebuild the share, check it, and tell the helpers.
pub async fn receive(conn: Connection, session_id: Uuid, events: mpsc::Sender<RepairEvent>, cancel: CancellationToken) -> Result<Repaired, CoreError> {
    let mut client = FrostdClient::new(format!("https://{}", conn.server_url), &conn.trust)?;
    let _ = events.send(RepairEvent::Connecting).await;
    client.login(&conn.comm_privkey, &conn.comm_pubkey).await?;
    let result = async {
        let info = client.get_session_info(&api::GetSessionInfoArgs { session_id }).await?;
        if info.message_count != REPAIR_SESSION || !info.pubkeys.contains(&conn.comm_pubkey) {
            return Err(CoreError::Ceremony("this isn't a repair of your share".into()));
        }
        let _ = events.send(RepairEvent::SessionReady { session_id }).await;
        let helpers: Vec<PublicKey> = info.pubkeys.iter().filter(|p| **p != conn.comm_pubkey).cloned().collect();
        let mut cipher = Cipher::new(conn.comm_privkey.clone(), info.pubkeys.clone()).map_err(|e| CoreError::Crypto(e.to_string()))?;
        let mut pending = VecDeque::new();
        let mut io = crate::dkg::SessionIo { client: &client, session_id, cipher: &mut cipher, cancel: &cancel, pending: &mut pending };

        let coordinator = info.coordinator_pubkey.clone();
        let identifier = loop {
            if let Some((_, RepairMsg::Request { identifier, new_key, helpers: named, .. })) =
                take_from(io.pending, std::slice::from_ref(&coordinator), |m| matches!(m, RepairMsg::Request { .. }))
            {
                let named: HashSet<String> = named.into_iter().collect();
                let actual: HashSet<String> = helpers.iter().map(pubkey_hex).collect();
                if new_key != pubkey_hex(&conn.comm_pubkey) || named != actual {
                    return Err(CoreError::Ceremony("this repair names someone else, so it was refused".into()));
                }
                break identifier;
            }
            io.receive_more().await?;
        };
        let mut shares: HashMap<PublicKey, (GroupData, String)> = HashMap::new();
        while shares.len() < helpers.len() {
            while let Some((sender, msg)) = take_from(io.pending, &helpers, |m| matches!(m, RepairMsg::Share { .. })) {
                let RepairMsg::Share { group, sigma } = msg else { unreachable!() };
                shares.insert(sender, (*group, sigma));
            }
            if shares.len() < helpers.len() {
                let missing: Vec<String> = helpers.iter().filter(|p| !shares.contains_key(*p)).map(pubkey_hex).collect();
                let _ = events.try_send(RepairEvent::Waiting { members: missing });
                io.receive_more().await?;
            }
        }
        let outcome = rebuild_from(&shares, &identifier, &conn.comm_pubkey);
        let done = encode(&RepairMsg::Done { ok: outcome.is_ok() })?;
        io.send_to(&helpers, &done).await?;
        outcome
    };
    let result = tokio::select! {
        r = result => r,
        _ = tokio::time::sleep(REPAIR_TIMEOUT) => Err(CoreError::Ceremony(
            "the helpers didn't all send their part within 30 minutes. Ask them to try again.".into(),
        )),
    };
    let _ = client.logout().await;
    result
}

/// The member's key package from the helpers' parts, which must all describe
/// the same group, naming this device's key as the member's. Where the
/// group's wallet starts scanning is each helper's own record, which can
/// differ a little: the earliest is kept, which never misses anything.
fn rebuild_from(shares: &HashMap<PublicKey, (GroupData, String)>, identifier: &str, me: &PublicKey) -> Result<Repaired, CoreError> {
    let without_birthdays = |g: &GroupData| GroupData { birthdays: BTreeMap::new(), ..g.clone() };
    let mut groups = shares.values().map(|(g, _)| g);
    let mut group = groups.next().ok_or_else(|| CoreError::Ceremony("no helper sent their part".into()))?.clone();
    if groups.any(|g| without_birthdays(g) != without_birthdays(&group)) {
        return Err(CoreError::Ceremony("the helpers described the group differently, so nothing was saved".into()));
    }
    for (g, _) in shares.values() {
        for (key, height) in &g.birthdays {
            let earliest = group.birthdays.entry(key.clone()).or_insert(*height);
            *earliest = (*earliest).min(*height);
        }
    }
    if group.members.get(identifier) != Some(&pubkey_hex(me)) {
        return Err(CoreError::Ceremony("the helpers' group doesn't name your new key, so nothing was saved".into()));
    }
    let key_package = match group.ciphersuite.as_str() {
        id if id == Ed25519Sha512::ID => rebuild_package::<Ed25519Sha512>(shares, identifier, &group)?,
        id if id == PallasBlake2b512::ID => rebuild_package::<PallasBlake2b512>(shares, identifier, &group)?,
        other => return Err(CoreError::Config(format!("can't repair a {other} group"))),
    };
    Ok(Repaired { group, key_package })
}

fn rebuild_package<C: Ciphersuite>(
    shares: &HashMap<PublicKey, (GroupData, String)>,
    identifier: &str,
    group: &GroupData,
) -> Result<Vec<u8>, CoreError> {
    let pkp_bytes = hex::decode(&group.public_key_package).map_err(|_| CoreError::Ceremony("a malformed public key package".into()))?;
    let pkp = PublicKeyPackage::<C>::deserialize(&pkp_bytes).map_err(|e| CoreError::Ceremony(e.to_string()))?;
    let key = pkp.verifying_key().serialize().map_err(|e| CoreError::Ceremony(e.to_string()))?;
    if hex::encode(key) != group.group_id {
        return Err(CoreError::Ceremony("the group's key isn't its id, so nothing was saved".into()));
    }
    let sigmas: Vec<Scalar<C>> = shares.values().map(|(_, s)| scalar_from_hex::<C>(s)).collect::<Result<_, _>>()?;
    let package = rebuild::<C>(&sigmas, identifier_from_hex::<C>(identifier)?, &pkp, group.threshold)?;
    package.serialize().map_err(|e| CoreError::Ceremony(e.to_string()))
}

fn encode(m: &RepairMsg) -> Result<Vec<u8>, CoreError> {
    serde_json::to_vec(m).map_err(|e| CoreError::Ceremony(e.to_string()))
}

/// Take the first message from `from` matching `want` out of `pending`.
fn take_from(
    pending: &mut VecDeque<api::Msg>,
    from: &[PublicKey],
    want: impl Fn(&RepairMsg) -> bool,
) -> Option<(PublicKey, RepairMsg)> {
    let at = pending.iter().position(|m| {
        from.contains(&m.sender) && serde_json::from_slice::<RepairMsg>(&m.msg).is_ok_and(|parsed| want(&parsed))
    })?;
    let msg = pending.remove(at)?;
    serde_json::from_slice(&msg.msg).ok().map(|parsed| (msg.sender, parsed))
}

/// [`take_from`] for a message from one sender, judged by sender and message.
fn take(pending: &mut VecDeque<api::Msg>, want: impl Fn(&PublicKey, &RepairMsg) -> bool) -> Option<RepairMsg> {
    let at = pending
        .iter()
        .position(|m| serde_json::from_slice::<RepairMsg>(&m.msg).is_ok_and(|parsed| want(&m.sender, &parsed)))?;
    let msg = pending.remove(at)?;
    serde_json::from_slice(&msg.msg).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use frost_core::keys::{generate_with_dealer, IdentifierList};

    /// A 2-of-3 group from a dealer: every member's key package, and the
    /// group's public key package.
    fn group<C: Ciphersuite>() -> (BTreeMap<Identifier<C>, KeyPackage<C>>, PublicKeyPackage<C>) {
        let (shares, pkp) = generate_with_dealer::<C, _>(3, 2, IdentifierList::Default, &mut OsRng).unwrap();
        let packages = shares.into_iter().map(|(id, s)| (id, KeyPackage::try_from(s).unwrap())).collect();
        (packages, pkp)
    }

    fn repair<C: Ciphersuite>() {
        let (packages, pkp) = group::<C>();
        let ids: Vec<Identifier<C>> = packages.keys().copied().collect();
        let (lost, helpers) = (ids[2], &ids[..2]);
        // Each helper's step 1, and each helper's step 2 from what it got.
        let all: Vec<BTreeMap<Identifier<C>, Scalar<C>>> =
            helpers.iter().map(|h| deltas::<C>(helpers, &packages[h], lost).unwrap()).collect();
        let sigmas: Vec<Scalar<C>> = helpers
            .iter()
            .map(|h| repairable::repair_share_step_2::<C>(&all.iter().map(|d| d[h]).collect::<Vec<_>>()))
            .collect();
        let rebuilt = rebuild::<C>(&sigmas, lost, &pkp, 2).unwrap();
        assert_eq!(rebuilt, packages[&lost], "the same share, and the rest of the key package");

        // A helper sending something else: the share doesn't match, and isn't kept.
        let wrong = vec![sigmas[0], sigmas[0]];
        assert!(rebuild::<C>(&wrong, lost, &pkp, 2).is_err());
    }

    #[test]
    fn helpers_rebuild_a_lost_share_that_matches_the_groups_record_of_it() {
        repair::<PallasBlake2b512>();
        repair::<Ed25519Sha512>();
    }

    #[test]
    fn a_repair_code_survives_being_pasted_through_a_chat_app() {
        let code = RepairCode {
            server: "frost.example.com:2744".into(),
            cert_pem: None,
            session_id: Uuid::new_v4(),
            group: "Treasury".into(),
            member: "Carol".into(),
        };
        let text = code.encode().unwrap();
        let (a, b) = text.split_at(30);
        assert_eq!(RepairCode::decode(&format!("Here it is: {a}\n{b} thanks")).unwrap(), code);
        assert!(RepairCode::decode("zffrost1notarepaircode").is_err());
    }

    #[test]
    fn a_repaired_member_keeps_nothing_unless_every_helper_describes_the_same_group_naming_their_key() {
        let me = PublicKey(vec![7; 32]);
        let group = GroupData {
            group_id: "g".into(),
            description: "Treasury".into(),
            ciphersuite: PallasBlake2b512::ID.into(),
            threshold: 2,
            public_key_package: String::new(),
            server_url: None,
            members: BTreeMap::from([("03".into(), hex::encode(&me.0))]),
            viewing_seed: None,
            birthdays: BTreeMap::new(),
        };
        let other = GroupData { description: "Not the treasury".into(), ..group.clone() };
        let shares = HashMap::from([(PublicKey(vec![1; 32]), (group.clone(), "00".into())), (PublicKey(vec![2; 32]), (other, "00".into()))]);
        assert!(rebuild_from(&shares, "03", &me).unwrap_err().to_string().contains("described the group differently"));
        let shares = HashMap::from([(PublicKey(vec![1; 32]), (group.clone(), "00".into()))]);
        assert!(rebuild_from(&shares, "03", &PublicKey(vec![8; 32])).unwrap_err().to_string().contains("doesn't name your new key"));
        // Helpers' wallet start heights may differ; that alone isn't a different group.
        let later = GroupData { birthdays: BTreeMap::from([("main/g".into(), 200)]), ..group.clone() };
        let earlier = GroupData { birthdays: BTreeMap::from([("main/g".into(), 100)]), ..group };
        let shares = HashMap::from([(PublicKey(vec![1; 32]), (later, "00".into())), (PublicKey(vec![2; 32]), (earlier, "00".into()))]);
        let err = rebuild_from(&shares, "03", &me).unwrap_err().to_string();
        assert!(!err.contains("described the group differently"), "{err}");
    }
}
