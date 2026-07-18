use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

#[derive(Serialize, Deserialize, Debug)]
pub enum ClientToRelay {
    Auth {
        sign_pk: [u8; 32],
        #[serde(with = "BigArray")]
        signature: [u8; 64],
    },
    Publish { bundle: Vec<u8> },
    GetBundle { pk: [u8; 32] },
    Send { to: [u8; 32], blob: Vec<u8> },
    Ack { id: u64 },
    Ping,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum RelayToClient {
    Challenge([u8; 32]),
    AuthOk,
    AuthFail,
    Bundle { pk: [u8; 32], bundle: Option<Vec<u8>> },
    Incoming { id: u64, from: [u8; 32], blob: Vec<u8> },
    Deposited { id: u64 },
    Pong,
    Error(String),
}

pub const HS_PORT: u16 = 443;
pub const HS_NICKNAME: &str = "gipny-relay";
pub const MAX_FRAME: u32 = 16 * 1024 * 1024;

/// Wire-format compatibility tests.
///
/// The relay uses `bincode::config::legacy()` (bincode 2.x) which MUST produce
/// byte-identical output to bincode 1.x serde encoding.  Clients are built
/// against the main workspace that still uses bincode 1.x, so any drift here
/// would silently break the protocol.  The golden bytes below were derived from
/// the bincode 1.x encoding spec:
///   - enum variant: u32 little-endian
///   - integer fields: little-endian fixed-width
///   - Vec<u8>: u64 LE length prefix + raw bytes
///   - [u8; N]: raw N bytes (no length prefix)
///   - Option<T>: u8 (0=None / 1=Some) then T if present
#[cfg(test)]
mod wire_compat {
    use super::*;
    use bincode::config::legacy;
    use bincode::serde::{decode_from_slice, encode_to_vec};

    fn enc<T: serde::Serialize>(v: &T) -> Vec<u8> {
        encode_to_vec(v, legacy()).expect("encode")
    }

    fn dec<T: serde::de::DeserializeOwned>(buf: &[u8]) -> T {
        decode_from_slice(buf, legacy()).expect("decode").0
    }

    // ── simple unit variants ─────────────────────────────────────────────────

    #[test]
    fn ping_golden() {
        // ClientToRelay::Ping is variant index 5
        let golden: &[u8] = &[0x05, 0x00, 0x00, 0x00];
        assert_eq!(enc(&ClientToRelay::Ping), golden);
        assert!(matches!(dec::<ClientToRelay>(golden), ClientToRelay::Ping));
    }

    #[test]
    fn pong_golden() {
        // RelayToClient::Pong is variant index 6
        let golden: &[u8] = &[0x06, 0x00, 0x00, 0x00];
        assert_eq!(enc(&RelayToClient::Pong), golden);
        assert!(matches!(dec::<RelayToClient>(golden), RelayToClient::Pong));
    }

    #[test]
    fn auth_ok_golden() {
        // RelayToClient::AuthOk is variant index 1
        let golden: &[u8] = &[0x01, 0x00, 0x00, 0x00];
        assert_eq!(enc(&RelayToClient::AuthOk), golden);
        assert!(matches!(dec::<RelayToClient>(golden), RelayToClient::AuthOk));
    }

    #[test]
    fn auth_fail_golden() {
        // RelayToClient::AuthFail is variant index 2
        let golden: &[u8] = &[0x02, 0x00, 0x00, 0x00];
        assert_eq!(enc(&RelayToClient::AuthFail), golden);
        assert!(matches!(dec::<RelayToClient>(golden), RelayToClient::AuthFail));
    }

    // ── u64-field variants ───────────────────────────────────────────────────

