use super::*;

#[cfg(test)]
mod atomic_group_outbox_tests {
    use super::*;

    fn prepared_attachments() -> Vec<NewGroupAttachment> {
        vec![
            NewGroupAttachment {
                attachment: NewAttachment {
                    name: "inline.txt".into(),
                    size: 3,
                    key: vec![1; 32],
                    path: "inline-cipher".into(),
                    chunk_size: None,
                },
                file_offer: None,
            },
            NewGroupAttachment {
                attachment: NewAttachment {
                    name: "parts.bin".into(),
                    size: 1234,
                    key: vec![2; 32],
                    path: "parts-cipher".into(),
                    chunk_size: Some(300),
                },
                file_offer: Some(([3; 16], [4; 32])),
            },
        ]
    }

    fn setup(path: &Path) -> (Db, Vec<i64>, i64) {
        let db = Db::open_plain(path).unwrap();
        db.create_group(b"atomic", "group").unwrap();
        let a = db.add_contact(&[1; 32], &[2; 32], "", "A", None).unwrap();
        let b = db.add_contact(&[3; 32], &[4; 32], "", "B", None).unwrap();
        let reply = db
            .insert_group_message(
                b"atomic",
                Some(&[5; 32]),
                Direction::In,
                "reply target",
                now_ms(),
                None,
                &[],
            )
            .unwrap();
        (db, vec![a, b], reply)
    }

