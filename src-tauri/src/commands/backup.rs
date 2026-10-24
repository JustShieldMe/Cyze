//! Key backups: export the encrypted keystore (with the wallet metadata a new
//! device needs) and restore it on another device. See `frost_app_core::backup`
//! for the file format.
//!
//! The save and open dialogs are opened here, in Rust, so these commands never
//! take a file path from the web layer: a compromised page can't point them at
//! other files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use frost_app_core::backup;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Restored metadata waiting for the first unlock, which provides its key.
const PENDING_META: &str = "restore-meta.pending";

/// What a backup carries besides the keystore: where each group's wallet
/// starts scanning, so a restored wallet finds the funds received before.
#[derive(Default, Serialize, Deserialize)]
struct BackupMeta {
    #[serde(default)]
    network_birthdays: HashMap<String, u64>,
    #[serde(default)]
    wallet_birthdays: HashMap<String, u64>,
    #[serde(default)]
    group_created_at: HashMap<String, u64>,
}

#[derive(Serialize)]
pub struct BackupStatus {
    /// When the keystore was last backed up (unix seconds).
    pub last_backup_at: Option<u64>,
    /// The last backup holds every group this keystore has.
    pub up_to_date: bool,
    /// Whether there is anything worth backing up yet (any group).
    pub has_groups: bool,
}

fn unix_now() -> u64 {
    crate::commands::wallet::unix_now()
}

/// The current groups' fingerprint (see `backup::groups_fingerprint`), and
/// whether there are any. A group that made its viewing key private counts
/// with its seed, so a backup from before then shows as out of date: restoring
/// it would bring back a device that can't see the group's funds.
async fn groups_fingerprint(state: &AppState) -> AppResult<(String, bool)> {
    let guard = state.unlocked.read().await;
    let unlocked = guard.as_ref().ok_or_else(AppError::locked)?;
    let ids: Vec<String> = unlocked
        .config
        .group
        .keys()
        .map(|id| match (unlocked.extras.has_old_viewing_key(id), unlocked.extras.viewing_seed(id)) {
            (true, Ok(Some(seed))) => {
                let seed = zeroize::Zeroizing::new(seed);
                let ak = hex::decode(id).unwrap_or_default();
                format!("{id}:{}", hex::encode(frost_app_core::viewing::digest(&ak, &seed)))
            }
            _ => id.clone(),
        })
        .collect();
    Ok((backup::groups_fingerprint(&ids), !ids.is_empty()))
}

#[tauri::command]
pub async fn backup_status(state: State<'_, AppState>) -> AppResult<BackupStatus> {
    let (fingerprint, has_groups) = groups_fingerprint(&state).await?;
    let last_backup_at = state.load_settings().last_backup_at;
    let backed_up = state.load_private().await?.backup_fingerprint;
    Ok(BackupStatus {
        last_backup_at,
        up_to_date: last_backup_at.is_some() && backed_up.as_deref() == Some(&fingerprint),
        has_groups,
    })
}

