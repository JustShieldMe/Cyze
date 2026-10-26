//! Co-signer verification of a send, before its signature is approved.
//!
//! A coordinator asks the group to sign a transaction's sighash. Signers used to
//! see the coordinator's own description of the payment (recipient, amount,
//! fee) next to an opaque hash, with nothing tying the two together: a dishonest
//! coordinator could describe "1 ZEC to Alice" while the hash spent the treasury
//! elsewhere, and every honest approval would help it.
//!
//! Now the coordinator also attaches the unsigned PCZT, and each signer:
//!
//! 1. recomputes the sighash from it and requires it to equal the message being
//!    signed, so the signature can only ever authorize this transaction;
//! 2. checks that every input carrying value is one of the group's own notes
//!    (nullifier and `rk` against the group's full viewing key);
//! 3. checks every output's recipient and amount against its note commitment
//!    and value commitment, which makes them facts rather than claims;
//! 4. works out where the money goes (payments, change back to the group, fee)
//!    from those facts, recovering each payment's memo with the group's
//!    outgoing viewing key;
//! 5. requires the randomizer it is asked to sign with to belong to one of the
//!    group's unsigned spends.
//!
//! A group that made its viewing key private (see `frost_app_core::viewing`)
//! may still hold notes under the key derived from its id alone, and moves them
//! with the same spend authority. Spends under either key verify, but only
//! outputs to the private key count as change: anything paid back to the old
//! address is listed as a payment, so it can't quietly go where anyone who
//! knows the group's id can see it.
//!
//! [`check_coordinator_claims`] then compares the coordinator's description
//! with the result, and the app refuses to sign if they disagree.

use base64::Engine as _;
use orchard::keys::{FullViewingKey, Scope};
use orchard::note_encryption::{IronwoodDomain, OrchardDomain};
use pczt::roles::redactor::Redactor;
use pczt::roles::verifier::{OrchardError, TransparentError, Verifier};
use serde::{Deserialize, Serialize};
use zcash_address::unified::{self, Container, Encoding};
use zcash_note_encryption::try_output_recovery_with_ovk;
use zcash_protocol::consensus::NetworkType;

use super::WalletNetwork;
use crate::error::CoreError;
use crate::events::SigningContext;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;


/// A transaction as a co-signer has proven it from its PCZT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedTransaction {
    /// The sighash the signature authorizes, recomputed from the PCZT.
    pub sighash_hex: String,
    /// The last block height the transaction can be mined at.
    pub expiry_height: u32,
    /// Value leaving the group, in output order.
    pub payments: Vec<VerifiedPayment>,
    /// Total value of the group's notes the transaction spends.
    pub spent_zatoshis: u64,
    /// Value returned to the group's own addresses.
    pub change_zatoshis: u64,
    /// What is left for the network: spent minus payments and change.
    pub fee_zatoshis: u64,
    /// It spends notes under the viewing key derived from the group's id: funds
    /// a group that made its viewing key private is moving off its old address.
    #[serde(default)]
    pub from_old_address: bool,
}

/// One payment leaving the group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedPayment {
    /// A unified address with the payment's Orchard receiver, or the
    /// transparent address it pays.
    pub address: String,
    pub value_zatoshis: u64,
    /// Paid to a transparent address: amount and recipient are public.
    pub transparent: bool,
    /// The memo, recovered with the group's outgoing viewing key. `None` if
    /// empty or unrecoverable.
    pub memo: Option<String>,
}

fn fail(msg: impl Into<String>) -> CoreError {
    CoreError::Verification(msg.into())
}

/// The group's Orchard full viewing key, from its id (its `ak`) and its viewing
/// seed if it has one, the same way the wallet derives it.
pub fn group_fvk(group_ak_hex: &str, seed: Option<&crate::zcash::ViewingSeed>) -> Result<FullViewingKey, CoreError> {
    let ak: [u8; 32] = hex::decode(group_ak_hex.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| fail("the group key is not 32 bytes of hex"))?;
    FullViewingKey::from_bytes(&crate::zcash::orchard_fvk_bytes(&ak, seed))
        .ok_or_else(|| fail("the group key is not a valid Orchard spend validating key"))
}

/// Whether `address` is one the group receives at under its current viewing
/// key (any of its diversified addresses): what a member's app checks to say a
/// payment goes to one of their own groups.
pub fn is_address_of(group_ak_hex: &str, seed: Option<&crate::zcash::ViewingSeed>, address: &str) -> bool {
    group_fvk(group_ak_hex, seed).is_ok_and(|fvk| is_group_address(address, &fvk))
}

/// The viewing keys a group's notes can be under: its current one and, for a
/// group with a viewing seed, the one derived from its id alone, which held its
/// funds if it made its viewing key private after it was created.
struct GroupKeys {
    current: FullViewingKey,
    old: Option<FullViewingKey>,
}

impl GroupKeys {
    fn new(group_ak_hex: &str, seed: Option<&crate::zcash::ViewingSeed>) -> Result<Self, CoreError> {
        Ok(Self {
            current: group_fvk(group_ak_hex, seed)?,
            old: seed.map(|_| group_fvk(group_ak_hex, None)).transpose()?,
        })
    }

    fn all(&self) -> impl Iterator<Item = &FullViewingKey> {
        std::iter::once(&self.current).chain(self.old.as_ref())
    }

    /// The key a spent note is under, judged by its recipient address. A note
    /// under neither (someone else's, or a dummy) is checked against the
    /// current key, which refuses the first and ignores the key for the second.
    fn for_spend(&self, spend: &orchard::pczt::Spend) -> &FullViewingKey {
        spend
            .recipient()
            .and_then(|r| self.all().find(|k| k.scope_for_address(&r).is_some()))
            .unwrap_or(&self.current)
    }
}

fn serialize_b64(pczt: pczt::Pczt) -> Result<String, CoreError> {
    let bytes = pczt
        .serialize()
        .map_err(|e| CoreError::Ceremony(format!("serialize pczt: {e:?}")))?;
    Ok(B64.encode(bytes))
}

