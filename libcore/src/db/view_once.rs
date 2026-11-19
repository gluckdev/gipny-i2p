//! The sender ledger, replay claim and receipt queue contain routing metadata only.
use super::*;

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ViewOnceDelivery {
    pub id: String,
    pub contact_id: i64,
    pub deadline: i64,
    pub status: String,
}
fn row(r: &Row<'_>) -> rusqlite::Result<ViewOnceDelivery> {
    Ok(ViewOnceDelivery {
        id: hex(&r.get::<_, Vec<u8>>(0)?),
        contact_id: r.get(1)?,
        deadline: r.get(2)?,
        status: r.get(3)?,
    })
}
impl Db {
    pub fn begin_view_once_delivery(
        &self,
        id: &[u8; 16],
        contact: i64,
        deadline: i64,
    ) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO view_once_delivery(id,contact_id,deadline,status)
                       VALUES(?1,?2,?3,'pending')",
                params![id, contact, deadline],
            )?;
            Ok(())
        })
    }
    pub fn view_once_relayed(&self, id: &[u8; 16]) -> Result<Option<ViewOnceDelivery>> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "UPDATE view_once_delivery SET status='relayed'
                            WHERE id=?1 AND status='pending'
                            RETURNING id,contact_id,deadline,status",
                [id],
                row,
            )
            .optional()?)
        })
    }
    pub fn view_once_received(
        &self,
        contact: i64,
        id: &[u8; 16],
    ) -> Result<Option<ViewOnceDelivery>> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "UPDATE view_once_delivery SET status='received'
                            WHERE id=?1 AND contact_id=?2
                            RETURNING id,contact_id,deadline,status",
                params![id, contact],
                row,
            )
            .optional()?)
        })
    }
    pub fn list_view_once_deliveries(&self) -> Result<Vec<ViewOnceDelivery>> {
        self.with_conn(|c| {
            c.prepare(
                "SELECT id,contact_id,deadline,status FROM view_once_delivery ORDER BY deadline,id",
            )?
            .query_map([], row)?
            .collect_rows()
        })
    }
    pub fn dismiss_view_once_deliveries(&self, ids: &[[u8; 16]]) -> Result<()> {
        self.with_tx(|tx| {
            let mut q=tx.prepare("DELETE FROM view_once_delivery WHERE id=?1 AND status IN ('unconfirmed','received')")?;
            for id in ids {q.execute([id])?;} Ok(())
        })
    }
    pub fn expire_view_once_deliveries(&self, now: i64) -> Result<Vec<ViewOnceDelivery>> {
        self.with_tx(|tx| {
            tx.execute("DELETE FROM view_once_delivery WHERE deadline<=?1 AND status='received'",[now])?;
            tx.execute("DELETE FROM view_once_receipt WHERE deadline<=?1",[now])?;
            tx.execute("DELETE FROM view_once_seen WHERE expires_at<=?1",[now])?;
            let notices=tx.prepare("UPDATE view_once_delivery SET status='unconfirmed' WHERE deadline<=?1 AND status IN ('pending','relayed') RETURNING id,contact_id,deadline,status")?.query_map([now],row)?.collect_rows()?;
            Ok(notices)
        })
    }
    pub fn claim_received_view_once(
        &self,
        id: &[u8; 16],
        contact: i64,
        expires_at: i64,
        now: i64,
    ) -> Result<bool> {
        self.with_tx(|tx| {
            if expires_at <= now
                || tx.execute(
                    "INSERT OR IGNORE INTO view_once_seen(id,expires_at,contact_id) VALUES(?1,?2,?3)",
                    params![id, expires_at,contact],
                )? != 1
            {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO view_once_receipt(id,contact_id,deadline,retry_at) VALUES(?1,?2,?3,0)",
                params![
                    id,
                    contact,
                    expires_at.min(now.saturating_add(RETRY_TTL_MS))
                ],
            )?;
            Ok(true)
        })
    }
    /// A lost final transfer ACK can lead to another fresh offer. Only the
    /// originally authenticated sender with the same expiry can refresh its receipt.
    pub fn reack_view_once(
        &self,
        id: &[u8; 16],
        contact: i64,
        expiry: i64,
        now: i64,
    ) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute("INSERT INTO view_once_receipt(id,contact_id,deadline,retry_at)
            SELECT id,contact_id,MIN(expires_at,?5),0 FROM view_once_seen WHERE id=?1 AND contact_id=?2 AND expires_at=?3 AND expires_at>?4
            ON CONFLICT(id) DO UPDATE SET retry_at=0",params![id,contact,expiry,now,now.saturating_add(RETRY_TTL_MS)])? == 1))
    }
    pub fn due_view_once_receipts(&self, now: i64) -> Result<Vec<(i64, [u8; 16])>> {
        self.with_tx(|tx| {
            let mut q=tx.prepare("UPDATE view_once_receipt SET retry_at=?2 WHERE id IN (SELECT id FROM view_once_receipt WHERE deadline>?1 AND retry_at<=?1 ORDER BY retry_at,id LIMIT 16) RETURNING contact_id,id")?;
            let rows=q.query_map(params![now,now.saturating_add(300_000)],|r| Ok((r.get(0)?,r.get(1)?)))?.collect_rows()?;
            Ok(rows)
        })
    }
    pub fn confirm_view_once_receipt(&self, contact: i64, id: &[u8; 16]) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM view_once_receipt WHERE id=?1 AND contact_id=?2",
                params![id, contact],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn view_once_receipt_claim_and_outgoing_status_survive_restart_without_body() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile");
        let db = std::sync::Arc::new(Db::open_plain(&path).unwrap());
        let bob = db.add_contact(&[1; 32], &[2; 32], "", "B", None).unwrap();
        let eve = db.add_contact(&[3; 32], &[4; 32], "", "C", None).unwrap();
        let id = [42; 16];
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let db = db.clone();
                std::thread::spawn(move || db.claim_received_view_once(&id, bob, 100, 1).unwrap())
            })
            .collect();
        assert_eq!(
            tasks
                .into_iter()
                .map(|task| task.join().unwrap() as usize)
                .sum::<usize>(),
            1
        );
        assert_eq!(db.due_view_once_receipts(1).unwrap(), vec![(bob, id)]);
        db.confirm_view_once_receipt(eve, &id).unwrap();
        db.confirm_view_once_receipt(bob, &id).unwrap();
        assert!(!db.reack_view_once(&id, eve, 100, 2).unwrap());
        assert!(!db.reack_view_once(&id, bob, 101, 2).unwrap());
        assert!(db.reack_view_once(&id, bob, 100, 2).unwrap());
        assert!(!db.reack_view_once(&id, bob, 100, 100).unwrap());
        assert_eq!(db.due_view_once_receipts(2).unwrap(), vec![(bob, id)]);
        db.begin_view_once_delivery(&id, bob, 99).unwrap();
        assert!(db.view_once_received(eve, &id).unwrap().is_none());
        assert_eq!(
            db.view_once_relayed(&id).unwrap().unwrap().status,
            "relayed"
        );
        drop(db);
        let db = Db::open_plain(&path).unwrap();
        assert!(!db.claim_received_view_once(&id, bob, 100, 2).unwrap());
        assert!(db.expire_view_once_deliveries(98).unwrap().is_empty());
        assert_eq!(
            db.expire_view_once_deliveries(99).unwrap()[0].status,
            "unconfirmed"
        );
        assert!(db.expire_view_once_deliveries(99).unwrap().is_empty());
        assert_eq!(
            db.view_once_received(bob, &id).unwrap().unwrap().status,
            "received"
        );
        assert!(db.view_once_relayed(&id).unwrap().is_none());
        db.dismiss_view_once_deliveries(&[id]).unwrap();
        assert!(db.list_view_once_deliveries().unwrap().is_empty());
    }
}
