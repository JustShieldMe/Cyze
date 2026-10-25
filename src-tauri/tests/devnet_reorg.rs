//! Chain reorganizations near a wallet's start, on a local devnet. A wallet
//! keeps checkpoints only from its start height on, and new wallets start at
//! the chain tip, so a shallow reorg in their first blocks used to leave sync
//! failing on every retry.
//!
//! A: the fork is above the wallet's start. The rewind is clamped to the
//!    earliest checkpoint and sync recovers in place.
//! B: the fork is below the wallet's start. The background service rebuilds
//!    the wallet from an earlier height and syncs again.
//!
//! Needs thus-spoke-zakura 0.3.x (NU6.3) or 0.4.x (NU7, with
//! `CYZE_DEVNET_UPGRADE=nu7`); ignored by default (see devnet_send.rs):
//!
//! ```sh
//! CYZE_DEVNET=cyze-nu7 CYZE_DEVNET_UPGRADE=nu7 cargo test --release -p frost-app --test devnet_reorg -- --ignored --nocapture
//! ```

mod common;

use std::time::{Duration, Instant};

use common::{dkg, pair, start_server, Pair};
use frost_app_core::ciphersuite::Suite;
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use frost_app_lib::sync_service;
use serde_json::Value;
use tauri::Manager;

fn devnet() -> String {
    std::env::var("CYZE_DEVNET").unwrap_or_else(|_| "cyze-e2e".into())
}

/// The upgrade the devnet runs: `nu6_3` for thus-spoke-zakura 0.3, `nu7` for
/// 0.4 (`CYZE_DEVNET_UPGRADE`).
fn devnet_upgrade() -> String {
    std::env::var("CYZE_DEVNET_UPGRADE").unwrap_or_else(|_| "nu6_3".into())
}

fn ths(args: &[&str]) -> String {
    let out = std::process::Command::new("ths").args(args).args(["--name", &devnet()]).output().expect("ths on PATH");
    assert!(out.status.success(), "ths {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn endpoint(name: &str) -> String {
    let e: Value = serde_json::from_str(&ths(&["endpoints", "--json"])).unwrap();
    e[name].as_str().unwrap().to_string()
}

/// One JSON-RPC call to the devnet node.
fn rpc(method: &str, params: Value) -> Value {
    let body = serde_json::json!({ "jsonrpc": "1.0", "id": "t", "method": method, "params": params });
    let out = std::process::Command::new("curl")
        .args(["-s", "-m", "20", "-H", "content-type: application/json", "--data"])
        .arg(body.to_string())
        .arg(endpoint("rpc"))
        .output()
        .expect("curl");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["error"].is_null(), "{method}: {v}");
    v["result"].clone()
}

fn tip() -> u64 {
    rpc("getblockcount", serde_json::json!([])).as_u64().unwrap()
}

/// Orphan every block from `height` up and mine `blocks` new ones on top of
/// the block before it.
fn reorg_from(height: u64, blocks: u32) {
    let hash = rpc("getblockhash", serde_json::json!([height]));
    rpc("invalidateblock", serde_json::json!([hash]));
    // On thus-spoke-zakura 0.3, the devnet's own dashboard wallet failed to
    // follow this reorg and `ths mine` exited non-zero, though the blocks were
    // mined, which is all this needs. 0.4 follows it.
    let out = std::process::Command::new("ths")
        .args(["mine", &blocks.to_string(), "--name", &devnet()])
        .output()
        .expect("ths on PATH");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() || stderr.contains("Blocks mined"), "ths mine: {stderr}");
}

/// A fresh group whose devnet wallet starts at `start` and is kept in sync by
/// the background service.
async fn wallet_starting_at(server: &common::TestServer, start: u64) -> (Pair, String) {
    let p = pair(server).await;
    let alice = p.alice.app.state::<AppState>();
    commands::wallet::set_wallet_config(alice.clone(), "regtest".into(), endpoint("lightwalletd"), Some(devnet_upgrade()))
        .await
        .unwrap();
    let group_id = dkg(&p, Suite::RedPallas).await;
    commands::wallet::wallet_init_account(alice.clone(), group_id.clone(), Some(start)).await.unwrap();
    sync_service::spawn(p.alice.app.handle().clone());
    commands::server::set_active_wallet(alice.clone(), group_id.clone()).await.unwrap();
    (p, group_id)
}

/// Where the group's wallet starts scanning, read from its database.
async fn start_height(p: &Pair, group_id: &str) -> Option<u64> {
    let alice = p.alice.app.state::<AppState>();
    let key = alice.wallet_db_key(group_id).await.unwrap();
    let wid = alice.wallet_dir_id(group_id).await.unwrap();
    let network = frost_app_core::wallet::WalletNetwork::Regtest(frost_app_core::wallet::RegtestUpgrade::parse(&devnet_upgrade()).unwrap());
    frost_app_core::wallet::wallet_start_height(&alice.data_dir, &wid, network, key.as_ref()).unwrap()
}

async fn wait_synced_to(p: &Pair, group_id: &str, height: u64, what: &str) {
    let alice = p.alice.app.state::<AppState>();
    let started = Instant::now();
    loop {
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let status = commands::wallet::wallet_group_status(alice.clone(), group_id.to_string()).await;
        if let Ok(s) = &status {
            if s.synced_height >= height && s.chain_tip_height >= height {
                eprintln!("{what}: synced to {}", s.synced_height);
                return;
            }
        }
        let sync = commands::wallet::wallet_sync_status(alice.clone(), group_id.to_string()).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "{what}: never synced to {height}; status {status:?}, sync {sync:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a thus-spoke-zakura devnet (see the file header)"]
async fn wallets_recover_from_reorgs_near_their_start() {
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    frost_app_lib::logbuf::init_logging(); // shows the clamp and the rebuild
    let server = start_server().await.expect("frostd sidecar binary");
    if tip() < 30 {
        ths(&["mine", "30"]);
    }

    // A: start 8 blocks back and catch up; then follow 3 new blocks at the tip
    // (only those get rewind checkpoints: a catch-up scans the tip first and
    // the blocks behind it without them). Orphan the last 2 and mine 4.
    let t = tip();
    let (a, group_a) = wallet_starting_at(&server, t - 8).await;
    wait_synced_to(&a, &group_a, t, "A before the reorg").await;
    ths(&["mine", "3"]);
    let t = tip();
    wait_synced_to(&a, &group_a, t, "A at the new tip").await;
    let start_a = start_height(&a, &group_a).await;
    assert!(start_a.is_some(), "A's wallet exists");
    reorg_from(t - 1, 4);
    wait_synced_to(&a, &group_a, tip(), "A after the reorg").await;
    assert_eq!(start_height(&a, &group_a).await, start_a, "A recovered in place, without a rebuild");

    // B: start 1 block back; orphan from 3 blocks back, below the start.
    let t = tip();
    let (b, group_b) = wallet_starting_at(&server, t - 1).await;
    wait_synced_to(&b, &group_b, t, "B before the reorg").await;
    let start_b = start_height(&b, &group_b).await.unwrap();
    reorg_from(t - 3, 5);
    wait_synced_to(&b, &group_b, tip(), "B after the reorg (rebuilt)").await;
    let rebuilt = start_height(&b, &group_b).await.unwrap();
    assert!(rebuilt < start_b, "B was rebuilt from an earlier start: {start_b} -> {rebuilt}");
}
