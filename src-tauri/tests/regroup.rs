//! Moving to a new group (to change members or threshold) between two
//! members' apps over a real frostd: payees copy over, untrusted, without
//! duplicates, and each app recognises an address of a group it's in.

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

/// A second 2-of-2 group of the pair (`dkg` waits for their first).
async fn second_group(p: &common::Pair, first: &str) -> String {
    let args = |participants: Vec<String>| commands::dkg::StartDkgArgs {
        suite: Suite::RedPallas,
        description: "new group".into(),
        threshold: 2,
        participants,
        server_url: Some(p.server_url.clone()),
        session_id: None,
        agree_viewing_key: true,
    };
    // The pair's first group has sessions open too, so Bob joins this one by id.
    let bob = p.bob.app.state::<AppState>();
    let sessions = async || -> Vec<uuid::Uuid> {
        commands::signing::list_pending_sessions(bob.clone(), None).await.unwrap().iter().map(|s| s.session_id).collect()
    };
    let before = sessions().await;
    commands::dkg::start_dkg(p.alice.app.handle().clone(), args(vec![p.bob_pubkey.clone()])).await.unwrap();
    let mut session = None;
    for _ in 0..60 {
        session = sessions().await.into_iter().find(|s| !before.contains(s));
        if session.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let joining = commands::dkg::StartDkgArgs { session_id: session, ..args(vec![]) };
    commands::dkg::start_dkg(p.bob.app.handle().clone(), joining).await.unwrap();
    let mut found = Vec::new();
    for user in [&p.alice, &p.bob] {
        let state = user.app.state::<AppState>();
        for _ in 0..240 {
            let groups = commands::groups::list_groups(state.clone()).await.unwrap();
            if let Some(g) = groups.iter().find(|g| g.id != first) {
                found.push(g.id.clone());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    assert!(found.len() == 2 && found[0] == found[1], "the second group never formed on both apps");
    found.remove(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn payees_copy_to_the_new_group_and_members_recognise_its_address() {
    let Some(server) = start_server().await else {
        eprintln!("skipping: sidecar frostd binary not built (run scripts/build-sidecar.sh)");
        return;
    };
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let p = pair(&server).await;
    let (alice, bob) = (p.alice.app.state::<AppState>(), p.bob.app.state::<AppState>());
    let alice_app = || p.alice.app.handle().clone();
    let old = dkg(&p, Suite::RedPallas).await;
    let new = second_group(&p, &old).await;

    let save = |group: &str, name: &str, seed: u8| {
        commands::payees::save_payee(alice_app(), group.to_string(), None, name.into(), address(seed))
    };
    save(&old, "Auditor", 1).await.unwrap();
    save(&old, "Landlord", 2).await.unwrap();
    save(&new, "Landlord (new)", 2).await.unwrap();

    // The landlord is in the new group already, by address: only the auditor
    // is added, as Alice's, and nobody else trusts it there yet.
    assert_eq!(commands::payees::copy_payees(alice_app(), old.clone(), new.clone()).await.unwrap(), 1);
    assert_eq!(commands::payees::copy_payees(alice_app(), old.clone(), new.clone()).await.unwrap(), 0, "nothing twice");
    assert!(commands::payees::copy_payees(alice_app(), old.clone(), old.clone()).await.is_err());
    let payees_of = async |state: &tauri::State<'_, AppState>| {
        commands::payees::list_payees(state.clone(), new.clone()).await.unwrap()
    };
    until(&p, &new, "Bob never got the copied payee", async || payees_of(&bob).await.len() == 2).await;
    let auditor = payees_of(&bob).await.into_iter().find(|p| p.payee.name == "Auditor").unwrap();
    assert_eq!((auditor.payee.address.as_str(), auditor.payee.author.as_str()), (address(1).as_str(), p.alice_pubkey.as_str()));
    assert_eq!(auditor.payee.trusted_by, vec![p.alice_pubkey.clone()]);

    // Both apps recognise the new group's address, from their own keys.
    let keys = commands::groups::group_orchard_keys(alice.clone(), new.clone()).await.unwrap().unwrap();
    for state in [&alice, &bob] {
        let own = commands::groups::own_group_for_address(state.clone(), keys.address.clone()).await.unwrap();
        assert_eq!(own.map(|g| g.group_id), Some(new.clone()));
        assert!(commands::groups::own_group_for_address(state.clone(), address(1)).await.unwrap().is_none());
    }
}
