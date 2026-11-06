//! Publish messages and their file/control metadata in one transaction. Disk
//! preparation happens before this boundary; no network work runs under it.
use super::*;
use crate::files::{self, Received};
use crate::session::{WirePayload, CONSOLE_COMMAND};

pub struct NewOutgoingDm<'a> {
    pub contact_id: i64,
    pub body: &'a str,
    pub sent_at: i64,
    pub expires_at: Option<i64>,
    pub reply_to: Option<i64>,
    pub buttons: Option<&'a [u8]>,
    pub notify_sound: Option<&'a [u8]>,
    pub console: Option<&'a [u8]>,
}

#[derive(Debug)]
pub struct IncomingCommit {
    pub id: i64,
    pub inserted: bool,
    pub pending: bool,
    pub deleted: bool,
}

impl Db {
    pub fn commit_outgoing_dm(
        &self,
        message: &NewOutgoingDm<'_>,
        attachments: &[NewGroupAttachment],
    ) -> Result<i64> {
        check_body(message.body)?;
        if attachments.len() > files::MAX_MESSAGE_FILES {
            return Err(DbError::TooLarge);
        }
        for staged in attachments {
            if staged.attachment.chunk_size.is_some() != staged.file_offer.is_some() {
                return Err(DbError::State);
            }
        }
        self.with_tx(|tx| {
            let allowed: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM contacts
                WHERE id=?1 AND trust!=2 AND request_state!=1)",
                params![message.contact_id],
                |r| r.get(0),
            )?;
            if !allowed {
                return Err(DbError::State);
            }
            tx.execute(
                "INSERT INTO messages(contact_id,direction,body,sent_at,expires_at,reply_to)
                VALUES(?1,1,?2,?3,?4,?5)",
                params![
                    message.contact_id,
                    message.body,
                    message.sent_at,
                    message.expires_at,
                    message.reply_to
                ],
            )?;
            let id = tx.last_insert_rowid();
            for (prefix, value) in [
                ("buttons", message.buttons),
                ("sound", message.notify_sound),
                ("console", message.console),
            ] {
                if let Some(value) = value {
                    tx.execute(
                        "INSERT INTO settings(k,v) VALUES(?1,?2)",
                        params![format!("{prefix}_{id}"), value],
                    )?;
                }
            }
            for staged in attachments {
                insert_attachments(tx, id, std::slice::from_ref(&staged.attachment))?;
                let attachment_id = tx.last_insert_rowid();
                if let Some((file_id, sha256)) = staged.file_offer {
                    tx.execute(
                        "INSERT INTO file_out(file_id,attachment_id,message_id,sha256,created_at)
                        VALUES(?1,?2,?3,?4,?5)",
                        params![
                            file_id.as_slice(),
                            attachment_id,
                            id,
                            sha256.as_slice(),
                            message.sent_at
                        ],
                    )?;
                    tx.execute(
                        "INSERT INTO file_out_peer(file_id,contact_id) VALUES(?1,?2)",
                        params![file_id.as_slice(), message.contact_id],
                    )?;
                }
            }
            Ok(id)
        })
    }

    /// Atomically keep a new incoming message, inline files, multipart offers,
    /// reply and control metadata. Pending contacts keep bounded offer metadata
    /// without creating a multipart file or receiving any parts. Their consent
    /// is rechecked by apply_file_chunk, including after restart.
    pub fn commit_incoming_message(
        &self,
        contact_id: i64,
        payload: &WirePayload,
        attachments: &[NewAttachment],
    ) -> Result<IncomingCommit> {
        check_body(&payload.body)?;
        files::validate_message_files(payload)?;
        if attachments.len() != payload.attachments.len()
            || attachments.iter().any(|a| a.chunk_size.is_some())
        {
            return Err(DbError::State);
        }
        let origin = i64::try_from(payload.origin_msg_id).map_err(|_| DbError::State)?;
        let buttons = payload
            .buttons
            .as_ref()
            .map(bincode::serialize)
            .transpose()
            .map_err(|_| DbError::State)?;
        let console = payload
            .console
            .as_ref()
            .map(bincode::serialize)
            .transpose()
            .map_err(|_| DbError::State)?;
        self.with_tx(|tx| {
            let (sender, trust, request): (Vec<u8>, i64, i64) = tx.query_row(
                "SELECT identity_sign,trust,request_state FROM contacts WHERE id=?1", params![contact_id],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let pending = request == RequestState::Incoming.to_i64();
            if trust == TrustLevel::Blocked.to_i64() || (pending && !files::is_request_content(payload)) {
                return Err(DbError::State);
            }
            let group = payload.group.as_ref().map(|g| g.id.as_slice());
            let existing: Option<(i64, bool)> = if origin > 0 {
                match group {
                    Some(gid) => tx.query_row("SELECT id,deleted_at IS NOT NULL FROM messages
                        WHERE group_id=?1 AND sender_sign_pk=?2 AND origin_msg_id=?3 ORDER BY id DESC LIMIT 1",
                        params![gid,sender,origin], |r| Ok((r.get(0)?,r.get(1)?))).optional()?,
                    None => tx.query_row("SELECT id,deleted_at IS NOT NULL FROM messages
                        WHERE contact_id=?1 AND group_id IS NULL AND direction=0 AND origin_msg_id=?2 ORDER BY id DESC LIMIT 1",
                        params![contact_id,origin], |r| Ok((r.get(0)?,r.get(1)?))).optional()?,
                }
            } else { None };
            if let Some((id, deleted)) = existing {
                // New records are already complete. Replays never add files to
                // an old/deleted message, reset keys, or re-run a command.
                return Ok(IncomingCommit { id, inserted: false, pending, deleted });
            }
            let expires_at = payload.ttl_ms.map(|ttl| payload.sent_at.saturating_add(ttl));
            if group.is_none() {
                let deleted: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM dm_tombstones
                    WHERE contact_id=?1 AND author=?2 AND origin=?3 AND sent_at=?4)",
                    params![contact_id,sender,origin,payload.sent_at], |r| r.get(0))?;
                if deleted { return Err(DbError::State); }
            }
            let reply = if let Some(reply) = &payload.reply_to {
                let reply_origin = i64::try_from(reply.origin_msg_id).map_err(|_| DbError::State)?;
                match group {
                    Some(gid) => tx.query_row("SELECT id FROM messages WHERE group_id=?1 AND sender_sign_pk=?2
                        AND ((direction=0 AND origin_msg_id=?3) OR (direction=1 AND id=?3)) ORDER BY id DESC LIMIT 1",
                        params![gid,reply.sender_sign_pk,reply_origin], |r| r.get::<_,i64>(0)).optional()?,
                    None => tx.query_row("SELECT id FROM messages WHERE contact_id=?1 AND group_id IS NULL AND sender_sign_pk=?2
                        AND ((direction=0 AND origin_msg_id=?3) OR (direction=1 AND id=?3)) ORDER BY id DESC LIMIT 1",
                        params![contact_id,reply.sender_sign_pk,reply_origin], |r| r.get::<_,i64>(0)).optional()?,
                }
            } else { None };
            tx.execute("INSERT INTO messages(contact_id,group_id,sender_sign_pk,direction,body,sent_at,expires_at,origin_msg_id,reply_to)
                VALUES(?1,?2,?3,0,?4,?5,?6,?7,?8)", params![group.is_none().then_some(contact_id),group,sender,
                payload.body,payload.sent_at,expires_at,origin,reply])?;
            let id = tx.last_insert_rowid();
            insert_attachments(tx, id, attachments)?;
            for (prefix, value) in [("buttons", buttons.as_deref()), ("console", console.as_deref()),
                ("sound", payload.notify_sound.as_ref().map(|s| s.as_bytes()))] {
                if let Some(value) = value {
                    tx.execute("INSERT INTO settings(k,v) VALUES(?1,?2)", params![format!("{prefix}_{id}"),value])?;
                }
            }
            if group.is_none() && payload.console.as_ref().is_some_and(|c| c.kind == CONSOLE_COMMAND) {
                tx.execute("INSERT INTO settings(k,v) VALUES(?1,?2)", params![format!("console_pending_{id}"),b"1".as_slice()])?;
            }
            let (total, peer): (i64, i64) = tx.query_row("SELECT COUNT(*),COALESCE(SUM(contact_id=?1),0) FROM file_in",
                params![contact_id], |r| Ok((r.get(0)?,r.get(1)?)))?;
            let (mut total, mut peer) = (total as usize, peer as usize);
            for offer in &payload.files {
                // A file id belongs to one message. Completed/cancelled files
                // cannot be resurrected with a new origin id.
                let used: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM file_in WHERE contact_id=?1 AND file_id=?2
                    UNION ALL SELECT 1 FROM file_completed WHERE contact_id=?1 AND file_id=?2
                    UNION ALL SELECT 1 FROM dm_deleted_files WHERE contact_id=?1 AND file_id=?2)",
                    params![contact_id,offer.file_id.as_slice()], |r| r.get(0))?;
                if used { return Err(DbError::State); }
                if total >= files::MAX_INCOMING_FILES || peer >= files::MAX_INCOMING_PER_CONTACT {
                    return Err(DbError::TooLarge);
                }
                let total_parts = files::chunk_count(offer.size, offer.chunk_size);
                let cipher = crate::crypto::AttachmentCipher::generate();
                let name: [u8;24] = crate::crypto::random_array();
                tx.execute("INSERT INTO file_in(file_id,contact_id,message_id,name,size,sha256,chunk_size,path,key,bits,created_at)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![offer.file_id.as_slice(),contact_id,id,offer.name,
                    offer.size as i64,offer.sha256.as_slice(),offer.chunk_size,hex(&name),cipher.key().as_slice(),Received::new(total_parts).bits(),now_ms()])?;
                if group.is_none() {
                    let mismatched_early: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM dm_early_files
                        WHERE contact_id=?1 AND file_id=?2 AND (author!=?3 OR origin!=?4 OR sent_at!=?5))",
                        params![contact_id,offer.file_id.as_slice(),sender,origin,payload.sent_at], |r| r.get(0))?;
                    if mismatched_early {
                        tx.execute("DELETE FROM file_early WHERE contact_id=?1 AND file_id=?2", params![contact_id,offer.file_id.as_slice()])?;
                    }
                    tx.execute("INSERT INTO dm_message_files(message_id,contact_id,file_id) VALUES(?1,?2,?3)",
                        params![id,contact_id,offer.file_id.as_slice()])?;
                    tx.execute("DELETE FROM dm_early_files WHERE contact_id=?1 AND file_id=?2", params![contact_id,offer.file_id.as_slice()])?;
                }
                total += 1;
                peer += 1;
            }
            Ok(IncomingCommit { id, inserted: true, pending, deleted: false })
        })
    }

    pub fn can_receive_files(&self, contact_id: i64) -> Result<bool> {
        self.with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM contacts
            WHERE id=?1 AND trust!=2 AND request_state!=1)",
                params![contact_id],
                |r| r.get(0),
            )?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::StagedAttachments;
    use crate::session::{WireAttachment, WireConsole, WireFileChunk, WireFileOffer};
    use sha2::{Digest, Sha256};

    fn database(path: &Path) -> (Db, i64) {
        let db = Db::open_plain(path).unwrap();
        db.configure_dm_identity(&[1; 32]).unwrap();
        let peer = db
            .add_contact(&[2; 32], &[3; 32], "", "peer", None)
            .unwrap();
        (db, peer)
    }

    fn incoming(body: &str) -> WirePayload {
        let mut payload = WirePayload::simple(77, body.into(), vec![], 1_800_000_000_000, None);
        payload.files.push(WireFileOffer {
            file_id: [4; 16],
            name: "document.bin".into(),
            size: 3,
            sha256: Sha256::digest(b"abc").into(),
            chunk_size: files::CHUNK_SIZE,
        });
        payload
    }

    fn count(db: &Db, table: &str) -> i64 {
        db.with_conn(|conn| {
            Ok(conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
        })
        .unwrap()
    }

    #[test]
    fn pending_file_only_and_text_file_survive_reopen_without_receiving_before_consent() {
        for body in ["", "text and a file"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("receiver.db");
            let attachment_dir = dir.path().join("attachments");
            std::fs::create_dir(&attachment_dir).unwrap();
            let (db, peer) = database(&path);
            db.set_contact_request_state(peer, RequestState::Incoming)
                .unwrap();
            let payload = incoming(body);
            let saved = db.commit_incoming_message(peer, &payload, &[]).unwrap();
            assert!(saved.inserted && saved.pending);
            assert_eq!(db.get_message(saved.id).unwrap().unwrap().body, body);
            let chunk = WireFileChunk {
                file_id: [4; 16],
                index: 0,
                data: b"abc".to_vec(),
            };
            assert!(db
                .apply_file_chunk(peer, &chunk, None, &attachment_dir)
                .unwrap()
                .is_none());
            assert_eq!(count(&db, "file_early"), 0);
            assert_eq!(std::fs::read_dir(&attachment_dir).unwrap().count(), 0);
            let key = db.file_in(&chunk.file_id, peer).unwrap().unwrap().key;
            let duplicate = db.commit_incoming_message(peer, &payload, &[]).unwrap();
            assert!(!duplicate.inserted);
            assert_eq!(duplicate.id, saved.id);
            assert_eq!(db.file_in(&chunk.file_id, peer).unwrap().unwrap().key, key);
            drop(db);
            let db = Db::open_plain(&path).unwrap();
            assert!(!db.can_receive_files(peer).unwrap());
            db.set_contact_request_state(peer, RequestState::None)
                .unwrap();
            let update = db
                .apply_file_chunk(peer, &chunk, None, &attachment_dir)
                .unwrap()
                .unwrap();
            assert!(update.received.complete() && !update.broken);
            let attachment = db.list_attachments(saved.id).unwrap().remove(0);
            let bytes = files::read_attachment(
                &attachment_dir.join(&attachment.path),
                attachment.key.try_into().unwrap(),
                attachment.size as u64,
                attachment.chunk_size,
            )
            .unwrap();
            assert_eq!(bytes, b"abc");
            assert!(
                !db.commit_incoming_message(peer, &payload, &[])
                    .unwrap()
                    .inserted
            );
            assert!(
                db.file_in(&chunk.file_id, peer).unwrap().is_none(),
                "replay cannot restart a completed transfer"
            );
        }
    }

    #[test]
    fn incoming_failure_at_each_table_rolls_back_message_offers_settings_and_staging() {
        for table in [
            "messages",
            "attachments",
            "settings",
            "file_in",
            "dm_message_files",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("receiver.db");
            let (db, peer) = database(&path);
            let attachment_dir = dir.path().join("attachments");
            let mut payload = incoming("atomic");
            payload.attachments.push(WireAttachment {
                name: "small".into(),
                data: b"inline".to_vec(),
            });
            payload.console = Some(WireConsole::new(CONSOLE_COMMAND));
            let injector = Connection::open(&path).unwrap();
            injector
                .execute_batch(&format!(
                    "CREATE TRIGGER injected BEFORE INSERT ON {table}
                BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;"
                ))
                .unwrap();
            drop(injector);
            let mut staged = StagedAttachments::new(&attachment_dir).unwrap();
            staged.inline("small", b"inline").unwrap();
            assert!(
                staged.publish_incoming(&db, peer, &payload).is_err(),
                "{table}"
            );
            for table in ["messages", "attachments", "file_in", "dm_message_files"] {
                assert_eq!(count(&db, table), 0);
            }
            assert_eq!(std::fs::read_dir(&attachment_dir).unwrap().count(), 0);
            db.with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER injected")?;
                Ok(())
            })
            .unwrap();
            // Retry the same origin after recovery; it must create a complete message.
            let mut staged = StagedAttachments::new(&attachment_dir).unwrap();
            staged.inline("small", b"inline").unwrap();
            let saved = staged.publish_incoming(&db, peer, &payload).unwrap();
            assert!(saved.inserted);
            assert_eq!(db.files_in_for_message(saved.id).unwrap().len(), 1);
            assert_eq!(
                db.get_setting(&format!("console_pending_{}", saved.id))
                    .unwrap(),
                Some(b"1".to_vec())
            );
        }
    }

    #[test]
    fn outgoing_failure_at_each_table_never_publishes_partial_message() {
        for table in [
            "messages",
            "attachments",
            "settings",
            "file_out",
            "file_out_peer",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("sender.db");
            let (db, peer) = database(&path);
            let attachment_dir = dir.path().join("attachments");
            let injector = Connection::open(&path).unwrap();
            injector
                .execute_batch(&format!(
                    "CREATE TRIGGER injected BEFORE INSERT ON {table}
                BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;"
                ))
                .unwrap();
            drop(injector);
            let draft = NewOutgoingDm {
                contact_id: peer,
                body: "atomic",
                sent_at: 42,
                expires_at: None,
                reply_to: None,
                buttons: Some(b"buttons"),
                notify_sound: Some(b"ping"),
                console: Some(b"console"),
            };
            let mut staged = StagedAttachments::new(&attachment_dir).unwrap();
            staged.inline("small", b"inline").unwrap();
            staged.parts("large", &b"parts"[..]).unwrap();
            assert!(staged.publish_dm(&db, &draft).is_err(), "{table}");
            for table in ["messages", "attachments", "file_out", "file_out_peer"] {
                assert_eq!(count(&db, table), 0);
            }
            assert_eq!(std::fs::read_dir(&attachment_dir).unwrap().count(), 0);
            db.with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER injected")?;
                Ok(())
            })
            .unwrap();
            let mut staged = StagedAttachments::new(&attachment_dir).unwrap();
            staged.parts("large", &b"parts"[..]).unwrap();
            let offer = staged.offers().remove(0);
            let id = staged.publish_dm(&db, &draft).unwrap();
            drop(db);
            let db = Db::open_plain(&path).unwrap();
            assert_eq!(
                db.files_out_for_message(id).unwrap()[0].file_id,
                offer.file_id
            );
            assert_eq!(db.file_peers_of(&offer.file_id).unwrap().len(), 1);
            assert_eq!(
                db.get_setting(&format!("console_{id}")).unwrap(),
                Some(b"console".to_vec())
            );
        }
    }

    #[test]
    fn deleted_and_cancelled_files_cannot_be_resurrected_or_attached_to_another_message() {
        let dir = tempfile::tempdir().unwrap();
        let (db, peer) = database(&dir.path().join("receiver.db"));
        let mut payload = incoming("delete");
        let saved = db.commit_incoming_message(peer, &payload, &[]).unwrap();
        db.delete_dm_message(saved.id).unwrap();
        let replay = db.commit_incoming_message(peer, &payload, &[]).unwrap();
        assert!(replay.deleted && !replay.inserted);
        assert!(db.files_in_for_message(saved.id).unwrap().is_empty());
        payload.origin_msg_id += 1;
        assert!(db.commit_incoming_message(peer, &payload, &[]).is_err());
        assert_eq!(count(&db, "messages"), 1);
    }

    #[test]
    fn pending_offer_quota_and_blocking_are_enforced_in_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let (db, peer) = database(&dir.path().join("receiver.db"));
        db.set_contact_request_state(peer, RequestState::Incoming)
            .unwrap();
        for n in 0..files::MAX_INCOMING_PER_CONTACT {
            let mut payload = incoming("");
            payload.origin_msg_id += n as u64;
            payload.files[0].file_id = [n as u8; 16];
            db.commit_incoming_message(peer, &payload, &[]).unwrap();
        }
        let mut excess = incoming("");
        excess.origin_msg_id = 100;
        excess.files[0].file_id = [99; 16];
        assert!(matches!(
            db.commit_incoming_message(peer, &excess, &[]),
            Err(DbError::TooLarge)
        ));
        assert_eq!(
            count(&db, "messages"),
            files::MAX_INCOMING_PER_CONTACT as i64
        );
        db.update_contact(peer, "peer", TrustLevel::Blocked)
            .unwrap();
        assert!(!db.can_receive_files(peer).unwrap());
        let chunk = WireFileChunk {
            file_id: [0; 16],
            index: 0,
            data: b"abc".to_vec(),
        };
        assert!(db
            .apply_file_chunk(peer, &chunk, None, dir.path())
            .unwrap()
            .is_none());
        db.delete_contact(peer).unwrap();
        assert_eq!(count(&db, "file_in"), 0);
    }
}
