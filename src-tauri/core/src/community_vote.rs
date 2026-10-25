//! Zcash community votes: coinholder votes run through ValarGroup's Shielded
//! Vote (https://valargroup.gitbook.io/shielded-vote-docs), in which a wallet
//! votes with the balance it held at a round's snapshot.
//!
//! This module reads what's being voted on: the rounds a vote chain knows,
//! their proposals and closing times, and the results once a round is
//! tallied. Rounds and their vote servers are published per network in the
//! operator's voting config: production for mainnet, staging for testnet.
//! Nothing here signs or reveals anything about a wallet; the requests are
//! the same for everyone.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Zatoshis per ballot: a round counts a wallet's weight in whole ballots of
/// 0.125 ZEC (`zcash_voting::governance::BALLOT_DIVISOR`), and tallies are in
/// ballots.
pub const BALLOT_ZATOSHIS: u64 = 12_500_000;

/// The voting config for a network: `"main"` reads production, `"test"`
/// staging. A local devnet has none.
pub fn config_url(network: &str) -> Option<&'static str> {
    match network {
        "main" => Some("https://voting.valargroup.org/prod/dynamic-voting-config.json"),
        "test" => Some("https://voting.valargroup.org/stage/dynamic-voting-config.json"),
        _ => None,
    }
}

#[derive(Debug, Clone, Deserialize)]
struct VotingConfig {
    vote_servers: Vec<VoteServer>,
}

#[derive(Debug, Clone, Deserialize)]
struct VoteServer {
    url: String,
}

/// A round's state, as the vote chain numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundStatus {
    /// Setting up its election key; not open yet.
    Pending,
    /// Open for votes until `vote_end_time`.
    Active,
    /// Closed, being counted.
    Tallying,
    /// Counted: results are final.
    Finalized,
    /// Setup failed; the round never opened.
    Failed,
    Unknown,
}

