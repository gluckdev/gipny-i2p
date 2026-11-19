//! Direct/group message history, read state, retention, and search.

use super::*;

impl Db {
    pub fn insert_message(
        &self,
        contact_id: i64,
        direction: Direction,
        body: &str,
        sent_at: i64,
        expires_at: Option<i64>,
        attachments: &[NewAttachment],
    ) -> Result<i64> {
        self.insert_message_with_origin(
            contact_id,
            direction,
            body,
            sent_at,
            expires_at,
            attachments,
            None,
        )
    }

    pub fn insert_message_with_origin(
        &self,
        contact_id: i64,
        direction: Direction,
        body: &str,
        sent_at: i64,
        expires_at: Option<i64>,
        attachments: &[NewAttachment],
        origin_msg_id: Option<i64>,
    ) -> Result<i64> {
        check_body(body)?;
        self.with_tx(|tx| {
            if let Some(origin) = origin_msg_id.filter(|v| *v > 0) {
                if matches!(direction, Direction::In) {
                    if let Some(id) = tx.query_row("SELECT id FROM messages WHERE contact_id=?1 AND group_id IS NULL
                        AND direction=0 AND origin_msg_id=?2 AND deleted_at IS NOT NULL", params![contact_id,origin], |r|r.get(0)).optional()? {
                        return Ok(id);
                    }
                }
            }
            tx.execute(
                "INSERT INTO messages (contact_id, direction, body, sent_at, expires_at, origin_msg_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![contact_id, direction.to_i64(), body, sent_at, expires_at, origin_msg_id])?;
            let id = tx.last_insert_rowid();
            insert_attachments(tx, id, attachments)?;
            Ok(id)
        })
    }

    pub fn find_message_by_origin(
        &self,
        contact_id: i64,
        origin_msg_id: i64,
    ) -> Result<Option<i64>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT id FROM messages WHERE contact_id = ?1 AND direction = 0 AND origin_msg_id = ?2 ORDER BY id DESC LIMIT 1")?
            .query_row(params![contact_id, origin_msg_id], |r| r.get::<_, i64>(0)).optional().map_err(Into::into))
    }

    pub fn resolve_contact_message(&self, contact_id: i64, origin: i64) -> Result<Option<i64>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT id FROM messages WHERE contact_id = ?1 AND (id = ?2 OR origin_msg_id = ?2) ORDER BY id DESC LIMIT 1")?
            .query_row(params![contact_id, origin], |r| r.get::<_, i64>(0)).optional().map_err(Into::into))
    }

    pub fn resolve_group_message(
        &self,
        group_id: &[u8],
        sender_sign_pk: &[u8],
        origin: i64,
        self_sign_pk: &[u8],
    ) -> Result<Option<i64>> {
        self.with_conn(|c| {
            if sender_sign_pk == self_sign_pk {
                c.prepare_cached(
                    "SELECT id FROM messages WHERE group_id = ?1 AND direction = 1 AND id = ?2",
                )?
                .query_row(params![group_id, origin], |r| r.get::<_, i64>(0))
                .optional()
                .map_err(Into::into)
            } else {
                c.prepare_cached(
                    "SELECT id FROM messages
                 WHERE group_id = ?1 AND sender_sign_pk = ?2 AND origin_msg_id = ?3
                 ORDER BY id DESC LIMIT 1",
                )?
                .query_row(params![group_id, sender_sign_pk, origin], |r| {
                    r.get::<_, i64>(0)
                })
                .optional()
                .map_err(Into::into)
            }
        })
    }

    pub fn message_origin(&self, id: i64) -> Result<Option<i64>> {
        self.with_conn(|c| {
            c.prepare_cached("SELECT origin_msg_id FROM messages WHERE id = ?1")?
                .query_row(params![id], |r| r.get::<_, Option<i64>>(0))
                .optional()
                .map(|o| o.flatten())
                .map_err(Into::into)
        })
    }

    pub fn get_message(&self, id: i64) -> Result<Option<Message>> {
        self.with_conn(|c| {
            c.prepare_cached(concat!(
                "SELECT ",
                message_cols!(),
                " FROM messages WHERE id = ?1"
            ))?
            .query_row(params![id], Self::map_message)
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn update_message_body(&self, id: i64, body: &str) -> Result<()> {
        check_body(body)?;
        self.with_conn(|c| {
            c.execute(
                "UPDATE messages SET body = ?1 WHERE id = ?2 AND deleted_at IS NULL",
                params![body, id],
            )?;
            Ok(())
        })
    }

    pub fn set_reply_to(&self, id: i64, reply_to: Option<i64>) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE messages SET reply_to = ?1 WHERE id = ?2",
                params![reply_to, id],
            )?;
            Ok(())
        })
    }

    pub fn insert_group_message(
        &self,
        group_id: &[u8],
        sender_sign_pk: Option<&[u8]>,
        direction: Direction,
        body: &str,
        sent_at: i64,
        expires_at: Option<i64>,
        attachments: &[NewAttachment],
    ) -> Result<i64> {
        self.insert_group_message_with_origin(
            group_id,
            sender_sign_pk,
            direction,
            body,
            sent_at,
            expires_at,
            attachments,
            None,
        )
    }

    pub fn insert_group_message_with_origin(
        &self,
        group_id: &[u8],
        sender_sign_pk: Option<&[u8]>,
        direction: Direction,
        body: &str,
        sent_at: i64,
        expires_at: Option<i64>,
        attachments: &[NewAttachment],
        origin_msg_id: Option<i64>,
    ) -> Result<i64> {
        check_body(body)?;
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO messages (group_id, sender_sign_pk, direction, body, sent_at, expires_at, origin_msg_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![group_id, sender_sign_pk, direction.to_i64(), body, sent_at, expires_at, origin_msg_id])?;
            let id = tx.last_insert_rowid();
            insert_attachments(tx, id, attachments)?;
            Ok(id)
        })
    }

    pub fn list_messages(
        &self,
        contact_id: i64,
        limit: i64,
        before_id: Option<i64>,
    ) -> Result<Vec<Message>> {
        self.with_conn(|c| match before_id {
            Some(b) => c
                .prepare_cached(concat!(
                    "SELECT ",
                    message_cols!(),
                    " FROM messages WHERE contact_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3"
                ))?
                .query_map(params![contact_id, b, limit], Self::map_message)?
                .collect_rows(),
            None => c
                .prepare_cached(concat!(
                    "SELECT ",
                    message_cols!(),
                    " FROM messages WHERE contact_id = ?1 ORDER BY id DESC LIMIT ?2"
                ))?
                .query_map(params![contact_id, limit], Self::map_message)?
                .collect_rows(),
        })
    }

    pub fn list_group_messages(
        &self,
        group_id: &[u8],
        limit: i64,
        before_id: Option<i64>,
    ) -> Result<Vec<Message>> {
        self.with_conn(|c| match before_id {
            Some(b) => c
                .prepare_cached(concat!(
                    "SELECT ",
                    message_cols!(),
                    " FROM messages WHERE group_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3"
                ))?
                .query_map(params![group_id, b, limit], Self::map_message)?
                .collect_rows(),
            None => c
                .prepare_cached(concat!(
                    "SELECT ",
                    message_cols!(),
                    " FROM messages WHERE group_id = ?1 ORDER BY id DESC LIMIT ?2"
                ))?
                .query_map(params![group_id, limit], Self::map_message)?
                .collect_rows(),
        })
    }

    pub fn message_position_dm(&self, contact_id: i64, message_id: i64) -> Result<Option<i64>> {
        self.with_conn(|c| {
            let exists: bool = c.prepare_cached(
                "SELECT 1 FROM messages WHERE id = ?1 AND contact_id = ?2 AND group_id IS NULL")?
                .query_row(params![message_id, contact_id], |_| Ok(())).optional()?.is_some();
            if !exists { return Ok(None); }
            let n: i64 = c.prepare_cached(
                "SELECT COUNT(*) FROM messages WHERE contact_id = ?1 AND group_id IS NULL AND id > ?2")?
                .query_row(params![contact_id, message_id], |r| r.get(0))?;
            Ok(Some(n))
        })
    }

    pub fn message_position_group(&self, group_id: &[u8], message_id: i64) -> Result<Option<i64>> {
        self.with_conn(|c| {
            let exists: bool = c
                .prepare_cached("SELECT 1 FROM messages WHERE id = ?1 AND group_id = ?2")?
                .query_row(params![message_id, group_id], |_| Ok(()))
                .optional()?
                .is_some();
            if !exists {
                return Ok(None);
            }
            let n: i64 = c
                .prepare_cached("SELECT COUNT(*) FROM messages WHERE group_id = ?1 AND id > ?2")?
                .query_row(params![group_id, message_id], |r| r.get(0))?;
            Ok(Some(n))
        })
    }

    pub fn mark_read(&self, contact_id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE messages SET read = 1 WHERE contact_id = ?1 AND direction = 0 AND read = 0",
                params![contact_id],
            )?;
            Ok(())
        })
    }

    pub fn mark_group_read(&self, group_id: &[u8]) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE messages SET read = 1 WHERE group_id = ?1 AND direction = 0 AND read = 0",
                params![group_id],
            )?;
            Ok(())
        })
    }

    pub fn unread_count(&self, contact_id: i64) -> Result<i64> {
        self.with_conn(|c| {
            Ok(c.prepare_cached(
            "SELECT COUNT(*) FROM messages WHERE contact_id = ?1 AND direction = 0 AND read = 0")?
            .query_row(params![contact_id], |r| r.get(0))?)
        })
    }

    pub fn group_unread_count(&self, group_id: &[u8]) -> Result<i64> {
        self.with_conn(|c| {
            Ok(c.prepare_cached(
                "SELECT COUNT(*) FROM messages WHERE group_id = ?1 AND direction = 0 AND read = 0",
            )?
            .query_row(params![group_id], |r| r.get(0))?)
        })
    }

    /// Every message in the 1:1 chat with `contact_id` (attachments go with
    /// them by cascade). The contact itself stays.
    pub fn delete_messages_for_contact(&self, contact_id: i64) -> Result<usize> {
        self.with_conn(|c| {
            Ok(c.execute(
                "DELETE FROM messages WHERE contact_id = ?1 AND group_id IS NULL",
                params![contact_id],
            )?)
        })
    }

    pub fn delete_message(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM messages WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn purge_expired(&self, now: i64) -> Result<usize> {
        let _files = self.file_mutations.lock().map_err(|_| DbError::State)?;
        self.with_tx(|tx| {
            tx.execute("DELETE FROM delivery_receipts WHERE deadline<=?1", [now])?;
            let ids = tx.prepare(
                "SELECT id FROM messages WHERE group_id IS NOT NULL AND expires_at <= ?1"
            )?.query_map(params![now], |row| row.get::<_, i64>(0))?.collect_rows()?;
            for id in &ids {
                tx.execute("INSERT OR IGNORE INTO deleted_file_cleanup(path)
                    SELECT path FROM attachments WHERE message_id=?1
                    UNION SELECT path FROM file_in WHERE message_id=?1", params![id])?;
                tx.execute("DELETE FROM settings WHERE k='sound_' || ?1", params![id])?;
                tx.execute("DELETE FROM messages WHERE id=?1", params![id])?;
            }
            Ok(ids.len())
        })
    }

    /// Preserve the existing local group-history cleanup, including pins.
    /// DM cleanup uses `clear_dm_history` so durable references and deletion
    /// obligations survive instead of physically dropping those messages.
    pub fn purge_all_messages(&self) -> Result<usize> {
        self.with_conn(|c| Ok(c.execute(
            "DELETE FROM messages WHERE group_id IS NOT NULL AND id NOT IN (SELECT message_id FROM pinned_messages)", [],
        )?))
    }

    /// Clear one group's local history without affecting saved chats.
    pub fn purge_group_messages(&self, group_id: &[u8]) -> Result<usize> {
        self.with_conn(|c| Ok(c.execute(
            "DELETE FROM messages WHERE group_id = ?1 AND id NOT IN (SELECT message_id FROM pinned_messages)",
            params![group_id],
        )?))
    }

    pub(super) fn map_message(r: &rusqlite::Row) -> rusqlite::Result<Message> {
        Ok(Message {
            id: r.get(0)?,
            contact_id: r.get(1)?,
            group_id: r.get(2)?,
            sender_sign_pk: r.get(3)?,
            direction: Direction::from_i64(r.get(4)?),
            body: r.get(5)?,
            sent_at: r.get(6)?,
            sent: r.get::<_, i64>(7)? != 0,
            delivered: r.get::<_, i64>(8)? != 0,
            read: r.get::<_, i64>(9)? != 0,
            expires_at: r.get(10)?,
            last_attempt_at: r.get(11)?,
            send_attempts: r.get(12)?,
            reply_to: r.get(13)?,
            dm_seq: r.get(14)?,
            deleted_at: r.get(15)?,
            delivery_expired_at: r.get(16)?,
        })
    }

    pub fn search_messages(
        &self,
        needle: &str,
        contact_id: Option<i64>,
        group_id: Option<&[u8]>,
        limit: i64,
    ) -> Result<Vec<Message>> {
        let query = fts_prefix_query(needle);
        if query.is_empty() {
            return Ok(Vec::new());
        }
        self.with_conn(|c| match (contact_id, group_id) {
            (Some(cid), _) => c
                .prepare_cached(concat!(
                    "SELECT ",
                    message_cols_m!(),
                    " FROM messages m JOIN messages_fts f ON f.rowid = m.id
                  WHERE messages_fts MATCH ?1 AND m.contact_id = ?2 ORDER BY m.id DESC LIMIT ?3"
                ))?
                .query_map(params![query, cid, limit], Self::map_message)?
                .collect_rows(),
            (_, Some(gid)) => c
                .prepare_cached(concat!(
                    "SELECT ",
                    message_cols_m!(),
                    " FROM messages m JOIN messages_fts f ON f.rowid = m.id
                  WHERE messages_fts MATCH ?1 AND m.group_id = ?2 ORDER BY m.id DESC LIMIT ?3"
                ))?
                .query_map(params![query, gid, limit], Self::map_message)?
                .collect_rows(),
            (None, None) => c
                .prepare_cached(concat!(
                    "SELECT ",
                    message_cols_m!(),
                    " FROM messages m JOIN messages_fts f ON f.rowid = m.id
                  WHERE messages_fts MATCH ?1 ORDER BY m.id DESC LIMIT ?2"
                ))?
                .query_map(params![query, limit], Self::map_message)?
                .collect_rows(),
        })
    }
}

#[cfg(test)]
mod history_retention_tests {
    use super::*;

    #[test]
    fn group_ttl_expires_pins_and_preserves_attachment_cleanup_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let db = Db::open_plain(&path).unwrap();
        db.create_group(b"group", "group").unwrap();
        let attachment = NewAttachment {
            name: "photo".into(), size: 1, key: vec![1; 32], path: "part.bin".into(), chunk_size: None,
        };
        std::fs::write(dir.path().join(&attachment.path), [1]).unwrap();
        let msg = db.insert_group_message(b"group", Some(&[3; 32]), Direction::In,
            "photo", 100, Some(200), &[attachment]).unwrap();
        db.pin_group_message(b"group", msg).unwrap();
        assert_eq!(db.purge_expired(199).unwrap(), 0);
        assert_eq!(db.purge_expired(200).unwrap(), 1);
        assert!(db.get_message(msg).unwrap().is_none());
        drop(db);
        let db = Db::open_plain(&path).unwrap();
        assert_eq!(db.cleanup_deleted_files(dir.path()).unwrap(), 1);
        assert!(!dir.path().join("part.bin").exists());
        assert_eq!(db.purge_expired(200).unwrap(), 0);
    }

    #[test]
    fn clearing_one_group_preserves_other_chats_and_existing_group_pins() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("history.db")).unwrap();
        db.configure_dm_identity(&[7; 32]).unwrap();
        let contact = db
            .add_contact(&[1; 32], &[2; 32], "", "peer", None)
            .unwrap();
        let direct = db
            .insert_message(contact, Direction::Out, "saved direct", now_ms(), None, &[])
            .unwrap();
        db.create_group(b"saved", "saved group").unwrap();
        db.create_group(b"clear", "clear group").unwrap();
        let saved = db
            .insert_group_message(
                b"saved",
                Some(&[7; 32]),
                Direction::Out,
                "keep",
                now_ms(),
                None,
                &[],
            )
            .unwrap();
        let cleared = db
            .insert_group_message(
                b"clear",
                Some(&[7; 32]),
                Direction::Out,
                "clear",
                now_ms(),
                None,
                &[],
            )
            .unwrap();
        let pinned = db
            .insert_group_message(
                b"clear",
                Some(&[7; 32]),
                Direction::Out,
                "pin",
                now_ms(),
                None,
                &[],
            )
            .unwrap();
        db.pin_group_message(b"clear", pinned).unwrap();
        assert_eq!(db.purge_group_messages(b"clear").unwrap(), 1);
        assert!(db.get_message(cleared).unwrap().is_none());
        assert_eq!(db.get_message(saved).unwrap().unwrap().body, "keep");
        assert_eq!(
            db.get_message(direct).unwrap().unwrap().body,
            "saved direct"
        );
        assert_eq!(db.get_message(pinned).unwrap().unwrap().body, "pin");
    }
}
