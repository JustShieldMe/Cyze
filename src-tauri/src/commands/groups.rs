use frost_app_core::config::GroupSummary;
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[tauri::command]
pub async fn list_groups(state: State<'_, AppState>) -> AppResult<Vec<GroupSummary>> {
    state
        .with_config(|config| {
            config
                .group
                .iter()
                .map(|(id, g)| frost_app_core::config::summarize_group(id, g).map_err(Into::into))
                .collect()
        })
        .await
}

/// Derive the Orchard unified address and UFVK for a RedPallas group. Returns
/// `None` for Ed25519 groups (no Zcash key material). The group's id is the hex
/// of its FROST verifying key, which for RedPallas is the Orchard `ak`.
#[tauri::command]
pub async fn group_orchard_keys(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<Option<frost_app_core::zcash::OrchardKeys>> {
    let ciphersuite = state
        .with_config(|config| {
            config
                .group
                .get(&id)
                .map(|g| g.ciphersuite.clone())
                .ok_or_else(|| AppError::new("config", "group not found"))
        })
        .await?;
    // Only RedPallas (Zcash Orchard) groups have an Orchard spend key.
    if !ciphersuite.contains("Pallas") {
        return Ok(None);
    }
    // Encode for the configured network, resolved the same way as every wallet
    // command (so the group page and the wallet never disagree on the prefix).
    let network = crate::commands::wallet::configured_network(&state.load_settings());
    let seed = state.viewing_seed(&id).await?;
    Ok(Some(frost_app_core::zcash::derive_orchard_keys_hex(
        &id,
        seed.as_deref(),
        network.network_type(),
    )?))
}

#[tauri::command]
pub async fn remove_group(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let _ = crate::commands::polls::forget(&state, &id).await;
    state
        .mutate_keystore(|config, extras| {
            extras.viewing_seeds.remove(&id);
            extras.old_viewing_keys.remove(&id);
            config
                .group
                .remove(&id)
                .map(|_| ())
                .ok_or_else(|| AppError::new("config", "group not found"))
        })
        .await
}

#[tauri::command]
pub async fn rename_group(
    state: State<'_, AppState>,
    id: String,
    description: String,
) -> AppResult<()> {
    state
        .mutate_config(|config| {
            config
                .group
                .get_mut(&id)
                .map(|g| g.description = description.clone())
                .ok_or_else(|| AppError::new("config", "group not found"))
        })
        .await
}

/// One of this member's groups, named.
#[derive(serde::Serialize)]
pub struct OwnGroup {
    pub group_id: String,
    pub description: String,
}

/// Which of this member's Zcash groups receives at `address`, if any, worked
/// out from each group's own viewing key: approving a payment, a member sees
/// that it goes to a group they're in.
#[tauri::command]
pub async fn own_group_for_address(state: State<'_, AppState>, address: String) -> AppResult<Option<OwnGroup>> {
    let groups: Vec<(String, String)> = state
        .with_config(|config| {
            Ok(config
                .group
                .iter()
                .filter(|(_, g)| g.ciphersuite.contains("Pallas"))
                .map(|(id, g)| (id.clone(), g.description.clone()))
                .collect())
        })
        .await?;
    for (group_id, description) in groups {
        let seed = state.viewing_seed(&group_id).await?;
        if frost_app_core::wallet::verify::is_address_of(&group_id, seed.as_deref(), &address) {
            return Ok(Some(OwnGroup { group_id, description }));
        }
    }
    Ok(None)
}
