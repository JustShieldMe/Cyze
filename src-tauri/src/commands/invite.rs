//! Invite codes (see `frost_app_core::invite`): the organizer creates one for
//! a key-generation session, and members join with it in one step.

use frost_app_core::ciphersuite::Suite;
use frost_app_core::invite::{Invite, Member};
use frost_client::api::PublicKey;
use frost_client::cli::contact::Contact;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::commands::dkg::{begin_dkg, StartDkgArgs};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

fn bare_host(url: &str) -> String {
    url.trim().trim_start_matches("https://").trim_end_matches('/').to_string()
}

/// Where a new group's key generation runs: the address this device uses, and
/// the one members use.
#[derive(Serialize)]
pub struct GroupServer {
    pub local: String,
    pub public: String,
    /// "tunnel" or "tailscale".
    pub via: &'static str,
}

/// Get the embedded server running and reachable by members, for a group this
/// user is organizing: Tailscale if they chose it in Connection settings,
/// otherwise a public link, which works for anyone.
#[tauri::command]
pub async fn prepare_group_server(app: AppHandle) -> AppResult<GroupServer> {
    let state = app.state::<AppState>();
    let port = match state.sidecar.lock().await.as_ref().map(|h| h.port) {
        Some(port) => port,
        None => crate::commands::server::start_sidecar(app.clone(), None, Some(false)).await?.port.unwrap_or(2744),
    };
    let local = format!("127.0.0.1:{port}");
    let tailscale = crate::tailscale::status(&state).await;
    if let (true, Some(url)) = (tailscale.serving, &tailscale.public_url) {
        return Ok(GroupServer { local, public: bare_host(url), via: "tailscale" });
    }
    let tunnel = crate::tunnel::status(&state).await;
    if let (true, Some(url)) = (tunnel.running, &tunnel.public_url) {
        return Ok(GroupServer { local, public: bare_host(url), via: "tunnel" });
    }
    if state.load_settings().coordinator_exposure.as_deref() == Some("tailscale") {
        let started = crate::tailscale::start(&state, port).await?;
        let url = started
            .public_url
            .ok_or_else(|| AppError::new("tailscale", "Tailscale didn't report an address"))?;
        return Ok(GroupServer { local, public: bare_host(&url), via: "tailscale" });
    }
    let started = crate::tunnel::start(&app, port).await?;
    let url = started
        .public_url
        .ok_or_else(|| AppError::new("tunnel", "the public link didn't report an address"))?;
    Ok(GroupServer { local, public: bare_host(&url), via: "tunnel" })
}

#[derive(Deserialize)]
pub struct CreateInviteArgs {
    pub session_id: Uuid,
    pub suite: Suite,
    pub threshold: u16,
    pub name: String,
    /// Hex comm pubkeys of the other members (all in this user's contacts).
    pub participants: Vec<String>,
    /// The server address members use (`GroupServer::public`).
    pub server: String,
}

/// The invite for a session this user started.
#[tauri::command]
pub async fn create_invite(state: State<'_, AppState>, args: CreateInviteArgs) -> AppResult<String> {
    let settings = state.load_settings();
    let own_name = settings.username.clone().unwrap_or_else(|| "organizer".into());
    let (organizer, mut members) = state
        .with_config(|config| {
            let me = config
                .communication_key
                .as_ref()
                .ok_or_else(|| AppError::new("config", "keystore has no communication key"))?;
            let members = args
                .participants
                .iter()
                .map(|hex_key| {
                    let contact = config
                        .contact
                        .values()
                        .find(|c| hex::encode(&c.pubkey.0).eq_ignore_ascii_case(hex_key))
                        .ok_or_else(|| AppError::new("config", "a member isn't in your contacts"))?;
                    Ok(Member { name: contact.name.clone(), pubkey: hex::encode(&contact.pubkey.0) })
                })
                .collect::<AppResult<Vec<_>>>()?;
            Ok((hex::encode(&me.pubkey.0), members))
        })
        .await?;
    if !members.iter().any(|m| m.pubkey == organizer) {
        members.insert(0, Member { name: own_name, pubkey: organizer.clone() });
    }
    let server = bare_host(&args.server);
    let invite = Invite {
        cert_pem: settings.trusted_certs.get(&server).cloned(),
        server,
        session_id: args.session_id,
        suite: args.suite,
        threshold: args.threshold,
        name: args.name.trim().to_string(),
        organizer,
        members,
    };
    Ok(invite.encode()?)
}

#[derive(Serialize)]
pub struct InviteMember {
    pub name: String,
    pub pubkey: String,
    /// "you", "contact" (already in your contacts), or "new".
    pub status: &'static str,
    /// Your name for them, when they're already a contact.
    pub contact_name: Option<String>,
}

