//! Message sending, history, search, and chat preferences.

use tauri::State;

use super::files::{attachment_privacy_enabled, read_one_attachment};
use crate::core::PendingAttachment;
use crate::dto::{attach_extras, MessageDto, SearchHitDto};
use crate::{base64_decode, core_of, err, hex, parse_group_id, AppCtx};

const SETTING_MUTES: &str = "muted_targets";
#[tauri::command]
pub(crate) async fn list_messages(
    contact_id: i64,
    limit: i64,
    before_id: Option<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<Vec<MessageDto>, String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    let list = db
        .list_messages(contact_id, limit, before_id)
        .map_err(err)?;
    let mut dtos: Vec<MessageDto> = list.into_iter().map(MessageDto::from).collect();
    attach_extras(db, &mut dtos)?;
    Ok(dtos)
}

#[tauri::command]
pub(crate) async fn message_position(
    contact_id: Option<i64>,
    group_id: Option<String>,
    message_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<Option<i64>, String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    if let Some(cid) = contact_id {
        db.message_position_dm(cid, message_id).map_err(err)
    } else if let Some(gid_hex) = group_id {
        let gid = parse_group_id(&gid_hex)?;
        db.message_position_group(&gid, message_id).map_err(err)
    } else {
        Err("target required".into())
    }
}

#[tauri::command]
pub(crate) async fn unread_count(contact_id: i64, ctx: State<'_, AppCtx>) -> Result<i64, String> {
    core_of(&ctx)
        .await?
        .db()
        .unread_count(contact_id)
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn mark_read(contact_id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.db().mark_read(contact_id).map_err(err)
}

#[tauri::command]
pub(crate) async fn delete_message(id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx).await?.delete_message(id).map_err(err)
}

#[tauri::command]
pub(crate) async fn acknowledge_deletion_notices(
    contact_id: i64,
    message_ids: Vec<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .acknowledge_deletion_notices(contact_id, &message_ids)
        .map_err(err)
}

