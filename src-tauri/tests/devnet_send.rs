//! A real send on a local devnet, end to end. A RedPallas DKG creates the
//! group; the devnet faucet funds it and the background service syncs Alice's
//! wallet. Alice saves the recipient as a group payee, then coordinates a
//! 2-of-2 send with a memo and a reason for the group. Bob's app verifies the
//! transaction itself before he approves, and recognises the payee from his
//! own copy of the group's payees. Cyze applies the signature, proves, and
//! broadcasts; the devnet mines it, and Bob's app learns what it was for.
//! Then a batch of the most payments a transaction makes, a few with memos,
//! about as large a request as co-signers can verify, after one too large for
//! that is refused at review. Then a payment proposed in a group poll: sent
//! once both agree, only by Alice, who proposed it, checked by Bob's app
//! against his copy of the poll, and only once. Then the group adopts rules
//! by poll: larger payments wait an hour after they're announced, which is
//! refused unannounced, too soon, and once Bob vetoes; then rules replacing
//! those, to pay only trusted payees: a payment to anyone else is refused
//! before anyone is asked, and "send all" to the trusted auditor goes through.
//!
//! Needs thus-spoke-zakura 0.3.x (NU6.3 from block 1) or 0.4.x (NU7 from block
//! 1, with `CYZE_DEVNET_UPGRADE=nu7`). Ignored by default; run in release mode,
//! because Halo2 proving is very slow in debug:
//!
//! ```sh
//! ths start --name cyze-e2e --port-offset 700 --no-open      # another terminal
//! CYZE_DEVNET=cyze-e2e cargo test --release -p frost-app --test devnet_send -- --ignored --nocapture
//! ```

mod common;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{dkg, pair, start_server, until, UserApp};
use frost_app_core::ciphersuite::Suite;
use frost_app_core::wallet::{RegtestUpgrade, WalletNetwork};
use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use frost_app_lib::sync_service;
use serde_json::Value;
use tauri::{Listener, Manager};

fn devnet() -> String {
    std::env::var("CYZE_DEVNET").unwrap_or_else(|_| "cyze-e2e".into())
}

/// The upgrade the devnet runs: `nu6_3` for thus-spoke-zakura 0.3, `nu7` for
/// 0.4 (`CYZE_DEVNET_UPGRADE`).
fn devnet_upgrade() -> String {
    std::env::var("CYZE_DEVNET_UPGRADE").unwrap_or_else(|_| "nu6_3".into())
}

