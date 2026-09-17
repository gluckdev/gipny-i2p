//! Messages between nodes. They travel inside the relay protocol as opaque
//! bytes (`ClientToRelay::Dht` / `RelayToClient::Dht`), so the relay enums in
//! both crates stay free of this one.

use serde::{Deserialize, Serialize};

use crate::crypto::{sha256, DhtKey};

pub const PROTOCOL_VERSION: u16 = 1;

/// The largest value a node stores. Mail beyond this waits for the
/// recipient's own relay instead.
pub const MAX_VALUE_BYTES: usize = 4 * 1024 * 1024;

/// A node as others know it. Its id is derived from the destination, never
/// from anything tied to the person running it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeInfo {
    pub destination: String,
    /// Holds items for others. Phones and clients behind an external relay
    /// do not, and are never chosen to.
    pub stores: bool,
    pub version: u16,
}

impl NodeInfo {
    pub fn id(&self) -> DhtKey {
        node_id(&self.destination)
    }
}

pub fn node_id(destination: &str) -> DhtKey {
    sha256(&[b"gipny-dht-node-v1", destination.as_bytes()])
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredItem {
    pub key: DhtKey,
    pub value: Vec<u8>,
    pub delete_hash: Option<[u8; 32]>,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DhtRequest {
    Ping,
    FindNode { target: DhtKey },
    /// Items under `key`, skipping the first `skip`, and nodes closer to it.
    Get { key: DhtKey, skip: u32 },
    /// `pow` makes storing cost the sender work bound to this connection's
    /// challenge, this key and this value.
    Store { item: StoredItem, pow: u64 },
    Delete { key: DhtKey, token: [u8; 32] },
}

/// A request with the sender's own node, if it is one, so the receiver learns
/// about it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DhtEnvelope {
    pub from: Option<NodeInfo>,
    pub req: DhtRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DhtResponse {
    Pong { node: NodeInfo },
    /// The answering node itself, so what it says about itself is learnt
    /// first-hand, and the nodes it knows closest to the target.
    Nodes { me: Option<NodeInfo>, nodes: Vec<NodeInfo> },
    Items { items: Vec<StoredItem>, more: bool, closer: Vec<NodeInfo> },
    Stored,
    Deleted(u32),
    Error(String),
}

pub fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    bincode::serialize(v).expect("serialize to memory")
}

pub fn decode<T: serde::de::DeserializeOwned>(b: &[u8]) -> Option<T> {
    bincode::deserialize(b).ok()
}

/// Leading zero bits of the store proof-of-work hash.
pub fn pow_hash(challenge: &[u8; 32], item: &StoredItem, nonce: u64) -> [u8; 32] {
    sha256(&[
        b"gipny-dht-pow-v1",
        challenge,
        &item.key,
        &sha256(&[&item.value]),
        &nonce.to_le_bytes(),
    ])
}

fn leading_zero_bits(h: &[u8; 32]) -> u32 {
    let mut n = 0;
    for b in h {
        if *b == 0 {
            n += 8;
        } else {
            return n + b.leading_zeros();
        }
    }
    n
}

pub fn pow_ok(challenge: &[u8; 32], item: &StoredItem, nonce: u64, difficulty: u32) -> bool {
    leading_zero_bits(&pow_hash(challenge, item, nonce)) >= difficulty
}

/// Blocking: run it off the async runtime.
pub fn solve_pow(challenge: &[u8; 32], item: &StoredItem, difficulty: u32) -> u64 {
    let value_hash = sha256(&[&item.value]);
    (0u64..)
        .find(|nonce| {
            let h = sha256(&[b"gipny-dht-pow-v1", challenge, &item.key, &value_hash, &nonce.to_le_bytes()]);
            leading_zero_bits(&h) >= difficulty
        })
        .expect("u64 nonce space")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> StoredItem {
        StoredItem { key: [1; 32], value: vec![2; 40], delete_hash: None, expires_at_ms: 9 }
    }

    #[test]
    fn work_is_bound_to_challenge_key_and_value() {
        let (c, it) = ([5; 32], item());
        let nonce = solve_pow(&c, &it, 12);
        assert!(pow_ok(&c, &it, nonce, 12));
        // A different connection, key or value needs its own work (with
        // overwhelming probability at 12 bits).
        let hits = [
            pow_ok(&[6; 32], &it, nonce, 12),
            pow_ok(&c, &StoredItem { key: [9; 32], ..it.clone() }, nonce, 12),
            pow_ok(&c, &StoredItem { value: vec![3; 40], ..it.clone() }, nonce, 12),
        ];
        assert!(hits.iter().filter(|h| **h).count() <= 1);
    }

    #[test]
    fn node_ids_follow_the_destination() {
        let a = NodeInfo { destination: "a".into(), stores: true, version: 1 };
        let b = NodeInfo { destination: "a".into(), stores: false, version: 7 };
        assert_eq!(a.id(), b.id());
        assert_ne!(a.id(), node_id("b"));
    }
}
