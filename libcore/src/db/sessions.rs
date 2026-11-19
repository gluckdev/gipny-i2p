//! Persisted ratchet sessions and resynchronization throttling.

use super::*;

impl Db {
    /// Commit the next ratchet state and its retryable ciphertext together.
    /// Until the relay confirms it, retries reuse the exact prepared envelope.
    pub fn seal_for_deposit(
        &self,
        contact_id: i64,
        msg_id: Option<i64>,
        session: &mut crate::crypto::RatchetState,
        plaintext: &[u8],
        ad: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        if let Some(id) = msg_id {
            if let Some(existing) = self.prepared_deposit(contact_id, id)? {
                return Ok(Some(existing));
            }
        }
        let mut candidate = session.clone();
        let (header, ciphertext) = candidate
            .encrypt(plaintext, ad)
            .map_err(|_| DbError::State)?;
        let blob = bincode::serialize(&crate::relay::EnvelopeBlob::Ratchet { header, ciphertext })
            .map_err(|_| DbError::State)?;
        let next = candidate.to_bytes().map_err(|_| DbError::State)?;
        if let Some(id) = msg_id {
            if !self.prepare_deposit(contact_id, id, &next, &blob)? {
                return Ok(None);
            }
        } else {
            self.put_session(contact_id, &next)?;
        }
        *session = candidate;
        Ok(Some(blob))
    }
    /// The new X3DH session and the exact opener owed to the peer survive a crash together.
    pub fn install_outbound_initial(
        &self,
        contact_id: i64,
        state: &[u8],
        envelope: &[u8],
    ) -> Result<bool> {
        self.with_conn(|conn| Ok(conn.execute(
            "INSERT INTO sessions(contact_id,state,updated_at,pending_initial) VALUES(?1,?2,?3,?4) ON CONFLICT(contact_id) DO NOTHING",
            params![contact_id,state,now_ms(),envelope])? != 0))
    }

    pub fn pending_initial(&self, contact_id: i64) -> Result<Option<Vec<u8>>> {
        self.with_conn(|conn| Ok(conn.query_row("SELECT pending_initial FROM sessions WHERE contact_id=?1 AND pending_initial IS NOT NULL",
            [contact_id], |row| row.get(0)).optional()?))
    }

    pub fn confirm_initial(&self, contact_id: i64, envelope: &[u8]) -> Result<()> {
        self.with_conn(|conn| { conn.execute("UPDATE sessions SET pending_initial=NULL WHERE contact_id=?1 AND pending_initial=?2",
            params![contact_id,envelope])?; Ok(()) })
    }

