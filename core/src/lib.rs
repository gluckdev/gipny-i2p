mod backup;
mod commands;
pub mod core;
mod diagnostics;
mod platform;
use commands::files::PASTE_DIR;
#[cfg(unix)]
use diagnostics::{install_log_capture, log_enabled};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use platform::install_tray;
use platform::register_aumid;
pub use platform::resolve_base_dir;
mod dto;
mod lifecycle;
pub use gipny_libcore::data_lock;
pub use lifecycle::Prewarm;
#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
mod android_keystore;
mod notify;
mod sanitizer;
mod tray;
#[cfg(any(target_os = "android", test))]
mod update_staging;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;
use tokio::sync::Mutex;

use crate::core::Core;
use gipny_libcore::security::Vault;

pub struct AppCtx {
    pub base_dir: PathBuf,
    /// Serializes opening, locking and deleting a profile, including the time
    /// before boot has transferred its destinations into `core`.
    pub(crate) profile_operation: std::sync::atomic::AtomicBool,
    pub(crate) prewarm_generation: std::sync::atomic::AtomicU64,
    pub profile: Mutex<Option<String>>,
    pub vault: Mutex<Option<Arc<Vault>>>,
    pub core: Mutex<Option<Arc<Core>>>,
    /// The i2p node built ahead of the password, or being built right now.
    ///
    /// Nothing in the transport needs the vault: the destination is ephemeral
    /// and the identity the relay routes by lives elsewhere. So the minutes
    /// i2p takes are spent while the unlock screen is on, not after it.
    pub prewarm: Mutex<Option<Prewarm>>,
}

impl AppCtx {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            profile_operation: std::sync::atomic::AtomicBool::new(false),
            prewarm_generation: std::sync::atomic::AtomicU64::new(0),
            profile: Mutex::new(None),
            vault: Mutex::new(None),
            core: Mutex::new(None),
            prewarm: Mutex::new(None),
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    register_aumid();
    gipny_libcore::security::harden_process().expect("process dump protection unavailable");
    let base_dir = resolve_base_dir();
    std::fs::create_dir_all(&base_dir).ok();
    // Plaintext stderr capture exists only in developer/test builds and in
    // prerelease builds made with GIPNY_RELEASE_LOGS=full. In a stable release
    // `log_enabled` is a constant false: no environment variable, saved
    // setting or UI command turns it on, and the capture code is optimised out.
    #[cfg(unix)]
    if log_enabled(&base_dir) {
        install_log_capture(&base_dir);
    }
    let ctx = AppCtx::new(base_dir);
    let builder = tauri::Builder::default();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        use tauri::Manager;
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
    }));
    let builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .manage(ctx)
        .invoke_handler(tauri::generate_handler![
            commands::profiles::list_profiles,
            commands::profiles::delete_profile,
            lifecycle::prewarm::prewarm_network,
            lifecycle::prewarm::prewarm_status,
            commands::profiles::vault_status,
            commands::profiles::vault_create,
            commands::profiles::vault_unlock,
            commands::vault::vault_lock,
            commands::vault::change_passphrase,
            commands::vault::set_duress,
            commands::vault::set_max_attempts,
            commands::identity::my_card,
            commands::identity::my_i2p_address,
            commands::identity::my_b32,
            commands::identity::my_fingerprint,
            commands::identity::my_bundle,
            commands::identity::qr_svg,
            commands::identity::get_display_name,
            commands::identity::set_display_name,
            commands::network::get_relay_address,
            commands::network::set_relay_address,
            commands::network::get_relay_info,
            commands::network::get_dht_status,
            commands::network::link_stats,
            commands::network::set_relay_mode,
            commands::messages::get_ui_data,
            commands::messages::set_ui_data,
            commands::messages::clear_message_history,
            commands::network::list_unreachable_contacts,
            commands::files::get_attachment_privacy,
            commands::files::set_attachment_privacy,
            commands::updates::update_configured,
            lifecycle::get_router_settings,
            lifecycle::set_router_settings,
            commands::contacts::add_contact,
            commands::contacts::list_contacts,
            commands::contacts::get_contact,
            commands::contacts::update_contact,
            commands::contacts::delete_contact,
            commands::contacts::accept_contact_request,
            commands::contacts::decline_contact_request,
            commands::contacts::set_contact_bot,
            commands::contacts::reset_contact_session,
            commands::agent::get_agent_mode,
            commands::agent::set_agent_mode,
            commands::agent::send_console_command,
            commands::agent::send_agent_off,
            commands::messages::list_messages,
            commands::messages::message_position,
            commands::messages::unread_count,
            commands::messages::mark_read,
            commands::messages::delete_message,
            commands::messages::acknowledge_deletion_notices,
            commands::messages::report_peer_event,
            commands::messages::clear_dm_chat,
            commands::messages::send_message,
            commands::messages::send_message_paths,
            commands::messages::send_edit,
            commands::messages::send_edit_group,
            commands::messages::forward_message,
            commands::files::list_attachments,
            commands::files::load_attachment,
            commands::files::save_attachment,
            commands::files::save_paste_temp,
            commands::files::list_media_contact,
            commands::files::list_media_group,
            commands::messages::search_messages,
            commands::files::files_progress,
            commands::files::message_files,
            commands::files::cancel_files,
            commands::messages::list_muted,
            commands::messages::set_muted,
            commands::files::paste_clipboard_image,
            commands::messages::press_button,
            commands::messages::press_group_button,
            commands::groups::list_groups,
            commands::groups::create_group,
            commands::groups::list_group_members,
            commands::groups::list_group_messages,
            commands::groups::add_group_member,
            commands::groups::send_group_message,
            commands::groups::send_group_message_paths,
            commands::groups::delete_group,
            commands::groups::mark_group_read,
            commands::groups::group_unread_count,
            commands::pins::pin_contact_message,
            commands::pins::unpin_contact_message,
            commands::pins::list_pinned_contact,
            commands::pins::pin_group_message,
            commands::pins::unpin_group_message,
            commands::pins::list_pinned_group,
            commands::pins::pin_chat,
            commands::pins::unpin_chat,
            commands::updates::check_update,
            commands::updates::install_update,
            commands::updates::update_installs_itself,
            commands::updates::update_asks_for_root,
            commands::updates::restart_app,
            commands::updates::dismiss_update,
            commands::updates::get_auto_update,
            commands::updates::set_auto_update,
            commands::updates::current_version,
            commands::updates::list_apk_artifacts,
            commands::updates::download_apk,
            diagnostics::read_debug_log,
            diagnostics::read_previous_log,
            diagnostics::log_settings,
            diagnostics::clear_debug_log,
            commands::backup::export_identity,
            commands::backup::import_identity_to_profile,
            commands::messages::send_typing,
            platform::play_notify_sound,
            platform::notify_os,
            platform::notify_probe,
            platform::update_tray_badge,
        ]);

    let builder = builder
        .setup(|app| {
            #[cfg(target_os = "android")]
            android::configure(app.handle().clone());
            // Where pasted, dropped and (on Android) picked files are copied
            // before they are read. std::env::temp_dir() is /data/local/tmp on
            // Android, which an app cannot write to.
            use tauri::Manager;
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                let ctx = app.state::<AppCtx>();
                match data_lock::DataDirLock::acquire(&ctx.base_dir) {
                    Ok(lock) => { app.manage(lock); }
                    // A second writer of the same profile would overwrite the
                    // first one's ratchet state: refuse to start, visibly.
                    Err(error) if data_lock::held_elsewhere(&error) => {
                        use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
                        app.dialog().message(format!("{error}\n\nЗакройте другой экземпляр gipny и запустите снова.\nClose the other gipny instance and start again."))
                            .title("gipny")
                            .kind(MessageDialogKind::Error)
                            .blocking_show();
                        std::process::exit(1);
                    }
                    Err(error) => eprintln!("WARNING: {error} Continuing without the advisory lock."),
                }
            }
            if let Ok(dir) = app.path().app_cache_dir() {
                #[cfg(target_os = "android")]
                update_staging::configure_cache(dir.clone());
                let _ = PASTE_DIR.set(dir.join("gipny-i2p-paste"));
            }
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            install_tray(app)?;
            Ok(())
        });

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder.on_window_event(|window, event| match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = window.hide();
        }
        // The window coming back is the desktop's only cheap signal that the
        // machine may have changed network while we were in the tray — a
        // laptop closed and reopened elsewhere. Without telling the router,
        // it keeps testing reachability against an address that no longer
        // routes and reports itself unreachable for as long as it is asleep.
        tauri::WindowEvent::Focused(true) => gipny_libcore::embedded::network_changed(),
        _ => {}
    });

    builder.run(tauri::generate_context!()).expect("tauri run");
}

