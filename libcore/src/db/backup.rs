//! Bulk snapshot export/import database operations.

use super::*;

impl Db {
    pub fn list_all_messages(&self) -> Result<Vec<Message>> {
        self.with_conn(|c| {
            c.prepare_cached(concat!(
                "SELECT ",
                message_cols!(),
                " FROM messages ORDER BY id ASC"
            ))?
            .query_map([], Self::map_message)?
            .collect_rows()
        })
    }

    pub fn list_all_attachments(&self) -> Result<Vec<Attachment>> {
        self.with_conn(|c| c.prepare_cached("SELECT id, message_id, name, size, key, path, chunk_size FROM attachments ORDER BY id ASC")?
            .query_map([], map_attachment)?.collect_rows())
    }

    pub fn list_all_pinned(&self) -> Result<Vec<(Option<i64>, Option<Vec<u8>>, i64, i64)>> {
        self.with_conn(|c| {
            c.prepare_cached(
                "SELECT contact_id, group_id, message_id, pinned_at FROM pinned_messages",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect_rows()
        })
    }

    pub fn list_all_prekeys(&self) -> Result<Vec<PreKey>> {
        self.with_conn(|c| {
            c.prepare_cached(
                "SELECT id, kind, private, public, created_at FROM prekeys ORDER BY id ASC",
            )?
            .query_map([], Self::map_prekey)?
            .collect_rows()
        })
    }

    pub fn list_all_settings(&self) -> Result<Vec<(String, Vec<u8>)>> {
        self.with_conn(|c| {
            c.prepare_cached("SELECT k, v FROM settings")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect_rows()
        })
    }

    pub fn bulk_insert_messages(&self, msgs: &[Message]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "INSERT OR IGNORE INTO messages (id, contact_id, group_id, sender_sign_pk, direction, body, sent_at, sent, delivered, read, expires_at, last_attempt_at, send_attempts, reply_to)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)")?;
            for m in msgs {
                s.execute(params![
                    m.id, m.contact_id, m.group_id, m.sender_sign_pk, m.direction.to_i64(), m.body, m.sent_at,
                    m.sent as i64, m.delivered as i64, m.read as i64,
                    m.expires_at, m.last_attempt_at, m.send_attempts, m.reply_to,
                ])?;
            }
            Ok(())
        })
    }

    pub fn bulk_insert_attachments(&self, atts: &[Attachment]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "INSERT OR IGNORE INTO attachments (id, message_id, name, size, key, path, chunk_size) VALUES (?1,?2,?3,?4,?5,?6,?7)")?;
            for a in atts {
                s.execute(params![a.id, a.message_id, a.name, a.size, a.key, a.path, a.chunk_size])?;
            }
            Ok(())
        })
    }

    pub fn bulk_insert_pinned(
        &self,
        rows: &[(Option<i64>, Option<Vec<u8>>, i64, i64)],
    ) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "INSERT OR IGNORE INTO pinned_messages (contact_id, group_id, message_id, pinned_at) VALUES (?1,?2,?3,?4)")?;
            for (cid, gid, mid, ts) in rows {
                s.execute(params![cid, gid, mid, ts])?;
            }
            Ok(())
        })
    }

    pub fn bulk_insert_prekeys(&self, prekeys: &[PreKey]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "INSERT OR IGNORE INTO prekeys (id, kind, private, public, created_at) VALUES (?1,?2,?3,?4,?5)")?;
            for p in prekeys {
                s.execute(params![p.id, p.kind as i64, p.private, p.public, p.created_at])?;
            }
            Ok(())
        })
    }

    pub fn bulk_set_settings(&self, rows: &[(String, Vec<u8>)]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s =
                tx.prepare_cached("INSERT OR REPLACE INTO settings (k, v) VALUES (?1, ?2)")?;
            for (k, v) in rows {
                s.execute(params![k, v])?;
            }
            Ok(())
        })
    }

    pub fn list_all_group_members(&self) -> Result<HashMap<Vec<u8>, Vec<GroupMember>>> {
        self.with_conn(|c| {
            let mut s = c.prepare_cached(
                "SELECT group_id, sign_pk, dh_pk, onion, display_name, is_self FROM group_members ORDER BY group_id, display_name")?;
            let mut out: HashMap<Vec<u8>, Vec<GroupMember>> = HashMap::new();
            for row in s.query_map([], map_group_member)? {
                let m = row?;
                out.entry(m.group_id.clone()).or_default().push(m);
            }
            Ok(out)
        })
    }

    pub fn bulk_add_group_members(&self, members: &[GroupMember]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "INSERT OR REPLACE INTO group_members (group_id, sign_pk, dh_pk, onion, display_name, is_self)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
            for m in members {
                s.execute(params![m.group_id, m.sign_pk, m.dh_pk, m.onion, m.display_name, m.is_self as i64])?;
            }
            Ok(())
        })
    }

    pub fn bulk_update_contacts(&self, rows: &[(i64, String, TrustLevel, bool)]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "UPDATE contacts SET display_name = ?2, trust = ?3, is_bot = ?4 WHERE id = ?1",
            )?;
            for (id, name, trust, is_bot) in rows {
                s.execute(params![id, name, trust.to_i64(), *is_bot as i64])?;
            }
            Ok(())
        })
    }
}
