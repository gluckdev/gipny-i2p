use std::sync::Arc;

use gipny_libcore::security::{DuressMode, UnlockOutcome, Vault};
use tauri::{AppHandle, State};

#[cfg(target_os = "android")]
use crate::android_keystore;
use crate::dto::VaultStatus;
use crate::lifecycle::boot;
use crate::{err, profile_dir, AppCtx};

#[tauri::command]
pub(crate) async fn list_profiles(ctx: State<'_, AppCtx>) -> Result<Vec<String>, String> {
    let pdir = ctx.base_dir.join("profiles");
    if !pdir.exists() {
        return Ok(vec![]);
    }
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
pub(crate) async fn delete_profile(profile: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
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
pub(crate) async fn vault_status(
    profile: String,
    ctx: State<'_, AppCtx>,
) -> Result<VaultStatus, String> {
    let dir = profile_dir(&ctx, &profile)?;
    let active_profile = ctx.profile.lock().await.clone();
    Ok(VaultStatus {
        exists: Vault::exists(&dir),
        unlocked: active_profile.as_deref() == Some(profile.as_str())
            && ctx.core.lock().await.is_some(),
    })
}

#[tauri::command]
pub(crate) async fn vault_create(
    profile: String,
    pass: String,
    display_name: String,
    duress_pass: Option<String>,
    duress_wipe: bool,
    max_attempts: u32,
    ctx: State<'_, AppCtx>,
    app: AppHandle,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    android_keystore::wait_ready().await?;
    let display = display_name.trim();
    if display.is_empty() {
        return Err("display name required".into());
    }
    if display.chars().count() > 64 {
        return Err("display name too long (max 64)".into());
    }
    let dir = profile_dir(&ctx, &profile)?;
    if Vault::exists(&dir) {
        // Resume a first run interrupted before boot wrote the display name.
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
        core.db()
            .set_setting("display_name", display.as_bytes())
            .map_err(err)?;
        return Ok(());
    }
    let mode = if duress_wipe {
        DuressMode::Wipe
    } else {
        DuressMode::Decoy
    };
    let vault = Arc::new(
        Vault::create(&dir, &pass, duress_pass.as_deref(), mode, max_attempts).map_err(err)?,
    );
    boot(&ctx, app, vault, &pass, &profile, &dir).await?;
    let core = ctx.core.lock().await.clone().ok_or("boot failed")?;
    core.db()
        .set_setting("display_name", display.as_bytes())
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn vault_unlock(
    profile: String,
    pass: String,
    ctx: State<'_, AppCtx>,
    app: AppHandle,
) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    android_keystore::wait_ready().await?;
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
