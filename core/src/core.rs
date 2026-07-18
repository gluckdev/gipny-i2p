use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::{rngs::OsRng, RngCore};
use serde::Serialize;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use gipny_libcore::crypto::{
    self, AttachmentCipher, Identity, IdentityCard, PreKeyBundle, PreKeyPair, RatchetState,
};
use gipny_libcore::db::{Attachment, Db, Direction, GroupMember, NewAttachment, PreKeyKind, TrustLevel};
use gipny_libcore::net::{NetError, TorNode};
use gipny_libcore::relay::{self, ClientToRelay, EnvelopeBlob, RelayClient, RelayToClient, DEFAULT_RELAY};
use gipny_libcore::update::{UpdateError, UpdateInfo, Updater};

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("db")] Db(#[from] gipny_libcore::db::DbError),
    #[error("crypto")] Crypto(#[from] gipny_libcore::crypto::CryptoError),
    #[error("net")] Net(#[from] NetError),
    #[error("relay")] Relay(#[from] gipny_libcore::relay::RelayError),
    #[error("io")] Io(#[from] std::io::Error),
    #[error("update")] Update(#[from] UpdateError),
    #[error("codec")] Codec,
    #[error("not found")] NotFound,
    #[error("state")] State,
    #[error("stale opk")] StaleOpk,
    #[error("sealed drop")] SealedDrop,
}

impl From<bincode::Error> for CoreError { fn from(_: bincode::Error) -> Self { Self::Codec } }

const SETTING_IDENTITY_SIGN: &str = "identity_sign";
const SETTING_IDENTITY_DH: &str = "identity_dh";
const SETTING_SIGNED_PREKEY_ID: &str = "signed_prekey_id";
const SETTING_RELAY_ONION: &str = "relay_onion";
const SETTING_DISMISSED_UPDATE: &str = "dismissed_update_version";
const ATTACHMENTS_DIR: &str = "attachments";
const TARGET_OPK: usize = 20;
const PURGE_INTERVAL_SECS: u64 = 60;
const RECONNECT_INITIAL_MS: u64 = 500;
const RECONNECT_MAX_MS: u64 = 15_000;
const PING_INTERVAL_SECS: u64 = 20;
const DEAD_THRESHOLD_SECS: u64 = 75;
const BUNDLE_REFRESH_SECS: u64 = 12 * 3600;
const UPDATE_CHECK_INITIAL_SECS: u64 = 30;
const UPDATE_CHECK_INTERVAL_SECS: u64 = 6 * 3600;
const EVENTS_CAPACITY: usize = 1024;
const PENDING_REQ_TIMEOUT_MS: u64 = 30_000;
const MAX_PAYLOAD_BYTES: usize = 14 * 1024 * 1024;
const RETRY_BASE_BACKOFF_MS: i64 = 5_000;
const RETRY_MAX_BACKOFF_MS: i64 = 300_000;
const FRESH_SESSION_GRACE_MS: i64 = 60_000;
const TIEBREAKER_TIMEOUT_MS: i64 = 10_000;
const KEEPALIVE_INCOMING_THRESHOLD: u32 = 100;

#[derive(Debug, Clone, Serialize)]
pub enum CoreEvent {
    IncomingMessage {
        contact_id: Option<i64>,
        group_id: Option<String>,
        sender_sign_pk: Option<String>,
        message_id: i64,
        body: String,
        sent_at: i64,
        notify_sound: Option<String>,
    },
    MessageEdited {
        message_id: i64,
        body: String,
        buttons: Option<Vec<Vec<WireButton>>>,
    },
    MessagePinned {
        contact_id: Option<i64>,
        group_id: Option<String>,
        message_id: i64,
    },
    MessageUnpinned {
        contact_id: Option<i64>,
        group_id: Option<String>,
        message_id: i64,
    },
    MessageSent { message_id: i64 },
    MessageDelivered { message_id: i64 },
    Typing {
        contact_id: Option<i64>,
        group_id: Option<String>,
        sender_sign_pk: Option<String>,
        typing: bool,
    },
    RelayConnected,
    RelayDisconnected,
    ContactAdded { contact_id: i64 },
    ContactUpdated { contact_id: i64 },
    GroupUpdated { group_id: String },
    UpdateAvailable { version: String, notes: String, target_key: String, size: u64 },
    UpdateProgress { downloaded: u64, total: u64, pct: u8 },
    UpdateReady { path: String },
    UpdateFailed { reason: String },
}

use gipny_libcore::{WirePayload, WireAttachment, WireButton, WireGroupRef, WireMember};

fn hex_bytes(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b { s.push_str(&format!("{:02x}", x)); }
    s
}
use gipny_libcore::session::{WirePin, WireReply, encode_payload, decode_payload, pad_payload, unpad_payload};

fn decode_with_padding_fallback(pt: &[u8]) -> std::result::Result<WirePayload, bincode::Error> {
    if let Some(unpadded) = unpad_payload(pt) {
        if let Ok(p) = decode_payload(&unpadded) { return Ok(p); }
    }
    decode_payload(pt)
}

pub struct PendingAttachment { pub name: String, pub data: Vec<u8> }

type BundleWaiter = tokio::sync::oneshot::Sender<Option<Vec<u8>>>;

pub struct Core {
    db: Arc<Db>,
    node: Arc<TorNode>,
    identity: Arc<Identity>,
    sessions: Arc<Mutex<HashMap<i64, RatchetState>>>,
    events: mpsc::Sender<CoreEvent>,
    data_dir: PathBuf,
    relay_out: Arc<RwLock<Option<mpsc::Sender<ClientToRelay>>>>,
    bundle_waiters: Arc<Mutex<HashMap<[u8; 32], Vec<BundleWaiter>>>>,
    send_kick: Arc<tokio::sync::Notify>,
    tiebreaker_waits: Arc<Mutex<HashMap<i64, i64>>>,
    session_created_at: Arc<Mutex<HashMap<i64, i64>>>,
    incoming_since_send: Arc<Mutex<HashMap<i64, u32>>>,
    updater: Arc<Updater>,
    pending_update: Arc<Mutex<Option<UpdateInfo>>>,
    tasks: Arc<std::sync::Mutex<Vec<JoinHandle<()>>>>,
}

impl Core {
    pub async fn start(
        data_dir: PathBuf,
        db: Arc<Db>,
        node: Arc<TorNode>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<CoreEvent>)> {
        std::fs::create_dir_all(data_dir.join(ATTACHMENTS_DIR))?;
        let identity = Arc::new(Self::load_or_create_identity(&db)?);
        let (events_tx, events_rx) = mpsc::channel(EVENTS_CAPACITY);
        let updater = Arc::new(Updater::new(node.clone()));
        let core = Arc::new(Self {
            db: db.clone(),
            node: node.clone(),
            identity,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            events: events_tx,
            data_dir,
            relay_out: Arc::new(RwLock::new(None)),
            bundle_waiters: Arc::new(Mutex::new(HashMap::new())),
            send_kick: Arc::new(tokio::sync::Notify::new()),
            tiebreaker_waits: Arc::new(Mutex::new(HashMap::new())),
            session_created_at: Arc::new(Mutex::new(HashMap::new())),
            incoming_since_send: Arc::new(Mutex::new(HashMap::new())),
            updater,
            pending_update: Arc::new(Mutex::new(None)),
            tasks: Arc::new(std::sync::Mutex::new(Vec::new())),
        });
        core.ensure_prekeys().await?;
        let _ = core.db.cleanup_orphan_pins();
        core.clone().spawn_relay_loop();
        core.clone().spawn_send_loop();
        core.clone().spawn_purge_loop();
        core.clone().spawn_update_loop();
        Ok((core, events_rx))
    }

    pub fn shutdown(&self) {
        let mut v = self.tasks.lock().unwrap();
        for h in v.drain(..) { h.abort(); }
    }

    fn relay_onion(&self) -> String {
        self.db.get_setting(SETTING_RELAY_ONION).ok().flatten()
            .and_then(|v| String::from_utf8(v).ok())
            .unwrap_or_else(|| DEFAULT_RELAY.to_string())
    }

    fn load_or_create_identity(db: &Db) -> Result<Identity> {
        match (db.get_setting(SETTING_IDENTITY_SIGN)?, db.get_setting(SETTING_IDENTITY_DH)?) {
            (Some(s), Some(d)) if s.len() == 32 && d.len() == 32 => {
                let mut sign = [0u8; 32]; sign.copy_from_slice(&s);
                let mut dh = [0u8; 32]; dh.copy_from_slice(&d);
                Ok(Identity::from_bytes(sign, dh))
            }
            _ => {
                let id = Identity::generate();
                db.set_setting(SETTING_IDENTITY_SIGN, id.sign_seed())?;
                db.set_setting(SETTING_IDENTITY_DH, id.dh_secret())?;
                Ok(id)
            }
        }
    }

