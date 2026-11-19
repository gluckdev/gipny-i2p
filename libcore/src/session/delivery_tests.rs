use super::*;

fn contact(db: &Db, key: u8) -> i64 {
    db.add_contact(&[key; 32], &[key + 10; 32], "", "recipient", None)
        .unwrap()
}

fn pending(db: &Db, recipient: i64) -> Vec<i64> {
    db.pending_outbound_for_recipient(recipient, now_ms(), 0, 0, 50)
        .unwrap()
}

fn ack(message_id: i64, group_id: &[u8]) -> WirePayload {
    let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
    payload.ack_for = Some(message_id as u64);
    payload.group = Some(WireGroupRef {
        id: group_id.to_vec(),
        name: "group".into(),
        members: Vec::new(),
    });
    payload
}

async fn enqueue(db: &Db, message_id: i64, recipient: i64, tx: &mpsc::Sender<ClientToRelay>) -> Result<()> {
    attempt_group_delivery(db, message_id, recipient, async {
        tx.send(ClientToRelay::Send {
            to: [1; 32],
            blob: vec![1, 2, 3],
        })
        .await
        .map_err(|_| SessionError::State)
    })
    .await
}

#[tokio::test]
async fn group_enqueue_survives_lost_frame_restart_and_partial_ack() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sender.db");
    let db = Db::open_plain(&path).unwrap();
    let bob = contact(&db, 1);
    let carol = contact(&db, 2);
    let outsider = contact(&db, 3);
    let group_id = b"durable-group";
    db.create_group(group_id, "group").unwrap();
    let message_id = db
        .insert_group_message(
            group_id,
            Some(&[9; 32]),
            Direction::Out,
            "survive a disconnect",
            now_ms(),
            None,
            &[],
        )
        .unwrap();
    db.queue_group_recipients(message_id, &[bob, carol])
        .unwrap();

    let (tx, rx) = mpsc::channel(4);
    let relay = tx;
    enqueue(&db, message_id, bob, &relay).await.unwrap();
    enqueue(&db, message_id, carol, &relay).await.unwrap();
    assert_eq!(
        rx.len(),
        2,
        "only queued locally; no socket delivered either frame"
    );
    assert_eq!(pending(&db, bob), vec![message_id]);
    assert_eq!(pending(&db, carol), vec![message_id]);
    assert!(!db.get_message(message_id).unwrap().unwrap().delivered);
    assert!(
        db.pending_outbound_for_recipient(
            bob,
            now_ms(),
            RETRY_BASE_BACKOFF_MS,
            RETRY_MAX_BACKOFF_MS,
            50,
        )
        .unwrap()
        .is_empty(),
        "the initial attempt must honor retry backoff"
    );

    // Lose both unsent frames and all local state, then reopen the same DB.
    drop(rx);
    drop(relay);
    drop(db);
    let db = Db::open_plain(&path).unwrap();
    assert_eq!(pending(&db, bob), vec![message_id]);
    assert_eq!(pending(&db, carol), vec![message_id]);

    let (tx, rx) = mpsc::channel(4);
    let relay = tx;
    enqueue(&db, message_id, bob, &relay).await.unwrap();
    assert_eq!(
        pending(&db, bob),
        vec![message_id],
        "a successful retry still needs an ACK"
    );
    drop(rx);
    assert!(enqueue(&db, message_id, bob, &relay).await.is_err());
    assert_eq!(
        pending(&db, bob),
        vec![message_id],
        "a failed retry must remain durable"
    );

    let receipt = ack(message_id, group_id);
    assert_eq!(acknowledge_delivery(&db, outsider, &receipt).unwrap(), None);
    assert_eq!(
        acknowledge_delivery(&db, bob, &ack(message_id, b"other-group")).unwrap(),
        None
    );
    let mut direct_receipt = receipt.clone();
    direct_receipt.group = None;
    assert_eq!(
        acknowledge_delivery(&db, bob, &direct_receipt).unwrap(),
        None
    );
    assert_eq!(pending(&db, bob), vec![message_id]);
    assert_eq!(pending(&db, carol), vec![message_id]);
    assert!(!db.get_message(message_id).unwrap().unwrap().delivered);

    // contact_id is supplied by the authenticated session, not the ACK body.
    assert_eq!(acknowledge_delivery(&db, bob, &receipt).unwrap(), None);
    assert!(pending(&db, bob).is_empty());
    assert_eq!(pending(&db, carol), vec![message_id]);
    assert!(!db.get_message(message_id).unwrap().unwrap().delivered);
    assert_eq!(
        acknowledge_delivery(&db, bob, &receipt).unwrap(),
        None,
        "a duplicate cannot ACK Carol"
    );
    assert_eq!(pending(&db, carol), vec![message_id]);

    assert_eq!(
        acknowledge_delivery(&db, carol, &receipt).unwrap(),
        Some(message_id)
    );
    assert!(pending(&db, carol).is_empty());
    assert!(db.get_message(message_id).unwrap().unwrap().delivered);
    assert_eq!(acknowledge_delivery(&db, carol, &receipt).unwrap(), None);
}

