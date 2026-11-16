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
