//! Saving what Cyze exports for the user, such as a wallet's history as CSV for
//! accounting. The interface builds the file; this asks where to save it.

use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;

use crate::error::{AppError, AppResult};

/// Ask where to save a CSV file (suggesting `file_name`), then write
/// `contents` there. Returns where it went, or None if the user cancelled.
#[tauri::command]
pub async fn save_csv<R: Runtime>(app: AppHandle<R>, file_name: String, contents: String) -> AppResult<Option<String>> {
    // A name, not a path: whatever the interface suggests stays a file name.
    let name: String = file_name.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '\0')).collect();
    let name = if name.to_lowercase().ends_with(".csv") { name } else { format!("{name}.csv") };
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().add_filter("CSV", &["csv"]).set_file_name(name).save_file(move |path| {
        let _ = tx.send(path);
    });
    let Some(chosen) = rx.await.map_err(|_| AppError::new("export", "the file dialog closed unexpectedly"))? else {
        return Ok(None);
    };
    let path = chosen.into_path().map_err(|e| AppError::new("export", e.to_string()))?;
    std::fs::write(&path, contents).map_err(|e| AppError::new("export", format!("couldn't save the file: {e}")))?;
    Ok(Some(path.display().to_string()))
}
