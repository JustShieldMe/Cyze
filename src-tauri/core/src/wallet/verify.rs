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
//! [`check_coordinator_claims`] then compares the coordinator's description
//! with the result, and the app refuses to sign if they disagree.

use base64::Engine as _;
use ff::PrimeField;
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

/// Largest base64 PCZT the coordinator attaches to a signing request. frostd
/// caps each encrypted message at 64 KiB, and the signing package and its
/// encryption need the rest.
pub const MAX_SIGNER_PCZT_B64: usize = 48 * 1024;

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

fn serialize_b64(pczt: pczt::Pczt) -> Result<String, CoreError> {
    let bytes = pczt
        .serialize()
        .map_err(|e| CoreError::Ceremony(format!("serialize pczt: {e:?}")))?;
    Ok(B64.encode(bytes))
}

/// Prepare a draft's PCZT (hex, as `prepare_send` returns it) for co-signers:
/// base64, without the Merkle witnesses only the prover needs. If that is still
/// over [`MAX_SIGNER_PCZT_B64`], also drop the fields signers can recompute
/// (`cv_net`, `cmx`, decryptable ciphertexts). That is safe: the signer
/// recomputes them, and then requires the sighash they produce to equal the
/// message being signed.
pub fn pczt_for_signers(pczt_hex: &str) -> Result<String, CoreError> {
    pczt_for_signers_within(pczt_hex, MAX_SIGNER_PCZT_B64)
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
            "this transaction spends too many notes for co-signers to verify in one request. \
             Send a smaller amount first so it uses fewer notes."
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

fn recover_memo(pool: Pool, action: &orchard::pczt::Action, fvk: &FullViewingKey) -> Option<String> {
    let out_ciphertext = &action.output().encrypted_note().out_ciphertext;
    [Scope::External, Scope::Internal].into_iter().find_map(|scope| {
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
    fvk: &FullViewingKey,
    net: NetworkType,
    totals: &mut Totals,
) -> Result<(), OrchardError<String>> {
    bundle.verify_cross_address_restriction().map_err(OrchardError::Verify)?;
    for action in bundle.actions() {
        let spend = action.spend();
        let output = action.output();
        // The spent note: real (the group's) or a zero-value dummy with its own key.
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
            if spend.spend_auth_sig().is_none() {
                if let Some(alpha) = spend.alpha() {
                    totals.unsigned_alphas.push(alpha.to_repr());
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
        if fvk.scope_for_address(&recipient).is_some() {
            totals.change += value;
        } else if value > 0 {
            totals.payments.push(VerifiedPayment {
                address: orchard_ua(recipient.to_raw_address_bytes(), net).map_err(OrchardError::Custom)?,
                value_zatoshis: value,
                transparent: false,
                memo: recover_memo(pool, action, fvk),
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
    let fvk = group_fvk(group_ak_hex, viewing_seed)?;
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
        .with_orchard(|b| check_bundle(Pool::Orchard, b, &fvk, net, &mut totals))
        .map_err(|e| fail(describe(e)))?
        .with_ironwood(|b| check_bundle(Pool::Ironwood, b, &fvk, net, &mut totals))
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
/// recipient (or none, for a send back to the group), and the stated memo.
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
        let memo = if memo.is_empty() { MemoBytes::empty() } else { memo.parse::<Memo>().unwrap().encode() };
        let change_to = fvk.address_at(0u32, Scope::Internal);
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
            builder
                .add_ironwood_output::<zip317::FeeRule>(ext_ovk.clone(), payee(), Zatoshis::const_from_u64(pay), memo.clone())
                .unwrap();
            builder
                .add_ironwood_output::<zip317::FeeRule>(int_ovk.clone(), change_to, Zatoshis::const_from_u64(change), MemoBytes::empty())
                .unwrap();
            builder
        };
        // The fee depends only on the shape of the transaction, so ask a
        // builder with placeholder change, then build with the real change.
        let fee = u64::from(builder_with_change(0).get_fee(&zip317::FeeRule::standard()).unwrap());
        let builder = builder_with_change(NOTE - pay - fee);
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
        let wrong = hex::encode(pasta_curves::pallas::Scalar::from(42u64).to_repr());
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
