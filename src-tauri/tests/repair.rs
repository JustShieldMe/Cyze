//! Repairing a member's lost share over a real frostd, in a 2-of-4 group:
//! Carol loses her device, Alice and Bob help, and Dave stands in for Carol's
//! new device. Dave's app rebuilds exactly Carol's key package and gets the
//! group's viewing seed; Alice's and Bob's apps switch to his key; Erin's,
//! who didn't help, switches once both helpers vouch for it.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{group_of, user_on};
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use tauri::{Listener, Manager};

/// Read every group-message session waiting for a member, as the app does,
/// shrugging off the ones it refuses (as it does, logging them).
async fn pump(m: &common::Member, group_id: &str) {
    for s in commands::signing::list_pending_sessions(state(m), None).await.unwrap_or_default() {
        if s.kind == "group_message" {
            let _ = commands::polls::receive_group_messages(m.user.app.handle().clone(), group_id.to_string(), s.session_id).await;
        }
    }
}

fn state(m: &common::Member) -> tauri::State<'_, AppState> {
    m.user.app.state::<AppState>()
}

/// Carol's entry in a member's copy of the group: identifier (hex) and key.
async fn member_key(state: &tauri::State<'_, AppState>, group_id: &str, identifier: &str) -> String {
    state
        .with_config(|c| Ok(hex::encode(&c.group[group_id].participant[identifier].pubkey.0)))
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn helpers_repair_a_lost_share_for_the_members_new_device() {
    let Some(server) = common::start_server().await else {
        eprintln!("skipping: sidecar frostd binary not built (run scripts/build-sidecar.sh)");
        return;
    };
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let (members, group_id) = group_of(&server, &["alice", "bob", "carol", "erin"], 2).await;
    let [alice, bob, carol, erin] = &members[..] else { unreachable!() };
    let carol_id = state(alice)
        .with_config(|c| {
            Ok(c.group[&group_id].participant.iter().find(|(_, p)| hex::encode(&p.pubkey.0) == carol.pubkey).unwrap().0.clone())
        })
        .await
        .unwrap();
    let carols_package = state(carol).with_config(|c| Ok(c.group[&group_id].key_package.clone())).await.unwrap();
    let seed = state(alice).viewing_seed(&group_id).await.unwrap().map(|s| *s);
    assert!(seed.is_some());

    // Carol's new device: a fresh keystore, a new key. She gives the group her
    // new contact code.
    let dave = user_on(&server, "carol").await;
    let new_contact = commands::contacts::export_my_contact(state(&dave), "carol".into()).await.unwrap().text;

    // Alice starts the repair with Bob, and hands Carol the repair code.
    let started = commands::repair::start_repair(
        alice.user.app.handle().clone(),
        group_id.clone(),
        carol_id.clone(),
        new_contact,
        vec![alice.pubkey.clone(), bob.pubkey.clone()],
    )
    .await
    .unwrap();
    let info = commands::repair::read_repair_code(started.code.clone()).await.unwrap();
    assert_eq!((info.group.as_str(), info.member.as_str()), ("test group", "carol"));
    let done: Arc<Mutex<Vec<String>>> = Arc::default();
    for (who, m) in [("alice", alice), ("dave", &dave), ("bob", bob)] {
        let done = done.clone();
        m.user.app.handle().listen("repair:complete", move |_| done.lock().unwrap().push(who.into()));
        let failed = who.to_string();
        m.user.app.handle().listen("repair:failed", move |e| panic!("{failed}'s repair failed: {}", e.payload()));
    }
    commands::repair::receive_repair(dave.user.app.handle().clone(), started.code).await.unwrap();

    // Bob sees the request, checks the new key with Carol, and approves.
    let asked: Arc<Mutex<Option<serde_json::Value>>> = Arc::default();
    let seen = asked.clone();
    bob.user.app.handle().listen("repair:progress", move |e| {
        let v: serde_json::Value = serde_json::from_str(e.payload()).unwrap();
        if v["event"]["phase"] == "awaiting_approval" {
            *seen.lock().unwrap() = Some(v);
        }
    });
    let started_at = Instant::now();
    let session = loop {
        let sessions = commands::signing::list_pending_sessions(state(bob), None).await.unwrap();
        if let Some(s) = sessions.into_iter().find(|s| s.kind == "repair") {
            assert_eq!(s.matching_groups, vec![group_id.clone()]);
            break s.session_id;
        }
        assert!(started_at.elapsed() < Duration::from_secs(30), "Bob never saw the repair");
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    let ceremony = commands::repair::join_repair(bob.user.app.handle().clone(), group_id.clone(), session).await.unwrap();
    let request = loop {
        if let Some(v) = asked.lock().unwrap().clone() {
            break v;
        }
        assert!(started_at.elapsed() < Duration::from_secs(60), "Bob was never asked");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(request["event"]["new_key"], dave.pubkey.as_str());
    assert_eq!(request["event"]["member"], carol.pubkey.as_str());
    commands::signing::respond_to_signing(state(bob), ceremony, true).await.unwrap();

    while done.lock().unwrap().len() < 3 {
        assert!(started_at.elapsed() < Duration::from_secs(120), "the repair didn't finish: {:?}", done.lock().unwrap());
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    // Dave's app holds exactly Carol's share, and the group's viewing key.
    let daves_package = state(&dave).with_config(|c| Ok(c.group[&group_id].key_package.clone())).await.unwrap();
    assert_eq!(daves_package, carols_package);
    assert_eq!(state(&dave).viewing_seed(&group_id).await.unwrap().map(|s| *s), seed);
    assert_eq!(member_key(&state(&dave), &group_id, &carol_id).await, dave.pubkey);

    // The helpers use his key now; Erin's app does once both have vouched.
    assert_eq!(member_key(&state(alice), &group_id, &carol_id).await, dave.pubkey);
    let switched = Instant::now();
    while member_key(&state(bob), &group_id, &carol_id).await != dave.pubkey
        || member_key(&state(erin), &group_id, &carol_id).await != dave.pubkey
    {
        for m in [alice, bob, erin] {
            pump(m, &group_id).await;
        }
        if switched.elapsed() > Duration::from_secs(60) {
            let bob_key = member_key(&state(bob), &group_id, &carol_id).await;
            let erin_key = member_key(&state(erin), &group_id, &carol_id).await;
            panic!("not switched: bob {} erin {} (dave {}, carol {}), done {:?}", &bob_key[..8], &erin_key[..8], &dave.pubkey[..8], &carol.pubkey[..8], done.lock().unwrap());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
