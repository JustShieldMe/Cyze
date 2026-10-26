//! A group's shared payees and payment reasons between two members' apps over
//! a real frostd: a payee one member saves reaches the other, only the member
//! who added it can change or remove it, a name or address can't be saved
//! twice, it's trusted once both trust it, the group adopts rules by poll, and
//! what a member says a payment was for reaches the others.

mod common;

use common::{dkg, pair, start_server, until};
use frost_app_core::ciphersuite::Suite;
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use tauri::Manager;

/// A mainnet address (the network apps start on) nobody in the test holds.
fn address(seed: u8) -> String {
    let sk = orchard::keys::SpendingKey::from_bytes([seed; 32]).unwrap();
    let ak: [u8; 32] = orchard::keys::FullViewingKey::from(&sk).to_bytes()[..32].try_into().unwrap();
    let net = frost_app_core::wallet::WalletNetwork::Main.network_type();
    frost_app_core::zcash::derive_orchard_keys(&ak, None, net).unwrap().address
}

#[tokio::test(flavor = "multi_thread")]
async fn members_share_payees_and_what_payments_were_for() {
    let Some(server) = start_server().await else {
        eprintln!("skipping: sidecar frostd binary not built (run scripts/build-sidecar.sh)");
        return;
    };
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let p = pair(&server).await;
    let (alice, bob) = (p.alice.app.state::<AppState>(), p.bob.app.state::<AppState>());
    let group_id = dkg(&p, Suite::RedPallas).await;
    let payees_of = async |state: &tauri::State<'_, AppState>| {
        commands::payees::list_payees(state.clone(), group_id.clone()).await.unwrap()
    };

    // Alice saves a payee; it reaches Bob as hers.
    let auditor = commands::payees::save_payee(
        p.alice.app.handle().clone(),
        group_id.clone(),
        None,
        " Auditor ".into(),
        address(1),
    )
    .await
    .unwrap();
    until(&p, &group_id, "Bob never got the payee", async || !payees_of(&bob).await.is_empty()).await;
    let bobs = payees_of(&bob).await;
    assert_eq!((bobs[0].payee.name.as_str(), bobs[0].payee.address.as_str()), ("Auditor", address(1).as_str()));
    assert_eq!(bobs[0].payee.author, p.alice_pubkey);
    assert!(!bobs[0].mine);
    assert!(payees_of(&alice).await[0].mine);

    // Bob can't change or remove it, or save its name or address again.
    let bob_app = || p.bob.app.handle().clone();
    let err = |r: Result<String, frost_app_lib::error::AppError>| r.unwrap_err().message;
    assert!(commands::payees::remove_payee(bob_app(), group_id.clone(), auditor.clone()).await.is_err());
    let changed = commands::payees::save_payee(bob_app(), group_id.clone(), Some(auditor.clone()), "Mine now".into(), address(9));
    assert!(err(changed.await).contains("only the member who added"));
    let same_name = commands::payees::save_payee(bob_app(), group_id.clone(), None, "auditor".into(), address(2));
    assert!(err(same_name.await).contains("already a payee called Auditor"));
    let same_address = commands::payees::save_payee(bob_app(), group_id.clone(), None, "Accounts".into(), address(1));
    assert!(err(same_address.await).contains("already a payee: Auditor"));
    let not_an_address = commands::payees::save_payee(bob_app(), group_id.clone(), None, "Typo".into(), "u1nope".into());
    assert!(not_an_address.await.is_err());

    // Bob trusts it; with Alice, who added it, that's both of a 2-of-2.
    assert!(!payees_of(&alice).await[0].payee.trusted);
    commands::payees::trust_payee(bob_app(), group_id.clone(), p.alice_pubkey.clone(), auditor.clone(), true).await.unwrap();
    until(&p, &group_id, "Alice never saw Bob's trust", async || payees_of(&alice).await[0].payee.trusted).await;
    assert_eq!(payees_of(&alice).await[0].payee.trusted_by, vec![p.alice_pubkey.clone(), p.bob_pubkey.clone()]);

    // Rules are proposed in a poll, and in effect once both adopt them. A
    // rule needing more approvals than the group has members is refused.
    use frost_app_core::rules::{GroupRules, LargePayments};
    let propose = |rules: GroupRules| {
        commands::polls::create_poll(
            p.alice.app.handle().clone(),
            commands::polls::CreatePollArgs {
                group_id: group_id.clone(),
                title: "Rules for the treasury".into(),
                details: String::new(),
                options: Vec::new(),
                open_for_secs: 24 * 60 * 60,
                payments: Vec::new(),
                rules: Some(rules),
            },
        )
    };
    let impossible = GroupRules { trusted_payees_only: false, large: Some(LargePayments { from_zatoshis: 1, approvals: 3 }) };
    assert!(propose(impossible).await.unwrap_err().message.contains("the group has 2 members"));
    let rules = GroupRules { trusted_payees_only: true, large: None };
    let poll_id = propose(rules.clone()).await.unwrap();
    let in_effect = async |state: &tauri::State<'_, AppState>| {
        commands::rules::group_rules(state.clone(), group_id.clone()).await.unwrap()
    };
    assert!(in_effect(&alice).await.rules.is_empty(), "not until the group agrees");
    until(&p, &group_id, "Bob never got the proposal", async || {
        commands::polls::list_polls(bob.clone(), None).await.unwrap().iter().any(|q| q.poll_id == poll_id)
    })
    .await;
    for app in [p.alice.app.handle().clone(), bob_app()] {
        commands::polls::vote_poll(app, group_id.clone(), poll_id.clone(), 0).await.unwrap();
    }
    until(&p, &group_id, "the rules never took effect", async || {
        in_effect(&alice).await.rules == rules && in_effect(&bob).await.rules == rules
    })
    .await;
    assert_eq!(in_effect(&bob).await.poll_id, Some(poll_id));

    // Alice renames it, then removes it; Bob's app follows.
    commands::payees::save_payee(
        p.alice.app.handle().clone(),
        group_id.clone(),
        Some(auditor.clone()),
        "Auditor (2026)".into(),
        address(1),
    )
    .await
    .unwrap();
    until(&p, &group_id, "Bob never saw the new name", async || {
        payees_of(&bob).await.first().is_some_and(|p| p.payee.name == "Auditor (2026)")
    })
    .await;
    commands::payees::remove_payee(p.alice.app.handle().clone(), group_id.clone(), auditor).await.unwrap();
    until(&p, &group_id, "Bob still has the removed payee", async || payees_of(&bob).await.is_empty()).await;

    // Bob says what a payment was for; Alice's app gets it.
    let txid = "AB".repeat(32);
    commands::payees::set_payment_reason(bob_app(), group_id.clone(), txid.clone(), "  Approved at the March meeting ".into())
        .await
        .unwrap();
    until(&p, &group_id, "Alice never got the reason", async || {
        !commands::payees::list_payment_reasons(alice.clone(), group_id.clone()).await.unwrap().is_empty()
    })
    .await;
    let reasons = commands::payees::list_payment_reasons(alice.clone(), group_id.clone()).await.unwrap();
    assert_eq!(reasons[0].txid, txid.to_lowercase());
    assert_eq!(reasons[0].text, "Approved at the March meeting");
    assert_eq!(reasons[0].author, p.bob_pubkey);
}
