//! Safety codes: a number people read to each other to confirm they hold each
//! other's real communication keys, not keys someone in the middle swapped in
//! when the contact codes were exchanged.
//!
//! Each key has its own code (as Signal's safety numbers are built from one
//! fingerprint per side): you read yours aloud and the other person checks it
//! against what their app shows for you. Faking that means finding another key
//! with the same 30-digit code, a second preimage of about 100 bits through an
//! iterated hash. A single code shared by a pair would be weaker: someone in the
//! middle controls both substituted keys and only needs two codes to collide.

use sha2::{Digest, Sha512};

const VERSION: &[u8] = b"cyze-safety-code-v1";
/// Slows each guess down, like Signal's fingerprint iterations.
const ITERATIONS: usize = 5200;

/// The safety code for a communication public key: 30 digits in six groups of
/// five, e.g. `"05732 81946 10293 77310 46281 90455"`.
pub fn safety_code(pubkey: &[u8]) -> String {
    let mut hash = Sha512::new().chain_update(VERSION).chain_update(pubkey).finalize();
    for _ in 0..ITERATIONS {
        hash = Sha512::new().chain_update(hash).chain_update(pubkey).finalize();
    }
    hash.chunks(5)
        .take(6)
        .map(|chunk| {
            let n = chunk.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
            format!("{:05}", n % 100_000)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_and_distinct_per_key() {
        let a = safety_code(&[1u8; 32]);
        assert_eq!(a, safety_code(&[1u8; 32]));
        assert_ne!(a, safety_code(&[2u8; 32]));
        assert_eq!(a.split(' ').count(), 6);
        assert!(a.split(' ').all(|g| g.len() == 5 && g.bytes().all(|b| b.is_ascii_digit())));
        // Pinned, so a change to the derivation can't silently change everyone's codes.
        assert_eq!(a, PINNED);
    }
    const PINNED: &str = "57347 48397 15719 52822 80859 68792";
}
