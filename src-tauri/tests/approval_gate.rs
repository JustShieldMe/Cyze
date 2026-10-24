//! The signer's approval gate for wallet (RedPallas) keys, end to end through
//! the command layer against a real frostd.
//!
//! A wallet signature can spend the group's funds, so a signer only signs a
//! request whose attached transaction verifies (frost_app_core::wallet::verify,
//! unit-tested there). A request with no transaction attached is a raw message:
//! refused automatically unless the signer has turned on advanced options.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{dkg, pair, start_server, UserApp};
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use serde_json::Value;
use tauri::{Listener, Manager};

type Events = Arc<Mutex<Vec<(String, Value)>>>;

fn record(user: &UserApp, names: &[&'static str]) -> Events {
    let events: Events = Arc::default();
    for &name in names {
        let sink = events.clone();
        user.app.handle().listen(name, move |e| {
            sink.lock().unwrap().push((name.to_string(), serde_json::from_str(e.payload()).unwrap()));
        });
    }
    events
}

async fn wait_for(events: &Events, what: &str, pred: impl Fn(&str, &Value) -> bool) -> Value {
    for _ in 0..240 {
        if let Some((_, v)) = events.lock().unwrap().iter().find(|(n, v)| pred(n, v)) {
            return v.clone();
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("timed out waiting for {what}; saw {:#?}", events.lock().unwrap());
}

/// Alice asks the group to sign a raw message; Bob joins. Returns Bob's
/// ceremony id.
async fn raw_request(alice: &UserApp, bob: &UserApp, group_id: &str, server_url: &str, signers: Vec<String>) -> uuid::Uuid {
    let created: Events = record(alice, &["signing:progress"]);
    commands::signing::create_signing_session(
        alice.app.handle().clone(),
        commands::signing::CreateSigningSessionArgs {
            group_id: group_id.to_string(),
            message_hex: hex::encode(b"not a transaction"),
            signers,
            server_url: Some(server_url.to_string()),
        },
    )
    .await
    .unwrap();
    let session = wait_for(&created, "the coordinator's session", |_, v| v["event"]["phase"] == "session_created").await;
    let session_id: uuid::Uuid = session["event"]["session_id"].as_str().unwrap().parse().unwrap();
    commands::signing::join_signing_session(
        bob.app.handle().clone(),
        commands::signing::JoinSigningSessionArgs {
            group_id: group_id.to_string(),
            session_id,
            server_url: Some(server_url.to_string()),
        },
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn wallet_keys_never_sign_unverifiable_requests_by_default() {
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let Some(server) = start_server().await else {
        eprintln!("skipping: sidecar frostd binary not built (run scripts/build-sidecar.sh)");
        return;
    };
    let p = pair(&server).await;
    let group_id = dkg(&p, frost_app_core::ciphersuite::Suite::RedPallas).await;
    let (alice, bob, server_url) = (&p.alice, &p.bob, p.server_url.clone());
    let bob_state = bob.app.state::<AppState>();
    let signers = vec![p.alice_pubkey.clone(), p.bob_pubkey.clone()];
    let bob_events = record(bob, &["signing:progress", "signing:failed", "signing:complete"]);

    // 1. Default settings: refused automatically, and nothing to approve.
    let ceremony = raw_request(alice, bob, &group_id, &server_url, signers.clone()).await;
    let id = ceremony.to_string();
    let gated = wait_for(&bob_events, "the gate's decision", |n, v| {
        n == "signing:progress" && v["ceremony_id"] == id.as_str() && v["event"]["phase"] == "awaiting_approval"
    })
    .await;
    let reason = gated["event"]["refused"].as_str().expect("refused automatically");
    assert!(reason.contains("no transaction attached"), "{reason}");
    let failed = wait_for(&bob_events, "the refusal to end the ceremony", |n, v| {
        n == "signing:failed" && v["ceremony_id"] == id.as_str()
    })
    .await;
    assert!(failed["error"].as_str().unwrap().contains("no transaction attached"), "{failed}");
    assert!(
        commands::signing::respond_to_signing(bob_state.clone(), ceremony, true).await.is_err(),
        "a refused request can't be approved afterwards"
    );

    // 2. With advanced options on, the raw request reaches the signer, marked
    //    unverified, and their approval produces the signature.
    commands::server::set_advanced_mode(bob_state.clone(), true).await.unwrap();
    let alice_events = record(alice, &["signing:complete"]);
    let ceremony = raw_request(alice, bob, &group_id, &server_url, signers).await;
    let id = ceremony.to_string();
    let gated = wait_for(&bob_events, "the raw request", |n, v| {
        n == "signing:progress" && v["ceremony_id"] == id.as_str() && v["event"]["phase"] == "awaiting_approval"
    })
    .await;
    assert_eq!(gated["event"]["unverified"], true, "{gated}");
    assert!(gated["event"].get("refused").is_none());
    commands::signing::respond_to_signing(bob_state.clone(), ceremony, true).await.unwrap();
    let done = wait_for(&alice_events, "the group signature", |_, v| v["signature_hex"].is_string()).await;
    assert!(!done["signature_hex"].as_str().unwrap().is_empty());
}