    pub fn prepared_deposit(&self, contact_id: i64, msg_id: i64) -> Result<Option<Vec<u8>>> {
        self.with_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT envelope FROM prepared_deposits WHERE contact_id=?1 AND msg_id=?2",
                    params![contact_id, msg_id],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    pub fn prepare_deposit(
        &self,
        contact_id: i64,
        msg_id: i64,
        state: &[u8],
        envelope: &[u8],
    ) -> Result<bool> {
        self.with_tx(|tx| {
            let added = tx.execute("INSERT INTO prepared_deposits(contact_id, msg_id, envelope)
                SELECT ?1,id,?3 FROM awaiting_delivery WHERE id=?2 AND deadline>?4 AND
                (contact_id=?1 OR EXISTS(SELECT 1 FROM pending_outbound WHERE msg_id=?2 AND recipient_contact_id=?1))",
                params![contact_id,msg_id,envelope,now_ms()])?;
            if added != 0 { tx.execute("UPDATE sessions SET state=?2, updated_at=?3 WHERE contact_id=?1", params![contact_id,state,now_ms()])?; }
            Ok(added != 0)
        })
    }

    pub fn confirm_deposit(&self, contact_id: i64, msg_id: i64, envelope: &[u8]) -> Result<bool> {
        self.with_tx(|tx| {
            let removed = tx.execute(
                "DELETE FROM prepared_deposits WHERE contact_id=?1 AND msg_id=?2 AND envelope=?3",
                params![contact_id, msg_id, envelope],
            )?;
            if removed == 0 {
                return Ok(false);
            }
            let now = now_ms();
            Ok(tx.execute(
                "UPDATE messages SET sent=1, last_attempt_at=?2, send_attempts=send_attempts+1
                WHERE id IN (SELECT id FROM awaiting_delivery WHERE id=?1 AND deadline>?2)",
                params![msg_id, now],
            )? != 0)
        })
    }

    pub fn put_session(&self, contact_id: i64, state: &[u8]) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT INTO sessions (contact_id, state, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(contact_id) DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at",
            params![contact_id, state, now_ms()])?; Ok(()) })
    }

    pub fn get_session(&self, contact_id: i64) -> Result<Option<Vec<u8>>> {
        self.with_conn(|c| {
            c.prepare_cached("SELECT state FROM sessions WHERE contact_id = ?1")?
                .query_row(params![contact_id], |r| r.get::<_, Vec<u8>>(0))
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn has_session(&self, contact_id: i64) -> Result<bool> {
        self.with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE contact_id=?1)",
                params![contact_id],
                |row| row.get(0),
            )?)
        })
    }

    pub fn delete_session(&self, contact_id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM sessions WHERE contact_id = ?1",
                params![contact_id],
            )?;
            Ok(())
        })
    }

    pub fn resync_recent(&self, contact_id: i64, within_ms: i64) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM resync_log WHERE contact_id = ?1 AND last_resync_at >= ?2",
                params![contact_id, now_ms() - within_ms],
                |r| r.get::<_, i64>(0),
            )? > 0)
        })
    }

    pub fn record_resync(&self, contact_id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO resync_log (contact_id, last_resync_at) VALUES (?1, ?2)
             ON CONFLICT(contact_id) DO UPDATE SET last_resync_at = excluded.last_resync_at",
                params![contact_id, now_ms()],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod deposit_tests {
    use super::*;
    use crate::crypto::{Identity, RatchetState};

    #[test]
    fn lost_deposit_ack_reuses_the_exact_ratchet_envelope_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retry.db");
        let db = Db::open_plain(&path).unwrap();
        let peer = Identity::generate().card();
        let id = db
            .add_contact(&peer.sign_pk, &peer.dh_pk, "", "recipient", None)
            .unwrap();
        let msg = db
            .insert_message(id, Direction::Out, "hello", now_ms(), None, &[])
            .unwrap();
        let mut ratchet = RatchetState::new_alice(&[42; 32], &peer.dh_pk);
        let opener = b"exact opener";
        assert!(db
            .install_outbound_initial(id, &ratchet.to_bytes().unwrap(), opener)
            .unwrap());
        let blob = db
            .seal_for_deposit(id, Some(msg), &mut ratchet, b"hello", b"ad")
            .unwrap()
            .unwrap();
        let persisted = db.get_session(id).unwrap().unwrap();
        assert!(!db.get_message(msg).unwrap().unwrap().sent);
        drop(db);
        let db = Db::open_plain(&path).unwrap();
        db.confirm_initial(id, b"stale opener").unwrap();
        assert_eq!(
            db.pending_initial(id).unwrap().as_deref(),
            Some(opener.as_slice())
        );
        db.confirm_initial(id, opener).unwrap();
        assert!(db.pending_initial(id).unwrap().is_none());
        let mut restored = RatchetState::from_bytes(&persisted).unwrap();
        let retry = db
            .seal_for_deposit(id, Some(msg), &mut restored, b"different padding", b"ad")
            .unwrap()
            .unwrap();
        assert_eq!(retry, blob);
        assert_eq!(db.get_session(id).unwrap().unwrap(), persisted);
        db.confirm_deposit(id, msg, b"stale").unwrap();
        assert_eq!(db.prepared_deposit(id, msg).unwrap().as_ref(), Some(&blob));
        assert!(db.acknowledge_dm_delivery(msg, id).unwrap());
        assert!(db.prepared_deposit(id, msg).unwrap().is_none());
        assert!(db
            .seal_for_deposit(id, Some(msg), &mut restored, b"already delivered", b"ad")
            .unwrap()
            .is_none());
        assert_eq!(db.get_session(id).unwrap().unwrap(), persisted);
    }
}
