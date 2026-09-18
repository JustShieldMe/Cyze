//! Find a local thus-spoke-zakura regtest devnet.
//!
//! [thus-spoke-zakura](https://github.com/zcashlabs/thus-spoke-zakura) runs a
//! Zakura regtest node and lightwalletd in Docker, publishing each on a random
//! loopback port. Its CLI reports them with `thus-spoke-zakura endpoints --json`:
//!
//! ```json
//! { "dashboard": "http://127.0.0.1:…", "rpc": "…", "lightwalletd": "http://127.0.0.1:…", "p2p": "…" }
//! ```
//!
//! Like Tailscale, this **drives a system CLI** that the user installs; nothing
//! is bundled. The CLI reads those endpoints from a saved `instance.json` that
//! outlives the devnet, so a result here says where the devnet *was*. The
//! command layer probes the lightwalletd URL before calling it running.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

use crate::error::{AppError, AppResult};

/// The launcher's binary name.
const BIN: &str = "thus-spoke-zakura";

/// How long any one CLI call may take. `endpoints` only reads a file, so this
/// is generous; it just keeps a wedged CLI from hanging the settings screen.
const CLI_TIMEOUT: Duration = Duration::from_secs(10);

/// Endpoints of a devnet instance, as printed by `endpoints --json`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Endpoints {
    pub dashboard: String,
    pub rpc: String,
    pub lightwalletd: String,
    pub p2p: String,
}

/// What looking for the devnet found, before any network probe.
#[derive(Debug)]
pub enum Lookup {
    /// The CLI isn't on this machine (or this OS isn't supported by it).
    NotInstalled,
    /// The CLI ran but has no endpoints for this instance — it was never
    /// started, or it was stopped and deleted. Carries the CLI's own message.
    NoInstance(String),
    Found(Endpoints),
}

/// Candidate binary locations: PATH first, then where `install.sh` puts it.
/// A GUI-launched app often doesn't inherit a shell PATH that includes
/// `~/.local/bin`, so the install locations are tried explicitly.
fn candidates() -> Vec<PathBuf> {
    let mut c = vec![PathBuf::from(BIN)];
    if let Some(dir) = std::env::var_os("TSZ_INSTALL_DIR") {
        c.push(PathBuf::from(dir).join(BIN));
    }
    if let Some(home) = dirs::home_dir() {
        c.push(home.join(".local/bin").join(BIN));
    }
    c.push(PathBuf::from("/usr/local/bin").join(BIN));
    c.push(PathBuf::from("/opt/homebrew/bin").join(BIN));
    c
}

/// The first candidate that answers `--version`, or `None` if not installed.
async fn resolve_bin() -> Option<PathBuf> {
    for bin in candidates() {
        let mut cmd = Command::new(&bin);
        cmd.arg("--version").stdout(Stdio::null()).stderr(Stdio::null());
        let ok = tokio::time::timeout(CLI_TIMEOUT, cmd.status())
            .await
            .ok()
            .and_then(Result::ok)
            .is_some_and(|s| s.success());
        if ok {
            return Some(bin);
        }
    }
    None
}

/// The launcher's instance-name rule: 1–40 lowercase letters, digits, or
/// internal hyphens. Checked here too so a name can never be read as a flag.
pub fn valid_instance_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Parse `endpoints --json` output.
pub fn parse_endpoints(stdout: &[u8]) -> AppResult<Endpoints> {
    serde_json::from_slice(stdout)
        .map_err(|e| AppError::new("devnet", format!("unexpected `{BIN} endpoints --json` output: {e}")))
}

/// Ask the CLI where the devnet named `instance` is.
pub async fn lookup(instance: &str) -> AppResult<Lookup> {
    if cfg!(target_os = "windows") {
        // install.sh supports Linux and macOS only.
        return Ok(Lookup::NotInstalled);
    }
    if !valid_instance_name(instance) {
        return Err(AppError::new(
            "devnet",
            "instance names use 1-40 lowercase letters, digits, or internal hyphens",
        ));
    }
    let Some(bin) = resolve_bin().await else {
        return Ok(Lookup::NotInstalled);
    };
    let mut cmd = Command::new(&bin);
    cmd.args(["--name", instance, "--json", "endpoints"])
        .stdin(Stdio::null());
    let out = tokio::time::timeout(CLI_TIMEOUT, cmd.output())
        .await
        .map_err(|_| AppError::new("devnet", format!("`{BIN} endpoints` timed out")))?
        .map_err(|e| AppError::new("devnet", format!("running `{BIN} endpoints`: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let reason = stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("no endpoints recorded")
            .to_string();
        return Ok(Lookup::NoInstance(reason));
    }
    Ok(Lookup::Found(parse_endpoints(&out.stdout)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_launchers_endpoints_json() {
        let json = br#"{
          "dashboard": "http://127.0.0.1:49153",
          "rpc": "http://127.0.0.1:49154",
          "lightwalletd": "http://127.0.0.1:49155",
          "p2p": "127.0.0.1:49156"
        }"#;
        let e = parse_endpoints(json).unwrap();
        assert_eq!(e.lightwalletd, "http://127.0.0.1:49155");
        assert_eq!(e.dashboard, "http://127.0.0.1:49153");
        assert!(parse_endpoints(b"not json").is_err());
        assert!(parse_endpoints(br#"{"dashboard":"x"}"#).is_err());
    }

    #[test]
    fn instance_names_follow_the_launchers_rule() {
        assert!(valid_instance_name("default"));
        assert!(valid_instance_name("my-devnet-2"));
        assert!(!valid_instance_name(""));
        assert!(!valid_instance_name("--json"));
        assert!(!valid_instance_name("trailing-"));
        assert!(!valid_instance_name("Upper"));
        assert!(!valid_instance_name(&"a".repeat(41)));
    }
}
