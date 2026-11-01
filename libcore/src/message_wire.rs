// Shared native/WASM message codec. Keep field order and legacy versions intact.
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WirePayload {
    pub origin_msg_id: u64,
    pub body: String,
    pub attachments: Vec<WireAttachment>,
    pub sent_at: i64,
    pub ttl_ms: Option<i64>,
    pub group: Option<WireGroupRef>,
    #[serde(default)]
    pub buttons: Option<Vec<Vec<WireButton>>>,
    #[serde(default)]
    pub callback_data: Option<String>,
    #[serde(default)]
    pub edit_of: Option<u64>,
    #[serde(default)]
    pub pin: Option<WirePin>,
    #[serde(default)]
    pub ack_for: Option<u64>,
    #[serde(default)]
    pub sender_name: Option<String>,
    #[serde(default)]
    pub reply_to: Option<WireReply>,
    #[serde(default)]
    pub typing: Option<bool>,
    #[serde(default)]
    pub notify_sound: Option<String>,
    #[serde(default)]
    pub console: Option<WireConsole>,
    #[serde(default)]
    pub relay_address: Option<String>,
    /// "Forget me": the sender deleted this contact and asks the recipient to
    /// delete the conversation and the contact too. Only ever honoured from
    /// the contact itself, inside its ratchet session.
    #[serde(default)]
    pub wipe: Option<bool>,
    /// Files too large to ride inside one letter, named here and sent in parts
    /// on the same session ([`WireFileChunk`]); see `crate::files`.
    #[serde(default)]
    pub files: Vec<WireFileOffer>,
    /// One part of an offered file. A letter carrying it has no text and no
    /// `origin_msg_id`: it is not a message in the chat.
    #[serde(default)]
    pub file_chunk: Option<WireFileChunk>,
    /// What the recipient holds of a file so far.
    #[serde(default)]
    pub file_ack: Option<WireFileAck>,
    /// The sender stopped sending this file (or the recipient refused it).
    #[serde(default)]
    pub file_cancel: Option<[u8; 16]>,
    /// DM content/file identity or authenticated deletion control.
    #[serde(default)]
    pub dm: Option<WireDm>,
}

/// A trailing extension: old clients still read ordinary content and ignore
/// empty controls. They never acknowledge an operation they do not support.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct WireDm {
    /// 0: content/chunk identity, 1: delete, 2: deletion ACK.
    pub kind: u8,
    pub reference: crate::db::DmReference,
}

impl WireDm {
    pub fn is_control_payload(&self, payload:&WirePayload) -> bool {
        matches!(self.kind,1|2) && payload.group.is_none() && payload.origin_msg_id==0
            && payload.body.is_empty() && payload.attachments.is_empty() && payload.files.is_empty()
            && payload.file_chunk.is_none() && payload.file_ack.is_none() && payload.file_cancel.is_none()
            && payload.edit_of.is_none() && payload.wipe.is_none() && payload.console.is_none()
            && payload.callback_data.is_none() && payload.ack_for.is_none() && payload.typing.is_none()
            && payload.pin.is_none() && payload.reply_to.is_none() && payload.ttl_ms.is_none()
            && payload.buttons.is_none() && payload.notify_sound.is_none()
    }
}


