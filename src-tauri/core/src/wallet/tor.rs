//! Optional Tor for wallet traffic.
//!
//! While Tor is on, every connection to a sync server goes through it, so the
//! server, and anyone watching the network, can't tie the wallet's requests or
//! the transactions it sends to this computer's IP address. Nothing falls back
//! to a direct connection: while Tor is starting, or if it failed, connecting
//! fails instead. A local devnet on this computer is reached directly, since
//! that traffic never leaves it.

use std::path::Path;
use std::sync::RwLock;

use serde::Serialize;
use zcash_client_backend::tor::Client;

use crate::error::CoreError;

enum Mode {
    Off,
    Starting,
    Ready(Box<Client>),
    Failed(String),
}

static MODE: RwLock<Mode> = RwLock::new(Mode::Off);

/// Where Tor stands, for the interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TorStatus {
    /// `"off"`, `"starting"`, `"ready"` or `"failed"`.
    pub state: &'static str,
    pub error: Option<String>,
}

pub fn status() -> TorStatus {
    match &*MODE.read().unwrap_or_else(|e| e.into_inner()) {
        Mode::Off => TorStatus { state: "off", error: None },
        Mode::Starting => TorStatus { state: "starting", error: None },
        Mode::Ready(_) => TorStatus { state: "ready", error: None },
        Mode::Failed(e) => TorStatus { state: "failed", error: Some(e.clone()) },
    }
}

/// Whether wallet traffic is meant to go through Tor (starting, ready or failed).
pub fn is_on() -> bool {
    !matches!(*MODE.read().unwrap_or_else(|e| e.into_inner()), Mode::Off)
}

fn set(mode: Mode) {
    *MODE.write().unwrap_or_else(|e| e.into_inner()) = mode;
}

/// Connect directly again.
pub fn turn_off() {
    set(Mode::Off);
}

/// Make sync-server connections wait for Tor from now on, ahead of [`start`],
/// so a caller can drop its direct connections without a gap.
pub fn mark_starting() {
    set(Mode::Starting);
}

/// Start Tor, keeping its state under `dir` (which makes later starts quicker).
/// From the moment this is called, sync-server connections wait for Tor rather
/// than going direct. Returns once Tor is ready or has failed.
pub async fn start(dir: &Path) -> Result<(), CoreError> {
    set(Mode::Starting);
    let result = async {
        std::fs::create_dir_all(dir)?;
        let _ = crate::keystore::restrict_dir_to_owner(dir);
        Client::create(dir, |_| {}).await.map_err(|e| CoreError::Connection(format!("starting Tor: {e}")))
    }
    .await;
    let mut mode = MODE.write().unwrap_or_else(|e| e.into_inner());
    // Turned off while it was starting: leave it off.
    if !matches!(*mode, Mode::Starting) {
        return Ok(());
    }
    match result {
        Ok(client) => {
            *mode = Mode::Ready(Box::new(client));
            Ok(())
        }
        Err(e) => {
            *mode = Mode::Failed(e.to_string());
            Err(e)
        }
    }
}

/// The Tor client to reach `url` through: `None` to connect directly, either
/// because Tor is off or because `url` is on this computer. An error while Tor
/// is on but can't carry traffic, so nothing goes out directly by mistake.
pub(super) fn route(normalized_url: &str) -> Result<Option<Client>, CoreError> {
    if super::is_loopback_host(normalized_url) {
        return Ok(None);
    }
    match &*MODE.read().unwrap_or_else(|e| e.into_inner()) {
        Mode::Off => Ok(None),
        Mode::Ready(client) => Ok(Some(Client::clone(client))),
        Mode::Starting => Err(CoreError::Connection("still connecting to Tor".into())),
        Mode::Failed(e) => Err(CoreError::Connection(format!(
            "Tor isn't working ({e}). Turn it off in Settings → Wallet to connect directly"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_goes_direct_while_tor_is_on_but_not_ready() {
        set(Mode::Starting);
        assert!(route("https://zec.rocks:443").is_err());
        // A devnet on this computer is still reached directly.
        assert!(matches!(route("http://127.0.0.1:9067"), Ok(None)));
        set(Mode::Failed("no network".into()));
        let Err(e) = route("https://zec.rocks:443") else { panic!("connected directly") };
        assert!(e.to_string().contains("Tor isn't working"), "{e}");
        assert_eq!(status().state, "failed");
        turn_off();
        assert!(matches!(route("https://zec.rocks:443"), Ok(None)));
        assert!(!is_on());
    }
}
