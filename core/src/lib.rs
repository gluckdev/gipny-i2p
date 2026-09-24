mod core;
mod notify;
mod sanitizer;
mod tray;

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

use crate::core::{AgentMaster, Core, PendingAttachment};
use gipny_libcore::{WireConsole, CONSOLE_COMMAND, CONSOLE_OFF};
use gipny_libcore::agent::BODY_OFF;
use gipny_libcore::crypto::IdentityCard;
use gipny_libcore::db::{Contact, Group, GroupMember, Message, RequestState, TrustLevel};
use gipny_libcore::net::I2pNode;
use gipny_libcore::security::{DuressMode, UnlockOutcome, Vault};

struct AppCtx {
    base_dir: PathBuf,
    profile: Mutex<Option<String>>,
    vault: Mutex<Option<Arc<Vault>>>,
    core: Mutex<Option<Arc<Core>>>,
    /// The i2p node built ahead of the password, or being built right now.
    ///
    /// Nothing in the transport needs the vault: the destination is ephemeral
    /// and the identity the relay routes by lives elsewhere. So the minutes
    /// i2p takes are spent while the unlock screen is on, not after it.
    prewarm: Mutex<Option<Prewarm>>,
}

/// A node started before any profile was opened, held for the profile it was
/// started for. Exactly one of these exists at a time: its destination and
/// its relay's are dropped when another profile is picked.
enum Prewarm {
    Building {
        profile: String,
        settings: gipny_libcore::router::RouterSettings,
        task: tokio::task::JoinHandle<Result<(Arc<I2pNode>, core::PrebuiltRelay), String>>,
    },
    Ready {
        profile: String,
        settings: gipny_libcore::router::RouterSettings,
        node: Arc<I2pNode>,
        /// The built-in relay's tunnels, building as soon as the router is up
        /// (serving nobody until the core claims it after unlock).
        relay: core::PrebuiltRelay,
    },
}

impl Prewarm {
    fn profile(&self) -> &str {
        match self {
            Self::Building { profile, .. } | Self::Ready { profile, .. } => profile,
        }
    }

    fn settings(&self) -> gipny_libcore::router::RouterSettings {
        match self {
            Self::Building { settings, .. } | Self::Ready { settings, .. } => *settings,
        }
    }
}

/// Where the level lives. Not a profile setting: capture starts before any
/// vault is open, and a log that only begins after unlocking would miss the
/// part people actually need — the router, the boot, the crash before that.
fn log_level_path(base_dir: &std::path::Path) -> std::path::PathBuf {
    base_dir.join("log.conf")
}

/// `true` unless the person turned it off. The default changed (2026-09-18, the
/// owner's call): when something goes wrong there has to be something to read,
/// and "reproduce it with the env var set" is not an answer for a phone.
fn log_enabled(base_dir: &std::path::Path) -> bool {
    if std::env::var_os("GIPNY_DEBUG_LOG").is_some() {
        return true;
    }
    !matches!(
        std::fs::read_to_string(log_level_path(base_dir)).map(|s| s.trim().to_ascii_lowercase()),
        Ok(ref v) if v == "off"
    )
}

/// Redirect this process's stderr into a file, one timestamped line at a time.
///
/// Unix only: it is a `dup2`, and Windows has no equivalent that survives the
/// way Tauri starts up. stderr goes into a pipe rather than straight into the
/// file so every line can be stamped — without that, a log of "relay failed"
/// and "router ready" says nothing about *when*, which is most of what one
/// needs from it.
#[cfg(unix)]
fn install_log_capture(base_dir: &std::path::Path) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::io::FromRawFd;

    let log_path = base_dir.join("debug.log");
    // Keep the previous run's log: a crash is only readable afterwards, and
    // truncating on start threw away exactly the interesting one.
    let _ = std::fs::rename(&log_path, base_dir.join("debug.prev.log"));
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(&log_path) else {
        return;
    };

    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return;
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
    if unsafe { libc::dup2(write_fd, libc::STDERR_FILENO) } < 0 {
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return;
    }
    unsafe { libc::close(write_fd) };

    std::thread::spawn(move || {
        let reader = BufReader::new(unsafe { std::fs::File::from_raw_fd(read_fd) });
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let _ = writeln!(file, "{} {line}", log_stamp());
            let _ = file.flush();
        }
    });
    eprintln!("log capture installed: {}", log_path.display());
    eprintln!("gipny {} on {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS);
}

/// `HH:MM:SS.mmm` in local time — enough to line our log up against i2pd's,
/// which stamps the same way, without dragging in a date library.
#[cfg(unix)]
fn log_stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as i64;
    let ms = now.subsec_millis();
    let local = secs + local_utc_offset_secs();
    let tod = local.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}.{:03}", tod / 3600, (tod % 3600) / 60, tod % 60, ms)
}

/// The offset `localtime_r` reports for now, in seconds.
#[cfg(unix)]
fn local_utc_offset_secs() -> i64 {
    unsafe {
        let mut t: libc::time_t = 0;
        libc::time(&mut t);
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

#[cfg(target_os = "windows")]
fn register_aumid() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    let id: Vec<u16> = OsStr::new("app.gipny.i2p").encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
    }
}

#[cfg(not(target_os = "windows"))]
fn register_aumid() {}

/// Best-effort overwrite-and-remove of the app-global debug.log (if any). Called
/// on a duress/attempt-limit wipe so no plaintext log survives outside the
/// per-profile dir that `secure_wipe_dir` scrubs.
fn scrub_debug_log(base_dir: &std::path::Path) {
    scrub_one_log(&base_dir.join("debug.prev.log"));
    scrub_one_log(&base_dir.join("debug.log"));
}

fn scrub_one_log(p: &std::path::Path) {
    let p = p.to_path_buf();
    if let Ok(meta) = std::fs::metadata(&p) {
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&p) {
            use std::io::Write;
            let mut f = f;
            let _ = f.write_all(&vec![0u8; meta.len() as usize]);
            let _ = f.flush();
        }
    }
    let _ = std::fs::remove_file(&p);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    register_aumid();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    gipny_libcore::security::harden_process();
    let base_dir = resolve_base_dir();
    std::fs::create_dir_all(&base_dir).ok();
    // Persistent stderr capture writes a PLAINTEXT debug.log at the app-global
    // base dir (outside the per-profile dir that duress-wipe scrubs). Off by
    // default in release so no transport/app traces survive at rest; opt in with
    // GIPNY_DEBUG_LOG=1 (or any debug build) when you actually need it.
    #[cfg(unix)]
    if log_enabled(&base_dir) {
        install_log_capture(&base_dir);
    }
    let ctx = AppCtx {
        base_dir,
        profile: Mutex::new(None),
        vault: Mutex::new(None),
        core: Mutex::new(None),
        prewarm: Mutex::new(None),
    };
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
            list_profiles, delete_profile, prewarm_network, prewarm_status,
            vault_status, vault_create, vault_unlock, vault_lock, verify_passphrase,
            change_passphrase, set_duress, set_max_attempts,
            my_card, my_onion, my_b32, my_fingerprint, my_bundle, qr_svg,
            get_display_name, set_display_name,
            get_relay_address, set_relay_address,
            get_relay_info, get_dht_status, link_stats, set_lane, set_relay_mode, get_ui_data, set_ui_data, list_unreachable_contacts,
            get_attachment_privacy, set_attachment_privacy,
            update_configured,
            get_router_settings, set_router_settings,
            add_contact, list_contacts, get_contact, update_contact, delete_contact,
            accept_contact_request, decline_contact_request,
            set_contact_bot, reset_contact_session,
            get_agent_mode, set_agent_mode, send_console_command, send_agent_off,
            list_messages, message_position, unread_count, mark_read, delete_message,
            send_message, send_message_paths, send_edit, send_edit_group,
            forward_message,
            list_attachments, load_attachment, save_attachment, save_paste_temp,
            list_media_contact, list_media_group, search_messages,
            list_muted, set_muted,
            paste_clipboard_image,
            press_button, press_group_button,
            list_groups, create_group, list_group_members, list_group_messages,
            add_group_member, send_group_message, send_group_message_paths,
            delete_group, mark_group_read, group_unread_count,
            pin_contact_message, unpin_contact_message, list_pinned_contact,
            pin_group_message, unpin_group_message, list_pinned_group,
            pin_chat, unpin_chat,
            check_update, install_update, update_installs_itself, update_asks_for_root, restart_app, dismiss_update, get_auto_update, set_auto_update, current_version,
            list_apk_artifacts, download_apk,
            read_debug_log, read_previous_log, log_settings, set_log_enabled, clear_debug_log,
            export_identity, import_identity_to_profile,
            send_typing,
            play_notify_sound,
            notify_os,
            notify_probe,
            update_tray_badge,
        ]);

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder
        .setup(|app| {
            // Where pasted, dropped and (on Android) picked files are copied
            // before they are read. std::env::temp_dir() is /data/local/tmp on
            // Android, which an app cannot write to.
            use tauri::Manager;
            if let Ok(dir) = app.path().app_cache_dir() {
                let _ = PASTE_DIR.set(dir.join("gipny-i2p-paste"));
            }
            install_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        });

    builder
        .run(tauri::generate_context!())
        .expect("tauri run");
}

fn resolve_base_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."));
        return base.join("gipny-i2p");
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return base.join("gipny-i2p");
    }
    #[cfg(target_os = "android")]
    {
        let pkg = std::fs::read_to_string("/proc/self/cmdline")
            .ok()
            .and_then(|s| {
                s.split('\0').next().map(|s| s.split(':').next().unwrap_or(s).to_string())
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "app.gipny.i2p".to_string());
        return PathBuf::from(format!("/data/user/0/{}", pkg)).join("gipny");
    }
    #[cfg(target_os = "macos")]
    {
        let base = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support"))
            .unwrap_or_else(|| PathBuf::from("."));
        return base.join("gipny-i2p");
    }
    #[allow(unreachable_code)]
    PathBuf::from(".").join("gipny-i2p")
}

fn profile_dir(ctx: &AppCtx, profile: &str) -> Result<PathBuf, String> {
    if profile.is_empty() || profile.len() > 32 {
        return Err("profile name 1..32 chars".into());
    }
    if !profile.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("profile name: alphanumeric + - _".into());
    }
    Ok(ctx.base_dir.join("profiles").join(profile))
}

#[derive(Serialize)]
struct VaultStatus { exists: bool, unlocked: bool }

#[derive(Serialize)]
struct ContactDto {
    id: i64, sign_pk: String, dh_pk: String, onion: String, name: String,
    trust: u8, created_at: i64, last_seen: Option<i64>, is_bot: bool,
    pinned_at: Option<i64>, last_message_at: Option<i64>,
    /// Relay this contact receives through, from their card. `None` means this
    /// client's own relay setting is used, as it was before cards carried one.
    relay: Option<String>,
    /// This contact is in agent mode with us as master: its console is open.
    agent_granted: bool,
    /// "none", "incoming" (they asked, we have not answered) or "outgoing"
    /// (we added their card and have not heard back).
    request: &'static str,
}

