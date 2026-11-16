use super::*;
use crate::files::{Flow, Sending};
use crate::session::WireFileAck;

#[derive(Default)]
pub struct FileBatch {
    pub due: Vec<u32>,
    pub next_before: u32,
    pub gave_up: bool,
}

impl Db {
    pub fn failed_files_for_message(&self, message_id: i64) -> Result<Vec<(String, u64, String)>> {
        self.with_conn(|conn| {
            conn.prepare_cached("SELECT name,size,status FROM file_failures WHERE message_id=?1")?
                .query_map(params![message_id], |row| {
                    Ok((row.get(0)?, row.get::<_, i64>(1)? as u64, row.get(2)?))
                })?
                .collect_rows()
        })
    }

    pub fn cancel_incoming_file(&self, file_id: &[u8; 16], contact_id: i64) -> Result<Option<i64>> {
        let _files = self.file_mutations.lock().map_err(|_| DbError::State)?;
        self.with_tx(|tx| {
            let message:Option<i64>=tx.query_row("SELECT message_id FROM file_in WHERE file_id=?1 AND contact_id=?2",
                params![file_id.as_slice(),contact_id],|r|r.get(0)).optional()?;
            if message.is_some() {
                tx.execute("INSERT OR REPLACE INTO file_failures(file_id,contact_id,message_id,name,size,status)
                    SELECT file_id,contact_id,message_id,name,size,'cancelled' FROM file_in WHERE file_id=?1 AND contact_id=?2",
                    params![file_id.as_slice(),contact_id])?;
                tx.execute("INSERT OR IGNORE INTO deleted_file_cleanup(path) SELECT path FROM file_in WHERE file_id=?1 AND contact_id=?2",
                    params![file_id.as_slice(),contact_id])?;
                tx.execute("INSERT OR IGNORE INTO dm_deleted_files(contact_id,file_id) VALUES(?1,?2)",params![contact_id,file_id.as_slice()])?;
                tx.execute("DELETE FROM file_in WHERE file_id=?1 AND contact_id=?2",params![file_id.as_slice(),contact_id])?;
            }
            Ok(message)
        })
    }