pub(crate) fn profile_dir(ctx: &AppCtx, profile: &str) -> Result<PathBuf, String> {
    if profile.is_empty() || profile.len() > 32 {
        return Err("profile name 1..32 chars".into());
    }
    if !profile
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("profile name: alphanumeric + - _".into());
    }
    Ok(ctx.base_dir.join("profiles").join(profile))
}

async fn core_of<'a>(ctx: &'a State<'_, AppCtx>) -> Result<Arc<Core>, String> {
    ctx.core
        .lock()
        .await
        .clone()
        .ok_or_else(|| "locked".to_string())
}

pub(crate) fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

pub(crate) fn parse_group_id(s: &str) -> Result<Vec<u8>, String> {
    hex_decode(s).ok_or_else(|| "bad group id".to_string())
}

pub(crate) fn now_ms_helper() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}

pub(crate) fn parse_hex32(s: &str) -> Result<[u8; 32], String> {
    let v = hex_decode(s).ok_or_else(|| "bad hex".to_string())?;
    if v.len() != 32 {
        return Err("bad length".into());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    Ok(out)
}

pub(crate) fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let b = s.as_bytes();
    let h = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..b.len()).step_by(2) {
        out.push((h(b[i])? << 4) | h(b[i + 1])?);
    }
    Some(out)
}

pub(crate) fn base64_encode(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        out.push(A[((n >> 18) & 63) as usize] as char);
        out.push(A[((n >> 12) & 63) as usize] as char);
        out.push(A[((n >> 6) & 63) as usize] as char);
        out.push(A[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = data.len() - i;
    if rem == 1 {
        let n = (data[i] as u32) << 16;
        out.push(A[((n >> 18) & 63) as usize] as char);
        out.push(A[((n >> 12) & 63) as usize] as char);
        out.push_str("==");
    } else if rem == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        out.push(A[((n >> 18) & 63) as usize] as char);
        out.push(A[((n >> 12) & 63) as usize] as char);
        out.push(A[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v: u32 = match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}
#[cfg(test)]
mod backup_tests {
    use super::*;

    #[test]
    fn profile_directory_validation_rejects_path_components() {
        let ctx = AppCtx::new(PathBuf::from("/unused"));
        for name in ["", ".", "..", "a/b", "a\\b"] {
            assert!(
                profile_dir(&ctx, name).is_err(),
                "accepted profile name {name:?}"
            );
        }
        assert!(profile_dir(&ctx, &"a".repeat(33)).is_err());
        assert_eq!(
            profile_dir(&ctx, "alice_2").unwrap(),
            PathBuf::from("/unused/profiles/alice_2")
        );
    }
}
