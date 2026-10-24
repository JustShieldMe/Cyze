//! An older group goes private on a local devnet, end to end. The group is
//! created the way versions before 0.3 did, with a viewing key derived from its
//! id, and funded at that address. Its members agree on a private viewing key;
//! Alice's wallet keeps watching the old address and gets a new one. She then
//! moves the funds across: Bob's app verifies the move (old-key notes in, the
//! group's private address out) before he approves, and the devnet mines it.
//!
//! Needs thus-spoke-zakura (see `devnet_send.rs`). Ignored by default; run in
//! release mode:
//!
//! ```sh
//! CYZE_DEVNET=cyze-nu7 CYZE_DEVNET_UPGRADE=nu7 \
//!   cargo test --release -p frost-app --test devnet_viewing_key -- --ignored --nocapture
//! ```

mod common;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use common::{bob_joins_viewing_key_session, dkg_with, pair, start_server, wait_until_private};
use frost_app_core::ciphersuite::Suite;
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use frost_app_lib::sync_service;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tauri::{Listener, Manager};

fn devnet() -> String {
    std::env::var("CYZE_DEVNET").unwrap_or_else(|_| "cyze-e2e".into())
}

fn ths(args: &[&str]) -> String {
    let out = std::process::Command::new("ths")
        .args(args)
        .args(["--name", &devnet()])
        .output()
        .expect("ths (thus-spoke-zakura) on PATH");
    assert!(out.status.success(), "ths {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

type Events = Arc<Mutex<Vec<(String, Value)>>>;

fn record(app: &tauri::App<tauri::test::MockRuntime>, names: &[&'static str]) -> Events {
    let events: Events = Arc::default();
    for &name in names {
        let sink = events.clone();
        app.handle().listen(name, move |e| {
            let v: Value = serde_json::from_str(e.payload()).unwrap();
            eprintln!("[{name}] {}", v.to_string().chars().take(300).collect::<String>());
            sink.lock().unwrap().push((name.to_string(), v));
        });
    }
    events
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a thus-spoke-zakura devnet (see the file header)"]
async fn an_older_group_goes_private_and_moves_its_funds() {
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let server = start_server().await.expect("frostd sidecar binary (scripts/build-sidecar.sh)");
    let p = pair(&server).await;
    let (alice, bob) = (p.alice.app.state::<AppState>(), p.bob.app.state::<AppState>());
    let endpoints: Value = serde_json::from_str(&ths(&["endpoints", "--json"])).unwrap();
    let lightwalletd = endpoints["lightwalletd"].as_str().unwrap().to_string();
    let upgrade = std::env::var("CYZE_DEVNET_UPGRADE").unwrap_or_else(|_| "nu6_3".into());
    for state in [&alice, &bob] {
        commands::wallet::set_wallet_config((*state).clone(), "regtest".into(), lightwalletd.clone(), Some(upgrade.clone()))
            .await
            .unwrap();
    }

    // An older group, funded at the address its id gives away.
    let group_id = dkg_with(&p, Suite::RedPallas, false).await;
    let old_address = commands::wallet::wallet_receive_address(alice.clone(), group_id.clone()).await.unwrap().address;
    ths(&["faucet", &old_address, "--amount", "2"]);
    ths(&["mine", "12"]);
    sync_service::spawn(p.alice.app.handle().clone());
    commands::server::set_active_wallet(alice.clone(), group_id.clone()).await.unwrap();
    let started = Instant::now();
    let funded = loop {
        let s = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await;
        if let Some(s) = s.as_ref().ok().filter(|s| s.spendable_zatoshis >= 100_000_000) {
            break s.spendable_zatoshis;
        }
        assert!(started.elapsed() < Duration::from_secs(300), "never became spendable: {s:?}");
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    eprintln!("funded at the old address: {funded} zatoshis");

    // The members agree on a private viewing key.
    commands::viewing::make_viewing_key_private(p.alice.app.handle().clone(), group_id.clone()).await.unwrap();
    bob_joins_viewing_key_session(&p, &group_id).await;
    wait_until_private(&alice, &group_id).await;
    wait_until_private(&bob, &group_id).await;
    let private_address = commands::wallet::wallet_receive_address(alice.clone(), group_id.clone()).await.unwrap().address;
    assert_ne!(private_address, old_address);

    // The old address's funds show up as still there, ready to move.
    let started = Instant::now();
    loop {
        let v = commands::viewing::viewing_key_status(alice.clone(), group_id.clone()).await.unwrap();
        let old = v.old_address.expect("an old address");
        if old.synced && old.spendable_zatoshis == funded {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(180), "the old wallet never caught up");
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // Alice moves them; Bob's app checks the move before he approves.
    let draft = commands::viewing::wallet_prepare_move(alice.clone(), group_id.clone()).await.unwrap();
    assert_eq!(draft.amount_zatoshis + draft.fee_zatoshis, funded, "everything moves; the fee comes out of it");
    let bob_events = record(&p.bob.app, &["signing:progress", "signing:failed"]);
    let alice_events = record(&p.alice.app, &["send:complete", "send:failed"]);
    commands::wallet::wallet_send(
        p.alice.app.handle().clone(),
        commands::wallet::WalletSendArgs {
            group_id: group_id.clone(),
            recipient: String::new(),
            amount_zatoshis: 0,
            signers: vec![p.alice_pubkey.clone(), p.bob_pubkey.clone()],
            memo: None,
            from_old_address: true,
        },
    )
    .await
    .unwrap();

    let (mut joined, mut approved) = (HashSet::new(), HashSet::new());
    let started = Instant::now();
    let txid = loop {
        if let Some((_, v)) = alice_events.lock().unwrap().iter().find(|(n, _)| n == "send:failed") {
            panic!("the move failed: {v}");
        }
        if let Some((_, v)) = alice_events.lock().unwrap().iter().find(|(n, _)| n == "send:complete") {
            break v["txid"].as_str().unwrap().to_string();
        }
        if let Some((_, v)) = bob_events.lock().unwrap().iter().find(|(n, _)| n == "signing:failed") {
            panic!("Bob's signing failed: {v}");
        }
        for s in commands::signing::list_pending_sessions(bob.clone(), None).await.unwrap_or_default() {
            if s.kind == "signing" && joined.insert(s.session_id) {
                commands::signing::join_signing_session(
                    p.bob.app.handle().clone(),
                    commands::signing::JoinSigningSessionArgs {
                        group_id: group_id.clone(),
                        session_id: s.session_id,
                        server_url: None,
                    },
                )
                .await
                .unwrap();
            }
        }
        let ready: Vec<(String, Value)> = bob_events
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, v)| v["event"]["phase"] == "awaiting_approval")
            .map(|(_, v)| (v["ceremony_id"].as_str().unwrap().to_string(), v["event"].clone()))
            .collect();
        for (ceremony, event) in ready {
            if !approved.insert(ceremony.clone()) {
                continue;
            }
            assert!(event.get("refused").is_none(), "Bob's app refused: {event}");
            let verified = &event["verified"];
            assert_eq!(verified["from_old_address"], true, "{verified}");
            assert_eq!(verified["payments"].as_array().map(Vec::len), Some(0), "nothing leaves the group: {verified}");
            assert_eq!(verified["change_zatoshis"], draft.amount_zatoshis);
            if event["auto_approved"] != true {
                commands::signing::respond_to_signing(bob.clone(), ceremony.parse().unwrap(), true).await.unwrap();
            }
        }
        assert!(started.elapsed() < Duration::from_secs(900), "the move didn't complete");
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    eprintln!("moved in {txid}");

    // Mined: the private wallet holds the funds, the old address is empty, and
    // the history shows one move.
    ths(&["mine", "2"]);
    let started = Instant::now();
    loop {
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let current = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await.unwrap();
        let v = commands::viewing::viewing_key_status(alice.clone(), group_id.clone()).await.unwrap();
        let history = commands::wallet::wallet_history(alice.clone(), group_id.clone()).await.unwrap();
        let moved = history.iter().filter(|t| t.txid == txid).collect::<Vec<_>>();
        if current.total_zatoshis == draft.amount_zatoshis
            && v.old_address.as_ref().is_some_and(|o| o.total_zatoshis == 0)
            && moved.len() == 1
        {
            assert_eq!(moved[0].direction, "move");
            assert!(history.iter().any(|t| t.direction == "receive"), "the original deposit is still in the history");
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(180), "the move never showed up: {current:?} {history:?}");
    }
}
