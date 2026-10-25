//! A real send on a local devnet, end to end. A RedPallas DKG creates the
//! group; the devnet faucet funds it and the background service syncs Alice's
//! wallet. Alice saves the recipient as a group payee, then coordinates a
//! 2-of-2 send with a memo and a reason for the group. Bob's app verifies the
//! transaction itself before he approves, and recognises the payee from his
//! own copy of the group's payees. Cyze applies the signature, proves, and
//! broadcasts; the devnet mines it, and Bob's app learns what it was for.
//!
//! Needs thus-spoke-zakura 0.3.x (NU6.3 from block 1) or 0.4.x (NU7 from block
//! 1, with `CYZE_DEVNET_UPGRADE=nu7`). Ignored by default; run in release mode,
//! because Halo2 proving is very slow in debug:
//!
//! ```sh
//! ths start --name cyze-e2e --port-offset 700 --no-open      # another terminal
//! CYZE_DEVNET=cyze-e2e cargo test --release -p frost-app --test devnet_send -- --ignored --nocapture
//! ```

mod common;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{dkg, pair, start_server, until, UserApp};
use frost_app_core::ciphersuite::Suite;
use frost_app_core::wallet::{RegtestUpgrade, WalletNetwork};
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use frost_app_lib::sync_service;
use serde_json::Value;
use tauri::{Listener, Manager};

fn devnet() -> String {
    std::env::var("CYZE_DEVNET").unwrap_or_else(|_| "cyze-e2e".into())
}

