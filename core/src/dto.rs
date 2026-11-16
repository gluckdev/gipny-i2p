//! Serialized IPC records and database-to-interface conversions.

use gipny_libcore::db::{Contact, Group, GroupMember, Message, RequestState, TrustLevel};
use gipny_libcore::WireConsole;
use serde::Serialize;

use crate::{err, hex};

#[derive(Serialize)]
pub(crate) struct VaultStatus {
    pub(crate) exists: bool,
    pub(crate) unlocked: bool,
}

#[derive(Serialize)]
pub(crate) struct ContactDto {
    pub(crate) id: i64,
    pub(crate) sign_pk: String,
    pub(crate) dh_pk: String,
    pub(crate) i2p_address: String,
    pub(crate) name: String,
    pub(crate) trust: u8,
    pub(crate) created_at: i64,
    pub(crate) last_seen: Option<i64>,
    pub(crate) is_bot: bool,
    pub(crate) pinned_at: Option<i64>,
    pub(crate) last_message_at: Option<i64>,
    /// Relay this contact receives through, from their card. `None` means this
    /// client's own relay setting is used, as it was before cards carried one.
    pub(crate) relay: Option<String>,
    /// This contact is in agent mode with us as master: its console is open.
    pub(crate) agent_granted: bool,
    /// "none", "incoming" (they asked, we have not answered) or "outgoing"
    /// (we added their card and have not heard back).
    pub(crate) request: &'static str,
}

impl From<Contact> for ContactDto {
    fn from(c: Contact) -> Self {
        Self {
            id: c.id,
            sign_pk: hex(&c.identity_sign),
            dh_pk: hex(&c.identity_dh),
            i2p_address: c.i2p_address,
            name: c.display_name,
            trust: match c.trust {
                TrustLevel::Unverified => 0,
                TrustLevel::Verified => 1,
                TrustLevel::Blocked => 2,
            },
            created_at: c.created_at,
            last_seen: c.last_seen,
            is_bot: c.is_bot,
            pinned_at: c.pinned_at,
            last_message_at: c.last_message_at,
            relay: c.relay_address,
            agent_granted: c.agent_granted,
            request: match c.request_state {
                RequestState::None => "none",
                RequestState::Incoming => "incoming",
                RequestState::Outgoing => "outgoing",
            },
        }
    }
}

#[derive(Serialize, Clone)]
pub(crate) struct ButtonDto {
    pub(crate) text: String,
    pub(crate) callback_data: String,
}

#[derive(Serialize)]
pub(crate) struct MessageDto {
    pub(crate) id: i64,
    pub(crate) contact_id: Option<i64>,
    pub(crate) group_id: Option<String>,
    pub(crate) sender_sign_pk: Option<String>,
    pub(crate) outgoing: bool,
    pub(crate) body: String,
    pub(crate) sent_at: i64,
    pub(crate) sent: bool,
    pub(crate) delivered: bool,
    pub(crate) read: bool,
    pub(crate) expires_at: Option<i64>,
    pub(crate) dm_seq: Option<i64>,
    pub(crate) deleted_at: Option<i64>,
    pub(crate) delete_pending: bool,
    pub(crate) deletion_notice: Option<gipny_libcore::db::DmDeletionNotice>,
    /// What the contact reported doing with this message (copy, save, screenshot).
    pub(crate) peer_events: Vec<gipny_libcore::db::PeerEvent>,
    pub(crate) buttons: Option<Vec<Vec<ButtonDto>>>,
    pub(crate) reply_to: Option<i64>,
    /// Console framing: a command, its output, or an agent-mode marker.
    pub(crate) console: Option<ConsoleDto>,
}

#[derive(Serialize)]
pub(crate) struct ConsoleDto {
    pub(crate) kind: u8,
    pub(crate) exit_code: Option<i32>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) truncated: bool,
}