    fn draft<'a>(recipients: &'a [i64], reply: i64, sent_at: i64) -> NewOutgoingGroup<'a> {
        NewOutgoingGroup {
            group_id: b"atomic",
            sender_sign_pk: &[9; 32],
            body: "complete outgoing message",
            sent_at,
            expires_at: Some(sent_at + 60_000),
            reply_to: Some(reply),
            buttons: Some(b"encoded buttons"),
            notify_sound: Some(b"ping"),
            recipients,
        }
    }

    fn counts(db: &Db) -> Vec<i64> {
        db.with_conn(|conn| {
            [
                "messages",
                "attachments",
                "settings",
                "file_out",
                "file_out_peer",
                "pending_outbound",
            ]
            .iter()
            .map(|table| {
                Ok(
                    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .collect()
        })
        .unwrap()
    }

    #[test]
    fn every_preparation_insert_rolls_back_the_entire_group_after_reopen() {
        // Each trigger aborts after earlier message/metadata/attachment rows
        // have been written. The last cases also abort a partial fan-out.
        for (table, condition) in [
            ("settings", "NEW.k LIKE 'sound_%'"),
            ("attachments", "NEW.name = 'parts.bin'"),
            ("file_out", "1"),
            ("file_out_peer", "NEW.contact_id = 2"),
            ("pending_outbound", "NEW.recipient_contact_id = 2"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("rollback.db");
            let (db, recipients, reply) = setup(&path);
            let before = counts(&db);
            db.with_conn(|conn| {
                conn.execute_batch(&format!(
                    "CREATE TRIGGER fail_group BEFORE INSERT ON {table} WHEN {condition}
                    BEGIN SELECT RAISE(ABORT, 'injected group publication failure'); END;"
                ))?;
                Ok(())
            })
            .unwrap();
            assert!(
                db.commit_outgoing_group(
                    &draft(&recipients, reply, now_ms()),
                    &prepared_attachments()
                )
                .is_err(),
                "{table}"
            );
            drop(db);
            let reopened = Db::open_plain(&path).unwrap();
            assert_eq!(
                counts(&reopened),
                before,
                "partial state escaped rollback at {table}"
            );
            assert_eq!(
                reopened
                    .list_group_messages(b"atomic", 10, None)
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[test]
    fn accepted_group_restores_reply_settings_offers_and_every_recipient() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accepted.db");
        let (db, recipients, reply) = setup(&path);
        let sent_at = now_ms();
        let id = db
            .commit_outgoing_group(&draft(&recipients, reply, sent_at), &prepared_attachments())
            .unwrap();
        drop(db);
        let db = Db::open_plain(&path).unwrap();
        let message = db.get_message(id).unwrap().unwrap();
        assert_eq!(message.body, "complete outgoing message");
        assert_eq!(message.reply_to, Some(reply));
        assert_eq!(message.sent_at, sent_at);
        assert_eq!(message.expires_at, Some(sent_at + 60_000));
        assert!(!message.sent && !message.delivered);
        assert_eq!(
            db.get_setting(&format!("buttons_{id}")).unwrap(),
            Some(b"encoded buttons".to_vec())
        );
        assert_eq!(
            db.get_setting(&format!("sound_{id}")).unwrap(),
            Some(b"ping".to_vec())
        );
        let attachments = db.list_attachments(id).unwrap();
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].path, "inline-cipher");
        assert_eq!(attachments[1].path, "parts-cipher");
        let offers = db.files_out_for_message(id).unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].attachment.id, attachments[1].id);
        assert_eq!(offers[0].file_id, [3; 16]);
        assert_eq!(offers[0].sha256, [4; 32]);
        assert_eq!(db.file_peers_of(&[3; 16]).unwrap().len(), 2);
        for recipient in recipients {
            assert!(db.file_peer(&[3; 16], recipient).unwrap().is_some());
            assert_eq!(
                db.pending_outbound_for_recipient(recipient, sent_at, 0, 0, 50)
                    .unwrap(),
                vec![id]
            );
        }
    }

    #[test]
    fn incomplete_file_offer_metadata_is_rejected_before_publication() {
        let dir = tempfile::tempdir().unwrap();
        let (db, recipients, reply) = setup(&dir.path().join("invalid.db"));
        let before = counts(&db);
        let mut attachments = prepared_attachments();
        attachments[1].file_offer = None;
        assert!(matches!(
            db.commit_outgoing_group(&draft(&recipients, reply, now_ms()), &attachments),
            Err(DbError::State)
        ));
        assert_eq!(counts(&db), before);
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    #[test]
    fn undelivered_mail_is_retried_until_it_is_a_week_old() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("t.db")).unwrap();
        let contact = db.add_contact(&[1; 32], &[2; 32], "", "bob", None).unwrap();
        let now = now_ms();
        let recent = db
            .insert_message(contact, Direction::Out, "recent", now, None, &[])
            .unwrap();
        let stale = db
            .insert_message(
                contact,
                Direction::Out,
                "stale",
                now - RETRY_TTL_MS - 1,
                None,
                &[],
            )
            .unwrap();
        for _ in 0..20 {
            db.mark_sent(recent).unwrap();
            db.mark_sent(stale).unwrap();
        }
        // Far enough ahead that the backoff is satisfied.
        let later = now + 3_600_000;
        let ids: Vec<i64> = db
            .list_unacked_outgoing(contact, later, 5_000, 300_000, 50)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, vec![recent]);

        db.pending_outbound_add(recent, contact).unwrap();
        for _ in 0..20 {
            db.pending_outbound_record_attempt(recent, contact).unwrap();
        }
        assert_eq!(
            db.pending_outbound_for_recipient(contact, later, 5_000, 300_000, 50)
                .unwrap(),
            vec![recent]
        );
    }
}

#[cfg(test)]
mod group_delivery_tests {
    use super::*;

    fn due(db: &Db, recipient: i64) -> Vec<i64> {
        db.pending_outbound_for_recipient(recipient, now_ms(), 0, 0, 100)
            .unwrap()
    }

