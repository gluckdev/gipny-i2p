use crate::{err, now_ms_helper, profile_dir, AppCtx};
use gipny_libcore::{crypto::AttachmentCipher, db::Db};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

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
    let backup: BackupV2<BackupAttachmentV3> = match version {
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
    }?;
    for attachment in &backup.attachments {
        attachment.validate()?;
    }
    Ok(backup)
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

impl BackupAttachmentV3 {
    fn validate(&self) -> Result<(), String> {
        let invalid = |reason: &str| format!("invalid attachment {}: {reason}", self.id);
        let key = self
            .key
            .as_slice()
            .try_into()
            .map_err(|_| invalid("invalid key"))?;
        let size = usize::try_from(self.size).map_err(|_| invalid("invalid size"))?;
        let chunk = match self.chunk_size {
            Some(n) => u32::try_from(n)
                .ok()
                .filter(|&n| n > 0)
                .ok_or_else(|| invalid("invalid chunk size"))? as usize,
            None => size.max(1),
        };
        // Even an empty file contains one authenticated chunk.
        const TAG: usize = 16;
        let parts = size.div_ceil(chunk).max(1);
        if parts
            .checked_mul(TAG)
            .and_then(|tags| size.checked_add(tags))
            != Some(self.bytes.len())
        {
            return Err(invalid("ciphertext length does not match its size"));
        }
        let cipher = AttachmentCipher::from_key(key);
        for (i, part) in self.bytes.chunks(chunk.saturating_add(TAG)).enumerate() {
            let _plain = zeroize::Zeroizing::new(
                cipher
                    .decrypt_chunk(i as u64, &[], part)
                    .map_err(|_| invalid("ciphertext authentication failed"))?,
            );
        }
        Ok(())
    }
}

pub(crate) fn export_raw_backup_data(db: &Db, profile: &Path) -> Result<Vec<u8>, String> {
    let mut settings = db.list_all_settings().map_err(err)?;
    settings.retain(|(key, _)| key != Db::DM_BACKUP_KEY && key != Db::DELIVERY_BACKUP_KEY);
    settings.push((Db::DM_BACKUP_KEY.into(), db.export_dm_state().map_err(err)?));
    settings.push((
        Db::DELIVERY_BACKUP_KEY.into(),
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
    let attach_dir = profile.canonicalize().map_err(err)?.join("attachments");
    for a in db.list_all_attachments().map_err(err)? {
        let read_error = |e| format!("cannot read attachment {} ({}): {e}", a.id, a.path);
        // Current rows store a filename. Older restores stored its absolute path.
        let path = attach_dir
            .join(&a.path)
            .canonicalize()
            .map_err(read_error)?;
        if path.parent() != Some(&attach_dir) {
            return Err(format!(
                "attachment {} is outside the profile attachment directory",
                a.id
            ));
        }
        let bytes = fs::read(path).map_err(read_error)?;
        let attachment = BackupAttachmentV3 {
            id: a.id,
            message_id: a.message_id,
            name: a.name,
            size: a.size,
            key: a.key,
            path: a.path,
            bytes,
            chunk_size: a.chunk_size,
        };
        attachment.validate()?;
        attachments.push(attachment);
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
    let _operation = ctx.begin_profile_operation()?;
    let dir = profile_dir(ctx, &profile)?;
    if vault_pass.len() < 8 {
        return Err("vault passphrase too short".into());
    }
    let backup = decode_backup(plain)?;
    with_staged_profile(&dir, |stage| {
        let vault = gipny_libcore::security::Vault::create(
            stage,
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
        let db = Db::open(&stage.join("data.db"), &mk).map_err(err)?;
        restore_contents(&db, stage, backup)
    })
}

/// Publish the entire closed database, vault and attachment tree with one rename.
/// The reserved staging parent has no vault, so list_profiles cannot expose it.
fn with_staged_profile(
    dir: &Path,
    populate: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    match fs::symlink_metadata(dir) {
        Ok(_) => return Err("profile already exists".into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(err(e)),
    }
    let parent = dir.parent().ok_or("invalid profile path")?;
    fs::create_dir_all(parent).map_err(err)?;
    let staging = parent.join(".restore");
    let builder = &mut fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&staging) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(&staging).map_err(err)?.is_dir() {
                return Err("invalid restore staging directory".into());
            }
        }
        result => result.map_err(err)?,
    }
    let nonce = u128::from_le_bytes(gipny_libcore::crypto::random_array());
    let stage = staging.join(format!("{nonce:032x}"));
    builder.create(&stage).map_err(err)?;
    let result = populate(&stage).and_then(|()| publish_profile(&stage, dir).map_err(err));
    if let Err(cause) = result {
        // Enrollment can succeed even when the subsequent Vault::create fails.
        // Keep its metadata available if the OS credential cannot be removed.
        match gipny_libcore::device_binding::destroy(&stage) {
            Ok(()) | Err(gipny_libcore::device_binding::BindingError::MissingCredential) => {}
            Err(e) => {
                return Err(format!(
                    "{cause}; credential rollback in {} failed: {e}",
                    stage.display()
                ))
            }
        }
        fs::remove_dir_all(&stage)
            .map_err(|e| format!("{cause}; rollback of {} failed: {e}", stage.display()))?;
        return Err(cause);
    }
    Ok(())
}

fn publish_profile(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(from)?.sync_all()?;
    // POSIX rename would replace an empty directory created during the import.
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let from = CString::new(from.as_os_str().as_bytes())?;
        let to = CString::new(to.as_os_str().as_bytes())?;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let rc = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_vendor = "apple")]
        let rc = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        }
        let wide = |path: &Path| {
            path.as_os_str()
                .encode_wide()
                .chain([0])
                .collect::<Vec<_>>()
        };
        let from = wide(&from.canonicalize()?);
        let parent = to
            .parent()
            .ok_or(io::ErrorKind::InvalidInput)?
            .canonicalize()?;
        let to = wide(&parent.join(to.file_name().ok_or(io::ErrorKind::InvalidInput)?));
        // No REPLACE_EXISTING flag, even if a competing creator left an empty directory.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple",
        windows
    )))]
    {
        Err(io::ErrorKind::Unsupported.into())
    }
}

