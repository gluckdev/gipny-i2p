use gipny_libcore::security::{DuressMode, UnlockOutcome};
use tauri::State;

use crate::{err, AppCtx};

/// Verify using the real vault: attempt limits and duress behavior must apply.
#[tauri::command]
pub(crate) async fn verify_passphrase(
    pass: String,
    ctx: State<'_, AppCtx>,
) -> Result<String, String> {
    let vault = ctx.vault.lock().await.clone().ok_or("no profile open")?;
    match vault.unlock(&pass).map_err(err)? {
        UnlockOutcome::Primary(_) => Ok("ok".into()),
        // A decoy cannot hide a profile that is already open on screen.
        UnlockOutcome::Decoy(_) => Err("invalid passphrase".into()),
        UnlockOutcome::Wiped => Ok("wiped".into()),
    }
}

#[tauri::command]
pub(crate) async fn vault_lock(ctx: State<'_, AppCtx>) -> Result<(), String> {
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