    #[test]
    fn prepared_group_and_complete_recipient_snapshot_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("group.db");
        let db = Db::open_plain(&path).unwrap();
        let a = db.add_contact(&[1; 32], &[2; 32], "", "A", None).unwrap();
        let b = db.add_contact(&[3; 32], &[4; 32], "", "B", None).unwrap();
        let group = [5; 32];
        db.create_group(&group, "Team").unwrap();
        let id = db
            .insert_group_message(
                &group,
                Some(&[6; 32]),
                Direction::Out,
                "durable",
                now_ms(),
                None,
                &[],
            )
            .unwrap();
        assert!(due(&db, a).is_empty());
        assert!(due(&db, b).is_empty());
        db.set_setting(&format!("sound_{id}"), b"prepared").unwrap();
        db.queue_group_recipients(id, &[a, b, a]).unwrap();
        drop(db); // No enqueue or network call has happened yet.
        let db = Db::open_plain(&path).unwrap();
        assert_eq!(db.get_message(id).unwrap().unwrap().body, "durable");
        assert_eq!(due(&db, a), vec![id]);
        assert_eq!(due(&db, b), vec![id]);
        assert!(!db.get_message(id).unwrap().unwrap().delivered);
    }

    #[test]
    fn failed_recipient_registration_rolls_back_every_recipient() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("group.db")).unwrap();
        let a = db.add_contact(&[1; 32], &[2; 32], "", "A", None).unwrap();
        let b = db.add_contact(&[3; 32], &[4; 32], "", "B", None).unwrap();
        let group = [5; 32];
        db.create_group(&group, "Team").unwrap();
        db.with_conn(|c| { c.execute_batch(&format!(
            "CREATE TEMP TRIGGER fail_second_recipient BEFORE INSERT ON pending_outbound
             WHEN NEW.recipient_contact_id = {b} BEGIN SELECT RAISE(ABORT, 'injected disk failure'); END;"
        ))?; Ok(()) }).unwrap();
        let id = db
            .insert_group_message(
                &group,
                Some(&[6; 32]),
                Direction::Out,
                "rollback",
                now_ms(),
                None,
                &[],
            )
            .unwrap();
        assert!(db.queue_group_recipients(id, &[a, b]).is_err());
        assert_eq!(db.list_group_messages(&group, 100, None).unwrap().len(), 1);
        assert!(!db.get_message(id).unwrap().unwrap().delivered);
        assert!(due(&db, a).is_empty());
        assert!(due(&db, b).is_empty());
    }

    #[test]
    fn group_ack_is_recipient_scoped_idempotent_and_delivered_only_when_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("group.db");
        let db = Db::open_plain(&path).unwrap();
        let a = db.add_contact(&[1; 32], &[2; 32], "", "A", None).unwrap();
        let b = db.add_contact(&[3; 32], &[4; 32], "", "B", None).unwrap();
        let other = db
            .add_contact(&[7; 32], &[8; 32], "", "other", None)
            .unwrap();
        let group = [5; 32];
        db.create_group(&group, "Team").unwrap();
        let id = db
            .insert_group_message(
                &group,
                Some(&[6; 32]),
                Direction::Out,
                "receipt",
                now_ms(),
                None,
                &[],
            )
            .unwrap();
        db.queue_group_recipients(id, &[a, b]).unwrap();
        assert_eq!(
            db.acknowledge_group_delivery(id, other).unwrap(),
            GroupDeliveryAck::NotPending
        );
        assert_eq!(
            db.acknowledge_group_delivery(id, a).unwrap(),
            GroupDeliveryAck::PendingRecipients
        );
        assert_eq!(
            db.acknowledge_group_delivery(id, a).unwrap(),
            GroupDeliveryAck::NotPending
        );
        assert!(!db.get_message(id).unwrap().unwrap().delivered);
        drop(db);
        let db = Db::open_plain(&path).unwrap();
        assert!(due(&db, a).is_empty());
        assert_eq!(due(&db, b), vec![id]);
        assert_eq!(
            db.acknowledge_group_delivery(id, b).unwrap(),
            GroupDeliveryAck::Complete
        );
        assert!(db.get_message(id).unwrap().unwrap().delivered);
        assert!(db.get_message(id).unwrap().unwrap().sent);
        assert_eq!(
            db.acknowledge_group_delivery(id, b).unwrap(),
            GroupDeliveryAck::NotPending
        );
    }

    #[test]
    fn concurrent_recipient_acks_complete_once_and_survive_reopen() {
        use std::sync::{Arc, Barrier};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("concurrent-group.db");
        let db = Arc::new(Db::open_plain(&path).unwrap());
        let a = db.add_contact(&[1; 32], &[2; 32], "", "A", None).unwrap();
        let b = db.add_contact(&[3; 32], &[4; 32], "", "B", None).unwrap();
        let group = [5; 32];
        db.create_group(&group, "Team").unwrap();
        let id = db
            .insert_group_message(
                &group,
                Some(&[6; 32]),
                Direction::Out,
                "concurrent receipts",
                now_ms(),
                None,
                &[],
            )
            .unwrap();
        db.queue_group_recipients(id, &[a, b]).unwrap();

        // Both authenticated receive workers reach the ACK boundary together.
        let ready = Arc::new(Barrier::new(3));
        let workers: Vec<_> = [a, b]
            .into_iter()
            .map(|recipient| {
                let db = Arc::clone(&db);
                let ready = Arc::clone(&ready);
                std::thread::spawn(move || {
                    ready.wait();
                    db.acknowledge_group_delivery(id, recipient).unwrap()
                })
            })
            .collect();
        ready.wait();
        let outcomes: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(
            outcomes
                .iter()
                .filter(|&&outcome| outcome == GroupDeliveryAck::Complete)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|&&outcome| outcome == GroupDeliveryAck::PendingRecipients)
                .count(),
            1
        );
        for recipient in [a, b] {
            assert_eq!(
                db.acknowledge_group_delivery(id, recipient).unwrap(),
                GroupDeliveryAck::NotPending
            );
        }
        drop(db);

        let db = Db::open_plain(&path).unwrap();
        let message = db.get_message(id).unwrap().unwrap();
        assert!(message.sent);
        assert!(message.delivered);
        for recipient in [a, b] {
            assert!(due(&db, recipient).is_empty());
            assert_eq!(
                db.acknowledge_group_delivery(id, recipient).unwrap(),
                GroupDeliveryAck::NotPending
            );
        }
    }

    #[test]
    fn expiration_clears_due_group_and_dm_queues_without_erasing_history() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("group.db")).unwrap();
        let a = db.add_contact(&[1; 32], &[2; 32], "", "A", None).unwrap();
        let group = [5; 32];
        db.create_group(&group, "Team").unwrap();
        let now = now_ms();
        let id = db
            .insert_group_message(
                &group,
                Some(&[6; 32]),
                Direction::Out,
                "expired",
                now,
                None,
                &[],
            )
            .unwrap();
        db.queue_group_recipients(id, &[a]).unwrap();
        let fresh = db
            .insert_group_message(
                &group,
                Some(&[6; 32]),
                Direction::Out,
                "fresh",
                now + 1,
                None,
                &[],
            )
            .unwrap();
        db.queue_group_recipients(fresh, &[a]).unwrap();
        let dm = db
            .insert_message(a, Direction::Out, "DM", now, None, &[])
            .unwrap();
        db.pending_outbound_add(dm, a).unwrap();
        assert_eq!(
            db.expire_outgoing_delivery(now + RETRY_TTL_MS).unwrap().len(),
            2
        );
        assert_eq!(
            db.acknowledge_group_delivery(id, a).unwrap(),
            GroupDeliveryAck::NotPending
        );
        assert!(!db.get_message(id).unwrap().unwrap().delivered);
        assert_eq!(due(&db, a), vec![fresh]);
        assert_eq!(db.list_group_messages(&group, 10, None).unwrap().len(), 2);
    }
}