/// Prepare a draft's PCZT (hex, as `prepare_send` returns it) for co-signers:
/// base64, without the Merkle witnesses only the prover needs. If that would
/// take the request's context over what fits one frostd message
/// ([`crate::signing::signer_context_budget`]), also drop the fields signers
/// can recompute (`cv_net`, `cmx`, decryptable ciphertexts). That is safe: the
/// signer recomputes them, and then requires the sighash they produce to equal
/// the message being signed.
pub fn pczt_for_signers(pczt_hex: &str) -> Result<String, CoreError> {
    pczt_for_signers_beside(pczt_hex, 0, 2)
}

/// [`pczt_for_signers`] for a request to `signers` signers whose context's
/// other fields take `others` bytes.
pub fn pczt_for_signers_beside(pczt_hex: &str, others: usize, signers: usize) -> Result<String, CoreError> {
    pczt_for_signers_within(pczt_hex, crate::signing::signer_context_budget(signers).saturating_sub(others))
}

fn pczt_for_signers_within(pczt_hex: &str, limit: usize) -> Result<String, CoreError> {
    let bytes = hex::decode(pczt_hex.trim())
        .map_err(|e| CoreError::Ceremony(format!("pczt hex: {e}")))?;
    let pczt = pczt::Pczt::parse(&bytes)
        .map_err(|e| CoreError::Ceremony(format!("parse pczt: {e:?}")))?;
    let without_witnesses = Redactor::new(pczt)
        .redact_orchard_with(|mut b| b.redact_actions(|mut a| a.clear_spend_witness()))
        .redact_ironwood_with(|mut b| b.redact_actions(|mut a| a.clear_spend_witness()))
        .finish();
    let encoded = serialize_b64(without_witnesses.clone())?;
    if encoded.len() <= limit {
        return Ok(encoded);
    }
    let compact = Redactor::new(without_witnesses)
        .redact_orchard_with(|mut b| b.compact_resolvable_fields())
        .redact_ironwood_with(|mut b| b.compact_resolvable_fields())
        .finish();
    let encoded = serialize_b64(compact)?;
    if encoded.len() > limit {
        return Err(CoreError::Ceremony(
            "this transaction is too large for co-signers to verify in one request. \
             Make fewer payments at once, or send a smaller amount so it uses fewer notes."
                .into(),
        ));
    }
    Ok(encoded)
}

#[derive(Default)]
struct Totals {
    spent: u64,
    change: u64,
    payments: Vec<VerifiedPayment>,
    /// Randomizers of the group's spends that still need its signature.
    unsigned_alphas: Vec<[u8; 32]>,
    /// Some spent note is under the group's old viewing key.
    from_old_address: bool,
}

#[derive(Clone, Copy)]
enum Pool {
    Orchard,
    Ironwood,
}

/// An Orchard-only unified address for `raw` (43-byte Orchard receiver).
fn orchard_ua(raw: [u8; 43], net: NetworkType) -> Result<String, String> {
    unified::Address::try_from_items(vec![unified::Receiver::Orchard(raw)])
        .map(|ua| ua.encode(&net))
        .map_err(|e| format!("unified address: {e:?}"))
}

fn recover_memo(pool: Pool, action: &orchard::pczt::Action, keys: &GroupKeys) -> Option<String> {
    let out_ciphertext = &action.output().encrypted_note().out_ciphertext;
    let scopes = [Scope::External, Scope::Internal];
    keys.all().flat_map(|fvk| scopes.map(|scope| (fvk, scope))).find_map(|(fvk, scope)| {
        let ovk = fvk.to_ovk(scope);
        let memo = match pool {
            Pool::Orchard => try_output_recovery_with_ovk(
                &OrchardDomain::for_pczt_action(action),
                &ovk,
                action,
                action.cv_net(),
                out_ciphertext,
            )
            .map(|(_, _, memo)| memo),
            Pool::Ironwood => try_output_recovery_with_ovk(
                &IronwoodDomain::for_pczt_action(action),
                &ovk,
                action,
                action.cv_net(),
                out_ciphertext,
            )
            .map(|(_, _, memo)| memo),
        }?;
        super::decode_zcash_memo(&memo)
    })
}

fn check_bundle(
    pool: Pool,
    bundle: &orchard::pczt::Bundle,
    keys: &GroupKeys,
    net: NetworkType,
    totals: &mut Totals,
) -> Result<(), OrchardError<String>> {
    bundle.verify_cross_address_restriction().map_err(OrchardError::Verify)?;
    for action in bundle.actions() {
        let spend = action.spend();
        let output = action.output();
        // The spent note: real (the group's) or a zero-value dummy with its own key.
        let fvk = keys.for_spend(spend);
        spend.verify_nullifier(Some(fvk)).map_err(OrchardError::Verify)?;
        spend.verify_rk(Some(fvk)).map_err(OrchardError::Verify)?;
        // The new note: its recipient and value are what its commitments say.
        output.verify_note_commitment(spend).map_err(OrchardError::Verify)?;
        action.verify_cv_net().map_err(OrchardError::Verify)?;

        let spent = spend.value().map_or(0, |v| v.inner());
        let groups_note = spend.fvk().as_ref().is_none_or(|f| f == fvk)
            && spend.recipient().is_some_and(|r| fvk.scope_for_address(&r).is_some());
        if groups_note {
            totals.spent += spent;
            totals.from_old_address |= spent > 0 && Some(fvk) == keys.old.as_ref();
            if spend.spend_auth_sig().is_none() {
                if let Some(alpha) = spend.alpha() {
                    totals.unsigned_alphas.push(alpha.to_bytes());
                }
            }
        } else if spent > 0 {
            return Err(OrchardError::Custom(
                "it spends a note that doesn't belong to this group".into(),
            ));
        }

        let value = output.value().map_or(0, |v| v.inner());
        let recipient = output
            .recipient()
            .ok_or_else(|| OrchardError::Custom("an output has no recipient".into()))?;
        if keys.current.scope_for_address(&recipient).is_some() {
            totals.change += value;
        } else if value > 0 {
            totals.payments.push(VerifiedPayment {
                address: orchard_ua(recipient.to_raw_address_bytes(), net).map_err(OrchardError::Custom)?,
                value_zatoshis: value,
                transparent: false,
                memo: recover_memo(pool, action, keys),
            });
        }
        // A zero-value output to someone else is bundle padding.
    }
    Ok(())
}