#[derive(Clone, Serialize, Deserialize, Debug)]
struct WireV10 {
    pub origin_msg_id: u64,
    pub body: String,
    pub attachments: Vec<WireAttachment>,
    pub sent_at: i64,
    pub ttl_ms: Option<i64>,
    pub group: Option<WireGroupRef>,
    #[serde(default)]
    pub buttons: Option<Vec<Vec<WireButton>>>,
    #[serde(default)]
    pub callback_data: Option<String>,
    #[serde(default)]
    pub edit_of: Option<u64>,
    #[serde(default)]
    pub pin: Option<WirePin>,
    #[serde(default)]
    pub ack_for: Option<u64>,
    #[serde(default)]
    pub sender_name: Option<String>,
    #[serde(default)]
    pub reply_to: Option<WireReply>,
    #[serde(default)]
    pub typing: Option<bool>,
    #[serde(default)]
    pub notify_sound: Option<String>,
    #[serde(default)]
    pub console: Option<WireConsole>,
    #[serde(default)]
    pub relay_address: Option<String>,
    /// "Forget me": the sender deleted this contact and asks the recipient to
    /// delete the conversation and the contact too. Only ever honoured from
    /// the contact itself, inside its ratchet session.
    #[serde(default)]
    pub wipe: Option<bool>,
    /// Files too large to ride inside one letter, named here and sent in parts
    /// on the same session ([`WireFileChunk`]); see `crate::files`.
    #[serde(default)]
    pub files: Vec<WireFileOffer>,
    /// One part of an offered file. A letter carrying it has no text and no
    /// `origin_msg_id`: it is not a message in the chat.
    #[serde(default)]
    pub file_chunk: Option<WireFileChunk>,
    /// What the recipient holds of a file so far.
    #[serde(default)]
    pub file_ack: Option<WireFileAck>,
    /// The sender stopped sending this file (or the recipient refused it).
    #[serde(default)]
    pub file_cancel: Option<[u8; 16]>,
}

impl From<&WirePayload> for WireV10 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id.clone(),
            body: p.body.clone(),
            attachments: p.attachments.clone(),
            sent_at: p.sent_at.clone(),
            ttl_ms: p.ttl_ms.clone(),
            group: p.group.clone(),
            buttons: p.buttons.clone(),
            callback_data: p.callback_data.clone(),
            edit_of: p.edit_of.clone(),
            pin: p.pin.clone(),
            ack_for: p.ack_for.clone(),
            sender_name: p.sender_name.clone(),
            reply_to: p.reply_to.clone(),
            typing: p.typing.clone(),
            notify_sound: p.notify_sound.clone(),
            console: p.console.clone(),
            relay_address: p.relay_address.clone(),
            wipe: p.wipe.clone(),
            files: p.files.clone(),
            file_chunk: p.file_chunk.clone(),
            file_ack: p.file_ack.clone(),
            file_cancel: p.file_cancel.clone(),
        }
    }
}

impl From<WireV10> for WirePayload {
    fn from(p: WireV10) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id,
            body: p.body,
            attachments: p.attachments,
            sent_at: p.sent_at,
            ttl_ms: p.ttl_ms,
            group: p.group,
            buttons: p.buttons,
            callback_data: p.callback_data,
            edit_of: p.edit_of,
            pin: p.pin,
            ack_for: p.ack_for,
            sender_name: p.sender_name,
            reply_to: p.reply_to,
            typing: p.typing,
            notify_sound: p.notify_sound,
            console: p.console,
            relay_address: p.relay_address,
            wipe: p.wipe,
            files: p.files,
            file_chunk: p.file_chunk,
            file_ack: p.file_ack,
            file_cancel: p.file_cancel,
            dm: None,
        }
    }
}

/// A file sent in parts: what it is, so the recipient can take the parts in
/// any order and check the whole at the end.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct WireFileOffer {
    pub file_id: [u8; 16],
    pub name: String,
    pub size: u64,
    pub sha256: [u8; 32],
    /// Plaintext bytes per part; the last may be shorter.
    pub chunk_size: u32,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct WireFileChunk {
    pub file_id: [u8; 16],
    pub index: u32,
    pub data: Vec<u8>,
}

/// Cumulative and selective: every part below `received_up_to` is here, and
/// of those after it, `missing` are known lost (a later one arrived). Only
/// `missing` makes the sender resend quickly; see `crate::files`.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct WireFileAck {
    pub file_id: [u8; 16],
    pub received_up_to: u32,
    pub missing: Vec<u32>,
    /// One past the last part held: parts below it and not `missing` are
    /// there, so the sender knows what it put in flight has arrived.
    pub seen_to: u32,
    /// Sent by a recipient just started: whatever was in flight to it and is
    /// not held is gone, and goes again now rather than at its timeout.
    pub resumed: bool,
    /// The part this ack answers and its letter's `sent_at`, echoed: the
    /// sender measures the round trip of the very copy that arrived, resends
    /// included (as TCP timestamps do). Without it a resend could not be
    /// measured, and after a recipient restarted on a slower path every part
    /// timed out at the old timeout, went again, and was never measured.
    pub echo_index: u32,
    pub echo_sent_at: i64,
}