impl From<Contact> for ContactDto {
    fn from(c: Contact) -> Self {
        Self {
            id: c.id, sign_pk: hex(&c.identity_sign), dh_pk: hex(&c.identity_dh),
            onion: c.onion_address, name: c.display_name,
            trust: match c.trust {
                TrustLevel::Unverified => 0, TrustLevel::Verified => 1, TrustLevel::Blocked => 2,
            },
            created_at: c.created_at, last_seen: c.last_seen, is_bot: c.is_bot,
            pinned_at: c.pinned_at, last_message_at: c.last_message_at,
            relay: c.relay_address,
            agent_granted: c.agent_granted,
            request: match c.request_state {
                RequestState::None => "none",
                RequestState::Incoming => "incoming",
                RequestState::Outgoing => "outgoing",
            },
        }
    }
}

#[derive(Serialize, Clone)]
struct ButtonDto { text: String, callback_data: String }

#[derive(Serialize)]
struct MessageDto {
    id: i64,
    contact_id: Option<i64>,
    group_id: Option<String>,
    sender_sign_pk: Option<String>,
    outgoing: bool,
    body: String,
    sent_at: i64,
    sent: bool,
    delivered: bool,
    read: bool,
    expires_at: Option<i64>,
    buttons: Option<Vec<Vec<ButtonDto>>>,
    reply_to: Option<i64>,
    /// Console framing: a command, its output, or an agent-mode marker.
    console: Option<ConsoleDto>,
}

#[derive(Serialize)]
struct ConsoleDto {
    kind: u8,
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
    truncated: bool,
}

/// Per-message extras kept beside the row: inline buttons and console frames.
fn attach_extras(db: &gipny_libcore::db::Db, dtos: &mut [MessageDto]) -> Result<(), String> {
    let ids: Vec<i64> = dtos.iter().map(|d| d.id).collect();
    let map = db.load_buttons_batch(&ids).map_err(err)?;
    let consoles = db.load_settings_batch("console_", &ids).map_err(err)?;
    for dto in dtos.iter_mut() {
        if let Some(bytes) = map.get(&dto.id) {
            if let Ok(wire) = bincode::deserialize::<Vec<Vec<gipny_libcore::WireButton>>>(bytes) {
                dto.buttons = Some(wire.into_iter()
                    .map(|row| row.into_iter().map(|b| ButtonDto { text: b.text, callback_data: b.callback_data }).collect())
                    .collect());
            }
        }
        if let Some(bytes) = consoles.get(&dto.id) {
            if let Ok(c) = bincode::deserialize::<WireConsole>(bytes) {
                dto.console = Some(ConsoleDto {
                    kind: c.kind, exit_code: c.exit_code, duration_ms: c.duration_ms, truncated: c.truncated,
                });
            }
        }
    }
    Ok(())
}

impl From<Message> for MessageDto {
    fn from(m: Message) -> Self {
        Self {
            id: m.id,
            contact_id: m.contact_id,
            group_id: m.group_id.as_ref().map(|v| hex(v)),
            sender_sign_pk: m.sender_sign_pk.as_ref().map(|v| hex(v)),
            outgoing: matches!(m.direction, gipny_libcore::db::Direction::Out),
            body: m.body, sent_at: m.sent_at,
            sent: m.sent, delivered: m.delivered, read: m.read, expires_at: m.expires_at,
            buttons: None,
            reply_to: m.reply_to,
            console: None,
        }
    }
}

#[derive(Serialize)]
struct GroupDto {
    id: String, name: String, created_at: i64,
    pinned_at: Option<i64>, last_message_at: Option<i64>,
}

impl From<Group> for GroupDto {
    fn from(g: Group) -> Self {
        Self {
            id: hex(&g.id), name: g.name, created_at: g.created_at,
            pinned_at: g.pinned_at, last_message_at: g.last_message_at,
        }
    }
}

#[derive(Serialize)]
struct GroupMemberDto {
    sign_pk: String, dh_pk: String, onion: String, name: String, is_self: bool,
}

impl From<GroupMember> for GroupMemberDto {
    fn from(m: GroupMember) -> Self {
        Self {
            sign_pk: hex(&m.sign_pk), dh_pk: hex(&m.dh_pk),
            onion: m.onion, name: m.display_name, is_self: m.is_self,
        }
    }
}

#[derive(Serialize)]
struct BundleDto {
    sign_pk: String, dh_pk: String, signed_prekey: String, signed_prekey_sig: String,
    one_time_prekey: Option<String>, one_time_id: Option<i64>,
}

#[derive(Serialize)]
struct AttachmentDto { id: i64, message_id: i64, name: String, size: i64 }

#[derive(Serialize)]
struct MediaItemDto {
    id: i64,
    message_id: i64,
    name: String,
    size: i64,
    sent_at: i64,
}

#[derive(Serialize)]
struct SearchHitDto {
    message: MessageDto,
    contact_id: Option<i64>,
    group_id: Option<String>,
    contact_name: Option<String>,
    group_name: Option<String>,
}

async fn core_of<'a>(ctx: &'a State<'_, AppCtx>) -> Result<Arc<Core>, String> {
    ctx.core.lock().await.clone().ok_or_else(|| "locked".to_string())
}

fn err<E: std::fmt::Display>(e: E) -> String { e.to_string() }

const SETTING_MUTES: &str = "muted_targets";
/// How much transit traffic the bundled router carries. See
/// [`gipny_libcore::router::TransitProfile`] — this is an anonymity setting.
const SETTING_ROUTER_TRANSIT: &str = "router_transit";
/// Whether the router also speaks over a Yggdrasil mesh, when one is running.
const SETTING_ROUTER_YGGDRASIL: &str = "router_yggdrasil";

fn parse_group_id(s: &str) -> Result<Vec<u8>, String> {
    hex_decode(s).ok_or_else(|| "bad group id".to_string())
}