#[cfg(test)]
mod group_expiry_boundary_tests {
    use super::*;

    #[test]
    fn legacy_recipient_timestamps_cannot_turn_partial_expiry_into_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("legacy.db")).unwrap();
        let a = db.add_contact(&[1; 32], &[2; 32], "", "A", None).unwrap();
        let b = db.add_contact(&[3; 32], &[4; 32], "", "B", None).unwrap();
        let group = [5; 32];
        db.create_group(&group, "Team").unwrap();
        let now = now_ms();
        let id = db
            .insert_group_message(
                &group,
                Some(&[6; 32]),
                Direction::Out,
                "legacy",
                now,
                None,
                &[],
            )
            .unwrap();
        db.queue_group_recipients(id, &[a, b]).unwrap();
        db.with_conn(|c| { c.execute(
            "UPDATE pending_outbound SET created_at = ?1 WHERE msg_id = ?2 AND recipient_contact_id = ?3",
            params![now + 60_000, id, b],
        )?; Ok(()) }).unwrap();
        assert_eq!(
            db.expire_outgoing_delivery(now + RETRY_TTL_MS).unwrap().len(),
            1
        );
        assert_eq!(
            db.acknowledge_group_delivery(id, b).unwrap(),
            GroupDeliveryAck::NotPending
        );
        assert!(!db.get_message(id).unwrap().unwrap().delivered);
    }
}
