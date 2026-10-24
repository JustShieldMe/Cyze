//! End-to-end check of the background wallet sync service against a real
//! frostd and mainnet lightwalletd. Ignored by default because it needs the
//! network:
//!
//! ```sh
//! cargo test -p frost-app --test sync_service_live -- --ignored --nocapture
//! ```
//!
//! Two users run a RedPallas DKG. The test then checks that the new group's
//! wallet birthday was recorded at key generation, and that the service, with
//! nothing asking it to, sets up and syncs the group's wallet, reports progress
//! and completion, and runs again on "Sync Now".

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{dkg, pair, start_server};
use frost_app_lib::commands;
use frost_app_lib::state::{wallet_session_key, AppState};
use frost_app_lib::sync_service;
use serde_json::Value;
use tauri::{Listener, Manager};

/// Wait until an event matching `pred` has been seen, or fail after `secs`.
async fn wait_for(events: &Arc<Mutex<Vec<Value>>>, secs: u64, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..secs * 4 {
        if let Some(v) = events.lock().unwrap().iter().find(|v| pred(v)) {
            return v.clone();
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("timed out waiting for {what}; saw: {:#?}", events.lock().unwrap());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs network access to mainnet lightwalletd"]
async fn service_sets_up_and_syncs_a_new_group() {
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let Some(server) = start_server().await else {
        eprintln!("skipping: sidecar frostd binary not built (run scripts/build-sidecar.sh)");
        return;
    };
    let p = pair(&server).await;
    let (alice, alice_state) = (&p.alice, p.alice.app.state::<AppState>());
    let sync_events: Arc<Mutex<Vec<Value>>> = Arc::default();
    let updated_events: Arc<Mutex<Vec<Value>>> = Arc::default();
    for (name, sink) in [("wallet:sync", sync_events.clone()), ("wallet:updated", updated_events.clone())] {
        alice.app.handle().listen(name, move |e| {
            let v: Value = serde_json::from_str(e.payload()).unwrap();
            eprintln!("[alice {name}] {v}");
            sink.lock().unwrap().push(v);
        });
    }
    // Running before any group exists, the service has nothing to sync yet.
    sync_service::spawn(alice.app.handle().clone());

    let group_id = dkg(&p, frost_app_core::ciphersuite::Suite::RedPallas).await;

    // Key generation recorded where the wallet starts, on the default network.
    let key = wallet_session_key(&group_id, "mainnet");
    let mut birthday = None;
    for _ in 0..60 {
        birthday = alice_state.load_private().await.unwrap().network_birthdays.get(&key).copied();
        if birthday.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let birthday = birthday.expect("DKG must record the group's birthday");
    assert!(alice_state.load_private().await.unwrap().group_created_at.contains_key(&group_id));
    eprintln!("birthday recorded at DKG: {birthday}");

    // The service picks up the only RedPallas group by itself: setup, sync, done.
    let gid = group_id.clone();
    let done = wait_for(&sync_events, 180, "a completed sync", |v| {
        v["group_id"] == gid.as_str() && v["state"] == "idle" && !v["last_synced_ms"].is_null()
    })
    .await;
    assert!(done["tip_height"].as_u64().unwrap() >= birthday);
    // With no server chosen, it synced from the fastest of the known ones.
    let cfg = commands::wallet::get_wallet_config(alice_state.clone()).await.unwrap();
    assert!(cfg.automatic && cfg.auto_latency_ms.is_some(), "no server was picked");
    assert!(frost_app_core::wallet::WalletNetwork::Main.known_lightwalletd().contains(&cfg.lightwalletd_url.as_str()));
    eprintln!("picked {} ({} ms)", cfg.lightwalletd_url, cfg.auto_latency_ms.unwrap());
    assert!(done["scanned_height"].as_u64().unwrap() >= birthday);
    assert!(
        updated_events.lock().unwrap().iter().any(|v| v["group_id"] == gid.as_str()),
        "the first sync must tell the UI to re-read the wallet"
    );
    // Setup reused the DKG birthday rather than starting from a fresh tip.
    assert_eq!(alice_state.load_private().await.unwrap().network_birthdays.get(&key), Some(&birthday));
    let status = commands::wallet::wallet_group_status(alice_state.clone(), group_id.clone()).await.unwrap();
    assert!(status.initialized);

    // "Sync Now" runs another sync, reported with a later completion time.
    let first_done = done["last_synced_ms"].as_i64().unwrap();
    commands::wallet::wallet_sync_now(alice_state.clone(), true).await.unwrap();
    wait_for(&sync_events, 60, "a second completed sync", |v| {
        v["group_id"] == gid.as_str() && v["state"] == "idle" && v["last_synced_ms"].as_i64() > Some(first_done)
    })
    .await;

    // Locked, the service goes quiet: a wake produces no new status.
    commands::keystore::lock_keystore(alice_state.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = sync_events.lock().unwrap().len();
    alice_state.sync_service.wake(true);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(sync_events.lock().unwrap().len(), before, "no syncing while locked");
}
