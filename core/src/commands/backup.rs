//! Password-protected backup import/export adapters.

use tauri::State;

#[cfg(target_os = "android")]
use crate::android_keystore;
use crate::{backup, core_of, err, AppCtx};

#[tauri::command]
pub(crate) async fn export_identity(
    passphrase: String,
    dest_path: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    if passphrase.len() < 8 {
        return Err("passphrase too short (min 8)".into());
    }
    let core = core_of(&ctx).await?;
    let bytes = backup::export_raw_backup_data(core.db())?;
    let sealed = gipny_libcore::security::backup_seal(&passphrase, &bytes).map_err(err)?;
    std::fs::write(&dest_path, &sealed).map_err(err)?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn import_identity_to_profile(
    profile: String,
    vault_pass: String,
    backup_path: String,
    backup_pass: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    android_keystore::wait_ready().await?;
    let blob = std::fs::read(&backup_path).map_err(err)?;
    let plain = gipny_libcore::security::backup_open(&backup_pass, &blob)
        .map_err(|_| "wrong backup passphrase or corrupt file".to_string())?;
    backup::restore_raw_backup_data(ctx.inner(), profile, vault_pass, &plain).await
}
