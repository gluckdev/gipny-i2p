//! Contact identity, trust, relay addresses, and introduction requests.

use super::*;

fn relay_timestamp_key(dh: &[u8]) -> String {
    format!("contact_relay_timestamp_{}", hex(dh))
}

fn advance_relay_timestamp(tx: &Transaction<'_>, dh: &[u8], issued: u64) -> Result<bool> {
    // Fixed-width big-endian blobs compare in timestamp order, including
    // unsigned signed-record timestamps outside SQLite's integer range.
    Ok(tx.execute(
        "INSERT INTO settings(k, v) VALUES (?1, ?2)
         ON CONFLICT(k) DO UPDATE SET v=excluded.v WHERE settings.v < excluded.v",
        params![relay_timestamp_key(dh), &issued.to_be_bytes()[..]],
    )? == 1)
}

fn set_direct_relay(tx: &Transaction<'_>, id: i64, relay: Option<&str>) -> Result<()> {
    let dh: Option<Vec<u8>> = tx
        .query_row(
            "UPDATE contacts SET relay_address=?2 WHERE id=?1 AND relay_address IS NOT ?2
         RETURNING identity_dh",
            params![id, relay],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(dh) = dh {
        advance_relay_timestamp(tx, &dh, now_ms() as u64)?;
    }
    Ok(())
}

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
                "INSERT OR IGNORE INTO contacts (identity_sign, identity_dh, i2p_address, display_name, trust, created_at) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                params![sign, dh, i2p_address, name, now_ms()],
            )?;
            let (id,pinned): (i64,Vec<u8>)=conn.query_row("SELECT id,identity_sign FROM contacts WHERE identity_dh=?1",[dh],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if pinned.as_slice()!=sign { return Err(DbError::State); }
            if crate::is_valid_i2p_address(i2p_address) {
                conn.execute("UPDATE contacts SET i2p_address=?2 WHERE id=?1",params![id,i2p_address])?;
            }
            if relay.is_some() {
                set_direct_relay(conn, id, relay)?;
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
        self.with_tx(|tx| set_direct_relay(tx, id, relay))
    }

    /// Apply a signed lookup result only while the contact still matches its
    /// pre-lookup snapshot and the record is fresher than the last route update.
    /// A newer publication of the current address advances its timestamp too.
    pub fn apply_dht_contact_relay(
        &self,
        contact: &Contact,
        relay: &str,
        issued: u64,
    ) -> Result<bool> {
        let relay = relay.trim();
        if relay.is_empty() {
            return Ok(false);
        }
        self.with_tx(|tx| {
            let current: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM contacts WHERE id=?1 AND identity_sign=?2
                 AND identity_dh=?3 AND relay_address IS ?4)",
                params![
                    contact.id,
                    contact.identity_sign,
                    contact.identity_dh,
                    contact.relay_address
                ],
                |r| r.get(0),
            )?;
            if !current || !advance_relay_timestamp(tx, &contact.identity_dh, issued)? {
                return Ok(false);
            }
            if contact
                .relay_address
                .as_deref()
                .is_some_and(|old| old == relay || crate::relay::same_destination(old, relay))
            {
                return Ok(false);
            }
            tx.execute(
                "UPDATE contacts SET relay_address=?2 WHERE id=?1",
                params![contact.id, relay],
            )?;
            Ok(true)
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
        self.with_tx(|tx| {
            let dh: Option<Vec<u8>> = tx
                .query_row(
                    "DELETE FROM contacts WHERE id=?1 RETURNING identity_dh",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(dh) = dh {
                tx.execute(
                    "DELETE FROM settings WHERE k=?1",
                    [relay_timestamp_key(&dh)],
                )?;
            }
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
mod dht_route_tests {
    use super::*;

    #[test]
    fn delayed_lookup_cannot_replace_a_new_direct_route() {
        let db = Db::open_plain(Path::new(":memory:")).unwrap();
        let id = db
            .add_contact(&[1; 32], &[2; 32], "", "peer", Some("initial"))
            .unwrap();
        let snapshot = db.get_contact(id).unwrap().unwrap();
        db.set_contact_relay(id, Some("direct")).unwrap();
        assert!(!db
            .apply_dht_contact_relay(&snapshot, "delayed", u64::MAX)
            .unwrap());

        // Even a subsequent lookup from the current route must reject a
        // publication issued before the direct update.
        let current = db.get_contact(id).unwrap().unwrap();
        assert!(!db.apply_dht_contact_relay(&current, "delayed", 1).unwrap());
        assert_eq!(db.contact_relay(id).unwrap().as_deref(), Some("direct"));
    }

    #[test]
    fn contact_added_with_a_relay_records_its_timestamp() {
        let db = Db::open_plain(Path::new(":memory:")).unwrap();
        let id = db
            .add_contact(&[1; 32], &[2; 32], "", "peer", Some("card"))
            .unwrap();
        let snapshot = db.get_contact(id).unwrap().unwrap();
        assert_eq!(snapshot.relay_address.as_deref(), Some("card"));
        assert!(!db.apply_dht_contact_relay(&snapshot, "stale", 1).unwrap());
        assert_eq!(db.contact_relay(id).unwrap().as_deref(), Some("card"));
    }

    #[test]
    fn repeated_signed_address_advances_freshness_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routes.db");
        let db = Db::open_plain(&path).unwrap();
        let id = db
            .add_contact(&[1; 32], &[2; 32], "", "peer", None)
            .unwrap();
        let snapshot = db.get_contact(id).unwrap().unwrap();
        assert!(db.apply_dht_contact_relay(&snapshot, "first", 10).unwrap());
        let current = db.get_contact(id).unwrap().unwrap();
        assert!(!db.apply_dht_contact_relay(&current, "first", 20).unwrap());
        drop(db);

        let db = Db::open_plain(&path).unwrap();
        assert!(!db.apply_dht_contact_relay(&current, "older", 19).unwrap());
        assert!(!db.apply_dht_contact_relay(&current, "tied", 20).unwrap());
        assert!(db.apply_dht_contact_relay(&current, "newer", 21).unwrap());
        assert_eq!(db.contact_relay(id).unwrap().as_deref(), Some("newer"));
    }

    #[test]
    fn lookup_requires_the_original_contact_identity_and_an_existing_row() {
        let db = Db::open_plain(Path::new(":memory:")).unwrap();
        let id = db
            .add_contact(&[1; 32], &[2; 32], "", "peer", None)
            .unwrap();
        let snapshot = db.get_contact(id).unwrap().unwrap();
        for other in [
            Contact {
                identity_sign: vec![3; 32],
                ..snapshot.clone()
            },
            Contact {
                identity_dh: vec![4; 32],
                ..snapshot.clone()
            },
        ] {
            assert!(!db.apply_dht_contact_relay(&other, "unrelated", 10).unwrap());
        }
        assert!(db.apply_dht_contact_relay(&snapshot, "first", 1).unwrap());
        let current = db.get_contact(id).unwrap().unwrap();
        db.delete_contact(id).unwrap();
        assert!(!db.apply_dht_contact_relay(&current, "gone", 11).unwrap());
        assert!(db
            .get_setting(&relay_timestamp_key(&current.identity_dh))
            .unwrap()
            .is_none());
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
