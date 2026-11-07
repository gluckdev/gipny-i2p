// Included by dm.rs. What the other participant did with a message of this
// direct chat on their device — copied its text, saved its attachment, pressed
// a screenshot shortcut — reported by their client over the E2E session and
// shown here only. No content travels or is stored: only which message, what,
// when and how many times. Deleting the message, by either side or by its
// timer, deletes these too, and drops reports still waiting to go out.
//
// Wire: WireDm kinds 5/6/7 carry the event, 8/9/10 confirm receipt of 5/6/7.

pub const PEER_EVENT_COPIED: u8 = 5;
pub const PEER_EVENT_SAVED: u8 = 6;
pub const PEER_EVENT_SCREENSHOT: u8 = 7;
/// Receipt for an event: its kind plus this.
pub const PEER_EVENT_RECEIPT_OFFSET: u8 = 3;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerEvent {
    pub kind: u8,
    pub at: i64,
    pub count: i64,
}

pub fn is_peer_event_kind(kind: u8) -> bool {
    (PEER_EVENT_COPIED..=PEER_EVENT_SCREENSHOT).contains(&kind)
}

impl Db {
    pub(super) fn migrate_peer_events(conn: &Connection) -> Result<()> {
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS dm_peer_events (
                message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                kind INTEGER NOT NULL, at INTEGER NOT NULL, count INTEGER NOT NULL DEFAULT 1,
                PRIMARY KEY(message_id, kind)
            );
            CREATE TABLE IF NOT EXISTS dm_peer_event_outbox (
                contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
                message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                kind INTEGER NOT NULL, at INTEGER NOT NULL,
                last_attempt_at INTEGER, attempts INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(message_id, kind)
            );")?;
        Ok(())
    }

