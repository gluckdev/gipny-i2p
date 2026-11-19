use crate::{err, now_ms_helper, profile_dir, AppCtx};

#[derive(serde::Serialize, serde::Deserialize)]
/// Version 2 carried attachments sealed whole; version 3 also says how each
/// was sealed (`chunk_size`, files sent in parts). bincode is positional, so
/// the attachment type is the parameter and `version` (first, fixed width)
/// says which to read.
struct BackupV2<A = BackupAttachment> {
    version: u32,
    settings: Vec<(String, Vec<u8>)>,
    contacts: Vec<BackupContact>,
    groups: Vec<BackupGroup>,
    messages: Vec<BackupMessage>,
    attachments: Vec<A>,
    pinned: Vec<(Option<i64>, Option<Vec<u8>>, i64, i64)>,
    prekeys: Vec<BackupPreKey>,
    exported_at: i64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupContact {
    sign_pk: Vec<u8>,
    dh_pk: Vec<u8>,
    i2p_address: String,
    name: String,
    trust: i32,
    is_bot: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupGroup {
    id: Vec<u8>,
    name: String,
    members: Vec<BackupMember>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupMember {
    sign_pk: Vec<u8>,
    dh_pk: Vec<u8>,
    i2p_address: String,
    name: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupMessage {
    id: i64,
    contact_id: Option<i64>,
    group_id: Option<Vec<u8>>,
    sender_sign_pk: Option<Vec<u8>>,
    direction: i32,
    body: String,
    sent_at: i64,
    sent: bool,
    delivered: bool,
    read: bool,
    expires_at: Option<i64>,
    last_attempt_at: Option<i64>,
    send_attempts: i32,
    reply_to: Option<i64>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupAttachment {
    id: i64,
    message_id: i64,
    name: String,
    size: i64,
    key: Vec<u8>,
    path: String,
    bytes: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupAttachmentV3 {
    id: i64,
    message_id: i64,
    name: String,
    size: i64,
    key: Vec<u8>,
    path: String,
    bytes: Vec<u8>,
    chunk_size: Option<i64>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BackupPreKey {
    id: i64,
    kind: u8,
    private: Vec<u8>,
    public: Vec<u8>,
    created_at: i64,
}

/// A backup of either version, as version 3 (`version` is its first field).
fn decode_backup(plain: &[u8]) -> Result<BackupV2<BackupAttachmentV3>, String> {
    let version = plain
        .get(..4)
        .map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
        .unwrap_or(0);
    let unknown = || "backup format unknown / corrupted".to_string();
    match version {
        2 => {
            let b: BackupV2<BackupAttachment> =
                bincode::deserialize(plain).map_err(|_| unknown())?;
            Ok(BackupV2 {
                version: b.version,
                settings: b.settings,
                contacts: b.contacts,
                groups: b.groups,
                messages: b.messages,
                attachments: b.attachments.into_iter().map(Into::into).collect(),
                pinned: b.pinned,
                prekeys: b.prekeys,
                exported_at: b.exported_at,
            })
        }
        3 => bincode::deserialize(plain).map_err(|_| unknown()),
        v => Err(format!("unsupported backup version: {v}")),
    }
}

impl From<BackupAttachment> for BackupAttachmentV3 {
    fn from(a: BackupAttachment) -> Self {
        Self {
            id: a.id,
            message_id: a.message_id,
            name: a.name,
            size: a.size,
            key: a.key,
            path: a.path,
            bytes: a.bytes,
            chunk_size: None,
        }
    }
}

pub(crate) fn export_raw_backup_data(db: &gipny_libcore::db::Db) -> Result<Vec<u8>, String> {
    let mut settings = db.list_all_settings().map_err(err)?;
    settings.retain(|(key, _)| key != gipny_libcore::db::Db::DM_BACKUP_KEY && key != gipny_libcore::db::Db::DELIVERY_BACKUP_KEY);
    settings.push((
        gipny_libcore::db::Db::DM_BACKUP_KEY.into(),
        db.export_dm_state().map_err(err)?,
    ));
    settings.push((
        gipny_libcore::db::Db::DELIVERY_BACKUP_KEY.into(),
        db.export_delivery_state().map_err(err)?,
    ));
    let contacts = db
        .list_contacts()
        .map_err(err)?
        .into_iter()
        .map(|c| BackupContact {
            sign_pk: c.identity_sign,
            dh_pk: c.identity_dh,
            i2p_address: c.i2p_address,
            name: c.display_name,
            trust: match c.trust {
                gipny_libcore::db::TrustLevel::Unverified => 0,
                gipny_libcore::db::TrustLevel::Verified => 1,
                gipny_libcore::db::TrustLevel::Blocked => 2,
            },
            is_bot: c.is_bot,
        })
        .collect();
    let mut members_by_group = db.list_all_group_members().map_err(err)?;
    let groups = db
        .list_groups()
        .map_err(err)?
        .into_iter()
        .map(|g| {
            let members = members_by_group
                .remove(&g.id)
                .unwrap_or_default()
                .into_iter()
                .map(|m| BackupMember {
                    sign_pk: m.sign_pk,
                    dh_pk: m.dh_pk,
                    i2p_address: m.i2p_address,
                    name: m.display_name,
                })
                .collect();
            BackupGroup {
                id: g.id,
                name: g.name,
                members,
            }
        })
        .collect();
    let messages = db
        .list_all_messages()
        .map_err(err)?
        .into_iter()
        .map(|m| BackupMessage {
            id: m.id,
            contact_id: m.contact_id,
            group_id: m.group_id,
            sender_sign_pk: m.sender_sign_pk,
            direction: match m.direction {
                gipny_libcore::db::Direction::In => 0,
                gipny_libcore::db::Direction::Out => 1,
            },
            body: m.body,
            sent_at: m.sent_at,
            sent: m.sent,
            delivered: m.delivered,
            read: m.read,
            expires_at: m.expires_at,
            last_attempt_at: m.last_attempt_at,
            send_attempts: m.send_attempts as i32,
            reply_to: m.reply_to,
        })
        .collect();
    let mut attachments = Vec::new();
    for a in db.list_all_attachments().map_err(err)? {
        let bytes = std::fs::read(&a.path).unwrap_or_default();
        attachments.push(BackupAttachmentV3 {
            id: a.id,
            message_id: a.message_id,
            name: a.name,
            size: a.size,
            key: a.key,
            path: a.path,
            bytes,
            chunk_size: a.chunk_size,
        });
    }
    let pinned = db.list_all_pinned().map_err(err)?;
    let prekeys = db
        .list_all_prekeys()
        .map_err(err)?
        .into_iter()
        .map(|p| BackupPreKey {
            id: p.id,
            kind: p.kind as u8,
            private: p.private,
            public: p.public,
            created_at: p.created_at,
        })
        .collect();
    let backup = BackupV2 {
        version: 3,
        settings,
        contacts,
        groups,
        messages,
        attachments,
        pinned,
        prekeys,
        exported_at: now_ms_helper(),
    };
    bincode::serialize(&backup).map_err(err)
}

pub(crate) async fn restore_raw_backup_data(
    ctx: &AppCtx,
    profile: String,
    vault_pass: String,
    plain: &[u8],
) -> Result<(), String> {
    let dir = profile_dir(ctx, &profile)?;
    if vault_pass.len() < 8 {
        return Err("vault passphrase too short".into());
    }
    let mut backup = decode_backup(plain)?;
    if dir.exists() {
        return Err("profile already exists".into());
    }
    std::fs::create_dir_all(dir.parent().ok_or("invalid profile path")?).map_err(err)?;
    std::fs::create_dir(&dir).map_err(err)?;
    let vault = gipny_libcore::security::Vault::create(
        &dir,
        &vault_pass,
        None,
        gipny_libcore::security::DuressMode::Wipe,
        0,
    )
    .map_err(err)?;
    let mk = match vault.unlock(&vault_pass).map_err(err)? {
        gipny_libcore::security::UnlockOutcome::Primary(k) => k,
        _ => return Err("unexpected unlock outcome".into()),
    };
    let db = gipny_libcore::db::Db::open(&dir.join("data.db"), &mk).map_err(err)?;
    db.bulk_set_settings(&backup.settings).map_err(err)?;
    let identity_sign = backup
        .settings
        .iter()
        .find(|(k, _)| k == "identity_sign")
        .map(|(_, v)| v.clone())
        .ok_or("backup missing identity_sign")?;
    let mut contact_updates = Vec::with_capacity(backup.contacts.len());
    for c in &backup.contacts {
        let cid = db
            .add_contact(&c.sign_pk, &c.dh_pk, &c.i2p_address, &c.name, None)
            .map_err(err)?;
        let trust = match c.trust {
            1 => gipny_libcore::db::TrustLevel::Verified,
            2 => gipny_libcore::db::TrustLevel::Blocked,
            _ => gipny_libcore::db::TrustLevel::Unverified,
        };
        contact_updates.push((cid, c.name.clone(), trust, c.is_bot));
    }
    db.bulk_update_contacts(&contact_updates).map_err(err)?;
    if let Some(contact_map) = db.dm_backup_contact_map().map_err(err)? {
        for message in &mut backup.messages {
            if let Some(old) = message.contact_id {
                message.contact_id = Some(
                    *contact_map
                        .get(&old)
                        .ok_or("backup contact identity is missing")?,
                );
            }
        }
        for (contact_id, _, _, _) in &mut backup.pinned {
            if let Some(old) = *contact_id {
                *contact_id = Some(
                    *contact_map
                        .get(&old)
                        .ok_or("backup pin contact identity is missing")?,
                );
            }
        }
    }
    let mut members_flat = Vec::new();
    for g in &backup.groups {
        db.create_group(&g.id, &g.name).map_err(err)?;
        for m in &g.members {
            members_flat.push(gipny_libcore::db::GroupMember {
                group_id: g.id.clone(),
                sign_pk: m.sign_pk.clone(),
                dh_pk: m.dh_pk.clone(),
                i2p_address: m.i2p_address.clone(),
                display_name: m.name.clone(),
                is_self: m.sign_pk.as_slice() == identity_sign.as_slice(),
            });
        }
    }
    db.bulk_add_group_members(&members_flat).map_err(err)?;
    let prekeys = backup
        .prekeys
        .into_iter()
        .map(|p| gipny_libcore::db::PreKey {
            id: p.id,
            kind: match p.kind {
                1 => gipny_libcore::db::PreKeyKind::Signed,
                2 => gipny_libcore::db::PreKeyKind::OneTime,
                _ => gipny_libcore::db::PreKeyKind::Identity,
            },
            private: p.private,
            public: p.public,
            created_at: p.created_at,
        })
        .collect::<Vec<_>>();
    db.bulk_insert_prekeys(&prekeys).map_err(err)?;
    let messages = backup
        .messages
        .into_iter()
        .map(|m| gipny_libcore::db::Message {
            id: m.id,
            contact_id: m.contact_id,
            group_id: m.group_id,
            sender_sign_pk: m.sender_sign_pk,
            direction: if m.direction == 1 {
                gipny_libcore::db::Direction::Out
            } else {
                gipny_libcore::db::Direction::In
            },
            body: m.body,
            sent_at: m.sent_at,
            sent: m.sent,
            delivered: m.delivered,
            read: m.read,
            expires_at: m.expires_at,
            last_attempt_at: m.last_attempt_at,
            send_attempts: m.send_attempts as i64,
            reply_to: m.reply_to,
            delivery_expired_at: None,
            dm_seq: None,
            deleted_at: None,
        })
        .collect::<Vec<_>>();
    db.bulk_insert_messages(&messages).map_err(err)?;
    let attach_dir = dir.join("attachments");
    std::fs::create_dir_all(&attach_dir).map_err(err)?;
    let mut attachments_db = Vec::new();
    for a in backup.attachments {
        let new_path = attach_dir.join(format!("att_{}", a.id));
        if !a.bytes.is_empty() {
            std::fs::write(&new_path, &a.bytes).map_err(err)?;
        }
        attachments_db.push(gipny_libcore::db::Attachment {
            id: a.id,
            message_id: a.message_id,
            name: a.name,
            size: a.size,
            key: a.key,
            path: new_path.to_string_lossy().to_string(),
            chunk_size: a.chunk_size,
        });
    }
    db.bulk_insert_attachments(&attachments_db).map_err(err)?;
    db.bulk_insert_pinned(&backup.pinned).map_err(err)?;
    db.restore_dm_state().map_err(err)?;
    db.restore_delivery_state().map_err(err)?;
    db.cleanup_deleted_files(&attach_dir).map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty<A>(version: u32, attachments: Vec<A>) -> BackupV2<A> {
        BackupV2 {
            version,
            settings: vec![],
            contacts: vec![],
            groups: vec![],
            messages: vec![],
            attachments,
            pinned: vec![],
            prekeys: vec![],
            exported_at: 1,
        }
    }

    #[test]
    fn a_version_2_backup_still_restores_and_version_3_keeps_chunk_size() {
        let v2 = BackupAttachment {
            id: 1,
            message_id: 2,
            name: "a".into(),
            size: 3,
            key: vec![4],
            path: "p".into(),
            bytes: vec![5],
        };
        let b = decode_backup(&bincode::serialize(&empty(2, vec![v2])).unwrap()).unwrap();
        assert_eq!(b.attachments[0].chunk_size, None);
        assert_eq!(b.attachments[0].bytes, vec![5]);
        let v3 = BackupAttachmentV3 {
            id: 1,
            message_id: 2,
            name: "a".into(),
            size: 3,
            key: vec![4],
            path: "p".into(),
            bytes: vec![5],
            chunk_size: Some(196_608),
        };
        let b = decode_backup(&bincode::serialize(&empty(3, vec![v3])).unwrap()).unwrap();
        assert_eq!(b.attachments[0].chunk_size, Some(196_608));
        assert!(
            decode_backup(&bincode::serialize(&empty::<BackupAttachment>(9, vec![])).unwrap())
                .is_err()
        );
        assert!(decode_backup(b"").is_err());
    }
}