impl RoundStatus {
    fn from_code(code: u32) -> Self {
        match code {
            1 => Self::Active,
            2 => Self::Tallying,
            3 => Self::Finalized,
            4 => Self::Pending,
            5 => Self::Failed,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RawRound {
    vote_round_id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    discussion_url: Option<String>,
    #[serde(default, deserialize_with = "number_or_string")]
    snapshot_height: u64,
    #[serde(default, deserialize_with = "number_or_string")]
    vote_end_time: u64,
    #[serde(default, deserialize_with = "number_or_string")]
    created_at_time: u64,
    #[serde(default)]
    status: u32,
    #[serde(default)]
    proposals: Vec<RawProposal>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawProposal {
    id: u32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    options: Vec<RawOption>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawOption {
    #[serde(default)]
    label: String,
}

fn number_or_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N {
        N(u64),
        S(String),
    }
    Ok(match N::deserialize(d)? {
        N::N(n) => n,
        N::S(s) => s.parse().unwrap_or(0),
    })
}

/// A round as the interface shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Round {
    /// Hex, as the vote chain's URLs take it.
    pub id: String,
    pub title: String,
    pub description: String,
    pub discussion_url: Option<String>,
    /// Weight is the balance a wallet held at this height.
    pub snapshot_height: u64,
    /// Unix seconds.
    pub vote_end_time: u64,
    pub created_at: u64,
    pub status: RoundStatus,
    /// A test round the operator ran on the live network ("[TEST]" in its title).
    pub test: bool,
    pub proposals: Vec<Proposal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Proposal {
    pub id: u32,
    pub title: String,
    pub description: String,
    pub options: Vec<String>,
    /// Once tallied: the weight behind each option, in zatoshis.
    pub results: Option<Vec<u64>>,
}

fn round_from(raw: RawRound) -> Option<Round> {
    let id = hex::encode(base64::engine::general_purpose::STANDARD.decode(raw.vote_round_id.trim()).ok()?);
    let title = if raw.title.trim().is_empty() {
        raw.proposals.first().map(|p| p.title.clone()).unwrap_or_default()
    } else {
        raw.title.trim().to_string()
    };
    Some(Round {
        id,
        test: title.starts_with("[TEST]"),
        title,
        description: raw.description,
        discussion_url: raw.discussion_url.filter(|u| u.starts_with("https://")),
        snapshot_height: raw.snapshot_height,
        vote_end_time: raw.vote_end_time,
        created_at: raw.created_at_time,
        status: RoundStatus::from_code(raw.status),
        proposals: raw
            .proposals
            .into_iter()
            .map(|p| Proposal {
                id: p.id,
                title: p.title,
                description: p.description,
                options: p.options.into_iter().map(|o| o.label).collect(),
                results: None,
            })
            .collect(),
    })
}

/// Parse a vote server's round list, newest first.
pub fn parse_rounds(json: &[u8]) -> Result<Vec<Round>, CoreError> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum List {
        Wrapped { rounds: Vec<RawRound> },
        Bare(Vec<RawRound>),
    }
    let raw = match serde_json::from_slice::<List>(json).map_err(|e| CoreError::Server(format!("rounds: {e}")))? {
        List::Wrapped { rounds } | List::Bare(rounds) => rounds,
    };
    let mut rounds: Vec<Round> = raw.into_iter().filter_map(round_from).collect();
    rounds.sort_by_key(|r| std::cmp::Reverse((r.created_at, r.vote_end_time)));
    Ok(rounds)
}

/// Fill in a round's results from a vote server's tally: weight per proposal
/// and option, in ballots (an absent option index means option 0).
pub fn apply_tally(round: &mut Round, json: &[u8]) -> Result<(), CoreError> {
    #[derive(Deserialize)]
    struct Tally {
        #[serde(default)]
        results: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        proposal_id: u32,
        #[serde(default)]
        vote_decision: u32,
        #[serde(default, deserialize_with = "number_or_string")]
        total_value: u64,
    }
    let tally: Tally = serde_json::from_slice(json).map_err(|e| CoreError::Server(format!("tally: {e}")))?;
    for proposal in &mut round.proposals {
        let mut weights = vec![0u64; proposal.options.len()];
        for entry in tally.results.iter().filter(|e| e.proposal_id == proposal.id) {
            if let Some(w) = weights.get_mut(entry.vote_decision as usize) {
                *w = w.saturating_add(entry.total_value.saturating_mul(BALLOT_ZATOSHIS));
            }
        }
        proposal.results = Some(weights);
    }
    Ok(())
}

/// What a network has to show: rounds that are open or about to open, and
/// the most recent finished ones with their results.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CommunityVotes {
    pub open: Vec<Round>,
    pub recent: Vec<Round>,
}

/// How many finished rounds to show.
const RECENT: usize = 3;

/// Read a network's rounds from its vote servers, trying each in turn, and
/// the results of the most recent finished ones. Test rounds are left out on
/// mainnet, where they'd only confuse.
pub async fn fetch(network: &str) -> Result<CommunityVotes, CoreError> {
    let Some(config_url) = config_url(network) else {
        return Ok(CommunityVotes::default());
    };
    let get = |url: String| async move { get_json(&url).await };
    let config: VotingConfig = serde_json::from_value(get(config_url.to_string()).await?)
        .map_err(|e| CoreError::Server(format!("voting config: {e}")))?;
    let mut last_error = CoreError::Server("the voting config lists no vote servers".into());
    for server in config.vote_servers.iter().filter(|s| s.url.starts_with("https://")) {
        let base = server.url.trim_end_matches('/');
        let rounds = match get(format!("{base}/shielded-vote/v1/rounds")).await.and_then(|v| parse_rounds(&v.to_string().into_bytes())) {
            Ok(r) => r,
            Err(e) => {
                last_error = e;
                continue;
            }
        };
        let rounds: Vec<Round> = rounds.into_iter().filter(|r| network != "main" || !r.test).collect();
        let open = rounds
            .iter()
            .filter(|r| matches!(r.status, RoundStatus::Active | RoundStatus::Pending))
            .cloned()
            .collect();
        let mut recent: Vec<Round> = rounds
            .into_iter()
            .filter(|r| matches!(r.status, RoundStatus::Tallying | RoundStatus::Finalized))
            .take(RECENT)
            .collect();
        for round in recent.iter_mut().filter(|r| r.status == RoundStatus::Finalized) {
            if let Ok(tally) = get(format!("{base}/shielded-vote/v1/tally-results/{}", round.id)).await {
                let _ = apply_tally(round, &tally.to_string().into_bytes());
            }
        }
        return Ok(CommunityVotes { open, recent });
    }
    Err(last_error)
}

/// GET a JSON document: through Tor while it's on for wallet traffic, never
/// directly while it's on but not ready.
async fn get_json(url: &str) -> Result<serde_json::Value, CoreError> {
    #[cfg(feature = "wallet")]
    if let Some(tor) = crate::wallet::tor::for_https()? {
        let uri = url.parse().map_err(|e| CoreError::Config(format!("{url}: {e}")))?;
        let response = tor
            .http_get_json::<serde_json::Value>(uri, 1, |_| None)
            .await
            .map_err(|e| CoreError::Connection(format!("{url} through Tor: {e}")))?;
        return Ok(response.into_body());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| CoreError::Connection(e.to_string()))?;
    let response = client.get(url).send().await.map_err(|e| CoreError::Connection(format!("{url}: {e}")))?;
    if !response.status().is_success() {
        return Err(CoreError::Server(format!("{url}: HTTP {}", response.status())));
    }
    response.json().await.map_err(|e| CoreError::Server(format!("{url}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUNDS: &str = r#"{"rounds":[
      {"vote_round_id":"LLpm6xbHWBoWkssLAzeF2gExBZ9kWg0vtNwqUTFQXgE=","title":"Coinholder Retroactive Grants Q3",
       "snapshot_height":"3485940","vote_end_time":"1790712000","created_at_time":"1789660920","status":3,
       "discussion_url":"https://forum.zcashcommunity.com/t/1",
       "proposals":[{"id":1,"title":"Zcash Grants Hub","description":"d","options":[{"label":"Support"},{"index":1,"label":"Oppose"}]}]},
      {"vote_round_id":"AA==","title":"[TEST] Snack Governance","snapshot_height":1,"vote_end_time":2,"created_at_time":3,"status":1,"proposals":[]}
    ]}"#;

    #[test]
    fn rounds_and_tallies_read_as_the_interface_shows_them() {
        let mut rounds = parse_rounds(ROUNDS.as_bytes()).unwrap();
        assert_eq!(rounds.len(), 2);
        let grants = rounds.iter_mut().find(|r| !r.test).unwrap();
        assert_eq!(grants.id, "2cba66eb16c7581a1692cb0b033785da0131059f645a0d2fb4dc2a5131505e01");
        assert_eq!(grants.status, RoundStatus::Finalized);
        assert_eq!(grants.snapshot_height, 3_485_940);
        assert_eq!(grants.proposals[0].options, vec!["Support", "Oppose"]);
        let tally = r#"{"results":[
          {"vote_round_id":"x","proposal_id":1,"total_value":8},
          {"vote_round_id":"x","proposal_id":1,"vote_decision":1,"total_value":"2"}]}"#;
        apply_tally(grants, tally.as_bytes()).unwrap();
        // 8 and 2 ballots of 0.125 ZEC.
        assert_eq!(grants.proposals[0].results, Some(vec![100_000_000, 25_000_000]));
        assert!(rounds.iter().any(|r| r.test && r.status == RoundStatus::Active));
    }

    #[test]
    fn a_devnet_has_no_community_votes() {
        assert_eq!(config_url("regtest"), None);
        assert!(config_url("main").unwrap().contains("/prod/"));
        assert!(config_url("test").unwrap().contains("/stage/"));
    }
}

#[cfg(test)]
mod live {
    /// Reads the real production and staging rounds:
    /// `cargo test -p frost-app-core community_vote::live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs network access to the vote servers"]
    async fn reads_the_live_rounds() {
        for network in ["main", "test"] {
            let votes = super::fetch(network).await.unwrap();
            eprintln!("{network}: {} open, {} recent", votes.open.len(), votes.recent.len());
            for r in &votes.recent {
                let results = r.proposals.first().and_then(|p| p.results.clone());
                eprintln!("  {} ({:?}), first proposal results: {results:?}", r.title, r.status);
            }
            assert!(!votes.recent.is_empty(), "{network} has finished rounds");
        }
    }
}