#[tauri::command]
async fn list_profiles(ctx: State<'_, AppCtx>) -> Result<Vec<String>, String> {
    let pdir = ctx.base_dir.join("profiles");
    if !pdir.exists() { return Ok(vec![]); }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&pdir).map_err(err)? {
        let e = entry.map_err(err)?;
        let p = e.path();
        if p.is_dir() && Vault::exists(&p) {
            if let Some(name) = e.file_name().to_str() {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

#[tauri::command]
async fn delete_profile(profile: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let dir = profile_dir(&ctx, &profile)?;
    if ctx.profile.lock().await.as_deref() == Some(profile.as_str()) {
        return Err("profile is active, lock first".into());
    }
    if dir.exists() {
        gipny_libcore::security::secure_wipe_dir(&dir).map_err(err)?;
        let _ = std::fs::remove_dir(&dir);
    }
    Ok(())
}

#[tauri::command]
async fn vault_status(profile: String, ctx: State<'_, AppCtx>) -> Result<VaultStatus, String> {
    let dir = profile_dir(&ctx, &profile)?;
    let active_profile = ctx.profile.lock().await.clone();
    Ok(VaultStatus {
        exists: Vault::exists(&dir),
        unlocked: active_profile.as_deref() == Some(profile.as_str()) && ctx.core.lock().await.is_some(),
    })
}

#[tauri::command]
async fn vault_create(
    profile: String, pass: String, display_name: String, duress_pass: Option<String>,
    duress_wipe: bool, max_attempts: u32,
    ctx: State<'_, AppCtx>, app: AppHandle,
) -> Result<(), String> {
    let display = display_name.trim();
    if display.is_empty() { return Err("display name required".into()); }
    if display.chars().count() > 64 { return Err("display name too long (max 64)".into()); }
    let dir = profile_dir(&ctx, &profile)?;
    if Vault::exists(&dir) {
        // A previous first run may have been killed between Vault::create and
        // the end of boot (on mobile the heavy KDF + i2p bootstrap made that
        // window wide). If the same passphrase opens the vault and the profile
        // never finished initializing (display name is only written at the very
        // end), resume initialization instead of dead-ending.
        let vault = Arc::new(Vault::open(&dir).map_err(err)?);
        let mk = match vault.unlock(&pass) {
            Ok(UnlockOutcome::Primary(k)) => k,
            _ => return Err("profile already exists".into()),
        };
        {
            let db = gipny_libcore::db::Db::open(&dir.join("data.db"), &mk).map_err(err)?;
            if db.get_setting("display_name").map_err(err)?.is_some() {
                return Err("profile already exists".into());
            }
        }
        boot(&ctx, app, vault, &pass, &profile, &dir).await?;
        let core = ctx.core.lock().await.clone().ok_or("boot failed")?;
        core.db().set_setting("display_name", display.as_bytes()).map_err(err)?;
        return Ok(());
    }
    let mode = if duress_wipe { DuressMode::Wipe } else { DuressMode::Decoy };
    let vault = Arc::new(
        Vault::create(&dir, &pass, duress_pass.as_deref(), mode, max_attempts).map_err(err)?,
    );
    boot(&ctx, app, vault, &pass, &profile, &dir).await?;
    let core = ctx.core.lock().await.clone().ok_or("boot failed")?;
    core.db().set_setting("display_name", display.as_bytes()).map_err(err)?;
    Ok(())
}

#[tauri::command]
async fn vault_unlock(
    profile: String, pass: String,
    ctx: State<'_, AppCtx>, app: AppHandle,
) -> Result<Option<String>, String> {
    let dir = profile_dir(&ctx, &profile)?;
    let prev = ctx.core.lock().await.take();
    if let Some(c) = prev {
        c.shutdown();
        drop(c);
        ctx.vault.lock().await.take();
        ctx.profile.lock().await.take();
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    let vault = Arc::new(Vault::open(&dir).map_err(err)?);
    boot(&ctx, app, vault, &pass, &profile, &dir).await
}

/// One step of opening a profile, for the screen that would otherwise show a
/// disabled button for three minutes. `stage` is a stable id the interface
/// turns into its own wording; `detail` is the technical line (the same one
/// that goes to stderr), shown under «технические подробности».
#[derive(Clone, serde::Serialize)]
struct BootStatus {
    stage: &'static str,
    state: &'static str,
    detail: String,
}

fn boot_status(app: &AppHandle, stage: &'static str, state: &'static str, detail: impl Into<String>) {
    let _ = app.emit("boot_status", BootStatus { stage, state, detail: detail.into() });
}

/// The callback libcore reports router and tunnel progress through. Stage ids come
/// from there (`router`, `router-reused`, `tunnels`, `tunnels-done`, `session`,
/// `session-done`); anything unknown still reaches the technical log.
fn boot_progress(app: &AppHandle) -> gipny_libcore::router::BootProgress {
    let app = app.clone();
    Arc::new(move |stage: &str, detail: &str| {
        let (stage, state) = match stage {
            "router" => ("router", "active"),
            "router-reused" => ("router", "done"),
            "tunnels" => ("tunnels", "active"),
            "tunnels-done" => ("tunnels", "done"),
            "session" => ("session", "active"),
            "session-done" => ("session", "done"),
            _ => ("router", "active"),
        };
        boot_status(&app, stage, state, detail);
    })
}

/// Point the router at the network database snapshot shipped as a Tauri
/// resource (the router itself is compiled in).
///
/// `resource_dir()` is authoritative: the deb and AppImage put resources under
/// `usr/lib/<product>/resources/`, which relative probing does not reach. On
/// Android the snapshot is compiled into the binary instead.
///
/// Called before *any* router start — the prewarm below runs before `boot`.
fn resolve_bundled_router(app: &AppHandle) {
    #[cfg(not(target_os = "android"))]
    if std::env::var_os("GIPNY_I2P_SEED").is_none() {
        use tauri::Manager;
        if let Ok(res) = app.path().resource_dir() {
            for cand in [res.join("i2pd-netdb-seed.tar.gz"), res.join("resources").join("i2pd-netdb-seed.tar.gz")] {
                if cand.exists() {
                    std::env::set_var("GIPNY_I2P_SEED", cand);
                    break;
                }
            }
        }
    }
    #[cfg(target_os = "android")]
    let _ = app;
}

/// Router knobs live in the encrypted database, which is unreadable until the
/// password is typed — and the router has to start before that. So the last
/// values used are left beside the profile in the clear.
///
/// Nothing new is disclosed: the router's own directory next to this file
/// holds `i2pd.log`, which says far more about how it was started. The file is
/// inside the profile directory, so a duress wipe takes it with everything else.
fn router_hint_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("router.hint")
}

fn read_router_hint(dir: &std::path::Path) -> gipny_libcore::router::RouterSettings {
    let raw = std::fs::read_to_string(router_hint_path(dir)).unwrap_or_default();
    let mut it = raw.split_whitespace();
    gipny_libcore::router::RouterSettings {
        transit: gipny_libcore::router::TransitProfile::parse(it.next().unwrap_or_default()),
        yggdrasil: gipny_libcore::router::Yggdrasil::parse(it.next().unwrap_or_default()),
    }
}

fn write_router_hint(dir: &std::path::Path, s: gipny_libcore::router::RouterSettings) {
    let _ = std::fs::write(
        router_hint_path(dir),
        format!("{} {}\n", s.transit.as_str(), s.yggdrasil.as_str()),
    );
}

/// Whether a node started for `have` can be handed to a profile that wants
/// `want`.
fn router_settings_match(
    have: gipny_libcore::router::RouterSettings,
    want: gipny_libcore::router::RouterSettings,
) -> bool {
    have == want
}

/// Start building i2p tunnels now, for the profile about to be opened.
///
/// The interface calls this as soon as it knows which profile that is — on the
/// unlock screen, and again after a logout — so the router, its tunnels and
/// our destination are up by the time the password is typed. If the guess was
/// wrong (another profile is picked), the node is dropped and a new one built.
#[tauri::command]
async fn prewarm_network(profile: String, ctx: State<'_, AppCtx>, app: AppHandle) -> Result<(), String> {
    let dir = profile_dir(&ctx, &profile)?;
    if !Vault::exists(&dir) { return Err("no such profile".into()); }
    // A profile is already open: it owns the router, leave it alone.
    if ctx.core.lock().await.is_some() { return Ok(()); }
    let settings = read_router_hint(&dir);

    let mut slot = ctx.prewarm.lock().await;
    if let Some(p) = slot.as_ref() {
        if p.profile() == profile && router_settings_match(p.settings(), settings) {
            return Ok(());
        }
    }
    // Strictly take-then-drop: the old node's destinations go before new ones
    // are built on the same router.
    if let Some(old) = slot.take() {
        drop_prewarm(old).await;
    }
    resolve_bundled_router(&app);
    let app2 = app.clone();
    let task = tokio::spawn(async move {
        let node = I2pNode::start_with_progress(&dir, settings, Some(boot_progress(&app2)))
            .await
            .map(Arc::new)
            .map_err(|e| format!("{e:?}"))?;
        // Our relay's tunnels take 20–40 s more; build them while the password
        // is typed too. Whose relay it is is only known after unlock.
        let relay = tokio::spawn(gipny_libcore::EphemeralRelay::start_unclaimed());
        Ok((node, relay))
    });
    *slot = Some(Prewarm::Building { profile, settings, task });
    Ok(())
}

/// What the unlock screen shows while it waits: `building`, `ready`, `off`.
#[tauri::command]
async fn prewarm_status(ctx: State<'_, AppCtx>) -> Result<&'static str, String> {
    let mut slot = ctx.prewarm.lock().await;
    // A finished build is only noticed when someone looks; do that here so the
    // state this reports is the real one.
    if let Some(Prewarm::Building { task, .. }) = slot.as_ref() {
        if !task.is_finished() { return Ok("building"); }
        let Some(Prewarm::Building { profile, settings, task }) = slot.take() else { unreachable!() };
        match task.await {
            Ok(Ok((node, relay))) => { *slot = Some(Prewarm::Ready { profile, settings, node, relay }); }
            // A failed prewarm is not an error the person has to act on: the
            // ordinary boot path will try again, out loud, after the password.
            _ => return Ok("off"),
        }
    }
    Ok(match slot.as_ref() {
        Some(Prewarm::Ready { .. }) => "ready",
        Some(Prewarm::Building { .. }) => "building",
        None => "off",
    })
}

/// Tear a prewarmed node down and wait for its router to actually be gone:
/// i2pd locks its data directory, and the next one refuses to start while the
/// lock is held ("Could not lock pid file").
async fn drop_prewarm(p: Prewarm) {
    match p {
        Prewarm::Building { task, .. } => {
            task.abort();
            let _ = task.await;
        }
        Prewarm::Ready { node, relay, .. } => {
            relay.abort();
            node.shutdown().await;
            drop(node);
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
}

/// The prewarmed node, if it fits this profile. Anything else is torn down
/// here, so the caller can safely start its own.
async fn take_prewarmed(
    ctx: &State<'_, AppCtx>, app: &AppHandle, profile: &str,
    settings: gipny_libcore::router::RouterSettings,
) -> Option<(Arc<I2pNode>, core::PrebuiltRelay)> {
    let taken = ctx.prewarm.lock().await.take()?;
    if taken.profile() != profile || !router_settings_match(taken.settings(), settings) {
        boot_status(&app, "router", "active", if taken.profile() != profile {
            "the router was started for another profile; restarting it"
        } else {
            "the router was started with other settings; restarting it"
        });
        drop_prewarm(taken).await;
        return None;
    }
    let ready = match taken {
        Prewarm::Ready { node, relay, .. } => (node, relay),
        Prewarm::Building { task, .. } => {
            // Still building: wait for it rather than start a second router.
            // Its progress is already reaching the same boot screen.
            match task.await {
                Ok(Ok(ready)) => ready,
                _ => {
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                    return None;
                }
            }
        }
    };
    // The screen's own listener may have attached after these stages went by.
    boot_status(app, "router", "done", "router ready (started before unlocking)");
    boot_status(app, "tunnels", "done", "tunnels built before unlocking");
    boot_status(app, "session", "done", "destination ready");
    Some(ready)
}

async fn boot(
    ctx: &State<'_, AppCtx>, app: AppHandle, vault: Arc<Vault>,
    pass: &str, profile: &str, dir: &std::path::Path,
) -> Result<Option<String>, String> {
    // As early as this profile's own startup gets: before the vault is even
    // unlocked, and well before the ~1-3 min router wait below. A staged
    // Windows update means the app closes and reopens once here, rather than
    // after the user has sat through both of those for nothing.
    gipny_libcore::update::apply_staged_windows_installer(dir);
    boot_status(&app, "vault", "active", "unlocking the vault (argon2id)");
    // Argon2id with 256 MiB takes a second or three on a desktop and longer on
    // a tired laptop, at 100% of a core. Saying so beats a frozen button.
    let outcome = vault.unlock(pass).map_err(|e| {
        boot_status(&app, "vault", "failed", format!("{e}"));
        err(e)
    })?;
    // The decoy key is freshly random and unrelated to the primary master key,
    // so it cannot open data.db — SQLCipher rejects it and the unlock screen
    // showed a "bad key" error, in front of whoever was applying the coercion.
    // That is the precise situation duress mode exists to avoid, so the decoy
    // gets a database of its own. It is created empty on first use and looks
    // like a profile nobody has written much in, which is the point.
    let (mk, db_name) = match outcome {
        UnlockOutcome::Primary(k) => (k, "data.db"),
        UnlockOutcome::Decoy(k) => (k, "decoy.db"),
        UnlockOutcome::Wiped => {
            // Duress / attempt-limit wipe: also scrub the app-global debug.log,
            // which lives outside the per-profile dir that was just wiped.
            scrub_debug_log(&ctx.base_dir);
            return Err("wiped".into());
        }
    };
    let db = Arc::new(gipny_libcore::db::Db::open(&dir.join(db_name), &mk).map_err(err)?);
    boot_status(&app, "vault", "done", format!("profile opened ({db_name})"));
    resolve_bundled_router(&app);
    // Router knobs are per profile and read once, here: i2pd takes them on its
    // command line and exposes no way to change them afterwards.
    let settings = gipny_libcore::router::RouterSettings {
        transit: db.get_setting(SETTING_ROUTER_TRANSIT)
            .ok()
            .flatten()
            .and_then(|v| String::from_utf8(v).ok())
            .map(|v| gipny_libcore::router::TransitProfile::parse(&v))
            .unwrap_or_default(),
        yggdrasil: db.get_setting(SETTING_ROUTER_YGGDRASIL)
            .ok()
            .flatten()
            .and_then(|v| String::from_utf8(v).ok())
            .map(|v| gipny_libcore::router::Yggdrasil::parse(&v))
            .unwrap_or_default(),
    };
    // So the next launch can start the router before the password (the vault
    // this was just read from is unreadable at that point).
    write_router_hint(dir, settings);
    // Ephemeral per-session i2p address: the node regenerates its destination
    // every launch (identity is the vault keypair, and the relay routes by that
    // key, not by address — so nothing about the address needs persisting).
    let (node, prebuilt_relay) = match take_prewarmed(ctx, &app, profile, settings).await {
        Some((node, relay)) => (node, Some(relay)),
        None => (Arc::new(
            I2pNode::start_with_progress(dir, settings, Some(boot_progress(&app)))
                .await
                .map_err(|e| {
                    boot_status(&app, "router", "failed", format!("{e:?}"));
                    err(e)
                })?,
        ), None),
    };
    let warning: Option<String> = None;
    boot_status(&app, "core", "active", "starting the messenger core");
    let (core, mut events) = Core::start(dir.to_path_buf(), db, node, prebuilt_relay).await.map_err(|e| {
        boot_status(&app, "core", "failed", format!("{e:?}"));
        err(e)
    })?;
    boot_status(&app, "core", "done", "core running");
    let app2 = app.clone();
    tokio::spawn(async move {
        while let Some(e) = events.recv().await {
            // The unlock screen waits for the relay and the network, and those
            // only exist as core events. Mirror the two it needs so it does not
            // depend on the main view's listener being attached yet.
            match &e {
                crate::core::CoreEvent::RelayInfoChanged { info } => match &info.hosted {
                    crate::core::HostedRelayState::Ready { address } => {
                        boot_status(&app2, "relay", "done", format!("built-in relay ready at {}", &address[..address.len().min(16)]));
                    }
                    crate::core::HostedRelayState::Failed { reason } => {
                        boot_status(&app2, "relay", "failed", reason.clone());
                    }
                    crate::core::HostedRelayState::Starting => {
                        boot_status(&app2, "relay", "active", "building the relay's tunnels");
                    }
                    crate::core::HostedRelayState::Off => {}
                },
                crate::core::CoreEvent::DhtJoined { peers } => {
                    boot_status(&app2, "dht", "done", format!("relay network: {peers} node(s) known"));
                }
                _ => {}
            }
            let _ = app2.emit("core_event", &e);
        }
    });
    *ctx.profile.lock().await = Some(profile.to_string());
    *ctx.vault.lock().await = Some(vault);
    *ctx.core.lock().await = Some(core);
    Ok(warning)
}

/// Check the profile's passphrase without touching the running session.
///
/// Used by «кофеин» to let someone out of a locked-to-one-chat screen. It goes
/// through the real `Vault::unlock`, so the attempt counter, the wipe limit and
/// the duress passphrase all behave exactly as they do on the unlock screen —
/// a mode that could be left with a password the vault does not honour would be
/// a second, weaker door.
#[tauri::command]
async fn verify_passphrase(pass: String, ctx: State<'_, AppCtx>) -> Result<String, String> {
    let vault = ctx.vault.lock().await.clone().ok_or("no profile open")?;
    match vault.unlock(&pass).map_err(err)? {
        UnlockOutcome::Primary(_) => Ok("ok".into()),
        // The real profile is already open on screen, so a decoy cannot hide
        // anything here; treat it as the wrong password rather than pretend.
        UnlockOutcome::Decoy(_) => Err("invalid passphrase".into()),
        UnlockOutcome::Wiped => Ok("wiped".into()),
    }
}

#[tauri::command]
async fn vault_lock(ctx: State<'_, AppCtx>) -> Result<(), String> {
    let core = ctx.core.lock().await.take();
    if let Some(c) = core {
        c.shutdown();
        drop(c);
    }
    ctx.vault.lock().await.take();
    ctx.profile.lock().await.take();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    Ok(())
}

#[tauri::command]
async fn change_passphrase(old: String, new: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    if new.len() < 8 { return Err("passphrase too short (min 8)".into()); }
    let v = ctx.vault.lock().await.clone().ok_or("locked")?;
    v.change_passphrase(&old, &new).map_err(err)
}

#[tauri::command]
async fn set_duress(
    pass: String, duress_pass: Option<String>, wipe: bool,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let v = ctx.vault.lock().await.clone().ok_or("locked")?;
    let mode = if wipe { DuressMode::Wipe } else { DuressMode::Decoy };
    v.set_duress(&pass, duress_pass.as_deref(), mode).map_err(err)
}

#[tauri::command]
async fn set_max_attempts(pass: String, max: u32, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let v = ctx.vault.lock().await.clone().ok_or("locked")?;
    v.set_max_attempts(&pass, max).map_err(err)
}

#[tauri::command]
async fn my_card(ctx: State<'_, AppCtx>) -> Result<serde_json::Value, String> {
    let core = core_of(&ctx).await?;
    let card = core.my_card();
    Ok(serde_json::json!({
        "sign_pk": hex(&card.sign_pk),
        "dh_pk": hex(&card.dh_pk),
    }))
}

#[tauri::command]
async fn my_onion(ctx: State<'_, AppCtx>) -> Result<String, String> {
    Ok(core_of(&ctx).await?.my_onion().to_string())
}

#[tauri::command]
async fn my_b32(ctx: State<'_, AppCtx>) -> Result<String, String> {
    Ok(core_of(&ctx).await?.my_b32())
}

#[tauri::command]
async fn get_relay_address(ctx: State<'_, AppCtx>) -> Result<String, String> {
    Ok(core_of(&ctx).await?.get_relay_address())
}

#[tauri::command]
async fn set_relay_address(addr: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.set_relay_address(&addr).map_err(err)
}

/// Relay mode, the saved external address, and the state of the built-in relay.
#[tauri::command]
async fn get_relay_info(ctx: State<'_, AppCtx>) -> Result<crate::core::RelayInfo, String> {
    Ok(core_of(&ctx).await?.relay_info())
}

/// The relay network as this node sees it (Settings → Сеть).
#[tauri::command]
async fn get_dht_status(ctx: State<'_, AppCtx>) -> Result<gipny_libcore::dht_client::DhtStatus, String> {
    Ok(core_of(&ctx).await?.dht_status())
}

/// The channel to one contact, for the readout above the chat.
#[tauri::command]
async fn link_stats(contact_id: i64, ctx: State<'_, AppCtx>) -> Result<crate::core::LinkStats, String> {
    Ok(core_of(&ctx).await?.link_stats(contact_id).await)
}

/// Trade anonymity for speed, or give it back.
///
/// Slow on purpose: tunnels are rebuilt, which takes tens of seconds, and the
/// interface waits on it rather than pretending the change was instant.
#[tauri::command]
async fn set_lane(lane: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let lane = match lane.as_str() {
        "normal" => crate::core::Lane::Normal,
        "fast" => crate::core::Lane::Fast,
        other => return Err(format!("unknown lane {other}")),
    };
    core_of(&ctx).await?.set_lane(lane).await.map_err(err)
}

#[tauri::command]
async fn set_relay_mode(mode: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let mode = crate::core::RelayMode::parse(&mode).ok_or("bad relay mode")?;
    core_of(&ctx).await?.set_relay_mode(mode).await.map_err(err)
}

/// Contacts whose relay has not answered for a while with mail waiting.
#[tauri::command]
async fn list_unreachable_contacts(ctx: State<'_, AppCtx>) -> Result<Vec<i64>, String> {
    Ok(core_of(&ctx).await?.unreachable_contacts().await)
}

const SETTING_ATTACHMENT_PRIVACY: &str = "attachment_privacy";

fn attachment_privacy_enabled(db: &gipny_libcore::db::Db) -> Result<bool, String> {
    Ok(!matches!(
        db.get_setting(SETTING_ATTACHMENT_PRIVACY).map_err(err)?.as_deref(),
        Some(b"0"),
    ))
}

#[tauri::command]
async fn get_attachment_privacy(ctx: State<'_, AppCtx>) -> Result<bool, String> {
    let core = core_of(&ctx).await?;
    attachment_privacy_enabled(core.db())
}

#[tauri::command]
async fn set_attachment_privacy(enabled: bool, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.db()
        .set_setting(SETTING_ATTACHMENT_PRIVACY, if enabled { b"1" } else { b"0" })
        .map_err(err)
}

#[tauri::command]
async fn my_fingerprint(ctx: State<'_, AppCtx>) -> Result<String, String> {
    Ok(hex(&core_of(&ctx).await?.my_fingerprint()))
}

#[tauri::command]
async fn my_bundle(ctx: State<'_, AppCtx>) -> Result<BundleDto, String> {
    let b = core_of(&ctx).await?.my_bundle().map_err(err)?;
    Ok(BundleDto {
        sign_pk: hex(&b.identity.sign_pk),
        dh_pk: hex(&b.identity.dh_pk),
        signed_prekey: hex(&b.signed_prekey),
        signed_prekey_sig: hex(&b.signed_prekey_sig),
        one_time_prekey: b.one_time_prekey.as_ref().map(|k| hex(&k[..])),
        one_time_id: b.one_time_id,
    })
}

#[tauri::command]
async fn get_display_name(ctx: State<'_, AppCtx>) -> Result<String, String> {
    core_of(&ctx).await?.display_name().map_err(err)
}

#[tauri::command]
async fn set_display_name(name: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.db().set_setting("display_name", name.as_bytes()).map_err(err)
}

#[tauri::command]
async fn add_contact(
    onion: String, sign_pk: String, dh_pk: String, name: String, relay: Option<String>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let sign = parse_hex32(&sign_pk)?;
    let dh = parse_hex32(&dh_pk)?;
    let card = IdentityCard { sign_pk: sign, dh_pk: dh };
    let relay = relay.as_deref().map(str::trim).filter(|r| !r.is_empty());
    core_of(&ctx).await?.add_contact_via(&card, &onion, &name, relay).await.map_err(err)
}

#[tauri::command]
async fn list_contacts(ctx: State<'_, AppCtx>) -> Result<Vec<ContactDto>, String> {
    let core = core_of(&ctx).await?;
    let list = core.db().list_contacts().map_err(err)?;
    // Contacts being deleted for both sides are gone as far as anyone looking
    // is concerned; they stay in the database only to carry the request.
    Ok(list.into_iter().filter(|c| core.wipe_pending_since(c.id).is_none()).map(ContactDto::from).collect())
}

#[tauri::command]
async fn get_contact(id: i64, ctx: State<'_, AppCtx>) -> Result<Option<ContactDto>, String> {
    Ok(core_of(&ctx).await?.db().get_contact(id).map_err(err)?.map(ContactDto::from))
}

#[tauri::command]
async fn update_contact(id: i64, name: String, trust: u8, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let t = match trust {
        0 => TrustLevel::Unverified, 1 => TrustLevel::Verified, 2 => TrustLevel::Blocked,
        _ => return Err("bad trust".into()),
    };
    core_of(&ctx).await?.update_contact(id, &name, t).await.map_err(err)
}

// ----- agent mode ------------------------------------------------------------

#[tauri::command]
async fn get_agent_mode(ctx: State<'_, AppCtx>) -> Result<Option<AgentMaster>, String> {
    Ok(core_of(&ctx).await?.agent_master())
}

/// `Some(contact)` switches agent mode on with that contact as master; `None`
/// switches it off and tells the master.
#[tauri::command]
async fn set_agent_mode(contact_id: Option<i64>, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.set_agent_mode(contact_id).await.map_err(err)
}

/// Master side: a line typed in the console, with any files to upload first.
#[tauri::command]
async fn send_console_command(
    contact_id: i64, body: String, paths: Vec<String>, ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let core = core_of(&ctx).await?;
    let mut atts = Vec::with_capacity(paths.len());
    for p in &paths {
        atts.push(read_one_attachment(p, false)?);
    }
    core.send_console(contact_id, body, WireConsole::new(CONSOLE_COMMAND), atts).await.map_err(err)
}

/// Master side: switch the agent's mode off remotely.
#[tauri::command]
async fn send_agent_off(contact_id: i64, ctx: State<'_, AppCtx>) -> Result<i64, String> {
    core_of(&ctx).await?
        .send_console(contact_id, BODY_OFF.into(), WireConsole::new(CONSOLE_OFF), vec![])
        .await.map_err(err)
}

#[tauri::command]
async fn delete_contact(id: i64, for_both: Option<bool>, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    if for_both == Some(true) {
        core.delete_contact_for_both(id).await.map_err(err)
    } else {
        core.delete_contact(id).await.map_err(err)
    }
}

#[tauri::command]
async fn accept_contact_request(id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.accept_contact_request(id).await.map_err(err)
}

#[tauri::command]
async fn decline_contact_request(id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.decline_contact_request(id).await.map_err(err)
}

#[tauri::command]
async fn set_contact_bot(id: i64, is_bot: bool, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.db().set_contact_is_bot(id, is_bot).map_err(err)?;
    Ok(())
}

#[tauri::command]
async fn reset_contact_session(id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.reset_contact_session(id).await.map_err(err)
}

#[tauri::command]
async fn list_messages(
    contact_id: i64, limit: i64, before_id: Option<i64>, ctx: State<'_, AppCtx>,
) -> Result<Vec<MessageDto>, String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    let list = db.list_messages(contact_id, limit, before_id).map_err(err)?;
    let mut dtos: Vec<MessageDto> = list.into_iter().map(MessageDto::from).collect();
    attach_extras(db, &mut dtos)?;
    Ok(dtos)
}

#[tauri::command]
async fn message_position(
    contact_id: Option<i64>, group_id: Option<String>, message_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<Option<i64>, String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    if let Some(cid) = contact_id {
        db.message_position_dm(cid, message_id).map_err(err)
    } else if let Some(gid_hex) = group_id {
        let gid = parse_group_id(&gid_hex)?;
        db.message_position_group(&gid, message_id).map_err(err)
    } else {
        Err("target required".into())
    }
}

#[tauri::command]
async fn unread_count(contact_id: i64, ctx: State<'_, AppCtx>) -> Result<i64, String> {
    core_of(&ctx).await?.db().unread_count(contact_id).map_err(err)
}

#[tauri::command]
async fn mark_read(contact_id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.db().mark_read(contact_id).map_err(err)
}

#[tauri::command]
async fn delete_message(id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.db().delete_message(id).map_err(err)
}

#[tauri::command]
async fn forward_message(
    source_message_id: i64,
    contact_id: Option<i64>,
    group_id: Option<String>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let core = core_of(&ctx).await?;
    let src = core.db().get_message(source_message_id).map_err(err)?
        .ok_or_else(|| "source not found".to_string())?;
    let attachments = core.db().list_attachments(source_message_id).map_err(err)?;
    let mut pending: Vec<PendingAttachment> = Vec::with_capacity(attachments.len());
    for a in &attachments {
        let data = core.read_attachment(a).map_err(err)?;
        pending.push(PendingAttachment { name: a.name.clone(), data });
    }
    if let Some(cid) = contact_id {
        core.send_message(cid, src.body, pending, None, None).await.map_err(err)
    } else if let Some(gid_hex) = group_id {
        let gid = parse_group_id(&gid_hex)?;
        core.send_to_group(&gid, src.body, pending, None, None).await.map_err(err)
    } else {
        Err("target required".into())
    }
}

#[tauri::command]
async fn send_message(
    contact_id: i64, body: String,
    attachments: Vec<serde_json::Value>, ttl_secs: Option<u64>,
    reply_to: Option<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let mut pending = Vec::with_capacity(attachments.len());
    for a in attachments {
        let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("file").to_string();
        let data_b64 = a.get("data").and_then(|v| v.as_str()).ok_or("bad attachment")?;
        let data = base64_decode(data_b64).ok_or("bad base64")?;
        pending.push(PendingAttachment { name, data });
    }
    let ttl = ttl_secs.map(std::time::Duration::from_secs);
    core_of(&ctx).await?.send_message(contact_id, body, pending, ttl, reply_to).await.map_err(err)
}

#[tauri::command]
async fn send_message_paths(
    contact_id: i64, body: String,
    paths: Vec<String>, ttl_secs: Option<u64>,
    reply_to: Option<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let core = core_of(&ctx).await?;
    let ttl = ttl_secs.map(std::time::Duration::from_secs);
    if paths.is_empty() {
        return core.send_message(contact_id, body, vec![], ttl, reply_to).await.map_err(err);
    }
    let sanitize = attachment_privacy_enabled(core.db())?;
    let mut last_id = 0;
    for (i, p) in paths.iter().enumerate() {
        let pa = read_one_attachment(p, sanitize)?;
        let msg_body = if i == 0 { body.clone() } else { String::new() };
        let rt = if i == 0 { reply_to } else { None };
        last_id = core.send_message(contact_id, msg_body, vec![pa], ttl, rt).await.map_err(err)?;
    }
    Ok(last_id)
}

const MAX_ATTACHMENT_BYTES: u64 = 12 * 1024 * 1024;

fn prepare_attachment(name: String, data: Vec<u8>, sanitize: bool) -> Result<PendingAttachment, String> {
    if sanitize {
        let (name, data) = sanitizer::sanitize_attachment_data(&name, &data)?;
        Ok(PendingAttachment { name, data })
    } else {
        // Console uploads are operational files: scripts, configs and command
        // arguments rely on the original name and exact bytes.
        Ok(PendingAttachment { name, data })
    }
}

static PASTE_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The directory temporary copies of attachments go to (see `setup`).
fn paste_dir() -> std::path::PathBuf {
    PASTE_DIR.get().cloned().unwrap_or_else(|| std::env::temp_dir().join("gipny-i2p-paste"))
}

fn read_one_attachment(p: &str, sanitize: bool) -> Result<PendingAttachment, String> {
    let path = std::path::PathBuf::from(p);
    let meta = std::fs::metadata(&path).map_err(err)?;
    if meta.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "файл слишком большой: {} ({} МБ, лимит {} МБ)",
            p, meta.len() / (1024 * 1024), MAX_ATTACHMENT_BYTES / (1024 * 1024)
        ));
    }
    let data = std::fs::read(&path).map_err(err)?;
    let name = path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| "file".into());
    let attachment = prepare_attachment(name, data, sanitize)?;

    // Pasted images and drops arrive through a temp copy (`save_paste_temp`,
    // `paste_clipboard_image`). Remove it only after preparation succeeded: a
    // rejected format can then be retried after privacy mode is switched off.
    if path.starts_with(paste_dir()) {
        let _ = std::fs::remove_file(&path);
    }
    Ok(attachment)
}

#[cfg(test)]
mod attachment_path_tests {
    use super::prepare_attachment;

    #[test]
    fn console_attachment_keeps_the_original_name_and_bytes() {
        let bytes = b"#!/bin/sh\necho hello\n".to_vec();
        let a = prepare_attachment("deploy.sh".into(), bytes.clone(), false).unwrap();
        assert_eq!(a.name, "deploy.sh");
        assert_eq!(a.data, bytes);
    }
}


#[tauri::command]
async fn send_edit(
    contact_id: i64, message_id: i64, new_body: String, ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx).await?.send_edit(contact_id, message_id, new_body).await.map_err(err)
}