fn describe<E: std::fmt::Debug>(e: OrchardError<E>) -> String {
    use orchard::pczt::VerifyError as V;
    match e {
        OrchardError::Custom(e) => format!("{e:?}").trim_matches('"').to_string(),
        OrchardError::Verify(V::MismatchedFullViewingKey | V::WrongFvkForNote) => {
            "it spends notes that don't belong to this group".into()
        }
        OrchardError::Verify(V::InvalidExtractedNoteCommitment) => {
            "an output's recipient or amount doesn't match its note commitment".into()
        }
        OrchardError::Verify(V::InvalidValueCommitment) => {
            "an amount doesn't match its value commitment".into()
        }
        OrchardError::Verify(e) => format!("its contents don't match its commitments ({e:?})"),
        OrchardError::Parse(e) => format!("it can't be parsed ({e:?})"),
        OrchardError::UnsupportedConsensusBranchId => "it targets an unknown network upgrade".into(),
    }
}

/// Verify a signing request against the PCZT attached to it (see the module
/// docs). `pczt_b64` is what [`pczt_for_signers`] produced; `message_hex` is the
/// message the signing package asks this signer to sign; `randomizer_hex` is
/// the re-randomization it would sign with.
pub fn verify_signing_request(
    network: WalletNetwork,
    group_ak_hex: &str,
    viewing_seed: Option<&crate::zcash::ViewingSeed>,
    pczt_b64: &str,
    message_hex: &str,
    randomizer_hex: Option<&str>,
) -> Result<VerifiedTransaction, CoreError> {
    let keys = GroupKeys::new(group_ak_hex, viewing_seed)?;
    let net = network.network_type();
    let bytes = B64
        .decode(pczt_b64.trim())
        .map_err(|_| fail("the transaction attached to this request isn't valid base64"))?;
    let mut pczt = pczt::Pczt::parse(&bytes)
        .map_err(|e| fail(format!("the attached transaction can't be parsed ({e:?})")))?;
    pczt.resolve_fields()
        .map_err(|e| fail(format!("the attached transaction's fields can't be restored ({e:?})")))?;

    // 1. The signature can only ever authorize this exact transaction.
    let sighash = pczt::roles::signer::Signer::new(pczt.clone())
        .map_err(|e| fail(format!("the attached transaction can't be signed ({e:?})")))?
        .shielded_sighash();
    let message = hex::decode(message_hex.trim()).map_err(|_| fail("the message to sign isn't hex"))?;
    if message.as_slice() != sighash.as_slice() {
        return Err(fail(
            "the message you're asked to sign is not this transaction's sighash",
        ));
    }
    if pczt.sapling().spends().len() + pczt.sapling().outputs().len() > 0 {
        return Err(fail("it moves funds through the Sapling pool, which a group wallet never uses"));
    }

    let expiry_height = *pczt.global().expiry_height();
    let mut totals = Totals::default();
    Verifier::new(pczt)
        .with_orchard(|b| check_bundle(Pool::Orchard, b, &keys, net, &mut totals))
        .map_err(|e| fail(describe(e)))?
        .with_ironwood(|b| check_bundle(Pool::Ironwood, b, &keys, net, &mut totals))
        .map_err(|e| fail(describe(e)))?
        .with_transparent(|b| {
            if !b.inputs().is_empty() {
                return Err(TransparentError::Custom(
                    "it spends transparent funds, which a group wallet never holds",
                ));
            }
            for out in b.outputs() {
                let address = transparent::address::TransparentAddress::from_script_pubkey(out.script_pubkey())
                    .ok_or(TransparentError::Custom("it pays a script that isn't a standard address"))?;
                totals.payments.push(VerifiedPayment {
                    address: address.to_zcash_address(net).encode(),
                    value_zatoshis: u64::from(*out.value()),
                    transparent: true,
                    memo: None,
                });
            }
            Ok(())
        })
        .map_err(|e| match e {
            TransparentError::Custom(m) => fail(m),
            other => fail(format!("its transparent part is invalid ({other:?})")),
        })?;

    // 5. The signature asked for is for one of the group's spends.
    let randomizer = randomizer_hex.ok_or_else(|| fail("a wallet signature must be re-randomized"))?;
    let randomizer = hex::decode(randomizer.trim()).map_err(|_| fail("the randomizer isn't hex"))?;
    if !totals.unsigned_alphas.iter().any(|a| a.as_slice() == randomizer.as_slice()) {
        return Err(fail(
            "the signature requested doesn't belong to any of the group's spends in this transaction",
        ));
    }

    let paid: u64 = totals.payments.iter().map(|p| p.value_zatoshis).sum();
    let fee = totals
        .spent
        .checked_sub(paid + totals.change)
        .ok_or_else(|| fail("it pays out more than the group's notes it spends"))?;
    Ok(VerifiedTransaction {
        sighash_hex: hex::encode(sighash),
        expiry_height,
        payments: totals.payments,
        spent_zatoshis: totals.spent,
        change_zatoshis: totals.change,
        fee_zatoshis: fee,
        from_old_address: totals.from_old_address,
    })
}

/// The Orchard receiver of a unified address, re-encoded as an Orchard-only
/// unified address, or a transparent address as given. Used to compare a
/// coordinator's recipient with a verified payment.
fn canonical_recipient(address: &str, net: NetworkType) -> Option<String> {
    let address = address.trim();
    if let Ok((_, ua)) = unified::Address::decode(address) {
        return ua.items().into_iter().find_map(|r| match r {
            unified::Receiver::Orchard(raw) => orchard_ua(raw, net).ok(),
            _ => None,
        });
    }
    Some(address.to_string())
}

