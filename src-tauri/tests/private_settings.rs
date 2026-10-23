//! Per-group settings are kept encrypted, readable only while unlocked, and an
//! older version's plaintext values and group-named wallet folders move over
//! on the first unlock.

mod common;

use common::make_user;
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use tauri::Manager;

const GROUP: &str = "6ebb833c0000000000000000000000000000000000000000000000000000007b07";
const PUBKEY: &str = "a1b2c3d4";

#[tokio::test(flavor = "multi_thread")]
async fn plaintext_group_settings_move_into_encrypted_storage_on_unlock() {
    let user = make_user();
    let state = user.app.state::<AppState>();
    commands::keystore::create_keystore(state.clone(), "passphrase".into()).await.unwrap();
    state
        .mutate_config(|c| {
            c.group.insert(
                GROUP.into(),
                frost_client::cli::config::Group {
                    description: "Treasury".into(),
                    ciphersuite: "FROST(Pallas, BLAKE2b-512)".into(),
                    public_key_package: vec![],
                    key_package: vec![],
                    server_url: None,
                    participant: Default::default(),
                },
            );
            Ok(())
        })
        .await
        .unwrap();

    // What an older version left behind: group ids and a contact's nickname in
    // the clear, and a wallet folder named by group id.
    let mut settings = state.load_settings();
    settings.network_birthdays.insert(format!("mainnet/{GROUP}"), 3_527_461);
    settings.group_created_at.insert(GROUP.into(), 1_792_731_000);
    settings.active_group_id = Some(GROUP.into());
    settings.contact_aliases.insert(PUBKEY.into(), "Alice".into());
    settings.backup_fingerprint = Some("digest".into());
    state.save_settings(&settings).unwrap();
    let old_wallet = state.data_dir.join("wallets").join(GROUP).join("mainnet");
    std::fs::create_dir_all(old_wallet.join("pending")).unwrap();
    std::fs::write(old_wallet.join("wallet-v2.sqlite"), b"synced").unwrap();
    std::fs::write(old_wallet.join("pending").join("c1.pczt.hex"), b"signed").unwrap();

    commands::keystore::lock_keystore(state.clone()).await.unwrap();
    assert!(state.load_private().await.is_err(), "unreadable while locked");
    commands::keystore::unlock_keystore(state.clone(), "passphrase".into()).await.unwrap();

    let on_disk = std::fs::read_to_string(state.data_dir.join("settings.json")).unwrap();
    assert!(!on_disk.contains(GROUP) && !on_disk.contains("Alice") && !on_disk.contains("digest"), "{on_disk}");
    let sealed = std::fs::read(state.data_dir.join("private-settings.enc")).unwrap();
    assert!(!sealed.windows(GROUP.len()).any(|w| w == GROUP.as_bytes()));

    let private = state.load_private().await.unwrap();
    assert_eq!(private.network_birthdays.get(&format!("mainnet/{GROUP}")), Some(&3_527_461));
    assert_eq!(private.group_created_at.get(GROUP), Some(&1_792_731_000));
    assert_eq!(private.contact_aliases.get(PUBKEY).map(String::as_str), Some("Alice"));
    assert_eq!(private.backup_fingerprint.as_deref(), Some("digest"));
    let active = commands::server::get_active_wallet(state.clone()).await.unwrap();
    assert_eq!(active.as_deref(), Some(GROUP));

    // The wallet folder kept its contents under an opaque name.
    assert!(!state.data_dir.join("wallets").join(GROUP).exists());
    let wid = state.wallet_dir_id(GROUP).await.unwrap();
    assert!(!wid.contains(&GROUP[..16]));
    let moved = state.data_dir.join("wallets").join(&wid).join("mainnet");
    assert_eq!(std::fs::read(moved.join("wallet-v2.sqlite")).unwrap(), b"synced");
    assert_eq!(std::fs::read(moved.join("pending").join("c1.pczt.hex")).unwrap(), b"signed");

    // Later changes stay encrypted.
    commands::contacts::set_contact_alias(state.clone(), PUBKEY.into(), "Alice (ops)".into()).await.unwrap();
    assert!(!std::fs::read_to_string(state.data_dir.join("settings.json")).unwrap().contains("Alice"));
    let private = state.load_private().await.unwrap();
    assert_eq!(private.contact_aliases.get(PUBKEY).map(String::as_str), Some("Alice (ops)"));
}

#[tokio::test(flavor = "multi_thread")]
async fn ui_history_is_encrypted_and_needs_the_keystore_unlocked() {
    let user = make_user();
    let state = user.app.state::<AppState>();
    commands::keystore::create_keystore(state.clone(), "passphrase".into()).await.unwrap();
    use commands::ui_state::{load_ui_state, save_ui_state};

    let key = "frost-ceremonies".to_string();
    assert_eq!(load_ui_state(state.clone(), key.clone()).await.unwrap(), None);
    let history = r#"{"state":{"ceremonies":{"c1":{"memo":"rent for march"}}},"version":0}"#;
    save_ui_state(state.clone(), key.clone(), Some(history.into())).await.unwrap();
    let on_disk = std::fs::read(state.data_dir.join("ui-frost-ceremonies.enc")).unwrap();
    assert!(!on_disk.windows(4).any(|w| w == b"rent"), "saved in the clear");
    assert_eq!(load_ui_state(state.clone(), key.clone()).await.unwrap().as_deref(), Some(history));

    for bad in ["../settings", "UPPER", "", "a/b"] {
        assert!(save_ui_state(state.clone(), bad.into(), Some("x".into())).await.is_err(), "{bad:?}");
    }

    commands::keystore::lock_keystore(state.clone()).await.unwrap();
    assert!(load_ui_state(state.clone(), key.clone()).await.is_err());
    assert!(save_ui_state(state.clone(), key.clone(), Some("{}".into())).await.is_err());
    commands::keystore::unlock_keystore(state.clone(), "passphrase".into()).await.unwrap();
    assert_eq!(load_ui_state(state.clone(), key.clone()).await.unwrap().as_deref(), Some(history));

    save_ui_state(state.clone(), key.clone(), None).await.unwrap();
    assert_eq!(load_ui_state(state.clone(), key).await.unwrap(), None);
}