fn ths(args: &[&str]) -> String {
    let out = std::process::Command::new("ths")
        .args(args)
        .args(["--name", &devnet()])
        .output()
        .expect("ths (thus-spoke-zakura) on PATH");
    assert!(out.status.success(), "ths {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// The devnet's lightwalletd endpoint, from `ths endpoints --json`.
fn lightwalletd_url() -> String {
    let endpoints: Value = serde_json::from_str(&ths(&["endpoints", "--json"])).unwrap();
    endpoints["lightwalletd"].as_str().expect("lightwalletd endpoint").to_string()
}

type Events = Arc<Mutex<Vec<(String, Value)>>>;

fn record(user: &UserApp, names: &[&'static str]) -> Events {
    let events: Events = Arc::default();
    for &name in names {
        let sink = events.clone();
        user.app.handle().listen(name, move |e| {
            let v: Value = serde_json::from_str(e.payload()).unwrap();
            eprintln!("[{name}] {}", v.to_string().chars().take(300).collect::<String>());
            sink.lock().unwrap().push((name.to_string(), v));
        });
    }
    events
}

/// A unified address that isn't the group's (a throwaway Orchard key).
fn outside_recipient() -> String {
    outside_recipient_at(42)
}

/// [`outside_recipient`], one per `seed`.
fn outside_recipient_at(seed: u8) -> String {
    let sk = orchard::keys::SpendingKey::from_bytes([seed; 32]).unwrap();
    let ak: [u8; 32] = orchard::keys::FullViewingKey::from(&sk).to_bytes()[..32].try_into().unwrap();
    let net = WalletNetwork::Regtest(RegtestUpgrade::Nu6_3).network_type();
    frost_app_core::zcash::derive_orchard_keys(&ak, None, net).unwrap().address
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a thus-spoke-zakura devnet (see the file header)"]
async fn a_two_of_two_send_is_verified_signed_and_mined() {
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let server = start_server().await.expect("frostd sidecar binary (scripts/build-sidecar.sh)");
    let p = pair(&server).await;
    let (alice, bob) = (p.alice.app.state::<AppState>(), p.bob.app.state::<AppState>());
    let lightwalletd = lightwalletd_url();
    eprintln!("devnet lightwalletd: {lightwalletd}");
    for state in [&alice, &bob] {
        commands::wallet::set_wallet_config((*state).clone(), "regtest".into(), lightwalletd.clone(), Some(devnet_upgrade()))
            .await
            .unwrap();
    }
    let group_id = dkg(&p, Suite::RedPallas).await;

    // Fund the group; received funds need 10 confirmations to be spendable.
    let address = commands::wallet::wallet_receive_address(alice.clone(), group_id.clone()).await.unwrap().address;
    ths(&["faucet", &address, "--amount", "2"]);
    ths(&["mine", "12"]);

    // The background service sets up and syncs Alice's wallet by itself.
    sync_service::spawn(p.alice.app.handle().clone());
    commands::server::set_active_wallet(alice.clone(), group_id.clone()).await.unwrap();
    let started = Instant::now();
    let funded = loop {
        let status = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await;
        if let Ok(s) = &status {
            if s.spendable_zatoshis >= 100_000_000 {
                break s.spendable_zatoshis;
            }
        }
        assert!(started.elapsed() < Duration::from_secs(300), "wallet never became spendable: {status:?}");
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    eprintln!("spendable after sync: {funded} zatoshis");

    // Alice saves the recipient as a group payee, and Bob's app gets it.
    let recipient = outside_recipient();
    commands::payees::save_payee(p.alice.app.handle().clone(), group_id.clone(), None, "Auditor".into(), recipient.clone())
        .await
        .unwrap();
    until(&p, &group_id, "Bob never got the payee", async || {
        !commands::payees::list_payees(bob.clone(), group_id.clone()).await.unwrap().is_empty()
    })
    .await;

    // Alice coordinates a send; Bob's app verifies it before he approves.
    let txid = send_with_bobs_approval(
        &p,
        commands::wallet::WalletSendArgs {
            group_id: group_id.clone(),
            recipient: recipient.clone(),
            amount_zatoshis: 50_000_000,
            signers: vec![p.alice_pubkey.clone(), p.bob_pubkey.clone()],
            memo: Some("cyze end-to-end".into()),
            from_old_address: false,
            send_max: false,
            reason: Some("Audit, invoice 12".into()),
            payments: Vec::new(),
            poll_id: None,
        },
        Some(50_000_000),
    )
    .await;
    eprintln!("broadcast txid {txid}");

    // Every member's history says what it was for, Bob's too.
    until(&p, &group_id, "Bob never learned what the payment was for", async || {
        commands::payees::list_payment_reasons(bob.clone(), group_id.clone())
            .await
            .unwrap()
            .iter()
            .any(|r| r.txid == txid.to_lowercase() && r.text == "Audit, invoice 12" && r.author == p.alice_pubkey)
    })
    .await;

    // Mined, and in the group's history once synced.
    ths(&["mine", "2"]);
    let started = Instant::now();
    loop {
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let history = commands::wallet::wallet_history(alice.clone(), group_id.clone()).await.unwrap();
        if history.iter().any(|t| t.txid == txid) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(180), "{txid} never showed up in history");
    }
    let after = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await.unwrap();
    eprintln!("total after send: {} zatoshis", after.total_zatoshis);
    assert!(after.total_zatoshis < funded - 50_000_000 + 1, "the payment left the wallet");

    // "Send all": everything spendable, with a memo, the fee coming out of it.
    // The change from the first send needs its confirmations first.
    ths(&["mine", "10"]);
    let started = Instant::now();
    loop {
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let s = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await.unwrap();
        if s.spendable_zatoshis > 0 && s.spendable_zatoshis == s.total_zatoshis {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(180), "the change never became spendable: {s:?}");
    }

    // A batch with a full memo on every payment is too large for co-signers
    // to verify in one request, and review says so.
    let batch = |memos: usize| -> Vec<frost_app_core::wallet::PaymentRequest> {
        (0..frost_app_core::wallet::MAX_PAYMENTS)
            .map(|i| frost_app_core::wallet::PaymentRequest {
                recipient: outside_recipient_at(100 + i as u8),
                amount_zatoshis: 1_000_000 + i as u64 * 1_000,
                memo: (i < memos).then(|| format!("Grant {i}: {}", "milestone two of three, as agreed. ".repeat(14))),
            })
            .collect()
    };
    let too_big = commands::wallet::wallet_prepare_payments(alice.clone(), group_id.clone(), batch(20)).await;
    assert!(too_big.unwrap_err().message.contains("Make fewer payments at once"));
    // The most payments one transaction makes, a few with memos.
    let batch = batch(3);
    let draft = commands::wallet::wallet_prepare_payments(alice.clone(), group_id.clone(), batch.clone()).await.unwrap();
    assert_eq!(draft.payments.len(), batch.len());
    let txid = send_with_bobs_approval(
        &p,
        commands::wallet::WalletSendArgs {
            group_id: group_id.clone(),
            recipient: String::new(),
            amount_zatoshis: 0,
            signers: vec![p.alice_pubkey.clone(), p.bob_pubkey.clone()],
            memo: None,
            from_old_address: false,
            send_max: false,
            reason: Some("Q4 grants".into()),
            payments: batch.clone(),
            poll_id: None,
        },
        None,
    )
    .await;
    eprintln!("batch sent in {txid}");
    ths(&["mine", "10"]);
    let started = Instant::now();
    loop {
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let s = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await.unwrap();
        let history = commands::wallet::wallet_history(alice.clone(), group_id.clone()).await.unwrap();
        if let Some(t) = history.iter().find(|t| t.txid == txid) {
            if s.spendable_zatoshis > 0 && s.spendable_zatoshis == s.total_zatoshis {
                // The wallet records its own sends as a balance change, without
                // their fee or outputs (TODO.md), so the amount includes the fee.
                let total: u64 = batch.iter().map(|p| p.amount_zatoshis).sum();
                assert!(t.amount_zatoshis >= total, "{} < {total}", t.amount_zatoshis);
                break;
            }
        }
        assert!(started.elapsed() < Duration::from_secs(180), "the batch's change never became spendable: {s:?}");
    }

    // Pay if agreed: Alice proposes a payment in a poll.
    let polls_of = async |state: &tauri::State<'_, AppState>| {
        commands::polls::list_polls(state.clone(), Some(group_id.clone())).await.unwrap()
    };
    let proposal = commands::polls::create_poll(
        p.alice.app.handle().clone(),
        commands::polls::CreatePollArgs {
            group_id: group_id.clone(),
            title: "Pay the auditor for Q4?".into(),
            details: String::new(),
            options: Vec::new(),
            open_for_secs: 24 * 60 * 60,
            payments: vec![frost_app_core::polls::ProposedPayment {
                recipient: recipient.clone(),
                amount_zatoshis: 10_000_000,
                memo: Some(" Q4 audit ".into()),
            }],
            rules: None,
        },
    )
    .await
    .unwrap();
    until(&p, &group_id, "Bob never got the proposal", async || {
        polls_of(&bob).await.iter().any(|q| q.poll_id == proposal && q.options == ["Pay", "Don't pay"])
    })
    .await;
    let send_poll = |user: &UserApp| {
        commands::wallet::wallet_send(user.app.handle().clone(), poll_send(&p, &group_id, &proposal, &recipient))
    };
    assert!(refused(send_poll(&p.alice).await).contains("hasn't agreed"), "nothing goes before the group agrees");
    for user in [&p.alice, &p.bob] {
        commands::polls::vote_poll(user.app.handle().clone(), group_id.clone(), proposal.clone(), 0).await.unwrap();
    }
    until(&p, &group_id, "the group never agreed", async || {
        let agreed = |ps: Vec<frost_app_core::polls::PollView>| ps.iter().any(|q| q.poll_id == proposal && q.agreed == Some(0));
        agreed(polls_of(&alice).await) && agreed(polls_of(&bob).await)
    })
    .await;
    assert!(refused(send_poll(&p.bob).await).contains("only the member who proposed"));
    // Alice sends exactly it; Bob's app checks it against the poll.
    let txid = send_with_bobs_approval(&p, poll_send(&p, &group_id, &proposal, &recipient), Some(10_000_000)).await;
    eprintln!("the poll's payment went in {txid}");
    until(&p, &group_id, "Bob never saw the poll paid", async || {
        polls_of(&bob).await.iter().any(|q| q.poll_id == proposal && q.paid_txid.as_deref() == Some(txid.to_lowercase().as_str()))
    })
    .await;
    assert!(refused(send_poll(&p.alice).await).contains("already sent"), "a poll pays once");
    ths(&["mine", "10"]);
    let started = Instant::now();
    loop {
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let s = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await.unwrap();
        if s.spendable_zatoshis > 0 && s.spendable_zatoshis == s.total_zatoshis {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(180), "the poll payment's change never became spendable: {s:?}");
    }

    // Rules: pay only trusted payees. Bob trusts the auditor Alice saved, and
    // both adopt the rule.
    let auditor = commands::payees::list_payees(alice.clone(), group_id.clone()).await.unwrap().remove(0).payee;
    commands::payees::trust_payee(p.bob.app.handle().clone(), group_id.clone(), auditor.author, auditor.payee_id, true)
        .await
        .unwrap();
    let adopt = async |rules: frost_app_core::rules::GroupRules, title: &str| {
        let poll = commands::polls::create_poll(
            p.alice.app.handle().clone(),
            commands::polls::CreatePollArgs {
                group_id: group_id.clone(),
                title: title.into(),
                details: String::new(),
                options: Vec::new(),
                open_for_secs: 24 * 60 * 60,
                payments: Vec::new(),
                rules: Some(rules.clone()),
            },
        )
        .await
        .unwrap();
        until(&p, &group_id, "Bob never got the rules", async || polls_of(&bob).await.iter().any(|q| q.poll_id == poll)).await;
        for user in [&p.alice, &p.bob] {
            commands::polls::vote_poll(user.app.handle().clone(), group_id.clone(), poll.clone(), 0).await.unwrap();
        }
        let in_effect = async |state: &tauri::State<'_, AppState>| {
            commands::rules::group_rules(state.clone(), group_id.clone()).await.unwrap().rules
        };
        until(&p, &group_id, "the rules never took effect", async || {
            let trusted = commands::payees::list_payees(bob.clone(), group_id.clone()).await.unwrap()[0].payee.trusted;
            trusted && in_effect(&alice).await == rules && in_effect(&bob).await == rules
        })
        .await;
    };

    // Larger payments wait an hour after they're announced; any veto stops them.
    let delay = frost_app_core::rules::Delay { from_zatoshis: 50_000_000, secs: 3_600 };
    adopt(frost_app_core::rules::GroupRules { trusted_payees_only: true, delay: Some(delay), ..Default::default() }, "Wait for vetoes")
        .await;
    let large = commands::wallet::WalletSendArgs {
        amount_zatoshis: 60_000_000,
        memo: None,
        reason: None,
        poll_id: None,
        ..poll_send(&p, &group_id, "", &recipient)
    };
    let refusal = refused(commands::wallet::wallet_send(p.alice.app.handle().clone(), large).await);
    assert!(refusal.contains("announced in a payment poll 1 hour before"), "{refusal}");
    let announced = commands::polls::create_poll(
        p.alice.app.handle().clone(),
        commands::polls::CreatePollArgs {
            group_id: group_id.clone(),
            title: "Pay the auditor's retainer?".into(),
            details: String::new(),
            options: Vec::new(),
            open_for_secs: 24 * 60 * 60,
            payments: vec![frost_app_core::polls::ProposedPayment {
                recipient: recipient.clone(),
                amount_zatoshis: 60_000_000,
                memo: None,
            }],
            rules: None,
        },
    )
    .await
    .unwrap();
    let refusal = refused(send_poll_id(&p, &group_id, &announced, &recipient).await);
    assert!(refusal.contains("can be sent in 60 minutes"), "{refusal}");
    until(&p, &group_id, "Bob never got the announcement", async || polls_of(&bob).await.iter().any(|q| q.poll_id == announced)).await;
    commands::polls::vote_poll(p.bob.app.handle().clone(), group_id.clone(), announced.clone(), 1).await.unwrap();
    until(&p, &group_id, "Alice never saw the veto", async || {
        polls_of(&alice).await.iter().any(|q| q.poll_id == announced && q.tally.get(1) == Some(&1))
    })
    .await;
    let refusal = refused(send_poll_id(&p, &group_id, &announced, &recipient).await);
    assert!(refusal.contains("vetoed"), "{refusal}");

    // Rules replacing those: pay only trusted payees.
    adopt(frost_app_core::rules::GroupRules { trusted_payees_only: true, ..Default::default() }, "Pay only trusted payees").await;
    let untrusted = commands::wallet::WalletSendArgs {
        group_id: group_id.clone(),
        recipient: outside_recipient_at(7),
        amount_zatoshis: 1_000_000,
        signers: vec![p.alice_pubkey.clone(), p.bob_pubkey.clone()],
        memo: None,
        from_old_address: false,
        send_max: false,
        reason: None,
        payments: Vec::new(),
        poll_id: None,
    };
    let refusal = refused(commands::wallet::wallet_send(p.alice.app.handle().clone(), untrusted).await);
    assert!(refusal.contains("only to trusted payees"), "{refusal}");

    let (txid, bob_saw) = send_and_see_what_bob_saw(
        &p,
        commands::wallet::WalletSendArgs {
            group_id: group_id.clone(),
            recipient: recipient.clone(),
            amount_zatoshis: 0,
            signers: vec![p.alice_pubkey.clone(), p.bob_pubkey.clone()],
            memo: Some("everything".into()),
            from_old_address: false,
            send_max: true,
            reason: None,
            payments: Vec::new(),
            poll_id: None,
        },
        None,
    )
    .await;
    eprintln!("sent everything in {txid}");
    // Bob's app checked it against the rules: to a trusted payee, asking both.
    assert_eq!(bob_saw["rules"]["trusted_payees_only"], true, "{bob_saw}");
    assert_eq!((bob_saw["rules"]["needed"].as_u64(), bob_saw["rules"]["asked"].as_u64()), (Some(2), Some(2)));
    ths(&["mine", "2"]);
    let started = Instant::now();
    loop {
        commands::wallet::wallet_sync_now(alice.clone(), false).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let s = commands::wallet::wallet_group_status(alice.clone(), group_id.clone()).await.unwrap();
        let history = commands::wallet::wallet_history(alice.clone(), group_id.clone()).await.unwrap();
        if s.total_zatoshis == 0 && history.iter().any(|t| t.txid == txid) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(180), "the wallet never emptied: {s:?}");
    }
}

/// Alice sending the payment poll `poll_id` proposes.
async fn send_poll_id(
    p: &common::Pair,
    group_id: &str,
    poll_id: &str,
    recipient: &str,
) -> Result<commands::wallet::SendStarted, frost_app_lib::error::AppError> {
    commands::wallet::wallet_send(p.alice.app.handle().clone(), poll_send(p, group_id, poll_id, recipient)).await
}

/// Why a send was refused; panics if it wasn't.
fn refused(result: Result<commands::wallet::SendStarted, frost_app_lib::error::AppError>) -> String {
    match result {
        Ok(_) => panic!("it was sent"),
        Err(e) => e.message,
    }
}

/// What Alice sends to carry out the poll `poll_id`: the payment comes from
/// the poll; the recipient, amount and memo here are what the checks in
/// [`send_with_bobs_approval`] expect.
fn poll_send(p: &common::Pair, group_id: &str, poll_id: &str, recipient: &str) -> commands::wallet::WalletSendArgs {
    commands::wallet::WalletSendArgs {
        group_id: group_id.to_string(),
        recipient: recipient.to_string(),
        amount_zatoshis: 10_000_000,
        signers: vec![p.alice_pubkey.clone(), p.bob_pubkey.clone()],
        memo: Some("Q4 audit".into()),
        from_old_address: false,
        send_max: false,
        reason: Some("Pay the auditor for Q4?".into()),
        payments: Vec::new(),
        poll_id: Some(poll_id.to_string()),
    }
}

/// Alice coordinates a send; Bob's app joins each signing request, verifies
/// it, and Bob approves. Returns the broadcast txid. For a batch, checks Bob's
/// app verified every payment and nothing else. Otherwise, with `amount`,
/// checks the one payment Bob's app verified is that much to the recipient,
/// with the memo; without, that it's everything spent, less the fee.
async fn send_with_bobs_approval(p: &common::Pair, args: commands::wallet::WalletSendArgs, amount: Option<u64>) -> String {
    send_and_see_what_bob_saw(p, args, amount).await.0
}

/// [`send_with_bobs_approval`], with the last request Bob's app showed him.
async fn send_and_see_what_bob_saw(
    p: &common::Pair,
    args: commands::wallet::WalletSendArgs,
    amount: Option<u64>,
) -> (String, Value) {
    let bob = p.bob.app.state::<AppState>();
    let (group_id, recipient, memo) = (args.group_id.clone(), args.recipient.clone(), args.memo.clone());
    let reason = args.reason.clone();
    let batch = args.payments.clone();
    let poll_id = args.poll_id.clone();
    let payees = commands::payees::list_payees(bob.clone(), group_id.clone()).await.unwrap();
    let bob_events = record(&p.bob, &["signing:progress", "signing:failed"]);
    let alice_events = record(&p.alice, &["send:progress", "send:complete", "send:failed"]);
    commands::wallet::wallet_send(p.alice.app.handle().clone(), args).await.unwrap();

    let mut joined = HashSet::new();
    let mut approved = HashSet::new();
    let started = Instant::now();
    loop {
        if let Some((_, v)) = alice_events.lock().unwrap().iter().find(|(n, _)| n == "send:failed") {
            panic!("send failed: {v}");
        }
        if let Some((_, v)) = alice_events.lock().unwrap().iter().find(|(n, _)| n == "send:complete") {
            let seen = bob_events
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|(_, v)| v["event"]["phase"] == "awaiting_approval")
                .map(|(_, v)| v["event"].clone())
                .unwrap_or_default();
            return (v["txid"].as_str().unwrap().to_string(), seen);
        }
        if let Some((_, v)) = bob_events.lock().unwrap().iter().find(|(n, _)| n == "signing:failed") {
            panic!("Bob's signing failed: {v}");
        }
        for s in commands::signing::list_pending_sessions(bob.clone(), None).await.unwrap_or_default() {
            // Group records (the payment's reason) aren't signing requests.
            if s.kind != "group_message" && joined.insert(s.session_id) {
                commands::signing::join_signing_session(
                    p.bob.app.handle().clone(),
                    commands::signing::JoinSigningSessionArgs {
                        group_id: group_id.clone(),
                        session_id: s.session_id,
                        server_url: None,
                    },
                )
                .await
                .unwrap();
            }
        }
        let ready: Vec<(String, Value)> = bob_events
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, v)| v["event"]["phase"] == "awaiting_approval")
            .map(|(_, v)| (v["ceremony_id"].as_str().unwrap().to_string(), v["event"].clone()))
            .collect();
        for (ceremony, event) in ready {
            if !approved.insert(ceremony.clone()) {
                continue;
            }
            assert!(event.get("refused").is_none(), "Bob's app refused: {event}");
            let verified = &event["verified"];
            let payments = verified["payments"].as_array().expect("a verified transaction");
            assert_eq!(event["context"]["reason"].as_str(), reason.as_deref(), "{event}");
            // A poll's payment: Bob's app found it matches his copy of the
            // poll, which the group agreed to.
            if let Some(poll_id) = &poll_id {
                assert_eq!(event["poll"]["poll_id"], poll_id.as_str(), "{event}");
                assert_eq!(event["poll"]["missing"], false, "{event}");
                assert_eq!(event["poll"]["agreed"], true, "{event}");
            }
            if !batch.is_empty() {
                assert_eq!(payments.len(), batch.len(), "{verified}");
                assert_eq!(event["context"]["payments"].as_array().map(Vec::len), Some(batch.len()));
                for want in &batch {
                    // Memos read back trimmed.
                    let found = payments.iter().any(|p| {
                        p["address"] == want.recipient.as_str()
                            && p["value_zatoshis"] == want.amount_zatoshis
                            && p["memo"].as_str() == want.memo.as_deref().map(str::trim)
                    });
                    assert!(found, "Bob's app didn't verify {want:?} among {payments:?}");
                }
            } else {
                assert_eq!(payments.len(), 1, "{verified}");
                assert_eq!(payments[0]["address"], recipient.as_str());
                assert_eq!(payments[0]["memo"].as_str(), memo.as_deref());
                // Bob's own copy of the group's payees names the address he
                // verified.
                let paid = payments[0]["address"].as_str().unwrap().to_string();
                assert!(payees.iter().any(|p| p.forms.contains(&paid)), "the recipient is a payee: {paid}");
                match amount {
                    Some(amount) => assert_eq!(payments[0]["value_zatoshis"], amount),
                    None => {
                        assert_eq!(verified["change_zatoshis"], 0, "nothing comes back: {verified}");
                        let spent = verified["spent_zatoshis"].as_u64().unwrap();
                        let fee = verified["fee_zatoshis"].as_u64().unwrap();
                        assert_eq!(payments[0]["value_zatoshis"].as_u64().unwrap(), spent - fee);
                    }
                }
            }
            // Bob sees how long is left, from his own view of the chain: the
            // standard window, open, and counted in blocks on a devnet.
            let window = &event["window"];
            assert_eq!(window["expiry_height"], verified["expiry_height"], "{event}");
            assert!(window["blocks_left"].as_u64().unwrap() > 0, "{window}");
            assert!(window["closes_at_ms"].is_null(), "{window}");
            if event["auto_approved"] != true {
                commands::signing::respond_to_signing(bob.clone(), ceremony.parse().unwrap(), true)
                    .await
                    .unwrap();
            }
        }
        assert!(started.elapsed() < Duration::from_secs(900), "the send didn't complete");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
