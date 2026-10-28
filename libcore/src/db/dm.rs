//! Durable direct-message tombstones and authenticated deletion obligations.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DmReference {
    pub author: [u8; 32],
    pub origin: u64,
    pub sent_at: i64,
}

#[derive(Clone, Debug)]
pub struct DmDelete {
    pub message_id: i64,
    pub contact_id: i64,
    pub changed: bool,
    pub files: Vec<[u8; 16]>,
}

pub struct DmFileUpdate {
    pub message_id: i64,
    pub fresh: bool,
    pub total: u32,
    pub received: crate::files::Received,
    pub attachment_id: Option<i64>,
    pub broken: bool,
}

#[derive(Serialize,Deserialize)]
struct DmBackup {
    contacts: Vec<(i64,[u8;32])>,
    messages: Vec<(i64,i64,Vec<u8>,Option<i64>,Option<i64>,Option<i64>)>,
    tombstones: Vec<(i64,DmReference,i64,i64)>,
    pending: Vec<(i64,DmReference)>,
    files: Vec<(i64,i64,[u8;16])>,
    blocked: Vec<(i64,[u8;16])>,
    counters: Vec<(i64,i64)>,
}

impl Db {
    pub const DM_BACKUP_KEY: &'static str = "dm_state_v1";

    /// Metadata only: no body, filename, file key or ciphertext is duplicated.
    /// The existing encrypted backup's settings vector carries this sidecar.
    pub fn export_dm_state(&self) -> Result<Vec<u8>> {
        self.with_conn(|conn| {
            let contacts=conn.prepare("SELECT id,identity_sign FROM contacts")?
                .query_map([],|r|Ok((r.get(0)?,blob32(r.get(1)?)?)))?.collect_rows()?;
            let messages=conn.prepare("SELECT id,contact_id,sender_sign_pk,origin_msg_id,dm_seq,deleted_at FROM messages
                WHERE contact_id IS NOT NULL AND group_id IS NULL")?
                .query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?.collect_rows()?;
            let tombstones=conn.prepare("SELECT contact_id,author,origin,sent_at,message_id,created_at FROM dm_tombstones")?
                .query_map([],|r|Ok((r.get(0)?,DmReference {author:blob32(r.get(1)?)?,origin:r.get::<_,i64>(2)? as u64,sent_at:r.get(3)?},r.get(4)?,r.get(5)?)))?.collect_rows()?;
            let pending=conn.prepare("SELECT contact_id,author,origin,sent_at FROM dm_delete_outbox")?
                .query_map([],|r|Ok((r.get(0)?,DmReference {author:blob32(r.get(1)?)?,origin:r.get::<_,i64>(2)? as u64,sent_at:r.get(3)?})))?.collect_rows()?;
            let files=conn.prepare("SELECT message_id,contact_id,file_id FROM dm_message_files")?
                .query_map([],|r|Ok((r.get(0)?,r.get(1)?,blob16(r.get(2)?)?)))?.collect_rows()?;
            let blocked=conn.prepare("SELECT contact_id,file_id FROM dm_deleted_files")?
                .query_map([],|r|Ok((r.get(0)?,blob16(r.get(1)?)?)))?.collect_rows()?;
            let counters=conn.prepare("SELECT contact_id,next_seq FROM dm_counters")?
                .query_map([],|r|Ok((r.get(0)?,r.get(1)?)))?.collect_rows()?;
            bincode::serialize(&DmBackup {contacts,messages,tombstones,pending,files,blocked,counters}).map_err(|_|DbError::State)
        })
    }

    /// Map exported local contact IDs by authenticated identity, before message
    /// rows are imported. The old backup layout omitted original contact IDs.
    pub fn dm_backup_contact_map(&self) -> Result<Option<HashMap<i64,i64>>> {
        let Some(bytes)=self.get_setting(Self::DM_BACKUP_KEY)? else {return Ok(None)};
        let backup:DmBackup=bincode::deserialize(&bytes).map_err(|_|DbError::State)?;
        self.with_conn(|conn| Ok(Some(backup.contacts.into_iter().map(|(old,peer)| {
            let id=conn.query_row("SELECT id FROM contacts WHERE identity_sign=?1",params![peer.as_slice()],|r|r.get(0))?;
            Ok((old,id))
        }).collect::<Result<HashMap<_,_>>>()?)))
    }

    /// Run after attachments have been imported, so a deleted snapshot cannot
    /// restore a file key or body from an inconsistent earlier snapshot.
    pub fn restore_dm_state(&self) -> Result<()> {
        let Some(bytes)=self.get_setting(Self::DM_BACKUP_KEY)? else {return Ok(())};
        let backup:DmBackup=bincode::deserialize(&bytes).map_err(|_|DbError::State)?;
        let contacts=self.dm_backup_contact_map()?.ok_or(DbError::State)?;
        let contact=|old:&i64|contacts.get(old).copied().ok_or(DbError::State);
        let _files=self.file_mutations.lock().map_err(|_|DbError::State)?;
        self.with_tx(|tx| {
            for (id,old,author,origin,sequence,deleted_at) in &backup.messages {
                tx.execute("UPDATE messages SET contact_id=?2,sender_sign_pk=?3,origin_msg_id=?4,dm_seq=?5
                    WHERE id=?1 AND group_id IS NULL",params![id,contact(old)?,author,origin,sequence])?;
                if deleted_at.is_some() {
                    delete_dm_tx(tx,*id,false)?;
                    tx.execute("UPDATE messages SET deleted_at=?2 WHERE id=?1",params![id,deleted_at])?;
                }
            }
            for (old,reference,id,created_at) in &backup.tombstones {
                validate_reference(reference)?;
                tx.execute("INSERT OR IGNORE INTO dm_tombstones(contact_id,author,origin,sent_at,message_id,created_at)
                    VALUES(?1,?2,?3,?4,?5,?6)",params![contact(old)?,reference.author.as_slice(),reference.origin as i64,reference.sent_at,id,created_at])?;
            }
            for (old,reference) in &backup.pending {
                validate_reference(reference)?;
                tx.execute("INSERT OR IGNORE INTO dm_delete_outbox(contact_id,author,origin,sent_at) VALUES(?1,?2,?3,?4)",
                    params![contact(old)?,reference.author.as_slice(),reference.origin as i64,reference.sent_at])?;
            }
            for (message_id,old,file) in &backup.files {
                tx.execute("INSERT OR IGNORE INTO dm_message_files(message_id,contact_id,file_id) VALUES(?1,?2,?3)",
                    params![message_id,contact(old)?,file.as_slice()])?;
            }
            for (old,file) in &backup.blocked {
                tx.execute("INSERT OR IGNORE INTO dm_deleted_files(contact_id,file_id) VALUES(?1,?2)",params![contact(old)?,file.as_slice()])?;
            }
            for (old,next) in &backup.counters {
                tx.execute("INSERT INTO dm_counters(contact_id,next_seq) VALUES(?1,?2)
                    ON CONFLICT(contact_id) DO UPDATE SET next_seq=MAX(next_seq,excluded.next_seq)",params![contact(old)?,next])?;
            }
            tx.execute("DELETE FROM settings WHERE k=?1",params![Self::DM_BACKUP_KEY])?;
            Ok(())
        })
    }

    pub(super) fn migrate_dm(conn: &Connection) -> Result<()> {
        Self::ensure_column(conn, "messages", "dm_seq", "INTEGER")?;
        Self::ensure_column(conn, "messages", "deleted_at", "INTEGER")?;
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS dm_counters (
                contact_id INTEGER PRIMARY KEY REFERENCES contacts(id) ON DELETE CASCADE,
                next_seq INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS dm_tombstones (
                contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
                author BLOB NOT NULL,
                origin INTEGER NOT NULL CHECK(origin > 0),
                sent_at INTEGER NOT NULL,
                message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                created_at INTEGER NOT NULL,
                PRIMARY KEY(contact_id, author, origin, sent_at)
            );
            CREATE TABLE IF NOT EXISTS dm_delete_outbox (
                contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
                author BLOB NOT NULL,
                origin INTEGER NOT NULL,
                sent_at INTEGER NOT NULL,
                last_attempt_at INTEGER,
                attempts INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(contact_id, author, origin, sent_at)
            );
            CREATE TABLE IF NOT EXISTS dm_message_files (
                message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
                file_id BLOB NOT NULL,
                PRIMARY KEY(message_id, contact_id, file_id)
            );
            CREATE TABLE IF NOT EXISTS dm_deleted_files (
                contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
                file_id BLOB NOT NULL,
                PRIMARY KEY(contact_id, file_id)
            );
            CREATE TABLE IF NOT EXISTS dm_early_files (
                contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
                author BLOB NOT NULL,
                origin INTEGER NOT NULL,
                sent_at INTEGER NOT NULL,
                file_id BLOB NOT NULL,
                PRIMARY KEY(contact_id,file_id)
            );
            CREATE TABLE IF NOT EXISTS deleted_file_cleanup (path TEXT PRIMARY KEY);
            CREATE INDEX IF NOT EXISTS idx_dm_tombstone_message ON dm_tombstones(message_id);
            CREATE TRIGGER IF NOT EXISTS tr_dm_number AFTER INSERT ON messages
                WHEN NEW.contact_id IS NOT NULL AND NEW.group_id IS NULL
                BEGIN
                    INSERT OR IGNORE INTO dm_counters(contact_id, next_seq) VALUES(NEW.contact_id, 1);
                    UPDATE messages SET
                        dm_seq = COALESCE(NEW.dm_seq, (SELECT next_seq FROM dm_counters WHERE contact_id = NEW.contact_id)),
                        sender_sign_pk = COALESCE(NEW.sender_sign_pk, CASE WHEN NEW.direction = 1
                            THEN (SELECT v FROM settings WHERE k = 'dm_self_sign')
                            ELSE (SELECT identity_sign FROM contacts WHERE id = NEW.contact_id) END),
                        origin_msg_id = CASE WHEN NEW.direction = 1 THEN COALESCE(NEW.origin_msg_id, NEW.id) ELSE NEW.origin_msg_id END
                    WHERE id = NEW.id;
                    UPDATE dm_counters SET next_seq = MAX(next_seq + 1, COALESCE(NEW.dm_seq + 1, 1)) WHERE contact_id = NEW.contact_id;
                END;
            CREATE TRIGGER IF NOT EXISTS tr_dm_no_attachment BEFORE INSERT ON attachments
                WHEN EXISTS(SELECT 1 FROM messages WHERE id = NEW.message_id AND deleted_at IS NOT NULL)
                BEGIN SELECT RAISE(IGNORE); END;
        ")?;
        conn.execute_batch("
            WITH numbered AS (
                SELECT id,ROW_NUMBER() OVER(PARTITION BY contact_id ORDER BY id) AS sequence
                FROM messages WHERE contact_id IS NOT NULL AND group_id IS NULL
            )
            UPDATE messages SET dm_seq = (SELECT sequence FROM numbered WHERE numbered.id=messages.id)
            WHERE contact_id IS NOT NULL AND group_id IS NULL AND dm_seq IS NULL;
            INSERT INTO dm_counters(contact_id, next_seq)
                SELECT contact_id, MAX(dm_seq) + 1 FROM messages
                WHERE contact_id IS NOT NULL AND group_id IS NULL GROUP BY contact_id
                ON CONFLICT(contact_id) DO UPDATE SET next_seq = MAX(next_seq, excluded.next_seq);
        ")?;
        Ok(())
    }

    /// Call before starting incoming/send tasks, after loading identity.
    pub fn configure_dm_identity(&self, self_sign: &[u8]) -> Result<()> {
        if self_sign.len() != 32 { return Err(DbError::State); }
        self.with_tx(|tx| {
            tx.execute("INSERT OR REPLACE INTO settings(k,v) VALUES('dm_self_sign', ?1)", params![self_sign])?;
            tx.execute("UPDATE messages SET sender_sign_pk = CASE WHEN direction = 1 THEN ?1
                ELSE (SELECT identity_sign FROM contacts WHERE id = messages.contact_id) END
                WHERE contact_id IS NOT NULL AND group_id IS NULL AND sender_sign_pk IS NULL", params![self_sign])?;
            tx.execute("UPDATE messages SET origin_msg_id = id WHERE contact_id IS NOT NULL
                AND group_id IS NULL AND direction = 1 AND origin_msg_id IS NULL", [])?;
            Ok(())
        })
    }

    pub fn dm_reference(&self, message_id: i64) -> Result<DmReference> {
        self.with_conn(|conn| dm_reference_conn(conn, message_id))
    }

    pub fn outgoing_dm_by_origin(&self, contact_id:i64, origin:u64) -> Result<Option<i64>> {
        if origin==0 || origin>i64::MAX as u64 {return Err(DbError::State);}
        self.with_conn(|conn|Ok(conn.query_row("SELECT id FROM messages WHERE contact_id=?1 AND group_id IS NULL
            AND direction=1 AND origin_msg_id=?2 AND deleted_at IS NULL",params![contact_id,origin as i64],|r|r.get(0)).optional()?))
    }

    pub fn dm_delete_pending(&self, message_id: i64) -> Result<bool> {
        self.with_conn(|conn| Ok(conn.query_row("SELECT EXISTS(
            SELECT 1 FROM dm_tombstones t JOIN dm_delete_outbox q
                ON q.contact_id=t.contact_id AND q.author=t.author AND q.origin=t.origin AND q.sent_at=t.sent_at
            WHERE t.message_id=?1)", params![message_id], |r| r.get(0))?))
    }

    pub fn dm_deleted_message_id(&self, contact_id:i64, reference:&DmReference) -> Result<Option<i64>> {
        self.with_conn(|conn| Ok(conn.query_row("SELECT message_id FROM dm_tombstones
            WHERE contact_id=?1 AND author=?2 AND origin=?3 AND sent_at=?4",
            params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at], |r|r.get(0)).optional()?))
    }

    pub fn dm_is_deleted(&self, contact_id: i64, reference: &DmReference) -> Result<bool> {
        validate_reference(reference)?;
        self.with_conn(|conn| Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM dm_tombstones
            WHERE contact_id=?1 AND author=?2 AND origin=?3 AND sent_at=?4)",
            params![contact_id, reference.author.as_slice(), reference.origin as i64, reference.sent_at], |r| r.get(0))?))
    }

    pub fn delete_dm_message(&self, message_id: i64) -> Result<DmDelete> {
        let _files = self.file_mutations.lock().map_err(|_| DbError::State)?;
        self.with_tx(|tx| delete_dm_tx(tx, message_id, true))
    }

    /// Only invoke for an E2E-authenticated, non-group control from contact_id.
    pub fn receive_dm_delete(&self, contact_id: i64, reference: &DmReference) -> Result<DmDelete> {
        self.apply_dm_delete(contact_id,reference,false)
    }

    pub fn expire_incoming_dm(&self, contact_id:i64, reference:&DmReference) -> Result<DmDelete> {
        self.apply_dm_delete(contact_id,reference,true)
    }

    fn apply_dm_delete(&self, contact_id: i64, reference: &DmReference, queue:bool) -> Result<DmDelete> {
        validate_reference(reference)?;
        let _files = self.file_mutations.lock().map_err(|_| DbError::State)?;
        self.with_tx(|tx| {
            let own: Vec<u8> = tx.query_row("SELECT v FROM settings WHERE k='dm_self_sign'", [], |r| r.get(0))?;
            let peer: Vec<u8> = tx.query_row("SELECT identity_sign FROM contacts WHERE id=?1", params![contact_id], |r| r.get(0))?;
            let direction = if reference.author.as_slice() == own { 1 }
                else if reference.author.as_slice() == peer { 0 }
                else { return Err(DbError::State); };
            let existing: Option<i64> = tx.query_row("SELECT id FROM messages
                WHERE contact_id=?1 AND group_id IS NULL AND direction=?2
                    AND origin_msg_id=?3 AND sent_at=?4", params![contact_id, direction, reference.origin as i64, reference.sent_at], |r| r.get(0)).optional()?;
            let id = match existing {
                Some(id) => id,
                // A sender can delete their own letter before it arrives, but
                // cannot pre-delete a future letter authored by this endpoint.
                None if direction == 0 => {
                    tx.execute("INSERT INTO messages(contact_id, sender_sign_pk, direction, body, sent_at, origin_msg_id)
                        VALUES(?1,?2,0,'',?3,?4)", params![contact_id, reference.author.as_slice(), reference.sent_at, reference.origin as i64])?;
                    tx.last_insert_rowid()
                }
                None => return Err(DbError::State),
            };
            delete_dm_tx(tx, id, queue)
        })
    }

    pub fn acknowledge_dm_delete(&self, contact_id: i64, reference: &DmReference) -> Result<bool> {
        validate_reference(reference)?;
        self.with_conn(|conn| Ok(conn.execute("DELETE FROM dm_delete_outbox
            WHERE contact_id=?1 AND author=?2 AND origin=?3 AND sent_at=?4",
            params![contact_id, reference.author.as_slice(), reference.origin as i64, reference.sent_at])? > 0))
    }

    pub fn pending_dm_deletes(&self, contact_id: i64, now: i64) -> Result<Vec<DmReference>> {
        self.with_conn(|conn| conn.prepare_cached("SELECT author,origin,sent_at FROM dm_delete_outbox
            WHERE contact_id=?1 AND (last_attempt_at IS NULL OR ?2-last_attempt_at >= MIN(300000, 5000*(1 << MIN(attempts,16))))
            ORDER BY origin LIMIT 50")?.query_map(params![contact_id,now], |r| Ok(DmReference {
                author: blob32(r.get(0)?)?, origin: r.get::<_,i64>(1)? as u64, sent_at:r.get(2)?,
            }))?.collect_rows())
    }

    pub fn record_dm_delete_attempt(&self, contact_id: i64, reference: &DmReference, now: i64) -> Result<()> {
        self.with_conn(|conn| { conn.execute("UPDATE dm_delete_outbox SET attempts=attempts+1,last_attempt_at=?5
            WHERE contact_id=?1 AND author=?2 AND origin=?3 AND sent_at=?4",
            params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at,now])?; Ok(()) })
    }

    pub fn delete_expired_dm(&self, now: i64) -> Result<Vec<DmDelete>> {
        // Old backups omitted received origins. Keep those records explicit
        // rather than guessing a remote ID, without blocking other timers.
        self.delete_matching_dm("expires_at IS NOT NULL AND expires_at <= ?1 AND expires_at > sent_at
            AND origin_msg_id > 0 AND length(sender_sign_pk)=32", now)
    }

    pub fn clear_dm_history(&self) -> Result<Vec<DmDelete>> {
        self.delete_matching_dm("?1=0", 0)
    }

    pub fn clear_dm_contact(&self, contact_id:i64) -> Result<Vec<DmDelete>> {
        self.delete_matching_dm("contact_id=?1",contact_id)
    }

    fn delete_matching_dm(&self, condition: &str, argument: i64) -> Result<Vec<DmDelete>> {
        let _files = self.file_mutations.lock().map_err(|_| DbError::State)?;
        self.with_tx(|tx| {
            let ids = tx.prepare(&format!("SELECT id FROM messages WHERE contact_id IS NOT NULL
                AND group_id IS NULL AND deleted_at IS NULL AND ({condition}) ORDER BY id"))?
                .query_map(params![argument], |r| r.get::<_,i64>(0))?.collect_rows()?;
            ids.into_iter().map(|id| delete_dm_tx(tx,id,true)).collect()
        })
    }

    /// Keys/metadata have already gone in the deletion transaction. Failed
    /// unlinks remain durable cleanup work and are retried after reopen.
    pub fn cleanup_deleted_files(&self, attachment_dir: &Path) -> Result<usize> {
        let _files = self.file_mutations.lock().map_err(|_| DbError::State)?;
        let paths: Vec<String> = self.with_conn(|conn| conn.prepare("SELECT path FROM deleted_file_cleanup")?
            .query_map([], |r| r.get(0))?.collect_rows())?;
        let mut cleaned = 0;
        for name in paths {
            match fs::remove_file(attachment_dir.join(&name)) {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(_) => continue,
            }
            self.with_conn(|conn| { conn.execute("DELETE FROM deleted_file_cleanup WHERE path=?1", params![name])?; Ok(()) })?;
            cleaned += 1;
        }
        Ok(cleaned)
    }

    pub fn dm_file_blocked(&self, contact_id: i64, file_id: &[u8;16]) -> Result<bool> {
        self.with_conn(|conn| Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM dm_deleted_files WHERE contact_id=?1 AND file_id=?2)",
            params![contact_id,file_id.as_slice()], |r| r.get(0))?))
    }

    /// Retain only the opaque association after completion, to reject chunks.
    pub fn track_dm_file(&self, contact_id:i64, message_id:i64, file_id:&[u8;16]) -> Result<()> {
        self.with_conn(|conn| { conn.execute("INSERT OR IGNORE INTO dm_message_files(message_id,contact_id,file_id)
            SELECT id,?2,?3 FROM messages WHERE id=?1 AND contact_id=?2 AND group_id IS NULL AND deleted_at IS NULL",
            params![message_id,contact_id,file_id.as_slice()])?;
            conn.execute("DELETE FROM dm_early_files WHERE contact_id=?1 AND file_id=?2 AND EXISTS(
                SELECT 1 FROM dm_message_files WHERE message_id=?3 AND contact_id=?1 AND file_id=?2)",
                params![contact_id,file_id.as_slice(),message_id])?;
            Ok(()) })
    }

    pub fn reject_deleted_dm_files(&self, contact_id:i64, reference:&DmReference, files:&[[u8;16]]) -> Result<bool> {
        self.with_tx(|tx| {
            let deleted: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM dm_tombstones
                WHERE contact_id=?1 AND author=?2 AND origin=?3 AND sent_at=?4)",
                params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at], |r| r.get(0))?;
            if deleted { for file in files {
                tx.execute("INSERT OR IGNORE INTO dm_deleted_files(contact_id,file_id) VALUES(?1,?2)",params![contact_id,file.as_slice()])?;
                tx.execute("DELETE FROM file_early WHERE contact_id=?1 AND file_id=?2",params![contact_id,file.as_slice()])?;
            }}
            Ok(deleted)
        })
    }

    /// Synchronous file mutation and DB publication share the deletion lock;
    /// no filesystem write can race an unlink after the tombstone commits.
    pub fn apply_file_chunk(&self, contact_id:i64, chunk:&crate::session::WireFileChunk,
        reference:Option<&DmReference>, attachment_dir:&Path) -> Result<Option<DmFileUpdate>> {
        self.apply_file_chunk_inner(contact_id,chunk,reference,attachment_dir,|| {})
    }

    fn apply_file_chunk_inner(&self, contact_id:i64, chunk:&crate::session::WireFileChunk,
        reference:Option<&DmReference>, attachment_dir:&Path, before_write:impl FnOnce()) -> Result<Option<DmFileUpdate>> {
        let _files=self.file_mutations.lock().map_err(|_| DbError::State)?;
        self.with_tx(|tx| {
            let blocked:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM dm_deleted_files WHERE contact_id=?1 AND file_id=?2)",
                params![contact_id,chunk.file_id.as_slice()],|r|r.get(0))?;
            if blocked { return Ok(None); }
            if let Some(reference)=reference {
                let deleted:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM dm_tombstones
                    WHERE contact_id=?1 AND author=?2 AND origin=?3 AND sent_at=?4)",
                    params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at],|r|r.get(0))?;
                if deleted { return Ok(None); }
            }
            let fin=tx.query_row("SELECT file_id,contact_id,message_id,name,size,sha256,chunk_size,path,key,bits,received,created_at
                FROM file_in WHERE file_id=?1 AND contact_id=?2 AND EXISTS(
                    SELECT 1 FROM messages WHERE id=file_in.message_id AND deleted_at IS NULL)",
                params![chunk.file_id.as_slice(),contact_id],Self::map_file_in).optional()?;
            let Some(fin)=fin else {
                if chunk.data.len() <= crate::files::CHUNK_SIZE as usize {
                    if let Some(reference)=reference {
                        tx.execute("INSERT OR IGNORE INTO dm_early_files(contact_id,author,origin,sent_at,file_id)
                            VALUES(?1,?2,?3,?4,?5)",params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at,chunk.file_id.as_slice()])?;
                    }
                    tx.execute("INSERT OR IGNORE INTO file_early(file_id,contact_id,idx,data,at) VALUES(?1,?2,?3,?4,?5)",
                        params![chunk.file_id.as_slice(),contact_id,chunk.index,&chunk.data,now_ms()])?;
                }
                return Ok(None);
            };
            let total=crate::files::chunk_count(fin.size,fin.chunk_size);
            let mut received=crate::files::Received::from_bits(fin.bits.clone(),total);
            let path=attachment_dir.join(&fin.path);
            let key:[u8;32]=fin.key.clone().try_into().map_err(|_|DbError::State)?;
            let cipher=crate::crypto::AttachmentCipher::from_key(key);
            let fresh=if received.has(chunk.index) {false} else {
                before_write();
                crate::files::write_part(&path,&cipher,fin.size,fin.chunk_size,chunk.index,&chunk.data)?;
                received.mark(chunk.index);
                tx.execute("UPDATE file_in SET bits=?3,received=?4 WHERE file_id=?1 AND contact_id=?2",
                    params![fin.file_id.as_slice(),contact_id,received.bits(),received.count()])?;
                true
            };
            let mut attachment_id=None;
            let mut broken=false;
            if received.complete() {
                broken=crate::files::open_to(&path,&cipher,fin.size,fin.chunk_size,&mut std::io::sink())? != fin.sha256;
                if broken { let _=fs::remove_file(path); }
                else {
                    tx.execute("INSERT INTO attachments(message_id,name,size,key,path,chunk_size) VALUES(?1,?2,?3,?4,?5,?6)",
                        params![fin.message_id,fin.name,fin.size as i64,fin.key,fin.path,fin.chunk_size])?;
                    attachment_id=Some(tx.last_insert_rowid());
                }
                tx.execute("DELETE FROM file_in WHERE file_id=?1 AND contact_id=?2",params![fin.file_id.as_slice(),contact_id])?;
            }
            Ok(Some(DmFileUpdate {message_id:fin.message_id,fresh,total,received,attachment_id,broken}))
        })
    }