/// Per-message extras kept beside the row: inline buttons and console frames.
pub(crate) fn attach_extras(
    db: &gipny_libcore::db::Db,
    dtos: &mut [MessageDto],
) -> Result<(), String> {
    let ids: Vec<i64> = dtos.iter().map(|d| d.id).collect();
    let map = db.load_buttons_batch(&ids).map_err(err)?;
    let consoles = db.load_settings_batch("console_", &ids).map_err(err)?;
    for dto in dtos.iter_mut() {
        dto.delete_pending =
            dto.deleted_at.is_some() && db.dm_delete_pending(dto.id).map_err(err)?;
        if dto.deleted_at.is_some() {
            dto.deletion_notice = db.dm_deletion_notice(dto.id).map_err(err)?;
        } else if dto.contact_id.is_some() && dto.group_id.is_none() {
            dto.peer_events = db.peer_events(dto.id).map_err(err)?;
        }
        if let Some(bytes) = map.get(&dto.id) {
            if let Ok(wire) = bincode::deserialize::<Vec<Vec<gipny_libcore::WireButton>>>(bytes) {
                dto.buttons = Some(
                    wire.into_iter()
                        .map(|row| {
                            row.into_iter()
                                .map(|b| ButtonDto {
                                    text: b.text,
                                    callback_data: b.callback_data,
                                })
                                .collect()
                        })
                        .collect(),
                );
            }
        }
        if let Some(bytes) = consoles.get(&dto.id) {
            if let Ok(c) = bincode::deserialize::<WireConsole>(bytes) {
                dto.console = Some(ConsoleDto {
                    kind: c.kind,
                    exit_code: c.exit_code,
                    duration_ms: c.duration_ms,
                    truncated: c.truncated,
                });
            }
        }
    }
    Ok(())
}

impl From<Message> for MessageDto {
    fn from(m: Message) -> Self {
        Self {
            id: m.id,
            contact_id: m.contact_id,
            group_id: m.group_id.as_ref().map(|v| hex(v)),
            sender_sign_pk: m.sender_sign_pk.as_ref().map(|v| hex(v)),
            outgoing: matches!(m.direction, gipny_libcore::db::Direction::Out),
            body: m.body,
            sent_at: m.sent_at,
            sent: m.sent,
            delivered: m.delivered,
            read: m.read,
            expires_at: m.expires_at,
            dm_seq: m.dm_seq,
            deleted_at: m.deleted_at,
            delete_pending: false,
            deletion_notice: None,
            peer_events: Vec::new(),
            buttons: None,
            reply_to: m.reply_to,
            console: None,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct GroupDto {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_at: i64,
    pub(crate) pinned_at: Option<i64>,
    pub(crate) last_message_at: Option<i64>,
}

impl From<Group> for GroupDto {
    fn from(g: Group) -> Self {
        Self {
            id: hex(&g.id),
            name: g.name,
            created_at: g.created_at,
            pinned_at: g.pinned_at,
            last_message_at: g.last_message_at,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct GroupMemberDto {
    pub(crate) sign_pk: String,
    pub(crate) dh_pk: String,
    pub(crate) i2p_address: String,
    pub(crate) name: String,
    pub(crate) is_self: bool,
}

impl From<GroupMember> for GroupMemberDto {
    fn from(m: GroupMember) -> Self {
        Self {
            sign_pk: hex(&m.sign_pk),
            dh_pk: hex(&m.dh_pk),
            i2p_address: m.i2p_address,
            name: m.display_name,
            is_self: m.is_self,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct BundleDto {
    pub(crate) sign_pk: String,
    pub(crate) dh_pk: String,
    pub(crate) signed_prekey: String,
    pub(crate) signed_prekey_sig: String,
    pub(crate) one_time_prekey: Option<String>,
    pub(crate) one_time_id: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct AttachmentDto {
    pub(crate) id: i64,
    pub(crate) message_id: i64,
    pub(crate) name: String,
    pub(crate) size: i64,
}

#[derive(Serialize)]
pub(crate) struct MediaItemDto {
    pub(crate) id: i64,
    pub(crate) message_id: i64,
    pub(crate) name: String,
    pub(crate) size: i64,
    pub(crate) sent_at: i64,
}

#[derive(Serialize)]
pub(crate) struct SearchHitDto {
    pub(crate) message: MessageDto,
    pub(crate) contact_id: Option<i64>,
    pub(crate) group_id: Option<String>,
    pub(crate) contact_name: Option<String>,
    pub(crate) group_name: Option<String>,
}

#[derive(serde::Serialize)]
pub(crate) struct FileProgressDto {
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) done: u32,
    pub(crate) total: u32,
    pub(crate) incoming: bool,
    pub(crate) status: String,
}

#[derive(Serialize)]
pub(crate) struct MessageFilesDto {
    pub(crate) attachments: Vec<AttachmentDto>,
    pub(crate) progress: Vec<FileProgressDto>,
}
