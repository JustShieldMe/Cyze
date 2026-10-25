//! Turning Tor on and off for wallet traffic (see `frost_app_core::wallet::tor`).

use frost_app_core::wallet::tor::{self, TorStatus};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::error::AppResult;
use crate::state::AppState;

#[tauri::command]
pub async fn tor_status() -> AppResult<TorStatus> {
    Ok(tor::status())
}

/// Turn Tor on or off for wallet traffic, and remember the choice.
#[tauri::command]
pub async fn set_use_tor<R: Runtime>(app: AppHandle<R>, enabled: bool) -> AppResult<()> {
    let state = app.state::<AppState>();
    let mut settings = state.load_settings();
    settings.use_tor = Some(enabled);
    state.save_settings(&settings)?;
    if enabled {
        // Connections wait for Tor from now on; it bootstraps in the
        // background, and `tor:status` says when it's ready.
        tor::mark_starting();
        let app = app.clone();
        tauri::async_runtime::spawn(async move { start(&app).await });
    } else {
        tor::turn_off();
        reconnect(&state).await;
        let _ = app.emit("tor:status", tor::status());
    }
    Ok(())
}

/// Start Tor if the settings say so: at launch.
pub fn start_if_enabled<R: Runtime>(app: &AppHandle<R>) {
    if app.state::<AppState>().load_settings().use_tor == Some(true) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { start(&app).await });
    }
}

/// Start Tor, stopping any direct connection first, then sync through it.
async fn start<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    tor::mark_starting();
    let _ = app.emit("tor:status", tor::status());
    reconnect(&state).await;
    match tor::start(&state.data_dir.join("tor")).await {
        Ok(()) => tracing::info!("Tor is ready; wallet traffic goes through it"),
        Err(e) => tracing::warn!("Tor didn't start: {e}"),
    }
    let _ = app.emit("tor:status", tor::status());
    state.sync_service.wake(true);
}

/// Drop the open sync-server connection and stop running syncs, so nothing
/// keeps using a connection made the other way.
async fn reconnect(state: &State<'_, AppState>) {
    state.lightwalletd.lock().await.take();
    for token in state.sync_cancels.lock().await.values() {
        token.cancel();
    }
    state.sync_service.wake(true);
}
