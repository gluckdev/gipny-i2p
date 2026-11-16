//! Attachment metadata queries and removal.

use super::*;

impl Db {
    pub fn list_attachments(&self, message_id: i64) -> Result<Vec<Attachment>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT id, message_id, name, size, key, path, chunk_size FROM attachments WHERE message_id = ?1")?
            .query_map(params![message_id], map_attachment)?.collect_rows())
    }

    pub fn list_attachments_for_messages(
        &self,
        ids: &[i64],
    ) -> Result<HashMap<i64, Vec<Attachment>>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = (0..ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, message_id, name, size, key, path, chunk_size FROM attachments WHERE message_id IN ({})", placeholders);
        self.with_conn(|c| {
            let mut s = c.prepare(&sql)?;
            let mut out: HashMap<i64, Vec<Attachment>> = HashMap::new();
            for row in s.query_map(rusqlite::params_from_iter(ids.iter()), map_attachment)? {
                let a = row?;
                out.entry(a.message_id).or_default().push(a);
            }
            Ok(out)
        })
    }

    pub fn list_attachments_for_contact(
        &self,
        contact_id: i64,
        limit: i64,
    ) -> Result<Vec<(Attachment, i64)>> {
        self.with_conn(|c| {
            c.prepare_cached(
                "SELECT a.id, a.message_id, a.name, a.size, a.key, a.path, a.chunk_size, m.sent_at
             FROM attachments a JOIN messages m ON m.id = a.message_id
             WHERE m.contact_id = ?1 ORDER BY m.id DESC LIMIT ?2",
            )?
            .query_map(params![contact_id, limit], |r| {
                Ok((map_attachment(r)?, r.get(7)?))
            })?
            .collect_rows()
        })
    }

    pub fn list_attachments_for_group(
        &self,
        group_id: &[u8],
        limit: i64,
    ) -> Result<Vec<(Attachment, i64)>> {
        self.with_conn(|c| {
            c.prepare_cached(
                "SELECT a.id, a.message_id, a.name, a.size, a.key, a.path, a.chunk_size, m.sent_at
             FROM attachments a JOIN messages m ON m.id = a.message_id
             WHERE m.group_id = ?1 ORDER BY m.id DESC LIMIT ?2",
            )?
            .query_map(params![group_id, limit], |r| {
                Ok((map_attachment(r)?, r.get(7)?))
            })?
            .collect_rows()
        })
    }

    pub fn get_attachment(&self, id: i64) -> Result<Option<Attachment>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT id, message_id, name, size, key, path, chunk_size FROM attachments WHERE id = ?1")?
            .query_row(params![id], map_attachment).optional().map_err(Into::into))
    }

    pub fn delete_attachment(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM attachments WHERE id = ?1", params![id])?;
            Ok(())
        })
    }
}
