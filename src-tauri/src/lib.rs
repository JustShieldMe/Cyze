pub mod commands;
pub mod devnet;
pub mod error;
pub mod logbuf;
pub mod private;
pub mod sidecar;
pub mod state;
pub mod sync_service;
pub mod tailscale;
pub mod tunnel;

use state::AppState;
use tauri::{Emitter, Manager};

/// Poll interval for the idle auto-lock monitor.
const AUTO_LOCK_POLL: std::time::Duration = std::time::Duration::from_secs(20);
/// Default idle timeout (minutes) when the user has not configured one.
const DEFAULT_AUTO_LOCK_MINUTES: u64 = 10;

/// Background task: locks the keystore after the configured idle period, zeroing
/// the in-memory key material and cancelling any running ceremonies. A value of
/// `Some(0)` for `auto_lock_minutes` disables the timeout.
async fn run_auto_lock_monitor<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    loop {
        tokio::time::sleep(AUTO_LOCK_POLL).await;
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let minutes = state
            .load_settings()
            .auto_lock_minutes
            .unwrap_or(DEFAULT_AUTO_LOCK_MINUTES);
        if minutes == 0 {
            continue; // auto-lock disabled
        }
        // Only act while unlocked and actually idle past the threshold.
        if state.unlocked.read().await.is_none() {
            continue;
        }
        if state.idle_millis() < (minutes as i64) * 60_000 {
            continue;
        }
        for (_, handle) in state.ceremonies.lock().await.drain() {
            handle.cancel.cancel();
        }
        state.set_unlocked(None).await;
        let _ = app.emit("keystore:auto-locked", minutes);
    }
}

pub fn run() {
    // Start capturing `tracing` output (to stdout + the in-app log buffer) before
    // anything else runs, so early diagnostics are recorded too.
    logbuf::init_logging();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState::new())
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(run_auto_lock_monitor(handle));
            sync_service::spawn(app.handle().clone());
            commands::polls::spawn(app.handle().clone());
            commands::tor::start_if_enabled(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::keystore::keystore_status,
            commands::about::third_party_notices,
            commands::notify::notify,
            commands::tor::tor_status,
            commands::polls::list_polls,
            commands::polls::create_poll,
            commands::polls::vote_poll,
            commands::polls::close_poll,
            commands::polls::receive_group_messages,
            commands::polls::refresh_polls,
            commands::polls::community_votes,
            commands::polls::poll_community_round,
            commands::payees::list_payees,
            commands::payees::save_payee,
            commands::payees::remove_payee,
            commands::payees::list_payment_reasons,
            commands::payees::set_payment_reason,
            commands::payees::trust_payee,
            commands::rules::group_rules,
            commands::schedules::list_schedules,
            commands::schedules::due_schedules,
            commands::schedules::create_schedule,
            commands::schedules::stop_schedule,
            commands::schedules::record_schedule_run,
            commands::export::save_csv,
            commands::repair::start_repair,
            commands::contacts::read_contact,
            commands::repair::join_repair,
            commands::repair::read_repair_code,
            commands::repair::receive_repair,
            commands::rules::auto_approval_status,
            commands::rules::set_auto_approve,
            commands::tor::set_use_tor,
            commands::backup::backup_status,
            commands::backup::export_backup,
            commands::backup::restore_backup,
            commands::contacts::set_contact_verified,
            commands::invite::prepare_group_server,
            commands::invite::create_invite,
            commands::invite::read_invite,
            commands::invite::accept_invite,
            commands::ui_state::load_ui_state,
            commands::ui_state::save_ui_state,
            commands::keystore::record_activity,
            commands::keystore::create_keystore,
            commands::keystore::import_upstream_config,
            commands::keystore::unlock_keystore,
            commands::keystore::recover_keystore,
            commands::keystore::lock_keystore,
            commands::keystore::change_passphrase,
            commands::keystore::generate_recovery_code,
            commands::keystore::get_identity,
            commands::keystore::set_username,
            commands::contacts::list_contacts,
            commands::contacts::add_contact,
            commands::contacts::remove_contact,
            commands::contacts::set_contact_alias,
            commands::contacts::export_my_contact,
            commands::groups::list_groups,
            commands::groups::group_orchard_keys,
            commands::groups::remove_group,
            commands::groups::rename_group,
            commands::wallet::get_wallet_config,
            commands::wallet::set_wallet_config,
            commands::wallet::lightwalletd_info,
            commands::wallet::detect_local_devnet,
            commands::wallet::wallet_group_status,
            commands::wallet::wallet_init_account,
            commands::wallet::wallet_sync_now,
            commands::wallet::wallet_sync_status,
            commands::wallet::wallet_history,
            commands::wallet::wallet_notes,
            commands::wallet::wallet_receive_address,
            commands::wallet::wallet_new_receive_address,
            commands::wallet::wallet_prepare_send,
            commands::wallet::wallet_prepare_payments,
            commands::wallet::wallet_prepare_send_max,
            commands::wallet::resolve_zns_name,
            commands::wallet::wallet_send,
            commands::wallet::wallet_rebroadcast,
            commands::viewing::viewing_key_status,
            commands::viewing::make_viewing_key_private,
            commands::viewing::join_viewing_key_session,
            commands::viewing::cancel_viewing_key_session,
            commands::viewing::wallet_prepare_move,
            commands::server::get_settings,
            commands::server::set_server_url,
            commands::server::set_advanced_mode,
            commands::server::set_session_config,
            commands::server::set_session_role,
            commands::server::get_active_wallet,
            commands::server::set_active_wallet,
            commands::server::test_server_connection,
            commands::server::trust_server_cert,
            commands::server::cert_fingerprint_of,
            commands::server::start_sidecar,
            commands::server::stop_sidecar,
            commands::server::sidecar_status,
            commands::server::export_sidecar_cert,
            commands::server::start_tunnel,
            commands::server::stop_tunnel,
            commands::server::tunnel_status,
            commands::server::start_tailscale_serve,
            commands::server::stop_tailscale_serve,
            commands::server::tailscale_status,
            commands::server::tailscale_sign_in,
            commands::server::open_url,
            commands::server::get_logs,
            commands::server::clear_logs,
            commands::dkg::start_dkg,
            commands::dkg::cancel_ceremony,
            commands::dkg::live_ceremonies,
            commands::signing::create_signing_session,
            commands::signing::join_signing_session,
            commands::signing::respond_to_signing,
            commands::signing::list_pending_sessions,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Make sure the frostd child does not outlive the app.
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<AppState>() {
                    if let Ok(mut guard) = state.sidecar.try_lock() {
                        if let Some(handle) = guard.take() {
                            let _ = handle.child.kill();
                        }
                    }
                    if let Ok(mut guard) = state.tunnel.try_lock() {
                        if let Some(handle) = guard.take() {
                            let _ = handle.child.kill();
                        }
                    }
                    // Tailscale `serve` lives in the tailscaled daemon, not a
                    // child process, so turn off the mapping synchronously if we
                    // set one — otherwise it outlives the app pointing at a dead
                    // port.
                    if let Ok(mut guard) = state.tailscale.try_lock() {
                        if guard.take().is_some() {
                            crate::tailscale::stop_serve_blocking();
                        }
                    }
                }
            }
        });
}