    pub fn my_card(&self) -> IdentityCard { self.identity.card() }
    pub fn my_fingerprint(&self) -> [u8; 32] { self.identity.fingerprint() }
    pub fn db(&self) -> &Arc<Db> { &self.db }
    pub fn my_onion(&self) -> &str { self.node.onion_address() }
    pub fn my_b32(&self) -> String { self.node.b32_address().unwrap_or_default() }

    pub fn get_relay_address(&self) -> String { self.relay_onion() }
    pub fn set_relay_address(&self, addr: &str) -> Result<()> {
        self.db.set_setting(SETTING_RELAY_ONION, addr.trim().as_bytes())?;
        Ok(())
    }

    pub fn display_name(&self) -> Result<String> {
        let v = self.db.get_setting("display_name")?;
        Ok(v.and_then(|b| String::from_utf8(b).ok()).unwrap_or_default())
    }

    fn outgoing_sender_name(&self) -> Option<String> {
        let user = self.db.get_setting("display_name").ok().flatten()
            .and_then(|b| String::from_utf8(b).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Some(user.unwrap_or_else(|| hex_short(&self.identity.card().sign_pk)))
    }

    fn apply_peer_name(&self, contact_id: i64, name: &str) {
        let contact_changed = self.db.update_contact_name(contact_id, name).unwrap_or(false);
        if contact_changed {
            let _ = self.events.try_send(CoreEvent::ContactUpdated { contact_id });
        }
        if let Ok(Some(c)) = self.db.get_contact(contact_id) {
            if let Ok(groups) = self.db.list_groups_with_member(&c.identity_sign) {
                for gid in groups {
                    if self.db.update_group_member_name(&gid, &c.identity_sign, name).unwrap_or(false) {
                        let _ = self.events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(&gid) });
                    }
                }
            }
        }
    }

    async fn ensure_prekeys(&self) -> Result<()> {
        let id = self.db.get_setting(SETTING_SIGNED_PREKEY_ID)?;
        if id.is_none() {
            let pair = PreKeyPair::generate();
            let pid = self.db.add_prekey(PreKeyKind::Signed, pair.secret(), pair.public())?;
            self.db.set_setting(SETTING_SIGNED_PREKEY_ID, &pid.to_be_bytes())?;
        }
        let count = self.db.count_prekeys(PreKeyKind::OneTime)?;
        for _ in count..(TARGET_OPK as i64) {
            let pair = PreKeyPair::generate();
            self.db.add_prekey(PreKeyKind::OneTime, pair.secret(), pair.public())?;
        }
        Ok(())
    }

    pub fn my_bundle(&self) -> Result<PreKeyBundle> {
        let signed_id = i64::from_be_bytes(
            self.db.get_setting(SETTING_SIGNED_PREKEY_ID)?.ok_or(CoreError::State)?
                .try_into().map_err(|_| CoreError::State)?);
        let signed = self.db.get_prekey(signed_id)?.ok_or(CoreError::State)?;
        let signed_pair = PreKeyPair::from_secret(to_arr32(signed.private.clone())?);
        let opk = self.db.peek_oldest_prekey(PreKeyKind::OneTime)?;
        let opk_pair = opk.as_ref().map(|p| {
            let sk = to_arr32(p.private.clone()).expect("prekey private size");
            (p.id, PreKeyPair::from_secret(sk))
        });
        let bundle = PreKeyBundle::new(
            &self.identity,
            &signed_pair,
            opk_pair.as_ref().map(|(id, kp)| (*id, kp)),
        );
        Ok(bundle)
    }

    pub async fn add_contact(&self, card: &gipny_libcore::crypto::IdentityCard, onion: &str, name: &str) -> Result<i64> {
        let id = self.db.add_contact(&card.sign_pk, &card.dh_pk, onion, name)?;
        let _ = self.events.try_send(CoreEvent::ContactAdded { contact_id: id });
        self.send_kick.notify_one();
        Ok(id)
    }

    pub async fn send_message(
        &self,
        contact_id: i64,
        body: String,
        attachments: Vec<PendingAttachment>,
        ttl: Option<Duration>,
        reply_to: Option<i64>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let expires_at = ttl.map(|d| sent_at + d.as_millis() as i64);
        let mut stored = Vec::with_capacity(attachments.len());
        for a in &attachments {
            let (key, path, size) = self.store_attachment(&a.data)?;
            stored.push(NewAttachment {
                name: a.name.clone(), size: size as i64, key: key.to_vec(), path,
            });
        }
        let msg_id = self.db.insert_message(
            contact_id, Direction::Out, &body, sent_at, expires_at, &stored,
        )?;
        if let Some(rt) = reply_to {
            self.db.set_reply_to(msg_id, Some(rt))?;
        }
        self.send_kick.notify_one();
        Ok(msg_id)
    }

    fn build_wire_reply(&self, local_id: i64) -> Result<Option<WireReply>> {
        let m = match self.db.get_message(local_id)? {
            Some(m) => m,
            None => return Ok(None),
        };
        match m.direction {
            Direction::Out => Ok(Some(WireReply {
                sender_sign_pk: self.identity.card().sign_pk.to_vec(),
                origin_msg_id: m.id as u64,
            })),
            Direction::In => {
                let sign_pk = match m.sender_sign_pk.clone() {
                    Some(s) => s,
                    None => {
                        let cid = m.contact_id.ok_or(CoreError::State)?;
                        self.db.get_contact(cid)?.ok_or(CoreError::NotFound)?.identity_sign
                    }
                };
                let origin = self.db.message_origin(m.id)?.unwrap_or(m.id);
                Ok(Some(WireReply { sender_sign_pk: sign_pk, origin_msg_id: origin as u64 }))
            }
        }
    }

    pub async fn press_button(&self, contact_id: i64, message_id: i64, callback_data: String) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        let origin_msg_id = self.db.message_origin(message_id)?.unwrap_or(message_id);
        let mut payload = WirePayload {
            origin_msg_id: origin_msg_id as u64,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons: None,
            callback_data: Some(callback_data),
            edit_of: None,
            pin: None,
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
        };
        let out = {
            let g = self.relay_out.read().await;
            g.clone().ok_or(CoreError::State)?
        };
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, &mut payload, &out).await
    }

    pub async fn press_group_button(&self, group_id: &[u8], message_id: i64, callback_data: String) -> Result<()> {
        let msg = self.db.get_message(message_id)?.ok_or(CoreError::NotFound)?;
        if msg.group_id.as_deref() != Some(group_id) { return Err(CoreError::State); }
        let sender_sign = msg.sender_sign_pk.clone().ok_or(CoreError::State)?;
        let contact = self.db.find_contact_by_sign_pk(&sender_sign)?.ok_or(CoreError::NotFound)?;
        let origin_msg_id = self.db.message_origin(message_id)?.unwrap_or(message_id);

        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(),
            dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(),
            name: m.display_name.clone(),
        }).collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef { id: group_id.to_vec(), name: gname, members: gref_members };

        let mut payload = WirePayload {
            origin_msg_id: origin_msg_id as u64,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: Some(gref),
            buttons: None,
            callback_data: Some(callback_data),
            edit_of: None,
            pin: None,
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
        };
        let out = {
            let g = self.relay_out.read().await;
            g.clone().ok_or(CoreError::State)?
        };
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, &mut payload, &out).await
    }

    pub async fn send_edit(&self, contact_id: i64, message_id: i64, new_body: String) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        let msg = self.db.get_message(message_id)?.ok_or(CoreError::NotFound)?;
        if !matches!(msg.direction, Direction::Out) || msg.contact_id != Some(contact_id) {
            return Err(CoreError::State);
        }
        self.db.update_message_body(message_id, &new_body)?;
        let buttons: Option<Vec<Vec<WireButton>>> = self.db.get_setting(&format!("buttons_{}", message_id))
            .ok().flatten()
            .and_then(|b| bincode::deserialize(&b).ok());
        let _ = self.events.try_send(CoreEvent::MessageEdited {
            message_id, body: new_body.clone(), buttons: buttons.clone(),
        });
        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: new_body,
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons,
            callback_data: None,
            edit_of: Some(message_id as u64),
            pin: None,
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
        };
        let out = {
            let g = self.relay_out.read().await;
            g.clone().ok_or(CoreError::State)?
        };
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, &mut payload, &out).await
    }

    pub async fn send_edit_group(&self, group_id: &[u8], message_id: i64, new_body: String) -> Result<()> {
        let msg = self.db.get_message(message_id)?.ok_or(CoreError::NotFound)?;
        if !matches!(msg.direction, Direction::Out) || msg.group_id.as_deref() != Some(group_id) {
            return Err(CoreError::State);
        }
        self.db.update_message_body(message_id, &new_body)?;
        let buttons: Option<Vec<Vec<WireButton>>> = self.db.get_setting(&format!("buttons_{}", message_id))
            .ok().flatten()
            .and_then(|b| bincode::deserialize(&b).ok());
        let _ = self.events.try_send(CoreEvent::MessageEdited {
            message_id, body: new_body.clone(), buttons: buttons.clone(),
        });

        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(),
            dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(),
            name: m.display_name.clone(),
        }).collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef { id: group_id.to_vec(), name: gname, members: gref_members };
        for m in members {
            if m.is_self { continue; }
            let contact = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if contact.trust == TrustLevel::Blocked { continue; }
            let mut payload = WirePayload {
                origin_msg_id: 0,
                body: new_body.clone(),
                attachments: vec![],
                sent_at: now_ms(),
                ttl_ms: None,
                group: Some(gref.clone()),
                buttons: buttons.clone(),
                callback_data: None,
                edit_of: Some(message_id as u64),
                pin: None,
                ack_for: None,
                sender_name: None,
                reply_to: None,
            typing: None,
            notify_sound: None,
            };
            let _ = self.send_to_contact(contact.id, &mut payload).await;
        }
        Ok(())
    }

    pub async fn pin_contact_message(&self, contact_id: i64, message_id: i64, unpin: bool) -> Result<()> {
        let msg = self.db.get_message(message_id)?.ok_or(CoreError::NotFound)?;
        if msg.contact_id != Some(contact_id) { return Err(CoreError::State); }

        let (sender_sign_pk, origin) = match msg.direction {
            Direction::Out => (self.identity.card().sign_pk.to_vec(), message_id as u64),
            Direction::In => {
                let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
                let origin = self.db.message_origin(message_id)?.unwrap_or(message_id);
                (contact.identity_sign, origin as u64)
            }
        };

        if unpin { self.db.unpin_contact_message(contact_id, message_id)?; }
        else { self.db.pin_contact_message(contact_id, message_id)?; }

        let ev = if unpin {
            CoreEvent::MessageUnpinned { contact_id: Some(contact_id), group_id: None, message_id }
        } else {
            CoreEvent::MessagePinned { contact_id: Some(contact_id), group_id: None, message_id }
        };
        let _ = self.events.try_send(ev);

        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons: None,
            callback_data: None,
            edit_of: None,
            pin: Some(WirePin { sender_sign_pk, origin_msg_id: origin, unpin }),
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
        };
        let out = {
            let g = self.relay_out.read().await;
            g.clone().ok_or(CoreError::State)?
        };
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, &mut payload, &out).await
    }

    pub async fn pin_group_message(&self, group_id: &[u8], message_id: i64, unpin: bool) -> Result<()> {
        let msg = self.db.get_message(message_id)?.ok_or(CoreError::NotFound)?;
        if msg.group_id.as_deref() != Some(group_id) { return Err(CoreError::State); }

        let (sender_sign_pk, origin) = match msg.sender_sign_pk.clone() {
            Some(pk) => {
                let origin = self.db.message_origin(message_id)?.unwrap_or(message_id);
                (pk, origin as u64)
            }
            None if matches!(msg.direction, Direction::Out) => {
                (self.identity.card().sign_pk.to_vec(), message_id as u64)
            }
            _ => return Err(CoreError::State),
        };

        if unpin { self.db.unpin_group_message(group_id, message_id)?; }
        else { self.db.pin_group_message(group_id, message_id)?; }

        let ev = if unpin {
            CoreEvent::MessageUnpinned { contact_id: None, group_id: Some(to_hex(group_id)), message_id }
        } else {
            CoreEvent::MessagePinned { contact_id: None, group_id: Some(to_hex(group_id)), message_id }
        };
        let _ = self.events.try_send(ev);

        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(),
            dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(),
            name: m.display_name.clone(),
        }).collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef { id: group_id.to_vec(), name: gname, members: gref_members };
        let wire = WirePin { sender_sign_pk, origin_msg_id: origin, unpin };
        for m in members {
            if m.is_self { continue; }
            let contact = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if contact.trust == TrustLevel::Blocked { continue; }
            let mut payload = WirePayload {
                origin_msg_id: 0,
                body: String::new(),
                attachments: vec![],
                sent_at: now_ms(),
                ttl_ms: None,
                group: Some(gref.clone()),
                buttons: None,
                callback_data: None,
                edit_of: None,
                pin: Some(wire.clone()),
                ack_for: None,
                sender_name: None,
                reply_to: None,
            typing: None,
            notify_sound: None,
            };
            let _ = self.send_to_contact(contact.id, &mut payload).await;
        }
        Ok(())
    }

    pub async fn check_and_emit_update(self: Arc<Self>) -> Result<Option<UpdateInfo>> {
        let info = match self.updater.check(env!("CARGO_PKG_VERSION")).await {
            Ok(Some(i)) => i,
            Ok(None) => return Ok(None),
            Err(e) => {
                eprintln!("[update] check err: {:?}", e);
                return Err(e.into());
            }
        };
        if let Ok(Some(v)) = self.db.get_setting(SETTING_DISMISSED_UPDATE) {
            if String::from_utf8_lossy(&v) == info.version {
                return Ok(None);
            }
        }
        *self.pending_update.lock().await = Some(info.clone());
        let _ = self.events.try_send(CoreEvent::UpdateAvailable {
            version: info.version.clone(),
            notes: info.notes.clone(),
            target_key: info.target_key.clone(),
            size: info.artifact.size,
        });
        Ok(Some(info))
    }

    pub async fn install_update(self: Arc<Self>) -> Result<()> {
        let info = self.pending_update.lock().await.clone().ok_or(CoreError::NotFound)?;
        let ev = self.events.clone();
        let ev_dl = ev.clone();
        let total = info.artifact.size;

        let path = match self.updater.download(&info, move |done, t| {
            let pct = if t > 0 { ((done * 100) / t).min(100) as u8 } else { 0 };
            let _ = ev_dl.try_send(CoreEvent::UpdateProgress { downloaded: done, total: t, pct });
        }).await {
            Ok(p) => p,
            Err(e) => {
                let _ = ev.send(CoreEvent::UpdateFailed { reason: e.to_string() }).await;
                return Err(e.into());
            }
        };
        let _ = total;

        match self.updater.install_and_respawn(&path, &info.target_key) {
            Ok(_) => Ok(()),
            Err(UpdateError::Unsupported(_)) => {
                let _ = ev.send(CoreEvent::UpdateReady { path: path.display().to_string() }).await;
                Ok(())
            }
            Err(e) => {
                let _ = ev.send(CoreEvent::UpdateFailed { reason: e.to_string() }).await;
                Err(e.into())
            }
        }
    }

    pub async fn dismiss_update(&self, version: String) -> Result<()> {
        self.db.set_setting(SETTING_DISMISSED_UPDATE, version.as_bytes())?;
        *self.pending_update.lock().await = None;
        Ok(())
    }

    pub async fn list_apk_artifacts(&self) -> Result<(String, Vec<(String, u64)>)> {
        let m = self.updater.manifest().await?;
        let mut out = Vec::new();
        for (key, art) in m.artifacts.iter() {
            if let Some(arch) = key.strip_prefix("android-apk-") {
                out.push((arch.to_string(), art.size));
            }
        }
        out.sort();
        Ok((m.version, out))
    }

    pub async fn download_apk(self: Arc<Self>, arch: String, dest_path: String) -> Result<()> {
        let m = self.updater.manifest().await?;
        let key = format!("android-apk-{}", arch);
        let art = m.artifacts.get(&key)
            .ok_or(CoreError::NotFound)?
            .clone();
        let total = art.size;
        let ev = self.events.clone();
        let ev_dl = ev.clone();
        let dest = std::path::PathBuf::from(dest_path);
        match self.updater.download_artifact_to(&art, &dest, move |done, t| {
            let pct = if t > 0 { ((done * 100) / t).min(100) as u8 } else { 0 };
            let _ = ev_dl.try_send(CoreEvent::UpdateProgress { downloaded: done, total: t, pct });
        }).await {
            Ok(_) => {
                let _ = ev.send(CoreEvent::UpdateReady { path: dest.display().to_string() }).await;
                let _ = total;
                Ok(())
            }
            Err(e) => {
                let _ = ev.send(CoreEvent::UpdateFailed { reason: e.to_string() }).await;
                Err(e.into())
            }
        }
    }

    pub async fn create_group(&self, name: &str, member_contact_ids: &[i64]) -> Result<Vec<u8>> {
        let mut gid = vec![0u8; 32];
        OsRng.fill_bytes(&mut gid);
        self.db.create_group(&gid, name)?;
        self.db.add_group_member(&gid, &self.identity.card().sign_pk, &self.identity.card().dh_pk,
            self.node.onion_address(), &self.display_name().unwrap_or_default(), true)?;
        let mut members_wire = Vec::new();
        members_wire.push(WireMember {
            sign_pk: self.identity.card().sign_pk.to_vec(),
            dh_pk: self.identity.card().dh_pk.to_vec(),
            onion: self.node.onion_address().to_string(),
            name: self.display_name().unwrap_or_default(),
        });
        for cid in member_contact_ids {
            let c = self.db.get_contact(*cid)?.ok_or(CoreError::NotFound)?;
            self.db.add_group_member(&gid, &c.identity_sign, &c.identity_dh, &c.onion_address, &c.display_name, false)?;
            members_wire.push(WireMember {
                sign_pk: c.identity_sign.clone(),
                dh_pk: c.identity_dh.clone(),
                onion: c.onion_address.clone(),
                name: c.display_name.clone(),
            });
        }
        let gref = WireGroupRef { id: gid.clone(), name: name.to_string(), members: members_wire };
        for cid in member_contact_ids {
            let sent_at = now_ms();
            let mut payload = WirePayload::simple(0, String::new(), Vec::new(), sent_at, None);
            payload.group = Some(gref.clone());
            self.send_to_contact(*cid, &mut payload).await?;
        }
        let _ = self.events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(&gid) });
        Ok(gid)
    }

    pub async fn add_group_member(&self, group_id: &[u8], contact_id: i64) -> Result<()> {
        let group_name = self.db.get_group_name(group_id)?.ok_or(CoreError::NotFound)?;
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        if self.db.is_group_member(group_id, &contact.identity_sign)? {
            return Ok(());
        }
        self.db.add_group_member(
            group_id,
            &contact.identity_sign,
            &contact.identity_dh,
            &contact.onion_address,
            &contact.display_name,
            false,
        )?;
        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(),
            dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(),
            name: m.display_name.clone(),
        }).collect();
        let gref = WireGroupRef { id: group_id.to_vec(), name: group_name, members: gref_members };
        for m in members {
            if m.is_self { continue; }
            let target = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if target.trust == TrustLevel::Blocked { continue; }
            let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
            payload.group = Some(gref.clone());
            let _ = self.send_to_contact(target.id, &mut payload).await;
        }
        let _ = self.events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(group_id) });
        Ok(())
    }

    pub async fn send_to_group(
        &self,
        group_id: &[u8],
        body: String,
        attachments: Vec<PendingAttachment>,
        _ttl: Option<Duration>,
        reply_to: Option<i64>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let expires_at: Option<i64> = None;
        let ttl: Option<Duration> = None;
        let mut stored = Vec::with_capacity(attachments.len());
        for a in &attachments {
            let (key, path, size) = self.store_attachment(&a.data)?;
            stored.push(NewAttachment {
                name: a.name.clone(), size: size as i64, key: key.to_vec(), path,
            });
        }
        let msg_id = self.db.insert_group_message(
            group_id, Some(&self.identity.card().sign_pk), Direction::Out,
            &body, sent_at, expires_at, &stored,
        )?;
        if let Some(rt) = reply_to {
            self.db.set_reply_to(msg_id, Some(rt))?;
        }
        let wire_reply = match reply_to {
            Some(rt) => self.build_wire_reply(rt)?,
            None => None,
        };
        let wire_atts: Vec<WireAttachment> = attachments.into_iter()
            .map(|a| WireAttachment { name: a.name, data: a.data }).collect();
        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(),
            dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(),
            name: m.display_name.clone(),
        }).collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef { id: group_id.to_vec(), name: gname, members: gref_members };
        for m in members {
            if m.is_self { continue; }
            let contact = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if contact.trust == TrustLevel::Blocked { continue; }
            self.db.pending_outbound_add(msg_id, contact.id)?;
            let mut payload = WirePayload::simple(
                msg_id as u64, body.clone(), wire_atts.clone(), sent_at,
                ttl.map(|d| d.as_millis() as i64),
            );
            payload.group = Some(gref.clone());
            payload.reply_to = wire_reply.clone();
            match self.send_to_contact(contact.id, &mut payload).await {
                Ok(()) => { let _ = self.db.pending_outbound_remove(msg_id, contact.id); }
                Err(e) => eprintln!("[relay-client] group send to contact {} failed (msg {}): {:?} — kept in pending_outbound", contact.id, msg_id, e),
            }
        }
        Ok(msg_id)
    }

    fn store_attachment(&self, data: &[u8]) -> Result<([u8; 32], String, u64)> {
        let cipher = AttachmentCipher::generate();
        let encrypted = cipher.encrypt_chunk(0, &[], data)?;
        let mut name = [0u8; 24];
        OsRng.fill_bytes(&mut name);
        let hex = to_hex(&name);
        let dir = self.data_dir.join(ATTACHMENTS_DIR);
        let path = dir.join(&hex);
        std::fs::write(&path, &encrypted)?;
        Ok((*cipher.key(), hex, data.len() as u64))
    }

    pub fn read_attachment(&self, att: &Attachment) -> Result<Vec<u8>> {
        let key = to_arr32(att.key.clone())?;
        let full = self.data_dir.join(ATTACHMENTS_DIR).join(&att.path);
        let enc = std::fs::read(&full)?;
        let pt = AttachmentCipher::from_key(key).decrypt_chunk(0, &[], &enc)?;
        Ok(pt)
    }

    fn spawn_relay_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut backoff = RECONNECT_INITIAL_MS;
            loop {
                let onion = this.relay_onion();
                if onion.is_empty() {
                    // No relay configured yet (i2p: DEFAULT_RELAY not baked in and
                    // none set in Settings). Wait quietly instead of hammering.
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                    backoff = (backoff * 2).min(RECONNECT_MAX_MS);
                    continue;
                }
                eprintln!("[relay-client] connecting to {}", &onion[..16.min(onion.len())]);
                match relay::connect(&this.node, &onion, &this.identity).await {
                    Ok(client) => {
                        eprintln!("[relay-client] connected & authed");
                        backoff = RECONNECT_INITIAL_MS;
                        *this.relay_out.write().await = Some(client.out_tx.clone());
                        let _ = this.events.try_send(CoreEvent::RelayConnected);

                        if let Ok(bundle) = this.my_bundle() {
                            if let Ok(bytes) = bincode::serialize(&bundle) {
                                let _ = client.out_tx.send(ClientToRelay::Publish { bundle: bytes }).await;
                            }
                        }
                        this.send_kick.notify_one();
                        this.clone().run_recv_loop(client).await;
                        *this.relay_out.write().await = None;
                        let _ = this.events.try_send(CoreEvent::RelayDisconnected);
                    }
                    Err(e) => eprintln!("[relay-client] connect fail: {:?}", e),
                }
                tokio::time::sleep(Duration::from_millis(backoff)).await;
                backoff = (backoff * 2).min(RECONNECT_MAX_MS);
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    async fn run_recv_loop(self: Arc<Self>, client: RelayClient) {
        let in_rx = client.in_rx.clone();
        let out_tx = client.out_tx.clone();
        let mut ping = tokio::time::interval(Duration::from_secs(PING_INTERVAL_SECS));
        ping.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ping.tick().await;
        let dead_threshold = Duration::from_secs(DEAD_THRESHOLD_SECS);
        let mut last_activity = std::time::Instant::now();
        loop {
            tokio::select! {
                _ = ping.tick() => {
                    if last_activity.elapsed() > dead_threshold {
                        eprintln!("[relay-client] no activity for {:?}, forcing reconnect", last_activity.elapsed());
                        break;
                    }
                    if out_tx.send(ClientToRelay::Ping).await.is_err() { break; }
                }
                frame = async { in_rx.lock().await.recv().await } => {
                    let Some(frame) = frame else { break; };
                    last_activity = std::time::Instant::now();
                    if let Err(e) = self.handle_relay_frame(frame, &out_tx).await {
                        eprintln!("[relay-client] handle err: {:?}", e);
                    }
                }
            }
        }
    }

    async fn handle_relay_frame(
        &self,
        frame: RelayToClient,
        out_tx: &mpsc::Sender<ClientToRelay>,
    ) -> Result<()> {
        match frame {
            RelayToClient::Incoming { id, from, blob } => {
                match self.handle_incoming_envelope(&from, &blob).await {
                    Ok(()) => { let _ = out_tx.send(ClientToRelay::Ack { id }).await; }
                    Err(CoreError::Codec) => {
                        eprintln!("[relay-client] codec err on msg {}, NOT acking (will retry)", id);
                    }
                    Err(CoreError::StaleOpk) => {
                        eprintln!("[relay-client] stale-OPK X3dhInit on msg {}, ACK and skip (zombie)", id);
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                    Err(CoreError::SealedDrop) => {
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                    Err(CoreError::Crypto(_)) => {
                        if let Ok(Some(c)) = self.db.find_contact_by_sign_pk(&from) {
                            let fresh = {
                                let m = self.session_created_at.lock().await;
                                m.get(&c.id).map(|t| now_ms() - *t < FRESH_SESSION_GRACE_MS).unwrap_or(false)
                            };
                            if fresh {
                                eprintln!("[relay-client] crypto err on msg {} within fresh-session grace, ACK and skip", id);
                                let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                            } else {
                                eprintln!("[relay-client] crypto err on msg {}, requesting resync, NOT acking", id);
                                let _ = self.request_resync(&c).await;
                            }
                        } else {
                            eprintln!("[relay-client] crypto err on msg {}, no contact, NOT acking", id);
                        }
                    }
                    Err(e) => {
                        eprintln!("[relay-client] incoming err: {:?}", e);
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                }
            }
            RelayToClient::Bundle { pk, bundle } => {
                let mut w = self.bundle_waiters.lock().await;
                if let Some(vec) = w.remove(&pk) {
                    for tx in vec { let _ = tx.send(bundle.clone()); }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_incoming_envelope(&self, from_pk: &[u8; 32], blob: &[u8]) -> Result<()> {
        let envelope: EnvelopeBlob = bincode::deserialize(blob)?;
        let sealed = from_pk == &[0u8; 32];
        match envelope {
            EnvelopeBlob::X3dhInit(init) => {
                let sender_sign = init.identity.sign_pk;
                let sender_dh = init.identity.dh_pk;
                let contact = match self.db.find_contact_by_sign_pk(&sender_sign)? {
                    Some(c) => c,
                    None => {
                        let name = hex_short(&sender_sign);
                        let id = self.db.add_contact(&sender_sign, &sender_dh, "", &name)?;
                        let _ = self.events.try_send(CoreEvent::ContactAdded { contact_id: id });
                        self.db.get_contact(id)?.ok_or(CoreError::NotFound)?
                    }
                };
                if contact.trust == TrustLevel::Blocked { return Ok(()); }
                if init.identity.sign_pk != contact.identity_sign.as_slice()
                    || init.identity.dh_pk != contact.identity_dh.as_slice()
                {
                    return Err(CoreError::State);
                }
                let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
                self.sessions.lock().await.remove(&contact.id);
                let _ = self.db.delete_session(contact.id);
                self.tiebreaker_waits.lock().await.remove(&contact.id);
                let (state, plaintext) = self.accept_x3dh(&init, &ad).await?;
                self.sessions.lock().await.insert(contact.id, state);
                self.session_created_at.lock().await.insert(contact.id, now_ms());
                let state_bytes = {
                    let s = self.sessions.lock().await;
                    s.get(&contact.id).unwrap().to_bytes()?
                };
                self.db.put_session(contact.id, &state_bytes)?;
                let payload: WirePayload = decode_with_padding_fallback(&plaintext)?;
                self.persist_incoming(contact.id, payload).await?;
                eprintln!("[relay-client] session established with contact {} via X3dhInit", contact.id);
                if init.one_time_id.is_some() {
                    self.republish_bundle().await;
                }
                self.send_kick.notify_one();
            }
            EnvelopeBlob::Ratchet { header, ciphertext } => {
                let mut decrypted: Option<(i64, Vec<u8>, Vec<u8>)> = None;
                let candidates: Vec<gipny_libcore::db::Contact> = if sealed {
                    self.db.list_contacts()?.into_iter().filter(|c| c.trust != TrustLevel::Blocked).collect()
                } else {
                    match self.db.find_contact_by_sign_pk(from_pk)? {
                        Some(c) if c.trust != TrustLevel::Blocked => vec![c],
                        _ => vec![],
                    }
                };
                for c in candidates {
                    let ad = build_ad(&self.identity.card().dh_pk, &c.identity_dh);
                    let mut sess = self.sessions.lock().await;
                    let attempt = match sess.get_mut(&c.id) {
                        Some(state) => state.decrypt(&header, &ciphertext, &ad).map(|pt| (pt, state.to_bytes())),
                        None => match self.db.get_session(c.id)? {
                            Some(blob) => {
                                let mut state = RatchetState::from_bytes(&blob)?;
                                let r = state.decrypt(&header, &ciphertext, &ad).map(|pt| (pt, state.to_bytes()));
                                if r.is_ok() { sess.insert(c.id, state); }
                                r
                            }
                            None => continue,
                        },
                    };
                    drop(sess);
                    if let Ok((pt, sb_res)) = attempt {
                        let sb = sb_res?;
                        decrypted = Some((c.id, pt, sb));
                        break;
                    }
                }
                let (cid, pt, sb) = match decrypted {
                    Some(x) => x,
                    None => {
                        if sealed {
                            eprintln!("[relay-client] sealed ratchet: no session matched, ACK and drop");
                            return Err(CoreError::SealedDrop);
                        }
                        if let Ok(Some(c)) = self.db.find_contact_by_sign_pk(from_pk) {
                            eprintln!("[relay-client] no session for contact {}, requesting resync", c.id);
                            let _ = self.request_resync(&c).await;
                        }
                        return Err(CoreError::Crypto(gipny_libcore::crypto::CryptoError::Mac));
                    }
                };
                self.db.put_session(cid, &sb)?;
                let payload: WirePayload = decode_with_padding_fallback(&pt)?;
                let is_keepalive = payload.ack_for == Some(0)
                    && payload.origin_msg_id == 0
                    && payload.body.is_empty()
                    && payload.attachments.is_empty();
                if !is_keepalive {
                    *self.incoming_since_send.lock().await.entry(cid).or_insert(0) += 1;
                }
                self.persist_incoming(cid, payload).await?;
                if !is_keepalive {
                    let counter = self.incoming_since_send.lock().await.get(&cid).copied().unwrap_or(0);
                    if counter >= KEEPALIVE_INCOMING_THRESHOLD {
                        self.send_kick.notify_one();
                    }
                }
            }
        }
        Ok(())
    }

    async fn accept_x3dh(
        &self,
        init: &gipny_libcore::crypto::X3dhInitial,
        ad: &[u8],
    ) -> Result<(RatchetState, Vec<u8>)> {
        let signed_id = i64::from_be_bytes(
            self.db.get_setting(SETTING_SIGNED_PREKEY_ID)?.ok_or(CoreError::State)?
                .try_into().map_err(|_| CoreError::State)?);
        let signed = self.db.get_prekey(signed_id)?.ok_or(CoreError::State)?;
        let signed_pair = PreKeyPair::from_secret(to_arr32(signed.private.clone())?);
        let opk_pair = if let Some(opk_id) = init.one_time_id {
            let p = self.db.get_prekey(opk_id)?;
            if p.is_none() { return Err(CoreError::StaleOpk); }
            if let Some(ref pk) = p { let _ = self.db.delete_prekey(pk.id); }
            p.map(|p| Ok::<PreKeyPair, CoreError>(PreKeyPair::from_secret(to_arr32(p.private.clone())?))).transpose()?
        } else { None };
        let (state, pt) = crypto::x3dh_respond(&self.identity, &signed_pair, opk_pair.as_ref(), init, ad)?;
        Ok((state, pt))
    }

    async fn persist_incoming(&self, contact_id: i64, payload: WirePayload) -> Result<()> {
        if let Some(typing) = payload.typing {
            let group_id_hex = payload.group.as_ref().map(|g| hex_bytes(&g.id));
            let sender_sign_hex = self.db.get_contact(contact_id).ok().flatten()
                .map(|c| hex_bytes(&c.identity_sign));
            let _ = self.events.try_send(CoreEvent::Typing {
                contact_id: if payload.group.is_none() { Some(contact_id) } else { None },
                group_id: group_id_hex,
                sender_sign_pk: sender_sign_hex,
                typing,
            });
            return Ok(());
        }
        if let Some(name) = payload.sender_name.as_deref() {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                self.apply_peer_name(contact_id, trimmed);
            }
        }
        if payload.buttons.is_some() || payload.callback_data.is_some() {
            if self.db.set_contact_is_bot(contact_id, true).unwrap_or(false) {
                let _ = self.events.try_send(CoreEvent::ContactUpdated { contact_id });
            }
        }
        if let Some(gref) = &payload.group {
            ensure_group_from_wire(&self.db, &self.events, &self.identity, gref).await?;
        }

        if let Some(ack_local) = payload.ack_for {
            let local_id = ack_local as i64;
            if let Some(msg) = self.db.get_message(local_id)? {
                let belongs = match (msg.contact_id, &msg.group_id, &payload.group) {
                    (Some(cid), _, None) => cid == contact_id,
                    (_, Some(gid), Some(gref)) => gid == &gref.id,
                    _ => false,
                };
                if belongs && matches!(msg.direction, Direction::Out) {
                    let _ = self.db.pending_outbound_remove(local_id, contact_id);
                    self.db.mark_delivered(local_id)?;
                    let _ = self.events.try_send(CoreEvent::MessageDelivered { message_id: local_id });
                }
            }
            return Ok(());
        }

        if payload.callback_data.is_some()
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.body.is_empty()
            && payload.attachments.is_empty()
        {
            return Ok(());
        }

        if let Some(edit_target_origin) = payload.edit_of {
            let lookup = if let Some(gref) = &payload.group {
                let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
                let self_sign = self.identity.card().sign_pk.to_vec();
                self.db.resolve_group_message(&gref.id, &contact.identity_sign, edit_target_origin as i64, &self_sign)?
            } else {
                self.db.find_message_by_origin(contact_id, edit_target_origin as i64)?
            };
            if let Some(local_id) = lookup {
                self.db.update_message_body(local_id, &payload.body)?;
                if let Some(btns) = &payload.buttons {
                    if let Ok(b) = bincode::serialize(btns) {
                        self.db.set_setting(&format!("buttons_{}", local_id), &b)?;
                    }
                } else {
                    let _ = self.db.delete_setting(&format!("buttons_{}", local_id));
                }
                let _ = self.events.try_send(CoreEvent::MessageEdited {
                    message_id: local_id,
                    body: payload.body.clone(),
                    buttons: payload.buttons.clone(),
                });
                return Ok(());
            }
        }

        if let Some(pin) = &payload.pin {
            let self_sign = self.identity.card().sign_pk.to_vec();
            let origin = pin.origin_msg_id as i64;
            let (resolved, target_cid, target_gid) = if let Some(gref) = &payload.group {
                (self.db.resolve_group_message(&gref.id, &pin.sender_sign_pk, origin, &self_sign)?,
                 None, Some(gref.id.clone()))
            } else {
                (self.db.resolve_contact_message(contact_id, origin)?,
                 Some(contact_id), None)
            };
            match resolved {
                Some(local) => {
                    match (&target_gid, pin.unpin) {
                        (Some(gid), true)  => { self.db.unpin_group_message(gid, local)?; }
                        (Some(gid), false) => { self.db.pin_group_message(gid, local)?; }
                        (None, true)       => { self.db.unpin_contact_message(contact_id, local)?; }
                        (None, false)      => { self.db.pin_contact_message(contact_id, local)?; }
                    }
                    let gid_hex = target_gid.as_deref().map(to_hex);
                    let _ = self.events.try_send(if pin.unpin {
                        CoreEvent::MessageUnpinned { contact_id: target_cid, group_id: gid_hex, message_id: local }
                    } else {
                        CoreEvent::MessagePinned { contact_id: target_cid, group_id: gid_hex, message_id: local }
                    });
                }
                None => match &target_gid {
                    Some(gid) => {
                        self.db.add_deferred_pin_group(gid, &pin.sender_sign_pk, origin, pin.unpin)?;
                        eprintln!("[relay-client] deferred pin for group {} origin={}", to_hex(gid), pin.origin_msg_id);
                    }
                    None => {
                        self.db.add_deferred_pin_contact(contact_id, &pin.sender_sign_pk, origin, pin.unpin)?;
                        eprintln!("[relay-client] deferred pin for contact {} origin={}", contact_id, pin.origin_msg_id);
                    }
                },
            }
            return Ok(());
        }

        let is_empty = payload.body.is_empty() && payload.attachments.is_empty() && payload.group.is_none() && payload.callback_data.is_none();
        if is_empty { return Ok(()); }

        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;

        if payload.origin_msg_id > 0 {
            let existing = if let Some(gref) = &payload.group {
                let self_sign = self.identity.card().sign_pk.to_vec();
                self.db.resolve_group_message(&gref.id, &contact.identity_sign, payload.origin_msg_id as i64, &self_sign)?
            } else {
                self.db.find_message_by_origin(contact_id, payload.origin_msg_id as i64)?
            };
            if existing.is_some() {
                eprintln!("[relay-client] duplicate origin={} from contact {}, re-acking", payload.origin_msg_id, contact_id);
                let _ = self.send_ack(&contact, &payload).await;
                return Ok(());
            }
        }

        let expires_at = payload.ttl_ms.map(|t| payload.sent_at + t);
        let mut atts = Vec::with_capacity(payload.attachments.len());
        for a in &payload.attachments {
            let (key, path, size) = store_attachment_raw(&self.data_dir, &a.data)?;
            atts.push(NewAttachment { name: a.name.clone(), size: size as i64, key: key.to_vec(), path });
        }
        let (mid, group_id_for_event) = if let Some(gref) = &payload.group {
            if payload.body.is_empty() && payload.attachments.is_empty() {
                let _ = self.events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(&gref.id) });
                return Ok(());
            }
            let mid = self.db.insert_group_message_with_origin(
                &gref.id, Some(&contact.identity_sign), Direction::In,
                &payload.body, payload.sent_at, expires_at, &atts,
                Some(payload.origin_msg_id as i64),
            )?;
            (mid, Some(gref.id.clone()))
        } else {
            let mid = self.db.insert_message_with_origin(
                contact_id, Direction::In, &payload.body, payload.sent_at, expires_at, &atts,
                Some(payload.origin_msg_id as i64),
            )?;
            (mid, None)
        };
        if let Some(btns) = &payload.buttons {
            if let Ok(b) = bincode::serialize(btns) {
                self.db.set_setting(&format!("buttons_{}", mid), &b)?;
            }
        }
        if let Some(rep) = &payload.reply_to {
            let self_sign = self.identity.card().sign_pk.to_vec();
            let local = if let Some(gid) = &group_id_for_event {
                self.db.resolve_group_message(gid, &rep.sender_sign_pk, rep.origin_msg_id as i64, &self_sign)?
            } else if rep.sender_sign_pk == self_sign {
                let cid = rep.origin_msg_id as i64;
                match self.db.get_message(cid)? {
                    Some(m) if m.contact_id == Some(contact_id) && matches!(m.direction, Direction::Out) => Some(cid),
                    _ => None,
                }
            } else {
                self.db.find_message_by_origin(contact_id, rep.origin_msg_id as i64)?
            };
            if let Some(rt) = local {
                self.db.set_reply_to(mid, Some(rt))?;
            }
        }
        self.db.touch_contact(contact_id)?;
        let deferred_unpin = match &group_id_for_event {
            Some(gid) => self.db.take_deferred_pin_group(gid, &contact.identity_sign, payload.origin_msg_id as i64)?,
            None => self.db.take_deferred_pin_contact(contact_id, &contact.identity_sign, payload.origin_msg_id as i64)?,
        };
        if let Some(unpin) = deferred_unpin {
            match (&group_id_for_event, unpin) {
                (Some(gid), true)  => { self.db.unpin_group_message(gid, mid)?; }
                (Some(gid), false) => { self.db.pin_group_message(gid, mid)?; }
                (None, true)       => { self.db.unpin_contact_message(contact_id, mid)?; }
                (None, false)      => { self.db.pin_contact_message(contact_id, mid)?; }
            }
            let (tcid, tgid) = match &group_id_for_event {
                Some(gid) => (None, Some(to_hex(gid))),
                None => (Some(contact_id), None),
            };
            let _ = self.events.try_send(if unpin {
                CoreEvent::MessageUnpinned { contact_id: tcid, group_id: tgid, message_id: mid }
            } else {
                CoreEvent::MessagePinned { contact_id: tcid, group_id: tgid, message_id: mid }
            });
        }
        eprintln!("[relay-client] received msg id={} from contact {}", mid, contact_id);
        let body_for_event = payload.body.clone();
        let _ = self.events.try_send(CoreEvent::IncomingMessage {
            contact_id: if group_id_for_event.is_none() { Some(contact_id) } else { None },
            group_id: group_id_for_event.as_deref().map(to_hex),
            sender_sign_pk: Some(to_hex(&contact.identity_sign)),
            message_id: mid,
            body: body_for_event, sent_at: payload.sent_at,
            notify_sound: payload.notify_sound.clone(),
        });
        let _ = self.send_ack(&contact, &payload).await;
        Ok(())
    }

    pub async fn reset_contact_session(&self, contact_id: i64) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        self.request_resync(&contact).await
    }

    async fn request_resync(&self, contact: &gipny_libcore::db::Contact) -> Result<()> {
        let throttled = self.db.resync_recent(contact.id, 60_000)?;
        if !throttled {
            eprintln!("[relay-client] forcing resync for contact {}", contact.id);
            self.db.record_resync(contact.id)?;
            self.sessions.lock().await.remove(&contact.id);
            self.session_created_at.lock().await.remove(&contact.id);
            let _ = self.db.delete_session(contact.id);
            let mut w = self.tiebreaker_waits.lock().await;
            w.insert(contact.id, now_ms() - TIEBREAKER_TIMEOUT_MS - 1);
        }
        self.send_kick.notify_one();
        Ok(())
    }

    async fn send_ack(&self, contact: &gipny_libcore::db::Contact, original: &WirePayload) -> Result<()> {
        if original.origin_msg_id == 0 { return Ok(()); }
        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: original.group.clone(),
            buttons: None,
            callback_data: None,
            edit_of: None,
            pin: None,
            ack_for: Some(original.origin_msg_id),
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
        };
        let out = {
            let g = self.relay_out.read().await;
            match g.clone() { Some(x) => x, None => return Ok(()) }
        };
        if self.ensure_session_for(contact, &out).await.is_err() {
            return Ok(());
        }
        let _ = self.send_payload_via_relay(contact, &mut payload, &out).await;
        Ok(())
    }

    fn spawn_send_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = this.send_kick.notified() => {}
                    _ = tick.tick() => {}
                }
                if let Err(e) = this.flush_all_pending().await {
                    eprintln!("[relay-client] flush err: {:?}", e);
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    async fn flush_all_pending(&self) -> Result<()> {
        let out = {
            let g = self.relay_out.read().await;
            match g.clone() { Some(x) => x, None => return Ok(()) }
        };
        let contacts = self.db.list_contacts()?;
        let groups_by_id: HashMap<Vec<u8>, String> = self.db.list_groups()?
            .into_iter().map(|g| (g.id, g.name)).collect();
        let members_by_group: HashMap<Vec<u8>, Vec<GroupMember>> = self.db.list_all_group_members()?;
        for contact in contacts {
            if contact.trust == TrustLevel::Blocked { continue; }
            let pending = self.db.list_unsent_outgoing(contact.id, 50)?;
            let unacked = self.db.list_unacked_outgoing(
                contact.id, now_ms(), RETRY_BASE_BACKOFF_MS, RETRY_MAX_BACKOFF_MS, 50,
            )?;
            let needs_session = self.db.resync_recent(contact.id, 120_000).unwrap_or(false)
                && !self.sessions.lock().await.contains_key(&contact.id);
            let needs_keepalive = self.incoming_since_send.lock().await.get(&contact.id).copied().unwrap_or(0) >= KEEPALIVE_INCOMING_THRESHOLD
                && self.sessions.lock().await.contains_key(&contact.id);
            if pending.is_empty() && unacked.is_empty() && !needs_session && !needs_keepalive { continue; }
            if self.ensure_session_for(&contact, &out).await.is_err() { continue; }
            if needs_keepalive && pending.is_empty() && unacked.is_empty() {
                let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
                payload.ack_for = Some(0);
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    eprintln!("[relay-client] keepalive err to contact {}: {:?}", contact.id, e);
                } else {
                    eprintln!("[relay-client] keepalive sent to contact {} (DH-roll forced)", contact.id);
                }
            }
            for msg in pending {
                let mut payload = self.build_payload_from_db(&msg)?;
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    eprintln!("[relay-client] send err to contact {}: {:?}", contact.id, e);
                    break;
                }
            }
            for msg in unacked {
                let mut payload = self.build_payload_from_db(&msg)?;
                eprintln!("[relay-client] retry unacked msg {} to contact {} (attempt {})",
                    msg.id, contact.id, msg.send_attempts + 1);
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    eprintln!("[relay-client] retry err to contact {}: {:?}", contact.id, e);
                    break;
                }
            }
            let group_pending = self.db.pending_outbound_for_recipient(
                contact.id, now_ms(), RETRY_BASE_BACKOFF_MS, RETRY_MAX_BACKOFF_MS, 50,
            )?;
            for msg_id in group_pending {
                let msg = match self.db.get_message(msg_id)? { Some(m) => m, None => {
                    let _ = self.db.pending_outbound_remove(msg_id, contact.id);
                    continue;
                } };
                let mut payload = self.build_payload_from_db(&msg)?;
                if let Some(gid) = &msg.group_id {
                    let gname = groups_by_id.get(gid).cloned().unwrap_or_default();
                    let gref_members: Vec<WireMember> = members_by_group.get(gid).map(|v| v.iter().map(|m| WireMember {
                        sign_pk: m.sign_pk.clone(), dh_pk: m.dh_pk.clone(),
                        onion: m.onion.clone(), name: m.display_name.clone(),
                    }).collect()).unwrap_or_default();
                    payload.group = Some(WireGroupRef { id: gid.clone(), name: gname, members: gref_members });
                }
                eprintln!("[relay-client] retry group msg {} to contact {}", msg_id, contact.id);
                self.db.pending_outbound_record_attempt(msg_id, contact.id)?;
                match self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    Ok(()) => { let _ = self.db.pending_outbound_remove(msg_id, contact.id); }
                    Err(e) => {
                        eprintln!("[relay-client] retry group err to contact {}: {:?}", contact.id, e);
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    async fn send_to_contact(&self, contact_id: i64, payload: &mut WirePayload) -> Result<()> {
        let out = {
            let g = self.relay_out.read().await;
            match g.clone() { Some(x) => x, None => return Err(CoreError::State) }
        };
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, payload, &out).await
    }

    pub async fn send_typing_dm(&self, contact_id: i64, typing: bool) -> Result<()> {
        if !self.sessions.lock().await.contains_key(&contact_id) { return Ok(()); }
        let mut payload = make_typing_payload(None, typing);
        let _ = self.send_to_contact(contact_id, &mut payload).await;
        Ok(())
    }

    pub async fn send_typing_group(&self, group_id: &[u8], typing: bool) -> Result<()> {
        let members = self.db.list_group_members(group_id)?;
        let gref = WireGroupRef {
            id: group_id.to_vec(),
            name: self.db.get_group_name(group_id)?.unwrap_or_default(),
            members: members.iter().map(|m| WireMember {
                sign_pk: m.sign_pk.clone(), dh_pk: m.dh_pk.clone(),
                onion: m.onion.clone(), name: m.display_name.clone(),
            }).collect(),
        };
        let self_sign = self.identity.card().sign_pk;
        for m in &members {
            if m.sign_pk == self_sign.as_slice() { continue; }
            if let Some(c) = self.db.find_contact_by_sign_pk(&m.sign_pk)? {
                if !self.sessions.lock().await.contains_key(&c.id) { continue; }
                let mut payload = make_typing_payload(Some(gref.clone()), typing);
                let _ = self.send_to_contact(c.id, &mut payload).await;
            }
        }
        Ok(())
    }

    async fn ensure_session_for(
        &self,
        contact: &gipny_libcore::db::Contact,
        out: &mpsc::Sender<ClientToRelay>,
    ) -> Result<()> {
        if self.sessions.lock().await.contains_key(&contact.id) {
            eprintln!("[relay-client] ensure_session: contact {} already in cache, no-op", contact.id);
            return Ok(());
        }
        if let Some(blob) = self.db.get_session(contact.id)? {
            eprintln!("[relay-client] ensure_session: contact {} found in DB, loading", contact.id);
            let state = RatchetState::from_bytes(&blob)?;
            self.sessions.lock().await.insert(contact.id, state);
            return Ok(());
        }
        eprintln!("[relay-client] ensure_session: contact {} has NO session, will initiate X3DH", contact.id);

        let me_sign = self.identity.card().sign_pk;
        let should_initiate = me_sign.as_slice() < contact.identity_sign.as_slice();
        if !should_initiate {
            let waited_ms = {
                let mut w = self.tiebreaker_waits.lock().await;
                let now = now_ms();
                let started = *w.entry(contact.id).or_insert(now);
                now - started
            };
            if waited_ms < TIEBREAKER_TIMEOUT_MS {
                eprintln!("[relay-client] tiebreaker: waiting for X3dhInit from contact {} ({}ms)", contact.id, waited_ms);
                return Err(CoreError::State);
            }
            eprintln!("[relay-client] tiebreaker timeout, initiating anyway for contact {}", contact.id);
        }
        self.tiebreaker_waits.lock().await.remove(&contact.id);

        let mut pk = [0u8; 32];
        pk.copy_from_slice(&contact.identity_sign);
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.bundle_waiters.lock().await.entry(pk).or_default().push(tx);
        if out.send(ClientToRelay::GetBundle { pk }).await.is_err() {
            eprintln!("[relay-client] get_bundle send failed for contact {} (relay channel closed)", contact.id);
            return Err(CoreError::State);
        }
        let bundle_bytes = match tokio::time::timeout(Duration::from_millis(PENDING_REQ_TIMEOUT_MS), rx).await {
            Err(_) => {
                eprintln!("[relay-client] get_bundle timeout for contact {} after {}ms — relay not responding", contact.id, PENDING_REQ_TIMEOUT_MS);
                return Err(CoreError::State);
            }
            Ok(Err(_)) => {
                eprintln!("[relay-client] get_bundle channel dropped for contact {}", contact.id);
                return Err(CoreError::State);
            }
            Ok(Ok(v)) => v,
        };
        let bundle_bytes = match bundle_bytes {
            Some(b) => b,
            None => {
                eprintln!("[relay-client] get_bundle: relay has NO bundle for contact {} — peer never published or relay dropped it", contact.id);
                return Err(CoreError::NotFound);
            }
        };
        let bundle: PreKeyBundle = bincode::deserialize(&bundle_bytes)?;

        if bundle.identity.sign_pk != contact.identity_sign.as_slice()
            || bundle.identity.dh_pk != contact.identity_dh.as_slice()
        {
            eprintln!("[relay-client] bundle identity mismatch for contact {} — bundle from someone else?", contact.id);
            return Err(CoreError::State);
        }
        let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
        let mut empty_payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
        empty_payload.sender_name = self.outgoing_sender_name();
        let pt = pad_payload(&encode_payload(&empty_payload)?);
        let (state, init) = crypto::x3dh_initiate(&self.identity, &bundle, &pt, &ad)?;
        self.db.put_session(contact.id, &state.to_bytes()?)?;
        self.sessions.lock().await.insert(contact.id, state);
        self.session_created_at.lock().await.insert(contact.id, now_ms());

        let mut to = [0u8; 32];
        to.copy_from_slice(&contact.identity_sign);
        let envelope = EnvelopeBlob::X3dhInit(init);
        let blob = bincode::serialize(&envelope)?;
        out.send(ClientToRelay::Send { to, blob }).await.map_err(|_| CoreError::State)?;
        eprintln!("[relay-client] x3dh sent to contact {}", contact.id);
        Ok(())
    }

    async fn send_payload_via_relay(
        &self,
        contact: &gipny_libcore::db::Contact,
        payload: &mut WirePayload,
        out: &mpsc::Sender<ClientToRelay>,
    ) -> Result<()> {
        if payload.sender_name.is_none() {
            payload.sender_name = self.outgoing_sender_name();
        }
        let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
        let raw = encode_payload(payload)?;
        if raw.len() > MAX_PAYLOAD_BYTES {
            eprintln!("[relay-client] payload too large ({}B), dropping msg id={}", raw.len(), payload.origin_msg_id);
            if payload.origin_msg_id > 0 {
                let _ = self.db.mark_sent(payload.origin_msg_id as i64);
                let _ = self.events.try_send(CoreEvent::MessageSent { message_id: payload.origin_msg_id as i64 });
            }
            return Err(CoreError::State);
        }
        let pt = pad_payload(&raw);
        let (header, ct) = {
            let mut sess = self.sessions.lock().await;
            let state = sess.get_mut(&contact.id).ok_or(CoreError::State)?;
            let r = state.encrypt(&pt, &ad)?;
            self.db.put_session(contact.id, &state.to_bytes()?)?;
            r
        };
        let envelope = EnvelopeBlob::Ratchet { header, ciphertext: ct };
        let blob = bincode::serialize(&envelope)?;
        let mut to = [0u8; 32];
        to.copy_from_slice(&contact.identity_sign);
        out.send(ClientToRelay::Send { to, blob }).await.map_err(|_| CoreError::State)?;

        self.incoming_since_send.lock().await.insert(contact.id, 0);

        if payload.origin_msg_id > 0
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.callback_data.is_none()
            && payload.ack_for.is_none()
        {
            self.db.mark_sent(payload.origin_msg_id as i64)?;
            let _ = self.events.try_send(CoreEvent::MessageSent { message_id: payload.origin_msg_id as i64 });
        }
        Ok(())
    }

    fn build_payload_from_db(&self, msg: &gipny_libcore::db::Message) -> Result<WirePayload> {
        let atts = self.db.list_attachments(msg.id)?;
        let mut wire_atts = Vec::with_capacity(atts.len());
        for a in atts {
            let key = to_arr32(a.key.clone())?;
            let full = self.data_dir.join(ATTACHMENTS_DIR).join(&a.path);
            let enc = std::fs::read(&full)?;
            let data = AttachmentCipher::from_key(key).decrypt_chunk(0, &[], &enc)?;
            wire_atts.push(WireAttachment { name: a.name, data });
        }
        let ttl_ms = msg.expires_at.map(|e| e - msg.sent_at);
        let mut p = WirePayload::simple(msg.id as u64, msg.body.clone(), wire_atts, msg.sent_at, ttl_ms);
        if let Some(rt) = msg.reply_to {
            p.reply_to = self.build_wire_reply(rt)?;
        }
        Ok(p)
    }

    fn spawn_purge_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(PURGE_INTERVAL_SECS));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            tick.tick().await;
            let mut bundle_refresh = tokio::time::interval(Duration::from_secs(BUNDLE_REFRESH_SECS));
            bundle_refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
            bundle_refresh.tick().await;
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        let _ = this.db.purge_expired(now_ms());
                        let _ = this.db.purge_old_deferred_pins(now_ms() - 7 * 24 * 3600 * 1000);
                    }
                    _ = bundle_refresh.tick() => {
                        this.republish_bundle().await;
                    }
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    async fn republish_bundle(&self) {
        let _ = self.ensure_prekeys().await;
        if let Some(tx) = self.relay_out.read().await.clone() {
            if let Ok(b) = self.my_bundle() {
                if let Ok(bytes) = bincode::serialize(&b) {
                    let _ = tx.send(ClientToRelay::Publish { bundle: bytes }).await;
                }
            }
        }
    }

    fn spawn_update_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(UPDATE_CHECK_INITIAL_SECS)).await;
            loop {
                let _ = this.clone().check_and_emit_update().await;
                tokio::time::sleep(Duration::from_secs(UPDATE_CHECK_INTERVAL_SECS)).await;
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }
}