#[tauri::command]
async fn send_edit_group(
    group_id: String, message_id: i64, new_body: String, ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx).await?.send_edit_group(&gid, message_id, new_body).await.map_err(err)
}

#[tauri::command]
async fn press_button(
    contact_id: i64, message_id: i64, callback_data: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx).await?.press_button(contact_id, message_id, callback_data).await.map_err(err)
}

#[tauri::command]
async fn press_group_button(
    group_id: String, message_id: i64, callback_data: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx).await?.press_group_button(&gid, message_id, callback_data).await.map_err(err)
}

#[tauri::command]
async fn list_attachments(message_id: i64, ctx: State<'_, AppCtx>) -> Result<Vec<AttachmentDto>, String> {
    let list = core_of(&ctx).await?.db().list_attachments(message_id).map_err(err)?;
    Ok(list.into_iter().map(|a| AttachmentDto {
        id: a.id, message_id: a.message_id, name: a.name, size: a.size,
    }).collect())
}

#[tauri::command]
async fn list_media_contact(contact_id: i64, limit: i64, ctx: State<'_, AppCtx>) -> Result<Vec<MediaItemDto>, String> {
    let rows = core_of(&ctx).await?.db().list_attachments_for_contact(contact_id, limit).map_err(err)?;
    Ok(rows.into_iter().map(|(a, ts)| MediaItemDto {
        id: a.id, message_id: a.message_id, name: a.name, size: a.size, sent_at: ts,
    }).collect())
}

