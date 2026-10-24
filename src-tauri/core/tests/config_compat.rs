//! M0 compatibility tests: parse a real upstream credentials.toml and round
//! trip it through the keystore envelope.

use frost_app_core::{config, keystore};

/// The user's real upstream config, if present (developer machine only).
fn real_config() -> Option<String> {
    let path = dirs::config_local_dir()?.join("frost").join("credentials.toml");
    std::fs::read_to_string(path).ok()
}

#[test]
fn parses_real_upstream_config() {
    let Some(toml_str) = real_config() else {
        eprintln!("skipping: no ~/.config/frost/credentials.toml on this machine");
        return;
    };
    let cfg = config::parse_config(&toml_str).expect("real config must parse");
    assert!(cfg.communication_key.is_some(), "config has a comm key");
    // Group summaries must build for every stored group (exercises postcard
    // decoding of KeyPackage/PublicKeyPackage for the group's ciphersuite).
    for (id, group) in &cfg.group {
        let summary = config::summarize_group(id, group).expect("group must summarize");
        assert!(summary.threshold >= 1);
        assert!(summary.num_participants as usize >= summary.threshold as usize);
        eprintln!(
            "group '{}' ({}) {}-of-{}",
            summary.description, summary.ciphersuite, summary.threshold, summary.num_participants
        );
    }
    // Round-trip: serialize back to TOML and reparse.
    let reserialized = config::serialize_config(&cfg).unwrap();
    let cfg2 = config::parse_config(&reserialized).expect("reserialized config must parse");
    assert_eq!(cfg.contact.len(), cfg2.contact.len());
    assert_eq!(cfg.group.len(), cfg2.group.len());
}

#[test]
fn keystore_roundtrips_real_config() {
    let Some(toml_str) = real_config() else {
        eprintln!("skipping: no ~/.config/frost/credentials.toml on this machine");
        return;
    };
    let params = keystore::KdfParams {
        m_cost_kib: 8,
        t_cost: 1,
        p_cost: 1,
    };
    let sealed = keystore::seal(toml_str.as_bytes(), "test-passphrase", &params).unwrap();
    let opened = keystore::open(&sealed, "test-passphrase").unwrap();
    let cfg = config::parse_config(std::str::from_utf8(&opened).unwrap()).unwrap();
    assert!(cfg.communication_key.is_some());
}

#[test]
fn contact_text_roundtrip() {
    // Generate a fresh comm keypair the same way `frost-client init` does.
    let (_priv, public) = frost_client::cipher::Cipher::generate_keypair().unwrap();
    let text = config::contact_to_text("alice", &public).unwrap();
    assert!(text.starts_with("zffrost1"));
    let contact = config::contact_from_text(&text).unwrap();
    assert_eq!(contact.name, "alice");
    assert_eq!(contact.pubkey, public);
}

#[test]
fn keystore_extras_round_trip_and_upstream_ignores_them() {
    let (privkey, pubkey) = frost_client::cipher::Cipher::generate_keypair().unwrap();
    let mut cfg = frost_client::cli::config::Config::default();
    cfg.communication_key = Some(frost_client::cli::config::CommunicationKey { privkey, pubkey });
    let mut extras = config::KeystoreExtras::default();

    // No extras: exactly the upstream TOML.
    assert_eq!(*config::serialize_keystore(&cfg, &extras).unwrap(), *config::serialize_config(&cfg).unwrap());

    extras.viewing_seeds.insert("ab".repeat(32), "07".repeat(32));
    let text = config::serialize_keystore(&cfg, &extras).unwrap();
    let (cfg2, extras2) = config::parse_keystore(&text).unwrap();
    assert_eq!(extras2, extras);
    assert_eq!(extras2.viewing_seed(&"ab".repeat(32)).unwrap(), Some([7u8; 32]));
    assert_eq!(extras2.viewing_seed("other").unwrap(), None);
    assert_eq!(cfg2.communication_key.unwrap().pubkey, cfg.communication_key.as_ref().unwrap().pubkey);
    // frost-client reads the same text and ignores the [cyze] table.
    assert!(config::parse_config(&text).is_ok());
}