async fn ensure_group_from_wire(
    db: &Arc<Db>,
    events: &mpsc::Sender<CoreEvent>,
    identity: &Arc<Identity>,
    gref: &WireGroupRef,
) -> Result<()> {
    if db.get_group_name(&gref.id)?.is_none() {
        db.create_group(&gref.id, &gref.name)?;
    }
    let my_sign = identity.card().sign_pk;
    for m in &gref.members {
        let is_self = m.sign_pk.as_slice() == my_sign.as_slice();
        if db.is_group_member(&gref.id, &m.sign_pk)? { continue; }
        let name = if m.name.is_empty() { hex_short(&m.sign_pk) } else { m.name.clone() };
        db.add_group_member(&gref.id, &m.sign_pk, &m.dh_pk, &m.onion, &name, is_self)?;
        if !is_self && db.find_contact_by_identity(&m.dh_pk)?.is_none() {
            let _ = db.add_contact(&m.sign_pk, &m.dh_pk, &m.onion, &name)?;
        }
    }
    let _ = events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(&gref.id) });
    Ok(())
}

fn store_attachment_raw(data_dir: &PathBuf, data: &[u8]) -> Result<([u8; 32], String, u64)> {
    let cipher = AttachmentCipher::generate();
    let encrypted = cipher.encrypt_chunk(0, &[], data)?;
    let mut name = [0u8; 24];
    OsRng.fill_bytes(&mut name);
    let hex = to_hex(&name);
    let path = data_dir.join(ATTACHMENTS_DIR).join(&hex);
    std::fs::write(&path, &encrypted)?;
    Ok((*cipher.key(), hex, data.len() as u64))
}

fn build_ad(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    if a <= b { out.extend_from_slice(a); out.extend_from_slice(b); }
    else      { out.extend_from_slice(b); out.extend_from_slice(a); }
    out
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn make_typing_payload(group: Option<WireGroupRef>, typing: bool) -> WirePayload {
    WirePayload {
        origin_msg_id: 0, body: String::new(), attachments: vec![], sent_at: now_ms(),
        ttl_ms: None, group, buttons: None, callback_data: None,
        edit_of: None, pin: None, ack_for: None, sender_name: None,
        reply_to: None, typing: Some(typing), notify_sound: None,
    }
}

fn to_arr32(v: Vec<u8>) -> Result<[u8; 32]> {
    v.try_into().map(|a: Vec<u8>| {
        let mut out = [0u8; 32];
        out.copy_from_slice(&a);
        out
    }).map_err(|_| CoreError::State)
}

fn to_hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b { s.push_str(&format!("{:02x}", x)); }
    s
}

fn hex_short(b: &[u8]) -> String {
    let mut s = String::new();
    for x in &b[..8.min(b.len())] { s.push_str(&format!("{:02x}", x)); }
    s
}