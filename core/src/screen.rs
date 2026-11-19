//! Public platform notifications identify a blanked screen or session lock.
//! Only an explicit application action requests sign-out and termination.

use tauri::{AppHandle, Emitter, State};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

pub(crate) fn install(app: &tauri::App) {
    let _ = APP.set(app.handle().clone());
    #[cfg(target_os = "linux")]
    linux::install();
    #[cfg(target_os = "windows")]
    windows::install(app);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    unsafe {
        gipny_observe_screen()
    };
}

#[cfg(target_os = "android")]
pub(crate) fn app_handle() -> Option<&'static AppHandle> {
    APP.get()
}

pub(crate) fn cover() {
    if let Some(app) = APP.get() {
        let _ = app.emit("screen_hidden", ());
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
extern "C" {
    fn gipny_observe_screen();
    fn gipny_device_auth_available() -> bool;
    fn gipny_device_authenticate(reason: *const libc::c_char, context: *mut std::ffi::c_void);
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[no_mangle]
pub extern "C" fn gipny_screen_hidden() {
    cover();
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[no_mangle]
pub unsafe extern "C" fn gipny_device_auth_finished(context: *mut std::ffi::c_void, success: bool) {
    // One callback per LAContext evaluation, even when the system cancels it.
    let sender = unsafe { Box::from_raw(context.cast::<tokio::sync::oneshot::Sender<bool>>()) };
    let _ = sender.send(success);
}

#[tauri::command]
pub(crate) async fn device_screen_auth_available() -> Result<bool, String> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    return Ok(unsafe { gipny_device_auth_available() });
    #[cfg(target_os = "windows")]
    return Ok(windows::available().await.unwrap_or(false));
    #[cfg(target_os = "android")]
    return crate::android::device_auth_available().await;
    #[cfg(not(any(
        target_os = "windows",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    Ok(false)
}

#[tauri::command]
pub(crate) async fn verify_device_screen_auth(
    profile: String,
    reason: String,
    ctx: State<'_, crate::AppCtx>,
) -> Result<(), String> {
    let vault = ctx
        .vault
        .lock()
        .await
        .as_ref()
        .map(std::sync::Arc::downgrade)
        .ok_or("locked")?;
    if ctx.profile.lock().await.as_deref() != Some(profile.as_str()) {
        return Err("profile changed".into());
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let success = {
        let reason = std::ffi::CString::new(reason).map_err(|_| "invalid authentication prompt")?;
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        unsafe { gipny_device_authenticate(reason.as_ptr(), Box::into_raw(Box::new(tx)).cast()) };
        rx.await.unwrap_or(false)
    };
    #[cfg(target_os = "windows")]
    let success = windows::verify(&reason)
        .await
        .map_err(|error| error.to_string())?;
    #[cfg(target_os = "android")]
    let success = crate::android::verify_device_auth(&reason).await?;
    #[cfg(not(any(
        target_os = "windows",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    let success = {
        let _ = reason;
        false
    };
    let same_session = ctx
        .vault
        .lock()
        .await
        .as_ref()
        .is_some_and(|current| vault.ptr_eq(&std::sync::Arc::downgrade(current)));
    if success && same_session {
        Ok(())
    } else {
        Err("authentication cancelled or unavailable".into())
    }
}
