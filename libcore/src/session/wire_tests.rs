use super::*;

fn sample() -> WirePayload {
    let mut p = WirePayload::simple(7, "uptime".into(), vec![], 1_700_000_000_000, None);
    p.sender_name = Some("laptop".into());
    p
}

#[test]
fn console_roundtrips() {
    let mut p = sample();
    p.console = Some(WireConsole {
        kind: CONSOLE_OUTPUT,
        exit_code: Some(0),
        duration_ms: Some(42),
        truncated: true,
    });
    let bytes = encode_payload(&p).unwrap();
    let back = decode_payload(&bytes).unwrap();
    assert_eq!(back.console, p.console);
    assert_eq!(back.body, "uptime");
    assert_eq!(back.sender_name.as_deref(), Some("laptop"));
}

#[test]
fn older_shapes_decode_with_no_console() {
    // sender_name alone is a WireV4; a newer client must still read it.
    let p = sample();
    let bytes = encode_payload(&p).unwrap();
    assert!(
        bincode::deserialize::<WirePayload>(&bytes).is_err(),
        "must fall through, not misread"
    );
    let back = decode_payload(&bytes).unwrap();
    assert_eq!(back.console, None);
    assert_eq!(back.sender_name.as_deref(), Some("laptop"));
}

#[test]
fn notify_sound_alone_is_the_v7_shape() {
    let mut p = sample();
    p.notify_sound = Some("ping".into());
    let bytes = encode_payload(&p).unwrap();
    assert_eq!(bytes, bincode::serialize(&WireV7::from(&p)).unwrap());
    let back = decode_payload(&bytes).unwrap();
    assert_eq!(back.notify_sound.as_deref(), Some("ping"));
    assert_eq!(back.console, None);
}

#[test]
fn a_build_without_the_console_field_still_reads_the_body() {
    // What a pre-console client does: deserialize its own newest shape from
    // bytes that carry one more trailing field. bincode's `deserialize`
    // tolerates trailing bytes, so the marker body comes through as text.
    let mut p = sample();
    p.body = crate::agent::BODY_GRANT.into();
    p.console = Some(WireConsole::new(CONSOLE_GRANT));
    let bytes = encode_payload(&p).unwrap();
    let old: WireV7 = bincode::deserialize(&bytes).expect("trailing field tolerated");
    assert_eq!(old.body, "[agent on]");
    assert_eq!(old.sender_name.as_deref(), Some("laptop"));
}

#[test]
fn wipe_roundtrips_and_leaves_other_payloads_as_they_were() {
    let mut p = sample();
    p.relay_address = Some("relay".into());
    // Without wipe: exactly the 0.4.13 bytes.
    assert_eq!(
        encode_payload(&p).unwrap(),
        bincode::serialize(&WireV8::from(&p)).unwrap()
    );
    p.wipe = Some(true);
    let bytes = encode_payload(&p).unwrap();
    assert_eq!(decode_payload(&bytes).unwrap().wipe, Some(true));
    // A 0.4.13 client reads its own newest shape and ignores the rest.
    let old: WireV8 = bincode::deserialize(&bytes).expect("trailing field tolerated");
    assert_eq!(old.relay_address.as_deref(), Some("relay"));
}

#[test]
fn files_roundtrip_and_leave_other_payloads_as_they_were() {
    let mut p = sample();
    p.wipe = Some(true);
    // No file fields: exactly the bytes of the build before them.
    assert_eq!(
        encode_payload(&p).unwrap(),
        bincode::serialize(&WireV9::from(&p)).unwrap()
    );
    p.wipe = None;
    p.files = vec![WireFileOffer {
        file_id: [7; 16],
        name: "big.bin".into(),
        size: 5 << 20,
        sha256: [9; 32],
        chunk_size: 196_608,
    }];
    let back = decode_payload(&encode_payload(&p).unwrap()).unwrap();
    assert_eq!(back.files, p.files);
    assert_eq!(back.body, p.body);

    let mut c = WirePayload::simple(0, String::new(), Vec::new(), 1, None);
    c.file_chunk = Some(WireFileChunk {
        file_id: [7; 16],
        index: 3,
        data: vec![1, 2, 3],
    });
    let back = decode_payload(&encode_payload(&c).unwrap()).unwrap();
    assert_eq!(back.file_chunk, c.file_chunk);

    let mut a = WirePayload::simple(0, String::new(), Vec::new(), 1, None);
    a.file_ack = Some(WireFileAck {
        file_id: [7; 16],
        received_up_to: 4,
        missing: vec![6],
        seen_to: 7,
        resumed: true,
        echo_index: 5,
        echo_sent_at: 1_234,
    });
    a.file_cancel = Some([8; 16]);
    let back = decode_payload(&encode_payload(&a).unwrap()).unwrap();
    assert_eq!(back.file_ack, a.file_ack);
    assert_eq!(back.file_cancel, a.file_cancel);
}