#[tauri::command]
async fn list_media_group(group_id: String, limit: i64, ctx: State<'_, AppCtx>) -> Result<Vec<MediaItemDto>, String> {
    let gid = parse_group_id(&group_id)?;
    let rows = core_of(&ctx).await?.db().list_attachments_for_group(&gid, limit).map_err(err)?;
    Ok(rows.into_iter().map(|(a, ts)| MediaItemDto {
        id: a.id, message_id: a.message_id, name: a.name, size: a.size, sent_at: ts,
    }).collect())
}

#[tauri::command]
async fn search_messages(
    query: String, contact_id: Option<i64>, group_id: Option<String>, limit: i64,
    ctx: State<'_, AppCtx>,
) -> Result<Vec<SearchHitDto>, String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    let gid_bytes = match group_id.as_deref() {
        Some(s) => Some(parse_group_id(s)?),
        None => None,
    };
    let trimmed = query.trim();
    if trimmed.is_empty() { return Ok(vec![]); }
    let rows = db.search_messages(trimmed, contact_id, gid_bytes.as_deref(), limit).map_err(err)?;
    let mut dtos: Vec<MessageDto> = rows.into_iter().map(MessageDto::from).collect();
    attach_extras(db, &mut dtos)?;
    let contacts_by_id: std::collections::HashMap<i64, String> =
        db.list_contacts().map_err(err)?.into_iter().map(|c| (c.id, c.display_name)).collect();
    let groups_by_hex: std::collections::HashMap<String, String> =
        db.list_groups().map_err(err)?.into_iter().map(|g| (hex(&g.id), g.name)).collect();
    Ok(dtos.into_iter().map(|dto| SearchHitDto {
        contact_id: dto.contact_id,
        group_id: dto.group_id.clone(),
        contact_name: dto.contact_id.and_then(|cid| contacts_by_id.get(&cid).cloned()),
        group_name: dto.group_id.as_deref().and_then(|g| groups_by_hex.get(g).cloned()),
        message: dto,
    }).collect())
}

