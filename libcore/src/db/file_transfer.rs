use super::*;
use crate::files::{Flow, Sending};
use crate::session::WireFileAck;

#[derive(Default)]
pub struct FileBatch {
    pub due: Vec<u32>,
    pub next_before: u32,
    pub gave_up: bool,
}

impl Db {
    pub fn failed_files_for_message(&self, message_id:i64) -> Result<Vec<(String,u64,String)>> {
        self.with_conn(|conn| conn.prepare_cached("SELECT name,size,status FROM file_failures WHERE message_id=?1")?
            .query_map(params![message_id],|row|Ok((row.get(0)?,row.get::<_,i64>(1)? as u64,row.get(2)?)))?.collect_rows())
    }

    pub fn cancel_incoming_file(&self, file_id:&[u8;16], contact_id:i64) -> Result<Option<i64>> {
        let _files=self.file_mutations.lock().map_err(|_|DbError::State)?;
        self.with_tx(|tx| {
            let message:Option<i64>=tx.query_row("SELECT message_id FROM file_in WHERE file_id=?1 AND contact_id=?2",
                params![file_id.as_slice(),contact_id],|r|r.get(0)).optional()?;
            if message.is_some() {
                tx.execute("INSERT OR REPLACE INTO file_failures(file_id,contact_id,message_id,name,size,status)
                    SELECT file_id,contact_id,message_id,name,size,'cancelled' FROM file_in WHERE file_id=?1 AND contact_id=?2",
                    params![file_id.as_slice(),contact_id])?;
                tx.execute("INSERT OR IGNORE INTO deleted_file_cleanup(path) SELECT path FROM file_in WHERE file_id=?1 AND contact_id=?2",
                    params![file_id.as_slice(),contact_id])?;
                tx.execute("INSERT OR IGNORE INTO dm_deleted_files(contact_id,file_id) VALUES(?1,?2)",params![contact_id,file_id.as_slice()])?;
                tx.execute("DELETE FROM file_in WHERE file_id=?1 AND contact_id=?2",params![file_id.as_slice(),contact_id])?;
            }
            Ok(message)
        })
    }

