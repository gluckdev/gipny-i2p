//! Update availability, installation, and package downloads.

use tauri::{AppHandle, State};

use crate::{core_of, err, AppCtx};

#[tauri::command]
pub(crate) async fn update_configured(ctx: State<'_, AppCtx>) -> Result<bool, String> {
    Ok(core_of(&ctx).await?.update_configured())
}

fn update_command_error(error: crate::core::CoreError) -> String {
    match error {
        crate::core::CoreError::Update(inner) => inner.to_string(),
        crate::core::CoreError::Io(inner) => inner.to_string(),
        other => other.to_string(),
    }
}

#[tauri::command]
pub(crate) async fn check_update(ctx: State<'_, AppCtx>) -> Result<serde_json::Value, String> {
    use gipny_libcore::update::CheckOutcome;
    let core = core_of(&ctx).await?;
    // Every outcome is named. «Установлена последняя версия» used to be the
    // answer to four different situations, one of which was "this install
    // cannot be updated from here at all" — which is what a .deb was told
    // while three releases went by without it.
    Ok(
        match core
            .check_update_detailed()
            .await
            .map_err(update_command_error)?
        {
            CheckOutcome::Update(i) => serde_json::json!({
                "status": "update",
                "version": i.version,
                "notes": i.notes,
                "size": i.asset.size,
            }),
            CheckOutcome::UpToDate { latest } => {
                serde_json::json!({ "status": "current", "latest": latest })
            }
            CheckOutcome::Dismissed { version } => {
                serde_json::json!({ "status": "dismissed", "version": version })
            }
            CheckOutcome::NotConfigured => serde_json::json!({ "status": "unavailable" }),
            CheckOutcome::NoStableRelease => serde_json::json!({ "status": "no_release" }),
            CheckOutcome::UnsupportedInstall { latest } => {
                serde_json::json!({ "status": "unsupported", "latest": latest })
            }
            CheckOutcome::NoAsset { latest, wanted } => {
                serde_json::json!({ "status": "no_asset", "latest": latest, "wanted": wanted })
            }
        },
    )
}

/// Restart into the version that was just installed. Desktop only: on
/// Android the system owns the process lifecycle, and on a package we do not
/// manage there is nothing new to restart into.
#[tauri::command]
pub(crate) fn restart_app(app: AppHandle) {
    app.restart();
}

#[tauri::command]
pub(crate) fn update_installs_itself() -> bool {
    cfg!(target_os = "windows")
        || cfg!(target_os = "macos")
        || (cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some())
        || cfg!(target_os = "android")
        || gipny_libcore::update::is_deb_install()
}

/// Whether installing will ask for the administrator password (a .deb goes
/// through the package manager). The interface says so instead of letting a
/// polkit dialog appear out of nowhere.
#[tauri::command]
pub(crate) fn update_asks_for_root() -> bool {
    gipny_libcore::update::is_deb_install()
}

#[tauri::command]
pub(crate) async fn install_update(ctx: State<'_, AppCtx>) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    core.install_update().await.map_err(update_command_error)
}

#[tauri::command]
pub(crate) async fn dismiss_update(version: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .dismiss_update(version)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn get_auto_update(ctx: State<'_, AppCtx>) -> Result<bool, String> {
    Ok(core_of(&ctx).await?.auto_update_enabled())
}

#[tauri::command]
pub(crate) async fn set_auto_update(enabled: bool, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.set_auto_update(enabled).map_err(err)
}

#[tauri::command]
pub(crate) fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
pub(crate) async fn list_apk_artifacts(
    ctx: State<'_, AppCtx>,
) -> Result<serde_json::Value, String> {
    let core = core_of(&ctx).await?;
    let (version, items) = core
        .list_apk_artifacts()
        .await
        .map_err(update_command_error)?;
    let arr: Vec<serde_json::Value> = items
        .into_iter()
        .map(|(arch, size)| serde_json::json!({ "arch": arch, "size": size }))
        .collect();
    Ok(serde_json::json!({ "version": version, "artifacts": arr }))
}

#[tauri::command]
pub(crate) async fn download_apk(
    arch: String,
    dest_path: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    core.download_apk(arch, dest_path)
        .await
        .map_err(update_command_error)
}