impl WirePayload {
    pub fn simple(origin: u64, body: String, attachments: Vec<WireAttachment>, sent_at: i64, ttl_ms: Option<i64>) -> Self {
        Self {
            origin_msg_id: origin, body, attachments, sent_at, ttl_ms,
            group: None, buttons: None, callback_data: None, edit_of: None, pin: None,
            ack_for: None, sender_name: None, reply_to: None,
            typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        }
    }
}

/// Agent-mode "console" marker on a message. Carried as an optional trailing
/// field so a client without this field simply sees the message's text body
/// (a short ASCII marker for control kinds, the command or its output otherwise)
/// and ignores the console framing. `kind` is a `u8`, not an enum, so an
/// unknown future kind decodes cleanly instead of failing the whole payload.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct WireConsole {
    /// See the `CONSOLE_*` constants.
    pub kind: u8,
    /// Process exit code for `CONSOLE_OUTPUT`; `None` if killed by signal/timeout.
    pub exit_code: Option<i32>,
    /// Wall-clock duration of the command for `CONSOLE_OUTPUT`.
    pub duration_ms: Option<u64>,
    /// Output was capped and trimmed.
    pub truncated: bool,
}

/// A command the master wants run on the agent.
pub const CONSOLE_COMMAND: u8 = 0;
/// The result of running a command, sent back to the master.
pub const CONSOLE_OUTPUT: u8 = 1;
/// The agent telling the master "you may drive my console" (agent mode on).
pub const CONSOLE_GRANT: u8 = 2;
/// The agent telling the master the console is closed (agent mode off).
pub const CONSOLE_REVOKE: u8 = 3;
/// The master telling the agent to switch agent mode off.
pub const CONSOLE_OFF: u8 = 4;

impl WireConsole {
    pub fn new(kind: u8) -> Self {
        Self { kind, exit_code: None, duration_ms: None, truncated: false }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WireAttachment { pub name: String, pub data: Vec<u8> }

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WireButton { pub text: String, pub callback_data: String }

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WireGroupRef { pub id: Vec<u8>, pub name: String, pub members: Vec<WireMember> }

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WireMember { pub sign_pk: Vec<u8>, pub dh_pk: Vec<u8>, pub onion: String, pub name: String }

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WirePin {
    pub sender_sign_pk: Vec<u8>,
    pub origin_msg_id: u64,
    pub unpin: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WireReply {
    pub sender_sign_pk: Vec<u8>,
    pub origin_msg_id: u64,
}

#[derive(Serialize, Deserialize)]
struct WireV0 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
}

#[derive(Serialize, Deserialize)]
struct WireV1 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct WireV2 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
}

#[derive(Serialize, Deserialize)]
struct WireV3 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct WireV4 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
    sender_name: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct WireV5 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
    sender_name: Option<String>,
    reply_to: Option<WireReply>,
}

/// Everything up to `wipe`: the full payload up to the files sent in parts.
#[derive(Serialize, Deserialize)]
struct WireV9 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
    sender_name: Option<String>,
    reply_to: Option<WireReply>,
    typing: Option<bool>,
    notify_sound: Option<String>,
    console: Option<WireConsole>,
    relay_address: Option<String>,
    wipe: Option<bool>,
}

impl From<&WirePayload> for WireV9 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(), reply_to: p.reply_to.clone(),
            typing: p.typing, notify_sound: p.notify_sound.clone(),
            console: p.console.clone(), relay_address: p.relay_address.clone(), wipe: p.wipe,
        }
    }
}

impl From<WireV9> for WirePayload {
    fn from(v: WireV9) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: v.reply_to, typing: v.typing, notify_sound: v.notify_sound,
            console: v.console, relay_address: v.relay_address, wipe: v.wipe,
            files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        }
    }
}

