//! Tor for wallet traffic, through the app's commands, against mainnet: turn it
//! on, wait until it's ready, reach a sync server through it, turn it off.
//!
//! ```sh
//! cargo test -p frost-app --test tor_live -- --ignored --nocapture
//! ```

mod common;

use std::time::{Duration, Instant};

use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use tauri::Manager;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs network access to the Tor network and mainnet lightwalletd"]
async fn wallet_traffic_goes_through_tor_when_turned_on() {
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let user = common::make_user();
    let state = user.app.state::<AppState>();

    commands::tor::set_use_tor(user.app.handle().clone(), true).await.unwrap();
    assert_eq!(state.load_settings().use_tor, Some(true));
    let started = Instant::now();
    loop {
        let status = commands::tor::tor_status().await.unwrap();
        match status.state {
            "ready" => break,
            "failed" => panic!("Tor didn't start: {:?}", status.error),
            _ => {}
        }
        assert!(started.elapsed() < Duration::from_secs(120), "Tor took too long");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eprintln!("Tor ready in {} ms", started.elapsed().as_millis());

    let info = commands::wallet::lightwalletd_info(state.clone(), Some("https://zec.rocks:443".into()))
        .await
        .unwrap();
    assert_eq!(info.chain_name, "main");
    eprintln!("reached zec.rocks through Tor at height {}", info.block_height);

    commands::tor::set_use_tor(user.app.handle().clone(), false).await.unwrap();
    assert_eq!(commands::tor::tor_status().await.unwrap().state, "off");
}
