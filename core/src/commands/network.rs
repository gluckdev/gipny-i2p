use tauri::State;

use crate::{core_of, err, AppCtx};

#[tauri::command]
pub(crate) async fn get_relay_address(ctx: State<'_, AppCtx>) -> Result<String, String> {
    Ok(core_of(&ctx).await?.get_relay_address())
}

#[tauri::command]
pub(crate) async fn set_relay_address(addr: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.set_relay_address(&addr).map_err(err)
}

/// Relay mode, the saved external address, and the state of the built-in relay.
#[tauri::command]
pub(crate) async fn get_relay_info(
    ctx: State<'_, AppCtx>,
) -> Result<crate::core::RelayInfo, String> {
    Ok(core_of(&ctx).await?.relay_info())
}

/// The relay network as this node sees it (Settings → Сеть).
#[tauri::command]
pub(crate) async fn get_dht_status(
    ctx: State<'_, AppCtx>,
) -> Result<gipny_libcore::dht_client::DhtStatus, String> {
    Ok(core_of(&ctx).await?.dht_status())
}

/// The channel to one contact, for the readout above the chat.
#[tauri::command]
pub(crate) async fn link_stats(
    contact_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<crate::core::LinkStats, String> {
    Ok(core_of(&ctx).await?.link_stats(contact_id).await)
}

#[tauri::command]
pub(crate) async fn set_relay_mode(mode: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let mode = crate::core::RelayMode::parse(&mode).ok_or("bad relay mode")?;
    core_of(&ctx).await?.set_relay_mode(mode).await.map_err(err)
}

/// Contacts whose relay has not answered for a while with mail waiting.
#[tauri::command]
pub(crate) async fn list_unreachable_contacts(ctx: State<'_, AppCtx>) -> Result<Vec<i64>, String> {
    Ok(core_of(&ctx).await?.unreachable_contacts().await)
}
