//! Peer discovery for personal Gipny I2P terminals.
//!
//! Desktop and headless clients use I2P connections and SQLite. Protocol
//! tests implement the same traits with an in-memory network.

pub mod crypto;
pub mod items;
pub mod proto;
pub mod store;
pub mod node;
