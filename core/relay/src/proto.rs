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