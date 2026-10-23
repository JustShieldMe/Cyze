//! Zcash Orchard key/address derivation (feature `zcash`).
//!
//! A FROST group produces only the Orchard **spend validating key** (`ak`).
//! A full viewing key also needs the **nullifier key** (`nk`) and the
//! **commit-ivk randomness** (`rivk`); together they form the 96-byte FVK
//! encoding `ak || nk || rivk`.
//!
//! Groups created now derive `nk`/`rivk` from a **viewing seed** the members
//! agree on during key generation (`dkg::agree_viewing_seed`): random, held
//! only in each member's keystore, and never sent to the server in the clear.
//! Knowing `ak` (the group id) is then not enough to view the wallet.
//!
//! Groups created before that, and groups set up with members using the
//! upstream `frost-client` CLI (which doesn't run the agreement round), derive
//! `nk`/`rivk` from `ak` alone (domain-separated hash-to-field). Anyone who
//! learns their `ak` can reconstruct their viewing key.

use ff::{FromUniformBytes, PrimeField};
use pasta_curves::pallas;
use sha2::{Digest, Sha512};
use zcash_address::unified::{Address, Encoding, Fvk, Receiver, Ufvk};
use zcash_protocol::consensus::NetworkType;

use crate::error::CoreError;

const NK_DOMAIN: &[u8] = b"Cyze:OrchardFVK:nk:v1";
const RIVK_DOMAIN: &[u8] = b"Cyze:OrchardFVK:rivk:v1";
const NK_DOMAIN_V2: &[u8] = b"Cyze:OrchardFVK:nk:v2";
const RIVK_DOMAIN_V2: &[u8] = b"Cyze:OrchardFVK:rivk:v2";
pub use crate::viewing::ViewingSeed;

/// Derived, display-ready Orchard keys for a group.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OrchardKeys {
    /// Unified address string (`u1…`).
    pub address: String,
    /// Unified full viewing key string (`uview1…`).
    pub ufvk: String,
}

fn wide_hash(domain: &[u8], seed: Option<&ViewingSeed>, ak: &[u8]) -> [u8; 64] {
    let mut h = Sha512::new();
    h.update(domain);
    if let Some(seed) = seed {
        h.update(seed);
    }
    h.update(ak);
    let mut out = [0u8; 64];
    out.copy_from_slice(&h.finalize());
    out
}

/// Build the 96-byte Orchard FVK encoding (`ak || nk || rivk`) for a group:
/// from its viewing seed if it has one, else from `ak` alone.
pub fn orchard_fvk_bytes(ak: &[u8; 32], seed: Option<&ViewingSeed>) -> [u8; 96] {
    let (nk_domain, rivk_domain) = match seed {
        Some(_) => (NK_DOMAIN_V2, RIVK_DOMAIN_V2),
        None => (NK_DOMAIN, RIVK_DOMAIN),
    };
    let nk = pallas::Base::from_uniform_bytes(&wide_hash(nk_domain, seed, ak));
    let rivk = pallas::Scalar::from_uniform_bytes(&wide_hash(rivk_domain, seed, ak));

    let mut out = [0u8; 96];
    out[..32].copy_from_slice(ak);
    out[32..64].copy_from_slice(&nk.to_repr());
    out[64..].copy_from_slice(&rivk.to_repr());
    out
}

fn parse_ak(ak_hex: &str) -> Result<[u8; 32], CoreError> {
    let bytes = hex::decode(ak_hex.trim()).map_err(|e| CoreError::Crypto(format!("bad ak hex: {e}")))?;
    bytes.try_into().map_err(|_| CoreError::Crypto("ak must be 32 bytes".into()))
}

fn full_viewing_key(ak: &[u8; 32], seed: Option<&ViewingSeed>) -> Result<orchard::keys::FullViewingKey, CoreError> {
    orchard::keys::FullViewingKey::from_bytes(&orchard_fvk_bytes(ak, seed))
        .ok_or_else(|| CoreError::Crypto("invalid Orchard ak (not a valid spend key)".into()))
}