#[tauri::command]
async fn list_muted(ctx: State<'_, AppCtx>) -> Result<Vec<String>, String> {
    let core = core_of(&ctx).await?;
    let bytes = core.db().get_setting(SETTING_MUTES).map_err(err)?;
    let list: Vec<String> = bytes
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    Ok(list)
}

#[tauri::command]
async fn set_muted(target_key: String, muted: bool, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    let bytes = core.db().get_setting(SETTING_MUTES).map_err(err)?;
    let mut list: Vec<String> = bytes
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let already = list.iter().any(|k| k == &target_key);
    if muted && !already { list.push(target_key); }
    else if !muted && already { list.retain(|k| k != &target_key); }
    let bytes = serde_json::to_vec(&list).map_err(err)?;
    core.db().set_setting(SETTING_MUTES, &bytes).map_err(err)?;
    Ok(())
}

#[tauri::command]
async fn load_attachment(attachment_id: i64, ctx: State<'_, AppCtx>) -> Result<String, String> {
    let core = core_of(&ctx).await?;
    let att = core.db().get_attachment(attachment_id).map_err(err)?.ok_or("not found")?;
    let bytes = core.read_attachment(&att).map_err(err)?;
    Ok(base64_encode(&bytes))
}

#[tauri::command]
async fn save_attachment(attachment_id: i64, dest_path: String, app: AppHandle, ctx: State<'_, AppCtx>) -> Result<(), String> {
    use std::io::Write;
    use std::str::FromStr;
    use tauri_plugin_fs::FsExt;
    let core = core_of(&ctx).await?;
    let att = core.db().get_attachment(attachment_id).map_err(err)?.ok_or("not found")?;
    let bytes = core.read_attachment(&att).map_err(err)?;
    // On Android the save dialog returns a content:// URI, which std::fs
    // cannot open; the fs plugin resolves it through the content resolver.
    // On the desktop it is an ordinary path either way.
    let path = tauri_plugin_fs::FilePath::from_str(&dest_path).map_err(|_| "bad path".to_string())?;
    let mut opts = tauri_plugin_fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    let mut file = app.fs().open(path, opts).map_err(err)?;
    file.write_all(&bytes).map_err(err)?;
    Ok(())
}

#[tauri::command]
async fn save_paste_temp(name: String, data: Vec<u8>) -> Result<String, String> {
    let dir = paste_dir();
    std::fs::create_dir_all(&dir).map_err(err)?;
    let safe_name = name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>();
    let prefix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("{}-{}", prefix, safe_name));
    std::fs::write(&path, &data).map_err(err)?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
async fn paste_clipboard_image() -> Result<Option<String>, String> {
    let img = match tokio::task::spawn_blocking(|| {
        let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        cb.get_image().map_err(|e| match e {
            arboard::Error::ContentNotAvailable => "no-image".to_string(),
            other => other.to_string(),
        })
    }).await.map_err(|e| e.to_string())? {
        Ok(img) => img,
        Err(s) if s == "no-image" => return Ok(None),
        Err(s) => return Err(s),
    };
    if img.width == 0 || img.height == 0 || img.bytes.is_empty() {
        return Ok(None);
    }
    let mut png_buf = Vec::with_capacity(img.bytes.len() / 4);
    {
        let mut enc = png::Encoder::new(&mut png_buf, img.width as u32, img.height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&img.bytes).map_err(|e| e.to_string())?;
    }
    let dir = paste_dir();
    std::fs::create_dir_all(&dir).map_err(err)?;
    let prefix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("{}-clipboard.png", prefix));
    std::fs::write(&path, &png_buf).map_err(err)?;
    Ok(Some(path.to_string_lossy().to_string()))
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
async fn paste_clipboard_image() -> Result<Option<String>, String> {
    Ok(None)
}

#[tauri::command]
async fn list_groups(ctx: State<'_, AppCtx>) -> Result<Vec<GroupDto>, String> {
    let list = core_of(&ctx).await?.db().list_groups().map_err(err)?;
    Ok(list.into_iter().map(GroupDto::from).collect())
}

#[tauri::command]
async fn create_group(
    name: String, member_contact_ids: Vec<i64>, ctx: State<'_, AppCtx>,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 { return Err("group name 1..64 chars".into()); }
    if member_contact_ids.is_empty() { return Err("pick at least 1 member".into()); }
    let gid = core_of(&ctx).await?.create_group(name, &member_contact_ids).await.map_err(err)?;
    Ok(hex(&gid))
}

#[tauri::command]
async fn list_group_members(group_id: String, ctx: State<'_, AppCtx>) -> Result<Vec<GroupMemberDto>, String> {
    let gid = parse_group_id(&group_id)?;
    let list = core_of(&ctx).await?.db().list_group_members(&gid).map_err(err)?;
    Ok(list.into_iter().map(GroupMemberDto::from).collect())
}

#[tauri::command]
async fn add_group_member(
    group_id: String, contact_id: i64, ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx).await?.add_group_member(&gid, contact_id).await.map_err(err)
}

#[tauri::command]
async fn list_group_messages(
    group_id: String, limit: i64, before_id: Option<i64>, ctx: State<'_, AppCtx>,
) -> Result<Vec<MessageDto>, String> {
    let gid = parse_group_id(&group_id)?;
    let core = core_of(&ctx).await?;
    let db = core.db();
    let list = db.list_group_messages(&gid, limit, before_id).map_err(err)?;
    let mut dtos: Vec<MessageDto> = list.into_iter().map(MessageDto::from).collect();
    attach_extras(db, &mut dtos)?;
    Ok(dtos)
}

#[tauri::command]
async fn send_group_message(
    group_id: String, body: String,
    attachments: Vec<serde_json::Value>, ttl_secs: Option<u64>,
    reply_to: Option<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let gid = parse_group_id(&group_id)?;
    let mut pending = Vec::with_capacity(attachments.len());
    for a in attachments {
        let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("file").to_string();
        let data_b64 = a.get("data").and_then(|v| v.as_str()).ok_or("bad attachment")?;
        let data = base64_decode(data_b64).ok_or("bad base64")?;
        pending.push(PendingAttachment { name, data });
    }
    let ttl = ttl_secs.map(std::time::Duration::from_secs);
    core_of(&ctx).await?.send_to_group(&gid, body, pending, ttl, reply_to).await.map_err(err)
}

#[tauri::command]
async fn send_group_message_paths(
    group_id: String, body: String,
    paths: Vec<String>, ttl_secs: Option<u64>,
    reply_to: Option<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let gid = parse_group_id(&group_id)?;
    let core = core_of(&ctx).await?;
    let ttl = ttl_secs.map(std::time::Duration::from_secs);
    if paths.is_empty() {
        return core.send_to_group(&gid, body, vec![], ttl, reply_to).await.map_err(err);
    }
    let sanitize = attachment_privacy_enabled(core.db())?;
    let mut last_id = 0;
    for (i, p) in paths.iter().enumerate() {
        let pa = read_one_attachment(p, sanitize)?;
        let msg_body = if i == 0 { body.clone() } else { String::new() };
        let rt = if i == 0 { reply_to } else { None };
        last_id = core.send_to_group(&gid, msg_body, vec![pa], ttl, rt).await.map_err(err)?;
    }
    Ok(last_id)
}

#[tauri::command]
async fn delete_group(group_id: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx).await?.db().delete_group(&gid).map_err(err)
}

#[tauri::command]
async fn mark_group_read(group_id: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx).await?.db().mark_group_read(&gid).map_err(err)
}

#[tauri::command]
async fn group_unread_count(group_id: String, ctx: State<'_, AppCtx>) -> Result<i64, String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx).await?.db().group_unread_count(&gid).map_err(err)
}

#[tauri::command]
async fn pin_contact_message(contact_id: i64, message_id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.pin_contact_message(contact_id, message_id, false).await.map_err(err)
}

#[tauri::command]
async fn unpin_contact_message(contact_id: i64, message_id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.pin_contact_message(contact_id, message_id, true).await.map_err(err)
}

#[tauri::command]
async fn list_pinned_contact(contact_id: i64, ctx: State<'_, AppCtx>) -> Result<Vec<MessageDto>, String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    let list = db.list_pinned_contact(contact_id).map_err(err)?;
    let mut dtos: Vec<MessageDto> = list.into_iter().map(MessageDto::from).collect();
    attach_extras(db, &mut dtos)?;
    Ok(dtos)
}

#[tauri::command]
async fn pin_group_message(group_id: String, message_id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx).await?.pin_group_message(&gid, message_id, false).await.map_err(err)
}

#[tauri::command]
async fn unpin_group_message(group_id: String, message_id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx).await?.pin_group_message(&gid, message_id, true).await.map_err(err)
}

#[tauri::command]
async fn pin_chat(contact_id: Option<i64>, group_id: Option<String>, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    let ts = now_ms_helper();
    if let Some(cid) = contact_id {
        db.pin_contact(cid, ts).map_err(err)?;
    } else if let Some(gid_hex) = group_id {
        let gid = parse_group_id(&gid_hex)?;
        db.pin_group(&gid, ts).map_err(err)?;
    } else {
        return Err("target required".into());
    }
    Ok(())
}

#[tauri::command]
async fn unpin_chat(contact_id: Option<i64>, group_id: Option<String>, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    if let Some(cid) = contact_id {
        db.unpin_contact(cid).map_err(err)?;
    } else if let Some(gid_hex) = group_id {
        let gid = parse_group_id(&gid_hex)?;
        db.unpin_group(&gid).map_err(err)?;
    } else {
        return Err("target required".into());
    }
    Ok(())
}

