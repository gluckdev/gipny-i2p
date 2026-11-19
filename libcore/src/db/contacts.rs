//! Contact identity, trust, relay addresses, and introduction requests.

use super::*;

impl Db {
    pub fn add_contact(
        &self,
        sign: &[u8],
        dh: &[u8],
        i2p_address: &str,
        name: &str,
        relay: Option<&str>,
    ) -> Result<i64> {
        let relay = relay.map(str::trim).filter(|r| !r.is_empty());
        self.with_tx(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO contacts (identity_sign, identity_dh, i2p_address, display_name, trust, created_at, relay_address) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
                params![sign, dh, i2p_address, name, now_ms(), relay],
            )?;
            let (id,pinned): (i64,Vec<u8>)=conn.query_row("SELECT id,identity_sign FROM contacts WHERE identity_dh=?1",[dh],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if pinned.as_slice()!=sign { return Err(DbError::State); }
            if crate::is_valid_i2p_address(i2p_address) {
                conn.execute("UPDATE contacts SET i2p_address=?2 WHERE id=?1",params![id,i2p_address])?;
            }
            if let Some(addr)=relay {
                conn.execute("UPDATE contacts SET relay_address=?2 WHERE id=?1",params![id,addr])?;
            }
            Ok(id)
        })
    }

    /// The relay this contact receives through, if their card named one.
    pub fn contact_relay(&self, id: i64) -> Result<Option<String>> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "SELECT relay_address FROM contacts WHERE id = ?1",
                params![id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
        })
    }

    pub fn set_contact_relay(&self, id: i64, relay: Option<&str>) -> Result<()> {
        let relay = relay.map(str::trim).filter(|r| !r.is_empty());
        self.with_conn(|c| {
            c.execute(
                "UPDATE contacts SET relay_address = ?2 WHERE id = ?1",
                params![id, relay],
            )?;
            Ok(())
        })
    }

    pub fn get_contact(&self, id: i64) -> Result<Option<Contact>> {
        self.with_conn(|c| {
            c.prepare_cached(concat!(select_contact!(), " WHERE id = ?1"))?
                .query_row(params![id], Self::map_contact)
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn find_contact_by_identity(&self, dh: &[u8]) -> Result<Option<Contact>> {
        self.with_conn(|c| {
            c.prepare_cached(concat!(select_contact!(), " WHERE identity_dh = ?1"))?
                .query_row(params![dh], Self::map_contact)
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn find_contact_by_sign(&self, sign: &[u8]) -> Result<Option<Contact>> {
        self.with_conn(|c| {
            c.prepare_cached(concat!(select_contact!(), " WHERE identity_sign = ?1"))?
                .query_row(params![sign], Self::map_contact)
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn list_contacts(&self) -> Result<Vec<Contact>> {
        self.with_conn(|c| c.prepare_cached(concat!(
            select_contact!(),
            " ORDER BY pinned_at IS NULL, pinned_at DESC, last_message_at IS NULL, last_message_at DESC, display_name"
        ))?
            .query_map([], Self::map_contact)?.collect_rows())
    }

    pub fn update_contact(&self, id: i64, name: &str, trust: TrustLevel) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE contacts SET display_name = ?1, trust = ?2 WHERE id = ?3",
                params![name, trust.to_i64(), id],
            )?;
            Ok(())
        })
    }

    pub fn update_contact_name(&self, id: i64, name: &str) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.execute(
                "UPDATE contacts SET display_name = ?1 WHERE id = ?2 AND display_name <> ?1",
                params![name, id],
            )? > 0)
        })
    }

    pub fn touch_contact(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE contacts SET last_seen = ?1 WHERE id = ?2",
                params![now_ms(), id],
            )?;
            Ok(())
        })
    }

    /// Record that this contact was heard from at `at` — but never move the
    /// mark backwards. Mail can arrive out of order and days late (the relay
    /// holds it for three days, the archive for seven), and a letter written
    /// on Monday must not make Monday the last time we heard from someone we
    /// spoke to this morning.
    pub fn note_last_seen(&self, id: i64, at: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE contacts SET last_seen = ?1 WHERE id = ?2 AND (last_seen IS NULL OR last_seen < ?1)",
                params![at, id],
            )?;
            Ok(())
        })
    }

    pub fn set_contact_request_state(&self, id: i64, state: RequestState) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE contacts SET request_state = ?2 WHERE id = ?1",
                params![id, state.to_i64()],
            )?;
            Ok(())
        })
    }

    /// Incoming requests, oldest first.
    pub fn list_incoming_requests(&self) -> Result<Vec<i64>> {
        self.with_conn(|c| {
            c.prepare_cached(
                "SELECT id FROM contacts WHERE request_state = 1 ORDER BY created_at ASC, id ASC",
            )?
            .query_map([], |r| r.get(0))?
            .collect_rows()
        })
    }

    /// Origins of the messages a contact sent us, for acknowledging them late.
    pub fn incoming_origins(&self, contact_id: i64) -> Result<Vec<i64>> {
        self.with_conn(|c| {
            c.prepare_cached(
                "SELECT origin_msg_id FROM messages
             WHERE contact_id = ?1 AND direction = 0 AND origin_msg_id IS NOT NULL ORDER BY id ASC",
            )?
            .query_map(params![contact_id], |r| r.get(0))?
            .collect_rows()
        })
    }

    pub fn delete_contact(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM contacts WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    fn map_contact(r: &rusqlite::Row) -> rusqlite::Result<Contact> {
        Ok(Contact {
            id: r.get(0)?,
            identity_sign: r.get(1)?,
            identity_dh: r.get(2)?,
            i2p_address: r.get(3)?,
            display_name: r.get(4)?,
            trust: TrustLevel::from_i64(r.get(5)?),
            created_at: r.get(6)?,
            last_seen: r.get(7)?,
            is_bot: r.get::<_, i64>(8)? != 0,
            pinned_at: r.get(9)?,
            last_message_at: r.get(10)?,
            relay_address: r.get(11)?,
            agent_granted: r.get::<_, i64>(12)? != 0,
            request_state: RequestState::from_i64(r.get(13)?),
        })
    }

    pub fn set_contact_is_bot(&self, id: i64, is_bot: bool) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.execute(
                "UPDATE contacts SET is_bot = ?1 WHERE id = ?2 AND is_bot <> ?1",
                params![is_bot as i64, id],
            )? > 0)
        })
    }

    /// Returns whether the flag actually changed.
    pub fn set_contact_agent_granted(&self, id: i64, granted: bool) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.execute(
                "UPDATE contacts SET agent_granted = ?1 WHERE id = ?2 AND agent_granted <> ?1",
                params![granted as i64, id],
            )? > 0)
        })
    }

    pub fn find_contact_by_sign_pk(&self, sign: &[u8]) -> Result<Option<Contact>> {
        self.find_contact_by_sign(sign)
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    #[test]
    fn requests_round_trip_and_list_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("t.db")).unwrap();
        let a = db.add_contact(&[1; 32], &[2; 32], "", "a", None).unwrap();
        let b = db.add_contact(&[3; 32], &[4; 32], "", "b", None).unwrap();
        let c = db.add_contact(&[5; 32], &[6; 32], "", "c", None).unwrap();
        assert_eq!(
            db.get_contact(a).unwrap().unwrap().request_state,
            RequestState::None
        );

        db.set_contact_request_state(b, RequestState::Incoming)
            .unwrap();
        db.set_contact_request_state(a, RequestState::Incoming)
            .unwrap();
        db.set_contact_request_state(c, RequestState::Outgoing)
            .unwrap();
        assert_eq!(db.list_incoming_requests().unwrap(), vec![a, b]);
        assert_eq!(
            db.get_contact(c).unwrap().unwrap().request_state,
            RequestState::Outgoing
        );

        let now = now_ms();
        db.insert_message_with_origin(a, Direction::In, "hi", now, None, &[], Some(7))
            .unwrap();
        db.insert_message_with_origin(a, Direction::In, "again", now, None, &[], Some(9))
            .unwrap();
        db.insert_message(a, Direction::Out, "mine", now, None, &[])
            .unwrap();
        assert_eq!(db.incoming_origins(a).unwrap(), vec![7, 9]);

        db.set_contact_request_state(a, RequestState::None).unwrap();
        assert_eq!(db.list_incoming_requests().unwrap(), vec![b]);
    }
}
