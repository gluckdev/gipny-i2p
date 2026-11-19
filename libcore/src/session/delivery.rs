//! Authenticated delivery acknowledgements and durable group attempts.

use super::*;

/// A successful send only enqueues or stores an envelope; the recipient's
/// authenticated ACK is what removes their durable group delivery record.
pub(super) async fn attempt_group_delivery(
    db: &Db,
    message_id: i64,
    recipient_contact_id: i64,
    send: impl std::future::Future<Output = Result<()>>,
) -> Result<()> {
    if db.pending_outbound_record_attempt(message_id, recipient_contact_id)? {
        send.await?;
    }
    Ok(())
}

/// `contact_id` is the peer authenticated by the incoming ratchet session.
/// Return a message only when this ACK completes its delivery.
pub(super) fn acknowledge_delivery(
    db: &Db,
    contact_id: i64,
    payload: &WirePayload,
) -> Result<Option<i64>> {
    let Some(ack_id) = payload.ack_for else {
        return Ok(None);
    };
    let Ok(local_id) = i64::try_from(ack_id) else {
        return Ok(None);
    };
    let Some(message) = db.get_message(local_id)? else {
        return Ok(None);
    };
    if message.deleted_at.is_some() || !matches!(message.direction, Direction::Out) {
        return Ok(None);
    }
    match (message.contact_id, &message.group_id, &payload.group) {
        (Some(cid), None, None) if cid == contact_id => {
            Ok(db.acknowledge_dm_delivery(local_id, contact_id)?.then_some(local_id))
        }
        (None, Some(gid), Some(group)) if gid == &group.id => {
            match db.acknowledge_group_delivery(local_id, contact_id)? {
                GroupDeliveryAck::Complete => Ok(Some(local_id)),
                GroupDeliveryAck::NotPending | GroupDeliveryAck::PendingRecipients => Ok(None),
            }
        }
        _ => Ok(None),
    }
}