    /// Always edit the latest row while holding the DB transaction. Callers
    /// hold Flow before DB, and release both before any network await.
    fn mutate_file_peer<T>(&self, file_id:&[u8;16], contact_id:i64,
        update:impl FnOnce(&mut FilePeer)->T) -> Result<Option<T>> {
        self.with_tx(|tx| {
            let Some(mut peer)=tx.query_row("SELECT file_id,contact_id,next,acked,resend,last_sent_at,last_ack_at,failed
                FROM file_out_peer WHERE file_id=?1 AND contact_id=?2",
                params![file_id.as_slice(),contact_id],Self::map_file_peer).optional()? else {return Ok(None)};
            let result=update(&mut peer);
            tx.execute("UPDATE file_out_peer SET next=?3,acked=?4,resend=?5,last_sent_at=?6,last_ack_at=?7,failed=?8
                WHERE file_id=?1 AND contact_id=?2",params![file_id.as_slice(),contact_id,peer.next,peer.acked,
                u32s_blob(&peer.resend),peer.last_sent_at,peer.last_ack_at,peer.failed as i64])?;
            Ok(Some(result))
        })
    }

    /// Reserve next before sending: a fast ACK must see the reserved indices.
    pub fn reserve_file_batch(&self, file_id:&[u8;16], contact_id:i64, total:u32,
        flow:&mut Flow, now:i64, heard:Option<i64>) -> Result<Option<FileBatch>> {
        self.mutate_file_peer(file_id,contact_id,|peer| {
            if peer.failed || peer.acked>=total {return FileBatch::default();}
            let mut sending=Sending {next:peer.next,acked:peer.acked,resend:peer.resend.clone()};
            if let Some(sent)=peer.last_sent_at {
                let alive=heard.is_some_and(|at|at>sent);
                if peer.last_ack_at.is_none() && alive && now-sent>24*3600*1000 {
                    peer.failed=true; flow.forget(*file_id);
                    return FileBatch {gave_up:true,..FileBatch::default()};
                }
                if sending.resend.is_empty() && sending.next>sending.acked && alive
                    && peer.last_ack_at.is_none_or(|at|at<sent) && now-sent>30*60*1000 {
                    sending.rewind(*file_id,flow);
                }
            }
            let next_before=sending.next;
            let due=sending.due(*file_id,total,flow,now);
            peer.next=sending.next; peer.resend=sending.resend;
            if !due.is_empty() {peer.last_sent_at=Some(now);}
            FileBatch {due,next_before,gave_up:false}
        })
    }

    pub fn acknowledge_file_parts(&self, contact_id:i64, ack:&WireFileAck, total:u32,
        flow:&mut Flow, now:i64) -> Result<Option<u32>> {
        if ack.received_up_to>total || ack.seen_to>total || ack.received_up_to>ack.seen_to
            || ack.missing.len()>64 || ack.missing.iter().any(|index|*index<ack.received_up_to || *index>=ack.seen_to) {
            return Err(DbError::State);
        }
        self.mutate_file_peer(&ack.file_id,contact_id,|peer| {
            let mut sending=Sending {next:peer.next,acked:peer.acked,resend:peer.resend.clone()};
            sending.on_ack(ack,flow,now);
            peer.acked=sending.acked; peer.resend=sending.resend; peer.last_ack_at=Some(now);
            if peer.acked>=total {flow.forget(ack.file_id);}
            peer.acked
        })
    }

    /// A failed write may race an ACK; it must never restore an old watermark.
    pub fn retry_file_parts(&self, file_id:&[u8;16], contact_id:i64, indices:&[u32], flow:&mut Flow) -> Result<()> {
        for index in indices {flow.unsent(*file_id,*index);}
        self.mutate_file_peer(file_id,contact_id,|peer| {
            for index in indices {
                if *index>=peer.acked && *index<peer.next {peer.resend.push(*index);}
            }
            peer.resend.sort_unstable(); peer.resend.dedup();
        })?;
        Ok(())
    }

    pub fn fail_file_peer(&self, file_id:&[u8;16], contact_id:i64) -> Result<bool> {
        Ok(self.mutate_file_peer(file_id,contact_id,|peer|peer.failed=true)?.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest,Sha256};

    #[test]
    fn completed_file_repeats_final_receipt_after_reopen_without_duplicate_attachment() {
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("db");
        let db=Db::open_plain(&path).unwrap();
        db.configure_dm_identity(&[1;32]).unwrap();
        let peer=db.add_contact(&[2;32],&[3;32],"relay","peer",None).unwrap();
        let message=db.insert_message_with_origin(peer,Direction::In,"",100,None,&[],Some(7)).unwrap();
        let reference=db.dm_reference(message).unwrap();
        let data=b"file";
        let fin=FileIn {file_id:[9;16],contact_id:peer,message_id:message,name:"file.bin".into(),size:4,
            sha256:Sha256::digest(data).into(),chunk_size:4,path:"sealed".into(),key:vec![5;32],bits:vec![0],received:0,created_at:100};
        db.file_in_add(&fin).unwrap();
        let chunk=crate::session::WireFileChunk {file_id:fin.file_id,index:0,data:data.to_vec()};
        let first=db.apply_file_chunk(peer,&chunk,Some(&reference),dir.path()).unwrap().unwrap();
        assert!(first.received.complete()); assert!(first.attachment_id.is_some());
        assert!(db.file_in(&fin.file_id,peer).unwrap().is_none());
        drop(db);
        let db=Db::open_plain(&path).unwrap();
        let repeated=db.apply_file_chunk(peer,&chunk,Some(&reference),dir.path()).unwrap().unwrap();
        assert!(repeated.received.complete()); assert!(!repeated.fresh);
        assert_eq!(db.list_attachments(message).unwrap().len(),1);
        assert!(!db.file_in_add(&fin).unwrap(),"duplicate offer cannot restart a finished file");
        db.delete_dm_message(message).unwrap();
        assert!(db.apply_file_chunk(peer,&chunk,Some(&reference),dir.path()).unwrap().is_none());
    }

    #[test]
    fn fast_ack_and_late_send_failure_never_regress_reserved_progress() {
        let dir=tempfile::tempdir().unwrap();
        let db=Db::open_plain(&dir.path().join("db")).unwrap();
        let peer=db.add_contact(&[1;32],&[2;32],"relay","peer",None).unwrap();
        let attachment=NewAttachment {name:"file".into(),size:1200,key:vec![3;32],path:"sealed".into(),chunk_size:Some(300)};
        let id=db.insert_message(peer,Direction::Out,"",100,None,&[attachment]).unwrap();
        let file=[9;16];
        db.file_out_add(&file,db.list_attachments(id).unwrap()[0].id,id,&[4;32],&[peer]).unwrap();
        let mut flow=Flow::default();
        let batch=db.reserve_file_batch(&file,peer,4,&mut flow,1000,None).unwrap().unwrap();
        assert_eq!(batch.due,vec![0,1,2,3]);
        assert_eq!(db.file_peer(&file,peer).unwrap().unwrap().next,4);
        // An ACK arrives while the async send function is suspended.
        let ack=WireFileAck {file_id:file,received_up_to:4,seen_to:4,missing:vec![],resumed:false,echo_index:3,echo_sent_at:1000};
        assert_eq!(db.acknowledge_file_parts(peer,&ack,4,&mut flow,1100).unwrap(),Some(4));
        // The suspended task then reports a write failure for its old batch.
        db.retry_file_parts(&file,peer,&batch.due,&mut flow).unwrap();
        let current=db.file_peer(&file,peer).unwrap().unwrap();
        assert_eq!(current.acked,4); assert_eq!(current.last_ack_at,Some(1100)); assert!(current.resend.is_empty());
        assert!(db.file_peers_for(peer).unwrap().is_empty());
    }
}