#[tauri::command]
async fn list_pinned_group(group_id: String, ctx: State<'_, AppCtx>) -> Result<Vec<MessageDto>, String> {
    let gid = parse_group_id(&group_id)?;
    let core = core_of(&ctx).await?;
    let db = core.db();
    let list = db.list_pinned_group(&gid).map_err(err)?;
    let mut dtos: Vec<MessageDto> = list.into_iter().map(MessageDto::from).collect();
    attach_extras(db, &mut dtos)?;
    Ok(dtos)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RouterSettingsDto {
    /// "frugal" | "balanced" | "generous"
    transit: String,
    /// "off" | "auto" | "on"
    yggdrasil: String,
}

#[tauri::command]
async fn get_router_settings(ctx: State<'_, AppCtx>) -> Result<RouterSettingsDto, String> {
    let db = core_of(&ctx).await?.db().clone();
    let transit = db.get_setting(SETTING_ROUTER_TRANSIT).map_err(err)?
        .and_then(|v| String::from_utf8(v).ok())
        .unwrap_or_else(|| gipny_libcore::router::TransitProfile::default().as_str().to_string());
    let yggdrasil = db.get_setting(SETTING_ROUTER_YGGDRASIL).map_err(err)?
        .and_then(|v| String::from_utf8(v).ok())
        .unwrap_or_else(|| gipny_libcore::router::Yggdrasil::default().as_str().to_string());
    Ok(RouterSettingsDto { transit, yggdrasil })
}

/// Stored for the next start. i2pd reads these from its command line and has no
/// reachable way to change them while running, so this cannot take effect until
/// the router restarts — the UI says so rather than implying otherwise.
#[tauri::command]
async fn set_router_settings(
    settings: RouterSettingsDto, ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let db = core_of(&ctx).await?.db().clone();
    // Normalise through the parser so an unknown value cannot be stored.
    let transit = gipny_libcore::router::TransitProfile::parse(&settings.transit);
    db.set_setting(SETTING_ROUTER_TRANSIT, transit.as_str().as_bytes()).map_err(err)?;
    let ygg = gipny_libcore::router::Yggdrasil::parse(&settings.yggdrasil);
    db.set_setting(SETTING_ROUTER_YGGDRASIL, ygg.as_str().as_bytes()).map_err(err)?;
    // Keep the plaintext hint in step, or the next launch prewarms a router
    // with the old knobs and then throws it away once the vault says otherwise.
    if let Some(p) = ctx.profile.lock().await.clone() {
        if let Ok(dir) = profile_dir(&ctx, &p) {
            write_router_hint(&dir, gipny_libcore::router::RouterSettings { transit, yggdrasil: ygg });
        }
    }
    Ok(())
}

#[tauri::command]
async fn update_configured(ctx: State<'_, AppCtx>) -> Result<bool, String> {
    Ok(core_of(&ctx).await?.update_configured())
}

#[tauri::command]
async fn check_update(ctx: State<'_, AppCtx>) -> Result<serde_json::Value, String> {
    use gipny_libcore::update::CheckOutcome;
    let core = core_of(&ctx).await?;
    // Every outcome is named. «Установлена последняя версия» used to be the
    // answer to four different situations, one of which was "this install
    // cannot be updated from here at all" — which is what a .deb was told
    // while three releases went by without it.
    Ok(match core.check_update_detailed().await.map_err(err)? {
        CheckOutcome::Update(i) => serde_json::json!({
            "status": "update",
            "version": i.version,
            "notes": i.notes,
            "size": i.asset.size,
        }),
        CheckOutcome::UpToDate { latest } => serde_json::json!({ "status": "current", "latest": latest }),
        CheckOutcome::Dismissed { version } => serde_json::json!({ "status": "dismissed", "version": version }),
        CheckOutcome::NotConfigured => serde_json::json!({ "status": "unavailable" }),
        CheckOutcome::UnsupportedInstall { latest } => serde_json::json!({ "status": "unsupported", "latest": latest }),
        CheckOutcome::NoAsset { latest, wanted } => serde_json::json!({ "status": "no_asset", "latest": latest, "wanted": wanted }),
    })
}

/// Whether this build can put an update in place itself. False on Android
/// (the system installer owns that) and on packages we do not manage (.deb,
/// macOS) — the interface then offers the file instead of a button that lies.
/// A contact card as a QR picture, so two people can exchange cards by
/// pointing one phone at another instead of copying 500 characters. Rendered
/// here (pure Rust) rather than in the interface: one small dependency instead
/// of a JavaScript one. Returns an SVG that scales to whatever box it is put in.
#[tauri::command]
fn qr_svg(text: String) -> Result<String, String> {
    // A v2 card is ~600 characters, comfortably inside QR's limits at the
    // lowest correction level; anything much larger is not a card.
    if text.is_empty() || text.len() > 2000 {
        return Err("nothing to encode".into());
    }
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L)
        .map_err(|e| format!("qr: {e}"))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .quiet_zone(true)
        .min_dimensions(240, 240)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

/// Restart into the version that was just installed. Desktop only: on
/// Android the system owns the process lifecycle, and on a package we do not
/// manage there is nothing new to restart into.
#[tauri::command]
fn restart_app(app: AppHandle) {
    app.restart();
}

#[tauri::command]
fn update_installs_itself() -> bool {
    cfg!(target_os = "windows")
        || (cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some())
        || gipny_libcore::update::is_deb_install()
}

/// Whether installing will ask for the administrator password (a .deb goes
/// through the package manager). The interface says so instead of letting a
/// polkit dialog appear out of nowhere.
#[tauri::command]
fn update_asks_for_root() -> bool {
    gipny_libcore::update::is_deb_install()
}

#[tauri::command]
async fn install_update(ctx: State<'_, AppCtx>) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    core.install_update().await.map_err(err)
}

#[tauri::command]
async fn dismiss_update(version: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.dismiss_update(version).await.map_err(err)
}

#[tauri::command]
async fn get_auto_update(ctx: State<'_, AppCtx>) -> Result<bool, String> {
    Ok(core_of(&ctx).await?.auto_update_enabled())
}

/// Interface data kept in the vault: `contact_folders`, `avatars`.
#[tauri::command]
async fn get_ui_data(key: String, ctx: State<'_, AppCtx>) -> Result<Option<String>, String> {
    core_of(&ctx).await?.ui_data(&key).map_err(err)
}

#[tauri::command]
async fn set_ui_data(key: String, json: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.set_ui_data(&key, &json).map_err(err)
}

#[tauri::command]
async fn set_auto_update(enabled: bool, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.set_auto_update(enabled).map_err(err)
}

#[tauri::command]
fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
async fn list_apk_artifacts(ctx: State<'_, AppCtx>) -> Result<serde_json::Value, String> {
    let core = core_of(&ctx).await?;
    let (version, items) = core.list_apk_artifacts().await.map_err(err)?;
    let arr: Vec<serde_json::Value> = items.into_iter()
        .map(|(arch, size)| serde_json::json!({ "arch": arch, "size": size }))
        .collect();
    Ok(serde_json::json!({ "version": version, "artifacts": arr }))
}

#[tauri::command]
async fn download_apk(arch: String, dest_path: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    core.download_apk(arch, dest_path).await.map_err(err)
}

#[tauri::command]
fn read_debug_log(ctx: State<'_, AppCtx>) -> Result<String, String> {
    read_log_file(&ctx.base_dir.join("debug.log"))
}

/// The log of the run before this one — where a crash left its last words.
#[tauri::command]
fn read_previous_log(ctx: State<'_, AppCtx>) -> Result<String, String> {
    read_log_file(&ctx.base_dir.join("debug.prev.log"))
}

fn read_log_file(p: &std::path::Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|_| String::from("(журнал пуст)"));
    let lines: Vec<&str> = raw.lines().collect();
    let take = lines.len().saturating_sub(2000);
    Ok(lines[take..].join("\n"))
}

/// Whether the log is being written, and where it is.
#[tauri::command]
fn log_settings(ctx: State<'_, AppCtx>) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "enabled": log_enabled(&ctx.base_dir),
        "path": ctx.base_dir.join("debug.log").display().to_string(),
    }))
}

/// Takes effect at the next launch: the capture replaces this process's stderr
/// once, at startup, and there is no way back to a file descriptor that is gone.
#[tauri::command]
fn set_log_enabled(enabled: bool, ctx: State<'_, AppCtx>) -> Result<(), String> {
    std::fs::write(log_level_path(&ctx.base_dir), if enabled { "debug" } else { "off" }).map_err(err)?;
    if !enabled {
        scrub_debug_log(&ctx.base_dir);
    }
    Ok(())
}

