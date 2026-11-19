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
                "INSERT OR IGNORE INTO messages (id, contact_id, group_id, sender_sign_pk, direction, body, sent_at, sent, delivered, read, expires_at, last_attempt_at, send_attempts, reply_to, delivery_expired_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)")?;
            for m in msgs {
                s.execute(params![
                    m.id, m.contact_id, m.group_id, m.sender_sign_pk, m.direction.to_i64(), m.body, m.sent_at,
                    m.sent as i64, m.delivered as i64, m.read as i64,
                    m.expires_at, m.last_attempt_at, m.send_attempts, m.reply_to, m.delivery_expired_at,
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
                "SELECT group_id, sign_pk, dh_pk, i2p_address, display_name, is_self FROM group_members ORDER BY group_id, display_name")?;
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
                "INSERT OR REPLACE INTO group_members (group_id, sign_pk, dh_pk, i2p_address, display_name, is_self)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
            for m in members {
                s.execute(params![m.group_id, m.sign_pk, m.dh_pk, m.i2p_address, m.display_name, m.is_self as i64])?;
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

/// Keep the positional v2/v3 backup readable while preserving final delivery
/// status and notices, including notices whose message content is already gone.
#[derive(serde::Serialize, serde::Deserialize)]
struct DeliveryBackup {
    expired: Vec<(i64, i64)>,
    // The restored profile assigns new contact IDs. Store the identity instead.
    notices: Vec<(i64, Option<Vec<u8>>, Option<Vec<u8>>, i64)>,
}

impl Db {
    pub const DELIVERY_BACKUP_KEY: &'static str = "delivery_expiry_v1";

    pub fn export_delivery_state(&self) -> Result<Vec<u8>> {
        self.with_conn(|conn| {
            let expired = conn
                .prepare(
                    "SELECT id, delivery_expired_at FROM messages
                WHERE delivery_expired_at IS NOT NULL",
                )?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect_rows()?;
            let notices = conn
                .prepare(
                    "SELECT n.message_id, c.identity_sign, n.group_id, n.expired_at
                FROM delivery_expiry_notices n LEFT JOIN contacts c ON c.id=n.contact_id",
                )?
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect_rows()?;
            bincode::serialize(&DeliveryBackup { expired, notices }).map_err(|_| DbError::State)
        })
    }

    pub fn restore_delivery_state(&self) -> Result<()> {
        let Some(bytes) = self.get_setting(Self::DELIVERY_BACKUP_KEY)? else {
            return Ok(());
        };
        let backup: DeliveryBackup = bincode::deserialize(&bytes).map_err(|_| DbError::State)?;
        self.with_tx(|tx| {
            for (id, expired_at) in backup.expired {
                tx.execute("UPDATE messages SET delivery_expired_at=?2 WHERE id=?1 AND direction=1 AND delivered=0",
                    params![id, expired_at])?;
            }
            for (id, identity, group_id, expired_at) in backup.notices {
                let contact_id: Option<i64> = identity.map(|key| {
                    tx.query_row("SELECT id FROM contacts WHERE identity_sign=?1", [key], |r| r.get(0))
                }).transpose()?;
                tx.execute("INSERT INTO delivery_expiry_notices(message_id,contact_id,group_id,expired_at)
                    VALUES (?1,?2,?3,?4)", params![id, contact_id, group_id, expired_at])?;
            }
            // An expired group row can have a higher ID than every retained
            // message. Reserve its ID while its notification is outstanding.
            tx.execute("INSERT INTO sqlite_sequence(name,seq) SELECT 'messages',0
                WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name='messages')", [])?;
            tx.execute("UPDATE sqlite_sequence SET seq=MAX(seq,
                COALESCE((SELECT MAX(message_id) FROM delivery_expiry_notices),0)) WHERE name='messages'", [])?;
            tx.execute("DELETE FROM settings WHERE k=?1", [Self::DELIVERY_BACKUP_KEY])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod delivery_backup_tests {
    use super::*;

    #[test]
    fn backup_preserves_consumed_status_and_pending_notice_for_purged_group() {
        let dir = tempfile::tempdir().unwrap();
        let source = Db::open_plain(&dir.path().join("source.db")).unwrap();
        let peer = source
            .add_contact(&[1; 32], &[2; 32], "", "peer", None)
            .unwrap();
        source.create_group(b"group", "group").unwrap();
        let now = now_ms();
        let dm = source
            .insert_message(peer, Direction::Out, "keep", now, None, &[])
            .unwrap();
        let gid = source
            .insert_group_message(
                b"group",
                Some(&[3; 32]),
                Direction::Out,
                "short",
                now,
                Some(now + 5),
                &[],
            )
            .unwrap();
        source.queue_group_recipients(gid, &[peer]).unwrap();
        source.expire_outgoing_delivery(now + RETRY_TTL_MS).unwrap();
        source.acknowledge_delivery_expiry_notices(&[dm]).unwrap();
        source.purge_expired(now + RETRY_TTL_MS).unwrap();
        let backup = source.export_delivery_state().unwrap();

        let restored = Db::open_plain(&dir.path().join("restored.db")).unwrap();
        let peer2 = restored
            .add_contact(&[1; 32], &[2; 32], "", "peer", None)
            .unwrap();
        restored.create_group(b"group", "group").unwrap();
        assert_eq!(
            restored
                .insert_message(peer2, Direction::Out, "keep", now, None, &[])
                .unwrap(),
            dm
        );
        restored
            .set_setting(Db::DELIVERY_BACKUP_KEY, &backup)
            .unwrap();
        restored.restore_delivery_state().unwrap();
        assert_eq!(
            restored
                .get_message(dm)
                .unwrap()
                .unwrap()
                .delivery_expired_at,
            Some(now + RETRY_TTL_MS)
        );
        assert!(restored
            .expire_outgoing_delivery(now + RETRY_TTL_MS)
            .unwrap()
            .is_empty());
        let notices = restored.list_delivery_expiry_notices().unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].message_id, gid);
        assert_eq!(notices[0].expired_at, now + 5);
        assert!(
            restored
                .insert_message(peer2, Direction::Out, "next", now + 100, None, &[])
                .unwrap()
                > gid
        );
    }
}
