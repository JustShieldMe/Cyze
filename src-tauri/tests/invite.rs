//! Joining a group with an invite: the organizer starts key generation and
//! sends one code; members who only sent the organizer their contact code
//! join with it, and everyone ends up with the same 2-of-3 group.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{make_user, start_server};
use frost_app_core::ciphersuite::Suite;
use frost_app_lib::commands;
use frost_app_lib::commands::invite::CreateInviteArgs;
use frost_app_lib::state::AppState;
use tauri::{Listener, Manager};

async fn wait_for_groups(state: &tauri::State<'_, AppState>) -> frost_app_core::config::GroupSummary {
    for _ in 0..90 {
        if let Some(g) = commands::groups::list_groups(state.clone()).await.unwrap().into_iter().next() {
            return g;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("key generation did not finish within 90s");
}

#[tokio::test(flavor = "multi_thread")]
async fn members_join_with_an_invite_and_get_the_same_group() {
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let Some(server) = start_server().await else {
        eprintln!("skipping: sidecar frostd binary not built (run scripts/build-sidecar.sh)");
        return;
    };
    let server_url = format!("127.0.0.1:{}", server.port);
    let users: Vec<_> = (0..4).map(|_| make_user()).collect();
    let states: Vec<_> = users.iter().map(|u| u.app.state::<AppState>()).collect();
    let (alice, bob, carol, dave) = (&states[0], &states[1], &states[2], &states[3]);
    let mut contacts = Vec::new();
    for (state, name) in states.iter().zip(["alice", "bob", "carol", "dave"]) {
        commands::keystore::create_keystore(state.clone(), format!("{name}-passphrase")).await.unwrap();
        commands::keystore::set_username(state.clone(), name.into()).await.unwrap();
        contacts.push(commands::contacts::export_my_contact(state.clone(), name.into()).await.unwrap());
    }
    let pubkey = |i: usize| contacts[i].pubkey.clone();

    // Alice runs the server and has Bob's and Carol's contact codes. They know
    // nobody; Carol happens to have someone else saved as "alice".
    commands::server::trust_server_cert(alice.clone(), server_url.clone(), server.cert_pem.clone()).await.unwrap();
    for i in [1, 2] {
        commands::contacts::add_contact(alice.clone(), contacts[i].text.clone(), None).await.unwrap();
    }
    let impostor = frost_app_core::config::contact_to_text(
        "alice",
        &frost_client::api::PublicKey(hex::decode(pubkey(3)).unwrap()),
    )
    .unwrap();
    commands::contacts::add_contact(carol.clone(), impostor, None).await.unwrap();

    let session: Arc<Mutex<Option<String>>> = Arc::default();
    let sink = session.clone();
    users[0].app.handle().listen("dkg:progress", move |e| {
        let v: serde_json::Value = serde_json::from_str(e.payload()).unwrap();
        if let Some(id) = v["event"]["session_id"].as_str() {
            sink.lock().unwrap().get_or_insert(id.to_string());
        }
    });
    commands::dkg::start_dkg(
        users[0].app.handle().clone(),
        commands::dkg::StartDkgArgs {
            suite: Suite::RedPallas,
            description: "Treasury".into(),
            threshold: 2,
            participants: vec![pubkey(1), pubkey(2)],
            server_url: Some(server_url.clone()),
            session_id: None,
        },
    )
    .await
    .unwrap();
    let mut session_id = None;
    for _ in 0..40 {
        session_id = session.lock().unwrap().clone();
        if session_id.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let session_id = session_id.expect("the organizer's session started");

    let code = commands::invite::create_invite(
        alice.clone(),
        CreateInviteArgs {
            session_id: session_id.parse().unwrap(),
            suite: Suite::RedPallas,
            threshold: 2,
            name: "Treasury".into(),
            participants: vec![pubkey(1), pubkey(2)],
            server: server_url.clone(),
        },
    )
    .await
    .unwrap();

    // Bob sees what he's joining: the organizer and Carol are new to him, and
    // the self-signed server's certificate comes with it.
    let summary = commands::invite::read_invite(bob.clone(), code.clone()).await.unwrap();
    assert!(summary.for_you);
    assert_eq!((summary.name.as_str(), summary.threshold, summary.members.len()), ("Treasury", 2, 3));
    assert_eq!(summary.organizer.status, "new");
    let status = |name: &str| summary.members.iter().find(|m| m.name == name).unwrap().status;
    assert_eq!((status("alice"), status("bob"), status("carol")), ("new", "you", "new"));
    assert_eq!(
        summary.cert_fingerprint.as_deref(),
        Some(frost_app_core::tls::cert_fingerprint(&server.cert_pem).unwrap().as_str())
    );

    // Someone the invite wasn't for can read it but not join.
    assert!(!commands::invite::read_invite(dave.clone(), code.clone()).await.unwrap().for_you);
    assert!(commands::invite::accept_invite(users[3].app.handle().clone(), code.clone(), None).await.is_err());

    commands::invite::accept_invite(users[1].app.handle().clone(), code.clone(), None).await.unwrap();
    commands::invite::accept_invite(users[2].app.handle().clone(), code.clone(), Some("Our treasury".into()))
        .await
        .unwrap();

    let groups = [wait_for_groups(alice).await, wait_for_groups(bob).await, wait_for_groups(carol).await];
    for g in &groups {
        assert_eq!((g.threshold, g.num_participants), (2, 3));
        assert_eq!(g.id, groups[0].id, "everyone holds a share of the same key");
    }
    assert_eq!(groups[2].description, "Our treasury", "a member can name it their own way");

    // Bob now has both other members as contacts and uses the invite's server.
    let bob_contacts = commands::contacts::list_contacts(bob.clone()).await.unwrap();
    assert!(bob_contacts.iter().any(|c| c.pubkey == pubkey(0)) && bob_contacts.iter().any(|c| c.pubkey == pubkey(2)));
    assert_eq!(bob.load_settings().server_url.as_deref(), Some(server_url.as_str()));
    // Carol's other "alice" is untouched; the organizer was added beside it.
    let carol_contacts = commands::contacts::list_contacts(carol.clone()).await.unwrap();
    let named = |n: &str| carol_contacts.iter().find(|c| c.name == n).map(|c| c.pubkey.clone());
    assert_eq!(named("alice"), Some(pubkey(3)));
    assert_eq!(named("alice (2)"), Some(pubkey(0)));
}
