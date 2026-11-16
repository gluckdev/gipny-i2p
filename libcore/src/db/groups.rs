//! Group identity, membership, and display metadata.

use super::*;

impl Db {
    pub fn create_group(&self, id: &[u8], name: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "INSERT OR IGNORE INTO groups (id, name, created_at) VALUES (?1, ?2, ?3)",
                params![id, name, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn update_group_name(&self, id: &[u8], name: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE groups SET name = ?1 WHERE id = ?2",
                params![name, id],
            )?;
            Ok(())
        })
    }

    pub fn get_group(&self, id: &[u8]) -> Result<Option<Group>> {
        self.with_conn(|c| {
            c.prepare_cached(concat!(select_group!(), " WHERE id = ?1"))?
                .query_row(params![id], map_group)
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn list_groups(&self) -> Result<Vec<Group>> {
        self.with_conn(|c| c.prepare_cached(concat!(
            select_group!(),
            " ORDER BY pinned_at IS NULL, pinned_at DESC, last_message_at IS NULL, last_message_at DESC, name"
        ))?
            .query_map([], map_group)?.collect_rows())
    }

    pub fn delete_group(&self, id: &[u8]) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM groups WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn get_group_name(&self, id: &[u8]) -> Result<Option<String>> {
        self.with_conn(|c| {
            c.prepare_cached("SELECT name FROM groups WHERE id = ?1")?
                .query_row(params![id], |r| r.get::<_, String>(0))
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn is_group_member(&self, group_id: &[u8], sign_pk: &[u8]) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM group_members WHERE group_id = ?1 AND sign_pk = ?2",
                params![group_id, sign_pk],
                |r| r.get::<_, i64>(0),
            )? > 0)
        })
    }

    pub fn add_group_member(
        &self,
        group_id: &[u8],
        sign_pk: &[u8],
        dh_pk: &[u8],
        onion: &str,
        name: &str,
        is_self: bool,
    ) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT OR REPLACE INTO group_members (group_id, sign_pk, dh_pk, onion, display_name, is_self)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![group_id, sign_pk, dh_pk, onion, name, is_self as i64])?; Ok(()) })
    }

    pub fn update_group_member_name(
        &self,
        group_id: &[u8],
        sign_pk: &[u8],
        name: &str,
    ) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.execute(
                "UPDATE group_members SET display_name = ?1
             WHERE group_id = ?2 AND sign_pk = ?3 AND display_name <> ?1",
                params![name, group_id, sign_pk],
            )? > 0)
        })
    }

    pub fn list_groups_with_member(&self, sign_pk: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.with_conn(|c| {
            c.prepare_cached("SELECT group_id FROM group_members WHERE sign_pk = ?1")?
                .query_map(params![sign_pk], |r| r.get::<_, Vec<u8>>(0))?
                .collect_rows()
        })
    }

    pub fn list_group_members(&self, group_id: &[u8]) -> Result<Vec<GroupMember>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT group_id, sign_pk, dh_pk, onion, display_name, is_self FROM group_members WHERE group_id = ?1 ORDER BY display_name")?
            .query_map(params![group_id], map_group_member)?.collect_rows())
    }
}
