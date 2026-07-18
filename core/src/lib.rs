mod core;
mod notify;
mod tray;

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;

use crate::core::{Core, PendingAttachment};
use gipny_libcore::crypto::IdentityCard;
use gipny_libcore::db::{Contact, Group, GroupMember, Message, TrustLevel};
use gipny_libcore::net::I2pNode;
use gipny_libcore::security::{DuressMode, UnlockOutcome, Vault};

struct AppCtx {
    base_dir: PathBuf,
    profile: Mutex<Option<String>>,
    vault: Mutex<Option<Arc<Vault>>>,
    core: Mutex<Option<Arc<Core>>>,
}

#[cfg(target_os = "android")]
fn install_log_capture(base_dir: &std::path::Path) {
    use std::os::unix::io::AsRawFd;
    let log_path = base_dir.join("debug.log");
    let _ = std::fs::OpenOptions::new()
        .create(true).write(true).truncate(true)
        .open(&log_path)
        .ok()
        .and_then(|file| {
            unsafe {
                if libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) >= 0 {
                    std::mem::forget(file);
                    Some(())
                } else { None }
            }
        });
    eprintln!("[gipny] log capture installed: {}", log_path.display());
}

#[cfg(target_os = "windows")]
fn register_aumid() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    let id: Vec<u16> = OsStr::new("app.gipny").encode_wide().chain(std::iter::once(0)).collect();
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
    let p = base_dir.join("debug.log");
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
    #[cfg(target_os = "android")]
    if cfg!(debug_assertions) || std::env::var_os("GIPNY_DEBUG_LOG").is_some() {
        install_log_capture(&base_dir);
    }
    let ctx = AppCtx {
        base_dir,
        profile: Mutex::new(None),
        vault: Mutex::new(None),
        core: Mutex::new(None),
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
        .plugin(tauri_plugin_notification::init())
        .manage(ctx)
        .invoke_handler(tauri::generate_handler![
            list_profiles, delete_profile,
            vault_status, vault_create, vault_unlock, vault_lock,
            change_passphrase, set_duress, set_max_attempts,
            my_card, my_onion, my_b32, my_fingerprint, my_bundle,
            get_display_name, set_display_name,
            get_relay_address, set_relay_address,
            add_contact, list_contacts, get_contact, update_contact, delete_contact,
            set_contact_bot, reset_contact_session,
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
            check_update, install_update, dismiss_update, current_version,
            list_apk_artifacts, download_apk,
            read_debug_log,
            export_identity, import_identity_to_profile,
            send_typing,
            play_notify_sound,
            notify_os,
            notify_probe,
            update_tray_badge,
        ]);

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder
        .setup(|app| { install_tray(app)?; Ok(()) })
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
        return base.join("gipny");
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return base.join("gipny");
    }
    #[cfg(target_os = "android")]
    {
        let pkg = std::fs::read_to_string("/proc/self/cmdline")
            .ok()
            .and_then(|s| {
                s.split('\0').next().map(|s| s.split(':').next().unwrap_or(s).to_string())
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "app.gipny".to_string());
        return PathBuf::from(format!("/data/user/0/{}", pkg)).join("gipny");
    }
    #[cfg(target_os = "macos")]
    {
        let base = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support"))
            .unwrap_or_else(|| PathBuf::from("."));
        return base.join("gipny");
    }
    #[allow(unreachable_code)]
    PathBuf::from(".").join("gipny")
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
}