    /// Emit a UI event at a point ordered with the tombstone transaction.
    pub fn if_message_live(&self, message_id:i64, emit:impl FnOnce()) -> Result<bool> {
        self.with_conn(|conn| {
            let live:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND deleted_at IS NULL)",params![message_id],|r|r.get(0))?;
            if live {emit();}
            Ok(live)
        })
    }
}

fn validate_reference(reference: &DmReference) -> Result<()> {
    if reference.origin == 0 || reference.origin > i64::MAX as u64 { return Err(DbError::State); }
    Ok(())
}

fn dm_reference_conn(conn: &Connection, id:i64) -> Result<DmReference> {
    let row: (Vec<u8>,Option<i64>,i64) = conn.query_row("SELECT sender_sign_pk,origin_msg_id,sent_at
        FROM messages WHERE id=?1 AND contact_id IS NOT NULL AND group_id IS NULL", params![id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let origin = row.1.filter(|value| *value > 0).ok_or(DbError::MissingMessageOrigin)?;
    Ok(DmReference { author:row.0.try_into().map_err(|_| DbError::State)?, origin:origin as u64, sent_at:row.2 })
}

fn delete_dm_tx(tx: &Transaction<'_>, id:i64, queue:bool) -> Result<DmDelete> {
    let reference = dm_reference_conn(tx,id)?;
    let (contact_id, already): (i64,Option<i64>) = tx.query_row("SELECT contact_id,deleted_at FROM messages WHERE id=?1", params![id], |r| Ok((r.get(0)?,r.get(1)?)))?;
    let mut files:Vec<[u8;16]> = tx.prepare("SELECT file_id FROM file_out WHERE message_id=?1
        UNION SELECT file_id FROM file_in WHERE message_id=?1
        UNION SELECT file_id FROM dm_message_files WHERE message_id=?1
        UNION SELECT file_id FROM dm_early_files WHERE contact_id=?2 AND author=?3 AND origin=?4 AND sent_at=?5")?
        .query_map(params![id,contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at], |r| blob16(r.get(0)?))?.collect_rows()?;
    files.sort(); files.dedup();
    if already.is_some() { return Ok(DmDelete { message_id:id,contact_id,changed:false,files }); }
    let now=now_ms();
    tx.execute("INSERT OR IGNORE INTO dm_tombstones(contact_id,author,origin,sent_at,message_id,created_at)
        VALUES(?1,?2,?3,?4,?5,?6)",params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at,id,now])?;
    if queue { tx.execute("INSERT OR IGNORE INTO dm_delete_outbox(contact_id,author,origin,sent_at)
        VALUES(?1,?2,?3,?4)",params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at])?; }
    tx.execute("INSERT OR IGNORE INTO deleted_file_cleanup(path) SELECT path FROM attachments WHERE message_id=?1
        UNION SELECT path FROM file_in WHERE message_id=?1",params![id])?;
    for file in &files {
        tx.execute("INSERT OR IGNORE INTO dm_deleted_files(contact_id,file_id) VALUES(?1,?2)",params![contact_id,file.as_slice()])?;
        tx.execute("DELETE FROM file_early WHERE contact_id=?1 AND file_id=?2",params![contact_id,file.as_slice()])?;
        tx.execute("DELETE FROM file_out_peer WHERE file_id=?1",params![file.as_slice()])?;
    }
    tx.execute("DELETE FROM file_out WHERE message_id=?1",params![id])?;
    tx.execute("DELETE FROM file_in WHERE message_id=?1",params![id])?;
    tx.execute("DELETE FROM dm_early_files WHERE contact_id=?1 AND author=?2 AND origin=?3 AND sent_at=?4",
        params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at])?;
    tx.execute("DELETE FROM attachments WHERE message_id=?1",params![id])?;
    tx.execute("DELETE FROM pending_outbound WHERE msg_id=?1",params![id])?;
    tx.execute("DELETE FROM pinned_messages WHERE message_id=?1",params![id])?;
    tx.execute("DELETE FROM settings WHERE k IN ('buttons_'||?1,'sound_'||?1,'console_'||?1,'console_pending_'||?1)",params![id])?;
    tx.execute("UPDATE messages SET body='',read=1,sent=1,expires_at=NULL,reply_to=NULL,deleted_at=?2 WHERE id=?1",params![id,now])?;
    Ok(DmDelete { message_id:id,contact_id,changed:true,files })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc,mpsc};
    use sha2::{Digest,Sha256};

    fn database(path:&Path, own:[u8;32], peer:[u8;32]) -> (Db,i64) {
        let db=Db::open_plain(path).unwrap();
        db.configure_dm_identity(&own).unwrap();
        let contact=db.add_contact(&peer,&[7;32],"relay","peer",None).unwrap();
        (db,contact)
    }

    #[test]
    fn receiver_deletes_peer_message_using_original_identity_not_local_id() {
        let directory=tempfile::tempdir().unwrap();
        let (alice,bob_id)=database(&directory.path().join("alice.db"),[1;32],[2;32]);
        let (bob,alice_id)=database(&directory.path().join("bob.db"),[2;32],[1;32]);
        for _ in 0..4 {bob.insert_message(alice_id,Direction::Out,"unrelated",100,None,&[]).unwrap();}
        let original=alice.insert_message(bob_id,Direction::Out,"erase both",200,None,&[]).unwrap();
        let copy=bob.insert_message_with_origin(alice_id,Direction::In,"erase both",200,None,&[],Some(original)).unwrap();
        assert_ne!(copy,original);
        let reference=bob.dm_reference(copy).unwrap();
        assert_eq!(reference,alice.dm_reference(original).unwrap());
        let third=alice.add_contact(&[3;32],&[8;32],"third","third",None).unwrap();
        assert!(alice.receive_dm_delete(third,&reference).is_err());
        assert_eq!(alice.get_message(original).unwrap().unwrap().body,"erase both");
        let gap=bob.get_message(copy).unwrap().unwrap().dm_seq;
        bob.delete_dm_message(copy).unwrap();
        assert_eq!(bob.pending_dm_deletes(alice_id,now_ms()).unwrap(),vec![reference.clone()]);
        alice.receive_dm_delete(bob_id,&reference).unwrap();
        assert_eq!(alice.get_message(original).unwrap().unwrap().body,"");
        assert!(alice.get_message(original).unwrap().unwrap().deleted_at.is_some());
        assert!(alice.receive_dm_delete(bob_id,&reference).unwrap().changed==false);
        let outsider=bob.add_contact(&[3;32],&[8;32],"other","other",None).unwrap();
        assert!(!bob.acknowledge_dm_delete(outsider,&reference).unwrap());
        bob.record_dm_delete_attempt(alice_id,&reference,now_ms()).unwrap();
        drop(bob);
        let bob=Db::open_plain(&directory.path().join("bob.db")).unwrap();
        assert!(bob.dm_delete_pending(copy).unwrap());
        assert_eq!(bob.get_message(copy).unwrap().unwrap().dm_seq,gap);
        assert!(bob.acknowledge_dm_delete(alice_id,&reference).unwrap());
        assert!(!bob.acknowledge_dm_delete(alice_id,&reference).unwrap());
        assert!(!bob.dm_delete_pending(copy).unwrap());
        assert!(bob.get_message(copy).unwrap().unwrap().deleted_at.is_some());
    }

    #[test]
    fn deletion_before_content_blocks_replay_edit_offers_and_chunks_after_reopen() {
        let directory=tempfile::tempdir().unwrap();
        let path=directory.path().join("chat.db");
        let (db,contact)=database(&path,[1;32],[2;32]);
        let reference=DmReference {author:[2;32],origin:55,sent_at:1234};
        let deleted=db.receive_dm_delete(contact,&reference).unwrap();
        let id=deleted.message_id;
        let file=[9;16];
        db.file_early_put(&file,contact,0,b"early plaintext").unwrap();
        assert!(db.reject_deleted_dm_files(contact,&reference,&[file]).unwrap());
        assert!(db.file_early_take(&file,contact).unwrap().is_empty());
        drop(db);
        let db=Db::open_plain(&path).unwrap();
        let duplicate=db.insert_message_with_origin(contact,Direction::In,"must not return",1234,None,&[
            NewAttachment {name:"private.txt".into(),size:10,key:vec![4;32],path:"must-not-exist".into(),chunk_size:None}
        ],Some(55)).unwrap();
        assert_eq!(duplicate,id);
        db.update_message_body(id,"late edit").unwrap();
        db.set_setting(&format!("buttons_{id}"),b"late buttons").unwrap();
        assert_eq!(db.get_message(id).unwrap().unwrap().body,"");
        assert!(db.list_attachments(id).unwrap().is_empty());
        assert!(db.get_setting(&format!("buttons_{id}")).unwrap().is_none());
        assert!(!db.pin_contact_message(contact,id).unwrap());
        let chunk=crate::session::WireFileChunk {file_id:file,index:0,data:vec![1;100]};
        assert!(db.apply_file_chunk(contact,&chunk,Some(&reference),directory.path()).unwrap().is_none());
        assert!(db.file_early_take(&file,contact).unwrap().is_empty());
        assert!(!directory.path().join("must-not-exist").exists());
        let mut forged=reference.clone(); forged.author=[3;32];
        assert!(db.receive_dm_delete(contact,&forged).is_err());
        let forged_future=DmReference {author:[1;32],origin:900,sent_at:2000};
        assert!(db.receive_dm_delete(contact,&forged_future).is_err());
    }

    #[test]
    fn failed_deletion_rolls_back_content_keys_queue_and_tombstone() {
        for table in ["dm_delete_outbox","deleted_file_cleanup"] {
            let directory=tempfile::tempdir().unwrap();
            let path=directory.path().join("chat.db");
            let (db,contact)=database(&path,[1;32],[2;32]);
            let id=db.insert_message(contact,Direction::Out,"keep on failure",100,None,&[
                NewAttachment {name:"a".into(),size:3,key:vec![6;32],path:"cipher".into(),chunk_size:None}
            ]).unwrap();
            db.with_conn(|conn| {conn.execute_batch(&format!("CREATE TRIGGER injected BEFORE INSERT ON {table}
                BEGIN SELECT RAISE(ABORT,'injected deletion failure'); END;"))?;Ok(())}).unwrap();
            assert!(db.delete_dm_message(id).is_err());
            drop(db);
            let db=Db::open_plain(&path).unwrap();
            let message=db.get_message(id).unwrap().unwrap();
            assert_eq!(message.body,"keep on failure");
            assert!(message.deleted_at.is_none());
            assert_eq!(db.list_attachments(id).unwrap()[0].key,vec![6;32]);
            assert!(!db.dm_delete_pending(id).unwrap());
            assert!(!db.dm_is_deleted(contact,&db.dm_reference(id).unwrap()).unwrap());
        }
    }

    #[test]
    fn early_dm_parts_are_deleted_even_if_the_offer_never_arrives() {
        let directory=tempfile::tempdir().unwrap();
        let (db,contact)=database(&directory.path().join("chat.db"),[1;32],[2;32]);
        let reference=DmReference {author:[2;32],origin:55,sent_at:1234};
        let file=[9;16];
        let chunk=crate::session::WireFileChunk {file_id:file,index:0,data:b"early private bytes".to_vec()};
        assert!(db.apply_file_chunk(contact,&chunk,Some(&reference),directory.path()).unwrap().is_none());
        let held:i64=db.with_conn(|conn|Ok(conn.query_row("SELECT COUNT(*) FROM file_early",[],|r|r.get(0))?)).unwrap();
        assert_eq!(held,1);
        db.receive_dm_delete(contact,&reference).unwrap();
        assert!(db.file_early_take(&file,contact).unwrap().is_empty());
        assert!(db.dm_file_blocked(contact,&file).unwrap());
        assert!(db.apply_file_chunk(contact,&chunk,Some(&reference),directory.path()).unwrap().is_none());
        assert!(db.file_early_take(&file,contact).unwrap().is_empty());
    }

    #[test]
    fn clear_and_timer_keep_numbers_and_expire_explicitly_pinned_messages() {
        let directory=tempfile::tempdir().unwrap();
        let path=directory.path().join("chat.db");
        let (db,contact)=database(&path,[1;32],[2;32]);
        let one=db.insert_message(contact,Direction::Out,"one",100,Some(200),&[]).unwrap();
        let two=db.insert_message_with_origin(contact,Direction::In,"two",101,None,&[],Some(99)).unwrap();
        let other=db.add_contact(&[3;32],&[8;32],"other","other",None).unwrap();
        let untouched=db.insert_message(other,Direction::Out,"other chat",102,None,&[]).unwrap();
        assert!(db.pin_contact_message(contact,one).unwrap());
        assert!(db.pin_contact_message(contact,two).unwrap());
        assert_eq!(db.delete_expired_dm(200).unwrap().iter().map(|m|m.message_id).collect::<Vec<_>>(),vec![one]);
        assert_eq!(db.clear_dm_contact(contact).unwrap().iter().map(|m|m.message_id).collect::<Vec<_>>(),vec![two]);
        assert_eq!(db.get_message(untouched).unwrap().unwrap().body,"other chat");
        assert!(db.list_attachments(one).unwrap().is_empty());
        assert_eq!(db.get_message(one).unwrap().unwrap().dm_seq,Some(1));
        assert_eq!(db.get_message(two).unwrap().unwrap().dm_seq,Some(2));
        drop(db);
        let db=Db::open_plain(&path).unwrap();
        let three=db.insert_message(contact,Direction::Out,"three",300,None,&[]).unwrap();
        assert_eq!(db.get_message(three).unwrap().unwrap().dm_seq,Some(3));
        assert_eq!(db.list_messages(contact,100,None).unwrap().len(),3);
        assert_eq!(db.pending_dm_deletes(contact,now_ms()).unwrap().len(),2);
    }

    #[test]
    fn unknown_legacy_origin_is_an_error_without_silent_local_deletion() {
        let directory=tempfile::tempdir().unwrap();
        let (db,contact)=database(&directory.path().join("chat.db"),[1;32],[2;32]);
        let id=db.insert_message(contact,Direction::In,"legacy",100,Some(200),&[]).unwrap();
        let known=db.insert_message(contact,Direction::Out,"known timer",101,Some(200),&[]).unwrap();
        assert!(matches!(db.delete_dm_message(id),Err(DbError::MissingMessageOrigin)));
        assert_eq!(db.delete_expired_dm(200).unwrap().iter().map(|row|row.message_id).collect::<Vec<_>>(),vec![known]);
        assert_eq!(db.get_message(id).unwrap().unwrap().body,"legacy");
        assert!(db.get_message(id).unwrap().unwrap().deleted_at.is_none());
    }

    #[test]
    fn backup_sidecar_restores_identity_mapping_gaps_and_pending_deletion_without_content() {
        let directory=tempfile::tempdir().unwrap();
        let source=Db::open_plain(&directory.path().join("source.db")).unwrap();
        source.configure_dm_identity(&[1;32]).unwrap();
        source.add_contact(&[3;32],&[8;32],"third","third",None).unwrap();
        let old_peer=source.add_contact(&[2;32],&[7;32],"peer","peer",None).unwrap();
        source.insert_message(old_peer,Direction::Out,"secret sender text",100,None,&[]).unwrap();
        let removed=source.insert_message_with_origin(old_peer,Direction::In,"secret received text",200,None,&[],Some(99)).unwrap();
        source.delete_dm_message(removed).unwrap();
        let sidecar=source.export_dm_state().unwrap();
        assert!(!sidecar.windows(b"secret sender text".len()).any(|bytes|bytes==b"secret sender text"));
        assert!(!sidecar.windows(b"secret received text".len()).any(|bytes|bytes==b"secret received text"));
        let (restored,new_peer)=database(&directory.path().join("restored.db"),[1;32],[2;32]);
        restored.add_contact(&[3;32],&[8;32],"third","third",None).unwrap();
        assert_ne!(new_peer,old_peer);
        restored.set_setting(Db::DM_BACKUP_KEY,&sidecar).unwrap();
        let map=restored.dm_backup_contact_map().unwrap().unwrap();
        let mut messages=source.list_all_messages().unwrap();
        for message in &mut messages {message.contact_id=message.contact_id.map(|old|map[&old]);}
        restored.bulk_insert_messages(&messages).unwrap();
        // An earlier attachment snapshot cannot restore a key for a tombstone.
        let path=directory.path().join("restored-cipher");
        fs::write(&path,b"ciphertext").unwrap();
        restored.bulk_insert_attachments(&[Attachment {id:900,message_id:removed,name:"private filename".into(),size:10,
            key:vec![9;32],path:path.to_string_lossy().into(),chunk_size:None}]).unwrap();
        restored.restore_dm_state().unwrap();
        restored.cleanup_deleted_files(directory.path()).unwrap();
        assert!(!path.exists());
        assert!(restored.list_attachments(removed).unwrap().is_empty());
        assert_eq!(restored.dm_reference(removed).unwrap().origin,99);
        let removed_row=restored.get_message(removed).unwrap().unwrap();
        assert_eq!(removed_row.contact_id,Some(new_peer));
        assert_eq!(removed_row.dm_seq,Some(2));
        assert!(removed_row.deleted_at.is_some());
        assert!(restored.dm_delete_pending(removed).unwrap());
        let next=restored.insert_message(new_peer,Direction::Out,"next",300,None,&[]).unwrap();
        assert_eq!(restored.get_message(next).unwrap().unwrap().dm_seq,Some(3));
        assert!(restored.get_setting(Db::DM_BACKUP_KEY).unwrap().is_none());
    }

    #[test]
    fn deletion_waits_for_inflight_chunk_then_erases_its_key_and_file() {
        let directory=tempfile::tempdir().unwrap();
        let (db,contact)=database(&directory.path().join("chat.db"),[1;32],[2;32]);
        let id=db.insert_message_with_origin(contact,Direction::In,"file",100,None,&[],Some(50)).unwrap();
        let reference=db.dm_reference(id).unwrap();
        let data=b"last private part";
        let cipher=crate::crypto::AttachmentCipher::generate();
        let file=[9;16];
        db.file_in_add(&FileIn {file_id:file,contact_id:contact,message_id:id,name:"private".into(),size:data.len() as u64,
            sha256:Sha256::digest(data).into(),chunk_size:crate::files::CHUNK_SIZE,path:"parted".into(),key:cipher.key().to_vec(),
            bits:crate::files::Received::new(1).bits().to_vec(),received:0,created_at:now_ms()}).unwrap();
        db.track_dm_file(contact,id,&file).unwrap();
        let db=Arc::new(db);
        let (entered_tx,entered_rx)=mpsc::channel();
        let (release_tx,release_rx)=mpsc::channel();
        let writer_db=db.clone(); let root=directory.path().to_owned();
        let writer=std::thread::spawn(move || writer_db.apply_file_chunk_inner(contact,
            &crate::session::WireFileChunk {file_id:file,index:0,data:data.to_vec()},Some(&reference),&root,|| {
                entered_tx.send(()).unwrap(); release_rx.recv().unwrap();
            }).unwrap());
        entered_rx.recv().unwrap();
        let (deleting_tx,deleting_rx)=mpsc::channel();
        let (done_tx,done_rx)=mpsc::channel();
        let delete_db=db.clone(); let root=directory.path().to_owned();
        let deleter=std::thread::spawn(move || {
            deleting_tx.send(()).unwrap();
            delete_db.delete_dm_message(id).unwrap();
            delete_db.cleanup_deleted_files(&root).unwrap();
            done_tx.send(()).unwrap();
        });
        deleting_rx.recv().unwrap();
        assert!(matches!(done_rx.try_recv(),Err(mpsc::TryRecvError::Empty)));
        release_tx.send(()).unwrap();
        writer.join().unwrap(); deleter.join().unwrap(); done_rx.recv().unwrap();
        assert!(db.list_attachments(id).unwrap().is_empty());
        assert!(db.file_in(&file,contact).unwrap().is_none());
        assert!(db.dm_file_blocked(contact,&file).unwrap());
        assert!(!directory.path().join("parted").exists());
        assert!(db.dm_delete_pending(id).unwrap());
    }
}
