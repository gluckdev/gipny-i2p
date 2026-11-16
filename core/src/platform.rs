//! Platform paths, desktop lifecycle, and notification adapters.

use std::path::PathBuf;

use tauri::AppHandle;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::AppCtx;
use crate::{notify, tray};

#[cfg(target_os = "windows")]
pub(crate) fn register_aumid() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    let id: Vec<u16> = OsStr::new("app.gipny.i2p")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn register_aumid() {}

pub fn resolve_base_dir() -> PathBuf {
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
                s.split('\0')
                    .next()
                    .map(|s| s.split(':').next().unwrap_or(s).to_string())
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

/// Empty the transcript on the way out, if that is what the person asked for.
///
/// Closing the window does not end the app — it hides it — so quitting happens
/// here, on the tray's own menu, and never passes through the interface. Asking
/// the window "did you want the history cleared?" at this point would be asking
/// something that is about to stop existing, so the answer is read from the
/// vault instead.
///
/// The one risk worth naming: on mobile this is not installed at all, and
/// Android kills the process instead of quitting, so on a phone the setting
/// fires on lock and on profile switch, not on the swipe away.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn clear_history_on_exit(app: &tauri::AppHandle) {
    use tauri::Manager;
    let ctx = app.state::<AppCtx>();
    // A local DELETE under the profile's own vault: short, and worth waiting
    // for. Skipping it is what would leave the history on disk.
    tauri::async_runtime::block_on(async {
        // Bounded, because this runs on the thread that has to hand control
        // back for the app to close. An exit that hangs is a far worse outcome
        // than an exit that leaves the history behind, so if the vault is busy
        // we give up and let the app go.
        let cleared = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let Some(core) = ctx.core.lock().await.clone() else {
                // Already locked. Whatever the vault holds belongs to a database
                // we have deliberately closed, and it will be cleared on the
                // next lock that does find it open — if the person locked, that
                // already ran.
                return Ok(false);
            };
            core.clear_history_if_asked()
        })
        .await;
        match cleared {
            Ok(Ok(true)) => {
                gipny_libcore::debug_trace!("[clear-on-exit] transcript cleared on quit")
            }
            Ok(Ok(false)) => {}
            // Nobody is left to tell, but stderr remains available: a person who
            // asked for this and finds the history still there should be able
            // to see that it was tried and failed, not wonder whether it ran.
            Ok(Err(e)) => eprintln!("[clear-on-exit] could not clear the transcript: {e}"),
            Err(_) => {
                eprintln!("[clear-on-exit] gave up waiting for the vault; the app is closing")
            }
        }
    });
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) fn install_tray(app: &tauri::App) -> tauri::Result<()> {
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
            "quit" => {
                clear_history_on_exit(app);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(move |tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } = event
            {
                raise(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[tauri::command]
pub(crate) fn play_notify_sound(name: Option<String>) -> Result<(), String> {
    notify::play_sound(name)
}

#[tauri::command]
pub(crate) fn notify_os(title: String, body: String) -> Result<(), String> {
    notify::notify_os(&title, &body)
}

#[tauri::command]
pub(crate) fn notify_probe() -> String {
    notify::probe_report()
}

#[tauri::command]
pub(crate) fn update_tray_badge(app: AppHandle, count: u32) -> Result<(), String> {
    tray::apply(&app, count)
}
