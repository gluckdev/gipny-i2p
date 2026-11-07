// Included by dm.rs: human acknowledgement is separate from deletion delivery.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DmDeletionNotice {
    pub deleted_at: i64,
    pub by_sender: bool,
    pub can_ack: bool,
}

impl Db {
    pub fn dm_deletion_notice(&self, message_id: i64) -> Result<Option<DmDeletionNotice>> {
        self.with_conn(|conn| {
            let row: Option<(bool, Option<i64>, Option<i64>, bool, bool)> = conn.query_row(
                "SELECT m.direction=1,n.local_at,n.remote_at,n.local_seen,n.remote_seen
                 FROM dm_deletion_notices n JOIN messages m ON m.id=n.message_id WHERE m.id=?1",
                params![message_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
            Ok(row.and_then(|(outgoing,local,remote,local_seen,remote_seen)| {
                if let Some(at) = remote.filter(|_| !remote_seen) {
                    Some(DmDeletionNotice { deleted_at:at, by_sender:!outgoing, can_ack:true })
                } else {
                    local.filter(|_| !local_seen).map(|at| DmDeletionNotice {
                        deleted_at:at, by_sender:outgoing, can_ack:false,
                    })
                }
            }))
        })
    }

    /// The original operation time is stable across retries and restarts.
    pub fn dm_local_delete_time(&self, contact_id:i64, reference:&DmReference) -> Result<i64> {
        self.with_conn(|conn| Ok(conn.query_row(
            "SELECT n.local_at FROM dm_deletion_notices n JOIN dm_tombstones t ON t.message_id=n.message_id
             WHERE t.contact_id=?1 AND t.author=?2 AND t.origin=?3 AND t.sent_at=?4",
            params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at], |r|r.get(0))?))
    }

    /// One explicit UI action; never call this from the automatic network ACK.
    /// Validate the whole batch before commit, including contact ownership.
    pub fn acknowledge_deletion_notices(&self, contact_id:i64, ids:&[i64]) -> Result<Vec<DmDelete>> {
        if ids.is_empty() || ids.len()>500 { return Err(DbError::State); }
        self.with_tx(|tx| {
            let mut result=Vec::with_capacity(ids.len());
            for id in ids {
                let seen:bool=tx.query_row("SELECT n.remote_seen FROM dm_deletion_notices n
                    JOIN messages m ON m.id=n.message_id WHERE m.id=?1 AND m.contact_id=?2
                    AND m.group_id IS NULL AND n.remote_at IS NOT NULL",
                    params![id,contact_id],|r|r.get(0))?;
                if !seen {
                    let reference=dm_reference_conn(tx,*id)?;
                    tx.execute("INSERT OR IGNORE INTO dm_notice_outbox(contact_id,author,origin,sent_at)
                        VALUES(?1,?2,?3,?4)",params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at])?;
                    tx.execute("UPDATE dm_deletion_notices SET remote_seen=1 WHERE message_id=?1",params![id])?;
                }
                result.push(DmDelete {message_id:*id,contact_id,changed:false,files:vec![]});
            }
            Ok(result)
        })
    }

    /// Only the authenticated other participant can dismiss our own notice.
    /// Duplicate OK controls still get a receipt after a lost kind-4 reply.
    pub fn receive_dm_notice_ack(&self, contact_id:i64, reference:&DmReference) -> Result<Option<i64>> {
        validate_reference(reference)?;
        self.with_tx(|tx| {
            let id:Option<i64>=tx.query_row("SELECT t.message_id FROM dm_tombstones t
                JOIN dm_deletion_notices n ON n.message_id=t.message_id
                WHERE t.contact_id=?1 AND t.author=?2 AND t.origin=?3 AND t.sent_at=?4 AND n.local_at IS NOT NULL",
                params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at],|r|r.get(0)).optional()?;
            if let Some(id)=id {
                tx.execute("UPDATE dm_deletion_notices SET local_seen=1 WHERE message_id=?1",params![id])?;
                // An explicit review also proves that the delete arrived.
                tx.execute("DELETE FROM dm_delete_outbox WHERE contact_id=?1 AND author=?2 AND origin=?3 AND sent_at=?4",
                    params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at])?;
            }
            Ok(id)
        })
    }

