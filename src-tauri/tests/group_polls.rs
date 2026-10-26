//! Group polls between two members' apps over a real frostd: a poll reaches
//! the other member, votes go both ways, the threshold marks agreement, and a
//! member who missed an update gets it by asking.

mod common;

use std::time::{Duration, Instant};

use common::{dkg, pair, polls_of, start_server, until};
use frost_app_core::ciphersuite::Suite;
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use tauri::Manager;

#[tokio::test(flavor = "multi_thread")]
async fn members_poll_each_other() {
    let Some(server) = start_server().await else {
        eprintln!("skipping: sidecar frostd binary not built (run scripts/build-sidecar.sh)");
        return;
    };
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let p = pair(&server).await;
    let bob = p.bob.app.state::<AppState>();
    let group_id = dkg(&p, Suite::RedPallas).await;

    // Alice asks.
    let poll_id = commands::polls::create_poll(
        p.alice.app.handle().clone(),
        commands::polls::CreatePollArgs {
            group_id: group_id.clone(),
            title: "Hire an auditor?".into(),
            details: "Quotes attached in the group chat.".into(),
            options: vec!["Yes".into(), "No".into()],
            open_for_secs: 3 * 24 * 60 * 60,
            payments: Vec::new(),
            rules: None,
        },
    )
    .await
    .unwrap();
    until(&p, &group_id, "Bob never got the poll", async || !polls_of(&p.bob).await.is_empty()).await;
    let bobs = polls_of(&p.bob).await;
    assert_eq!(bobs[0].poll_id, poll_id);
    assert_eq!(bobs[0].author, p.alice_pubkey);
    assert_eq!(bobs[0].details, "Quotes attached in the group chat.");

    // Both answer; Bob changes his mind. Two of two agreeing is agreement.
    commands::polls::vote_poll(p.bob.app.handle().clone(), group_id.clone(), poll_id.clone(), 1).await.unwrap();
    commands::polls::vote_poll(p.bob.app.handle().clone(), group_id.clone(), poll_id.clone(), 0).await.unwrap();
    commands::polls::vote_poll(p.alice.app.handle().clone(), group_id.clone(), poll_id.clone(), 0).await.unwrap();
    until(&p, &group_id, "the votes never met", async || {
        let (a, b) = (polls_of(&p.alice).await, polls_of(&p.bob).await);
        a[0].tally == vec![2, 0] && b[0].tally == vec![2, 0]
    })
    .await;
    let a = &polls_of(&p.alice).await[0];
    assert_eq!(a.agreed, Some(0));
    assert_eq!(a.votes.get(&p.bob_pubkey), Some(&0), "Bob's latest answer counts");
    assert_eq!(a.my_vote, Some(0));

    // Only the member who asked can close it early.
    assert!(commands::polls::close_poll(p.bob.app.handle().clone(), group_id.clone(), poll_id.clone()).await.is_err());

    // Bob misses Alice's next poll (its session is gone before he reads it),
    // then catches up by asking.
    let before: Vec<_> = commands::signing::list_pending_sessions(bob.clone(), None).await.unwrap();
    let second = commands::polls::create_poll(
        p.alice.app.handle().clone(),
        commands::polls::CreatePollArgs {
            group_id: group_id.clone(),
            title: "Move to a 2-of-3?".into(),
            details: String::new(),
            options: vec!["Yes".into(), "No".into(), "Later".into()],
            open_for_secs: 24 * 60 * 60,
            payments: Vec::new(),
            rules: None,
        },
    )
    .await
    .unwrap();
    let started = Instant::now();
    loop {
        let now: Vec<_> = commands::signing::list_pending_sessions(bob.clone(), None).await.unwrap();
        let fresh: Vec<_> = now.iter().filter(|s| !before.iter().any(|b| b.session_id == s.session_id)).collect();
        if let Some(s) = fresh.iter().find(|s| s.kind == "group_message") {
            bob.group_message_sessions.lock().unwrap().insert(s.session_id);
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "Alice's second poll never went out");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(polls_of(&p.bob).await.len(), 1);
    commands::polls::refresh_polls(p.bob.app.handle().clone(), group_id.clone()).await.unwrap();
    until(&p, &group_id, "Bob never caught up", async || polls_of(&p.bob).await.len() == 2).await;
    assert!(polls_of(&p.bob).await.iter().any(|p| p.poll_id == second && p.options.len() == 3));
}