#[tokio::test]
async fn unacknowledged_group_retries_are_bounded_by_retention() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open_plain(&dir.path().join("sender.db")).unwrap();
    let recipient = contact(&db, 1);
    let group_id = b"expired-group";
    db.create_group(group_id, "group").unwrap();
    let message_id = db
        .insert_group_message(
            group_id,
            Some(&[9; 32]),
            Direction::Out,
            "not acknowledged",
            now_ms(),
            None,
            &[],
        )
        .unwrap();
    db.queue_group_recipients(message_id, &[recipient]).unwrap();
    let (tx, _rx) = mpsc::channel(1);
    enqueue(&db, message_id, recipient, &tx)
        .await
        .unwrap();

    let after_backoff = now_ms() + RETRY_MAX_BACKOFF_MS + 1;
    assert_eq!(
        db.pending_outbound_for_recipient(
            recipient,
            after_backoff,
            RETRY_BASE_BACKOFF_MS,
            RETRY_MAX_BACKOFF_MS,
            50,
        )
        .unwrap(),
        vec![message_id]
    );
    let expired = now_ms() + crate::db::RETRY_TTL_MS + 1_000;
    assert!(db
        .pending_outbound_for_recipient(
            recipient,
            expired,
            RETRY_BASE_BACKOFF_MS,
            RETRY_MAX_BACKOFF_MS,
            50,
        )
        .unwrap()
        .is_empty());
    assert_eq!(db.expire_outgoing_delivery(expired).unwrap().len(), 1);
    assert!(pending(&db, recipient).is_empty());
    assert!(
        !db.get_message(message_id).unwrap().unwrap().delivered,
        "expiry is not delivery"
    );
    assert_eq!(
        acknowledge_delivery(&db, recipient, &ack(message_id, group_id)).unwrap(),
        None
    );
    assert!(!db.get_message(message_id).unwrap().unwrap().delivered);
}

#[test]
fn direct_ack_remains_bound_to_contact_and_direction() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open_plain(&dir.path().join("sender.db")).unwrap();
    let recipient = contact(&db, 1);
    let outsider = contact(&db, 2);
    let outgoing = db
        .insert_message(recipient, Direction::Out, "direct", now_ms(), None, &[])
        .unwrap();
    let incoming = db
        .insert_message(recipient, Direction::In, "received", now_ms(), None, &[])
        .unwrap();
    let mut receipt = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
    receipt.ack_for = Some(outgoing as u64);
    assert_eq!(acknowledge_delivery(&db, outsider, &receipt).unwrap(), None);
    assert!(!db.get_message(outgoing).unwrap().unwrap().delivered);
    receipt.ack_for = Some(incoming as u64);
    assert_eq!(
        acknowledge_delivery(&db, recipient, &receipt).unwrap(),
        None
    );
    assert!(!db.get_message(incoming).unwrap().unwrap().delivered);
    receipt.ack_for = Some(u64::MAX);
    assert_eq!(
        acknowledge_delivery(&db, recipient, &receipt).unwrap(),
        None
    );
    receipt.ack_for = Some(outgoing as u64);
    assert_eq!(
        acknowledge_delivery(&db, recipient, &receipt).unwrap(),
        Some(outgoing)
    );
    assert!(db.get_message(outgoing).unwrap().unwrap().delivered);
}
