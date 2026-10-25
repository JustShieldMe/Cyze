//! Group records from a newer version of Cyze, which this one can't use, over
//! a real frostd: they mustn't stop the records after them arriving, or leave
//! a gap the other member keeps asking to have filled.

mod common;

use std::time::Duration;

use common::{dkg, pair, polls_of, start_server, until};
use frost_app_core::ciphersuite::Suite;
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use tauri::Manager;

/// Records a newer version of Cyze sends, which this version can't use: one of
/// a kind it doesn't know, and a poll with more options than it allows. They
/// mustn't stop the next poll from arriving, or leave a gap that makes Bob ask
/// Alice for them again and again.
#[tokio::test(flavor = "multi_thread")]
async fn records_from_a_newer_version_dont_hold_up_the_rest() {
    use frost_app_core::polls::{self, Channel, Message, Record, RecordBody};

    let Some(server) = start_server().await else {
        eprintln!("skipping: sidecar frostd binary not built (run scripts/build-sidecar.sh)");
        return;
    };
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let p = pair(&server).await;
    let alice = p.alice.app.state::<AppState>();
    let group_id = dkg(&p, Suite::RedPallas).await;

    let channel = Channel {
        trust: commands::server::trust_for(&alice, &p.server_url).await,
        server_url: p.server_url.clone(),
        comm_privkey: alice.with_config(|c| Ok(c.communication_key.as_ref().unwrap().privkey.clone())).await.unwrap(),
        comm_pubkey: alice.with_config(|c| Ok(c.communication_key.as_ref().unwrap().pubkey.clone())).await.unwrap(),
    };
    let bob_key = alice
        .with_config(|c| {
            Ok(c.group[&group_id].participant.values().map(|m| m.pubkey.clone()).find(|k| *k != channel.comm_pubkey))
        })
        .await
        .unwrap()
        .unwrap();
    let record = |seq, body| Message::Record(Record { group_id: group_id.clone(), seq, at: 100 + seq, body });
    let poll = |id: &str, options: Vec<String>| RecordBody::Poll {
        poll_id: id.into(),
        title: "Pay the auditor?".into(),
        details: String::new(),
        options,
        closes_at: 4_000_000_000,
        community: None,
    };
    let future: serde_json::Value = serde_json::json!({ "kind": "from_the_future", "what": "a kind no version uses" });
    let batch = [
        record(1, RecordBody::Unknown(future.as_object().unwrap().clone())),
        record(2, poll("wide", (0..20).map(|i| i.to_string()).collect())),
        record(3, poll("p", vec!["Yes".into(), "No".into()])),
    ];
    polls::send(&channel, std::slice::from_ref(&bob_key), &batch).await.unwrap();

    until(&p, &group_id, "Bob never got the poll", async || !polls_of(&p.bob).await.is_empty()).await;
    let bobs = polls_of(&p.bob).await;
    assert_eq!(bobs.iter().map(|p| p.poll_id.as_str()).collect::<Vec<_>>(), ["p"], "only the poll Bob can use shows");

    // Nothing is missing, so Bob doesn't ask Alice to resend.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let asked = commands::signing::list_pending_sessions(alice.clone(), None).await.unwrap();
    assert!(!asked.iter().any(|s| s.kind == "group_message"), "Bob asked Alice to resend what he has");
}