    /// Queue a report of what we did with a message of a direct chat. Copying
    /// or saving is reported only for the contact's own messages (their
    /// content); a screenshot shortcut for any live message of the chat.
    /// Returns the contact to kick the send loop for.
    pub fn queue_peer_event(&self, message_id: i64, kind: u8) -> Result<i64> {
        if !is_peer_event_kind(kind) { return Err(DbError::State); }
        self.with_tx(|tx| {
            let (contact_id, incoming, deleted): (i64, bool, bool) = tx.query_row(
                "SELECT contact_id, direction=0, deleted_at IS NOT NULL FROM messages
                 WHERE id=?1 AND contact_id IS NOT NULL AND group_id IS NULL",
                params![message_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            if deleted || (kind != PEER_EVENT_SCREENSHOT && !incoming) { return Err(DbError::State); }
            dm_reference_conn(tx, message_id)?;
            tx.execute("INSERT INTO dm_peer_event_outbox(contact_id,message_id,kind,at) VALUES(?1,?2,?3,?4)
                ON CONFLICT(message_id,kind) DO UPDATE SET at=excluded.at, attempts=0, last_attempt_at=NULL",
                params![contact_id, message_id, kind, now_ms()])?;
            Ok(contact_id)
        })
    }

    /// Reports due to go to this contact, with backoff between attempts.
    pub fn pending_peer_events(&self, contact_id: i64, now: i64) -> Result<Vec<(DmReference, u8)>> {
        self.with_conn(|conn| {
            let rows: Vec<(i64, u8)> = conn.prepare_cached("SELECT message_id,kind FROM dm_peer_event_outbox
                WHERE contact_id=?1 AND (last_attempt_at IS NULL OR ?2-last_attempt_at >= MIN(300000,5000*(1 << MIN(attempts,16))))
                ORDER BY at LIMIT 50")?.query_map(params![contact_id, now], |r| Ok((r.get(0)?, r.get(1)?)))?.collect_rows()?;
            rows.into_iter().map(|(id, kind)| Ok((dm_reference_conn(conn, id)?, kind))).collect()
        })
    }

    pub fn record_peer_event_attempt(&self, contact_id: i64, reference: &DmReference, kind: u8, now: i64) -> Result<()> {
        self.with_conn(|conn| {
            if let Some(id) = dm_message_by_reference(conn, contact_id, reference)? {
                conn.execute("UPDATE dm_peer_event_outbox SET attempts=MIN(attempts+1,30),last_attempt_at=?3
                    WHERE message_id=?1 AND kind=?2", params![id, kind, now])?;
            }
            Ok(())
        })
    }

    /// Their receipt arrived: stop sending that report.
    pub fn confirm_peer_event(&self, contact_id: i64, reference: &DmReference, kind: u8) -> Result<()> {
        validate_reference(reference)?;
        self.with_conn(|conn| {
            if let Some(id) = dm_message_by_reference(conn, contact_id, reference)? {
                conn.execute("DELETE FROM dm_peer_event_outbox WHERE message_id=?1 AND kind=?2 AND contact_id=?3",
                    params![id, kind, contact_id])?;
            }
            Ok(())
        })
    }

    /// Record what the contact reports. Returns the message it is about, or
    /// `None` when that message is unknown or already deleted here (the
    /// report is then confirmed and forgotten).
    pub fn receive_peer_event(&self, contact_id: i64, reference: &DmReference, kind: u8, at: i64) -> Result<Option<i64>> {
        validate_reference(reference)?;
        if !is_peer_event_kind(kind) { return Err(DbError::State); }
        let at = at.clamp(0, now_ms());
        self.with_tx(|tx| {
            let Some(id) = dm_message_by_reference(tx, contact_id, reference)? else { return Ok(None) };
            let (outgoing, deleted): (bool, bool) = tx.query_row(
                "SELECT direction=1, deleted_at IS NOT NULL FROM messages WHERE id=?1",
                params![id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            // Copying or saving is about our content; anything else is refused.
            if deleted || (kind != PEER_EVENT_SCREENSHOT && !outgoing) { return Ok(None); }
            tx.execute("INSERT INTO dm_peer_events(message_id,kind,at,count) VALUES(?1,?2,?3,1)
                ON CONFLICT(message_id,kind) DO UPDATE SET at=MAX(at,excluded.at), count=MIN(count+1,1000000)",
                params![id, kind, at])?;
            Ok(Some(id))
        })
    }

    pub fn peer_events(&self, message_id: i64) -> Result<Vec<PeerEvent>> {
        self.with_conn(|conn| conn.prepare_cached("SELECT e.kind,e.at,e.count FROM dm_peer_events e
            JOIN messages m ON m.id=e.message_id WHERE e.message_id=?1 AND m.deleted_at IS NULL ORDER BY e.kind")?
            .query_map(params![message_id], |r| Ok(PeerEvent { kind: r.get(0)?, at: r.get(1)?, count: r.get(2)? }))?
            .collect_rows())
    }
}

/// The message of this direct chat a reference names, whoever wrote it.
fn dm_message_by_reference(conn: &Connection, contact_id: i64, reference: &DmReference) -> Result<Option<i64>> {
    let own: Option<Vec<u8>> = conn.query_row("SELECT v FROM settings WHERE k='dm_self_sign'", [], |r| r.get(0)).optional()?;
    let peer: Vec<u8> = conn.query_row("SELECT identity_sign FROM contacts WHERE id=?1", params![contact_id], |r| r.get(0))?;
    let direction = if own.as_deref() == Some(reference.author.as_slice()) { 1 }
        else if reference.author.as_slice() == peer { 0 }
        else { return Err(DbError::State); };
    Ok(conn.query_row("SELECT id FROM messages WHERE contact_id=?1 AND group_id IS NULL AND direction=?2
        AND origin_msg_id=?3 AND sent_at=?4",
        params![contact_id, direction, reference.origin as i64, reference.sent_at], |r| r.get(0)).optional()?)
}

#[cfg(test)]
mod peer_event_tests {
    use super::*;

    fn pair(dir: &Path) -> ((Db, i64), (Db, i64)) {
        let alice = Db::open_plain(&dir.join("alice.db")).unwrap();
        alice.configure_dm_identity(&[1; 32]).unwrap();
        let bob_id = alice.add_contact(&[2; 32], &[12; 32], "", "bob", None).unwrap();
        let bob = Db::open_plain(&dir.join("bob.db")).unwrap();
        bob.configure_dm_identity(&[2; 32]).unwrap();
        let alice_id = bob.add_contact(&[1; 32], &[11; 32], "", "alice", None).unwrap();
        ((alice, bob_id), (bob, alice_id))
    }

    #[test]
    fn only_the_author_sees_that_their_message_was_copied_and_deletion_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let ((alice, bob_id), (bob, alice_id)) = pair(dir.path());
        let original = alice.insert_message(bob_id, Direction::Out, "secret", 500, None, &[]).unwrap();
        let copy = bob.insert_message_with_origin(alice_id, Direction::In, "secret", 500, None, &[], Some(original)).unwrap();

        // Bob copies Alice's text: queued on Bob's side, nothing stored there.
        assert_eq!(bob.queue_peer_event(copy, PEER_EVENT_COPIED).unwrap(), alice_id);
        let due = bob.pending_peer_events(alice_id, now_ms()).unwrap();
        assert_eq!(due.len(), 1);
        assert!(bob.peer_events(copy).unwrap().is_empty());
        // Bob cannot report copying his own message, nor a non-event kind.
        let own = bob.insert_message(alice_id, Direction::Out, "mine", 600, None, &[]).unwrap();
        assert!(bob.queue_peer_event(own, PEER_EVENT_COPIED).is_err());
        assert!(bob.queue_peer_event(copy, 4).is_err());

        // Alice receives it on her own message, twice: one row, counted.
        let (reference, kind) = due[0].clone();
        assert_eq!(alice.receive_peer_event(bob_id, &reference, kind, 700).unwrap(), Some(original));
        assert_eq!(alice.receive_peer_event(bob_id, &reference, kind, 800).unwrap(), Some(original));
        assert_eq!(alice.peer_events(original).unwrap(), vec![PeerEvent { kind: PEER_EVENT_COPIED, at: 800, count: 2 }]);
        // Her receipt stops Bob repeating it.
        bob.confirm_peer_event(alice_id, &reference, kind).unwrap();
        assert!(bob.pending_peer_events(alice_id, now_ms()).unwrap().is_empty());

        // Deleting the message removes the record with it.
        alice.delete_dm_message(original).unwrap();
        assert!(alice.peer_events(original).unwrap().is_empty());
        let left: i64 = alice.with_conn(|c| Ok(c.query_row("SELECT COUNT(*) FROM dm_peer_events", [], |r| r.get(0))?)).unwrap();
        assert_eq!(left, 0);
        // A late report about a deleted message is not stored.
        assert_eq!(alice.receive_peer_event(bob_id, &reference, kind, 900).unwrap(), None);
    }

    #[test]
    fn a_report_waiting_to_go_out_is_dropped_when_its_message_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let ((_alice, _), (bob, alice_id)) = pair(dir.path());
        let copy = bob.insert_message_with_origin(alice_id, Direction::In, "secret", 500, None, &[], Some(42)).unwrap();
        bob.queue_peer_event(copy, PEER_EVENT_SAVED).unwrap();
        bob.queue_peer_event(copy, PEER_EVENT_SCREENSHOT).unwrap();
        assert_eq!(bob.pending_peer_events(alice_id, now_ms()).unwrap().len(), 2);
        bob.delete_dm_message(copy).unwrap();
        assert!(bob.pending_peer_events(alice_id, now_ms()).unwrap().is_empty());
        assert!(bob.queue_peer_event(copy, PEER_EVENT_COPIED).is_err(), "nothing to report about a deleted message");
    }

    #[test]
    fn a_stranger_or_a_forged_author_cannot_attach_events() {
        let dir = tempfile::tempdir().unwrap();
        let ((alice, bob_id), _) = pair(dir.path());
        let original = alice.insert_message(bob_id, Direction::Out, "secret", 500, None, &[]).unwrap();
        let mut reference = alice.dm_reference(original).unwrap();
        let carol = alice.add_contact(&[3; 32], &[13; 32], "", "carol", None).unwrap();
        // Carol names Alice's message to Bob: not found in her chat.
        assert_eq!(alice.receive_peer_event(carol, &reference, PEER_EVENT_COPIED, 1).unwrap(), None);
        reference.author = [9; 32];
        assert!(alice.receive_peer_event(bob_id, &reference, PEER_EVENT_COPIED, 1).is_err());
        assert!(alice.peer_events(original).unwrap().is_empty());
    }
}