/// Wipe what has been written so far, for handing the app to someone else.
#[tauri::command]
fn clear_debug_log(ctx: State<'_, AppCtx>) -> Result<(), String> {
    scrub_debug_log(&ctx.base_dir);
    Ok(())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
    use tauri::Manager;
    let show = MenuItemBuilder::with_id("show", "Show").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&show, &quit]).build()?;
    let raise = |app: &tauri::AppHandle| {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
    };
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().expect("no icon").clone())
        .tooltip("gipny (i2p)")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => raise(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(move |tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                raise(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[tauri::command]
fn play_notify_sound(name: Option<String>) -> Result<(), String> {
    notify::play_sound(name)
}

#[tauri::command]
fn notify_os(title: String, body: String) -> Result<(), String> {
    notify::notify_os(&title, &body)
}

#[tauri::command]
fn notify_probe() -> String {
    notify::probe_report()
}

#[tauri::command]
fn update_tray_badge(app: AppHandle, count: u32) -> Result<(), String> {
    tray::apply(&app, count)
}

#[derive(serde::Serialize, serde::Deserialize)]
/// Version 2 carried attachments sealed whole; version 3 also says how each
/// was sealed (`chunk_size`, files sent in parts). bincode is positional, so
/// the attachment type is the parameter and `version` (first, fixed width)
/// says which to read.
struct BackupV2<A = BackupAttachment> {
    version: u32,
    settings: Vec<(String, Vec<u8>)>,
    contacts: Vec<BackupContact>,
    groups: Vec<BackupGroup>,
    messages: Vec<BackupMessage>,
    attachments: Vec<A>,
    pinned: Vec<(Option<i64>, Option<Vec<u8>>, i64, i64)>,
    prekeys: Vec<BackupPreKey>,
    exported_at: i64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupContact {
    sign_pk: Vec<u8>,
    dh_pk: Vec<u8>,
    onion: String,
    name: String,
    trust: i32,
    is_bot: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupGroup {
    id: Vec<u8>,
    name: String,
    members: Vec<BackupMember>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupMember {
    sign_pk: Vec<u8>,
    dh_pk: Vec<u8>,
    onion: String,
    name: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupMessage {
    id: i64,
    contact_id: Option<i64>,
    group_id: Option<Vec<u8>>,
    sender_sign_pk: Option<Vec<u8>>,
    direction: i32,
    body: String,
    sent_at: i64,
    sent: bool,
    delivered: bool,
    read: bool,
    expires_at: Option<i64>,
    last_attempt_at: Option<i64>,
    send_attempts: i32,
    reply_to: Option<i64>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupAttachment {
    id: i64,
    message_id: i64,
    name: String,
    size: i64,
    key: Vec<u8>,
    path: String,
    bytes: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupAttachmentV3 {
    id: i64,
    message_id: i64,
    name: String,
    size: i64,
    key: Vec<u8>,
    path: String,
    bytes: Vec<u8>,
    chunk_size: Option<i64>,
}

impl From<BackupAttachment> for BackupAttachmentV3 {
    fn from(a: BackupAttachment) -> Self {
        Self { id: a.id, message_id: a.message_id, name: a.name, size: a.size, key: a.key, path: a.path, bytes: a.bytes, chunk_size: None }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupPreKey {
    id: i64,
    kind: u8,
    private: Vec<u8>,
    public: Vec<u8>,
    created_at: i64,
}

#[tauri::command]
async fn export_identity(passphrase: String, dest_path: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    if passphrase.len() < 8 { return Err("passphrase too short (min 8)".into()); }
    let core = core_of(&ctx).await?;
    let db = core.db();
    let settings = db.list_all_settings().map_err(err)?;
    let contacts: Vec<BackupContact> = db.list_contacts().map_err(err)?.into_iter().map(|c| BackupContact {
        sign_pk: c.identity_sign, dh_pk: c.identity_dh, onion: c.onion_address, name: c.display_name,
        trust: match c.trust {
            gipny_libcore::db::TrustLevel::Unverified => 0,
            gipny_libcore::db::TrustLevel::Verified => 1,
            gipny_libcore::db::TrustLevel::Blocked => 2,
        },
        is_bot: c.is_bot,
    }).collect();
    let mut members_by_group = db.list_all_group_members().map_err(err)?;
    let groups: Vec<BackupGroup> = db.list_groups().map_err(err)?.into_iter().map(|g| {
        let members = members_by_group.remove(&g.id).unwrap_or_default().into_iter().map(|m| BackupMember {
            sign_pk: m.sign_pk, dh_pk: m.dh_pk, onion: m.onion, name: m.display_name,
        }).collect();
        BackupGroup { id: g.id, name: g.name, members }
    }).collect();
    let messages: Vec<BackupMessage> = db.list_all_messages().map_err(err)?.into_iter().map(|m| BackupMessage {
        id: m.id, contact_id: m.contact_id, group_id: m.group_id, sender_sign_pk: m.sender_sign_pk,
        direction: match m.direction { gipny_libcore::db::Direction::In => 0, gipny_libcore::db::Direction::Out => 1 },
        body: m.body, sent_at: m.sent_at, sent: m.sent, delivered: m.delivered, read: m.read,
        expires_at: m.expires_at, last_attempt_at: m.last_attempt_at,
        send_attempts: m.send_attempts as i32, reply_to: m.reply_to,
    }).collect();
    let mut attachments: Vec<BackupAttachmentV3> = Vec::new();
    for a in db.list_all_attachments().map_err(err)? {
        let bytes = std::fs::read(&a.path).unwrap_or_default();
        attachments.push(BackupAttachmentV3 {
            id: a.id, message_id: a.message_id, name: a.name, size: a.size,
            key: a.key, path: a.path, bytes, chunk_size: a.chunk_size,
        });
    }
    let pinned = db.list_all_pinned().map_err(err)?;
    let prekeys: Vec<BackupPreKey> = db.list_all_prekeys().map_err(err)?.into_iter().map(|p| BackupPreKey {
        id: p.id, kind: p.kind as u8, private: p.private, public: p.public, created_at: p.created_at,
    }).collect();
    let backup = BackupV2 {
        version: 3, settings, contacts, groups, messages, attachments, pinned, prekeys,
        exported_at: now_ms_helper(),
    };
    let bytes = bincode::serialize(&backup).map_err(err)?;
    let sealed = gipny_libcore::security::backup_seal(&passphrase, &bytes).map_err(err)?;
    std::fs::write(&dest_path, &sealed).map_err(err)?;
    Ok(())
}

#[tauri::command]
async fn import_identity_to_profile(
    profile: String, vault_pass: String, backup_path: String, backup_pass: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    if profile.is_empty() || profile.contains('/') || profile.contains('\\') {
        return Err("bad profile name".into());
    }
    if vault_pass.len() < 8 { return Err("vault passphrase too short".into()); }
    let blob = std::fs::read(&backup_path).map_err(err)?;
    let plain = gipny_libcore::security::backup_open(&backup_pass, &blob).map_err(|_| "wrong backup passphrase or corrupt file".to_string())?;
    let version = plain.get(..4).map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]])).unwrap_or(0);
    let unknown = || "backup format unknown / corrupted".to_string();
    let backup: BackupV2<BackupAttachmentV3> = match version {
        2 => {
            let b: BackupV2<BackupAttachment> = bincode::deserialize(&plain).map_err(|_| unknown())?;
            BackupV2 {
                version: b.version, settings: b.settings, contacts: b.contacts, groups: b.groups,
                messages: b.messages, attachments: b.attachments.into_iter().map(Into::into).collect(),
                pinned: b.pinned, prekeys: b.prekeys, exported_at: b.exported_at,
            }
        }
        3 => bincode::deserialize(&plain).map_err(|_| unknown())?,
        v => return Err(format!("unsupported backup version: {v}")),
    };
    let dir = ctx.base_dir.join("profiles").join(&profile);
    if dir.exists() {
        if Vault::exists(&dir) { return Err("profile already exists".into()); }
        // Remnants of an interrupted restore (dir created, vault never
        // written) — clear them and retry instead of dead-ending.
        std::fs::remove_dir_all(&dir).map_err(err)?;
    }
    std::fs::create_dir_all(&dir).map_err(err)?;
    let vault = gipny_libcore::security::Vault::create(&dir, &vault_pass, None, gipny_libcore::security::DuressMode::Wipe, 0).map_err(err)?;
    let mk = match vault.unlock(&vault_pass).map_err(err)? {
        gipny_libcore::security::UnlockOutcome::Primary(k) => k,
        _ => return Err("unexpected unlock outcome".into()),
    };
    let db = gipny_libcore::db::Db::open(&dir.join("data.db"), &mk).map_err(err)?;

    db.bulk_set_settings(&backup.settings).map_err(err)?;

    let identity_sign = backup.settings.iter()
        .find(|(k, _)| k == "identity_sign").map(|(_, v)| v.clone())
        .ok_or("backup missing identity_sign")?;

    let mut contact_updates: Vec<(i64, String, gipny_libcore::db::TrustLevel, bool)> = Vec::with_capacity(backup.contacts.len());
    for c in &backup.contacts {
        let cid = db.add_contact(&c.sign_pk, &c.dh_pk, &c.onion, &c.name, None).map_err(err)?;
        let trust = match c.trust { 1 => gipny_libcore::db::TrustLevel::Verified, 2 => gipny_libcore::db::TrustLevel::Blocked, _ => gipny_libcore::db::TrustLevel::Unverified };
        contact_updates.push((cid, c.name.clone(), trust, c.is_bot));
    }
    db.bulk_update_contacts(&contact_updates).map_err(err)?;

    let mut members_flat: Vec<gipny_libcore::db::GroupMember> = Vec::new();
    for g in &backup.groups {
        db.create_group(&g.id, &g.name).map_err(err)?;
        for m in &g.members {
            members_flat.push(gipny_libcore::db::GroupMember {
                group_id: g.id.clone(), sign_pk: m.sign_pk.clone(), dh_pk: m.dh_pk.clone(),
                onion: m.onion.clone(), display_name: m.name.clone(),
                is_self: m.sign_pk.as_slice() == identity_sign.as_slice(),
            });
        }
    }
    db.bulk_add_group_members(&members_flat).map_err(err)?;

    let prekeys: Vec<gipny_libcore::db::PreKey> = backup.prekeys.into_iter().map(|p| gipny_libcore::db::PreKey {
        id: p.id,
        kind: match p.kind { 1 => gipny_libcore::db::PreKeyKind::Signed, 2 => gipny_libcore::db::PreKeyKind::OneTime, _ => gipny_libcore::db::PreKeyKind::Identity },
        private: p.private, public: p.public, created_at: p.created_at,
    }).collect();
    db.bulk_insert_prekeys(&prekeys).map_err(err)?;

    let messages: Vec<gipny_libcore::db::Message> = backup.messages.into_iter().map(|m| gipny_libcore::db::Message {
        id: m.id, contact_id: m.contact_id, group_id: m.group_id, sender_sign_pk: m.sender_sign_pk,
        direction: if m.direction == 1 { gipny_libcore::db::Direction::Out } else { gipny_libcore::db::Direction::In },
        body: m.body, sent_at: m.sent_at, sent: m.sent, delivered: m.delivered, read: m.read,
        expires_at: m.expires_at, last_attempt_at: m.last_attempt_at,
        send_attempts: m.send_attempts as i64, reply_to: m.reply_to,
    }).collect();
    db.bulk_insert_messages(&messages).map_err(err)?;

    let attach_dir = dir.join("attachments");
    std::fs::create_dir_all(&attach_dir).map_err(err)?;
    let mut attachments_db: Vec<gipny_libcore::db::Attachment> = Vec::new();
    for a in backup.attachments {
        let new_path = attach_dir.join(format!("att_{}", a.id));
        if !a.bytes.is_empty() {
            std::fs::write(&new_path, &a.bytes).map_err(err)?;
        }
        attachments_db.push(gipny_libcore::db::Attachment {
            id: a.id, message_id: a.message_id, name: a.name, size: a.size, key: a.key,
            path: new_path.to_string_lossy().to_string(), chunk_size: a.chunk_size,
        });
    }
    db.bulk_insert_attachments(&attachments_db).map_err(err)?;

    db.bulk_insert_pinned(&backup.pinned).map_err(err)?;

    Ok(())
}

#[tauri::command]
async fn send_typing(
    contact_id: Option<i64>, group_id: Option<String>, typing: bool,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    if let Some(cid) = contact_id {
        let _ = core.send_typing_dm(cid, typing).await;
    } else if let Some(gid_hex) = group_id {
        let gid = parse_group_id(&gid_hex)?;
        let _ = core.send_typing_group(&gid, typing).await;
    }
    Ok(())
}

fn now_ms_helper() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b { s.push_str(&format!("{:02x}", x)); }
    s
}

fn parse_hex32(s: &str) -> Result<[u8; 32], String> {
    let v = hex_decode(s).ok_or_else(|| "bad hex".to_string())?;
    if v.len() != 32 { return Err("bad length".into()); }
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    Ok(out)
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 { return None; }
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
    for i in (0..b.len()).step_by(2) { out.push((h(b[i])? << 4) | h(b[i + 1])?); }
    Some(out)
}

fn base64_encode(data: &[u8]) -> String {
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

fn base64_decode(s: &str) -> Option<Vec<u8>> {
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