    #[test]
    fn ack_golden() {
        // ClientToRelay::Ack { id } is variant index 4; id is u64 LE
        let msg = ClientToRelay::Ack { id: 0x0102_0304_0506_0708 };
        #[rustfmt::skip]
        let golden: &[u8] = &[
            0x04, 0x00, 0x00, 0x00,              // variant 4
            0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, // id LE
        ];
        assert_eq!(enc(&msg), golden);
        if let ClientToRelay::Ack { id } = dec::<ClientToRelay>(golden) {
            assert_eq!(id, 0x0102_0304_0506_0708);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn deposited_golden() {
        // RelayToClient::Deposited { id } is variant index 5
        let msg = RelayToClient::Deposited { id: 0x0102_0304_0506_0708 };
        #[rustfmt::skip]
        let golden: &[u8] = &[
            0x05, 0x00, 0x00, 0x00,
            0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01,
        ];
        assert_eq!(enc(&msg), golden);
        if let RelayToClient::Deposited { id } = dec::<RelayToClient>(golden) {
            assert_eq!(id, 0x0102_0304_0506_0708);
        } else {
            panic!("wrong variant");
        }
    }

    // ── fixed-array variants ─────────────────────────────────────────────────

    #[test]
    fn challenge_golden() {
        // RelayToClient::Challenge([u8; 32]) is variant index 0
        // fixed array encodes as raw bytes — no length prefix
        let mut arr = [0u8; 32];
        arr[0] = 0xAB;
        arr[31] = 0xCD;
        let msg = RelayToClient::Challenge(arr);
        let encoded = enc(&msg);
        assert_eq!(encoded.len(), 4 + 32);
        assert_eq!(&encoded[..4], &[0x00, 0x00, 0x00, 0x00]);
        assert_eq!(encoded[4], 0xAB);
        assert_eq!(encoded[35], 0xCD);
        if let RelayToClient::Challenge(a) = dec::<RelayToClient>(&encoded) {
            assert_eq!(a, arr);
        } else {
            panic!("wrong variant");
        }
    }

    // ── Auth (sign_pk + 64-byte BigArray signature) ──────────────────────────

    #[test]
    fn auth_golden() {
        // ClientToRelay::Auth is variant index 0
        // sign_pk: [u8;32] raw bytes; signature: [u8;64] via BigArray (raw bytes)
        let sign_pk = [0xAA_u8; 32];
        let signature = [0xBB_u8; 64];
        let msg = ClientToRelay::Auth { sign_pk, signature };
        let encoded = enc(&msg);
        assert_eq!(encoded.len(), 4 + 32 + 64); // variant + pk + sig
        assert_eq!(&encoded[..4], &[0x00, 0x00, 0x00, 0x00]);
        assert!(encoded[4..36].iter().all(|&b| b == 0xAA));
        assert!(encoded[36..100].iter().all(|&b| b == 0xBB));
    }

    // ── Send (to + Vec<u8> blob) ─────────────────────────────────────────────

    #[test]
    fn send_golden() {
        // ClientToRelay::Send is variant index 3
        // Vec<u8> encodes as u64 LE length + raw bytes
        let to = [0x11_u8; 32];
        let blob = vec![0x01_u8, 0x02, 0x03];
        let msg = ClientToRelay::Send { to, blob };
        let encoded = enc(&msg);
        // variant(4) + to(32) + len(8) + blob(3)
        assert_eq!(encoded.len(), 4 + 32 + 8 + 3);
        assert_eq!(&encoded[..4], &[0x03, 0x00, 0x00, 0x00]);
        assert!(encoded[4..36].iter().all(|&b| b == 0x11));
        // length = 3 as u64 LE
        assert_eq!(&encoded[36..44], &[0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(&encoded[44..47], &[0x01, 0x02, 0x03]);
    }

    // ── Bundle (Option<Vec<u8>>) ─────────────────────────────────────────────

    #[test]
    fn bundle_none_golden() {
        // RelayToClient::Bundle is variant index 3; Option encodes as u8 tag
        let msg = RelayToClient::Bundle { pk: [0u8; 32], bundle: None };
        let encoded = enc(&msg);
        // variant(4) + pk(32) + option_tag(1)
        assert_eq!(encoded.len(), 4 + 32 + 1);
        assert_eq!(&encoded[..4], &[0x03, 0x00, 0x00, 0x00]);
        assert_eq!(encoded[36], 0x00); // None
    }

    #[test]
    fn bundle_some_golden() {
        let data = vec![0xDE_u8, 0xAD, 0xBE, 0xEF];
        let msg = RelayToClient::Bundle { pk: [0u8; 32], bundle: Some(data.clone()) };
        let encoded = enc(&msg);
        // variant(4) + pk(32) + option_tag(1) + vec_len(8) + data(4)
        assert_eq!(encoded.len(), 4 + 32 + 1 + 8 + 4);
        assert_eq!(encoded[36], 0x01); // Some
        assert_eq!(&encoded[37..45], &[0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(&encoded[45..], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    // ── full round-trips ─────────────────────────────────────────────────────

    #[test]
    fn roundtrip_all_client_variants() {
        let msgs: Vec<ClientToRelay> = vec![
            ClientToRelay::Auth { sign_pk: [1u8; 32], signature: [2u8; 64] },
            ClientToRelay::Publish { bundle: vec![9, 8, 7] },
            ClientToRelay::GetBundle { pk: [3u8; 32] },
            ClientToRelay::Send { to: [4u8; 32], blob: vec![5, 6] },
            ClientToRelay::Ack { id: 42 },
            ClientToRelay::Ping,
        ];
        for msg in msgs {
            let encoded = enc(&msg);
            let decoded: ClientToRelay = dec(&encoded);
            // re-encode the decoded value and compare bytes
            assert_eq!(encoded, enc(&decoded));
        }
    }

    #[test]
    fn roundtrip_all_relay_variants() {
        let msgs: Vec<RelayToClient> = vec![
            RelayToClient::Challenge([7u8; 32]),
            RelayToClient::AuthOk,
            RelayToClient::AuthFail,
            RelayToClient::Bundle { pk: [1u8; 32], bundle: None },
            RelayToClient::Bundle { pk: [2u8; 32], bundle: Some(vec![3, 4]) },
            RelayToClient::Incoming { id: 99, from: [5u8; 32], blob: vec![6, 7, 8] },
            RelayToClient::Deposited { id: 123 },
            RelayToClient::Pong,
            RelayToClient::Error("oops".into()),
        ];
        for msg in msgs {
            let encoded = enc(&msg);
            let decoded: RelayToClient = dec(&encoded);
            assert_eq!(encoded, enc(&decoded));
        }
    }
}