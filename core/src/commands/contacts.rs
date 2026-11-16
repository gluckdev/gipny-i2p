use gipny_libcore::crypto::IdentityCard;
use gipny_libcore::db::TrustLevel;
use tauri::State;

use crate::dto::ContactDto;
use crate::{core_of, err, parse_hex32, AppCtx};

#[tauri::command]
pub(crate) async fn add_contact(
    i2p_address: String,
    sign_pk: String,
    dh_pk: String,
    name: String,
    relay: Option<String>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let sign = parse_hex32(&sign_pk)?;
    let dh = parse_hex32(&dh_pk)?;
    let card = IdentityCard {
        sign_pk: sign,
        dh_pk: dh,
    };
    let relay = relay.as_deref().map(str::trim).filter(|r| !r.is_empty());
    core_of(&ctx)
        .await?
        .add_contact_via(&card, &i2p_address, &name, relay)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn list_contacts(ctx: State<'_, AppCtx>) -> Result<Vec<ContactDto>, String> {
    let core = core_of(&ctx).await?;
    let list = core.db().list_contacts().map_err(err)?;
    // Pending mutual deletions stay in the database only to carry the request.
    Ok(list
        .into_iter()
        .filter(|c| core.wipe_pending_since(c.id).is_none())
        .map(ContactDto::from)
        .collect())
}

#[tauri::command]
pub(crate) async fn get_contact(
    id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<Option<ContactDto>, String> {
    Ok(core_of(&ctx)
        .await?
        .db()
        .get_contact(id)
        .map_err(err)?
        .map(ContactDto::from))
}

#[tauri::command]
pub(crate) async fn update_contact(
    id: i64,
    name: String,
    trust: u8,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let t = match trust {
        0 => TrustLevel::Unverified,
        1 => TrustLevel::Verified,
        2 => TrustLevel::Blocked,
        _ => return Err("bad trust".into()),
    };
    core_of(&ctx)
        .await?
        .update_contact(id, &name, t)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn delete_contact(
    id: i64,
    for_both: Option<bool>,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    if for_both == Some(true) {
        core.delete_contact_for_both(id).await.map_err(err)
    } else {
        core.delete_contact(id).await.map_err(err)
    }
}

#[tauri::command]
pub(crate) async fn accept_contact_request(id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .accept_contact_request(id)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn decline_contact_request(id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .decline_contact_request(id)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn set_contact_bot(
    id: i64,
    is_bot: bool,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .db()
        .set_contact_is_bot(id, is_bot)
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn reset_contact_session(id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .reset_contact_session(id)
        .await
        .map_err(err)
}