/// A civil date (YYYY-MM-DD) for a unix time, for the backup's file name.
fn date(unix: u64) -> String {
    // Howard Hinnant's days-from-civil, inverted.
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Write a backup of the unlocked keystore to `path` and record it.
pub async fn export_backup_to(state: &AppState, path: &Path) -> AppResult<BackupStatus> {
    let private = state.load_private().await?;
    let meta = serde_json::to_vec(&BackupMeta {
        network_birthdays: private.network_birthdays,
        wallet_birthdays: private.wallet_birthdays,
        group_created_at: private.group_created_at,
    })
    .map_err(|e| AppError::new("backup", e.to_string()))?;
    let encrypted = state.keystore().read_encrypted()?;
    let now = unix_now();
    let file = {
        let guard = state.unlocked.read().await;
        let unlocked = guard.as_ref().ok_or_else(AppError::locked)?;
        backup::create(&encrypted, &unlocked.file, &meta, now)?
    };
    frost_app_core::keystore::write_atomic(path, &file)?;

    let (fingerprint, has_groups) = groups_fingerprint(state).await?;
    state.update_private(|p| p.backup_fingerprint = Some(fingerprint)).await?;
    let mut settings = state.load_settings();
    settings.last_backup_at = Some(now);
    state.save_settings(&settings)?;
    Ok(BackupStatus { last_backup_at: Some(now), up_to_date: true, has_groups })
}

/// Install the keystore from the backup at `path` on a device that has none.
/// Its metadata is applied after the first unlock ([`apply_pending_restore`]).
pub async fn restore_backup_from(state: &AppState, path: &Path) -> AppResult<()> {
    let bytes = std::fs::read(path)
        .map_err(|e| AppError::new("backup", format!("can't read {}: {e}", path.display())))?;
    let parsed = backup::parse(&bytes)?;
    state.keystore().install(&parsed.keystore)?;
    frost_app_core::keystore::write_atomic(&state.data_dir.join(PENDING_META), &parsed.sealed_meta)?;
    Ok(())
}

/// After an unlock: if a restored backup's metadata is waiting, decrypt it with
/// the now-unlocked keystore and merge it into the private settings (never overwriting
/// what this device already recorded). Best effort: failing here only loses
/// the recorded wallet birthdays, so it is logged rather than surfaced.
pub(crate) async fn apply_pending_restore(state: &AppState) {
    let pending = state.data_dir.join(PENDING_META);
    let Ok(sealed) = std::fs::read(&pending) else {
        return;
    };
    let opened = {
        let guard = state.unlocked.read().await;
        match guard.as_ref() {
            Some(unlocked) => backup::open_meta(&unlocked.file, &sealed),
            None => return,
        }
    };
    match opened.map_err(AppError::from).and_then(|plain| {
        serde_json::from_slice::<BackupMeta>(&plain).map_err(|e| AppError::new("backup", e.to_string()))
    }) {
        Ok(meta) => {
            let merged = state
                .update_private(|p| {
                    for (k, v) in meta.network_birthdays {
                        p.network_birthdays.entry(k).or_insert(v);
                    }
                    for (k, v) in meta.wallet_birthdays {
                        p.wallet_birthdays.entry(k).or_insert(v);
                    }
                    for (k, v) in meta.group_created_at {
                        p.group_created_at.entry(k).or_insert(v);
                    }
                })
                .await;
            if let Err(e) = merged {
                tracing::warn!("restore: couldn't save the backup's wallet metadata: {}", e.message);
                return;
            }
            tracing::info!("restore: applied the backup's wallet metadata");
        }
        Err(e) => tracing::warn!("restore: couldn't read the backup's wallet metadata: {}", e.message),
    }
    let _ = std::fs::remove_file(pending);
}

async fn pick<R: tauri::Runtime>(app: &AppHandle<R>, save: bool) -> AppResult<Option<PathBuf>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let dialog = app
        .dialog()
        .file()
        .add_filter("Cyze backup", &[backup::EXTENSION]);
    if save {
        dialog
            .set_file_name(format!("Cyze backup {}.{}", date(unix_now()), backup::EXTENSION))
            .save_file(move |path| {
                let _ = tx.send(path);
            });
    } else {
        dialog.pick_file(move |path| {
            let _ = tx.send(path);
        });
    }
    let chosen = rx.await.map_err(|_| AppError::new("backup", "the file dialog closed unexpectedly"))?;
    chosen
        .map(|p| p.into_path().map_err(|e| AppError::new("backup", e.to_string())))
        .transpose()
}

/// Ask where to save a backup, then write it. `None` if the user cancelled.
#[tauri::command]
pub async fn export_backup<R: tauri::Runtime>(app: AppHandle<R>) -> AppResult<Option<BackupStatus>> {
    let state = app.state::<AppState>();
    if state.unlocked.read().await.is_none() {
        return Err(AppError::locked());
    }
    let Some(mut path) = pick(&app, true).await? else {
        return Ok(None);
    };
    if path.extension().and_then(|e| e.to_str()) != Some(backup::EXTENSION) {
        path.set_extension(backup::EXTENSION);
    }
    Ok(Some(export_backup_to(&state, &path).await?))
}

/// Ask for a backup file and install it. `false` if the user cancelled.
#[tauri::command]
pub async fn restore_backup<R: tauri::Runtime>(app: AppHandle<R>) -> AppResult<bool> {
    let state = app.state::<AppState>();
    if state.keystore().exists() {
        return Err(AppError::new("keystore_exists", "this device already has a keystore"));
    }
    let Some(path) = pick(&app, false).await? else {
        return Ok(false);
    };
    restore_backup_from(&state, &path).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::date;

    #[test]
    fn dates_for_file_names() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_792_731_791), "2026-10-23");
    }
}