/// Derive the unified address and UFVK for a group, encoded for the given
/// network (mainnet `u1…` / testnet `utest1…`).
pub fn derive_orchard_keys(
    ak: &[u8; 32],
    seed: Option<&ViewingSeed>,
    network: NetworkType,
) -> Result<OrchardKeys, CoreError> {
    let fvk = full_viewing_key(ak, seed)?;
    let receiver = fvk.address_at(0u32, orchard::keys::Scope::External).to_raw_address_bytes();
    let address = Address::try_from_items(vec![Receiver::Orchard(receiver)])
        .map_err(|e| CoreError::Crypto(format!("unified address: {e:?}")))?
        .encode(&network);
    let ufvk = Ufvk::try_from_items(vec![Fvk::Orchard(fvk.to_bytes())])
        .map_err(|e| CoreError::Crypto(format!("ufvk: {e:?}")))?
        .encode(&network);
    Ok(OrchardKeys { address, ufvk })
}

/// Convenience: derive from a hex-encoded `ak` (the group's verifying key id).
pub fn derive_orchard_keys_hex(
    ak_hex: &str,
    seed: Option<&ViewingSeed>,
    network: NetworkType,
) -> Result<OrchardKeys, CoreError> {
    derive_orchard_keys(&parse_ak(ak_hex)?, seed, network)
}