#[test]
fn a_released_client_sees_a_part_as_an_empty_letter() {
    // 0.4.11–0.4.13 read their newest shape (V8) and ignore the tail: a
    // part is then a letter with no text, which persist_incoming drops.
    let mut c = WirePayload::simple(0, String::new(), Vec::new(), 1, None);
    c.file_chunk = Some(WireFileChunk {
        file_id: [7; 16],
        index: 0,
        data: vec![0; 1000],
    });
    let old: WireV8 =
        bincode::deserialize(&encode_payload(&c).unwrap()).expect("trailing fields tolerated");
    assert!(old.body.is_empty() && old.attachments.is_empty() && old.ack_for.is_none());
    // And an offer shows its text.
    let mut p = sample();
    p.files = vec![WireFileOffer {
        file_id: [1; 16],
        name: "a".into(),
        size: 1,
        sha256: [0; 32],
        chunk_size: 1,
    }];
    let old: WireV8 =
        bincode::deserialize(&encode_payload(&p).unwrap()).expect("trailing fields tolerated");
    assert_eq!(old.body, p.body);
}

#[test]
fn crossing_inits_pick_the_same_session_on_both_ends() {
    let (a, b) = ([1u8; 32], [2u8; 32]);
    assert!(ours_stands(&a, &b));
    assert!(!ours_stands(&b, &a), "exactly one side keeps its own");
}

#[test]
fn a_failed_relay_dial_is_retried_soon_then_less_often() {
    let secs: Vec<u64> = (0..8).map(|n| peer_relay_backoff(n).as_secs()).collect();
    assert_eq!(secs, vec![5, 10, 20, 40, 80, 120, 120, 120]);
}

#[test]
fn an_unreachable_relay_is_dialled_again_for_a_few_minutes_then_left() {
    let waits: Vec<Option<u64>> = (0..7)
        .map(|n| peer_relay_redial_after(n).map(|d| d.as_secs()))
        .collect();
    assert_eq!(
        waits,
        vec![Some(5), Some(10), Some(20), Some(40), Some(80), None, None]
    );
}

#[test]
fn extra_connections_to_our_relay_only_while_parts_come_to_one_elsewhere() {
    let now = 1_000_000;
    let limit = crate::files::lanes::limit();
    assert!(!collect_lane_wanted(true, 0, now, 0), "no part yet");
    assert_eq!(collect_lane_wanted(true, now - 1_000, now, 0), limit > 1);
    assert!(
        !collect_lane_wanted(false, now - 1_000, now, 0),
        "a relay in this process is a pipe"
    );
    if limit > 1 {
        assert!(collect_lane_wanted(true, now - 1_000, now, limit - 2));
    }
    assert!(
        !collect_lane_wanted(true, now - 1_000, now, limit - 1),
        "no more than the sender's lanes"
    );
    assert!(collect_lane_holds(now - COLLECT_LANE_IDLE_MS + 1, now));
    assert!(
        !collect_lane_holds(now - COLLECT_LANE_IDLE_MS, now),
        "a quiet minute closes them"
    );
}

#[test]
fn unknown_console_kind_still_decodes() {
    let mut p = sample();
    p.console = Some(WireConsole::new(200));
    let back = decode_payload(&encode_payload(&p).unwrap()).unwrap();
    assert_eq!(back.console.map(|c| c.kind), Some(200));
}

/// Older clients could omit the padding bytes. Incoming messages must remain
/// readable after outgoing messages always use the standard size buckets.
#[test]
fn an_unpadded_payload_unpacks_like_a_padded_one() {
    let body = b"who is going to be at the thing on friday";
    let padded = pad_payload(body);
    let mut bare = (body.len() as u32).to_be_bytes().to_vec();
    bare.extend_from_slice(body);

    assert_eq!(unpad_payload(&padded).as_deref(), Some(&body[..]));
    assert_eq!(unpad_payload(&bare).as_deref(), Some(&body[..]));
    assert_eq!(
        padded.len(),
        256,
        "a short message still fills the smallest bucket"
    );
    assert_eq!(
        bare.len(),
        4 + body.len(),
        "unpadded is the length prefix and the message"
    );
}

/// Small text and voice payloads both use the next standard size bucket.
#[test]
fn text_and_voice_payloads_use_standard_padding_buckets() {
    let text = vec![0u8; 40];
    assert_eq!(pad_payload(&text).len(), 256);

    let voice = vec![0u8; 5 * 1024];
    assert_eq!(pad_payload(&voice).len(), 16_384);
}
