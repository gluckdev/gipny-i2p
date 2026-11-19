use gipny_libcore::security::DuressMode;
use tauri::State;

use crate::{err, AppCtx};

#[tauri::command]
pub(crate) async fn vault_lock(ctx: State<'_, AppCtx>) -> Result<(), String> {
    let _operation = ctx.begin_profile_operation()?;
    let core = ctx.core.lock().await.take();
    if let Some(c) = core {
        c.shutdown();
        drop(c);
    }
    ctx.vault.lock().await.take();
    ctx.profile.lock().await.take();
    crate::lifecycle::cancel_pending_prewarm(&ctx).await;
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    Ok(())
}

#[tauri::command]
pub(crate) async fn change_passphrase(
    old: String,
    new: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    if new.len() < 8 {
        return Err("passphrase too short (min 8)".into());
    }
    let v = ctx.vault.lock().await.clone().ok_or("locked")?;
    v.change_passphrase(&old, &new).map_err(err)
}

#[tauri::command]
pub(crate) async fn set_duress(
    pass: String,
    duress_pass: Option<String>,
    wipe: bool,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let v = ctx.vault.lock().await.clone().ok_or("locked")?;
    let mode = if wipe {
        DuressMode::Wipe
    } else {
        DuressMode::Decoy
    };
    v.set_duress(&pass, duress_pass.as_deref(), mode)
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn set_max_attempts(
    pass: String,
    max: u32,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let v = ctx.vault.lock().await.clone().ok_or("locked")?;
    v.set_max_attempts(&pass, max).map_err(err)
}

/// Authenticate the profile that is currently displayed without restarting its
/// relay. A decoy password must never expose the primary session (or vice versa).
#[tauri::command]
pub(crate) async fn verify_screen_password(
    profile: String,
    pass: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    use gipny_libcore::security::UnlockOutcome;
    let _operation = ctx.begin_profile_operation()?;
    if ctx.profile.lock().await.as_deref() != Some(&profile) {
        return Err("profile changed".into());
    }
    let vault = ctx.vault.lock().await.clone().ok_or("locked")?;
    let dir = crate::profile_dir(&ctx, &profile)?;
    let decoy = ctx.decoy_session.load(std::sync::atomic::Ordering::Acquire);
    let result = crate::lifecycle::unlock_profile_vault(&ctx, &profile, &dir, &vault, &pass).await;
    match result {
        Ok(UnlockOutcome::Primary(_)) if !decoy => Ok(()),
        Ok(UnlockOutcome::Decoy(_)) if decoy => Ok(()),
        Err(error) if gipny_libcore::security::Vault::exists(&dir) => Err(error),
        _ => {
            if let Some(core) = ctx.core.lock().await.take() { core.shutdown(); }
            ctx.vault.lock().await.take();
            ctx.profile.lock().await.take();
            Err("session closed".into())
        }
    }
}

#[tauri::command]
pub(crate) async fn close_locked_app(app: tauri::AppHandle, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let _operation = ctx.begin_profile_operation()?;
    if ctx.core.lock().await.is_some() || ctx.vault.lock().await.is_some() {
        return Err("lock the profile before closing".into());
    }
    #[cfg(target_os = "android")]
    crate::android::finish_locked_app(&app).await?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    app.exit(0);
    #[cfg(target_os = "ios")]
    let _ = app; // UIKit owns dismissal and termination.
    Ok(())
}