#[derive(Serialize)]
pub struct InviteSummary {
    pub name: String,
    pub threshold: u16,
    pub suite: Suite,
    pub server: String,
    pub organizer: InviteMember,
    pub members: Vec<InviteMember>,
    /// Whether this user is one of the members.
    pub for_you: bool,
    /// SHA-256 of the server certificate the invite carries, if any.
    pub cert_fingerprint: Option<String>,
}

/// What an invite asks this user to join, for them to check before joining.
#[tauri::command]
pub async fn read_invite(state: State<'_, AppState>, code: String) -> AppResult<InviteSummary> {
    let invite = Invite::decode(&code)?;
    let (me, contacts) = state
        .with_config(|config| {
            let me = config.communication_key.as_ref().map(|k| hex::encode(&k.pubkey.0)).unwrap_or_default();
            let contacts: Vec<(String, String)> =
                config.contact.values().map(|c| (hex::encode(&c.pubkey.0), c.name.clone())).collect();
            Ok((me, contacts))
        })
        .await?;
    let aliases = state.load_private().await?.contact_aliases;
    let view = |m: &Member| {
        let known = contacts.iter().find(|(k, _)| k.eq_ignore_ascii_case(&m.pubkey));
        let status = if m.pubkey.eq_ignore_ascii_case(&me) {
            "you"
        } else if known.is_some() {
            "contact"
        } else {
            "new"
        };
        InviteMember {
            name: m.name.clone(),
            pubkey: m.pubkey.to_ascii_lowercase(),
            status,
            contact_name: known.map(|(k, name)| aliases.get(k).cloned().unwrap_or_else(|| name.clone())),
        }
    };
    let organizer = invite
        .members
        .iter()
        .find(|m| m.pubkey.eq_ignore_ascii_case(&invite.organizer))
        .map(view)
        .ok_or_else(|| AppError::new("config", "the invite's organizer isn't a member"))?;
    Ok(InviteSummary {
        name: invite.name.clone(),
        threshold: invite.threshold,
        suite: invite.suite,
        server: invite.server.clone(),
        organizer,
        members: invite.members.iter().map(view).collect(),
        for_you: invite.includes(&me),
        cert_fingerprint: invite.cert_pem.as_deref().map(frost_app_core::tls::cert_fingerprint).transpose()?,
    })
}

/// Join the session an invite describes: add the members who aren't contacts
/// yet (the user confirmed them on the summary), trust the server's
/// certificate if the invite carries one, remember the server, and start key
/// generation. Returns the ceremony id.
#[tauri::command]
pub async fn accept_invite<R: tauri::Runtime>(app: AppHandle<R>, code: String, name: Option<String>) -> AppResult<Uuid> {
    let state = app.state::<AppState>();
    let invite = Invite::decode(&code)?;
    let me = state
        .with_config(|c| Ok(c.communication_key.as_ref().map(|k| hex::encode(&k.pubkey.0)).unwrap_or_default()))
        .await?;
    if !invite.includes(&me) {
        return Err(AppError::new(
            "config",
            "this invite doesn't include you. Send the organizer your contact code and ask for a new invite.",
        ));
    }
    let members = invite.members.clone();
    state
        .mutate_config(move |config| {
            for m in members.iter().filter(|m| !m.pubkey.eq_ignore_ascii_case(&me)) {
                let key = PublicKey(hex::decode(&m.pubkey).map_err(|e| AppError::new("config", e.to_string()))?);
                if config.contact.values().any(|c| c.pubkey == key) {
                    continue;
                }
                // Contacts are keyed by name; don't overwrite someone else's.
                let mut name = m.name.clone();
                let mut n = 2;
                while config.contact.contains_key(&name) {
                    name = format!("{} ({n})", m.name);
                    n += 1;
                }
                config.contact.insert(name.clone(), Contact { version: None, name, pubkey: key });
            }
            Ok(())
        })
        .await?;
    let mut settings = state.load_settings();
    if let Some(pem) = &invite.cert_pem {
        settings.trusted_certs.insert(invite.server.clone(), pem.clone());
    }
    settings.server_url = Some(invite.server.clone());
    settings.session_role = Some("participant".into());
    settings.session_configured = Some(true);
    state.save_settings(&settings)?;

    let description = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or(invite.name);
    begin_dkg(
        &app,
        StartDkgArgs {
            suite: invite.suite,
            description,
            threshold: invite.threshold,
            participants: vec![],
            server_url: Some(invite.server),
            session_id: Some(invite.session_id),
        },
    )
    .await
}
