//! Durable inbox: a decrypted letter is stored in the same transaction that
//! saves the ratchet state (and spends the one-time prekey) that read it.
//!
//! Decrypting consumes the message key, so a relay copy can never be read
//! again: before this, a crash or a full disk between saving the ratchet and
//! saving the message lost the message for good, while the relay had already
//! been told to drop it. Now the relay is acknowledged only after this commit,
//! and the letter is processed from here, again after a restart if need be.
//! Rows live in the encrypted profile database and go once processed.
use super::*;

/// Letters kept waiting at most this long: something that keeps failing for
/// a week is not going to succeed, and the plaintext must not linger.
pub const INBOX_TTL_MS: i64 = 7 * 24 * 3600 * 1000;

pub struct InboxLetter {
    pub id: i64,
    pub contact_id: i64,
    pub payload: Zeroizing<Vec<u8>>,
}

impl Db {
    pub(super) fn migrate_inbox(conn: &Connection) -> Result<()> {
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS inbox (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
                payload BLOB NOT NULL,
                received_at INTEGER NOT NULL
            );")?;
        Ok(())
    }

    /// Save what reading one letter changed, and the letter, at once:
    /// the new ratchet state (`None` for a session kept only in memory), the
    /// one-time prekey it used (`None` if none), and the plaintext.
    ///
    /// The relay is told to drop its copy right after this returns, so this
    /// one commit is synced to disk (`synchronous = FULL`), not merely ordered
    /// in the WAL as the rest of the database is.
    pub fn commit_received(
        &self, contact_id: i64, session: Option<&[u8]>, spent_prekey: Option<i64>, payload: &[u8],
    ) -> Result<i64> {
        let mut conn = self.conn.lock().map_err(|_| DbError::State)?;
        conn.execute_batch("PRAGMA synchronous = FULL")?;
        let committed = Self::commit_received_tx(&mut conn, contact_id, session, spent_prekey, payload);
        let restored = conn.execute_batch("PRAGMA synchronous = NORMAL");
        let id = committed?;
        restored?;
        Ok(id)
    }

    fn commit_received_tx(
        conn: &mut Connection, contact_id: i64, session: Option<&[u8]>, spent_prekey: Option<i64>, payload: &[u8],
    ) -> Result<i64> {
        let tx = conn.transaction()?;
        let id = {
            if let Some(id) = spent_prekey {
                tx.execute("DELETE FROM prekeys WHERE id=?1", params![id])?;
            }
            if let Some(state) = session {
                tx.execute(
                    "INSERT INTO sessions (contact_id, state, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(contact_id) DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at",
                    params![contact_id, state, now_ms()])?;
            }
            tx.execute("INSERT INTO inbox (contact_id, payload, received_at) VALUES (?1, ?2, ?3)",
                params![contact_id, payload, now_ms()])?;
            tx.last_insert_rowid()
        };
        tx.commit()?;
        Ok(id)
    }

    pub fn inbox_done(&self, id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute("DELETE FROM inbox WHERE id=?1", params![id])?; Ok(()) })
    }

    /// Letters still to process, oldest first. Expired ones are dropped here.
    pub fn inbox_pending(&self, limit: usize) -> Result<Vec<InboxLetter>> {
        self.with_conn(|c| {
            c.execute("DELETE FROM inbox WHERE received_at < ?1", params![now_ms() - INBOX_TTL_MS])?;
            c.prepare_cached("SELECT id, contact_id, payload FROM inbox ORDER BY id LIMIT ?1")?
                .query_map(params![limit as i64], |r| Ok(InboxLetter {
                    id: r.get(0)?, contact_id: r.get(1)?, payload: Zeroizing::new(r.get(2)?),
                }))?.collect_rows()
        })
    }
}

/// Worth retrying later: the database or disk failed, not the letter.
/// A constraint violation is the letter's own fault and stays permanent.
pub fn is_transient(error: &DbError) -> bool {
    match error {
        DbError::Io(_) | DbError::Memory(_) => true,
        DbError::Sql(rusqlite::Error::SqliteFailure(e, _)) => matches!(e.code,
            rusqlite::ErrorCode::DiskFull | rusqlite::ErrorCode::DatabaseBusy
            | rusqlite::ErrorCode::DatabaseLocked | rusqlite::ErrorCode::SystemIoFailure
            | rusqlite::ErrorCode::OutOfMemory | rusqlite::ErrorCode::CannotOpen
            | rusqlite::ErrorCode::ReadOnly | rusqlite::ErrorCode::FileLockingProtocolFailed),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> (tempfile::TempDir, Db, i64) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("t.db")).unwrap();
        let peer = db.add_contact(&[2; 32], &[3; 32], "", "peer", None).unwrap();
        (dir, db, peer)
    }

    #[test]
    fn session_prekey_and_letter_commit_together_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = Db::open_plain(&path).unwrap();
        let peer = db.add_contact(&[2; 32], &[3; 32], "", "peer", None).unwrap();
        let opk = db.add_prekey(PreKeyKind::OneTime, &[7; 32], &[8; 32]).unwrap();
        let id = db.commit_received(peer, Some(b"state-1"), Some(opk), b"letter").unwrap();
        drop(db);
        // A crash right after the commit: everything is there on reopen.
        let db = Db::open_plain(&path).unwrap();
        assert_eq!(db.get_session(peer).unwrap().as_deref(), Some(b"state-1".as_slice()));
        assert!(db.get_prekey(opk).unwrap().is_none(), "the prekey is spent with the letter");
        let pending = db.inbox_pending(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!((pending[0].id, pending[0].contact_id, pending[0].payload.as_slice()), (id, peer, b"letter".as_slice()));
        db.inbox_done(id).unwrap();
        assert!(db.inbox_pending(10).unwrap().is_empty());
    }

    #[test]
    fn a_failed_commit_leaves_session_and_prekey_untouched() {
        let (_dir, db, peer) = database();
        db.put_session(peer, b"old").unwrap();
        let opk = db.add_prekey(PreKeyKind::OneTime, &[7; 32], &[8; 32]).unwrap();
        // A contact that does not exist violates the inbox foreign key: the
        // whole transaction rolls back.
        assert!(db.commit_received(peer + 1000, Some(b"new"), Some(opk), b"letter").is_err());
        assert_eq!(db.get_session(peer).unwrap().as_deref(), Some(b"old".as_slice()));
        assert!(db.get_prekey(opk).unwrap().is_some());
        assert!(db.inbox_pending(10).unwrap().is_empty());
    }

    #[test]
    fn letters_go_with_their_contact_and_keep_their_order() {
        let (_dir, db, peer) = database();
        let first = db.commit_received(peer, None, None, b"1").unwrap();
        let second = db.commit_received(peer, None, None, b"2").unwrap();
        assert_eq!(db.inbox_pending(10).unwrap().iter().map(|l| l.id).collect::<Vec<_>>(), vec![first, second]);
        db.delete_contact(peer).unwrap();
        assert!(db.inbox_pending(10).unwrap().is_empty());
    }

    #[test]
    fn storage_failures_are_transient_and_constraint_violations_are_not() {
        let full = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL), None);
        assert!(is_transient(&DbError::Sql(full)));
        assert!(is_transient(&DbError::Io(std::io::Error::other("disk"))));
        let constraint = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT), None);
        assert!(!is_transient(&DbError::Sql(constraint)));
        assert!(!is_transient(&DbError::State));
        assert!(!is_transient(&DbError::TooLarge));
    }
}
