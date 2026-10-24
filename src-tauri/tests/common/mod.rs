//! Helpers shared by the integration tests: a real frostd on a free port, and
//! mock-runtime app instances that stand in for separate users' installs.

#![allow(dead_code)] // each test binary uses a different subset

use std::process::{Child, Command};
use std::time::Duration;

use frost_app_lib::commands;
use frost_app_lib::state::AppState;
use tauri::Manager;

pub struct TestServer {
    child: Child,
    pub port: u16,
    pub cert_pem: String,
    _dir: tempfile::TempDir,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn frostd_path() -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join("frostd-x86_64-unknown-linux-gnu");
    // CI places an empty stub here so Tauri's build script is satisfied; that
    // is not a server we can run.
    let is_real_binary = std::fs::metadata(&path).is_ok_and(|m| m.len() > 0);
    is_real_binary.then_some(path)
}

pub async fn start_server() -> Option<TestServer> {
    let frostd = frostd_path()?;
    let dir = tempfile::tempdir().unwrap();
    let cert = frost_app_core::tls::generate_self_signed(&[]).unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    frost_app_core::tls::write_cert_files(&cert, &cert_path, &key_path).unwrap();

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let child = Command::new(frostd)
        .args([
            "--ip",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--tls-cert",
            &cert_path.to_string_lossy(),
            "--tls-key",
            &key_path.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn frostd");

    let server = TestServer {
        child,
        port,
        cert_pem: cert.cert_pem,
        _dir: dir,
    };

    let client = frost_app_core::transport::FrostdClient::new(
        format!("https://127.0.0.1:{}", server.port),
        &frost_app_core::transport::ServerTrust::PinnedCertificate(
            server.cert_pem.clone().into_bytes(),
        ),
    )
    .unwrap();
    for _ in 0..40 {
        if client.challenge().await.is_ok() {
            return Some(server);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("frostd did not become healthy");
}

/// A mock-runtime app instance with its own data dir, as if it were a
/// separate user's installation.
pub struct UserApp {
    pub app: tauri::App<tauri::test::MockRuntime>,
    _dir: tempfile::TempDir,
}

pub fn make_user() -> UserApp {
    let dir = tempfile::tempdir().unwrap();
    let app = tauri::test::mock_builder()
        .manage(AppState::with_dir(dir.path().to_path_buf()))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    UserApp { app, _dir: dir }
}

pub async fn wait_for_group(state: &tauri::State<'_, AppState>) -> String {
    for _ in 0..60 {
        let groups = commands::groups::list_groups(state.clone()).await.unwrap();
        if let Some(g) = groups.first() {
            assert_eq!(g.threshold, 2);
            assert_eq!(g.num_participants, 2);
            return g.id.clone();
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("DKG did not complete within 60s");
}

/// Two members, Alice and Bob: keystores, each other's contacts, and the test
/// server configured and trusted on both.
pub struct Pair {
    pub alice: UserApp,
    pub bob: UserApp,
    pub alice_pubkey: String,
    pub bob_pubkey: String,
    pub server_url: String,
}

pub async fn pair(server: &TestServer) -> Pair {
    let server_url = format!("127.0.0.1:{}", server.port);
    let (alice, bob) = (make_user(), make_user());
    let (a, b) = (alice.app.state::<AppState>(), bob.app.state::<AppState>());
    commands::keystore::create_keystore(a.clone(), "alice-passphrase".into()).await.unwrap();
    commands::keystore::create_keystore(b.clone(), "bob-passphrase".into()).await.unwrap();
    let alice_contact = commands::contacts::export_my_contact(a.clone(), "alice".into()).await.unwrap();
    let bob_contact = commands::contacts::export_my_contact(b.clone(), "bob".into()).await.unwrap();
    commands::contacts::add_contact(a.clone(), bob_contact.text.clone(), None).await.unwrap();
    commands::contacts::add_contact(b.clone(), alice_contact.text.clone(), None).await.unwrap();
    for state in [&a, &b] {
        commands::server::set_server_url((*state).clone(), server_url.clone()).await.unwrap();
        commands::server::trust_server_cert((*state).clone(), server_url.clone(), server.cert_pem.clone())
            .await
            .unwrap();
    }
    Pair {
        alice_pubkey: alice_contact.pubkey,
        bob_pubkey: bob_contact.pubkey,
        alice,
        bob,
        server_url,
    }
}

/// Run a 2-of-2 DKG (Alice initiates, Bob joins) and return the group id.
pub async fn dkg(pair: &Pair, suite: frost_app_core::ciphersuite::Suite) -> String {
    dkg_with(pair, suite, true).await
}

/// [`dkg`], choosing whether the members agree on a private viewing key. A
/// group without one is what versions before 0.3 created.
pub async fn dkg_with(pair: &Pair, suite: frost_app_core::ciphersuite::Suite, agree_viewing_key: bool) -> String {
    let args = |participants: Vec<String>| commands::dkg::StartDkgArgs {
        suite,
        description: "test group".into(),
        threshold: 2,
        participants,
        server_url: Some(pair.server_url.clone()),
        session_id: None,
        agree_viewing_key,
    };
    commands::dkg::start_dkg(pair.alice.app.handle().clone(), args(vec![pair.bob_pubkey.clone()]))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    commands::dkg::start_dkg(pair.bob.app.handle().clone(), args(vec![])).await.unwrap();
    let group_id = wait_for_group(&pair.alice.app.state::<AppState>()).await;
    wait_for_group(&pair.bob.app.state::<AppState>()).await;
    group_id
}

/// Have Bob's app join the viewing-key session Alice started, the way the UI's
/// auto-join does, once it shows up in his poll.
pub async fn bob_joins_viewing_key_session(pair: &Pair, group_id: &str) {
    let bob = pair.bob.app.state::<AppState>();
    for _ in 0..60 {
        let sessions = commands::signing::list_pending_sessions(bob.clone(), None).await.unwrap();
        if let Some(s) = sessions.iter().find(|s| s.kind == "viewing_key") {
            assert_eq!(s.matching_groups, vec![group_id.to_string()]);
            commands::viewing::join_viewing_key_session(
                pair.bob.app.handle().clone(),
                group_id.to_string(),
                s.session_id,
                Some(s.coordinator_pubkey.clone()),
            )
            .await
            .unwrap();
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("Alice's viewing-key session never reached Bob");
}

/// Wait until a member's app has saved the group's new, private viewing key.
pub async fn wait_until_private(state: &tauri::State<'_, AppState>, group_id: &str) {
    for _ in 0..120 {
        let status = commands::viewing::viewing_key_status(state.clone(), group_id.to_string()).await.unwrap();
        if let Some(p) = status.progress.as_ref().filter(|p| p.state == "failed") {
            panic!("agreeing on a viewing key failed: {:?}", p.error);
        }
        if status.private && status.old_address.is_some() && status.progress.is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("the group's viewing key never became private");
}
