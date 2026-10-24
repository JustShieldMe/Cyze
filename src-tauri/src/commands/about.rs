//! What Settings → About shows.

use tauri::{AppHandle, Manager, Runtime};

use crate::error::{AppError, AppResult};

/// The third-party notices bundled with the app: the open-source software it's
/// built on and its licenses (written by scripts/third-party-notices.sh).
#[tauri::command]
pub async fn third_party_notices<R: Runtime>(app: AppHandle<R>) -> AppResult<String> {
    let path = app
        .path()
        .resource_dir()
        .map_err(|e| AppError::new("io", e.to_string()))?
        .join("THIRD-PARTY-NOTICES.txt");
    std::fs::read_to_string(&path).map_err(|e| AppError::new("io", format!("couldn't read the notices: {e}")))
}
