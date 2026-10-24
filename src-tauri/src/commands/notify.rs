//! System notifications, for when something needs the user and the app isn't
//! in front: an approval request, or a payment that finished. The interface
//! decides when; the text stays generic, since it can show on a lock screen.

use tauri::{AppHandle, Runtime};
use tauri_plugin_notification::NotificationExt;

use crate::error::{AppError, AppResult};

#[tauri::command]
pub async fn notify<R: Runtime>(app: AppHandle<R>, title: String, body: String) -> AppResult<()> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| AppError::new("notification", e.to_string()))
}