/// Everything before `wipe`: the full payload up to 0.4.13.
#[derive(Serialize, Deserialize)]
struct WireV8 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
    sender_name: Option<String>,
    reply_to: Option<WireReply>,
    typing: Option<bool>,
    notify_sound: Option<String>,
    console: Option<WireConsole>,
    relay_address: Option<String>,
}

impl From<&WirePayload> for WireV8 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(), reply_to: p.reply_to.clone(),
            typing: p.typing, notify_sound: p.notify_sound.clone(),
            console: p.console.clone(), relay_address: p.relay_address.clone(),
        }
    }
}

impl From<WireV8> for WirePayload {
    fn from(v: WireV8) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: v.reply_to, typing: v.typing, notify_sound: v.notify_sound,
            console: v.console, relay_address: v.relay_address, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct WireV7 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
    sender_name: Option<String>,
    reply_to: Option<WireReply>,
    typing: Option<bool>,
    notify_sound: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct WireV6 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
    sender_name: Option<String>,
    reply_to: Option<WireReply>,
    typing: Option<bool>,
}

impl From<&WirePayload> for WireV6 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(), reply_to: p.reply_to.clone(),
            typing: p.typing,
        }
    }
}

impl From<&WirePayload> for WireV7 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(), reply_to: p.reply_to.clone(),
            typing: p.typing, notify_sound: p.notify_sound.clone(),
        }
    }
}

impl From<WireV7> for WirePayload {
    fn from(v: WireV7) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: v.reply_to, typing: v.typing, notify_sound: v.notify_sound, console: None,
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        }
    }
}

impl From<WireV6> for WirePayload {
    fn from(v: WireV6) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: v.reply_to, typing: v.typing, notify_sound: None, console: None,
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        }
    }
}

impl From<&WirePayload> for WireV5 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(), reply_to: p.reply_to.clone(),
        }
    }
}

impl From<&WirePayload> for WireV4 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(),
        }
    }
}

impl From<&WirePayload> for WireV3 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
        }
    }
}

impl From<&WirePayload> for WireV2 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(),
        }
    }
}

impl From<&WirePayload> for WireV1 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
        }
    }
}

impl From<WireV5> for WirePayload {
    fn from(v: WireV5) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: v.reply_to, typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        }
    }
}

impl From<WireV4> for WirePayload {
    fn from(v: WireV4) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        }
    }
}

impl From<WireV3> for WirePayload {
    fn from(v: WireV3) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for,
            sender_name: None, reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        }
    }
}

impl From<WireV2> for WirePayload {
    fn from(v: WireV2) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin,
            ack_for: None, sender_name: None, reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        }
    }
}

impl From<WireV1> for WirePayload {
    fn from(v: WireV1) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: None, pin: None, ack_for: None, sender_name: None, reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        }
    }
}

impl From<WireV0> for WirePayload {
    fn from(v: WireV0) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: None, callback_data: None,
            edit_of: None, pin: None, ack_for: None, sender_name: None, reply_to: None, typing: None, notify_sound: None, console: None,
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        }
    }
}

pub fn encode_payload(p: &WirePayload) -> std::result::Result<Vec<u8>, bincode::Error> {
    let has_files = !p.files.is_empty() || p.file_chunk.is_some() || p.file_ack.is_some() || p.file_cancel.is_some();
    if p.dm.is_some()                                    { bincode::serialize(p) }
    else if has_files                                   { bincode::serialize(&WireV10::from(p)) }
    else if p.wipe.is_some()                             { bincode::serialize(&WireV9::from(p)) }
    else if p.console.is_some() || p.relay_address.is_some() { bincode::serialize(&WireV8::from(p)) }
    else if p.notify_sound.is_some()                    { bincode::serialize(&WireV7::from(p)) }
    else if p.typing.is_some()                     { bincode::serialize(&WireV6::from(p)) }
    else if p.reply_to.is_some()                   { bincode::serialize(&WireV5::from(p)) }
    else if p.sender_name.is_some()                { bincode::serialize(&WireV4::from(p)) }
    else if p.ack_for.is_some()                    { bincode::serialize(&WireV3::from(p)) }
    else if p.edit_of.is_some() || p.pin.is_some() { bincode::serialize(&WireV2::from(p)) }
    else                                           { bincode::serialize(&WireV1::from(p)) }
}

