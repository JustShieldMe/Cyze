use frost_client::cli::contact::Contact;
use serde::Serialize;
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Serialize)]
pub struct ContactDto {
    pub name: String,
    pub pubkey: String,
    /// Local nickname for this contact, if set.
    pub alias: Option<String>,
    /// The shareable `zffrost1...` string.
    pub text: String,
    /// The key's safety code, for comparing with the person out loud
    /// (`frost_app_core::safety`).
    pub safety_code: String,
    /// The user compared the safety code with this contact.
    pub verified: bool,
}

#[tauri::command]
pub async fn list_contacts(state: State<'_, AppState>) -> AppResult<Vec<ContactDto>> {
    let private = state.load_private().await?;
    let (aliases, verified) = (private.contact_aliases.clone(), private.verified_contacts.clone());
    state
        .with_config(|config| {
            config
                .contact
                .values()
                .map(|c| {
                    let pubkey = hex::encode(&c.pubkey.0);
                    Ok(ContactDto {
                        name: c.name.clone(),
                        alias: aliases.get(&pubkey).cloned(),
                        text: frost_app_core::config::contact_to_text(&c.name, &c.pubkey)?,
                        safety_code: frost_app_core::safety::safety_code(&c.pubkey.0),
                        verified: verified.contains(&pubkey),
                        pubkey,
                    })
                })
                .collect::<AppResult<Vec<_>>>()
        })
        .await
}

/// Set or clear the local alias for a contact (empty string clears it).
#[tauri::command]
pub async fn set_contact_alias(
    state: State<'_, AppState>,
    pubkey: String,
    alias: String,
) -> AppResult<()> {
    let trimmed = alias.trim().to_string();
    state
        .update_private(|p| {
            if trimmed.is_empty() {
                p.contact_aliases.remove(&pubkey);
            } else {
                p.contact_aliases.insert(pubkey, trimmed);
            }
        })
        .await
}

#[tauri::command]
pub async fn add_contact(
    state: State<'_, AppState>,
    text: String,
    alias: Option<String>,
) -> AppResult<ContactDto> {
    let mut contact = frost_app_core::config::contact_from_text(&text)?;
    state
        .mutate_config(|config| {
            if config.contact.contains_key(&contact.name) {
                return Err(AppError::new(
                    "config",
                    format!("a contact named \"{}\" already exists", contact.name),
                ));
            }
            if config.contact.values().any(|c| c.pubkey == contact.pubkey) {
                return Err(AppError::new("config", "this public key is already registered"));
            }
            if config.communication_key.as_ref().map(|c| &c.pubkey) == Some(&contact.pubkey) {
                return Err(AppError::new("config", "this is your own public key"));
            }
            // Upstream convention: version field is dropped when stored.
            contact.version = None;
            config.contact.insert(contact.name.clone(), contact.clone());
            Ok(())
        })
        .await?;

    let pubkey = hex::encode(&contact.pubkey.0);
    // Persist the local alias, if one was provided at import.
    let alias = alias.and_then(|a| {
        let a = a.trim().to_string();
        (!a.is_empty()).then_some(a)
    });
    if let Some(alias) = &alias {
        let (key, alias) = (pubkey.clone(), alias.clone());
        state.update_private(|p| p.contact_aliases.insert(key, alias)).await?;
    }
    Ok(ContactDto {
        name: contact.name.clone(),
        alias,
        safety_code: frost_app_core::safety::safety_code(&contact.pubkey.0),
        verified: false,
        pubkey,
        text,
    })
}

#[tauri::command]
pub async fn remove_contact(state: State<'_, AppState>, pubkey: String) -> AppResult<()> {
    state
        .mutate_config(|config| {
            let name = config
                .contact
                .iter()
                .find_map(|(name, c)| (hex::encode(&c.pubkey.0) == pubkey).then(|| name.clone()))
                .ok_or_else(|| AppError::new("config", "contact not found"))?;
            config.contact.remove(&name);
            Ok(())
        })
        .await?;
    // Drop any alias for the removed contact.
    state
        .update_private(|p| {
            p.contact_aliases.remove(&pubkey);
            p.verified_contacts.remove(&pubkey);
        })
        .await?;
    Ok(())
}

/// Export the user's own contact string for sharing with peers.
#[tauri::command]
pub async fn export_my_contact(state: State<'_, AppState>, name: String) -> AppResult<ContactDto> {
    state
        .with_config(|config| {
            let pubkey = &config
                .communication_key
                .as_ref()
                .ok_or_else(|| AppError::new("config", "keystore has no communication key"))?
                .pubkey;
            let contact = Contact {
                version: Some(0),
                name: name.clone(),
                pubkey: pubkey.clone(),
            };
            Ok(ContactDto {
                name,
                alias: None,
                safety_code: frost_app_core::safety::safety_code(&pubkey.0),
                verified: false,
                pubkey: hex::encode(&pubkey.0),
                text: contact
                    .as_text()
                    .map_err(|e| AppError::new("config", e.to_string()))?,
            })
        })
        .await
}

/// Record whether the user compared a contact's safety code with them.
#[tauri::command]
pub async fn set_contact_verified(state: State<'_, AppState>, pubkey: String, verified: bool) -> AppResult<()> {
    let known = state
        .with_config(|c| Ok(c.contact.values().any(|x| hex::encode(&x.pubkey.0) == pubkey)))
        .await?;
    if !known {
        return Err(AppError::new("config", "contact not found"));
    }
    state
        .update_private(|p| {
            if verified {
                p.verified_contacts.insert(pubkey);
            } else {
                p.verified_contacts.remove(&pubkey);
            }
        })
        .await
}
