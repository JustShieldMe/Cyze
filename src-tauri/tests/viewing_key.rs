//! Making an older group's viewing key private, through the app's commands:
//! two members' apps agree on a new seed over a real frostd, end up with the
//! same one, and switch to a private address while keeping the old wallet.

mod common;

use common::{bob_joins_viewing_key_session, dkg_with, pair, start_server, wait_until_private};
use frost_app_core::ciphersuite::Suite;
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use tauri::Manager;

#[tokio::test(flavor = "multi_thread")]
async fn an_older_group_makes_its_viewing_key_private() {
    let Some(server) = start_server().await else {
        eprintln!("skipping: sidecar frostd binary not built (run scripts/build-sidecar.sh)");
        return;
    };
    let p = pair(&server).await;
    let (alice, bob) = (p.alice.app.state::<AppState>(), p.bob.app.state::<AppState>());
    // A devnet network, so saving the key doesn't reach out for a chain tip.
    for state in [&alice, &bob] {
        commands::wallet::set_wallet_config((*state).clone(), "regtest".into(), "http://127.0.0.1:9".into(), None)
            .await
            .unwrap();
    }

    // A group made the way versions before 0.3 did: its viewing key comes from its id.
    let group_id = dkg_with(&p, Suite::RedPallas, false).await;
    let status = commands::viewing::viewing_key_status(alice.clone(), group_id.clone()).await.unwrap();
    assert!(!status.private && status.old_address.is_none());
    let old_address = commands::wallet::wallet_receive_address(alice.clone(), group_id.clone()).await.unwrap().address;
    let backup = tempfile::tempdir().unwrap();
    commands::backup::export_backup_to(&alice, &backup.path().join("cyze.backup")).await.unwrap();
    assert!(commands::backup::backup_status(alice.clone()).await.unwrap().up_to_date);

    commands::viewing::make_viewing_key_private(p.alice.app.handle().clone(), group_id.clone()).await.unwrap();
    bob_joins_viewing_key_session(&p, &group_id).await;
    wait_until_private(&alice, &group_id).await;
    wait_until_private(&bob, &group_id).await;

    let (a_seed, b_seed) = (alice.viewing_seed(&group_id).await.unwrap(), bob.viewing_seed(&group_id).await.unwrap());
    assert!(a_seed.is_some());
    assert_eq!(a_seed, b_seed, "both members have the same new viewing key");
    assert!(alice.has_old_viewing_key(&group_id).await.unwrap());

    // The wallet now hands out a private address, from a wallet of its own.
    let new_address = commands::wallet::wallet_receive_address(alice.clone(), group_id.clone()).await.unwrap().address;
    assert_ne!(new_address, old_address);
    assert_eq!(
        new_address,
        commands::wallet::wallet_receive_address(bob.clone(), group_id.clone()).await.unwrap().address
    );
    let old_dir = alice.wallet_dir_id_of(&group_id, frost_app_lib::state::WalletKind::Old).await.unwrap();
    assert_ne!(alice.wallet_dir_id(&group_id).await.unwrap(), old_dir);

    // A backup from before doesn't have the new key, so it's out of date.
    assert!(!commands::backup::backup_status(alice.clone()).await.unwrap().up_to_date);

    // The finished session isn't offered again.
    let sessions = commands::signing::list_pending_sessions(bob.clone(), None).await.unwrap();
    assert!(sessions.iter().all(|s| s.kind != "viewing_key"), "{:?}", sessions.len());

    // A group whose members agreed on a key when it was created has nothing to do.
    alice
        .mutate_keystore(|_, extras| {
            extras.old_viewing_keys.remove(&group_id);
            Ok(())
        })
        .await
        .unwrap();
    let err = commands::viewing::make_viewing_key_private(p.alice.app.handle().clone(), group_id.clone())
        .await
        .unwrap_err();
    assert!(err.message.contains("already private"), "{}", err.message);
}
