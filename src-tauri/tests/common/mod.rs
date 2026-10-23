//! Helpers shared by the integration tests: a real frostd on a free port, and
//! mock-runtime app instances that stand in for separate users' installs.

#![allow(dead_code)] // each test binary uses a different subset

use std::process::{Child, Command};
use std::time::Duration;

use frost_app_lib::commands;
use frost_app_lib::state::AppState;

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