    /// Always edit the latest row while holding the DB transaction. Callers
    /// hold Flow before DB, and release both before any network await.
    fn mutate_file_peer<T>(
        &self,
        file_id: &[u8; 16],
        contact_id: i64,
        update: impl FnOnce(&mut FilePeer) -> T,
    ) -> Result<Option<T>> {
        self.with_tx(|tx| {
            let Some(mut peer)=tx.query_row("SELECT file_id,contact_id,next,acked,resend,last_sent_at,last_ack_at,failed
                FROM file_out_peer WHERE file_id=?1 AND contact_id=?2",
                params![file_id.as_slice(),contact_id],Self::map_file_peer).optional()? else {return Ok(None)};
            let result=update(&mut peer);
            tx.execute("UPDATE file_out_peer SET next=?3,acked=?4,resend=?5,last_sent_at=?6,last_ack_at=?7,failed=?8
                WHERE file_id=?1 AND contact_id=?2",params![file_id.as_slice(),contact_id,peer.next,peer.acked,
                u32s_blob(&peer.resend),peer.last_sent_at,peer.last_ack_at,peer.failed as i64])?;
            Ok(Some(result))
        })
    }

    /// Reserve next before sending: a fast ACK must see the reserved indices.
    pub fn reserve_file_batch(
        &self,
        file_id: &[u8; 16],
        contact_id: i64,
        total: u32,
        flow: &mut Flow,
        now: i64,
        heard: Option<i64>,
    ) -> Result<Option<FileBatch>> {
        self.mutate_file_peer(file_id, contact_id, |peer| {
            if peer.failed || peer.acked >= total {
                return FileBatch::default();
            }
            let mut sending = Sending {
                next: peer.next,
                acked: peer.acked,
                resend: peer.resend.clone(),
            };
            if let Some(sent) = peer.last_sent_at {
                let alive = heard.is_some_and(|at| at > sent);
                if peer.last_ack_at.is_none() && alive && now - sent > 24 * 3600 * 1000 {
                    peer.failed = true;
                    flow.forget(*file_id);
                    return FileBatch {
                        gave_up: true,
                        ..FileBatch::default()
                    };
                }
                if sending.resend.is_empty()
                    && sending.next > sending.acked
                    && alive
                    && peer.last_ack_at.is_none_or(|at| at < sent)
                    && now - sent > 30 * 60 * 1000
                {
                    sending.rewind(*file_id, flow);
                }
            }
            let next_before = sending.next;
            let due = sending.due(*file_id, total, flow, now);
            peer.next = sending.next;
            peer.resend = sending.resend;
            if !due.is_empty() {
                peer.last_sent_at = Some(now);
            }
            FileBatch {
                due,
                next_before,
                gave_up: false,
            }
        })
    }

    pub fn acknowledge_file_parts(
        &self,
        contact_id: i64,
        ack: &WireFileAck,
        total: u32,
        flow: &mut Flow,
        now: i64,
    ) -> Result<Option<u32>> {
        if ack.received_up_to > total
            || ack.seen_to > total
            || ack.received_up_to > ack.seen_to
            || ack.missing.len() > 64
            || ack
                .missing
                .iter()
                .any(|index| *index < ack.received_up_to || *index >= ack.seen_to)
        {
            return Err(DbError::State);
        }
        self.mutate_file_peer(&ack.file_id, contact_id, |peer| {
            let mut sending = Sending {
                next: peer.next,
                acked: peer.acked,
                resend: peer.resend.clone(),
            };
            sending.on_ack(ack, flow, now);
            peer.acked = sending.acked;
            peer.resend = sending.resend;
            peer.last_ack_at = Some(now);
            if peer.acked >= total {
                flow.forget(ack.file_id);
            }
            peer.acked
        })
    }

    /// A failed write may race an ACK; it must never restore an old watermark.
    pub fn retry_file_parts(
        &self,
        file_id: &[u8; 16],
        contact_id: i64,
        indices: &[u32],
        flow: &mut Flow,
    ) -> Result<()> {
        for index in indices {
            flow.unsent(*file_id, *index);
        }
        self.mutate_file_peer(file_id, contact_id, |peer| {
            for index in indices {
                if *index >= peer.acked && *index < peer.next {
                    peer.resend.push(*index);
                }
            }
            peer.resend.sort_unstable();
            peer.resend.dedup();
        })?;
        Ok(())
    }

    pub fn fail_file_peer(&self, file_id: &[u8; 16], contact_id: i64) -> Result<bool> {
        Ok(self
            .mutate_file_peer(file_id, contact_id, |peer| peer.failed = true)?
            .is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn completed_file_repeats_final_receipt_after_reopen_without_duplicate_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let db = Db::open_plain(&path).unwrap();
        db.configure_dm_identity(&[1; 32]).unwrap();
        let peer = db
            .add_contact(&[2; 32], &[3; 32], "relay", "peer", None)
            .unwrap();
        let message = db
            .insert_message_with_origin(peer, Direction::In, "", 100, None, &[], Some(7))
            .unwrap();
        let reference = db.dm_reference(message).unwrap();
        let data = b"file";
        let fin = FileIn {
            file_id: [9; 16],
            contact_id: peer,
            message_id: message,
            name: "file.bin".into(),
            size: 4,
            sha256: Sha256::digest(data).into(),
            chunk_size: 4,
            path: "sealed".into(),
            key: vec![5; 32],
            bits: vec![0],
            received: 0,
            created_at: 100,
        };
        db.file_in_add(&fin).unwrap();
        let chunk = crate::session::WireFileChunk {
            file_id: fin.file_id,
            index: 0,
            data: data.to_vec(),
        };
        let first = db
            .apply_file_chunk(peer, &chunk, Some(&reference), dir.path())
            .unwrap()
            .unwrap();
        assert!(first.received.complete());
        assert!(first.attachment_id.is_some());
        assert!(db.file_in(&fin.file_id, peer).unwrap().is_none());
        drop(db);
        let db = Db::open_plain(&path).unwrap();
        let repeated = db
            .apply_file_chunk(peer, &chunk, Some(&reference), dir.path())
            .unwrap()
            .unwrap();
        assert!(repeated.received.complete());
        assert!(!repeated.fresh);
        assert_eq!(db.list_attachments(message).unwrap().len(), 1);
        assert!(
            !db.file_in_add(&fin).unwrap(),
            "duplicate offer cannot restart a finished file"
        );
        db.delete_dm_message(message).unwrap();
        assert!(db
            .apply_file_chunk(peer, &chunk, Some(&reference), dir.path())
            .unwrap()
            .is_none());
    }

    #[test]
    fn fast_ack_and_late_send_failure_never_regress_reserved_progress() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("db")).unwrap();
        let peer = db
            .add_contact(&[1; 32], &[2; 32], "relay", "peer", None)
            .unwrap();
        let attachment = NewAttachment {
            name: "file".into(),
            size: 1200,
            key: vec![3; 32],
            path: "sealed".into(),
            chunk_size: Some(300),
        };
        let id = db
            .insert_message(peer, Direction::Out, "", 100, None, &[attachment])
            .unwrap();
        let file = [9; 16];
        db.file_out_add(
            &file,
            db.list_attachments(id).unwrap()[0].id,
            id,
            &[4; 32],
            &[peer],
        )
        .unwrap();
        let mut flow = Flow::default();
        let batch = db
            .reserve_file_batch(&file, peer, 4, &mut flow, 1000, None)
            .unwrap()
            .unwrap();
        assert_eq!(batch.due, vec![0, 1, 2, 3]);
        assert_eq!(db.file_peer(&file, peer).unwrap().unwrap().next, 4);
        // An ACK arrives while the async send function is suspended.
        let ack = WireFileAck {
            file_id: file,
            received_up_to: 4,
            seen_to: 4,
            missing: vec![],
            resumed: false,
            echo_index: 3,
            echo_sent_at: 1000,
        };
        assert_eq!(
            db.acknowledge_file_parts(peer, &ack, 4, &mut flow, 1100)
                .unwrap(),
            Some(4)
        );
        // The suspended task then reports a write failure for its old batch.
        db.retry_file_parts(&file, peer, &batch.due, &mut flow)
            .unwrap();
        let current = db.file_peer(&file, peer).unwrap().unwrap();
        assert_eq!(current.acked, 4);
        assert_eq!(current.last_ack_at, Some(1100));
        assert!(current.resend.is_empty());
        assert!(db.file_peers_for(peer).unwrap().is_empty());
    }
}

