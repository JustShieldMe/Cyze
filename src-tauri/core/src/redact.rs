//! Keeping wallet secrets out of logs.
//!
//! Users copy the in-app diagnostics log into bug reports, and a desktop session
//! may capture stdout into the system journal. A group's id is its Orchard `ak`,
//! and the group's full viewing key is derived from `ak` alone (see
//! `zcash::orchard_fvk_bytes`), so a logged group id exposes every balance and
//! transaction the group has. Log a [`log_tag`] instead, and run every log line
//! through [`redact_secrets`] as a backstop.

use std::borrow::Cow;

use sha2::{Digest, Sha256};

/// A short, one-way label for a group in log lines: stable for a given group
/// (so lines can be correlated) but useless for recovering its id.
pub fn log_tag(group_id: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"cyze-log-tag:");
    h.update(group_id.as_bytes());
    hex::encode(&h.finalize()[..4])
}

/// Hex runs at least this long are treated as secret-bearing: a group id /
/// `ak` (64 hex chars), and anything longer such as a serialized PCZT. Txids and
/// sighashes are also 64 chars; they link a log to on-chain activity, so they go
/// too.
const MIN_SECRET_HEX: usize = 64;

/// Bech32 human-readable prefixes of unified full viewing keys on every network
/// (`uview1…`, `uviewtest1…`, `uviewregtest1…`).
const VIEWING_KEY_PREFIX: &str = "uview";

/// Replace long hex strings and unified viewing keys in `line`. Returns the
/// input unchanged (borrowed) when there is nothing to redact, which is the
/// common case.
pub fn redact_secrets(line: &str) -> Cow<'_, str> {
    if !needs_redaction(line) {
        return Cow::Borrowed(line);
    }
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < bytes.len() {
        if line[i..].starts_with(VIEWING_KEY_PREFIX) && at_token_start(bytes, i) {
            let end = token_end(bytes, i);
            out.push_str("[redacted viewing key]");
            i = end;
            continue;
        }
        if bytes[i].is_ascii_hexdigit() {
            let end = hex_run_end(bytes, i);
            if end - i >= MIN_SECRET_HEX {
                out.push_str("[redacted]");
            } else {
                out.push_str(&line[i..end]);
            }
            i = end;
            continue;
        }
        // Copy one whole UTF-8 character.
        let ch_len = line[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&line[i..i + ch_len]);
        i += ch_len;
    }
    Cow::Owned(out)
}

fn needs_redaction(line: &str) -> bool {
    if line.contains(VIEWING_KEY_PREFIX) {
        return true;
    }
    let mut run = 0;
    for b in line.bytes() {
        if b.is_ascii_hexdigit() {
            run += 1;
            if run >= MIN_SECRET_HEX {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

fn hex_run_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
        end += 1;
    }
    end
}

fn at_token_start(bytes: &[u8], i: usize) -> bool {
    i == 0 || !bytes[i - 1].is_ascii_alphanumeric()
}

fn token_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_alphanumeric() {
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    const AK: &str = "d5a1c1b0e2f3a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3";

    #[test]
    fn log_tag_is_short_stable_and_not_the_id() {
        let t = log_tag(AK);
        assert_eq!(t.len(), 8);
        assert_eq!(t, log_tag(AK));
        assert_ne!(t, log_tag("another group"));
        assert!(!AK.contains(&t));
    }

    #[test]
    fn redacts_group_ids_and_longer_hex() {
        let line = format!("wallet setup: group={AK} tip=4200000");
        assert_eq!(redact_secrets(&line), "wallet setup: group=[redacted] tip=4200000");
        let pczt = "ab".repeat(400);
        assert_eq!(redact_secrets(&format!("pczt {pczt}.")), "pczt [redacted].");
    }

    #[test]
    fn redacts_viewing_keys_on_every_network() {
        for key in ["uview1qqqsome0key", "uviewtest1abc0def", "uviewregtest1zz9"] {
            let line = format!("imported {key}, done");
            assert_eq!(redact_secrets(&line), "imported [redacted viewing key], done");
        }
    }

    #[test]
    fn leaves_ordinary_lines_alone() {
        for line in [
            "pipelined scan: 5000 blocks for [4100000, 4105000) in 812 ms",
            "session 3f2a9c1e-77b0-4c1d-9e8f-0123456789ab joined",
            "connecting to https://zec.rocks:443 — résumé ✓",
            "short hex deadbeef and a 63-char run 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde",
        ] {
            assert!(matches!(redact_secrets(line), Cow::Borrowed(_)), "{line}");
        }
    }

    #[test]
    fn handles_multibyte_text_around_secrets() {
        let line = format!("é{AK}→ok");
        assert_eq!(redact_secrets(&line), "é[redacted]→ok");
    }
}
