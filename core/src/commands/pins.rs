use tauri::State;

use crate::dto::{attach_extras, MessageDto};
use crate::{core_of, err, now_ms_helper, parse_group_id, AppCtx};

#[tauri::command]
pub(crate) async fn pin_contact_message(
    contact_id: i64,
    message_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .pin_contact_message(contact_id, message_id, false)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn unpin_contact_message(
    contact_id: i64,
    message_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .pin_contact_message(contact_id, message_id, true)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn list_pinned_contact(
    contact_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<Vec<MessageDto>, String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    let list = db.list_pinned_contact(contact_id).map_err(err)?;
    let mut dtos: Vec<MessageDto> = list.into_iter().map(MessageDto::from).collect();
    attach_extras(db, &mut dtos)?;
    Ok(dtos)
}

#[tauri::command]
pub(crate) async fn pin_group_message(
    group_id: String,
    message_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx)
        .await?
        .pin_group_message(&gid, message_id, false)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn unpin_group_message(
    group_id: String,
    message_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx)
        .await?
        .pin_group_message(&gid, message_id, true)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn pin_chat(
    contact_id: Option<i64>,
    group_id: Option<String>,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    let ts = now_ms_helper();
    if let Some(cid) = contact_id {
        db.pin_contact(cid, ts).map_err(err)?;
    } else if let Some(gid_hex) = group_id {
        let gid = parse_group_id(&gid_hex)?;
        db.pin_group(&gid, ts).map_err(err)?;
    } else {
        return Err("target required".into());
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn unpin_chat(
    contact_id: Option<i64>,
    group_id: Option<String>,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    if let Some(cid) = contact_id {
        db.unpin_contact(cid).map_err(err)?;
    } else if let Some(gid_hex) = group_id {
        let gid = parse_group_id(&gid_hex)?;
        db.unpin_group(&gid).map_err(err)?;
    } else {
        return Err("target required".into());
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn list_pinned_group(
    group_id: String,
    ctx: State<'_, AppCtx>,
) -> Result<Vec<MessageDto>, String> {
    let gid = parse_group_id(&group_id)?;
    let core = core_of(&ctx).await?;
    let db = core.db();
    let list = db.list_pinned_group(&gid).map_err(err)?;
    let mut dtos: Vec<MessageDto> = list.into_iter().map(MessageDto::from).collect();
    attach_extras(db, &mut dtos)?;
    Ok(dtos)
}
