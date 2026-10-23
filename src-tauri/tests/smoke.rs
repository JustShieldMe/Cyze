//! Headless smoke test of the full Tauri command layer.
//!
//! Uses Tauri's mock runtime (no display required) to drive two app
//! instances — Alice and Bob — through the complete user journey:
//! keystore creation, contact exchange, server trust, a 2-of-2 DKG, and a
//! signing session with the approval gate, against a real frostd.

mod common;

use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use common::{make_user, start_server, wait_for_group};
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use tauri::{Listener, Manager};

#[tokio::test(flavor = "multi_thread")]
async fn full_user_journey() {
    tauri::async_runtime::set(tokio::runtime::Handle::current());

    let Some(server) = start_server().await else {
        eprintln!("skipping: sidecar frostd binary not built (run scripts/build-sidecar.sh)");
        return;
    };
    let server_url = format!("127.0.0.1:{}", server.port);

    let alice = make_user();
    let bob = make_user();
    let alice_state = alice.app.state::<AppState>();
    let bob_state = bob.app.state::<AppState>();

    // Surface ceremony failures and progress in test output.
    for (name, user) in [("alice", &alice), ("bob", &bob)] {
        for event in ["dkg:progress", "dkg:failed", "signing:progress", "signing:failed"] {
            let label = format!("{name} {event}");
            user.app.handle().listen(event, move |e| {
                eprintln!("[{label}] {}", e.payload());
            });
        }
    }

    // --- Keystores ---
    commands::keystore::create_keystore(alice_state.clone(), "alice-passphrase".into())
        .await
        .expect("alice keystore");
    commands::keystore::create_keystore(bob_state.clone(), "bob-passphrase".into())
        .await
        .expect("bob keystore");
    let status = commands::keystore::keystore_status(alice_state.clone())
        .await
        .unwrap();
    assert!(status.exists && status.unlocked);

    // Lock/unlock round-trip exercises the encrypted file path.
    commands::keystore::lock_keystore(alice_state.clone())
        .await
        .unwrap();
    assert!(
        commands::keystore::unlock_keystore(alice_state.clone(), "wrong".into())
            .await
            .is_err(),
        "wrong passphrase must fail"
    );
    commands::keystore::unlock_keystore(alice_state.clone(), "alice-passphrase".into())
        .await
        .expect("unlock with correct passphrase");

    // --- Contact exchange ---
    let alice_contact = commands::contacts::export_my_contact(alice_state.clone(), "alice".into())
        .await
        .unwrap();
    let bob_contact = commands::contacts::export_my_contact(bob_state.clone(), "bob".into())
        .await
        .unwrap();
    commands::contacts::add_contact(alice_state.clone(), bob_contact.text.clone(), None)
        .await
        .expect("alice imports bob");
    commands::contacts::add_contact(bob_state.clone(), alice_contact.text.clone(), None)
        .await
        .expect("bob imports alice");
    assert_eq!(
        commands::contacts::list_contacts(alice_state.clone())
            .await
            .unwrap()
            .len(),
        1
    );

    // --- Server config + cert trust (TOFU) ---
    for state in [&alice_state, &bob_state] {
        commands::server::set_server_url((*state).clone(), server_url.clone())
            .await
            .unwrap();
        commands::server::trust_server_cert(
            (*state).clone(),
            server_url.clone(),
            server.cert_pem.clone(),
        )
        .await
        .unwrap();
        let test = commands::server::test_server_connection((*state).clone(), server_url.clone())
            .await
            .unwrap();
        assert!(test.ok, "connection test must pass: {:?}", test.error);
    }

    // --- DKG: Alice initiates a 2-of-2 with Bob; Bob joins ---
    commands::dkg::start_dkg(
        alice.app.handle().clone(),
        commands::dkg::StartDkgArgs {
            suite: frost_app_core::ciphersuite::Suite::Ed25519,
            description: "smoke test group".into(),
            threshold: 2,
            participants: vec![bob_contact.pubkey.clone()],
            server_url: Some(server_url.clone()),
            session_id: None,
        },
    )
    .await
    .expect("alice starts DKG");

    tokio::time::sleep(Duration::from_millis(1500)).await;
    commands::dkg::start_dkg(
        bob.app.handle().clone(),
        commands::dkg::StartDkgArgs {
            suite: frost_app_core::ciphersuite::Suite::Ed25519,
            description: "smoke test group".into(),
            threshold: 2,
            participants: vec![],
            server_url: Some(server_url.clone()),
            session_id: None,
        },
    )
    .await
    .expect("bob joins DKG");

    // Wait until the group lands in both keystores.
    let group_id = wait_for_group(&alice_state).await;
    let bob_group_id = wait_for_group(&bob_state).await;
    assert_eq!(group_id, bob_group_id, "both must derive the same group");

    // --- Signing: Alice coordinates, both sign (2-of-2) ---
    let (sig_tx, sig_rx) = std_mpsc::channel::<String>();
    alice.app.handle().listen("signing:complete", move |event| {
        let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
        if let Some(sig) = payload["signature_hex"].as_str() {
            let _ = sig_tx.send(sig.to_string());
        }
    });

    // Capture the signing session id the coordinator creates, so the test
    // is deterministic even if a just-completed DKG session is still being
    // torn down on the server.
    let (sid_tx, sid_rx) = std_mpsc::channel::<String>();
    alice.app.handle().listen("signing:progress", move |event| {
        let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
        if payload["event"]["phase"] == "session_created" {
            if let Some(sid) = payload["event"]["session_id"].as_str() {
                let _ = sid_tx.send(sid.to_string());
            }
        }
    });

    let message_hex = hex::encode(b"smoke test message");
    commands::signing::create_signing_session(
        alice.app.handle().clone(),
        commands::signing::CreateSigningSessionArgs {
            group_id: group_id.clone(),
            message_hex: message_hex.clone(),
            signers: vec![alice_contact.pubkey.clone(), bob_contact.pubkey.clone()],
            server_url: Some(server_url.clone()),
        },
    )
    .await
    .expect("alice creates signing session");

    let session_id: uuid::Uuid = tokio::task::spawn_blocking(move || {
        sid_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("coordinator must create a session")
    })
    .await
    .unwrap()
    .parse()
    .unwrap();

    // Bob's inbox must surface this exact signing session (alongside any
    // session still being cleaned up), attributed to alice and matching the
    // group.
    {
        let mut seen = false;
        for _ in 0..30 {
            let pending = commands::signing::list_pending_sessions(
                bob_state.clone(),
                Some(server_url.clone()),
            )
            .await
            .unwrap_or_default();
            if let Some(p) = pending.iter().find(|p| p.session_id == session_id) {
                assert_eq!(p.coordinator.as_deref(), Some("alice"));
                assert!(p.matching_groups.contains(&group_id));
                seen = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        assert!(seen, "bob must see the signing session in his inbox");
    }

    // Alice is the coordinator and also a selected signer, so she contributes
    // her share locally — only Bob joins over the network. Bob approves (the
    // approval oneshot is buffered, so approving right after join is valid).
    let bob_ceremony = commands::signing::join_signing_session(
        bob.app.handle().clone(),
        commands::signing::JoinSigningSessionArgs {
            group_id: group_id.clone(),
            session_id,
            server_url: Some(server_url.clone()),
        },
    )
    .await
    .expect("join signing session");
    commands::signing::respond_to_signing(
        bob.app.state::<AppState>().clone(),
        bob_ceremony,
        true,
    )
    .await
    .expect("approve");

    // The coordinator must emit the final signature.
    let signature_hex = tokio::task::spawn_blocking(move || {
        sig_rx
            .recv_timeout(Duration::from_secs(60))
            .expect("signature within 60s")
    })
    .await
    .unwrap();
    assert!(!signature_hex.is_empty());
    eprintln!("smoke test signature: {signature_hex}");
}
