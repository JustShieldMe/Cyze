//! Scheduled payments between two members' apps over a real frostd: a
//! schedule one member makes reaches the other, only its author's app starts
//! its payments, records them sent or stops it, and a schedule the group's
//! rules would refuse is refused.

mod common;

use common::{dkg, pair, start_server, until};
use frost_app_core::ciphersuite::Suite;
use frost_app_core::polls::ProposedPayment;
use frost_app_core::schedules::Every;
use frost_app_lib::commands;
use frost_app_lib::commands::schedules::CreateScheduleArgs;
use frost_app_lib::state::AppState;
use tauri::Manager;

/// A mainnet address (the network apps start on) nobody in the test holds.
fn address(seed: u8) -> String {
    let sk = orchard::keys::SpendingKey::from_bytes([seed; 32]).unwrap();
    let ak: [u8; 32] = orchard::keys::FullViewingKey::from(&sk).to_bytes()[..32].try_into().unwrap();
    let net = frost_app_core::wallet::WalletNetwork::Main.network_type();
    frost_app_core::zcash::derive_orchard_keys(&ak, None, net).unwrap().address
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_members_schedule_reaches_the_group_and_only_its_author_runs_it() {
    let Some(server) = start_server().await else {
        eprintln!("skipping: sidecar frostd binary not built (run scripts/build-sidecar.sh)");
        return;
    };
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let p = pair(&server).await;
    let (alice, bob) = (p.alice.app.state::<AppState>(), p.bob.app.state::<AppState>());
    let alice_app = || p.alice.app.handle().clone();
    let bob_app = || p.bob.app.handle().clone();
    let group_id = dkg(&p, Suite::RedPallas).await;
    let first_due = now() - 3_600;
    let args = |signers: Vec<String>, first_due: u64, every: Every| CreateScheduleArgs {
        group_id: group_id.clone(),
        payments: vec![ProposedPayment { recipient: address(1), amount_zatoshis: 100_000_000, memo: Some(" March ".into()) }],
        reason: " Rent ".into(),
        every,
        first_due,
        signers,
    };
    let both = vec![p.alice_pubkey.clone(), p.bob_pubkey.clone()];
    let err = async |args| commands::schedules::create_schedule(alice_app(), args).await.unwrap_err().message;

    // Its author approves each payment, with enough others for the threshold,
    // from today on, at a valid interval.
    assert!(err(args(vec![p.bob_pubkey.clone()], first_due, Every::Months(1))).await.contains("you approve"));
    assert!(err(args(vec![p.alice_pubkey.clone()], first_due, Every::Months(1))).await.contains("at least 2"));
    assert!(err(args(both.clone(), now() - 3 * 86_400, Every::Months(1))).await.contains("between today"));
    assert!(err(args(both.clone(), first_due, Every::Days(0))).await.contains("how often"));

    let id = commands::schedules::create_schedule(alice_app(), args(both.clone(), first_due, Every::Months(1))).await.unwrap();
    let due = commands::schedules::due_schedules(alice.clone()).await.unwrap();
    assert_eq!(due.len(), 1, "due today, for Alice's app to start");
    assert_eq!((due[0].schedule.due_now, due[0].schedule.reason.as_str()), (Some(first_due), "Rent"));
    assert_eq!(due[0].schedule.payments[0].memo.as_deref(), Some("March"));

    let schedules_of =
        async |state: &tauri::State<'_, AppState>| commands::schedules::list_schedules(state.clone(), group_id.clone()).await.unwrap();
    until(&p, &group_id, "Bob never got the schedule", async || !schedules_of(&bob).await.is_empty()).await;
    let bobs = schedules_of(&bob).await;
    assert!(!bobs[0].mine);
    assert_eq!(bobs[0].schedule.author, p.alice_pubkey);
    assert!(commands::schedules::due_schedules(bob.clone()).await.unwrap().is_empty(), "Bob's app doesn't start it");
    assert!(commands::schedules::stop_schedule(bob_app(), group_id.clone(), id.clone()).await.is_err());
    let txid = "ab".repeat(32);
    let bob_records = commands::schedules::record_schedule_run(bob_app(), group_id.clone(), id.clone(), first_due, txid.clone());
    assert!(bob_records.await.is_err());

    // Alice's app records the payment sent: it isn't due again, and Bob sees
    // the transaction. Only a date of the schedule, once.
    let record = |due: u64, txid: String| commands::schedules::record_schedule_run(alice_app(), group_id.clone(), id.clone(), due, txid);
    assert!(record(first_due + 60, txid.clone()).await.unwrap_err().message.contains("isn't a date"));
    record(first_due, txid.clone()).await.unwrap();
    record(first_due, txid.to_uppercase()).await.expect("recording the same payment again is harmless");
    assert!(record(first_due, "cd".repeat(32)).await.unwrap_err().message.contains("already sent"));
    assert!(commands::schedules::due_schedules(alice.clone()).await.unwrap().is_empty());
    until(&p, &group_id, "Bob never saw the payment sent", async || {
        schedules_of(&bob).await[0].schedule.runs.get(&first_due) == Some(&txid)
    })
    .await;

    // Alice stops it; Bob's app follows.
    commands::schedules::stop_schedule(alice_app(), group_id.clone(), id.clone()).await.unwrap();
    until(&p, &group_id, "Bob never saw it stopped", async || schedules_of(&bob).await[0].schedule.stopped).await;

    // Once the group allows payments only to trusted payees, a schedule
    // paying anyone else is refused.
    use frost_app_core::rules::GroupRules;
    let rules = GroupRules { trusted_payees_only: true, ..GroupRules::default() };
    let poll_id = commands::polls::create_poll(
        alice_app(),
        commands::polls::CreatePollArgs {
            group_id: group_id.clone(),
            title: "Trusted payees only".into(),
            details: String::new(),
            options: Vec::new(),
            open_for_secs: 24 * 60 * 60,
            payments: Vec::new(),
            rules: Some(rules.clone()),
        },
    )
    .await
    .unwrap();
    until(&p, &group_id, "Bob never got the proposal", async || {
        commands::polls::list_polls(bob.clone(), None).await.unwrap().iter().any(|q| q.poll_id == poll_id)
    })
    .await;
    for app in [alice_app(), bob_app()] {
        commands::polls::vote_poll(app, group_id.clone(), poll_id.clone(), 0).await.unwrap();
    }
    until(&p, &group_id, "the rule never took effect", async || {
        commands::rules::group_rules(alice.clone(), group_id.clone()).await.unwrap().rules == rules
    })
    .await;
    assert!(err(args(both.clone(), now(), Every::Days(7))).await.contains("only to trusted payees"));
}