/// The forms an address takes in what members see: as given, and as a
/// verified payment to it reads (its Orchard receiver alone, see
/// [`canonical_recipient`]). So a saved payee is recognised in a payment
/// whichever form the payment shows. Err if it isn't an address on `network`.
pub fn address_forms(network: WalletNetwork, address: &str) -> Result<Vec<String>, CoreError> {
    let given = address.trim();
    crate::wallet::decode_recipient(network, given)?;
    let mut forms = vec![given.to_string()];
    if let Some(canonical) = canonical_recipient(given, network.network_type()).filter(|c| c != given) {
        forms.push(canonical);
    }
    Ok(forms)
}

fn is_group_address(address: &str, fvk: &FullViewingKey) -> bool {
    let Ok((_, ua)) = unified::Address::decode(address.trim()) else {
        return false;
    };
    ua.items().into_iter().any(|r| match r {
        unified::Receiver::Orchard(raw) => Option::<orchard::Address>::from(orchard::Address::from_raw_address_bytes(&raw))
            .is_some_and(|a| fvk.scope_for_address(&a).is_some()),
        _ => false,
    })
}

/// Require the coordinator's description of a send to match what the signer
/// verified: the fee, a single payment of the stated amount to the stated
/// recipient (or none, for a send back to the group), and the stated memo. For
/// a batch, every stated payment, each matching one payment the transaction
/// makes, and no others (outputs are shuffled, so in any order).
pub fn check_coordinator_claims(
    network: WalletNetwork,
    group_ak_hex: &str,
    viewing_seed: Option<&crate::zcash::ViewingSeed>,
    verified: &VerifiedTransaction,
    claimed: &SigningContext,
) -> Result<(), CoreError> {
    let fvk = group_fvk(group_ak_hex, viewing_seed)?;
    let net = network.network_type();
    let zec = |z: u64| format!("{}", z as f64 / 1e8);
    if claimed.fee_zatoshis != verified.fee_zatoshis {
        return Err(fail(format!(
            "the coordinator says the fee is {} but the transaction pays {}",
            zec(claimed.fee_zatoshis),
            zec(verified.fee_zatoshis)
        )));
    }
    if !claimed.payments.is_empty() {
        return check_batch_claims(net, &fvk, verified, &claimed.payments);
    }
    if is_group_address(&claimed.recipient, &fvk) {
        if let Some(p) = verified.payments.first() {
            return Err(fail(format!(
                "the coordinator describes a transfer back to the group, but it pays {} to {}",
                zec(p.value_zatoshis),
                p.address
            )));
        }
        return Ok(());
    }
    let [payment] = verified.payments.as_slice() else {
        return Err(fail(format!(
            "the coordinator describes one payment, but the transaction makes {}",
            verified.payments.len()
        )));
    };
    let same_recipient = canonical_recipient(&claimed.recipient, net).as_deref() == Some(payment.address.as_str());
    if !same_recipient || payment.value_zatoshis != claimed.amount_zatoshis {
        return Err(fail(format!(
            "the coordinator says it pays {} to {}, but it pays {} to {}",
            zec(claimed.amount_zatoshis),
            claimed.recipient.trim(),
            zec(payment.value_zatoshis),
            payment.address
        )));
    }
    if payment.transparent != claimed.is_unshield {
        return Err(fail("the coordinator mislabels whether the payment is public (transparent)"));
    }
    if !payment.transparent && claimed.memo.as_deref().filter(|m| !m.is_empty()) != payment.memo.as_deref() {
        return Err(fail("the memo in the transaction isn't the one the coordinator describes"));
    }
    Ok(())
}

