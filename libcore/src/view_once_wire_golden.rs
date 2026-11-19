// Wire format shared by native and browser personal terminals.
#[test]
fn view_once_v1_frame_layout() {
    use crate::view_once::Request;
    let request = Request {
        to: [1; 32],
        id: [2; 16],
        nonce: [3; 32],
        expires_at: 4,
    };
    let request_bytes = [&[1; 32][..], &[2; 16], &[3; 32], &4i64.to_le_bytes()].concat();
    let vector = [&2u64.to_le_bytes()[..], &[7, 8]].concat();
    let token = [5; 16];
    let client = vec![
        (
            ClientToRelay::ViewOnceOfferV1(request),
            8u32,
            request_bytes.clone(),
        ),
        (
            ClientToRelay::ViewOncePartV1 {
                index: 1,
                total: 2,
                bytes: vec![7, 8],
            },
            9,
            [&[1, 0, 2, 0][..], &vector].concat(),
        ),
    ];
    for (frame, index, body) in client {
        assert_eq!(enc(&frame), [&index.to_le_bytes()[..], &body].concat());
    }
    let relay = vec![
        (
            RelayToClient::ViewOnceReadyV1 { offer: vec![7, 8] },
            9u32,
            vector.clone(),
        ),
        (
            RelayToClient::ViewOnceStoredV1 { id: token },
            10,
            token.to_vec(),
        ),
        (
            RelayToClient::ViewOncePartAckV1 { index: 1 },
            11,
            vec![1, 0],
        ),
    ];
    for (frame, index, body) in relay {
        assert_eq!(enc(&frame), [&index.to_le_bytes()[..], &body].concat());
    }
}

#[test]
fn ordinary_fragment_frame_layout() {
    let address = [5u8; 32];
    let signature = [6u8; 64];
    let vector = [&[1, 0, 2, 0][..], &2u64.to_le_bytes(), &[7, 8]].concat();
    let client_part = ClientToRelay::SendPartV1 {
        to: address,
        index: 1,
        total: 2,
        bytes: vec![7, 8],
    };
    assert_eq!(
        enc(&client_part),
        [&10u32.to_le_bytes()[..], &address, &vector].concat()
    );
    let login = ClientToRelay::AuthFramedV1 {
        sign_pk: address,
        nonce: [7; 32],
        signature,
    };
    assert_eq!(
        enc(&login),
        [&15u32.to_le_bytes()[..], &address, &[7; 32], &signature].concat()
    );
    assert_eq!(
        enc(&RelayToClient::SendPartAckV1 { index: 1 }),
        [12, 0, 0, 0, 1, 0]
    );
    assert_eq!(
        enc(&RelayToClient::AuthFramedOkV1 { proof: signature }),
        [&17u32.to_le_bytes()[..], &signature].concat()
    );
    let incoming = RelayToClient::IncomingPartV1 {
        id: 3,
        from: address,
        index: 1,
        total: 2,
        bytes: vec![7, 8],
    };
    assert_eq!(
        enc(&incoming),
        [
            &18u32.to_le_bytes()[..],
            &3u64.to_le_bytes(),
            &address,
            &vector
        ]
        .concat()
    );
}