/// The upgrade the devnet runs: `nu6_3` for thus-spoke-zakura 0.3, `nu7` for
/// 0.4 (`CYZE_DEVNET_UPGRADE`).
fn devnet_upgrade() -> String {
    std::env::var("CYZE_DEVNET_UPGRADE").unwrap_or_else(|_| "nu6_3".into())
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

/// The devnet's lightwalletd endpoint, from `ths endpoints --json`.
fn lightwalletd_url() -> String {
    let endpoints: Value = serde_json::from_str(&ths(&["endpoints", "--json"])).unwrap();
    endpoints["lightwalletd"].as_str().expect("lightwalletd endpoint").to_string()
}

type Events = Arc<Mutex<Vec<(String, Value)>>>;

fn record(user: &UserApp, names: &[&'static str]) -> Events {
    let events: Events = Arc::default();
    for &name in names {
        let sink = events.clone();
        user.app.handle().listen(name, move |e| {
            let v: Value = serde_json::from_str(e.payload()).unwrap();
            eprintln!("[{name}] {}", v.to_string().chars().take(300).collect::<String>());
            sink.lock().unwrap().push((name.to_string(), v));
        });
    }
    events
}

/// A unified address that isn't the group's (a throwaway Orchard key).
fn outside_recipient() -> String {
    let sk = orchard::keys::SpendingKey::from_bytes([42; 32]).unwrap();
    let ak: [u8; 32] = orchard::keys::FullViewingKey::from(&sk).to_bytes()[..32].try_into().unwrap();
    let net = WalletNetwork::Regtest(RegtestUpgrade::Nu6_3).network_type();
    frost_app_core::zcash::derive_orchard_keys(&ak, None, net).unwrap().address
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a thus-spoke-zakura devnet (see the file header)"]
async fn a_two_of_two_send_is_verified_signed_and_mined() {
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let server = start_server().await.expect("frostd sidecar binary (scripts/build-sidecar.sh)");
    let p = pair(&server).await;
    let (alice, bob) = (p.alice.app.state::<AppState>(), p.bob.app.state::<AppState>());
    let lightwalletd = lightwalletd_url();
    eprintln!("devnet lightwalletd: {lightwalletd}");
    for state in [&alice, &bob] {
        commands::wallet::set_wallet_config((*state).clone(), "regtest".into(), lightwalletd.clone(), Some(devnet_upgrade()))
            .await
            .unwrap();
    }
    let group_id = dkg(&p, Suite::RedPallas).await;

    // Fund the group; received funds need 10 confirmations to be spendable.
    let address = commands::wallet::wallet_receive_address(alice.clone(), group_id.clone()).await.unwrap().address;
    ths(&["faucet", &address, "--amount", "2"]);
    ths(&["mine", "12"]);

    // The background service sets up and syncs Alice's wallet by itself.
    sync_service::spawn(p.alice.app.handle().clone());
    commands::server::set_active_wallet(alice.clone(), group_id.clone()).await.unwrap();
    let started = Instant::now();
    let funded = loop {
        let status = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await;
        if let Ok(s) = &status {
            if s.spendable_zatoshis >= 100_000_000 {
                break s.spendable_zatoshis;
            }
        }
        assert!(started.elapsed() < Duration::from_secs(300), "wallet never became spendable: {status:?}");
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    eprintln!("spendable after sync: {funded} zatoshis");

    // Alice saves the recipient as a group payee, and Bob's app gets it.
    let recipient = outside_recipient();
    commands::payees::save_payee(p.alice.app.handle().clone(), group_id.clone(), None, "Auditor".into(), recipient.clone())
        .await
        .unwrap();
    until(&p, &group_id, "Bob never got the payee", async || {
        !commands::payees::list_payees(bob.clone(), group_id.clone()).await.unwrap().is_empty()
    })
    .await;

    // Alice coordinates a send; Bob's app verifies it before he approves.
    let txid = send_with_bobs_approval(
        &p,
        commands::wallet::WalletSendArgs {
            group_id: group_id.clone(),
            recipient: recipient.clone(),
            amount_zatoshis: 50_000_000,
            signers: vec![p.alice_pubkey.clone(), p.bob_pubkey.clone()],
            memo: Some("cyze end-to-end".into()),
            from_old_address: false,
            send_max: false,
            reason: Some("Audit, invoice 12".into()),
        },
        Some(50_000_000),
    )
    .await;
    eprintln!("broadcast txid {txid}");

    // Every member's history says what it was for, Bob's too.
    until(&p, &group_id, "Bob never learned what the payment was for", async || {
        commands::payees::list_payment_reasons(bob.clone(), group_id.clone())
            .await
            .unwrap()
            .iter()
            .any(|r| r.txid == txid.to_lowercase() && r.text == "Audit, invoice 12" && r.author == p.alice_pubkey)
    })
    .await;

    // Mined, and in the group's history once synced.
    ths(&["mine", "2"]);
    let started = Instant::now();
    loop {
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let history = commands::wallet::wallet_history(alice.clone(), group_id.clone()).await.unwrap();
        if history.iter().any(|t| t.txid == txid) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(180), "{txid} never showed up in history");
    }
    let after = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await.unwrap();
    eprintln!("total after send: {} zatoshis", after.total_zatoshis);
    assert!(after.total_zatoshis < funded - 50_000_000 + 1, "the payment left the wallet");

    // "Send all": everything spendable, with a memo, the fee coming out of it.
    // The change from the first send needs its confirmations first.
    ths(&["mine", "10"]);
    let started = Instant::now();
    loop {
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let s = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await.unwrap();
        if s.spendable_zatoshis > 0 && s.spendable_zatoshis == s.total_zatoshis {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(180), "the change never became spendable: {s:?}");
    }
    let txid = send_with_bobs_approval(
        &p,
        commands::wallet::WalletSendArgs {
            group_id: group_id.clone(),
            recipient: recipient.clone(),
            amount_zatoshis: 0,
            signers: vec![p.alice_pubkey.clone(), p.bob_pubkey.clone()],
            memo: Some("everything".into()),
            from_old_address: false,
            send_max: true,
            reason: None,
        },
        None,
    )
    .await;
    eprintln!("sent everything in {txid}");
    ths(&["mine", "2"]);
    let started = Instant::now();
    loop {
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let s = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await.unwrap();
        let history = commands::wallet::wallet_history(alice.clone(), group_id.clone()).await.unwrap();
        if s.total_zatoshis == 0 && history.iter().any(|t| t.txid == txid) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(180), "the wallet never emptied: {s:?}");
    }
}

/// Alice coordinates a send; Bob's app joins each signing request, verifies
/// it, and Bob approves. Returns the broadcast txid. With `amount`, checks the
/// one payment Bob's app verified is that much to the recipient, with the
/// memo; without, that it's everything spent, less the fee.
async fn send_with_bobs_approval(p: &common::Pair, args: commands::wallet::WalletSendArgs, amount: Option<u64>) -> String {
    let bob = p.bob.app.state::<AppState>();
    let (group_id, recipient, memo) = (args.group_id.clone(), args.recipient.clone(), args.memo.clone());
    let reason = args.reason.clone();
    let payees = commands::payees::list_payees(bob.clone(), group_id.clone()).await.unwrap();
    let bob_events = record(&p.bob, &["signing:progress", "signing:failed"]);
    let alice_events = record(&p.alice, &["send:progress", "send:complete", "send:failed"]);
    commands::wallet::wallet_send(p.alice.app.handle().clone(), args).await.unwrap();

    let mut joined = HashSet::new();
    let mut approved = HashSet::new();
    let started = Instant::now();
    loop {
        if let Some((_, v)) = alice_events.lock().unwrap().iter().find(|(n, _)| n == "send:failed") {
            panic!("send failed: {v}");
        }
        if let Some((_, v)) = alice_events.lock().unwrap().iter().find(|(n, _)| n == "send:complete") {
            return v["txid"].as_str().unwrap().to_string();
        }
        if let Some((_, v)) = bob_events.lock().unwrap().iter().find(|(n, _)| n == "signing:failed") {
            panic!("Bob's signing failed: {v}");
        }
        for s in commands::signing::list_pending_sessions(bob.clone(), None).await.unwrap_or_default() {
            // Group records (the payment's reason) aren't signing requests.
            if s.kind != "group_message" && joined.insert(s.session_id) {
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
            let payments = verified["payments"].as_array().expect("a verified transaction");
            assert_eq!(payments.len(), 1, "{verified}");
            assert_eq!(payments[0]["address"], recipient.as_str());
            assert_eq!(payments[0]["memo"].as_str(), memo.as_deref());
            // What Alice says it's for reaches Bob as she wrote it, and Bob's
            // own copy of the group's payees names the address he verified.
            assert_eq!(event["context"]["reason"].as_str(), reason.as_deref(), "{event}");
            let paid = payments[0]["address"].as_str().unwrap().to_string();
            assert!(payees.iter().any(|p| p.forms.contains(&paid)), "the recipient is a payee: {paid}");
            match amount {
                Some(amount) => assert_eq!(payments[0]["value_zatoshis"], amount),
                None => {
                    assert_eq!(verified["change_zatoshis"], 0, "nothing comes back: {verified}");
                    let spent = verified["spent_zatoshis"].as_u64().unwrap();
                    let fee = verified["fee_zatoshis"].as_u64().unwrap();
                    assert_eq!(payments[0]["value_zatoshis"].as_u64().unwrap(), spent - fee);
                }
            }
            // Bob sees how long is left, from his own view of the chain: the
            // standard window, open, and counted in blocks on a devnet.
            let window = &event["window"];
            assert_eq!(window["expiry_height"], verified["expiry_height"], "{event}");
            assert!(window["blocks_left"].as_u64().unwrap() > 0, "{window}");
            assert!(window["closes_at_ms"].is_null(), "{window}");
            if event["auto_approved"] != true {
                commands::signing::respond_to_signing(bob.clone(), ceremony.parse().unwrap(), true)
                    .await
                    .unwrap();
            }
        }
        assert!(started.elapsed() < Duration::from_secs(900), "the send didn't complete");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