/// [`check_coordinator_claims`] for a batch: each stated payment matches a
/// different payment the transaction makes, and there are no others.
fn check_batch_claims(
    net: NetworkType,
    fvk: &FullViewingKey,
    verified: &VerifiedTransaction,
    claimed: &[crate::events::ClaimedPayment],
) -> Result<(), CoreError> {
    let zec = |z: u64| format!("{}", z as f64 / 1e8);
    if claimed.len() != verified.payments.len() {
        return Err(fail(format!(
            "the coordinator describes {} payments, but the transaction makes {}",
            claimed.len(),
            verified.payments.len()
        )));
    }
    let mut unmatched: Vec<&VerifiedPayment> = verified.payments.iter().collect();
    for c in claimed {
        if is_group_address(&c.recipient, fvk) {
            return Err(fail("the coordinator describes a payment back to the group among others"));
        }
        let address = canonical_recipient(&c.recipient, net);
        let found = unmatched.iter().position(|p| {
            Some(p.address.as_str()) == address.as_deref()
                && p.value_zatoshis == c.amount_zatoshis
                && p.transparent == c.transparent
        });
        match found {
            Some(i) => {
                unmatched.swap_remove(i);
            }
            None => {
                return Err(fail(format!(
                    "the coordinator says it pays {} to {}, but the transaction makes no such payment",
                    zec(c.amount_zatoshis),
                    c.recipient.trim(),
                )))
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet::RegtestUpgrade;
    use incrementalmerkletree::Retention;
    use orchard::keys::SpendingKey;
    use orchard::tree::MerkleHashOrchard;
    use orchard::value::NoteValue;
    use pczt::roles::{creator::Creator, io_finalizer::IoFinalizer, signer::Signer};
    use shardtree::{store::memory::MemoryShardStore, ShardTree};
    use zcash_primitives::transaction::builder::{BuildConfig, Builder, BundlePadding, PcztResult};
    use zcash_primitives::transaction::fees::zip317;
    use zcash_protocol::memo::{Memo, MemoBytes};
    use zcash_protocol::value::Zatoshis;

    const NOTE: u64 = 1_000_000;
    const DEVNET: WalletNetwork = WalletNetwork::Regtest(RegtestUpgrade::Nu6_3);

    /// A FROST group as Cyze sees it: an id (`ak`) and the viewing key derived
    /// from it.
    fn group(seed: u8) -> (String, FullViewingKey) {
        let sk = SpendingKey::from_bytes([seed; 32]).unwrap();
        let ak_hex = hex::encode(&FullViewingKey::from(&sk).to_bytes()[..32]);
        let fvk = group_fvk(&ak_hex, None).unwrap();
        (ak_hex, fvk)
    }

    fn payee() -> orchard::Address {
        let sk = SpendingKey::from_bytes([9; 32]).unwrap();
        FullViewingKey::from(&sk).address_at(0u32, Scope::External)
    }

    #[test]
    fn an_address_is_recognised_as_the_group_it_belongs_to() {
        let (ak, fvk) = group(1);
        let net = DEVNET.network_type();
        let rotated = crate::zcash::derive_orchard_address_at(&ak, None, net, 7).unwrap();
        for address in [ua(fvk.address_at(0u32, Scope::External)), rotated] {
            assert!(is_address_of(&ak, None, &address));
            assert!(!is_address_of(&group(2).0, None, &address), "another group's");
        }
        let seed = [5u8; 32];
        let private = crate::zcash::derive_orchard_keys_hex(&ak, Some(&seed), net).unwrap().address;
        assert!(is_address_of(&ak, Some(&seed), &private));
        assert!(!is_address_of(&ak, None, &private), "not under the key derived from its id alone");
        assert!(!is_address_of(&ak, None, &ua(payee())));
        assert!(!is_address_of(&ak, None, "tmNotAUnifiedAddress"));
    }

    fn ua(addr: orchard::Address) -> String {
        orchard_ua(addr.to_raw_address_bytes(), DEVNET.network_type()).unwrap()
    }

    struct Draft {
        pczt_hex: String,
        sighash_hex: String,
        alpha_hex: String,
        fee: u64,
    }

    /// A note of `NOTE` zatoshis the group received in `pool`: a legacy
    /// Orchard note (V2) or an Ironwood note (V3).
    fn received_note(pool: Pool, fvk: &FullViewingKey) -> orchard::Note {
        use orchard::note::{NoteVersion, RandomSeed, Rho};
        use rand10::RngExt as _;
        let mut rng = super::super::wallet_rng();
        let version = match pool {
            Pool::Orchard => NoteVersion::V2,
            Pool::Ironwood => NoteVersion::V3,
        };
        loop {
            let mut rho = rng.random::<[u8; 32]>();
            rho[31] &= 0x3f; // below the Pallas base field modulus
            let Some(rho) = Option::<Rho>::from(Rho::from_bytes(&rho)) else { continue };
            let Some(rseed) = Option::<RandomSeed>::from(RandomSeed::from_bytes(rng.random(), &rho)) else {
                continue;
            };
            let note = orchard::Note::from_parts(
                fvk.address_at(0u32, Scope::External),
                NoteValue::from_raw(NOTE),
                rho,
                rseed,
                version,
            );
            if let Some(note) = Option::<orchard::Note>::from(note) {
                return note;
            }
        }
    }

    /// Build an unsigned send of `pay` (with `memo`) from a note the group
    /// received in `pool`, with the rest coming back as change, the way
    /// `prepare_send` builds one after NU6.3: Creator, then IO Finalizer. New
    /// value always lands in Ironwood, so spending a legacy Orchard note is a
    /// "turnstile" transaction across both bundles.
    fn draft(pool: Pool, fvk: &FullViewingKey, pay: u64, memo: &str) -> Draft {
        draft_to(pool, fvk, payee(), pay, memo, fvk.address_at(0u32, Scope::Internal))
    }

    /// [`draft`], with the payment going to `pay_to` and the change to `change_to`.
    fn draft_to(
        pool: Pool,
        fvk: &FullViewingKey,
        pay_to: orchard::Address,
        pay: u64,
        memo: &str,
        change_to: orchard::Address,
    ) -> Draft {
        draft_many(pool, fvk, &[(pay_to, pay, memo)], change_to)
    }

    /// [`draft_to`], making every payment in `pays` (recipient, amount, memo).
    fn draft_many(pool: Pool, fvk: &FullViewingKey, pays: &[(orchard::Address, u64, &str)], change_to: orchard::Address) -> Draft {
        let note = received_note(pool, fvk);
        let (anchor, path): (orchard::Anchor, orchard::tree::MerklePath) = {
            let leaf = MerkleHashOrchard::from_cmx(&note.commitment().into());
            let mut tree = ShardTree::<_, 32, 16>::new(MemoryShardStore::<MerkleHashOrchard, u32>::empty(), 100);
            tree.append(leaf, Retention::Marked).unwrap();
            tree.checkpoint(9_999_999).unwrap();
            let path = tree.witness_at_checkpoint_depth(0.into(), 0).unwrap().unwrap();
            (path.root(leaf).into(), path.into())
        };
        let (orchard_anchor, ironwood_anchor) = match pool {
            Pool::Orchard => (Some(anchor), Some(orchard::Anchor::empty_tree())),
            Pool::Ironwood => (None, Some(anchor)),
        };
        let memo_bytes = |memo: &str| if memo.is_empty() { MemoBytes::empty() } else { memo.parse::<Memo>().unwrap().encode() };
        let total: u64 = pays.iter().map(|(_, pay, _)| pay).sum();
        let (ext_ovk, int_ovk) = (Some(fvk.to_ovk(Scope::External)), Some(fvk.to_ovk(Scope::Internal)));
        let builder_with_change = |change: u64| {
            let mut builder = Builder::new(
                RegtestUpgrade::Nu6_3.local_network(),
                1_000.into(),
                BuildConfig::Standard {
                    sapling_anchor: None,
                    orchard_anchor,
                    ironwood_anchor,
                    orchard_padding: BundlePadding::DEFAULT,
                    ironwood_padding: BundlePadding::DEFAULT,
                },
            );
            match pool {
                Pool::Orchard => builder.add_orchard_spend::<zip317::FeeRule>(fvk.clone(), note, path.clone()).unwrap(),
                Pool::Ironwood => builder.add_ironwood_spend::<zip317::FeeRule>(fvk.clone(), note, path.clone()).unwrap(),
            }
            for (pay_to, pay, memo) in pays {
                builder
                    .add_ironwood_output::<zip317::FeeRule>(ext_ovk.clone(), *pay_to, Zatoshis::const_from_u64(*pay), memo_bytes(memo))
                    .unwrap();
            }
            builder
                .add_ironwood_output::<zip317::FeeRule>(int_ovk.clone(), change_to, Zatoshis::const_from_u64(change), MemoBytes::empty())
                .unwrap();
            builder
        };
        // The fee depends only on the shape of the transaction, so ask a
        // builder with placeholder change, then build with the real change.
        let fee = u64::from(builder_with_change(0).get_fee(&zip317::FeeRule::standard()).unwrap());
        let builder = builder_with_change(NOTE - total - fee);
        let PcztResult { pczt_parts, .. } = builder.build_for_pczt(super::super::wallet_rng(), &zip317::FeeRule::standard()).unwrap();
        let pczt = IoFinalizer::new(Creator::build_from_parts(pczt_parts).unwrap()).finalize_io().unwrap();
        let sighash = Signer::new(pczt.clone()).unwrap().shielded_sighash();
        let spends = super::super::spends_to_sign(pczt.clone()).unwrap();
        assert_eq!(spends.len(), 1, "one real spend to sign");
        Draft {
            pczt_hex: hex::encode(pczt.serialize().unwrap()),
            sighash_hex: hex::encode(sighash),
            alpha_hex: spends[0].alpha_hex.clone(),
            fee,
        }
    }

    fn claims(d: &Draft, recipient: String, amount: u64, memo: Option<&str>) -> SigningContext {
        SigningContext {
            recipient,
            amount_zatoshis: amount,
            fee_zatoshis: d.fee,
            memo: memo.map(str::to_string),
            is_unshield: false,
            network: "regtest".into(),
            plan_id: String::new(),
            spend_index: 1,
            spend_total: 1,
            tx_sighash: String::new(),
            pczt: None,
            reason: None,
            payments: Vec::new(),
            poll_id: None,
        }
    }

    fn verify(ak: &str, d: &Draft, pczt_b64: &str) -> Result<VerifiedTransaction, CoreError> {
        verify_signing_request(DEVNET, ak, None, pczt_b64, &d.sighash_hex, Some(&d.alpha_hex))
    }

    fn assert_refused(result: Result<impl std::fmt::Debug, CoreError>, why: &str) {
        match result {
            Err(CoreError::Verification(msg)) => assert!(msg.contains(why), "expected {why:?}, got {msg:?}"),
            other => panic!("expected a refusal ({why}), got {other:?}"),
        }
    }

    fn second_payee() -> orchard::Address {
        let sk = SpendingKey::from_bytes([10; 32]).unwrap();
        FullViewingKey::from(&sk).address_at(0u32, Scope::External)
    }

    fn batch_claims(d: &Draft, payments: &[(String, u64, &str)]) -> SigningContext {
        SigningContext {
            payments: payments
                .iter()
                .map(|(recipient, amount, _memo)| crate::events::ClaimedPayment {
                    recipient: recipient.clone(),
                    amount_zatoshis: *amount,
                    transparent: false,
                })
                .collect(),
            ..claims(d, String::new(), payments.iter().map(|p| p.1).sum(), None)
        }
    }

    /// A batch of `n` payments, each with `memo`.
    fn batch_draft(fvk: &FullViewingKey, n: usize, memo: &str) -> Draft {
        let pays: Vec<_> = (0..n)
            .map(|i| {
                let sk = SpendingKey::from_bytes([20 + i as u8; 32]).unwrap();
                (FullViewingKey::from(&sk).address_at(0u32, Scope::External), 1_000, memo)
            })
            .collect();
        draft_many(Pool::Ironwood, fvk, &pays, fvk.address_at(0u32, Scope::Internal))
    }

    /// A signing request has to fit one frostd message beside the signers'
    /// commitments. The most payments a transaction makes fit, for a few
    /// signers; a memo keeps its payment's ciphertext, so with full memos on
    /// all of them it doesn't, and is refused before anyone is asked, saying
    /// why.
    #[test]
    fn a_batch_fits_one_signing_request_or_is_refused_saying_why() {
        let (_, fvk) = group(1);
        // The rest of the context: every payment described, and a full reason.
        let others = |n: usize| n * 250 + 1_000;
        let most = crate::wallet::MAX_PAYMENTS;
        let b64 = pczt_for_signers_beside(&batch_draft(&fvk, most, "").pczt_hex, others(most), 5).unwrap();
        assert!(b64.len() + others(most) <= crate::signing::signer_context_budget(5));
        let memo = "x".repeat(512);
        pczt_for_signers_beside(&batch_draft(&fvk, 10, &memo).pczt_hex, others(10), 5).unwrap();
        let too_big = pczt_for_signers_beside(&batch_draft(&fvk, most, &memo).pczt_hex, others(most), 5);
        assert!(matches!(too_big, Err(CoreError::Ceremony(m)) if m.contains("Make fewer payments at once")));
    }

    #[test]
    fn a_batch_verifies_when_every_payment_is_described_in_any_order() {
        let (ak, fvk) = group(1);
        let pays = [(payee(), 100_000, "invoice 7"), (second_payee(), 250_000, "")];
        let d = draft_many(Pool::Ironwood, &fvk, &pays, fvk.address_at(0u32, Scope::Internal));
        let v = verify(&ak, &d, &pczt_for_signers(&d.pczt_hex).unwrap()).unwrap();
        assert_eq!(v.payments.len(), 2);
        assert_eq!(v.change_zatoshis, NOTE - 350_000 - d.fee);
        let described = batch_claims(&d, &[(ua(second_payee()), 250_000, ""), (ua(payee()), 100_000, "invoice 7")]);
        check_coordinator_claims(DEVNET, &ak, None, &v, &described).unwrap();
    }

    #[test]
    fn a_batch_is_refused_unless_it_describes_exactly_the_payments_made() {
        let (ak, fvk) = group(1);
        let pays = [(payee(), 100_000, "invoice 7"), (second_payee(), 250_000, "")];
        let d = draft_many(Pool::Ironwood, &fvk, &pays, fvk.address_at(0u32, Scope::Internal));
        let v = verify(&ak, &d, &pczt_for_signers(&d.pczt_hex).unwrap()).unwrap();
        let check = |payments: &[(String, u64, &str)]| check_coordinator_claims(DEVNET, &ak, None, &v, &batch_claims(&d, payments));
        let (a, b) = (ua(payee()), ua(second_payee()));
        assert_refused(check(&[(a.clone(), 100_000, "invoice 7")]), "describes 1 payments, but the transaction makes 2");
        assert_refused(check(&[(a.clone(), 100_000, "invoice 7"), (b.clone(), 260_000, "")]), "no such payment");
        // Each described payment has to be a different one.
        assert_refused(check(&[(a.clone(), 100_000, "invoice 7"), (a.clone(), 100_000, "invoice 7")]), "no such payment");
        let group_address = ua(fvk.address_at(0u32, Scope::External));
        assert_refused(check(&[(a, 100_000, "invoice 7"), (group_address, 250_000, "")]), "back to the group");
    }

    #[test]
    fn a_payee_address_is_recognised_in_the_form_a_verified_payment_shows() {
        let raw = payee().to_raw_address_bytes();
        let net = DEVNET.network_type();
        let with_transparent =
            unified::Address::try_from_items(vec![unified::Receiver::Orchard(raw), unified::Receiver::P2pkh([7; 20])])
                .unwrap()
                .encode(&net);
        let forms = address_forms(DEVNET, &format!("  {with_transparent} ")).unwrap();
        assert_eq!(forms, vec![with_transparent, ua(payee())]);
        assert_eq!(address_forms(DEVNET, &ua(payee())).unwrap(), vec![ua(payee())], "already in that form");

        let transparent = transparent::address::TransparentAddress::PublicKeyHash([7; 20]).to_zcash_address(net).encode();
        assert_eq!(address_forms(DEVNET, &transparent).unwrap(), vec![transparent]);

        let mainnet = unified::Address::try_from_items(vec![unified::Receiver::Orchard(raw)])
            .unwrap()
            .encode(&zcash_protocol::consensus::NetworkType::Main);
        assert!(address_forms(DEVNET, &mainnet).is_err(), "an address on another network");
        assert!(address_forms(DEVNET, "not an address").is_err());
    }

    #[test]
    fn an_honest_send_verifies_spending_from_either_pool() {
        let (ak, fvk) = group(1);
        for pool in [Pool::Orchard, Pool::Ironwood] {
            let d = draft(pool, &fvk, 100_000, "rent for May");
            let v = verify(&ak, &d, &pczt_for_signers(&d.pczt_hex).unwrap()).unwrap();
            assert_eq!(v.sighash_hex, d.sighash_hex);
            assert_eq!(v.spent_zatoshis, NOTE);
            assert_eq!(v.change_zatoshis, NOTE - 100_000 - d.fee);
            assert_eq!(v.fee_zatoshis, d.fee);
            assert_eq!(
                v.payments,
                vec![VerifiedPayment {
                    address: ua(payee()),
                    value_zatoshis: 100_000,
                    transparent: false,
                    memo: Some("rent for May".into()),
                }]
            );
            check_coordinator_claims(DEVNET, &ak, None, &v, &claims(&d, ua(payee()), 100_000, Some("rent for May"))).unwrap();
        }
    }

    /// A group with a viewing seed is verified with its seed. Without it, the
    /// verifier sees a different wallet and refuses.
    #[test]
    fn a_seeded_group_verifies_only_with_its_seed() {
        let (ak, _) = group(1);
        let seed = [5u8; 32];
        let fvk = group_fvk(&ak, Some(&seed)).unwrap();
        let d = draft(Pool::Ironwood, &fvk, 100_000, "");
        let b64 = pczt_for_signers(&d.pczt_hex).unwrap();
        let v = verify_signing_request(DEVNET, &ak, Some(&seed), &b64, &d.sighash_hex, Some(&d.alpha_hex)).unwrap();
        assert_eq!(v.change_zatoshis, NOTE - 100_000 - d.fee);
        assert!(verify_signing_request(DEVNET, &ak, None, &b64, &d.sighash_hex, Some(&d.alpha_hex)).is_err());
    }

    /// A group that made its viewing key private moves its funds off the
    /// address derived from its id: the spend is under the old key, the money
    /// arrives at the private one, and it verifies as a move within the group.
    #[test]
    fn moving_funds_off_the_old_address_verifies_as_a_transfer_within_the_group() {
        let (ak, old) = group(1);
        let seed = [5u8; 32];
        let private = group_fvk(&ak, Some(&seed)).unwrap();
        let to = private.address_at(0u32, Scope::External);
        let d = draft_to(Pool::Ironwood, &old, to, 600_000, "", private.address_at(0u32, Scope::Internal));
        let b64 = pczt_for_signers(&d.pczt_hex).unwrap();
        let v = verify_signing_request(DEVNET, &ak, Some(&seed), &b64, &d.sighash_hex, Some(&d.alpha_hex)).unwrap();
        assert!(v.from_old_address);
        assert!(v.payments.is_empty(), "{:?}", v.payments);
        assert_eq!(v.change_zatoshis, NOTE - d.fee);
        check_coordinator_claims(DEVNET, &ak, Some(&seed), &v, &claims(&d, ua(to), 600_000, None)).unwrap();

        // An ordinary send from the private key isn't flagged.
        let d = draft(Pool::Ironwood, &private, 100_000, "");
        let b64 = pczt_for_signers(&d.pczt_hex).unwrap();
        let v = verify_signing_request(DEVNET, &ak, Some(&seed), &b64, &d.sighash_hex, Some(&d.alpha_hex)).unwrap();
        assert!(!v.from_old_address);
    }

    /// Change sent back to the old address isn't change: anyone who knows the
    /// group's id could watch it, so the signer sees it as a payment.
    #[test]
    fn change_to_the_old_address_is_shown_as_a_payment() {
        let (ak, old) = group(1);
        let seed = [5u8; 32];
        let private = group_fvk(&ak, Some(&seed)).unwrap();
        let d = draft_to(Pool::Ironwood, &private, payee(), 100_000, "", old.address_at(0u32, Scope::Internal));
        let b64 = pczt_for_signers(&d.pczt_hex).unwrap();
        let v = verify_signing_request(DEVNET, &ak, Some(&seed), &b64, &d.sighash_hex, Some(&d.alpha_hex)).unwrap();
        assert_eq!(v.change_zatoshis, 0);
        assert_eq!(v.payments.len(), 2);
        let old_change = ua(old.address_at(0u32, Scope::Internal));
        assert!(v.payments.iter().any(|p| p.address == old_change && p.value_zatoshis == NOTE - 100_000 - d.fee));
        assert_refused(
            check_coordinator_claims(DEVNET, &ak, Some(&seed), &v, &claims(&d, ua(payee()), 100_000, None)),
            "makes 2",
        );
    }

    #[test]
    fn refuses_a_message_that_isnt_this_transactions_sighash() {
        let (ak, fvk) = group(1);
        let d = draft(Pool::Orchard, &fvk, 100_000, "");
        let other = draft(Pool::Orchard, &fvk, 200_000, "");
        let b64 = pczt_for_signers(&d.pczt_hex).unwrap();
        assert_refused(
            verify_signing_request(DEVNET, &ak, None, &b64, &other.sighash_hex, Some(&d.alpha_hex)),
            "not this transaction's sighash",
        );
    }

    #[test]
    fn refuses_a_randomizer_that_isnt_one_of_the_groups_spends() {
        let (ak, fvk) = group(1);
        let d = draft(Pool::Orchard, &fvk, 100_000, "");
        let b64 = pczt_for_signers(&d.pczt_hex).unwrap();
        let wrong = hex::encode(ff::PrimeField::to_repr(&pasta_curves::pallas::Scalar::from(42u64)));
        assert_refused(
            verify_signing_request(DEVNET, &ak, None, &b64, &d.sighash_hex, Some(&wrong)),
            "doesn't belong to any of the group's spends",
        );
        assert_refused(
            verify_signing_request(DEVNET, &ak, None, &b64, &d.sighash_hex, None),
            "must be re-randomized",
        );
    }

    #[test]
    fn refuses_a_transaction_spending_another_groups_notes() {
        let (_, fvk) = group(1);
        let (other_ak, _) = group(2);
        let d = draft(Pool::Orchard, &fvk, 100_000, "");
        assert_refused(
            verify(&other_ak, &d, &pczt_for_signers(&d.pczt_hex).unwrap()),
            "don't belong to this group",
        );
    }

    #[test]
    fn refuses_a_pczt_whose_stated_amount_was_altered() {
        // The coordinator edits the payment amount it shows signers. The note
        // commitment (part of the signed transaction) gives it away.
        let (ak, fvk) = group(1);
        let d = draft(Pool::Orchard, &fvk, 100_000, "");
        let mut bytes = B64.decode(pczt_for_signers(&d.pczt_hex).unwrap()).unwrap();
        // postcard varint of 100_000 is a0 8d 06; make it 100_001.
        let needle = [0xa0u8, 0x8d, 0x06];
        assert_eq!(bytes.windows(3).filter(|w| *w == needle).count(), 1);
        let at = bytes.windows(3).position(|w| w == needle).unwrap();
        bytes[at] = 0xa1;
        assert_refused(verify(&ak, &d, &B64.encode(&bytes)), "doesn't match its note commitment");
    }

    #[test]
    fn refuses_a_coordinator_description_that_doesnt_match() {
        let (ak, fvk) = group(1);
        let d = draft(Pool::Ironwood, &fvk, 100_000, "invoice 7");
        let v = verify(&ak, &d, &pczt_for_signers(&d.pczt_hex).unwrap()).unwrap();
        let check = |c: SigningContext| check_coordinator_claims(DEVNET, &ak, None, &v, &c);

        assert_refused(check(claims(&d, ua(payee()), 1_000, Some("invoice 7"))), "but it pays");
        let elsewhere = FullViewingKey::from(&SpendingKey::from_bytes([3; 32]).unwrap()).address_at(0u32, Scope::External);
        assert_refused(check(claims(&d, ua(elsewhere), 100_000, Some("invoice 7"))), "but it pays");
        assert_refused(check(claims(&d, ua(payee()), 100_000, Some("invoice 8"))), "memo");
        assert_refused(check(claims(&d, ua(payee()), 100_000, None)), "memo");
        assert_refused(check(SigningContext { fee_zatoshis: 1, ..claims(&d, ua(payee()), 100_000, Some("invoice 7")) }), "fee");
        assert_refused(check(SigningContext { is_unshield: true, ..claims(&d, ua(payee()), 100_000, Some("invoice 7")) }), "transparent");
        // "Sending to ourselves" can't hide a payment to someone else.
        let own = ua(fvk.address_at(0u32, Scope::External));
        assert_refused(check(claims(&d, own, 100_000, None)), "back to the group");
    }

    #[test]
    fn a_compacted_pczt_still_verifies() {
        let (ak, fvk) = group(1);
        let d = draft(Pool::Ironwood, &fvk, 250_000, "compact me");
        let full = pczt_for_signers(&d.pczt_hex).unwrap();
        // Force the second, compacting step with a limit just under the full size.
        let compact = pczt_for_signers_within(&d.pczt_hex, full.len() - 1).unwrap();
        assert!(compact.len() < full.len());
        let v = verify(&ak, &d, &compact).unwrap();
        assert_eq!(v.payments[0].memo.as_deref(), Some("compact me"));
        assert_eq!(v.fee_zatoshis, d.fee);
        // And too large even compacted is a clear error, not a silent truncation.
        assert!(pczt_for_signers_within(&d.pczt_hex, 100).is_err());
    }

    #[test]
    fn signers_get_the_transaction_without_merkle_witnesses() {
        let (_, fvk) = group(1);
        let d = draft(Pool::Orchard, &fvk, 100_000, "");
        let raw = hex::decode(&d.pczt_hex).unwrap().len();
        let sent = B64.decode(pczt_for_signers(&d.pczt_hex).unwrap()).unwrap().len();
        assert!(sent + 900 < raw, "the 32-level witness (~1 KiB) should be gone: {sent} vs {raw}");
    }
}