fn restore_contents(
    db: &Db,
    dir: &Path,
    mut backup: BackupV2<BackupAttachmentV3>,
) -> Result<(), String> {
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
        let name = format!("att_{}", a.id);
        let write = || -> io::Result<()> {
            let mut file = fs::File::create_new(attach_dir.join(&name))?;
            file.write_all(&a.bytes)?;
            file.sync_all()
        };
        write().map_err(|e| format!("cannot restore attachment {}: {e}", a.id))?;
        attachments_db.push(gipny_libcore::db::Attachment {
            id: a.id,
            message_id: a.message_id,
            name: a.name,
            size: a.size,
            key: a.key,
            path: name,
            chunk_size: a.chunk_size,
        });
    }
    db.bulk_insert_attachments(&attachments_db).map_err(err)?;
    db.bulk_insert_pinned(&backup.pinned).map_err(err)?;
    db.restore_dm_state().map_err(err)?;
    db.restore_delivery_state().map_err(err)?;
    db.cleanup_deleted_files(&attach_dir).map_err(err)?;
    #[cfg(unix)]
    fs::File::open(&attach_dir)
        .and_then(|dir| dir.sync_all())
        .map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gipny_libcore::db::{Direction, NewAttachment};

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

    fn source(dir: &Path, content: &[u8], chunk_size: Option<u32>) -> (Db, std::path::PathBuf) {
        let db = Db::open_plain(&dir.join("source.db")).unwrap();
        db.set_setting("identity_sign", &[7; 32]).unwrap();
        db.create_group(b"group", "group").unwrap();
        let attach_dir = dir.join("attachments");
        fs::create_dir(&attach_dir).unwrap();
        let path = attach_dir.join("ciphertext");
        let cipher = AttachmentCipher::generate();
        if let Some(n) = chunk_size {
            gipny_libcore::files::seal_from(content, &path, &cipher, n).unwrap();
        } else {
            fs::write(&path, cipher.encrypt_chunk(0, &[], content).unwrap()).unwrap();
        }
        db.insert_group_message(
            b"group",
            Some(&[7; 32]),
            Direction::Out,
            "message",
            1,
            None,
            &[NewAttachment {
                name: "file".into(),
                path: "ciphertext".into(),
                key: cipher.key().to_vec(),
                size: content.len() as i64,
                chunk_size: chunk_size.map(i64::from),
            }],
        )
        .unwrap();
        (db, path)
    }

    #[test]
    fn export_reports_unreadable_or_corrupt_attachments() {
        let temp = tempfile::tempdir().unwrap();
        let (db, path) = source(temp.path(), b"parts", Some(2));
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(export_raw_backup_data(&db, temp.path())
            .unwrap_err()
            .contains("authentication failed"));
        fs::remove_file(path).unwrap();
        assert!(export_raw_backup_data(&db, temp.path())
            .unwrap_err()
            .contains("cannot read attachment 1"));
    }

    #[tokio::test]
    async fn restore_rejects_missing_bytes_and_damaged_ciphertext_including_empty_files() {
        let temp = tempfile::tempdir().unwrap();
        let (db, _) = source(temp.path(), b"", None);
        let raw = export_raw_backup_data(&db, temp.path()).unwrap();
        let destination = temp.path().join("profiles/alice");
        let ctx = AppCtx::new(temp.path().into());
        for missing in [true, false] {
            let mut backup = decode_backup(&raw).unwrap();
            let bytes = &mut backup.attachments[0].bytes;
            if missing {
                bytes.clear(); // Old exports used this for an unreadable file.
            } else {
                bytes[0] ^= 1;
            }
            let error = restore_raw_backup_data(
                &ctx,
                "alice".into(),
                "passphrase".into(),
                &bincode::serialize(&backup).unwrap(),
            )
            .await
            .unwrap_err();
            assert!(error.contains("invalid attachment 1"), "{error}");
            assert!(!destination.exists());
        }
        let backup = decode_backup(&raw).unwrap();
        with_staged_profile(&destination, |stage| {
            let db = Db::open_plain(&stage.join("data.db")).unwrap();
            restore_contents(&db, stage, backup)
        })
        .unwrap();
        assert_eq!(
            fs::read(destination.join("attachments/att_1"))
                .unwrap()
                .len(),
            16
        );
    }

    #[test]
    fn late_database_error_rolls_back_files_and_allows_retry() {
        let temp = tempfile::tempdir().unwrap();
        let (db, source_path) = source(temp.path(), b"several parts", Some(3));
        let raw = export_raw_backup_data(&db, temp.path()).unwrap();
        let destination = temp.path().join("profiles/alice");
        let mut broken = decode_backup(&raw).unwrap();
        broken
            .settings
            .iter_mut()
            .find(|(k, _)| k == Db::DELIVERY_BACKUP_KEY)
            .unwrap()
            .1
            .clear();
        let mut staged = None;
        with_staged_profile(&destination, |stage| {
            staged = Some(stage.to_owned());
            let db = Db::open_plain(&stage.join("data.db")).unwrap();
            let result = restore_contents(&db, stage, broken);
            assert_eq!(db.list_all_attachments().unwrap().len(), 1);
            assert!(stage.join("attachments/att_1").is_file());
            assert!(!destination.exists());
            result
        })
        .unwrap_err();
        assert!(!staged.unwrap().exists());
        assert!(!destination.exists());
        with_staged_profile(&destination, |stage| {
            let db = Db::open_plain(&stage.join("data.db")).unwrap();
            restore_contents(&db, stage, decode_backup(&raw)?)
        })
        .unwrap();
        let restored = Db::open_plain(&destination.join("data.db")).unwrap();
        let attachment = restored.get_attachment(1).unwrap().unwrap();
        let path = destination.join("attachments").join(attachment.path);
        assert_eq!(fs::read(&path).unwrap(), fs::read(source_path).unwrap());
        let bytes = gipny_libcore::files::read_attachment(
            &path,
            attachment.key.try_into().unwrap(),
            attachment.size as u64,
            attachment.chunk_size,
        )
        .unwrap();
        assert_eq!(bytes, b"several parts");
        let reexported =
            decode_backup(&export_raw_backup_data(&restored, &destination).unwrap()).unwrap();
        assert_eq!(
            reexported.attachments[0].bytes,
            decode_backup(&raw).unwrap().attachments[0].bytes
        );
    }

    #[test]
    fn export_supports_legacy_absolute_paths_inside_the_profile() {
        let temp = tempfile::tempdir().unwrap();
        let (db, path) = source(temp.path(), b"inline", None);
        let mut legacy = db.get_attachment(1).unwrap().unwrap();
        legacy.id = 2;
        legacy.path = path.to_str().unwrap().into();
        db.bulk_insert_attachments(&[legacy]).unwrap();
        let backup = decode_backup(&export_raw_backup_data(&db, temp.path()).unwrap()).unwrap();
        assert_eq!(backup.attachments.len(), 2);
        assert_eq!(backup.attachments[0].bytes, backup.attachments[1].bytes);
    }

    #[test]
    fn publication_refuses_a_destination_created_during_restore() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("alice");
        let mut staged = None;
        with_staged_profile(&destination, |stage| {
            staged = Some(stage.to_owned());
            fs::write(stage.join("vault.bin"), b"staged").unwrap();
            fs::create_dir(&destination).unwrap();
            Ok(())
        })
        .unwrap_err();
        assert!(destination.is_dir());
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
        assert!(!staged.unwrap().exists());
    }

    #[test]
    fn a_version_2_backup_still_restores_and_version_3_keeps_chunk_size() {
        let key = [4u8; 32];
        let bytes = AttachmentCipher::from_key(key)
            .encrypt_chunk(0, &[], b"abc")
            .unwrap();
        let v2 = BackupAttachment {
            id: 1,
            message_id: 2,
            name: "a".into(),
            size: 3,
            key: key.to_vec(),
            path: "p".into(),
            bytes: bytes.clone(),
        };
        let b = decode_backup(&bincode::serialize(&empty(2, vec![v2])).unwrap()).unwrap();
        assert_eq!(b.attachments[0].chunk_size, None);
        assert_eq!(b.attachments[0].bytes, bytes);
        let v3 = BackupAttachmentV3 {
            id: 1,
            message_id: 2,
            name: "a".into(),
            size: 3,
            key: key.to_vec(),
            path: "p".into(),
            bytes,
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
