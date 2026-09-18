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
    Ok(Some(frost_app_core::zcash::derive_orchard_keys_hex(
        &id,
        network.network_type(),
    )?))
}

#[tauri::command]
pub async fn remove_group(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state
        .mutate_config(|config| {
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
