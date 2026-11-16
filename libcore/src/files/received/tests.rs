use super::*;
use crate::files::{Flow, Sending};

const FILE: [u8; 16] = [1; 16];

fn verify_receipt(receiver: &Received, ack: &WireFileAck) {
    assert!(ack.received_up_to <= ack.seen_to);
    assert!(ack.seen_to <= receiver.total);
    assert!(ack.missing.len() <= MAX_MISSING);
    for index in 0..receiver.total {
        let claimed =
            index < ack.received_up_to || (index < ack.seen_to && !ack.missing.contains(&index));
        assert!(!claimed || receiver.has(index), "false ACK at {index}");
        if index < ack.seen_to {
            assert_eq!(claimed, receiver.has(index), "incomplete bounded range");
        }
    }
    assert!(ack
        .missing
        .iter()
        .all(|&i| i >= ack.received_up_to && i < ack.seen_to && !receiver.has(i)));
}

#[test]
fn a_sixty_fifth_hole_is_unknown_not_received() {
    let mut receiver = Received::new(200);
    receiver.mark(130);
    let ack = receiver.ack(FILE);
    assert_eq!(ack.received_up_to, 0);
    assert_eq!(ack.seen_to, 64);
    assert_eq!(ack.missing, (0..64).collect::<Vec<_>>());
    verify_receipt(&receiver, &ack);

    // Arrival of the first range lets the next receipt describe more of the
    // same saved bitmap, without losing the out-of-order part at 130.
    for index in 0..64 {
        receiver.mark(index);
    }
    assert_eq!(receiver.ack(FILE).seen_to, 128);
    for index in 64..128 {
        receiver.mark(index);
    }
    assert_eq!(receiver.ack(FILE).seen_to, 131);
    assert!(receiver.has(130));
    verify_receipt(&receiver, &receiver.ack(FILE));
}

#[test]
fn exactly_sixty_four_holes_can_ack_a_later_held_run() {
    let mut receiver = Received::new(70);
    for index in 64..70 {
        receiver.mark(index);
    }
    let ack = receiver.ack(FILE);
    assert_eq!(ack.seen_to, 70);
    assert_eq!(ack.missing.len(), 64);
    verify_receipt(&receiver, &ack);
}

#[test]
fn sparse_resume_does_not_discard_missing_sender_work() {
    let mut receiver = Received::new(200);
    receiver.mark(130);
    let restored = Received::from_bits(receiver.bits().to_vec(), 200);
    let ack = restored.resume_ack(FILE);
    let mut flow = Flow::default();
    flow.adopt(FILE, 0..200, 0);
    let mut sender = Sending {
        next: 200,
        acked: 0,
        resend: vec![],
    };
    sender.on_ack(&ack, &mut flow, 1000);
    for index in [0, 63, 64, 65, 129, 131, 199] {
        assert!(
            sender.resend.contains(&index),
            "missing part {index} was suppressed"
        );
    }
    let pending = sender.resend.clone();
    sender.on_ack(&ack, &mut flow, 2000);
    assert_eq!(
        sender.resend, pending,
        "duplicate resume changes pending work"
    );
    assert!(!sender.done(200));
    verify_receipt(&restored, &ack);
}

#[test]
fn receipt_semantics_hold_for_exhaustive_small_and_large_sparse_bitmaps() {
    for mask in 0u32..1024 {
        let receiver = Received::from_bits(mask.to_le_bytes().to_vec(), 10);
        verify_receipt(&receiver, &receiver.ack(FILE));
        verify_receipt(&receiver, &receiver.resume_ack(FILE));
    }
    // Deterministic diverse gaps exercise the bounded representation after
    // persistence, including well over 64 holes and non-byte-aligned totals.
    for stride in [2, 3, 7, 17, 65, 127] {
        for offset in 0..stride {
            let mut receiver = Received::new(513);
            for index in (offset..513).step_by(stride as usize) {
                receiver.mark(index);
            }
            let restored = Received::from_bits(receiver.bits().to_vec(), 513);
            assert_eq!(restored, receiver);
            verify_receipt(&restored, &restored.resume_ack(FILE));
        }
    }
    verify_receipt(&Received::new(0), &Received::new(0).ack(FILE));
    verify_receipt(
        &Received::completed(513),
        &Received::completed(513).ack(FILE),
    );
}