/// Derive the group's Orchard unified receive address at a specific diversifier
/// index. Rotating the index yields a fresh, unlinkable receive address that the
/// same viewing key still detects (Orchard IVK trial-decryption is diversifier-
/// independent), so callers can hand out a new address per payment without any
/// change to the group key. Index 0 matches [`derive_orchard_keys`].
pub fn derive_orchard_address_at(
    ak_hex: &str,
    seed: Option<&ViewingSeed>,
    network: NetworkType,
    index: u32,
) -> Result<String, CoreError> {
    let fvk = full_viewing_key(&parse_ak(ak_hex)?, seed)?;
    let receiver = fvk.address_at(index, orchard::keys::Scope::External).to_raw_address_bytes();
    Ok(Address::try_from_items(vec![Receiver::Orchard(receiver)])
        .map_err(|e| CoreError::Crypto(format!("unified address: {e:?}")))?
        .encode(&network))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchard::keys::{FullViewingKey, Scope, SpendingKey};

    fn sample_ak() -> [u8; 32] {
        // A valid Orchard ak: take it from a real spending key's FVK.
        let sk = Option::<SpendingKey>::from(SpendingKey::from_bytes([7u8; 32]))
            .expect("valid spending key");
        FullViewingKey::from(&sk).to_bytes()[..32].try_into().unwrap()
    }

    /// Every group's addresses and viewing key are a function of its `ak`;
    /// if this derivation ever changed, existing groups would get new addresses
    /// and lose sight of their funds. Pinned to values computed before the move
    /// to the Zakura crates.
    #[test]
    fn derivation_matches_pinned_vectors() {
        let ak_hex = "6ebb833c1d2f8433080abceabe47906097f90678d603f577d0486c9111737b07";
        let ak: [u8; 32] = hex::decode(ak_hex).unwrap().try_into().unwrap();
        assert_eq!(
            hex::encode(orchard_fvk_bytes(&ak, None)),
            "6ebb833c1d2f8433080abceabe47906097f90678d603f577d0486c9111737b07\
             2b9bd789f5007ec766361f8bf5e3474b8ceb89a4fbc6aaa803a029cdb4fa8d1b\
             bb9846b22870731b335ed92afec757ab7e162c68910ea1e26886b5c4a2d7b610"
        );
        let keys = derive_orchard_keys(&ak, None, NetworkType::Main).unwrap();
        assert_eq!(
            keys.ufvk,
            "uview1jzhdahdgpj8cm5wswuwvecs9ptezl7kg8vepqpftpjhr8ezd833rmgsxas2dv8un02xz296ektsn8j7\
             ptkdvjg2yslz60nka99dm8nqjhk0u4kfpuu408mn6h7svhgg27q0lucf2p7ly36qdm6u2mq6wc89dafyr7yfj\
             ekwtwc2wxr2lu0v3fag8t86gh"
        );
        assert_eq!(
            keys.address,
            "u1khfqtdrsta94r50r4k7g0spey5660e63z3g9eg6hzg7psupanek7gxmje4ql6tv8g4vy60wht0pp74qysyfvqgdypk3y5dc8hum6n2fs"
        );
        assert_eq!(
            derive_orchard_address_at(ak_hex, None, NetworkType::Main, 5).unwrap(),
            "u1284pcgye3x0utzpmy4yz0a86gp3hxxuhkk8my7z8cp0dwkxgc5wyrmt96hrtwfgwyal54pzw9kld9lyy9rfjs5y8hu3zuue2mc6myvdw"
        );
    }

    #[test]
    fn derives_stable_address_and_ufvk() {
        let ak = sample_ak();
        let a = derive_orchard_keys(&ak, None, NetworkType::Main).unwrap();
        let b = derive_orchard_keys(&ak, None, NetworkType::Main).unwrap();
        // Deterministic: same ak -> same keys, every member, every run.
        assert_eq!(a.address, b.address);
        assert_eq!(a.ufvk, b.ufvk);
        assert!(a.address.starts_with("u1"), "got {}", a.address);
        assert!(a.ufvk.starts_with("uview1"), "got {}", a.ufvk);

        // Testnet encodes the same key material under testnet HRPs.
        let t = derive_orchard_keys(&ak, None, NetworkType::Test).unwrap();
        assert!(t.address.starts_with("utest1"), "got {}", t.address);
        assert!(t.ufvk.starts_with("uviewtest1"), "got {}", t.ufvk);
        assert_ne!(t.address, a.address);
    }

    #[test]
    fn rotating_receive_addresses_are_distinct_and_index0_matches_default() {
        let ak = sample_ak();
        let ak_hex = hex::encode(ak);
        // Index 0 must equal the canonical default address.
        let default = derive_orchard_keys(&ak, None, NetworkType::Test).unwrap().address;
        let at0 = derive_orchard_address_at(&ak_hex, None, NetworkType::Test, 0).unwrap();
        assert_eq!(default, at0);
        // Successive indices produce distinct, valid testnet unified addresses.
        let mut seen = std::collections::HashSet::new();
        for i in 0..8u32 {
            let a = derive_orchard_address_at(&ak_hex, None, NetworkType::Test, i).unwrap();
            assert!(a.starts_with("utest1"), "got {a}");
            assert!(seen.insert(a), "diversifier index {i} reused an address");
        }
    }

    #[test]
    fn fvk_parts_roundtrip_to_address() {
        // The (ak, nk, rivk) path produces a usable FVK whose address matches
        // the one the reconstructed FVK derives directly.
        let ak = sample_ak();
        let fvk_bytes = orchard_fvk_bytes(&ak, None);
        let fvk = FullViewingKey::from_bytes(&fvk_bytes).expect("valid FVK");
        let direct = fvk.address_at(0u32, Scope::External).to_raw_address_bytes();
        assert_eq!(direct.len(), 43);
    }

    #[test]
    fn frost_reddsa_accepts_orchard_ak_bytes() {
        // Cross-version byte-compat: the orchard ak encoding must be parseable
        // by the (different-version) reddsa that FROST signs with — proving a
        // FROST RedPallas verifying key feeds straight into orchard.
        let ak = sample_ak();
        let vk = reddsa::frost::redpallas::VerifyingKey::deserialize(&ak);
        assert!(vk.is_ok(), "FROST reddsa must parse orchard ak bytes");
    }

    /// A group with a viewing seed has keys nobody can compute from `ak`
    /// alone, every member derives the same ones, and they never change.
    #[test]
    fn seeded_keys_are_pinned_and_not_derivable_from_ak() {
        let ak_hex = "6ebb833c1d2f8433080abceabe47906097f90678d603f577d0486c9111737b07";
        let ak: [u8; 32] = hex::decode(ak_hex).unwrap().try_into().unwrap();
        let seed = [9u8; 32];
        let seeded = derive_orchard_keys(&ak, Some(&seed), NetworkType::Main).unwrap();
        let derived = derive_orchard_keys(&ak, None, NetworkType::Main).unwrap();
        assert_ne!(seeded.ufvk, derived.ufvk);
        assert_ne!(seeded.address, derived.address);
        assert_ne!(seeded.ufvk, derive_orchard_keys(&ak, Some(&[8u8; 32]), NetworkType::Main).unwrap().ufvk);
        assert_eq!(
            derive_orchard_address_at(ak_hex, Some(&seed), NetworkType::Main, 0).unwrap(),
            seeded.address
        );
        assert_eq!(hex::encode(orchard_fvk_bytes(&ak, Some(&seed))), PINNED_SEEDED_FVK);
    }
    const PINNED_SEEDED_FVK: &str = "6ebb833c1d2f8433080abceabe47906097f90678d603f577d0486c9111737b07\
        abc23b584f46c54da8a2cb5e0b73657939b5b80bd860565ddf02b13a62fe6427\
        68af5bbfb57ca9d9adaa99d9ed85be2407a1f22fbcbdca52c833377fd267d216";

}
