//! The viewing seed a group's members agree on during key generation (see
//! `dkg::agree_viewing_seed`), from which its Orchard viewing key is derived
//! (`zcash::orchard_fvk_bytes`). Kept apart from the `zcash` feature because
//! key generation runs without it.

use sha2::{Digest, Sha256};

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