impl Db {
    // --- Files sent in parts (crate::files) -------------------------------

    /// Offer a file (sealed in parts as `attachment_id`) to `recipients`.
    pub fn file_out_add(
        &self,
        file_id: &[u8; 16],
        attachment_id: i64,
        message_id: i64,
        sha256: &[u8; 32],
        recipients: &[i64],
    ) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO file_out (file_id, attachment_id, message_id, sha256, created_at)
                 SELECT ?1,?2,?3,?4,?5 WHERE EXISTS(SELECT 1 FROM messages WHERE id=?3 AND deleted_at IS NULL)
                    AND EXISTS(SELECT 1 FROM attachments WHERE id=?2 AND message_id=?3)",
                params![file_id.as_slice(), attachment_id, message_id, sha256.as_slice(), now_ms()])?;
            for c in recipients {
                tx.execute("INSERT OR IGNORE INTO file_out_peer (file_id, contact_id)
                    SELECT ?1,?2 WHERE EXISTS(SELECT 1 FROM file_out WHERE file_id=?1)", params![file_id.as_slice(), c])?;
                tx.execute("INSERT OR IGNORE INTO dm_message_files(message_id,contact_id,file_id)
                    SELECT id,?2,?3 FROM messages WHERE id=?1 AND contact_id=?2 AND group_id IS NULL AND deleted_at IS NULL",
                    params![message_id,c,file_id.as_slice()])?;
            }
            Ok(())
        })
    }

    fn map_file_out(r: &Row<'_>) -> rusqlite::Result<FileOut> {
        Ok(FileOut {
            file_id: blob16(r.get(0)?)?,
            message_id: r.get(1)?,
            sha256: blob32(r.get(2)?)?,
            cancelled: r.get::<_, i64>(3)? != 0,
            attachment: Attachment {
                id: r.get(4)?,
                message_id: r.get(5)?,
                name: r.get(6)?,
                size: r.get(7)?,
                key: r.get(8)?,
                path: r.get(9)?,
                chunk_size: r.get(10)?,
            },
        })
    }

    /// The files a message offers, for (re)building its letter.
    pub fn files_out_for_message(&self, message_id: i64) -> Result<Vec<FileOut>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT f.file_id, f.message_id, f.sha256, f.cancelled,
                    a.id, a.message_id, a.name, a.size, a.key, a.path, a.chunk_size
             FROM file_out f JOIN attachments a ON a.id = f.attachment_id WHERE f.message_id = ?1 ORDER BY a.id")?
            .query_map(params![message_id], Self::map_file_out)?.collect_rows())
    }

    pub fn file_out(&self, file_id: &[u8; 16]) -> Result<Option<FileOut>> {
        self.with_conn(|c| {
            c.prepare_cached(
                "SELECT f.file_id, f.message_id, f.sha256, f.cancelled,
                    a.id, a.message_id, a.name, a.size, a.key, a.path, a.chunk_size
             FROM file_out f JOIN attachments a ON a.id = f.attachment_id WHERE f.file_id = ?1",
            )?
            .query_row(params![file_id.as_slice()], Self::map_file_out)
            .optional()
            .map_err(Into::into)
        })
    }

    pub(super) fn map_file_peer(r: &Row<'_>) -> rusqlite::Result<FilePeer> {
        Ok(FilePeer {
            file_id: blob16(r.get(0)?)?,
            contact_id: r.get(1)?,
            next: r.get(2)?,
            acked: r.get(3)?,
            resend: u32s(r.get(4)?),
            last_sent_at: r.get(5)?,
            last_ack_at: r.get(6)?,
            failed: r.get::<_, i64>(7)? != 0,
        })
    }

    /// Files still going to `contact_id`, oldest first.
    pub fn file_peers_for(&self, contact_id: i64) -> Result<Vec<FilePeer>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT p.file_id, p.contact_id, p.next, p.acked, p.resend, p.last_sent_at, p.last_ack_at, p.failed
             FROM file_out_peer p JOIN file_out f ON f.file_id = p.file_id
             JOIN attachments a ON a.id = f.attachment_id
             WHERE p.contact_id = ?1 AND p.failed = 0 AND f.cancelled = 0
               AND p.acked < ((a.size + a.chunk_size - 1) / a.chunk_size)
             ORDER BY f.created_at")?
            .query_map(params![contact_id], Self::map_file_peer)?.collect_rows())
    }

    pub fn file_peer(&self, file_id: &[u8; 16], contact_id: i64) -> Result<Option<FilePeer>> {
        self.with_conn(|c| {
            c.prepare_cached(
                "SELECT file_id, contact_id, next, acked, resend, last_sent_at, last_ack_at, failed
             FROM file_out_peer WHERE file_id = ?1 AND contact_id = ?2",
            )?
            .query_row(params![file_id.as_slice(), contact_id], Self::map_file_peer)
            .optional()
            .map_err(Into::into)
        })
    }

    /// All recipients of one file, for its progress.
    pub fn file_peers_of(&self, file_id: &[u8; 16]) -> Result<Vec<FilePeer>> {
        self.with_conn(|c| {
            c.prepare_cached(
                "SELECT file_id, contact_id, next, acked, resend, last_sent_at, last_ack_at, failed
             FROM file_out_peer WHERE file_id = ?1",
            )?
            .query_map(params![file_id.as_slice()], Self::map_file_peer)?
            .collect_rows()
        })
    }

    #[cfg(test)]
    pub fn file_peer_save(&self, p: &FilePeer) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "UPDATE file_out_peer SET next = ?3, acked = ?4, resend = ?5, last_sent_at = ?6, last_ack_at = ?7, failed = ?8
             WHERE file_id = ?1 AND contact_id = ?2",
            params![p.file_id.as_slice(), p.contact_id, p.next, p.acked, u32s_blob(&p.resend),
                    p.last_sent_at, p.last_ack_at, p.failed as i64])?; Ok(()) })
    }

    pub fn file_out_cancel(&self, file_id: &[u8; 16]) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE file_out SET cancelled = 1 WHERE file_id = ?1",
                params![file_id.as_slice()],
            )?;
            Ok(())
        })
    }

    pub fn file_in_add(&self, f: &FileIn) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "INSERT OR IGNORE INTO file_in (file_id, contact_id, message_id, name, size, sha256, chunk_size, path, key, bits, received, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
             WHERE EXISTS(SELECT 1 FROM messages WHERE id=?3 AND deleted_at IS NULL)
                AND NOT EXISTS(SELECT 1 FROM dm_deleted_files WHERE contact_id=?2 AND file_id=?1)
                AND NOT EXISTS(SELECT 1 FROM file_completed WHERE contact_id=?2 AND file_id=?1)",
            params![f.file_id.as_slice(), f.contact_id, f.message_id, f.name, f.size as i64, f.sha256.as_slice(),
                    f.chunk_size, f.path, f.key, f.bits, f.received, f.created_at])? > 0))
    }

    pub(super) fn map_file_in(r: &Row<'_>) -> rusqlite::Result<FileIn> {
        Ok(FileIn {
            file_id: blob16(r.get(0)?)?,
            contact_id: r.get(1)?,
            message_id: r.get(2)?,
            name: r.get(3)?,
            size: r.get::<_, i64>(4)? as u64,
            sha256: blob32(r.get(5)?)?,
            chunk_size: r.get(6)?,
            path: r.get(7)?,
            key: r.get(8)?,
            bits: r.get(9)?,
            received: r.get(10)?,
            created_at: r.get(11)?,
        })
    }

    pub fn file_in(&self, file_id: &[u8; 16], contact_id: i64) -> Result<Option<FileIn>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT file_id, contact_id, message_id, name, size, sha256, chunk_size, path, key, bits, received, created_at
             FROM file_in WHERE file_id = ?1 AND contact_id = ?2")?
            .query_row(params![file_id.as_slice(), contact_id], Self::map_file_in).optional().map_err(Into::into))
    }

    /// Files still coming in for a message, for its progress.
    pub fn files_in_for_message(&self, message_id: i64) -> Result<Vec<FileIn>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT file_id, contact_id, message_id, name, size, sha256, chunk_size, path, key, bits, received, created_at
             FROM file_in WHERE message_id = ?1")?
            .query_map(params![message_id], Self::map_file_in)?.collect_rows())
    }

    pub fn file_in_save(
        &self,
        file_id: &[u8; 16],
        contact_id: i64,
        bits: &[u8],
        received: u32,
    ) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
            "UPDATE file_in SET bits = ?3, received = ?4 WHERE file_id = ?1 AND contact_id = ?2",
            params![file_id.as_slice(), contact_id, bits, received])?;
            Ok(())
        })
    }

    /// The file is whole: it becomes an attachment of its message, and the
    /// transfer is forgotten. Returns the attachment's id.
    pub fn file_in_finish(&self, f: &FileIn) -> Result<i64> {
        let _files = self.file_mutations.lock().map_err(|_| DbError::State)?;
        self.with_tx(|tx| {
            let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM file_in WHERE file_id=?1 AND contact_id=?2
                AND EXISTS(SELECT 1 FROM messages WHERE id=file_in.message_id AND deleted_at IS NULL))",
                params![f.file_id.as_slice(),f.contact_id],|r|r.get(0))?;
            if !live {return Err(DbError::State);}
            tx.execute(
                "INSERT INTO attachments (message_id, name, size, key, path, chunk_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![f.message_id, f.name, f.size as i64, f.key, f.path, f.chunk_size])?;
            let id = tx.last_insert_rowid();
            tx.execute("DELETE FROM file_in WHERE file_id = ?1 AND contact_id = ?2", params![f.file_id.as_slice(), f.contact_id])?;
            Ok(id)
        })
    }

    pub fn file_in_delete(&self, file_id: &[u8; 16], contact_id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM file_in WHERE file_id = ?1 AND contact_id = ?2",
                params![file_id.as_slice(), contact_id],
            )?;
            Ok(())
        })
    }

    /// Transfers begun before `older_than` and never finished.
    pub fn files_in_stale(&self, older_than: i64) -> Result<Vec<FileIn>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT file_id, contact_id, message_id, name, size, sha256, chunk_size, path, key, bits, received, created_at
             FROM file_in WHERE created_at < ?1")?
            .query_map(params![older_than], Self::map_file_in)?.collect_rows())
    }

    /// A part that came before its offer.
    pub fn file_early_put(
        &self,
        file_id: &[u8; 16],
        contact_id: i64,
        index: u32,
        data: &[u8],
    ) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT OR IGNORE INTO file_early (file_id, contact_id, idx, data, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![file_id.as_slice(), contact_id, index, data, now_ms()])?; Ok(()) })
    }

    /// The parts set aside for a file whose offer has now come; forgotten here.
    pub fn file_early_take(
        &self,
        file_id: &[u8; 16],
        contact_id: i64,
    ) -> Result<Vec<(u32, Vec<u8>)>> {
        self.with_tx(|tx| {
            let rows: Vec<(u32, Vec<u8>)> = tx.prepare_cached(
                "SELECT idx, data FROM file_early WHERE file_id = ?1 AND contact_id = ?2 ORDER BY idx")?
                .query_map(params![file_id.as_slice(), contact_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect_rows()?;
            tx.execute("DELETE FROM file_early WHERE file_id = ?1 AND contact_id = ?2", params![file_id.as_slice(), contact_id])?;
            Ok(rows)
        })
    }

    pub fn file_early_gc(&self, older_than: i64) -> Result<usize> {
        self.with_conn(|c| {
            Ok(c.execute("DELETE FROM file_early WHERE at < ?1", params![older_than])?)
        })
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;

    #[test]
    fn a_file_in_parts_round_trips_through_the_tables() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("t.db")).unwrap();
        let bob = db.add_contact(&[1; 32], &[2; 32], "", "bob", None).unwrap();
        let att = NewAttachment {
            name: "big.bin".into(),
            size: 1000,
            key: vec![3; 32],
            path: "p".into(),
            chunk_size: Some(300),
        };
        let msg = db
            .insert_message(bob, Direction::Out, "here", now_ms(), None, &[att])
            .unwrap();
        let aid = db.list_attachments(msg).unwrap()[0].id;
        let fid = [9u8; 16];
        db.file_out_add(&fid, aid, msg, &[7; 32], &[bob]).unwrap();

        let offers = db.files_out_for_message(msg).unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].attachment.chunk_size, Some(300));
        assert_eq!(offers[0].sha256, [7; 32]);

        let mut peer = db
            .file_peers_for(bob)
            .unwrap()
            .pop()
            .expect("pending for bob");
        peer.next = 4;
        peer.acked = 2;
        peer.resend = vec![2, 3];
        db.file_peer_save(&peer).unwrap();
        assert_eq!(db.file_peer(&fid, bob).unwrap().unwrap().resend, vec![2, 3]);
        // All four parts acknowledged: nothing left for bob.
        peer.acked = 4;
        db.file_peer_save(&peer).unwrap();
        assert!(db.file_peers_for(bob).unwrap().is_empty());

        // Receiving side, with a part that came before the offer.
        db.file_early_put(&fid, bob, 1, b"early").unwrap();
        let fin = FileIn {
            file_id: fid,
            contact_id: bob,
            message_id: msg,
            name: "big.bin".into(),
            size: 1000,
            sha256: [7; 32],
            chunk_size: 300,
            path: "in".into(),
            key: vec![4; 32],
            bits: vec![0],
            received: 0,
            created_at: now_ms(),
        };
        assert!(db.file_in_add(&fin).unwrap());
        assert!(
            !db.file_in_add(&fin).unwrap(),
            "a repeated offer changes nothing"
        );
        assert_eq!(
            db.file_early_take(&fid, bob).unwrap(),
            vec![(1, b"early".to_vec())]
        );
        assert!(db.file_early_take(&fid, bob).unwrap().is_empty());
        db.file_in_save(&fid, bob, &[0b1111], 4).unwrap();
        let got = db.file_in(&fid, bob).unwrap().unwrap();
        assert_eq!(got.received, 4);
        let aid2 = db.file_in_finish(&got).unwrap();
        assert!(db.file_in(&fid, bob).unwrap().is_none());
        assert_eq!(
            db.get_attachment(aid2).unwrap().unwrap().chunk_size,
            Some(300)
        );

        // Deleting the message forgets the transfer.
        db.delete_message(msg).unwrap();
        assert!(db.file_out(&fid).unwrap().is_none());
    }
}