fn attach_buttons(db: &gipny_libcore::db::Db, dtos: &mut [MessageDto]) -> Result<(), String> {
    let ids: Vec<i64> = dtos.iter().map(|d| d.id).collect();
    let map = db.load_buttons_batch(&ids).map_err(err)?;
    for dto in dtos.iter_mut() {
        if let Some(bytes) = map.get(&dto.id) {
            if let Ok(wire) = bincode::deserialize::<Vec<Vec<gipny_libcore::WireButton>>>(bytes) {
                dto.buttons = Some(wire.into_iter()
                    .map(|row| row.into_iter().map(|b| ButtonDto { text: b.text, callback_data: b.callback_data }).collect())
                    .collect());
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
    if Vault::exists(&dir) { return Err("profile already exists".into()); }
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

async fn boot(
    ctx: &State<'_, AppCtx>, app: AppHandle, vault: Arc<Vault>,
    pass: &str, profile: &str, dir: &std::path::Path,
) -> Result<Option<String>, String> {
    let outcome = vault.unlock(pass).map_err(err)?;
    let mk = match outcome {
        UnlockOutcome::Primary(k) => k,
        UnlockOutcome::Decoy(k) => k,
        UnlockOutcome::Wiped => {
            // Duress / attempt-limit wipe: also scrub the app-global debug.log,
            // which lives outside the per-profile dir that was just wiped.
            scrub_debug_log(&ctx.base_dir);
            return Err("wiped".into());
        }
    };
    let db = Arc::new(gipny_libcore::db::Db::open(&dir.join("data.db"), &mk).map_err(err)?);
    // Point the transport at the bundled go-i2p router shipped as a Tauri
    // resource. On desktop it's spawned as a child; on Android the router is
    // started in-process by the foreground service, so this is a no-op there.
    #[cfg(not(target_os = "android"))]
    if std::env::var_os("GIPNY_I2P_BIN").is_none() {
        if let Ok(res) = app.path().resource_dir() {
            let name = if cfg!(windows) { "gipny-i2p-router.exe" } else { "gipny-i2p-router" };
            let cand = res.join(name);
            if cand.exists() {
                std::env::set_var("GIPNY_I2P_BIN", cand);
            }
        }
    }
    // Ephemeral per-session i2p address: the node regenerates its destination
    // every launch (identity is the vault keypair, and the relay routes by that
    // key, not by address — so nothing about the address needs persisting).
    let node = Arc::new(I2pNode::start(dir).await.map_err(err)?);
    let warning: Option<String> = None;
    let (core, mut events) = Core::start(dir.to_path_buf(), db, node).await.map_err(err)?;
    let app2 = app.clone();
    tokio::spawn(async move {
        while let Some(e) = events.recv().await {
            let _ = app2.emit("core_event", &e);
        }
    });
    *ctx.profile.lock().await = Some(profile.to_string());
    *ctx.vault.lock().await = Some(vault);
    *ctx.core.lock().await = Some(core);
    Ok(warning)
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
    onion: String, sign_pk: String, dh_pk: String, name: String,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let sign = parse_hex32(&sign_pk)?;
    let dh = parse_hex32(&dh_pk)?;
    let card = IdentityCard { sign_pk: sign, dh_pk: dh };
    core_of(&ctx).await?.add_contact(&card, &onion, &name).await.map_err(err)
}

#[tauri::command]
async fn list_contacts(ctx: State<'_, AppCtx>) -> Result<Vec<ContactDto>, String> {
    let list = core_of(&ctx).await?.db().list_contacts().map_err(err)?;
    Ok(list.into_iter().map(ContactDto::from).collect())
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
    core_of(&ctx).await?.db().update_contact(id, &name, t).map_err(err)
}

#[tauri::command]
async fn delete_contact(id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.db().delete_contact(id).map_err(err)
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
    attach_buttons(db, &mut dtos)?;
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
    let mut last_id = 0;
    for (i, p) in paths.iter().enumerate() {
        let pa = read_one_attachment(p)?;
        let msg_body = if i == 0 { body.clone() } else { String::new() };
        let rt = if i == 0 { reply_to } else { None };
        last_id = core.send_message(contact_id, msg_body, vec![pa], ttl, rt).await.map_err(err)?;
    }
    Ok(last_id)
}

const MAX_ATTACHMENT_BYTES: u64 = 12 * 1024 * 1024;

fn read_one_attachment(p: &str) -> Result<PendingAttachment, String> {
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
        .map(|s| s.to_string())
        .unwrap_or_else(|| "file".into());
    Ok(PendingAttachment { name, data })
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
    attach_buttons(db, &mut dtos)?;
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
async fn save_attachment(attachment_id: i64, dest_path: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    let att = core.db().get_attachment(attachment_id).map_err(err)?.ok_or("not found")?;
    let bytes = core.read_attachment(&att).map_err(err)?;
    std::fs::write(&dest_path, &bytes).map_err(err)?;
    Ok(())
}

#[tauri::command]
async fn save_paste_temp(name: String, data: Vec<u8>) -> Result<String, String> {
    let dir = std::env::temp_dir().join("gipny-paste");
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
    let dir = std::env::temp_dir().join("gipny-paste");
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
    attach_buttons(db, &mut dtos)?;
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
    let mut last_id = 0;
    for (i, p) in paths.iter().enumerate() {
        let pa = read_one_attachment(p)?;
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
    attach_buttons(db, &mut dtos)?;
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
    attach_buttons(db, &mut dtos)?;
    Ok(dtos)
}

#[tauri::command]
async fn check_update(ctx: State<'_, AppCtx>) -> Result<Option<serde_json::Value>, String> {
    let core = core_of(&ctx).await?;
    let info = core.check_and_emit_update().await.map_err(err)?;
    Ok(info.map(|i| serde_json::json!({
        "version": i.version,
        "notes": i.notes,
        "target_key": i.target_key,
        "size": i.artifact.size,
    })))
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
    let p = ctx.base_dir.join("debug.log");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|_| String::from("(no log)"));
    let lines: Vec<&str> = raw.lines().collect();
    let take = lines.len().saturating_sub(500);
    Ok(lines[take..].join("\n"))
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
        .tooltip("gipny")
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
struct BackupV2 {
    version: u32,
    settings: Vec<(String, Vec<u8>)>,
    contacts: Vec<BackupContact>,
    groups: Vec<BackupGroup>,
    messages: Vec<BackupMessage>,
    attachments: Vec<BackupAttachment>,
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
    let mut attachments: Vec<BackupAttachment> = Vec::new();
    for a in db.list_all_attachments().map_err(err)? {
        let bytes = std::fs::read(&a.path).unwrap_or_default();
        attachments.push(BackupAttachment {
            id: a.id, message_id: a.message_id, name: a.name, size: a.size,
            key: a.key, path: a.path, bytes,
        });
    }
    let pinned = db.list_all_pinned().map_err(err)?;
    let prekeys: Vec<BackupPreKey> = db.list_all_prekeys().map_err(err)?.into_iter().map(|p| BackupPreKey {
        id: p.id, kind: p.kind as u8, private: p.private, public: p.public, created_at: p.created_at,
    }).collect();
    let backup = BackupV2 {
        version: 2, settings, contacts, groups, messages, attachments, pinned, prekeys,
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
    let backup: BackupV2 = bincode::deserialize(&plain).map_err(|_| "backup format unknown / corrupted".to_string())?;
    if backup.version != 2 { return Err(format!("unsupported backup version: {}", backup.version)); }
    let dir = ctx.base_dir.join("profiles").join(&profile);
    if dir.exists() { return Err("profile already exists".into()); }
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
        let cid = db.add_contact(&c.sign_pk, &c.dh_pk, &c.onion, &c.name).map_err(err)?;
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
            path: new_path.to_string_lossy().to_string(),
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