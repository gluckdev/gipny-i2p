use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::net::SocketAddr;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

use gipny_libcore::crypto::IdentityCard;
use gipny_libcore::db::TrustLevel;
use gipny_libcore::net::I2pNode;
use gipny_libcore::security::{backup_open_3factor, backup_seal_3factor, DuressMode, ServerBind, UnlockOutcome, Vault};

use crate::core::{Core, PendingAttachment};
use crate::{
    attach_extras, err, profile_dir, AttachmentDto,
    ContactDto, GroupDto, GroupMemberDto, MessageDto,
};
use crate::AppCtx;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebEvent {
    pub event: String,
    pub payload: Value,
}

#[derive(Deserialize, Debug)]
struct RpcRequest {
    pub id: Option<u64>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Serialize, Debug)]
struct RpcResponse {
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct WebServerState {
    pub ctx: Arc<AppCtx>,
    pub event_tx: broadcast::Sender<WebEvent>,
}

pub fn emit_web_event(state: &WebServerState, event: &str, payload: Value) {
    let _ = state.event_tx.send(WebEvent {
        event: event.to_string(),
        payload,
    });
}

pub fn emit_boot_status(state: &WebServerState, stage: &'static str, state_str: &'static str, detail: impl Into<String>) {
    emit_web_event(
        state,
        "boot_status",
        json!({
            "stage": stage,
            "state": state_str,
            "detail": detail.into(),
        }),
    );
}

pub async fn run_server(
    ctx: Arc<AppCtx>,
    addr: SocketAddr,
    static_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (event_tx, _) = broadcast::channel(1024);
    let state = Arc::new(WebServerState { ctx, event_tx });

    let index_file = static_dir.join("index.html");
    let serve_dir = ServeDir::new(&static_dir).not_found_service(ServeFile::new(index_file));

    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route(
            "/ws",
            get(|ws: WebSocketUpgrade, State(s): State<Arc<WebServerState>>| async move {
                ws.on_upgrade(|socket| handle_socket(socket, s))
            }),
        )
        .fallback_service(serve_dir)
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("[web] listening on http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_socket(ws: WebSocket, state: Arc<WebServerState>) {
    let (mut sender, mut receiver) = ws.split();
    let mut rx = state.event_tx.subscribe();

    loop {
        tokio::select! {
            broadcast_msg = rx.recv() => {
                match broadcast_msg {
                    Ok(evt) => {
                        let text = serde_json::to_string(&json!({
                            "event": evt.event,
                            "payload": evt.payload,
                        })).unwrap_or_default();
                        if sender.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = receiver.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(req) = serde_json::from_str::<RpcRequest>(&text) {
                            let res = dispatch_rpc(&state, &req.method, req.params).await;
                            let resp = match res {
                                Ok(val) => RpcResponse { id: req.id, result: Some(val), error: None },
                                Err(e) => RpcResponse { id: req.id, result: None, error: Some(e) },
                            };
                            let resp_text = serde_json::to_string(&resp).unwrap_or_default();
                            if sender.send(Message::Text(resp_text.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Ok(Message::Ping(p)) => {
                        let _ = sender.send(Message::Pong(p)).await;
                    }
                    Ok(Message::Close(_)) => break,
                    Err(_) => break,
                    _ => {}
                }
            }
        }
    }
}

async fn core_of(ctx: &AppCtx) -> Result<Arc<Core>, String> {
    ctx.core.lock().await.clone().ok_or_else(|| "locked".to_string())
}

async fn dispatch_rpc(
    state: &Arc<WebServerState>,
    method: &str,
    p: Value,
) -> Result<Value, String> {
    let ctx = &state.ctx;

    match method {
        // --- Profiles & Vault ---
        "list_profiles" => {
            let pdir = ctx.base_dir.join("profiles");
            if !pdir.exists() {
                return Ok(json!(Vec::<String>::new()));
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
            Ok(json!(out))
        }
        "delete_profile" => {
            let profile = p["profile"].as_str().ok_or("missing profile")?;
            let dir = profile_dir(ctx, profile)?;
            if ctx.profile.lock().await.as_deref() == Some(profile) {
                return Err("cannot delete active profile".into());
            }
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(err)?;
            }
            Ok(json!(null))
        }
        "vault_status" => {
            let active_profile = ctx.profile.lock().await.clone();
            let unlocked = ctx.vault.lock().await.is_some();
            let exists = if let Some(ref prof) = active_profile {
                profile_dir(ctx, prof).map(|d| Vault::exists(&d)).unwrap_or(false)
            } else {
                let pdir = ctx.base_dir.join("profiles");
                pdir.exists() && std::fs::read_dir(&pdir).map(|mut it| it.next().is_some()).unwrap_or(false)
            };
            Ok(json!({
                "exists": exists,
                "unlocked": unlocked,
                "profile": active_profile
            }))
        }
        "vault_create" => {
            let profile = p["profile"].as_str().ok_or("missing profile")?;
            let pass = p["passphrase"].as_str().ok_or("missing passphrase")?;
            let decoy_pass = p["decoyPassphrase"].as_str();
            let duress_str = p["duressMode"].as_str().unwrap_or("wipe");
            let duress = match duress_str {
                "decoy" => DuressMode::Decoy,
                _ => DuressMode::Wipe,
            };
            let max_attempts = p["maxAttempts"].as_u64().unwrap_or(0) as u32;

            let dir = profile_dir(ctx, profile)?;
            if dir.exists() && Vault::exists(&dir) {
                return Err("profile already exists".into());
            }
            std::fs::create_dir_all(&dir).map_err(err)?;
            let vault = Arc::new(Vault::create(&dir, pass, decoy_pass, duress, max_attempts).map_err(err)?);
            boot_core_web(state, vault, pass, profile, &dir).await?;
            Ok(json!(null))
        }
        "vault_unlock" => {
            let profile = p["profile"].as_str().ok_or("missing profile")?;
            let pass = p["passphrase"].as_str().ok_or("missing passphrase")?;
            let dir = profile_dir(ctx, profile)?;

            let prev = ctx.core.lock().await.take();
            if let Some(c) = prev {
                c.shutdown();
                drop(c);
                ctx.vault.lock().await.take();
                ctx.profile.lock().await.take();
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            }

            let vault = Arc::new(Vault::open(&dir).map_err(err)?);
            boot_core_web(state, vault, pass, profile, &dir).await?;
            Ok(json!(null))
        }
        "vault_lock" => {
            let prev = ctx.core.lock().await.take();
            if let Some(c) = prev {
                c.shutdown();
            }
            ctx.vault.lock().await.take();
            ctx.profile.lock().await.take();
            emit_web_event(state, "vault_locked", json!({}));
            Ok(json!(null))
        }
        "verify_passphrase" => {
            let pass = p["passphrase"].as_str().ok_or("missing passphrase")?;
            let vault = ctx.vault.lock().await.clone().ok_or("no profile open")?;
            match vault.unlock(pass).map_err(err)? {
                UnlockOutcome::Primary(_) => Ok(json!("ok")),
                _ => Err("wrong passphrase".into()),
            }
        }
        "change_passphrase" => {
            let old_p = p["oldPassphrase"].as_str().ok_or("missing oldPassphrase")?;
            let new_p = p["newPassphrase"].as_str().ok_or("missing newPassphrase")?;
            let vault = ctx.vault.lock().await.clone().ok_or("no profile open")?;
            vault.change_passphrase(old_p, new_p).map_err(err)?;
            Ok(json!(null))
        }
        "prewarm_network" => {
            Ok(json!(null))
        }
        "prewarm_status" => {
            Ok(json!("ready"))
        }

        // --- Identity ---
        "my_card" => {
            let core = core_of(ctx).await?;
            let card = core.my_card();
            Ok(json!({
                "sign_pk": crate::hex(&card.sign_pk),
                "dh_pk": crate::hex(&card.dh_pk),
            }))
        }
        "my_onion" => {
            let core = core_of(ctx).await?;
            Ok(json!(core.my_onion().to_string()))
        }
        "my_b32" => {
            let core = core_of(ctx).await?;
            Ok(json!(core.my_b32()))
        }
        "my_fingerprint" => {
            let core = core_of(ctx).await?;
            Ok(json!(crate::hex(&core.my_fingerprint())))
        }
        "get_display_name" => {
            let core = core_of(ctx).await?;
            Ok(json!(core.display_name().map_err(err)?))
        }
        "set_display_name" => {
            let name = p["name"].as_str().ok_or("missing name")?;
            let core = core_of(ctx).await?;
            core.db().set_setting("display_name", name.as_bytes()).map_err(err)?;
            Ok(json!(null))
        }

        // --- Contacts ---
        "list_contacts" => {
            let core = core_of(ctx).await?;
            let list = core.db().list_contacts().map_err(err)?;
            let dtos: Vec<ContactDto> = list.into_iter().filter(|c| core.wipe_pending_since(c.id).is_none()).map(ContactDto::from).collect();
            Ok(json!(dtos))
        }
        "get_contact" => {
            let id = p["id"].as_i64().ok_or("missing id")?;
            let core = core_of(ctx).await?;
            let c = core.db().get_contact(id).map_err(err)?;
            Ok(json!(c.map(ContactDto::from)))
        }
        "add_contact" => {
            let onion = p["onion"].as_str().ok_or("missing onion")?;
            let sign_pk = p["signPk"].as_str().or_else(|| p["sign_pk"].as_str()).ok_or("missing signPk")?;
            let dh_pk = p["dhPk"].as_str().or_else(|| p["dh_pk"].as_str()).ok_or("missing dhPk")?;
            let name = p["name"].as_str().unwrap_or("Contact");
            let relay = p["relay"].as_str().map(str::trim).filter(|r| !r.is_empty());
            let sign = crate::parse_hex32(sign_pk)?;
            let dh = crate::parse_hex32(dh_pk)?;
            let card = IdentityCard { sign_pk: sign, dh_pk: dh };
            let core = core_of(ctx).await?;
            let id = core.add_contact_via(&card, onion, name, relay).await.map_err(err)?;
            Ok(json!(id))
        }
        "update_contact" => {
            let id = p["id"].as_i64().ok_or("missing id")?;
            let name = p["name"].as_str().ok_or("missing name")?;
            let trust = p["trust"].as_u64().unwrap_or(0) as u8;
            let t = match trust { 1 => TrustLevel::Verified, 2 => TrustLevel::Blocked, _ => TrustLevel::Unverified };
            core_of(ctx).await?.update_contact(id, name, t).await.map_err(err)?;
            Ok(json!(null))
        }
        "delete_contact" => {
            let id = p["id"].as_i64().ok_or("missing id")?;
            let for_both = p["forBoth"].as_bool().or_else(|| p["for_both"].as_bool());
            let core = core_of(ctx).await?;
            if for_both == Some(true) {
                core.delete_contact_for_both(id).await.map_err(err)?;
            } else {
                core.delete_contact(id).await.map_err(err)?;
            }
            Ok(json!(null))
        }
        "accept_contact_request" => {
            let id = p["id"].as_i64().ok_or("missing id")?;
            core_of(ctx).await?.accept_contact_request(id).await.map_err(err)?;
            Ok(json!(null))
        }
        "decline_contact_request" => {
            let id = p["id"].as_i64().ok_or("missing id")?;
            core_of(ctx).await?.decline_contact_request(id).await.map_err(err)?;
            Ok(json!(null))
        }
        "set_contact_bot" => {
            let id = p["id"].as_i64().ok_or("missing id")?;
            let is_bot = p["isBot"].as_bool().unwrap_or(false);
            core_of(ctx).await?.db().set_contact_is_bot(id, is_bot).map_err(err)?;
            Ok(json!(null))
        }
        "reset_contact_session" => {
            let id = p["id"].as_i64().ok_or("missing id")?;
            core_of(ctx).await?.reset_contact_session(id).await.map_err(err)?;
            Ok(json!(null))
        }
        "list_unreachable_contacts" => {
            let core = core_of(ctx).await?;
            Ok(json!(core.unreachable_contacts().await))
        }

        // --- Messages ---
        "list_messages" => {
            let contact_id = p["contactId"].as_i64().ok_or("missing contactId")?;
            let limit = p["limit"].as_i64().unwrap_or(100);
            let before_id = p["beforeId"].as_i64();
            let core = core_of(ctx).await?;
            let msgs = core.db().list_messages(contact_id, limit, before_id).map_err(err)?;
            let mut dtos: Vec<MessageDto> = msgs.into_iter().map(MessageDto::from).collect();
            attach_extras(core.db(), &mut dtos)?;
            Ok(json!(dtos))
        }
        "unread_count" => {
            let contact_id = p["contactId"].as_i64().ok_or("missing contactId")?;
            let count = core_of(ctx).await?.db().unread_count(contact_id).map_err(err)?;
            Ok(json!(count))
        }
        "mark_read" => {
            let contact_id = p["contactId"].as_i64().ok_or("missing contactId")?;
            core_of(ctx).await?.db().mark_read(contact_id).map_err(err)?;
            Ok(json!(null))
        }
        "delete_message" => {
            let id = p["id"].as_i64().ok_or("missing id")?;
            core_of(ctx).await?.db().delete_message(id).map_err(err)?;
            Ok(json!(null))
        }
        "send_message" => {
            let contact_id = p["contactId"].as_i64().ok_or("missing contactId")?;
            let body = p["body"].as_str().unwrap_or("").to_string();
            let ttl = p["ttlSecs"].as_u64().map(std::time::Duration::from_secs);
            let reply_to = p["replyTo"].as_i64();
            let mut pending = Vec::new();
            if let Some(atts) = p["attachments"].as_array() {
                for a in atts {
                    let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("file").to_string();
                    let data_b64 = a.get("data").and_then(|v| v.as_str()).ok_or("bad attachment")?;
                    let data = crate::base64_decode(data_b64).ok_or("bad base64")?;
                    pending.push(PendingAttachment { name, data, from: None });
                }
            }
            let id = core_of(ctx).await?.send_message(contact_id, body, pending, ttl, reply_to).await.map_err(err)?;
            Ok(json!(id))
        }
        "send_edit" => {
            let contact_id = p["contactId"].as_i64().ok_or("missing contactId")?;
            let message_id = p["messageId"].as_i64().ok_or("missing messageId")?;
            let new_body = p["newBody"].as_str().ok_or("missing newBody")?.to_string();
            core_of(ctx).await?.send_edit(contact_id, message_id, new_body).await.map_err(err)?;
            Ok(json!(null))
        }
        "send_edit_group" => {
            let group_id = p["groupId"].as_str().ok_or("missing groupId")?;
            let message_id = p["messageId"].as_i64().ok_or("missing messageId")?;
            let new_body = p["newBody"].as_str().ok_or("missing newBody")?.to_string();
            let gid = crate::parse_group_id(group_id)?;
            core_of(ctx).await?.send_edit_group(&gid, message_id, new_body).await.map_err(err)?;
            Ok(json!(null))
        }
        "press_button" => {
            let contact_id = p["contactId"].as_i64().ok_or("missing contactId")?;
            let msg_id = p["messageId"].as_i64().ok_or("missing messageId")?;
            let data = p["callbackData"].as_str().ok_or("missing callbackData")?.to_string();
            core_of(ctx).await?.press_button(contact_id, msg_id, data).await.map_err(err)?;
            Ok(json!(null))
        }
        "press_group_button" => {
            let group_id = p["groupId"].as_str().ok_or("missing groupId")?;
            let msg_id = p["messageId"].as_i64().ok_or("missing messageId")?;
            let data = p["callbackData"].as_str().ok_or("missing callbackData")?.to_string();
            let gid = crate::parse_group_id(group_id)?;
            core_of(ctx).await?.press_group_button(&gid, msg_id, data).await.map_err(err)?;
            Ok(json!(null))
        }
        "send_typing" => {
            let contact_id = p["contactId"].as_i64();
            let group_id = p["groupId"].as_str();
            let typing = p["typing"].as_bool().unwrap_or(false);
            let core = core_of(ctx).await?;
            if let Some(cid) = contact_id {
                let _ = core.send_typing_dm(cid, typing).await;
            } else if let Some(gid_hex) = group_id {
                let gid = crate::parse_group_id(gid_hex)?;
                let _ = core.send_typing_group(&gid, typing).await;
            }
            Ok(json!(null))
        }
        "list_attachments" => {
            let msg_id = p["messageId"].as_i64().ok_or("missing messageId")?;
            let list = core_of(ctx).await?.db().list_attachments(msg_id).map_err(err)?;
            let dtos: Vec<AttachmentDto> = list.into_iter().map(|a| AttachmentDto {
                id: a.id,
                message_id: a.message_id,
                name: a.name,
                size: a.size,
            }).collect();
            Ok(json!(dtos))
        }
        "load_attachment" => {
            let id = p["attachmentId"].as_i64().ok_or("missing attachmentId")?;
            let core = core_of(ctx).await?;
            let att = core.db().get_attachment(id).map_err(err)?.ok_or("not found")?;
            let bytes = core.read_attachment(&att).map_err(err)?;
            Ok(json!(crate::base64_encode(&bytes)))
        }

        // --- Groups ---
        "list_groups" => {
            let groups = core_of(ctx).await?.db().list_groups().map_err(err)?;
            let dtos: Vec<GroupDto> = groups.into_iter().map(GroupDto::from).collect();
            Ok(json!(dtos))
        }
        "create_group" => {
            let name = p["name"].as_str().ok_or("missing name")?;
            let member_ids: Vec<i64> = serde_json::from_value(p["memberContactIds"].clone()).unwrap_or_default();
            let gid = core_of(ctx).await?.create_group(name, &member_ids).await.map_err(err)?;
            Ok(json!(crate::hex(&gid)))
        }
        "list_group_members" => {
            let gid_str = p["groupId"].as_str().ok_or("missing groupId")?;
            let gid = crate::parse_group_id(gid_str)?;
            let members = core_of(ctx).await?.db().list_group_members(&gid).map_err(err)?;
            let dtos: Vec<GroupMemberDto> = members.into_iter().map(GroupMemberDto::from).collect();
            Ok(json!(dtos))
        }
        "add_group_member" => {
            let gid_str = p["groupId"].as_str().ok_or("missing groupId")?;
            let gid = crate::parse_group_id(gid_str)?;
            let cid = p["contactId"].as_i64().ok_or("missing contactId")?;
            core_of(ctx).await?.add_group_member(&gid, cid).await.map_err(err)?;
            Ok(json!(null))
        }
        "list_group_messages" => {
            let gid_str = p["groupId"].as_str().ok_or("missing groupId")?;
            let gid = crate::parse_group_id(gid_str)?;
            let limit = p["limit"].as_i64().unwrap_or(100);
            let before_id = p["beforeId"].as_i64();
            let core = core_of(ctx).await?;
            let msgs = core.db().list_group_messages(&gid, limit, before_id).map_err(err)?;
            let mut dtos: Vec<MessageDto> = msgs.into_iter().map(MessageDto::from).collect();
            attach_extras(core.db(), &mut dtos)?;
            Ok(json!(dtos))
        }
        "send_group_message" => {
            let gid_str = p["groupId"].as_str().ok_or("missing groupId")?;
            let gid = crate::parse_group_id(gid_str)?;
            let body = p["body"].as_str().unwrap_or("").to_string();
            let ttl = p["ttlSecs"].as_u64().map(std::time::Duration::from_secs);
            let reply_to = p["replyTo"].as_i64();
            let mut pending = Vec::new();
            if let Some(atts) = p["attachments"].as_array() {
                for a in atts {
                    let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("file").to_string();
                    let data_b64 = a.get("data").and_then(|v| v.as_str()).ok_or("bad attachment")?;
                    let data = crate::base64_decode(data_b64).ok_or("bad base64")?;
                    pending.push(PendingAttachment { name, data, from: None });
                }
            }
            let id = core_of(ctx).await?.send_to_group(&gid, body, pending, ttl, reply_to).await.map_err(err)?;
            Ok(json!(id))
        }
        "delete_group" => {
            let gid_str = p["groupId"].as_str().ok_or("missing groupId")?;
            let gid = crate::parse_group_id(gid_str)?;
            core_of(ctx).await?.db().delete_group(&gid).map_err(err)?;
            Ok(json!(null))
        }
        "mark_group_read" => {
            let gid_str = p["groupId"].as_str().ok_or("missing groupId")?;
            let gid = crate::parse_group_id(gid_str)?;
            core_of(ctx).await?.db().mark_group_read(&gid).map_err(err)?;
            Ok(json!(null))
        }
        "group_unread_count" => {
            let gid_str = p["groupId"].as_str().ok_or("missing groupId")?;
            let gid = crate::parse_group_id(gid_str)?;
            let count = core_of(ctx).await?.db().group_unread_count(&gid).map_err(err)?;
            Ok(json!(count))
        }

        // --- Relay & Router settings ---
        "get_relay_address" => {
            Ok(json!(core_of(ctx).await?.get_relay_address()))
        }
        "set_relay_address" => {
            let addr = p["addr"].as_str().ok_or("missing addr")?;
            core_of(ctx).await?.set_relay_address(addr).map_err(err)?;
            Ok(json!(null))
        }
        "get_relay_info" => {
            let core = core_of(ctx).await?;
            let info = core.relay_info();
            Ok(json!(info))
        }
        "set_relay_mode" => {
            let mode_str = p["mode"].as_str().ok_or("missing mode")?;
            let mode = crate::core::RelayMode::parse(mode_str).ok_or("bad relay mode")?;
            core_of(ctx).await?.set_relay_mode(mode).await.map_err(err)?;
            Ok(json!(null))
        }
        "get_dht_status" => {
            let core = core_of(ctx).await?;
            let s = core.dht_status();
            Ok(json!(s))
        }
        "set_lane" => {
            let lane_str = p["lane"].as_str().ok_or("missing lane")?;
            let lane = match lane_str {
                "fast" => crate::core::Lane::Fast,
                _ => crate::core::Lane::Normal,
            };
            core_of(ctx).await?.set_lane(lane).await.map_err(err)?;
            Ok(json!(null))
        }
        "get_ui_data" => {
            let key = p["key"].as_str().ok_or("missing key")?;
            let val = core_of(ctx).await?.ui_data(key).map_err(err)?;
            Ok(json!(val))
        }
        "set_ui_data" => {
            let key = p["key"].as_str().ok_or("missing key")?;
            let val = p["val"].as_str().ok_or("missing val")?;
            core_of(ctx).await?.set_ui_data(key, val).map_err(err)?;
            Ok(json!(null))
        }

        // --- 3-Factor Backup ---
        "export_backup_3factor" => {
            let pass = p["passphrase"].as_str().ok_or("missing passphrase")?;
            let client_sec_hex = p["clientSecret"].as_str().ok_or("missing clientSecret")?;
            let client_secret = crate::parse_hex32(client_sec_hex)?;
            let server_key = ServerBind::ensure(&ctx.base_dir).map_err(err)?;

            let core = core_of(ctx).await?;
            let backup_blob = crate::export_raw_backup_data(core.db()).map_err(err)?;
            let sealed = backup_seal_3factor(pass, &server_key, &client_secret, &backup_blob).map_err(err)?;
            Ok(json!(crate::base64_encode(&sealed)))
        }
        "import_backup_3factor" => {
            let profile = p["profile"].as_str().ok_or("missing profile")?;
            let pass = p["passphrase"].as_str().ok_or("missing passphrase")?;
            let client_sec_hex = p["clientSecret"].as_str().ok_or("missing clientSecret")?;
            let client_secret = crate::parse_hex32(client_sec_hex)?;
            let server_key = ServerBind::ensure(&ctx.base_dir).map_err(err)?;

            let blob_b64 = p["blob"].as_str().ok_or("missing blob")?;
            let blob = crate::base64_decode(blob_b64).ok_or("bad base64")?;
            let plain = backup_open_3factor(pass, &server_key, &client_secret, &blob)
                .map_err(|_| "Decryption failed: check password, server key, and client token".to_string())?;

            crate::restore_raw_backup_data(ctx, profile.to_string(), pass.to_string(), &plain).await?;
            Ok(json!({ "ok": true }))
        }

        // --- Other UI Utilities ---
        "qr_svg" => {
            let text = p["text"].as_str().ok_or("missing text")?;
            let code = qrcode::QrCode::new(text.as_bytes()).map_err(err)?;
            let svg = code.render::<qrcode::render::svg::Color>().build();
            Ok(json!(svg))
        }
        "current_version" => {
            Ok(json!("0.4.18-web"))
        }

        _ => Err(format!("unknown RPC method: {method}")),
    }
}

async fn boot_core_web(
    state: &Arc<WebServerState>,
    vault: Arc<Vault>,
    pass: &str,
    profile: &str,
    dir: &Path,
) -> Result<(), String> {
    emit_boot_status(state, "vault", "active", "unlocking the vault (argon2id)");
    let outcome = vault.unlock(pass).map_err(|e| {
        emit_boot_status(state, "vault", "failed", format!("{e}"));
        err(e)
    })?;
    let (mk, db_name) = match outcome {
        UnlockOutcome::Primary(k) => (k, "data.db"),
        UnlockOutcome::Decoy(k) => (k, "decoy.db"),
        UnlockOutcome::Wiped => {
            emit_boot_status(state, "vault", "failed", "vault wiped");
            return Err("wiped".into());
        }
    };
    let db = match gipny_libcore::db::Db::open(&dir.join(db_name), &mk) {
        Ok(db) => Arc::new(db),
        Err(e) => {
            let msg = err(e);
            emit_boot_status(state, "vault", "failed", format!("db open error: {msg}"));
            return Err(msg);
        }
    };
    emit_boot_status(state, "vault", "done", format!("profile opened ({db_name})"));

    // Server-side settings: 1 hop on server, zero transit
    std::env::set_var("GIPNY_SERVER_HOPS", "1");
    let settings = gipny_libcore::router::RouterSettings {
        transit: gipny_libcore::router::TransitProfile::Zero,
        yggdrasil: gipny_libcore::router::Yggdrasil::Off,
    };

    emit_boot_status(state, "router", "active", "starting 1-hop embedded router");
    let state_progress = state.clone();
    let progress_cb: gipny_libcore::router::BootProgress = Arc::new(move |stage: &str, detail: &str| {
        let (stage_name, state_val) = match stage {
            "router" => ("router", "active"),
            "router-reused" => ("router", "done"),
            "tunnels" => ("tunnels", "active"),
            "tunnels-done" => ("tunnels", "done"),
            "session" => ("session", "active"),
            "session-done" => ("session", "done"),
            _ => ("core", "active"),
        };
        emit_boot_status(&state_progress, stage_name, state_val, detail);
    });

    let node = Arc::new(I2pNode::start_with_progress(dir, settings, Some(progress_cb)).await.map_err(err)?);
    emit_boot_status(state, "core", "active", "starting core engine");
    let (core, mut events) = Core::start(dir.to_path_buf(), db, node, None).await.map_err(|e| {
        emit_boot_status(state, "core", "failed", format!("{e:?}"));
        err(e)
    })?;
    emit_boot_status(state, "core", "done", "core running");

    let state_events = state.clone();
    tokio::spawn(async move {
        while let Some(e) = events.recv().await {
            match &e {
                crate::core::CoreEvent::RelayInfoChanged { info } => match &info.hosted {
                    crate::core::HostedRelayState::Ready { address } => {
                        emit_boot_status(&state_events, "relay", "done", format!("built-in relay ready at {}", &address[..address.len().min(16)]));
                        emit_boot_status(&state_events, "dht", "active", "looking for the relay network");
                    }
                    crate::core::HostedRelayState::Failed { reason } => {
                        emit_boot_status(&state_events, "relay", "failed", reason.clone());
                    }
                    crate::core::HostedRelayState::Starting => {
                        emit_boot_status(&state_events, "relay", "active", "building the relay's tunnels");
                    }
                    crate::core::HostedRelayState::Off => {
                        emit_boot_status(&state_events, "relay", "skipped", "not our relay to raise — an external one is configured");
                        emit_boot_status(&state_events, "dht", "skipped", "no relay of ours to announce — the external one handles it");
                    }
                },
                crate::core::CoreEvent::DhtJoined { peers, reached } => {
                    let (state_val, detail) = if *reached {
                        ("done", format!("relay network: {peers} node(s) known"))
                    } else {
                        ("skipped", "no relay node answered yet — we look again on the next tick".to_string())
                    };
                    emit_boot_status(&state_events, "dht", state_val, detail);
                }
                _ => {}
            }
            emit_web_event(&state_events, "core_event", serde_json::to_value(&e).unwrap_or_default());
        }
    });

    *state.ctx.profile.lock().await = Some(profile.to_string());
    *state.ctx.vault.lock().await = Some(vault);
    *state.ctx.core.lock().await = Some(core);

    Ok(())
}