    pub fn pending_dm_notice_acks(&self, contact_id:i64, now:i64) -> Result<Vec<DmReference>> {
        self.with_conn(|conn| conn.prepare_cached("SELECT author,origin,sent_at FROM dm_notice_outbox
            WHERE contact_id=?1 AND (last_attempt_at IS NULL OR ?2-last_attempt_at >= MIN(300000,5000*(1 << MIN(attempts,16))))
            ORDER BY origin LIMIT 50")?.query_map(params![contact_id,now],|r|Ok(DmReference {
                author:blob32(r.get(0)?)?,origin:r.get::<_,i64>(1)? as u64,sent_at:r.get(2)?,
            }))?.collect_rows())
    }

    pub fn record_dm_notice_attempt(&self, contact_id:i64, reference:&DmReference, now:i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("UPDATE dm_notice_outbox SET attempts=MIN(attempts+1,30),last_attempt_at=?5
                WHERE contact_id=?1 AND author=?2 AND origin=?3 AND sent_at=?4",
                params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at,now])?;
            Ok(())
        })
    }

    pub fn confirm_dm_notice_ack(&self, contact_id:i64, reference:&DmReference) -> Result<()> {
        validate_reference(reference)?;
        self.with_conn(|conn| {
            conn.execute("DELETE FROM dm_notice_outbox WHERE contact_id=?1 AND author=?2 AND origin=?3 AND sent_at=?4",
                params![contact_id,reference.author.as_slice(),reference.origin as i64,reference.sent_at])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;

    fn open(path:&Path, own:[u8;32], peer:[u8;32]) -> (Db,i64) {
        let db=Db::open_plain(path).unwrap();
        db.configure_dm_identity(&own).unwrap();
        let contact=db.add_contact(&peer,&[7;32],"relay","peer",None).unwrap();
        (db,contact)
    }

    #[test]
    fn automatic_ack_never_hides_notice_and_human_ack_survives_restart() {
        let directory=tempfile::tempdir().unwrap();
        let path=directory.path().join("alice.db");
        let (alice,bob_id)=open(&path,[1;32],[2;32]);
        let (bob,alice_id)=open(&directory.path().join("bob.db"),[2;32],[1;32]);
        let id=alice.insert_message(bob_id,Direction::Out,"erase",100,None,&[]).unwrap();
        let copy=bob.insert_message_with_origin(alice_id,Direction::In,"erase",100,None,&[],Some(id)).unwrap();
        let reference=alice.dm_reference(id).unwrap();
        alice.delete_dm_message(id).unwrap();
        let at=alice.dm_local_delete_time(bob_id,&reference).unwrap();
        bob.receive_dm_delete_at(alice_id,&reference,at).unwrap();
        alice.acknowledge_dm_delete(bob_id,&reference).unwrap();
        assert_eq!(alice.dm_deletion_notice(id).unwrap(),Some(DmDeletionNotice {deleted_at:at,by_sender:true,can_ack:false}));
        assert!(alice.acknowledge_deletion_notices(bob_id,&[id]).is_err());
        assert_eq!(bob.dm_deletion_notice(copy).unwrap(),Some(DmDeletionNotice {deleted_at:at,by_sender:true,can_ack:true}));
        drop(bob);
        let bob=Db::open_plain(&directory.path().join("bob.db")).unwrap();
        bob.acknowledge_deletion_notices(alice_id,&[copy]).unwrap();
        assert!(bob.dm_deletion_notice(copy).unwrap().is_none());
        drop(bob);
        let bob=Db::open_plain(&directory.path().join("bob.db")).unwrap();
        assert_eq!(bob.pending_dm_notice_acks(alice_id,now_ms()).unwrap(),vec![reference.clone()]);
        let outsider=alice.add_contact(&[3;32],&[8;32],"other","other",None).unwrap();
        assert!(alice.receive_dm_notice_ack(outsider,&reference).unwrap().is_none());
        assert_eq!(alice.receive_dm_notice_ack(bob_id,&reference).unwrap(),Some(id));
        assert_eq!(alice.receive_dm_notice_ack(bob_id,&reference).unwrap(),Some(id));
        bob.confirm_dm_notice_ack(alice_id,&reference).unwrap();
        assert!(bob.pending_dm_notice_acks(alice_id,now_ms()).unwrap().is_empty());
        // A lost ACK causes a retry, never a new notice or a new timestamp.
        bob.receive_dm_delete_at(alice_id,&reference,at+10).unwrap();
        assert!(bob.dm_deletion_notice(copy).unwrap().is_none());
        assert!(bob.dm_is_deleted(alice_id,&reference).unwrap());
        drop(alice);
        let alice=Db::open_plain(&path).unwrap();
        assert!(alice.dm_deletion_notice(id).unwrap().is_none());
        assert!(alice.get_message(id).unwrap().unwrap().body.is_empty());
    }

    #[test]
    fn recipient_deletion_and_batch_validation_preserve_the_other_participants_choice() {
        let directory=tempfile::tempdir().unwrap();
        let (db,peer)=open(&directory.path().join("db"),[1;32],[2;32]);
        let id=db.insert_message(peer,Direction::Out,"sent",100,None,&[]).unwrap();
        let reference=db.dm_reference(id).unwrap();
        db.receive_dm_delete_at(peer,&reference,200).unwrap();
        assert_eq!(db.dm_deletion_notice(id).unwrap(),Some(DmDeletionNotice {deleted_at:200,by_sender:false,can_ack:true}));
        assert!(db.acknowledge_deletion_notices(peer,&[id,id+100]).is_err());
        assert!(db.dm_deletion_notice(id).unwrap().is_some());
        assert!(db.pending_dm_notice_acks(peer,now_ms()).unwrap().is_empty());
        db.acknowledge_deletion_notices(peer,&[id]).unwrap();
        assert!(db.dm_deletion_notice(id).unwrap().is_none());
    }
}
