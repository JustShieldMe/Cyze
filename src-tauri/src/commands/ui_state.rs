//! UI history (ceremonies, sends in flight, who signed what), kept encrypted
//! the same way as the private settings (`crate::private`). It names groups,
//! recipients, amounts, memos, and signed transactions, so it doesn't belong
//! in the web view's plaintext storage.

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

const KEY_INFO: &[u8] = b"ui-state-v1";
/// Generous for a few hundred ceremonies, and a bound on what a misbehaving
/// page can make the app write.
const MAX_LEN: usize = 8 << 20;

fn path(state: &AppState, key: &str) -> AppResult<std::path::PathBuf> {
    let valid = !key.is_empty() && key.len() <= 32 && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'-');
    if !valid {
        return Err(AppError::new("config", "invalid UI state name"));
    }
    Ok(state.data_dir.join(format!("ui-{key}.enc")))
}

fn label(key: &str) -> Vec<u8> {
    format!("cyze-ui-state-v1:{key}").into_bytes()
}

async fn subkey(state: &AppState) -> AppResult<zeroize::Zeroizing<[u8; 32]>> {
    let guard = state.unlocked.read().await;
    let unlocked = guard.as_ref().ok_or_else(AppError::locked)?;
    Ok(unlocked.file.derive_subkey(KEY_INFO))
}

/// The saved value under `key`, or `None` if nothing was saved. Needs the
/// keystore unlocked.
#[tauri::command]
pub async fn load_ui_state(state: State<'_, AppState>, key: String) -> AppResult<Option<String>> {
    let path = path(&state, &key)?;
    let subkey = subkey(&state).await?;
    let Ok(sealed) = std::fs::read(&path) else {
        return Ok(None);
    };
    let plain = frost_app_core::sealed::open(&subkey, &label(&key), &sealed)?;
    String::from_utf8(plain.to_vec()).map(Some).map_err(|e| AppError::new("config", e.to_string()))
}

/// Save `value` under `key`, or delete it when `value` is `None`.
#[tauri::command]
pub async fn save_ui_state(state: State<'_, AppState>, key: String, value: Option<String>) -> AppResult<()> {
    let path = path(&state, &key)?;
    let subkey = subkey(&state).await?;
    let Some(value) = value else {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    };
    if value.len() > MAX_LEN {
        return Err(AppError::new("config", "UI state is too large to save"));
    }
    let sealed = frost_app_core::sealed::seal(&subkey, &label(&key), value.as_bytes())?;
    frost_app_core::keystore::write_atomic(&path, &sealed)?;
    Ok(())
}