/// `kind`: 5 text copied, 6 attachment saved, 7 screenshot shortcut.
#[tauri::command]
pub(crate) async fn report_peer_event(
    message_id: i64,
    kind: u8,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .report_peer_event(message_id, kind)
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn clear_dm_chat(
    contact_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<usize, String> {
    core_of(&ctx)
        .await?
        .clear_dm_contact(contact_id)
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn forward_message(
    source_message_id: i64,
    contact_id: Option<i64>,
    group_id: Option<String>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let core = core_of(&ctx).await?;
    let src = core
        .db()
        .get_message(source_message_id)
        .map_err(err)?
        .ok_or_else(|| "source not found".to_string())?;
    let attachments = core.db().list_attachments(source_message_id).map_err(err)?;
    let mut pending: Vec<PendingAttachment> = Vec::with_capacity(attachments.len());
    for a in &attachments {
        let data = core.read_attachment(a).map_err(err)?;
        pending.push(PendingAttachment {
            name: a.name.clone(),
            data,
            from: None,
        });
    }
    if let Some(cid) = contact_id {
        core.send_message(cid, src.body, pending, None, None)
            .await
            .map_err(err)
    } else if let Some(gid_hex) = group_id {
        let gid = parse_group_id(&gid_hex)?;
        core.send_to_group(&gid, src.body, pending, None, None)
            .await
            .map_err(err)
    } else {
        Err("target required".into())
    }
}

#[tauri::command]
pub(crate) async fn send_message(
    contact_id: i64,
    body: String,
    attachments: Vec<serde_json::Value>,
    ttl_secs: Option<u64>,
    reply_to: Option<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
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
        .send_message(contact_id, body, pending, ttl, reply_to)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn send_message_paths(
    contact_id: i64,
    body: String,
    paths: Vec<String>,
    ttl_secs: Option<u64>,
    reply_to: Option<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let core = core_of(&ctx).await?;
    let ttl = ttl_secs.map(std::time::Duration::from_secs);
    if paths.is_empty() {
        return core
            .send_message(contact_id, body, vec![], ttl, reply_to)
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
            .send_message(contact_id, msg_body, vec![pa], ttl, rt)
            .await
            .map_err(err)?;
    }
    Ok(last_id)
}

#[tauri::command]
pub(crate) async fn send_edit(
    contact_id: i64,
    message_id: i64,
    new_body: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .send_edit(contact_id, message_id, new_body)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn send_edit_group(
    group_id: String,
    message_id: i64,
    new_body: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx)
        .await?
        .send_edit_group(&gid, message_id, new_body)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn press_button(
    contact_id: i64,
    message_id: i64,
    callback_data: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .press_button(contact_id, message_id, callback_data)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn press_group_button(
    group_id: String,
    message_id: i64,
    callback_data: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let gid = parse_group_id(&group_id)?;
    core_of(&ctx)
        .await?
        .press_group_button(&gid, message_id, callback_data)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn search_messages(
    query: String,
    contact_id: Option<i64>,
    group_id: Option<String>,
    limit: i64,
    ctx: State<'_, AppCtx>,
) -> Result<Vec<SearchHitDto>, String> {
    let core = core_of(&ctx).await?;
    let db = core.db();
    let gid_bytes = match group_id.as_deref() {
        Some(s) => Some(parse_group_id(s)?),
        None => None,
    };
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }
    let rows = db
        .search_messages(trimmed, contact_id, gid_bytes.as_deref(), limit)
        .map_err(err)?;
    let mut dtos: Vec<MessageDto> = rows.into_iter().map(MessageDto::from).collect();
    attach_extras(db, &mut dtos)?;
    let contacts_by_id: std::collections::HashMap<i64, String> = db
        .list_contacts()
        .map_err(err)?
        .into_iter()
        .map(|c| (c.id, c.display_name))
        .collect();
    let groups_by_hex: std::collections::HashMap<String, String> = db
        .list_groups()
        .map_err(err)?
        .into_iter()
        .map(|g| (hex(&g.id), g.name))
        .collect();
    Ok(dtos
        .into_iter()
        .map(|dto| SearchHitDto {
            contact_id: dto.contact_id,
            group_id: dto.group_id.clone(),
            contact_name: dto
                .contact_id
                .and_then(|cid| contacts_by_id.get(&cid).cloned()),
            group_name: dto
                .group_id
                .as_deref()
                .and_then(|g| groups_by_hex.get(g).cloned()),
            message: dto,
        })
        .collect())
}

#[tauri::command]
pub(crate) async fn list_muted(ctx: State<'_, AppCtx>) -> Result<Vec<String>, String> {
    let core = core_of(&ctx).await?;
    let bytes = core.db().get_setting(SETTING_MUTES).map_err(err)?;
    let list: Vec<String> = bytes
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    Ok(list)
}

#[tauri::command]
pub(crate) async fn set_muted(
    target_key: String,
    muted: bool,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    let bytes = core.db().get_setting(SETTING_MUTES).map_err(err)?;
    let mut list: Vec<String> = bytes
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let already = list.iter().any(|k| k == &target_key);
    if muted && !already {
        list.push(target_key);
    } else if !muted && already {
        list.retain(|k| k != &target_key);
    }
    let bytes = serde_json::to_vec(&list).map_err(err)?;
    core.db().set_setting(SETTING_MUTES, &bytes).map_err(err)?;
    Ok(())
}

/// Interface data kept in the vault: `contact_folders`, `avatars`, `chat_prefs`, `exfiltration_receipts`.
#[tauri::command]
pub(crate) async fn get_ui_data(
    key: String,
    ctx: State<'_, AppCtx>,
) -> Result<Option<String>, String> {
    core_of(&ctx).await?.ui_data(&key).map_err(err)
}

#[tauri::command]
pub(crate) async fn set_ui_data(
    key: String,
    json: String,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx).await?.set_ui_data(&key, &json).map_err(err)
}

/// Clear all history, or honour per-chat retention during automatic clearing
/// when `preserve_saved` is true.
#[tauri::command]
pub(crate) async fn clear_message_history(
    preserve_saved: Option<bool>,
    ctx: State<'_, AppCtx>,
) -> Result<usize, String> {
    let core = core_of(&ctx).await?;
    if preserve_saved == Some(true) {
        core.clear_unsaved_history().map_err(err)
    } else {
        core.clear_message_history().map_err(err)
    }
}

#[tauri::command]
pub(crate) async fn send_typing(
    contact_id: Option<i64>,
    group_id: Option<String>,
    typing: bool,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let core = core_of(&ctx).await?;
    if let Some(cid) = contact_id {
        let _ = core.send_typing_dm(cid, typing).await;
    } else if let Some(gid_hex) = group_id {
        let gid = parse_group_id(&gid_hex)?;
        let _ = core.send_typing_group(&gid, typing).await;
    }
    Ok(())
}
