//! ZcashNames (ZNS) resolution: turning a human-readable name like `alice`
//! (or `alice.zcash`) into the Zcash unified address it points at, so a user can
//! send to a name instead of a long address.
//!
//! # Protocol (as of 2026-08)
//!
//! Names are claimed on-chain as Ed25519-signed memos inside Orchard notes —
//! colon-delimited UTF-8, e.g. `ZNS:CLAIM:<name>:<ua>:<sig>[:<pubkey>]` where the
//! signature is base64 Ed25519 over everything between `ZNS:` and `:<sig>`. Names
//! match `[a-z0-9]{1,62}`. A **ZNS indexer** scans the chain, verifies every
//! signature, and exposes a JSON-RPC API; resolution is a query against it, not a
//! chain scan by this wallet.
//!
//! Public indexer endpoints (zcash.me):
//! - testnet: `https://light.zcash.me/zns-testnet`
//! - mainnet: `https://light.zcash.me/zns-mainnet-test`
//!
//! The `resolve` method takes `params: [query, limit?, offset?]`. For a **name**
//! query it returns a single [`ResolveResult`] (or `null` if unregistered); the
//! `address` field is the unified address to pay. (An address query returns an
//! array — one address may own several names; an empty query lists all.)
//!
//! # Trust
//!
//! The indexer is external infrastructure. A wrong or compromised resolver could
//! return an attacker's address, so callers MUST show the resolved address for the
//! user to confirm before sending — a name is a convenience, never an
//! authorization. This module only *reads*; claiming/registering a name (the
//! signed-memo write path) is a separate, larger piece (see TODO.md).

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// The public ZNS indexer JSON-RPC endpoint for a network, if there is one
/// to rely on. Mainnet has none yet: the only mainnet indexer is a test
/// service (`zns-mainnet-test`), and a resolver decides where money goes, so
/// mainnet lookups stay off until a production one exists.
pub fn endpoint(mainnet: bool) -> Option<&'static str> {
    if mainnet {
        None
    } else {
        Some("https://light.zcash.me/zns-testnet")
    }
}

/// Why a lookup can't run on mainnet, for the user.
pub const MAINNET_UNAVAILABLE: &str =
    "ZcashNames lookups are off on mainnet until there's a production directory. Paste the address instead.";

/// A resolved ZNS registration. Extra fields returned by the indexer (txid,
/// height, nonce, signature, pubkey, …) are ignored — the wallet needs the name,
/// the address to pay, and enough to warn the user.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResolveResult {
    /// The resolved name (without the `.zcash` suffix).
    pub name: String,
    /// The Zcash unified address the name points at — the send recipient.
    pub address: String,
    /// The most recent on-chain action for the name (e.g. `CLAIM`, `UPDATE`).
    #[serde(default)]
    pub last_action: Option<String>,
    /// Present when the name is currently listed for sale. Surfaced so the UI can
    /// warn that ownership may be about to change hands.
    #[serde(default)]
    pub listing: Option<Listing>,
}

/// A name's active sale listing, if any (price is in zatoshis).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Listing {
    #[serde(default)]
    pub price: Option<u64>,
    #[serde(default)]
    pub pay_taddr: Option<String>,
}

