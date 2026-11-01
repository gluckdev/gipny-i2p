#[cfg(not(target_arch = "wasm32"))]
pub mod agent;
pub mod card;
pub mod crypto;
#[cfg(not(target_arch = "wasm32"))]
pub mod device_binding;
#[cfg(not(target_arch = "wasm32"))]
pub mod db;
#[cfg(target_arch = "wasm32")]
#[path = "dm_reference.rs"]
pub mod db;
#[cfg(not(target_arch = "wasm32"))]
pub mod dht_client;
#[cfg(not(target_arch = "wasm32"))]
pub mod i2p_http;
#[cfg(not(target_arch = "wasm32"))]
pub mod embedded;
#[cfg(not(target_arch = "wasm32"))]
pub mod files;
#[cfg(not(target_arch = "wasm32"))]
pub mod memory;
#[cfg(target_arch = "wasm32")]
#[path = "memory_browser.rs"]
pub mod memory;
#[cfg(not(target_arch = "wasm32"))]
pub mod net;
#[cfg(not(target_arch = "wasm32"))]
pub mod relay;
#[cfg(target_arch = "wasm32")]
#[path = "relay_wire.rs"]
pub mod relay;
#[cfg(not(target_arch = "wasm32"))]
pub mod relay_server;
#[cfg(not(target_arch = "wasm32"))]
pub mod router;
#[cfg(not(target_arch = "wasm32"))]
pub mod security;
#[cfg(not(target_arch = "wasm32"))]
pub mod session;
#[cfg(target_arch = "wasm32")]
#[path = "message_wire.rs"]
pub mod session;
#[cfg(not(target_arch = "wasm32"))]
pub mod update;

#[cfg(not(target_arch = "wasm32"))]
pub use i2p_embed::{debug_trace, diagnostics};

pub use crypto::{Identity, IdentityCard, PreKeyBundle, PreKeyPair, RatchetState, X3dhInitial, CryptoError};
#[cfg(not(target_arch = "wasm32"))]
pub use db::{Db, DbError, Contact, Message, NewAttachment, Direction, TrustLevel, PreKeyKind};
#[cfg(not(target_arch = "wasm32"))]
pub use net::{I2pNode, TorNode, NetError};
#[cfg(not(target_arch = "wasm32"))]
pub use embedded::fast_on;
pub use relay::{ClientToRelay, RelayToClient, EnvelopeBlob, DEFAULT_RELAY};
#[cfg(not(target_arch = "wasm32"))]
pub use relay::{RelayClient, RelayError};
#[cfg(not(target_arch = "wasm32"))]
pub use relay_server::{DhtHandler, EphemeralRelay, MemStoreLimits};
#[cfg(not(target_arch = "wasm32"))]
pub use security::{MasterKey, Vault, DuressMode, UnlockOutcome};
pub use session::{WirePayload, WireAttachment, WireButton, WireGroupRef, WireMember, WirePin, WireConsole};
#[cfg(not(target_arch = "wasm32"))]
pub use session::{SessionManager, SessionError, SessionEvent};
pub use session::{CONSOLE_COMMAND, CONSOLE_OUTPUT, CONSOLE_GRANT, CONSOLE_REVOKE, CONSOLE_OFF};
pub use card::{ContactCard, CardError, is_valid_i2p_address};
#[cfg(not(target_arch = "wasm32"))]
pub use update::{Updater, UpdateError, UpdateInfo, ReleaseAsset, ReleaseInfo, Sha256Digest, InstallOutcome, Component as UpdateComponent};
