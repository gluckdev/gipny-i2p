//! A network of gipny relays that hold sealed items for each other.
//!
//! Transport and storage are traits: the desktop app and the agent plug in
//! their i2p connections and SQLite, the standalone relay its own, and the
//! tests an in-memory network.

pub mod crypto;
pub mod items;
pub mod proto;
pub mod store;
pub mod node;