#[derive(Deserialize)]
struct RpcResponse {
    #[serde(default)]
    result: Option<ResolveResult>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

/// Whether `input` looks like a ZNS name worth trying to resolve (as opposed to a
/// raw Zcash address the user typed). True for a bare label like `alice` or a
/// dotted `alice.zcash`; false for addresses (which contain no `.zcash` and are
/// far longer than a label). Used to decide whether to attempt resolution.
pub fn looks_like_zns_name(input: &str) -> bool {
    let t = input.trim();
    if let Some(label) = t.strip_suffix(".zcash") {
        return is_valid_label(label);
    }
    // A bare label that fits the ZNS charset/length and doesn't begin like a
    // Zcash address. This is only a UI hint — the send flow resolves a recipient
    // by trying to decode it as an address first and only falling back to ZNS —
    // so the rare name that happens to start with an address prefix still works
    // when entered as `name.zcash`.
    is_valid_label(t) && !starts_like_address(t)
}

/// Whether `s` begins with a Zcash address HRP/prefix. Those prefixes are also
/// valid ZNS-label characters, so a bare label starting with one is treated as a
/// (possible) address rather than a name.
fn starts_like_address(s: &str) -> bool {
    const PREFIXES: [&str; 8] = ["u1", "utest", "zs1", "ztest", "t1", "t3", "tm", "tex"];
    PREFIXES.iter().any(|p| s.starts_with(p))
}

fn is_valid_label(label: &str) -> bool {
    let label = label.trim();
    (1..=62).contains(&label.len())
        && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Normalize user input to a bare ZNS label: drop an optional `.zcash` suffix,
/// trim, and require the `[a-z0-9]{1,62}` charset. Returns `None` if it is not a
/// syntactically valid name.
pub fn normalize_name(input: &str) -> Option<String> {
    let t = input.trim();
    let label = t.strip_suffix(".zcash").unwrap_or(t).trim();
    if is_valid_label(label) {
        Some(label.to_string())
    } else {
        None
    }
}

/// Resolve a ZNS name to its registration via the public indexer. Returns
/// `Ok(None)` when the name is syntactically valid but unregistered, and an error
/// on a bad name or a transport/indexer failure. Touches the network.
pub async fn resolve_name(input: &str, mainnet: bool) -> Result<Option<ResolveResult>, CoreError> {
    let name = normalize_name(input).ok_or_else(|| {
        CoreError::Config(format!(
            "'{}' is not a valid ZNS name (expected [a-z0-9], 1-62 chars, optionally .zcash)",
            input.trim()
        ))
    })?;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resolve",
        "params": [name],
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| CoreError::Connection(format!("ZNS client: {e}")))?;

    let url = endpoint(mainnet).ok_or_else(|| CoreError::Config(MAINNET_UNAVAILABLE.into()))?;
    let response = client
        .post(url)
        .json(&request)
        .send()
        .await
        .map_err(|e| CoreError::Connection(format!("ZNS resolve request failed: {e}")))?;

    if !response.status().is_success() {
        return Err(CoreError::Connection(format!(
            "ZNS resolver returned HTTP {}",
            response.status()
        )));
    }

    let parsed: RpcResponse = response
        .json()
        .await
        .map_err(|e| CoreError::Connection(format!("ZNS resolve response: {e}")))?;

    if let Some(err) = parsed.error {
        return Err(CoreError::Connection(format!(
            "ZNS resolve error {}: {}",
            err.code, err.message
        )));
    }
    Ok(parsed.result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mainnet_lookups_are_off_until_a_production_indexer_exists() {
        assert_eq!(endpoint(true), None);
        assert!(endpoint(false).unwrap().contains("testnet"));
    }

    #[tokio::test]
    async fn a_mainnet_lookup_fails_without_contacting_anyone() {
        let err = resolve_name("alice.zcash", true).await.unwrap_err();
        assert!(err.to_string().contains("off on mainnet"), "{err}");
    }

    #[test]
    fn recognizes_names_but_not_addresses() {
        assert!(looks_like_zns_name("alice"));
        assert!(looks_like_zns_name("alice.zcash"));
        assert!(looks_like_zns_name("bob123"));
        // A unified/transparent address is not a name.
        assert!(!looks_like_zns_name(
            "u1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq"
        ));
        assert!(!looks_like_zns_name("t1exampletaddr"));
        assert!(!looks_like_zns_name("Alice")); // uppercase not allowed
        assert!(!looks_like_zns_name("a b"));
    }

    #[test]
    fn normalizes_and_strips_suffix() {
        assert_eq!(normalize_name("  alice  ").as_deref(), Some("alice"));
        assert_eq!(normalize_name("alice.zcash").as_deref(), Some("alice"));
        assert_eq!(normalize_name("ALICE"), None);
        assert_eq!(normalize_name(""), None);
        assert_eq!(normalize_name(&"a".repeat(63)), None); // too long
        assert_eq!(normalize_name(&"a".repeat(62)).as_deref(), Some(&"a".repeat(62)[..]));
    }

    #[test]
    fn parses_a_resolve_result_ignoring_extra_fields() {
        // Shape mirrors the indexer's openrpc `resolve` example.
        let json = r#"{
            "name": "alice",
            "address": "utest1qqqfff",
            "txid": "abc123",
            "height": 3901200,
            "nonce": 2,
            "signature": "AQID",
            "last_action": "UPDATE",
            "pubkey": null,
            "listing": null
        }"#;
        let r: ResolveResult = serde_json::from_str(json).unwrap();
        assert_eq!(r.name, "alice");
        assert_eq!(r.address, "utest1qqqfff");
        assert_eq!(r.last_action.as_deref(), Some("UPDATE"));
        assert!(r.listing.is_none());
    }

    #[test]
    fn parses_a_listed_name() {
        let json = r#"{"name":"bob","address":"utest1aaa","listing":{"price":100000,"pay_taddr":"t1x"}}"#;
        let r: ResolveResult = serde_json::from_str(json).unwrap();
        assert_eq!(r.listing.as_ref().and_then(|l| l.price), Some(100000));
    }
}
