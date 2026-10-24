//! A member's backup restores their identity and key shares on a new device,
//! together with the wallet birthdays a restored wallet needs to find funds
//! received earlier, using either the passphrase or the recovery code.

mod common;

use common::make_user;
use frost_app_lib::commands;
use frost_app_lib::commands::backup::{export_backup_to, restore_backup_from};
use frost_app_lib::state::AppState;
use tauri::Manager;

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_restores_identity_and_wallet_birthdays_on_a_new_device() {
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let original = make_user();
    let state = original.app.state::<AppState>();
    let phrase = commands::keystore::create_keystore(state.clone(), "original-passphrase".into()).await.unwrap();
    let identity = commands::keystore::get_identity(state.clone()).await.unwrap();
    // Recorded by key generation and wallet setup on the original device.
    state
        .update_private(|p| {
            p.network_birthdays.insert("mainnet/abc".into(), 3_527_461);
            p.group_created_at.insert("abc".into(), 1_792_731_000);
        })
        .await
        .unwrap();

    assert!(commands::backup::backup_status(state.clone()).await.unwrap().last_backup_at.is_none());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("member.cyzebackup");
    let status = export_backup_to(&state, &file).await.unwrap();
    assert!(status.up_to_date && status.last_backup_at.is_some());
    let backup_text = std::fs::read_to_string(&file).unwrap();
    assert!(!backup_text.contains("abc"), "group ids must not appear in the clear");

    // New device, unlocked with the passphrase.
    let device = make_user();
    let restored = device.app.state::<AppState>();
    restore_backup_from(&restored, &file).await.unwrap();
    assert!(restore_backup_from(&restored, &file).await.is_err(), "never replaces a keystore");
    commands::keystore::unlock_keystore(restored.clone(), "original-passphrase".into()).await.unwrap();
    let id = commands::keystore::get_identity(restored.clone()).await.unwrap();
    assert_eq!(id.pubkey, identity.pubkey, "same member identity");
    let private = restored.load_private().await.unwrap();
    assert_eq!(private.network_birthdays.get("mainnet/abc"), Some(&3_527_461));
    assert_eq!(private.group_created_at.get("abc"), Some(&1_792_731_000));

    // Another device, recovered with the recovery code (passphrase forgotten).
    let device = make_user();
    let recovered = device.app.state::<AppState>();
    restore_backup_from(&recovered, &file).await.unwrap();
    commands::keystore::recover_keystore(recovered.clone(), phrase.clone(), "new-passphrase".into())
        .await
        .unwrap();
    assert_eq!(commands::keystore::get_identity(recovered.clone()).await.unwrap().pubkey, identity.pubkey);
    assert_eq!(recovered.load_private().await.unwrap().network_birthdays.get("mainnet/abc"), Some(&3_527_461));
}
