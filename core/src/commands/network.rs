use tauri::State;

use crate::{core_of, AppCtx};

#[tauri::command]
pub(crate) async fn get_relay_address(ctx: State<'_, AppCtx>) -> Result<String, String> {
    Ok(core_of(&ctx).await?.get_relay_address())
}

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

/// Contacts whose relay has not answered for a while with mail waiting.
#[tauri::command]
pub(crate) async fn list_unreachable_contacts(ctx: State<'_, AppCtx>) -> Result<Vec<i64>, String> {
    Ok(core_of(&ctx).await?.unreachable_contacts().await)
}