pub fn decode_payload(pt: &[u8]) -> std::result::Result<WirePayload, bincode::Error> {
    if let Ok(v) = bincode::deserialize::<WirePayload>(pt) { return Ok(v); }
    if let Ok(v) = bincode::deserialize::<WireV10>(pt)     { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV9>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV8>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV7>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV6>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV5>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV4>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV3>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV2>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV1>(pt)      { return Ok(v.into()); }
    Ok(bincode::deserialize::<WireV0>(pt)?.into())
}

#[cfg(test)]
mod dm_wire_tests {
    use super::*;

    #[test]
    fn deletion_extension_roundtrips_and_old_readers_see_no_chat_message() {
        let mut payload=WirePayload::simple(0,String::new(),vec![],300,None);
        payload.dm=Some(WireDm {kind:1,reference:crate::db::DmReference {author:[2;32],origin:44,sent_at:100}});
        let bytes=encode_payload(&payload).unwrap();
        assert_eq!(decode_payload(&bytes).unwrap().dm,payload.dm);
        let old=bincode::deserialize::<WireV10>(&bytes).unwrap();
        assert_eq!(old.origin_msg_id,0);
        assert!(old.body.is_empty() && old.attachments.is_empty() && old.files.is_empty());
        assert!(old.ack_for.is_none());
        assert!(payload.dm.as_ref().unwrap().is_control_payload(&payload));
        payload.edit_of=Some(1);
        assert!(!payload.dm.as_ref().unwrap().is_control_payload(&payload));
    }

    #[test]
    fn historical_chunk_payload_remains_decodable_without_dm_extension() {
        let mut payload=WirePayload::simple(0,String::new(),vec![],300,None);
        payload.file_chunk=Some(WireFileChunk {file_id:[3;16],index:0,data:vec![7;32]});
        let historical=bincode::serialize(&WireV10::from(&payload)).unwrap();
        let decoded=decode_payload(&historical).unwrap();
        assert_eq!(decoded.file_chunk,payload.file_chunk);
        assert!(decoded.dm.is_none());
    }
}

const PADDING_BUCKETS: &[usize] = &[
    256, 1024, 4096, 16_384, 65_536, 262_144, 1_048_576, 4_194_304, 16_777_216,
];

pub fn pad_payload(pt: &[u8]) -> Vec<u8> {
    let with_header = 4 + pt.len();
    let bucket = PADDING_BUCKETS.iter().copied().find(|&b| b >= with_header).unwrap_or(with_header);
    let mut out = Vec::with_capacity(bucket);
    out.extend_from_slice(&(pt.len() as u32).to_be_bytes());
    out.extend_from_slice(pt);
    if bucket > with_header {
        let mut pad = vec![0u8; bucket - with_header];
        crate::crypto::fill_random(&mut pad);
        out.extend_from_slice(&pad);
    }
    out
}

/// As [`pad_payload`], but the padding is optional.
///
/// The length prefix is written either way, so the receiver does not care and
/// does not need to be told: an unpadded payload is simply one whose bucket
/// happened to equal its length, and `unpad_payload` reads it unchanged. That
/// is what lets the fast lanes drop padding without a protocol change and
/// without breaking anyone running an older build.
pub fn pack_payload(pt: &[u8], padded: bool) -> Vec<u8> {
    if padded {
        return pad_payload(pt);
    }
    let mut out = Vec::with_capacity(4 + pt.len());
    out.extend_from_slice(&(pt.len() as u32).to_be_bytes());
    out.extend_from_slice(pt);
    out
}

pub fn unpad_payload(padded: &[u8]) -> Option<Vec<u8>> {
    if padded.len() < 4 { return None; }
    let len = u32::from_be_bytes(padded[0..4].try_into().ok()?) as usize;
    if 4 + len > padded.len() { return None; }
    Some(padded[4..4 + len].to_vec())
}

