use tauri::State;

use super::files::{attachment_privacy_enabled, read_one_attachment};
use crate::core::PendingAttachment;
use crate::dto::{attach_extras, GroupDto, GroupMemberDto, MessageDto};
use crate::{base64_decode, core_of, err, hex, parse_group_id, AppCtx};

#[tauri::command]
pub(crate) async fn list_groups(ctx: State<'_, AppCtx>) -> Result<Vec<GroupDto>, String> {
    let list = core_of(&ctx).await?.db().list_groups().map_err(err)?;
    Ok(list.into_iter().map(GroupDto::from).collect())
}

#[tauri::command]
pub(crate) async fn create_group(
    name: String,
    member_contact_ids: Vec<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err("group name 1..64 chars".into());
    }
    if member_contact_ids.is_empty() {
        return Err("pick at least 1 member".into());
    }
    let gid = core_of(&ctx)
        .await?
        .create_group(name, &member_contact_ids)
        .await
        .map_err(err)?;
    Ok(hex(&gid))
}

#[tauri::command]
pub(crate) async fn list_group_members(
    group_id: String,
    ctx: State<'_, AppCtx>,
) -> Result<Vec<GroupMemberDto>, String> {
    let gid = parse_group_id(&group_id)?;
    let list = core_of(&ctx)
        .await?
        .db()
        .list_group_members(&gid)
        .map_err(err)?;
    Ok(list.into_iter().map(GroupMemberDto::from).collect())
}

#[tauri::command]
pub(crate) async fn add_group_member(
    group_id: String,
    contact_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx)
        .await?
        .add_group_member(&gid, contact_id)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn list_group_messages(
    group_id: String,
    limit: i64,
    before_id: Option<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<Vec<MessageDto>, String> {
    let gid = parse_group_id(&group_id)?;
    let core = core_of(&ctx).await?;
    let db = core.db();
    let list = db
        .list_group_messages(&gid, limit, before_id)
        .map_err(err)?;
    let mut dtos: Vec<MessageDto> = list.into_iter().map(MessageDto::from).collect();
    attach_extras(db, &mut dtos)?;
    Ok(dtos)
}

#[tauri::command]
pub(crate) async fn send_group_message(
    group_id: String,
    body: String,
    attachments: Vec<serde_json::Value>,
    ttl_secs: Option<u64>,
    reply_to: Option<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let gid = parse_group_id(&group_id)?;
    let mut pending = Vec::with_capacity(attachments.len());
    for a in attachments {
        let name = a
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("file")
            .to_string();
        let data_b64 = a
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or("bad attachment")?;
        let data = base64_decode(data_b64).ok_or("bad base64")?;
        pending.push(PendingAttachment {
            name,
            data,
            from: None,
        });
    }
    let ttl = ttl_secs.map(std::time::Duration::from_secs);
    core_of(&ctx)
        .await?
        .send_to_group(&gid, body, pending, ttl, reply_to)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn send_group_message_paths(
    group_id: String,
    body: String,
    paths: Vec<String>,
    ttl_secs: Option<u64>,
    reply_to: Option<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let gid = parse_group_id(&group_id)?;
    let core = core_of(&ctx).await?;
    let ttl = ttl_secs.map(std::time::Duration::from_secs);
    if paths.is_empty() {
        return core
            .send_to_group(&gid, body, vec![], ttl, reply_to)
            .await
            .map_err(err);
    }
    let sanitize = attachment_privacy_enabled(core.db())?;
    let mut last_id = 0;
    for (i, p) in paths.iter().enumerate() {
        let pa = read_one_attachment(p, sanitize)?;
        let msg_body = if i == 0 { body.clone() } else { String::new() };
        let rt = if i == 0 { reply_to } else { None };
        last_id = core
            .send_to_group(&gid, msg_body, vec![pa], ttl, rt)
            .await
            .map_err(err)?;
    }
    Ok(last_id)
}

#[tauri::command]
pub(crate) async fn delete_group(group_id: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx).await?.db().delete_group(&gid).map_err(err)
}

#[tauri::command]
pub(crate) async fn mark_group_read(
    group_id: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx).await?.db().mark_group_read(&gid).map_err(err)
}

#[tauri::command]
pub(crate) async fn group_unread_count(
    group_id: String,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx)
        .await?
        .db()
        .group_unread_count(&gid)
        .map_err(err)
}
