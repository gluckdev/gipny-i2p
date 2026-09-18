use std::collections::{HashMap, HashSet};
use gipny_libcore::dht_client;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gipny_libcore::crypto::fill_random;
use serde::Serialize;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use gipny_libcore::crypto::{
    self, AttachmentCipher, Identity, IdentityCard, PreKeyBundle, PreKeyPair, RatchetState,
};
use gipny_libcore::db::{Attachment, Db, Direction, GroupMember, NewAttachment, PreKeyKind, RequestState, TrustLevel};
use gipny_libcore::net::{NetError, TorNode};
use gipny_libcore::relay::{self, ClientToRelay, EnvelopeBlob, RelayClient, RelayToClient, DEFAULT_RELAY};
use gipny_libcore::update::{Component as UpdateComponent, InstallOutcome, UpdateError, UpdateInfo, Updater};

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("db")] Db(#[from] gipny_libcore::db::DbError),
    #[error("crypto")] Crypto(#[from] gipny_libcore::crypto::CryptoError),
    #[error("net")] Net(#[from] NetError),
    #[error("relay")] Relay(#[from] gipny_libcore::relay::RelayError),
    #[error("io")] Io(#[from] std::io::Error),
    #[error("update")] Update(#[from] UpdateError),
    #[error("codec")] Codec,
    #[error("not found")] NotFound,
    #[error("state")] State,
    #[error("stale opk")] StaleOpk,
    #[error("sealed drop")] SealedDrop,
}

impl From<bincode::Error> for CoreError { fn from(_: bincode::Error) -> Self { Self::Codec } }

const SETTING_IDENTITY_SIGN: &str = "identity_sign";
const SETTING_IDENTITY_DH: &str = "identity_dh";
const SETTING_SIGNED_PREKEY_ID: &str = "signed_prekey_id";
const SETTING_RELAY_ONION: &str = "relay_onion";
const SETTING_RELAY_MODE: &str = "relay_mode";
/// A contact's relay that has not answered for this long, while mail for them
/// is queued, is reported to the user: with built-in relays it most likely
/// means the contact restarted and collects somewhere else now.
const CONTACT_UNREACHABLE_AFTER: Duration = Duration::from_secs(600);
/// Beyond this a round trip is not a slow link but a message that sat in the
/// retry queue, or a clock that moved. Ten minutes is already far past the
/// worst honest case (a cold router rebuilding tunnels).
const MAX_PLAUSIBLE_RTT_MS: i64 = 600_000;
const SETTING_DISMISSED_UPDATE: &str = "dismissed_update_version";
/// Default on: absent or anything but `"0"` means auto-update stays on,
/// matching `attachment_privacy`'s convention.
const SETTING_AUTO_UPDATE: &str = "auto_update";
/// What the interface keeps in the vault for itself (JSON it does not
/// interpret here): contact folders, chosen avatars. Only these keys.
const UI_DATA_KEYS: &[&str] = &["contact_folders", "avatars"];
const MAX_UI_DATA_BYTES: usize = 64 * 1024;
const ATTACHMENTS_DIR: &str = "attachments";
const TARGET_OPK: usize = 20;
const PURGE_INTERVAL_SECS: u64 = 60;
const RECONNECT_INITIAL_MS: u64 = 500;
const RECONNECT_MAX_MS: u64 = 15_000;
/// How long to wait for another person's relay to answer before giving up on
/// this attempt. Nothing in the dial path has a timeout of its own, and opening
/// an i2p destination means building tunnels, so an unreachable relay would
/// otherwise hang its connection task forever.
/// State of a connection to somebody else's relay.
///
/// `Connecting` and `Failed` exist so the send loop never dials: it checks this
/// map, and either gets a live sender or moves on to the next contact.
/// An unconfirmed address announcement is repeated no more often than this.
const ANNOUNCE_REPEAT_EVERY: Duration = Duration::from_secs(20 * 60);

/// A contact whose relay is silent gets one address lookup in the network per
/// this interval; each one costs tunnels and tens of seconds.
const DHT_ADDRESS_LOOKUP_EVERY: Duration = Duration::from_secs(10 * 60);

/// How often to look in the network for letters left while we were away.
const DHT_COLLECT_EVERY: Duration = Duration::from_secs(10 * 60);

/// Letters are remembered as handled for as long as one can live in the
/// network, plus a day.
const DHT_SEEN_TTL_MS: i64 = 8 * 24 * 3600 * 1000;

/// How far back the first pass after a launch looks; a letter lives a week.
const DHT_COLLECT_DAYS_FIRST: u32 = 7;
/// Later passes only need today and (around midnight) yesterday.
const DHT_COLLECT_DAYS: u32 = 2;

/// Where an outgoing letter goes. `Relay` is a live connection to the relay
/// the contact collects from; `Dht` leaves it in the relay network, where it
/// waits until they come back (see libcore/src/dht_client.rs).
enum Route {
    Relay(mpsc::Sender<ClientToRelay>),
    Dht,
}

enum PeerRelay {
    Ready(mpsc::Sender<ClientToRelay>),
    Connecting,
    Failed { until: Instant },
}

/// How long to wait for another person's relay to answer before giving up on
/// this attempt. Nothing in the dial path has a timeout of its own, and opening
/// an i2p destination means building tunnels, so an unreachable relay would
/// otherwise hang its connection task forever.
const PEER_RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(90);
/// How long to leave a peer relay alone after a failed dial. The send loop runs
/// every few seconds; without this it would rebuild tunnels to a dead relay on
/// every tick.
const PEER_RELAY_RETRY_BACKOFF: Duration = Duration::from_secs(120);
const PING_INTERVAL_SECS: u64 = 20;
const DEAD_THRESHOLD_SECS: u64 = 75;
const BUNDLE_REFRESH_SECS: u64 = 12 * 3600;
const EVENTS_CAPACITY: usize = 1024;
const PENDING_REQ_TIMEOUT_MS: u64 = 30_000;
const MAX_PAYLOAD_BYTES: usize = 14 * 1024 * 1024;
const RETRY_BASE_BACKOFF_MS: i64 = 5_000;
const RETRY_MAX_BACKOFF_MS: i64 = 300_000;
const FRESH_SESSION_GRACE_MS: i64 = 60_000;
const TIEBREAKER_TIMEOUT_MS: i64 = 10_000;
const KEEPALIVE_INCOMING_THRESHOLD: u32 = 100;
/// Unanswered introductions kept at once; the oldest goes first.
const MAX_INCOMING_REQUESTS: usize = 50;

#[derive(Debug, Clone, Serialize)]
pub enum CoreEvent {
    IncomingMessage {
        contact_id: Option<i64>,
        group_id: Option<String>,
        sender_sign_pk: Option<String>,
        message_id: i64,
        body: String,
        sent_at: i64,
        notify_sound: Option<String>,
        /// Set when the message is console-framed (a command, output, or an
        /// agent-mode marker), so the UI can skip the usual notification.
        console_kind: Option<u8>,
    },
    MessageEdited {
        message_id: i64,
        body: String,
        buttons: Option<Vec<Vec<WireButton>>>,
    },
    MessagePinned {
        contact_id: Option<i64>,
        group_id: Option<String>,
        message_id: i64,
    },
    MessageUnpinned {
        contact_id: Option<i64>,
        group_id: Option<String>,
        message_id: i64,
    },
    MessageSent { message_id: i64 },
    MessageDelivered { message_id: i64 },
    /// Median round trip to this contact, in milliseconds, after a fresh
    /// measurement. Drives the link readout above the chat.
    LinkRtt { contact_id: i64, ms: u32 },
    Typing {
        contact_id: Option<i64>,
        group_id: Option<String>,
        sender_sign_pk: Option<String>,
        typing: bool,
    },
    RelayConnected,
    RelayDisconnected,
    ContactAdded { contact_id: i64 },
    ContactUpdated { contact_id: i64 },
    /// Someone we do not know introduced themselves.
    ContactRequest { contact_id: i64 },
    /// Joined the relay network (or tried to): how many nodes answered. The
    /// unlock screen shows this as its last step.
    DhtJoined { peers: usize },
    GroupUpdated { group_id: String },
    /// A newer version exists and auto-update is off — the UI's manual
    /// install prompt is the only path from here.
    UpdateAvailable { version: String, notes: String, size: u64 },
    UpdateProgress { downloaded: u64, total: u64, pct: u8 },
    /// Downloaded and installed (or staged for the next launch, on Windows) —
    /// nothing to click, just a notice.
    UpdateStaged { version: String },
    /// Downloaded, but this build cannot install it automatically; `path` is
    /// where it landed.
    UpdateReady { path: String },
    UpdateFailed { reason: String },
    /// Agent mode switched on (with this master) or off, locally or remotely.
    AgentModeChanged { master: Option<AgentMaster> },
    /// This client, in agent mode, produced console output for `contact_id`.
    ConsoleActivity { contact_id: i64 },
    /// The relay mode changed, or the built-in relay changed state.
    RelayInfoChanged { info: RelayInfo },
    /// Mail for this contact is queued and their relay has not answered for a
    /// while (`unreachable`), or it answers again.
    ContactReachability { contact_id: i64, unreachable: bool },
}

/// Where this client collects its mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayMode {
    /// A relay inside this process, on a destination made at launch and never
    /// written down. The default: a fresh install works with no setup.
    Builtin,
    /// A relay somebody operates, named in Settings. A mailbox that is there
    /// while this app is not.
    External,
}

impl RelayMode {
    fn as_str(self) -> &'static str {
        match self { Self::Builtin => "builtin", Self::External => "external" }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s { "builtin" => Some(Self::Builtin), "external" => Some(Self::External), _ => None }
    }
}

/// The saved choice if there is one. Without one, a profile that already names
/// a relay keeps using it, and a profile that names none gets the built-in one.
fn resolve_relay_mode(saved: Option<&[u8]>, external: &str) -> RelayMode {
    match saved.and_then(|b| std::str::from_utf8(b).ok()).and_then(RelayMode::parse) {
        Some(mode) => mode,
        None if external.trim().is_empty() => RelayMode::Builtin,
        None => RelayMode::External,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum HostedRelayState {
    Off,
    /// A published destination is usable once its tunnels exist: a minute or two.
    Starting,
    Ready { address: String },
    Failed { reason: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct RelayInfo {
    pub mode: RelayMode,
    /// The address saved for external mode, whether or not it is in use.
    pub external: String,
    pub hosted: HostedRelayState,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentMaster {
    pub contact_id: i64,
    pub name: String,
    pub sign_pk: String,
}

use gipny_libcore::{WirePayload, WireAttachment, WireButton, WireGroupRef, WireMember, WireConsole};
use gipny_libcore::{CONSOLE_COMMAND, CONSOLE_GRANT, CONSOLE_REVOKE, CONSOLE_OFF};
use gipny_libcore::agent::{self, ExecOptions, SETTING_AGENT_MASTER, BODY_GRANT, BODY_REVOKE};

fn hex_bytes(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b { s.push_str(&format!("{:02x}", x)); }
    s
}
use gipny_libcore::session::{WirePin, WireReply, encode_payload, decode_payload, pad_payload, unpad_payload};

fn decode_with_padding_fallback(pt: &[u8]) -> std::result::Result<WirePayload, bincode::Error> {
    if let Some(unpadded) = unpad_payload(pt) {
        if let Ok(p) = decode_payload(&unpadded) { return Ok(p); }
    }
    decode_payload(pt)
}

pub struct PendingAttachment { pub name: String, pub data: Vec<u8> }

type BundleWaiter = tokio::sync::oneshot::Sender<Option<Vec<u8>>>;

pub struct Core {
    db: Arc<Db>,
    node: Arc<TorNode>,
    identity: Arc<Identity>,
    sessions: Arc<Mutex<HashMap<i64, RatchetState>>>,
    events: mpsc::Sender<CoreEvent>,
    data_dir: PathBuf,
    /// Connection to our own relay — where we receive, and where our prekey
    /// bundle is published.
    relay_out: Arc<RwLock<Option<mpsc::Sender<ClientToRelay>>>>,
    /// Connections to other people's relays, keyed by destination. A message
    /// goes to where its recipient collects it, which is usually not here.
    peer_relays: Arc<Mutex<HashMap<String, PeerRelay>>>,
    bundle_waiters: Arc<Mutex<HashMap<[u8; 32], Vec<BundleWaiter>>>>,
    send_kick: Arc<tokio::sync::Notify>,
    tiebreaker_waits: Arc<Mutex<HashMap<i64, i64>>>,
    session_created_at: Arc<Mutex<HashMap<i64, i64>>>,
    incoming_since_send: Arc<Mutex<HashMap<i64, u32>>>,
    updater: Arc<Updater>,
    pending_update: Arc<Mutex<Option<UpdateInfo>>>,
    tasks: Arc<std::sync::Mutex<Vec<JoinHandle<()>>>>,
    /// Ids of console commands to run, in order, by the single agent worker.
    agent_tx: mpsc::UnboundedSender<i64>,
    /// The relay this process hosts for itself in built-in mode. Holding it is
    /// what keeps it serving; dropping it takes the destination away.
    hosted_relay: Arc<std::sync::Mutex<Option<gipny_libcore::EphemeralRelay>>>,
    hosted_state: Arc<std::sync::RwLock<HostedRelayState>>,
    hosted_task: Arc<std::sync::Mutex<Option<JoinHandle<()>>>>,
    /// Contacts that have not been told this launch's relay address yet.
    announce_pending: Arc<Mutex<HashSet<i64>>>,
    /// When each of them was last told, so the repeat is paced.
    announce_sent_at: Arc<Mutex<HashMap<i64, Instant>>>,
    /// When each peer relay first stopped answering, and who was told about it.
    relay_down_since: Arc<Mutex<HashMap<String, Instant>>>,
    unreachable_reported: Arc<Mutex<HashSet<i64>>>,
    /// Our node in the relay network, answering through the built-in relay.
    dht: Arc<dht_client::Node>,
    /// When we last asked the network where a contact collects, so a contact
    /// whose relay is down does not start a lookup on every send tick.
    dht_addr_asked: Arc<Mutex<HashMap<i64, Instant>>>,
    /// Measured round trip per contact: how long the last few messages took
    /// from leaving here to their acknowledgement coming back. Kept in memory
    /// on purpose — it describes this link right now, and a number from last
    /// week would only mislead.
    rtt: Arc<Mutex<HashMap<i64, Rtt>>>,
}

/// Hops in a tunnel as i2p builds them by default, and as we have always
/// asked for them. Named rather than spelled `3` at the call site because
/// TURBO is about to make it a choice.
const DEFAULT_TUNNEL_HOPS: u8 = 3;

/// How a letter to this contact leaves right now.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkRoute {
    /// Straight to the relay they collect from.
    Relay,
    /// Their relay is silent; the network holds the letter until they return.
    Archive,
    /// Nowhere to put it — their relay is unknown and we are not in the
    /// network either.
    None,
}

/// The channel to one contact, as the readout above the chat shows it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct LinkStats {
    /// Hops on our outbound tunnel — our leg, our exposure.
    pub our_hops: u8,
    /// Hops on the inbound tunnel of the relay they collect from — their leg.
    pub their_hops: u8,
    /// Whether messages are still padded to fixed size buckets.
    pub padded: bool,
    pub route: LinkRoute,
    /// Median of the last few measured round trips, absent until one message
    /// has been acknowledged.
    pub rtt_ms: Option<u32>,
}

/// The last handful of round trips to one contact.
///
/// The median, not the mean: one message that waited out a tunnel rebuild
/// would drag an average somewhere it has never been, and the number exists to
/// tell the user what the link feels like.
#[derive(Default, Clone)]
pub struct Rtt {
    samples: Vec<u32>,
}

impl Rtt {
    const KEEP: usize = 7;

    fn push(&mut self, ms: u32) {
        self.samples.push(ms);
        if self.samples.len() > Self::KEEP {
            self.samples.remove(0);
        }
    }

    pub fn median_ms(&self) -> Option<u32> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        Some(sorted[sorted.len() / 2])
    }
}

impl Core {
    pub async fn start(
        data_dir: PathBuf,
        db: Arc<Db>,
        node: Arc<TorNode>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<CoreEvent>)> {
        // A staged Windows update is applied earlier than this, in `lib.rs`'s
        // `boot()` — before the vault unlock and the router wait below, not
        // after them.
        std::fs::create_dir_all(data_dir.join(ATTACHMENTS_DIR))?;
        let identity = Arc::new(Self::load_or_create_identity(&db)?);
        let (events_tx, events_rx) = mpsc::channel(EVENTS_CAPACITY);
        let updater = Arc::new(Updater::new(node.clone(), UpdateComponent::App));
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();
        let core = Arc::new(Self {
            db: db.clone(),
            node: node.clone(),
            identity,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            events: events_tx,
            data_dir,
            relay_out: Arc::new(RwLock::new(None)),
            peer_relays: Arc::new(Mutex::new(HashMap::new())),
            bundle_waiters: Arc::new(Mutex::new(HashMap::new())),
            send_kick: Arc::new(tokio::sync::Notify::new()),
            tiebreaker_waits: Arc::new(Mutex::new(HashMap::new())),
            session_created_at: Arc::new(Mutex::new(HashMap::new())),
            incoming_since_send: Arc::new(Mutex::new(HashMap::new())),
            updater,
            pending_update: Arc::new(Mutex::new(None)),
            tasks: Arc::new(std::sync::Mutex::new(Vec::new())),
            agent_tx,
            hosted_relay: Arc::new(std::sync::Mutex::new(None)),
            hosted_state: Arc::new(std::sync::RwLock::new(HostedRelayState::Off)),
            hosted_task: Arc::new(std::sync::Mutex::new(None)),
            announce_pending: Arc::new(Mutex::new(HashSet::new())),
            announce_sent_at: Arc::new(Mutex::new(HashMap::new())),
            relay_down_since: Arc::new(Mutex::new(HashMap::new())),
            unreachable_reported: Arc::new(Mutex::new(HashSet::new())),
            dht: dht_client::new_node(node.clone(), db.clone()),
            dht_addr_asked: Arc::new(Mutex::new(HashMap::new())),
            rtt: Arc::new(Mutex::new(HashMap::new())),
        });
        core.ensure_prekeys().await?;
        let _ = core.db.cleanup_orphan_pins();
        core.clone().spawn_relay_loop();
        core.clone().spawn_send_loop();
        core.clone().spawn_purge_loop();
        core.clone().spawn_update_loop();
        core.clone().spawn_agent_worker(agent_rx);
        core.clone().spawn_dht_loop();
        if core.relay_mode() == RelayMode::Builtin {
            core.start_hosted_relay();
        }
        if let Some(m) = core.agent_master() {
            // Commands that arrived while the app was closed are on disk with
            // their pending markers; pick them up in order.
            core.enqueue_pending_commands(m.contact_id);
        }
        Ok((core, events_rx))
    }

    pub fn shutdown(&self) {
        let mut v = self.tasks.lock().unwrap();
        for h in v.drain(..) { h.abort(); }
    }

    /// The relay named in Settings, used in external mode.
    fn external_relay(&self) -> String {
        self.db.get_setting(SETTING_RELAY_ONION).ok().flatten()
            .and_then(|v| String::from_utf8(v).ok())
            .unwrap_or_else(|| DEFAULT_RELAY.to_string())
    }

    pub fn relay_mode(&self) -> RelayMode {
        resolve_relay_mode(
            self.db.get_setting(SETTING_RELAY_MODE).ok().flatten().as_deref(),
            &self.external_relay(),
        )
    }

    /// Where we collect right now; empty while there is nowhere yet. In
    /// built-in mode this is the address of this launch and lives in memory
    /// only — it is never written into the settings, which is how 0.4.0 left
    /// clients listening on a relay that no longer existed.
    fn relay_onion(&self) -> String {
        match self.relay_mode() {
            RelayMode::External => self.external_relay(),
            RelayMode::Builtin => match &*self.hosted_state.read().unwrap_or_else(|p| p.into_inner()) {
                HostedRelayState::Ready { address } => address.clone(),
                _ => String::new(),
            },
        }
    }

    pub fn relay_info(&self) -> RelayInfo {
        RelayInfo {
            mode: self.relay_mode(),
            external: self.external_relay(),
            hosted: self.hosted_state.read().unwrap_or_else(|p| p.into_inner()).clone(),
        }
    }

    fn set_hosted_state(&self, state: HostedRelayState) {
        if !matches!(state, HostedRelayState::Ready { .. }) {
            // Without our relay nobody can reach this node; keep asking only.
            self.dht.set_me(None);
        }
        *self.hosted_state.write().unwrap_or_else(|p| p.into_inner()) = state;
        let _ = self.events.try_send(CoreEvent::RelayInfoChanged { info: self.relay_info() });
    }

    pub async fn set_relay_mode(self: &Arc<Self>, mode: RelayMode) -> Result<()> {
        self.db.set_setting(SETTING_RELAY_MODE, mode.as_str().as_bytes())?;
        match mode {
            RelayMode::Builtin => self.start_hosted_relay(),
            RelayMode::External => {
                if let Some(task) = self.hosted_task.lock().unwrap_or_else(|p| p.into_inner()).take() {
                    task.abort();
                }
                *self.hosted_relay.lock().unwrap_or_else(|p| p.into_inner()) = None;
                self.announce_pending.lock().await.clear();
                self.set_hosted_state(HostedRelayState::Off);
            }
        }
        // The open connection to the old relay notices at its next ping that
        // it is no longer the one to collect from, and the loop redials.
        Ok(())
    }

    /// Starts the relay this client hosts for itself, unless it is already
    /// starting or up.
    fn start_hosted_relay(self: &Arc<Self>) {
        let mut slot = self.hosted_task.lock().unwrap_or_else(|p| p.into_inner());
        let busy = slot.as_ref().is_some_and(|t| !t.is_finished());
        let up = matches!(&*self.hosted_state.read().unwrap_or_else(|p| p.into_inner()), HostedRelayState::Ready { .. });
        if busy || up {
            return;
        }
        let this = self.clone();
        *slot = Some(tokio::spawn(async move { this.run_hosted_relay().await }));
    }

    /// Brings the built-in relay up and hands it over. Off the path anything
    /// user-facing waits on: `EphemeralRelay::start` returns once the
    /// destination's tunnels exist, commonly a minute or two.
    async fn run_hosted_relay(self: Arc<Self>) {
        let mut wait = Duration::from_secs(15);
        loop {
            if self.relay_mode() != RelayMode::Builtin {
                self.set_hosted_state(HostedRelayState::Off);
                return;
            }
            self.set_hosted_state(HostedRelayState::Starting);
            eprintln!("[relay-hosted] starting the built-in relay on SAM port {}...", self.node.sam_port());
            match gipny_libcore::EphemeralRelay::start(
                self.node.sam_port(),
                // This relay is our inbox and nobody else's.
                gipny_libcore::MemStoreLimits::personal(self.identity.card().sign_pk),
                Some(dht_client::handler(&self.dht)),
            ).await {
                Ok(relay) => {
                    // The mode may have changed while the tunnels were building.
                    if self.relay_mode() != RelayMode::Builtin {
                        drop(relay);
                        self.set_hosted_state(HostedRelayState::Off);
                        return;
                    }
                    let address = relay.address().to_string();
                    eprintln!("[relay-hosted] built-in relay ready at {}", &address[..address.len().min(16)]);
                    *self.hosted_relay.lock().unwrap_or_else(|p| p.into_inner()) = Some(relay);
                    // Every contact still holds the previous launch's address.
                    // They are told as soon as their relay can be reached; the
                    // send loop keeps trying until each has been.
                    if let Ok(contacts) = self.db.list_contacts() {
                        let mut pending = self.announce_pending.lock().await;
                        pending.extend(contacts.iter().filter(|c| c.trust != TrustLevel::Blocked).map(|c| c.id));
                    }
                    self.set_hosted_state(HostedRelayState::Ready { address: address.clone() });
                    self.send_kick.notify_one();
                    let (dht, db, identity) = (self.dht.clone(), self.db.clone(), self.identity.clone());
                    let this = self.clone();
                    let join = tokio::spawn(async move {
                        dht_client::join(&dht, &db, &identity, &address).await;
                        let _ = this.events.try_send(CoreEvent::DhtJoined { peers: this.dht.peer_count() });
                        this.publish_bundle_to_dht().await;
                        // Anything left for us while we were away.
                        this.collect_from_dht(DHT_COLLECT_DAYS_FIRST).await;
                    });
                    self.tasks.lock().unwrap().push(join);
                    return;
                }
                Err(e) => {
                    eprintln!("[relay-hosted] could not start: {e:?}; retrying in {wait:?}");
                    self.set_hosted_state(HostedRelayState::Failed { reason: format!("{e:?}") });
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(Duration::from_secs(300));
                }
            }
        }
    }

    /// Tells contacts where we collect during this launch. An empty payload:
    /// the receiver reads the relay address off it and drops it.
    ///
    /// A contact is never dropped from this list for being away (2026-09-17,
    /// owner's requirement): an address that changed must not cost people the
    /// contact. So an announcement goes out again and again — over their relay,
    /// or through the network when that relay is silent, opening a session if
    /// there is none — until something arrives *from* them, which is the only
    /// proof they have our address. `announce_sent_at` keeps that from
    /// repeating on every tick of the send loop.
    async fn flush_relay_announcements(self: &Arc<Self>) {
        let ids: Vec<i64> = self.announce_pending.lock().await.iter().copied().collect();
        for id in ids {
            let contact = match self.db.get_contact(id) {
                Ok(Some(c)) if c.trust != TrustLevel::Blocked && c.request_state != RequestState::Incoming => c,
                _ => { self.announce_pending.lock().await.remove(&id); continue; }
            };
            {
                let sent = self.announce_sent_at.lock().await;
                if sent.get(&id).is_some_and(|t| t.elapsed() < ANNOUNCE_REPEAT_EVERY) {
                    continue;
                }
            }
            // Dialing happens in the background; an unreachable relay is tried
            // again on a later tick, and the network carries it meanwhile.
            let Some(out) = self.route_for(&contact).await else { continue };
            if self.ensure_session_for(&contact, &out).await.is_err() { continue; }
            let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
            if self.send_payload_via_relay(&contact, &mut payload, &out).await.is_ok() {
                eprintln!("[relay-hosted] told contact {id} where we collect now (waiting to hear back)");
                self.announce_sent_at.lock().await.insert(id, Instant::now());
            }
        }
    }

    /// Reports a contact whose relay has been silent for a while with mail
    /// waiting, once, and reports it again when the relay answers.
    async fn note_reachability(&self, contact: &gipny_libcore::db::Contact, reachable: bool, has_mail: bool) {
        let Some(relay) = contact.relay_address.as_deref().map(str::trim).filter(|r| !r.is_empty()) else { return };
        if reachable {
            self.relay_down_since.lock().await.remove(relay);
            if self.unreachable_reported.lock().await.remove(&contact.id) {
                let _ = self.events.try_send(CoreEvent::ContactReachability { contact_id: contact.id, unreachable: false });
            }
            return;
        }
        let since = *self.relay_down_since.lock().await.entry(relay.to_string()).or_insert_with(Instant::now);
        if has_mail && since.elapsed() >= CONTACT_UNREACHABLE_AFTER
            && self.unreachable_reported.lock().await.insert(contact.id)
        {
            eprintln!("[relay-client] contact {} has been unreachable for {:?}", contact.id, since.elapsed());
            let _ = self.events.try_send(CoreEvent::ContactReachability { contact_id: contact.id, unreachable: true });
        }
    }

    /// Contacts currently reported as unreachable, for a UI that starts late.
    pub async fn unreachable_contacts(&self) -> Vec<i64> {
        self.unreachable_reported.lock().await.iter().copied().collect()
    }

    fn load_or_create_identity(db: &Db) -> Result<Identity> {
        match (db.get_setting(SETTING_IDENTITY_SIGN)?, db.get_setting(SETTING_IDENTITY_DH)?) {
            (Some(s), Some(d)) if s.len() == 32 && d.len() == 32 => {
                let mut sign = [0u8; 32]; sign.copy_from_slice(&s);
                let mut dh = [0u8; 32]; dh.copy_from_slice(&d);
                Ok(Identity::from_bytes(sign, dh))
            }
            _ => {
                let id = Identity::generate();
                db.set_setting(SETTING_IDENTITY_SIGN, id.sign_seed())?;
                db.set_setting(SETTING_IDENTITY_DH, id.dh_secret())?;
                Ok(id)
            }
        }
    }

    pub fn my_card(&self) -> IdentityCard { self.identity.card() }
    pub fn my_fingerprint(&self) -> [u8; 32] { self.identity.fingerprint() }
    pub fn db(&self) -> &Arc<Db> { &self.db }
    pub fn my_onion(&self) -> &str { self.node.onion_address() }
    pub fn my_b32(&self) -> String { self.node.b32_address().unwrap_or_default() }

    pub fn get_relay_address(&self) -> String { self.relay_onion() }
    pub fn set_relay_address(&self, addr: &str) -> Result<()> {
        self.db.set_setting(SETTING_RELAY_ONION, addr.trim().as_bytes())?;
        Ok(())
    }

    // ----- agent mode ------------------------------------------------------

    /// The master this client runs console commands for: agent mode is on and
    /// that contact still exists.
    pub fn agent_master(&self) -> Option<AgentMaster> {
        let pk = self.db.get_setting(SETTING_AGENT_MASTER).ok().flatten()?;
        let pk: [u8; 32] = pk.as_slice().try_into().ok()?;
        let c = self.db.find_contact_by_sign_pk(&pk).ok().flatten()?;
        Some(AgentMaster { contact_id: c.id, name: c.display_name, sign_pk: to_hex(&c.identity_sign) })
    }

    /// Switches agent mode on for `contact_id`, or off with `None`. On: the
    /// master is told (GRANT), and every command of theirs that arrived while
    /// the mode was off is queued to run now, oldest first.
    pub async fn set_agent_mode(self: &Arc<Self>, contact_id: Option<i64>) -> Result<()> {
        let Some(cid) = contact_id else { return self.disable_agent_mode(true).await; };
        let c = self.db.get_contact(cid)?.ok_or(CoreError::NotFound)?;
        if c.trust == TrustLevel::Blocked { return Err(CoreError::State); }
        if let Some(prev) = self.agent_master() {
            if prev.contact_id != cid {
                let _ = self.send_console(prev.contact_id, BODY_REVOKE.into(), WireConsole::new(CONSOLE_REVOKE), vec![]).await;
            }
        }
        self.db.set_setting(SETTING_AGENT_MASTER, &c.identity_sign)?;
        self.send_console(cid, BODY_GRANT.into(), WireConsole::new(CONSOLE_GRANT), vec![]).await?;
        eprintln!("[agent] agent mode on, master = contact {cid}");
        let _ = self.events.try_send(CoreEvent::AgentModeChanged { master: self.agent_master() });
        self.enqueue_pending_commands(cid);
        Ok(())
    }

    async fn disable_agent_mode(&self, notify_master: bool) -> Result<()> {
        let prev = self.agent_master();
        self.db.delete_setting(SETTING_AGENT_MASTER)?;
        if let (true, Some(m)) = (notify_master, &prev) {
            let _ = self.send_console(m.contact_id, BODY_REVOKE.into(), WireConsole::new(CONSOLE_REVOKE), vec![]).await;
        }
        eprintln!("[agent] agent mode off");
        let _ = self.events.try_send(CoreEvent::AgentModeChanged { master: None });
        Ok(())
    }

    /// Name/trust update that also drops agent mode when the master is blocked.
    pub async fn update_contact(&self, id: i64, name: &str, trust: TrustLevel) -> Result<()> {
        self.db.update_contact(id, name, trust)?;
        if trust == TrustLevel::Blocked && self.agent_master().is_some_and(|m| m.contact_id == id) {
            self.disable_agent_mode(false).await?;
        }
        Ok(())
    }

    fn enqueue_pending_commands(&self, master_id: i64) {
        let ids = self.db.list_setting_ids_with_prefix("console_pending_").unwrap_or_default();
        for id in ids {
            if let Ok(Some(m)) = self.db.get_message(id) {
                if m.contact_id == Some(master_id) && matches!(m.direction, Direction::In) {
                    let _ = self.agent_tx.send(id);
                }
            }
        }
    }

    /// Sends a console-framed message: a command, its output, or a control
    /// marker. The frame rides in `console_<id>` next to the row, as buttons
    /// do, so it is queued, retried and acked like any other message.
    pub async fn send_console(
        &self,
        contact_id: i64,
        body: String,
        console: WireConsole,
        attachments: Vec<PendingAttachment>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let mut stored = Vec::with_capacity(attachments.len());
        for a in &attachments {
            let (key, path, size) = self.store_attachment(&a.data)?;
            stored.push(NewAttachment {
                name: a.name.clone(), size: size as i64, key: key.to_vec(), path,
            });
        }
        let msg_id = self.db.insert_message(
            contact_id, Direction::Out, &body, sent_at, None, &stored,
        )?;
        self.db.set_setting(&format!("console_{}", msg_id), &bincode::serialize(&console)?)?;
        self.send_kick.notify_one();
        Ok(msg_id)
    }

    fn spawn_agent_worker(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<i64>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            while let Some(mid) = rx.recv().await {
                this.run_console_command(mid).await;
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// Runs one queued command. Everything is re-checked at run time, because
    /// the queue outlives mode switches: the pending marker must still be
    /// there, the mode still on, and the message still the master's.
    async fn run_console_command(self: &Arc<Self>, mid: i64) {
        let pending_key = format!("console_pending_{mid}");
        if self.db.get_setting(&pending_key).ok().flatten().is_none() { return; }
        let Some(master) = self.agent_master() else { return; };
        let msg = match self.db.get_message(mid) {
            Ok(Some(m)) if m.contact_id == Some(master.contact_id) && matches!(m.direction, Direction::In) => m,
            _ => { let _ = self.db.delete_setting(&pending_key); return; }
        };
        let _ = self.db.delete_setting(&pending_key);
        let files = self.load_attachment_data(mid).unwrap_or_default();
        eprintln!(
            "[agent] exec from {}: {}{}",
            &master.sign_pk[..16], msg.body,
            if files.is_empty() { String::new() } else { format!(" (+{} files)", files.len()) },
        );
        let reply = agent::handle_console_request(&msg.body, &files, &ExecOptions::default()).await;
        eprintln!(
            "[agent] exit={:?} dur={:?}ms truncated={}",
            reply.console.exit_code, reply.console.duration_ms, reply.console.truncated,
        );
        let atts = reply.attachments.into_iter()
            .map(|(name, data)| PendingAttachment { name, data })
            .collect();
        match self.send_console(master.contact_id, reply.body, reply.console, atts).await {
            Ok(_) => { let _ = self.events.try_send(CoreEvent::ConsoleActivity { contact_id: master.contact_id }); }
            Err(e) => eprintln!("[agent] reply failed: {e}"),
        }
    }

    fn load_attachment_data(&self, msg_id: i64) -> Result<Vec<(String, Vec<u8>)>> {
        let mut out = Vec::new();
        for a in self.db.list_attachments(msg_id)? {
            let key = to_arr32(a.key.clone())?;
            let full = self.data_dir.join(ATTACHMENTS_DIR).join(&a.path);
            let enc = std::fs::read(&full)?;
            let data = AttachmentCipher::from_key(key).decrypt_chunk(0, &[], &enc)?;
            out.push((a.name, data));
        }
        Ok(out)
    }

    /// Delete a contact and purge all associated in-memory state.
    ///
    /// The DB row deletion cascades to sessions/messages/attachments via
    /// `ON DELETE CASCADE`. This method additionally removes the five
    /// in-memory maps that would otherwise hold stale entries forever
    /// (sessions, tiebreaker_waits, session_created_at, incoming_since_send,
    /// and the bundle_waiters entry keyed on the contact's signing key).
    pub async fn delete_contact(self: &Arc<Self>, contact_id: i64) -> Result<()> {
        // Capture identity_sign before the DB row is gone so we can purge
        // the bundle_waiters map (keyed by sign_pk, not by contact id).
        let sign_pk: Option<[u8; 32]> = self.db.get_contact(contact_id)?
            .and_then(|c| c.identity_sign.as_slice().try_into().ok());

        // A master that is gone cannot be told, and a mode with no master is
        // just a stale key.
        if self.agent_master().is_some_and(|m| m.contact_id == contact_id) {
            let _ = self.disable_agent_mode(false).await;
        }

        self.db.delete_contact(contact_id)?;

        // Purge all in-memory state for this contact.
        self.sessions.lock().await.remove(&contact_id);
        self.session_created_at.lock().await.remove(&contact_id);
        self.tiebreaker_waits.lock().await.remove(&contact_id);
        self.incoming_since_send.lock().await.remove(&contact_id);
        if let Some(pk) = sign_pk {
            self.bundle_waiters.lock().await.remove(&pk);
        }
        Ok(())
    }

    pub fn display_name(&self) -> Result<String> {
        let v = self.db.get_setting("display_name")?;
        Ok(v.and_then(|b| String::from_utf8(b).ok()).unwrap_or_default())
    }

    fn outgoing_sender_name(&self) -> Option<String> {
        let user = self.db.get_setting("display_name").ok().flatten()
            .and_then(|b| String::from_utf8(b).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Some(user.unwrap_or_else(|| hex_short(&self.identity.card().sign_pk)))
    }

    fn apply_peer_name(&self, contact_id: i64, name: &str) {
        let contact_changed = self.db.update_contact_name(contact_id, name).unwrap_or(false);
        if contact_changed {
            let _ = self.events.try_send(CoreEvent::ContactUpdated { contact_id });
        }
        if let Ok(Some(c)) = self.db.get_contact(contact_id) {
            if let Ok(groups) = self.db.list_groups_with_member(&c.identity_sign) {
                for gid in groups {
                    if self.db.update_group_member_name(&gid, &c.identity_sign, name).unwrap_or(false) {
                        let _ = self.events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(&gid) });
                    }
                }
            }
        }
    }

    async fn ensure_prekeys(&self) -> Result<()> {
        let id = self.db.get_setting(SETTING_SIGNED_PREKEY_ID)?;
        if id.is_none() {
            let pair = PreKeyPair::generate();
            let pid = self.db.add_prekey(PreKeyKind::Signed, pair.secret(), pair.public())?;
            self.db.set_setting(SETTING_SIGNED_PREKEY_ID, &pid.to_be_bytes())?;
        }
        let count = self.db.count_prekeys(PreKeyKind::OneTime)?;
        for _ in count..(TARGET_OPK as i64) {
            let pair = PreKeyPair::generate();
            self.db.add_prekey(PreKeyKind::OneTime, pair.secret(), pair.public())?;
        }
        Ok(())
    }

    pub fn my_bundle(&self) -> Result<PreKeyBundle> {
        let signed_id = i64::from_be_bytes(
            self.db.get_setting(SETTING_SIGNED_PREKEY_ID)?.ok_or(CoreError::State)?
                .try_into().map_err(|_| CoreError::State)?);
        let signed = self.db.get_prekey(signed_id)?.ok_or(CoreError::State)?;
        let signed_pair = PreKeyPair::from_secret(to_arr32(signed.private.clone())?);
        let opk = self.db.peek_oldest_prekey(PreKeyKind::OneTime)?;
        let opk_pair = opk.as_ref().map(|p| {
            let sk = to_arr32(p.private.clone()).expect("prekey private size");
            (p.id, PreKeyPair::from_secret(sk))
        });
        let bundle = PreKeyBundle::new(
            &self.identity,
            &signed_pair,
            opk_pair.as_ref().map(|(id, kp)| (*id, kp)),
        );
        Ok(bundle)
    }

    /// Accept an introduction: show what they sent, acknowledge it, and tell
    /// them where we collect.
    pub async fn accept_contact_request(self: &Arc<Self>, contact_id: i64) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        if contact.request_state != RequestState::Incoming {
            return Ok(());
        }
        self.accept_request_now(contact_id).await?;
        let _ = self.events.try_send(CoreEvent::ContactUpdated { contact_id });
        Ok(())
    }

    async fn accept_request_now(self: &Arc<Self>, contact_id: i64) -> Result<()> {
        self.db.set_contact_request_state(contact_id, RequestState::None)?;
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        // An ack that finds no relay yet is not lost: the sender retries, and
        // the duplicate is acknowledged on arrival.
        for origin in self.db.incoming_origins(contact_id)? {
            let original = WirePayload::simple(origin as u64, String::new(), Vec::new(), now_ms(), None);
            let _ = self.send_ack(&contact, &original).await;
        }
        // Also what tells them we accepted when there was nothing to ack.
        self.announce_pending.lock().await.insert(contact_id);
        self.send_kick.notify_one();
        Ok(())
    }

    /// Decline an introduction: forget them and whatever they sent. They can
    /// ask again; blocking is what keeps them out.
    pub async fn decline_contact_request(self: &Arc<Self>, contact_id: i64) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        if contact.request_state != RequestState::Incoming {
            return Err(CoreError::State);
        }
        self.delete_contact(contact_id).await
    }

    /// Add a contact, recording the relay their card named.
    ///
    /// `relay` is where *they* receive: messages to this contact are deposited
    /// there, not on whatever relay this client happens to use. `None` keeps the
    /// old behaviour of falling back to this client's configured relay.
    pub async fn add_contact_via(
        self: &Arc<Self>, card: &gipny_libcore::crypto::IdentityCard, onion: &str, name: &str,
        relay: Option<&str>,
    ) -> Result<i64> {
        let known = self.db.find_contact_by_sign_pk(&card.sign_pk)?;
        let id = self.db.add_contact(&card.sign_pk, &card.dh_pk, onion, name, relay)?;
        match known.map(|c| c.request_state) {
            // A fresh card: introduce ourselves right away, so they see us
            // without waiting for a first message.
            None => {
                self.db.set_contact_request_state(id, RequestState::Outgoing)?;
                // Nobody on their side knows us yet to open the session, so do
                // not sit out the tiebreaker waiting for them.
                self.tiebreaker_waits.lock().await.insert(id, now_ms() - TIEBREAKER_TIMEOUT_MS);
            }
            // Adding the card of someone who asked is accepting them.
            Some(RequestState::Incoming) => {
                self.accept_request_now(id).await?;
            }
            Some(_) => {}
        }
        let _ = self.events.try_send(CoreEvent::ContactAdded { contact_id: id });
        self.send_kick.notify_one();
        Ok(id)
    }

    pub async fn send_message(
        &self,
        contact_id: i64,
        body: String,
        attachments: Vec<PendingAttachment>,
        ttl: Option<Duration>,
        reply_to: Option<i64>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let expires_at = ttl.map(|d| sent_at + d.as_millis() as i64);
        let mut stored = Vec::with_capacity(attachments.len());
        for a in &attachments {
            let (key, path, size) = self.store_attachment(&a.data)?;
            stored.push(NewAttachment {
                name: a.name.clone(), size: size as i64, key: key.to_vec(), path,
            });
        }
        let msg_id = self.db.insert_message(
            contact_id, Direction::Out, &body, sent_at, expires_at, &stored,
        )?;
        if let Some(rt) = reply_to {
            self.db.set_reply_to(msg_id, Some(rt))?;
        }
        self.send_kick.notify_one();
        Ok(msg_id)
    }

    fn build_wire_reply(&self, local_id: i64) -> Result<Option<WireReply>> {
        let m = match self.db.get_message(local_id)? {
            Some(m) => m,
            None => return Ok(None),
        };
        match m.direction {
            Direction::Out => Ok(Some(WireReply {
                sender_sign_pk: self.identity.card().sign_pk.to_vec(),
                origin_msg_id: m.id as u64,
            })),
            Direction::In => {
                let sign_pk = match m.sender_sign_pk.clone() {
                    Some(s) => s,
                    None => {
                        let cid = m.contact_id.ok_or(CoreError::State)?;
                        self.db.get_contact(cid)?.ok_or(CoreError::NotFound)?.identity_sign
                    }
                };
                let origin = self.db.message_origin(m.id)?.unwrap_or(m.id);
                Ok(Some(WireReply { sender_sign_pk: sign_pk, origin_msg_id: origin as u64 }))
            }
        }
    }

    pub async fn press_button(self: &Arc<Self>, contact_id: i64, message_id: i64, callback_data: String) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        let origin_msg_id = self.db.message_origin(message_id)?.unwrap_or(message_id);
        let mut payload = WirePayload {
            origin_msg_id: origin_msg_id as u64,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons: None,
            callback_data: Some(callback_data),
            edit_of: None,
            pin: None,
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
        };
        let out = self.route_for(&contact).await.ok_or(CoreError::State)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, &mut payload, &out).await
    }

    pub async fn press_group_button(self: &Arc<Self>, group_id: &[u8], message_id: i64, callback_data: String) -> Result<()> {
        let msg = self.db.get_message(message_id)?.ok_or(CoreError::NotFound)?;
        if msg.group_id.as_deref() != Some(group_id) { return Err(CoreError::State); }
        let sender_sign = msg.sender_sign_pk.clone().ok_or(CoreError::State)?;
        let contact = self.db.find_contact_by_sign_pk(&sender_sign)?.ok_or(CoreError::NotFound)?;
        let origin_msg_id = self.db.message_origin(message_id)?.unwrap_or(message_id);

        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(),
            dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(),
            name: m.display_name.clone(),
        }).collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef { id: group_id.to_vec(), name: gname, members: gref_members };

        let mut payload = WirePayload {
            origin_msg_id: origin_msg_id as u64,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: Some(gref),
            buttons: None,
            callback_data: Some(callback_data),
            edit_of: None,
            pin: None,
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
        };
        let out = self.route_for(&contact).await.ok_or(CoreError::State)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, &mut payload, &out).await
    }

    pub async fn send_edit(self: &Arc<Self>, contact_id: i64, message_id: i64, new_body: String) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        let msg = self.db.get_message(message_id)?.ok_or(CoreError::NotFound)?;
        if !matches!(msg.direction, Direction::Out) || msg.contact_id != Some(contact_id) {
            return Err(CoreError::State);
        }
        self.db.update_message_body(message_id, &new_body)?;
        let buttons: Option<Vec<Vec<WireButton>>> = self.db.get_setting(&format!("buttons_{}", message_id))
            .ok().flatten()
            .and_then(|b| bincode::deserialize(&b).ok());
        let _ = self.events.try_send(CoreEvent::MessageEdited {
            message_id, body: new_body.clone(), buttons: buttons.clone(),
        });
        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: new_body,
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons,
            callback_data: None,
            edit_of: Some(message_id as u64),
            pin: None,
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
        };
        let out = self.route_for(&contact).await.ok_or(CoreError::State)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, &mut payload, &out).await
    }

    pub async fn send_edit_group(self: &Arc<Self>, group_id: &[u8], message_id: i64, new_body: String) -> Result<()> {
        let msg = self.db.get_message(message_id)?.ok_or(CoreError::NotFound)?;
        if !matches!(msg.direction, Direction::Out) || msg.group_id.as_deref() != Some(group_id) {
            return Err(CoreError::State);
        }
        self.db.update_message_body(message_id, &new_body)?;
        let buttons: Option<Vec<Vec<WireButton>>> = self.db.get_setting(&format!("buttons_{}", message_id))
            .ok().flatten()
            .and_then(|b| bincode::deserialize(&b).ok());
        let _ = self.events.try_send(CoreEvent::MessageEdited {
            message_id, body: new_body.clone(), buttons: buttons.clone(),
        });

        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(),
            dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(),
            name: m.display_name.clone(),
        }).collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef { id: group_id.to_vec(), name: gname, members: gref_members };
        for m in members {
            if m.is_self { continue; }
            let contact = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if contact.trust == TrustLevel::Blocked { continue; }
            let mut payload = WirePayload {
                origin_msg_id: 0,
                body: new_body.clone(),
                attachments: vec![],
                sent_at: now_ms(),
                ttl_ms: None,
                group: Some(gref.clone()),
                buttons: buttons.clone(),
                callback_data: None,
                edit_of: Some(message_id as u64),
                pin: None,
                ack_for: None,
                sender_name: None,
                reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
            };
            let _ = self.send_to_contact(contact.id, &mut payload).await;
        }
        Ok(())
    }

    pub async fn pin_contact_message(self: &Arc<Self>, contact_id: i64, message_id: i64, unpin: bool) -> Result<()> {
        let msg = self.db.get_message(message_id)?.ok_or(CoreError::NotFound)?;
        if msg.contact_id != Some(contact_id) { return Err(CoreError::State); }

        let (sender_sign_pk, origin) = match msg.direction {
            Direction::Out => (self.identity.card().sign_pk.to_vec(), message_id as u64),
            Direction::In => {
                let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
                let origin = self.db.message_origin(message_id)?.unwrap_or(message_id);
                (contact.identity_sign, origin as u64)
            }
        };

        if unpin { self.db.unpin_contact_message(contact_id, message_id)?; }
        else { self.db.pin_contact_message(contact_id, message_id)?; }

        let ev = if unpin {
            CoreEvent::MessageUnpinned { contact_id: Some(contact_id), group_id: None, message_id }
        } else {
            CoreEvent::MessagePinned { contact_id: Some(contact_id), group_id: None, message_id }
        };
        let _ = self.events.try_send(ev);

        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons: None,
            callback_data: None,
            edit_of: None,
            pin: Some(WirePin { sender_sign_pk, origin_msg_id: origin, unpin }),
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
        };
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        let out = self.route_for(&contact).await.ok_or(CoreError::State)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, &mut payload, &out).await
    }

    pub async fn pin_group_message(self: &Arc<Self>, group_id: &[u8], message_id: i64, unpin: bool) -> Result<()> {
        let msg = self.db.get_message(message_id)?.ok_or(CoreError::NotFound)?;
        if msg.group_id.as_deref() != Some(group_id) { return Err(CoreError::State); }

        let (sender_sign_pk, origin) = match msg.sender_sign_pk.clone() {
            Some(pk) => {
                let origin = self.db.message_origin(message_id)?.unwrap_or(message_id);
                (pk, origin as u64)
            }
            None if matches!(msg.direction, Direction::Out) => {
                (self.identity.card().sign_pk.to_vec(), message_id as u64)
            }
            _ => return Err(CoreError::State),
        };

        if unpin { self.db.unpin_group_message(group_id, message_id)?; }
        else { self.db.pin_group_message(group_id, message_id)?; }

        let ev = if unpin {
            CoreEvent::MessageUnpinned { contact_id: None, group_id: Some(to_hex(group_id)), message_id }
        } else {
            CoreEvent::MessagePinned { contact_id: None, group_id: Some(to_hex(group_id)), message_id }
        };
        let _ = self.events.try_send(ev);

        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(),
            dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(),
            name: m.display_name.clone(),
        }).collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef { id: group_id.to_vec(), name: gname, members: gref_members };
        let wire = WirePin { sender_sign_pk, origin_msg_id: origin, unpin };
        for m in members {
            if m.is_self { continue; }
            let contact = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if contact.trust == TrustLevel::Blocked { continue; }
            let mut payload = WirePayload {
                origin_msg_id: 0,
                body: String::new(),
                attachments: vec![],
                sent_at: now_ms(),
                ttl_ms: None,
                group: Some(gref.clone()),
                buttons: None,
                callback_data: None,
                edit_of: None,
                pin: Some(wire.clone()),
                ack_for: None,
                sender_name: None,
                reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
            };
            let _ = self.send_to_contact(contact.id, &mut payload).await;
        }
        Ok(())
    }

    /// Whether a found update installs by itself.
    ///
    /// Always yes, by the owner's decision (2026-09-17): a messenger whose
    /// security fixes wait for someone to press a button is only as safe as
    /// the least attentive install, and the old switch made "I turned it off
    /// once" a permanent state. `GIPNY_NO_AUTO_UPDATE=1` is the developer's
    /// escape hatch — an environment variable, not a setting, so it cannot be
    /// left behind in a profile.
    pub fn auto_update_enabled(&self) -> bool {
        if std::env::var_os("GIPNY_NO_AUTO_UPDATE").is_some() {
            return false;
        }
        let _ = SETTING_AUTO_UPDATE; // kept for older profiles; no longer read
        true
    }

    /// The interface's own data under `key` (see `UI_DATA_KEYS`), kept in the
    /// vault so it comes back with the profile. `None` when never saved.
    pub fn ui_data(&self, key: &str) -> Result<Option<String>> {
        if !UI_DATA_KEYS.contains(&key) {
            return Err(CoreError::NotFound);
        }
        Ok(self.db.get_setting(&format!("ui_{key}"))?.and_then(|v| String::from_utf8(v).ok()))
    }

    pub fn set_ui_data(&self, key: &str, json: &str) -> Result<()> {
        if !UI_DATA_KEYS.contains(&key) {
            return Err(CoreError::NotFound);
        }
        if json.len() > MAX_UI_DATA_BYTES {
            return Err(CoreError::Db(gipny_libcore::db::DbError::TooLarge));
        }
        self.db.set_setting(&format!("ui_{key}"), json.as_bytes())?;
        Ok(())
    }

    /// Kept so an older interface does not fail; updates install either way.
    pub fn set_auto_update(&self, enabled: bool) -> Result<()> {
        self.db.set_setting(SETTING_AUTO_UPDATE, if enabled { b"1" } else { b"0" })?;
        Ok(())
    }

    /// Checks GitHub for a newer release. With auto-update on (the default)
    /// and a matching asset for this platform, this also downloads and
    /// installs it — silently, without waiting for anything to be clicked —
    /// before returning; the caller only ever needs to act on the result when
    /// auto-update is off.
    pub async fn check_and_emit_update(self: Arc<Self>) -> Result<Option<UpdateInfo>> {
        let info = match self.updater.check(env!("CARGO_PKG_VERSION")).await {
            Ok(Some(i)) => i,
            Ok(None) => return Ok(None),
            Err(e) => {
                eprintln!("[update] check err: {:?}", e);
                return Err(e.into());
            }
        };
        if let Ok(Some(v)) = self.db.get_setting(SETTING_DISMISSED_UPDATE) {
            if String::from_utf8_lossy(&v) == info.version {
                return Ok(None);
            }
        }
        *self.pending_update.lock().await = Some(info.clone());
        // Two places where installing needs a person: Android (putting an APK
        // in place is the system installer's job, and a file in our private
        // directory is not something anyone can tap) and a .deb install, where
        // the package manager asks for the administrator password. A password
        // dialog appearing by itself, with no context, is not an update — it is
        // something people rightly refuse. Both get a notice and a button.
        let needs_a_person = cfg!(target_os = "android") || gipny_libcore::update::is_deb_install();
        if self.auto_update_enabled() && !needs_a_person {
            let this = self.clone();
            tokio::spawn(async move {
                if let Err(e) = this.install_update().await {
                    eprintln!("[update] auto-install failed: {e:?}");
                }
            });
        } else {
            let _ = self.events.try_send(CoreEvent::UpdateAvailable {
                version: info.version.clone(),
                notes: info.notes.clone(),
                size: info.asset.size,
            });
        }
        Ok(Some(info))
    }

    /// Downloads and installs the pending update (from `check_and_emit_update`
    /// or a manual "Update now" click), then marks that version dismissed so
    /// it is not re-installed on every later check within the same launch.
    pub async fn install_update(self: Arc<Self>) -> Result<()> {
        let info = self.pending_update.lock().await.clone().ok_or(CoreError::NotFound)?;
        let ev = self.events.clone();
        let ev_dl = ev.clone();
        let dl_dir = self.data_dir.join("update_dl");

        let path = match self.updater.download(&info, &dl_dir, move |done, t| {
            let pct = if t > 0 { ((done * 100) / t).min(100) as u8 } else { 0 };
            let _ = ev_dl.try_send(CoreEvent::UpdateProgress { downloaded: done, total: t, pct });
        }).await {
            Ok(p) => p,
            Err(e) => {
                let _ = ev.send(CoreEvent::UpdateFailed { reason: e.to_string() }).await;
                return Err(e.into());
            }
        };

        let outcome = match self.updater.install(&path, &self.data_dir, None) {
            Ok(o) => o,
            Err(e) => {
                let _ = ev.send(CoreEvent::UpdateFailed { reason: e.to_string() }).await;
                return Err(e.into());
            }
        };
        match outcome {
            InstallOutcome::InstalledNow | InstallOutcome::StagedForNextLaunch => {
                let _ = std::fs::remove_dir_all(&dl_dir);
                self.dismiss_update(info.version.clone()).await?;
                let _ = ev.send(CoreEvent::UpdateStaged { version: info.version }).await;
            }
            InstallOutcome::Unsupported(msg) => {
                self.dismiss_update(info.version.clone()).await?;
                let _ = ev.send(CoreEvent::UpdateReady { path: msg }).await;
            }
        }
        Ok(())
    }

    pub async fn dismiss_update(&self, version: String) -> Result<()> {
        self.db.set_setting(SETTING_DISMISSED_UPDATE, version.as_bytes())?;
        *self.pending_update.lock().await = None;
        Ok(())
    }

    pub async fn list_apk_artifacts(&self) -> Result<(String, Vec<(String, u64)>)> {
        let release = self.updater.latest_release().await?;
        let mut out = Vec::new();
        for asset in &release.assets {
            if let Some(rest) = asset.name.strip_prefix("gipny-i2p_") {
                if let Some(arch) = rest.split("_android-").nth(1).and_then(|s| s.strip_suffix(".apk")) {
                    out.push((arch.to_string(), asset.size));
                }
            }
        }
        out.sort();
        Ok((release.version, out))
    }

    pub async fn download_apk(self: Arc<Self>, arch: String, dest_path: String) -> Result<()> {
        let release = self.updater.latest_release().await?;
        let suffix = format!("_android-{arch}.apk");
        let asset = release.assets.iter().find(|a| a.name.ends_with(&suffix))
            .ok_or(CoreError::NotFound)?
            .clone();
        let ev = self.events.clone();
        let ev_dl = ev.clone();
        let dest = std::path::PathBuf::from(dest_path);
        match self.updater.download_asset_to(&asset, None, &dest, move |done, t| {
            let pct = if t > 0 { ((done * 100) / t).min(100) as u8 } else { 0 };
            let _ = ev_dl.try_send(CoreEvent::UpdateProgress { downloaded: done, total: t, pct });
        }).await {
            Ok(_) => {
                let _ = ev.send(CoreEvent::UpdateReady { path: dest.display().to_string() }).await;
                Ok(())
            }
            Err(e) => {
                let _ = ev.send(CoreEvent::UpdateFailed { reason: e.to_string() }).await;
                Err(e.into())
            }
        }
    }

    pub async fn create_group(self: &Arc<Self>, name: &str, member_contact_ids: &[i64]) -> Result<Vec<u8>> {
        let mut gid = vec![0u8; 32];
        fill_random(&mut gid);
        self.db.create_group(&gid, name)?;
        self.db.add_group_member(&gid, &self.identity.card().sign_pk, &self.identity.card().dh_pk,
            self.node.onion_address(), &self.display_name().unwrap_or_default(), true)?;
        let mut members_wire = Vec::new();
        members_wire.push(WireMember {
            sign_pk: self.identity.card().sign_pk.to_vec(),
            dh_pk: self.identity.card().dh_pk.to_vec(),
            onion: self.node.onion_address().to_string(),
            name: self.display_name().unwrap_or_default(),
        });
        for cid in member_contact_ids {
            let c = self.db.get_contact(*cid)?.ok_or(CoreError::NotFound)?;
            self.db.add_group_member(&gid, &c.identity_sign, &c.identity_dh, &c.onion_address, &c.display_name, false)?;
            members_wire.push(WireMember {
                sign_pk: c.identity_sign.clone(),
                dh_pk: c.identity_dh.clone(),
                onion: c.onion_address.clone(),
                name: c.display_name.clone(),
            });
        }
        let gref = WireGroupRef { id: gid.clone(), name: name.to_string(), members: members_wire };
        for cid in member_contact_ids {
            let sent_at = now_ms();
            let mut payload = WirePayload::simple(0, String::new(), Vec::new(), sent_at, None);
            payload.group = Some(gref.clone());
            self.send_to_contact(*cid, &mut payload).await?;
        }
        let _ = self.events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(&gid) });
        Ok(gid)
    }

    pub async fn add_group_member(self: &Arc<Self>, group_id: &[u8], contact_id: i64) -> Result<()> {
        let group_name = self.db.get_group_name(group_id)?.ok_or(CoreError::NotFound)?;
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        if self.db.is_group_member(group_id, &contact.identity_sign)? {
            return Ok(());
        }
        self.db.add_group_member(
            group_id,
            &contact.identity_sign,
            &contact.identity_dh,
            &contact.onion_address,
            &contact.display_name,
            false,
        )?;
        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(),
            dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(),
            name: m.display_name.clone(),
        }).collect();
        let gref = WireGroupRef { id: group_id.to_vec(), name: group_name, members: gref_members };
        for m in members {
            if m.is_self { continue; }
            let target = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if target.trust == TrustLevel::Blocked { continue; }
            let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
            payload.group = Some(gref.clone());
            let _ = self.send_to_contact(target.id, &mut payload).await;
        }
        let _ = self.events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(group_id) });
        Ok(())
    }

    pub async fn send_to_group(
        self: &Arc<Self>,
        group_id: &[u8],
        body: String,
        attachments: Vec<PendingAttachment>,
        _ttl: Option<Duration>,
        reply_to: Option<i64>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let expires_at: Option<i64> = None;
        let ttl: Option<Duration> = None;
        let mut stored = Vec::with_capacity(attachments.len());
        for a in &attachments {
            let (key, path, size) = self.store_attachment(&a.data)?;
            stored.push(NewAttachment {
                name: a.name.clone(), size: size as i64, key: key.to_vec(), path,
            });
        }
        let msg_id = self.db.insert_group_message(
            group_id, Some(&self.identity.card().sign_pk), Direction::Out,
            &body, sent_at, expires_at, &stored,
        )?;
        if let Some(rt) = reply_to {
            self.db.set_reply_to(msg_id, Some(rt))?;
        }
        let wire_reply = match reply_to {
            Some(rt) => self.build_wire_reply(rt)?,
            None => None,
        };
        let wire_atts: Vec<WireAttachment> = attachments.into_iter()
            .map(|a| WireAttachment { name: a.name, data: a.data }).collect();
        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(),
            dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(),
            name: m.display_name.clone(),
        }).collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef { id: group_id.to_vec(), name: gname, members: gref_members };
        for m in members {
            if m.is_self { continue; }
            let contact = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if contact.trust == TrustLevel::Blocked { continue; }
            self.db.pending_outbound_add(msg_id, contact.id)?;
            let mut payload = WirePayload::simple(
                msg_id as u64, body.clone(), wire_atts.clone(), sent_at,
                ttl.map(|d| d.as_millis() as i64),
            );
            payload.group = Some(gref.clone());
            payload.reply_to = wire_reply.clone();
            match self.send_to_contact(contact.id, &mut payload).await {
                Ok(()) => { let _ = self.db.pending_outbound_remove(msg_id, contact.id); }
                Err(e) => eprintln!("[relay-client] group send to contact {} failed (msg {}): {:?} — kept in pending_outbound", contact.id, msg_id, e),
            }
        }
        Ok(msg_id)
    }

    fn store_attachment(&self, data: &[u8]) -> Result<([u8; 32], String, u64)> {
        let cipher = AttachmentCipher::generate();
        let encrypted = cipher.encrypt_chunk(0, &[], data)?;
        let mut name = [0u8; 24];
        fill_random(&mut name);
        let hex = to_hex(&name);
        let dir = self.data_dir.join(ATTACHMENTS_DIR);
        let path = dir.join(&hex);
        std::fs::write(&path, &encrypted)?;
        Ok((*cipher.key(), hex, data.len() as u64))
    }

    pub fn read_attachment(&self, att: &Attachment) -> Result<Vec<u8>> {
        let key = to_arr32(att.key.clone())?;
        let full = self.data_dir.join(ATTACHMENTS_DIR).join(&att.path);
        let enc = std::fs::read(&full)?;
        let pt = AttachmentCipher::from_key(key).decrypt_chunk(0, &[], &enc)?;
        Ok(pt)
    }

    fn spawn_relay_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut backoff = RECONNECT_INITIAL_MS;
            loop {
                let onion = this.relay_onion();
                if onion.is_empty() {
                    // No relay configured yet (i2p: DEFAULT_RELAY not baked in and
                    // none set in Settings). Wait quietly instead of hammering.
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                    backoff = (backoff * 2).min(RECONNECT_MAX_MS);
                    continue;
                }
                eprintln!("[relay-client] connecting to {}", &onion[..16.min(onion.len())]);
                match relay::connect(&this.node, &onion, &this.identity).await {
                    Ok(client) => {
                        eprintln!("[relay-client] connected & authed");
                        backoff = RECONNECT_INITIAL_MS;
                        *this.relay_out.write().await = Some(client.out_tx.clone());
                        let _ = this.events.try_send(CoreEvent::RelayConnected);

                        if let Ok(bundle) = this.my_bundle() {
                            if let Ok(bytes) = bincode::serialize(&bundle) {
                                let _ = client.out_tx.send(ClientToRelay::Publish { bundle: bytes }).await;
                            }
                        }
                        this.send_kick.notify_one();
                        this.clone().run_recv_loop(client, Some(onion.clone())).await;
                        *this.relay_out.write().await = None;
                        let _ = this.events.try_send(CoreEvent::RelayDisconnected);
                    }
                    Err(e) => eprintln!("[relay-client] connect fail: {:?}", e),
                }
                tokio::time::sleep(Duration::from_millis(backoff)).await;
                backoff = (backoff * 2).min(RECONNECT_MAX_MS);
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// The connection to deposit this contact's mail on.
    ///
    /// Their card names where they collect; ours is only the fallback for
    /// contacts added before cards carried a relay. Connections to other
    /// people's relays are opened on demand and kept for reuse — a contact is
    /// written to repeatedly, and i2p charges tunnel setup for every new
    /// destination.
    ///
    /// Returns a boxed future on purpose: this calls into the spawned receive
    /// loop, which handles a frame, which may send an ack, which comes back
    /// here. `async fn` would make that an infinitely recursive future type and
    /// the compiler cannot prove it `Send`.
    /// Where a letter goes out: straight to the contact's relay, or into the
    /// relay network when that relay is not answering. The network path is
    /// slower and needs proof of work, so it is the fallback, never the
    /// default.
    async fn route_for(self: &Arc<Self>, contact: &gipny_libcore::db::Contact) -> Option<Route> {
        if let Some(tx) = self.relay_for(contact).await {
            return Some(Route::Relay(tx));
        }
        // Their relay is down or unknown. The network holds the letter until
        // they come back, but only if we are in it.
        (self.dht.peer_count() > 0).then_some(Route::Dht)
    }

    /// Hand one envelope to the contact by `route`.
    async fn deliver(
        &self,
        contact: &gipny_libcore::db::Contact,
        blob: Vec<u8>,
        route: &Route,
        first_letter: bool,
    ) -> Result<()> {
        let (their_sign, their_dh) = (to_arr32(contact.identity_sign.clone())?, to_arr32(contact.identity_dh.clone())?);
        match route {
            Route::Relay(out) => out.send(ClientToRelay::Send { to: their_sign, blob }).await.map_err(|_| CoreError::State),
            Route::Dht => {
                // A first letter goes to the box anyone holding their card can
                // find (they may not know us yet); everything else to the one
                // only the two of us can compute.
                let ok = if first_letter {
                    dht_client::put_intro(&self.dht, &their_sign, &their_dh, &blob).await
                } else {
                    dht_client::put_mail(&self.dht, &self.identity, &their_sign, &their_dh, &blob).await
                };
                if ok {
                    eprintln!("[dht] letter for contact {} left in the network", contact.id);
                    Ok(())
                } else {
                    Err(CoreError::State)
                }
            }
        }
    }

    fn relay_for<'a>(
        self: &'a Arc<Self>,
        contact: &'a gipny_libcore::db::Contact,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<mpsc::Sender<ClientToRelay>>> + Send + 'a>> {
        Box::pin(async move {
        let theirs = contact.relay_address.as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty());
        let Some(theirs) = theirs else {
            return self.relay_out.read().await.clone();
        };
        if theirs == self.relay_onion() {
            return self.relay_out.read().await.clone();
        }

        // Never connect on this path. It runs inside the send loop, which walks
        // contacts one at a time, and opening an i2p destination means building
        // tunnels — seconds when it works and unbounded when the relay is gone,
        // since nothing in the dial path has a timeout. Blocking here would stall
        // delivery to *every other contact* behind one unreachable relay.
        //
        // So: hand back a connection if we have one, otherwise start one in the
        // background and skip this contact for now. The send loop comes round
        // every few seconds and the message is still queued.
        {
            let mut pool = self.peer_relays.lock().await;
            match pool.get(theirs) {
                Some(PeerRelay::Ready(tx)) if !tx.is_closed() => return Some(tx.clone()),
                // A dead sender means the recv loop is on its way out; let it
                // finish cleaning up rather than racing a second connection.
                Some(PeerRelay::Ready(_)) | Some(PeerRelay::Connecting) => return None,
                Some(PeerRelay::Failed { until }) if Instant::now() < *until => return None,
                _ => {}
            }
            pool.insert(theirs.to_string(), PeerRelay::Connecting);
        }

        let this = self.clone();
        let key = theirs.to_string();
        let handle = tokio::spawn(async move {
            let short = &key[..16.min(key.len())];
            let dial = tokio::time::timeout(
                PEER_RELAY_CONNECT_TIMEOUT,
                relay::connect_peer(&this.node, &key, &this.identity),
            ).await;
            let client = match dial {
                Ok(Ok(c)) => c,
                other => {
                    match other {
                        Err(_) => eprintln!("[relay-client] peer relay {short} did not answer in {:?}",
                            PEER_RELAY_CONNECT_TIMEOUT),
                        Ok(Err(e)) => eprintln!("[relay-client] peer relay {short} unreachable: {e:?}"),
                        Ok(Ok(_)) => unreachable!(),
                    }
                    // Back off rather than redialing on every send tick: a relay
                    // that is down stays down for a while, and each attempt costs
                    // tunnel building.
                    this.peer_relays.lock().await.insert(
                        key.clone(),
                        PeerRelay::Failed { until: Instant::now() + PEER_RELAY_RETRY_BACKOFF },
                    );
                    return;
                }
            };
            eprintln!("[relay-client] connected to peer relay {short}");
            this.peer_relays.lock().await
                .insert(key.clone(), PeerRelay::Ready(client.out_tx.clone()));
            this.send_kick.notify_one();

            // Drain it like our own: a relay we deposit on may also be holding
            // mail for us, and the frame handling is identical. No
            // RelayConnected event — that state is about our own relay, and
            // flipping it here would tell the user the wrong thing.
            this.clone().run_recv_loop(client, None).await;
            this.peer_relays.lock().await.remove(&key);
            eprintln!("[relay-client] peer relay {short} disconnected");
        });
        self.tasks.lock().unwrap().push(handle);
        None
        })
    }

    /// `own` is the address this connection was opened as *our* relay under.
    /// When the relay we collect from changes — another mode, another address
    /// in Settings — the connection ends at its next ping and the loop redials;
    /// before, a new address only took effect when the old relay went away.
    async fn run_recv_loop(self: Arc<Self>, client: RelayClient, own: Option<String>) {
        let in_rx = client.in_rx.clone();
        let out_tx = client.out_tx.clone();
        let mut ping = tokio::time::interval(Duration::from_secs(PING_INTERVAL_SECS));
        ping.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ping.tick().await;
        let dead_threshold = Duration::from_secs(DEAD_THRESHOLD_SECS);
        let mut last_activity = std::time::Instant::now();
        loop {
            tokio::select! {
                _ = ping.tick() => {
                    if own.as_ref().is_some_and(|o| *o != self.relay_onion()) {
                        eprintln!("[relay-client] our relay changed, reconnecting");
                        break;
                    }
                    if last_activity.elapsed() > dead_threshold {
                        eprintln!("[relay-client] no activity for {:?}, forcing reconnect", last_activity.elapsed());
                        break;
                    }
                    if out_tx.send(ClientToRelay::Ping).await.is_err() { break; }
                }
                frame = async { in_rx.lock().await.recv().await } => {
                    let Some(frame) = frame else { break; };
                    last_activity = std::time::Instant::now();
                    // Logged in with plain Auth after an AuthV2 attempt fell
                    // through: this relay would take our deposits and never
                    // hand us our mail. Reconnect, which tries AuthV2 again.
                    if own.is_some() && matches!(&frame, RelayToClient::Error(e) if e == relay::ERR_NEEDS_AUTH_V2) {
                        eprintln!("[relay-client] our relay wants AuthV2, reconnecting");
                        break;
                    }
                    if let Err(e) = self.handle_relay_frame(frame, &out_tx).await {
                        eprintln!("[relay-client] handle err: {:?}", e);
                    }
                }
            }
        }
    }

    async fn handle_relay_frame(
        self: &Arc<Self>,
        frame: RelayToClient,
        out_tx: &mpsc::Sender<ClientToRelay>,
    ) -> Result<()> {
        match frame {
            RelayToClient::Incoming { id, from, blob } => {
                match self.handle_incoming_envelope(&from, &blob).await {
                    Ok(()) => { let _ = out_tx.send(ClientToRelay::Ack { id }).await; }
                    Err(CoreError::Codec) => {
                        eprintln!("[relay-client] codec err on msg {}, NOT acking (will retry)", id);
                    }
                    Err(CoreError::StaleOpk) => {
                        eprintln!("[relay-client] stale-OPK X3dhInit on msg {}, ACK and skip (zombie)", id);
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                    Err(CoreError::SealedDrop) => {
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                    Err(CoreError::Crypto(_)) => {
                        if let Ok(Some(c)) = self.db.find_contact_by_sign_pk(&from) {
                            let fresh = {
                                let m = self.session_created_at.lock().await;
                                m.get(&c.id).map(|t| now_ms() - *t < FRESH_SESSION_GRACE_MS).unwrap_or(false)
                            };
                            if fresh {
                                eprintln!("[relay-client] crypto err on msg {} within fresh-session grace, ACK and skip", id);
                                let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                            } else {
                                eprintln!("[relay-client] crypto err on msg {}, requesting resync, NOT acking", id);
                                let _ = self.request_resync(&c).await;
                            }
                        } else {
                            eprintln!("[relay-client] crypto err on msg {}, no contact, NOT acking", id);
                        }
                    }
                    Err(e) => {
                        eprintln!("[relay-client] incoming err: {:?}", e);
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                }
            }
            RelayToClient::Bundle { pk, bundle } => {
                let mut w = self.bundle_waiters.lock().await;
                if let Some(vec) = w.remove(&pk) {
                    for tx in vec { let _ = tx.send(bundle.clone()); }
                }
            }
            RelayToClient::Error(reason) => {
                // Dropped silently before. The usual cause now: a deposit for a
                // contact whose card names a relay built into someone else's
                // app, which holds mail for its owner only.
                eprintln!("[relay-client] relay error: {reason}");
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_incoming_envelope(self: &Arc<Self>, from_pk: &[u8; 32], blob: &[u8]) -> Result<()> {
        let envelope: EnvelopeBlob = bincode::deserialize(blob)?;
        eprintln!("[recv] {} envelope, {} bytes",
            match &envelope { EnvelopeBlob::X3dhInit(_) => "x3dh", EnvelopeBlob::Ratchet { .. } => "ratchet" },
            blob.len());
        let sealed = from_pk == &[0u8; 32];
        match envelope {
            EnvelopeBlob::X3dhInit(init) => {
                let sender_sign = init.identity.sign_pk;
                let sender_dh = init.identity.dh_pk;
                let contact = match self.db.find_contact_by_sign_pk(&sender_sign)? {
                    Some(c) => c,
                    None => {
                        let name = hex_short(&sender_sign);
                        let id = self.db.add_contact(&sender_sign, &sender_dh, "", &name, None)?;
                        self.db.set_contact_request_state(id, RequestState::Incoming)?;
                        let requests = self.db.list_incoming_requests()?;
                        for &old in requests.iter().take(requests.len().saturating_sub(MAX_INCOMING_REQUESTS)) {
                            eprintln!("[relay-client] too many open requests, dropping contact {old}");
                            let _ = self.delete_contact(old).await;
                        }
                        let _ = self.events.try_send(CoreEvent::ContactRequest { contact_id: id });
                        self.db.get_contact(id)?.ok_or(CoreError::NotFound)?
                    }
                };
                if contact.trust == TrustLevel::Blocked { return Ok(()); }
                if init.identity.sign_pk != contact.identity_sign.as_slice()
                    || init.identity.dh_pk != contact.identity_dh.as_slice()
                {
                    return Err(CoreError::State);
                }
                let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
                self.sessions.lock().await.remove(&contact.id);
                let _ = self.db.delete_session(contact.id);
                self.tiebreaker_waits.lock().await.remove(&contact.id);
                let (state, plaintext) = self.accept_x3dh(&init, &ad).await?;
                self.sessions.lock().await.insert(contact.id, state);
                self.session_created_at.lock().await.insert(contact.id, now_ms());
                let state_bytes = {
                    let s = self.sessions.lock().await;
                    s.get(&contact.id).unwrap().to_bytes()?
                };
                self.db.put_session(contact.id, &state_bytes)?;
                let payload: WirePayload = decode_with_padding_fallback(&plaintext)?;
                self.persist_incoming(contact.id, payload).await?;
                eprintln!("[relay-client] session established with contact {} via X3dhInit", contact.id);
                if init.one_time_id.is_some() {
                    self.republish_bundle().await;
                }
                self.send_kick.notify_one();
            }
            EnvelopeBlob::Ratchet { header, ciphertext } => {
                let mut decrypted: Option<(i64, Vec<u8>, Vec<u8>)> = None;
                let candidates: Vec<gipny_libcore::db::Contact> = if sealed {
                    self.db.list_contacts()?.into_iter().filter(|c| c.trust != TrustLevel::Blocked).collect()
                } else {
                    match self.db.find_contact_by_sign_pk(from_pk)? {
                        Some(c) if c.trust != TrustLevel::Blocked => vec![c],
                        _ => vec![],
                    }
                };
                for c in candidates {
                    let ad = build_ad(&self.identity.card().dh_pk, &c.identity_dh);
                    let mut sess = self.sessions.lock().await;
                    let attempt = match sess.get_mut(&c.id) {
                        Some(state) => state.decrypt(&header, &ciphertext, &ad).map(|pt| (pt, state.to_bytes())),
                        None => match self.db.get_session(c.id)? {
                            Some(blob) => {
                                let mut state = RatchetState::from_bytes(&blob)?;
                                let r = state.decrypt(&header, &ciphertext, &ad).map(|pt| (pt, state.to_bytes()));
                                if r.is_ok() { sess.insert(c.id, state); }
                                r
                            }
                            None => continue,
                        },
                    };
                    drop(sess);
                    if let Ok((pt, sb_res)) = attempt {
                        let sb = sb_res?;
                        decrypted = Some((c.id, pt, sb));
                        break;
                    }
                }
                let (cid, pt, sb) = match decrypted {
                    Some(x) => x,
                    None => {
                        if sealed {
                            eprintln!("[relay-client] sealed ratchet: no session matched, ACK and drop");
                            return Err(CoreError::SealedDrop);
                        }
                        if let Ok(Some(c)) = self.db.find_contact_by_sign_pk(from_pk) {
                            eprintln!("[relay-client] no session for contact {}, requesting resync", c.id);
                            let _ = self.request_resync(&c).await;
                        }
                        return Err(CoreError::Crypto(gipny_libcore::crypto::CryptoError::Mac));
                    }
                };
                self.db.put_session(cid, &sb)?;
                let payload: WirePayload = decode_with_padding_fallback(&pt)?;
                let is_keepalive = payload.ack_for == Some(0)
                    && payload.origin_msg_id == 0
                    && payload.body.is_empty()
                    && payload.attachments.is_empty();
                if !is_keepalive {
                    *self.incoming_since_send.lock().await.entry(cid).or_insert(0) += 1;
                }
                self.persist_incoming(cid, payload).await?;
                if !is_keepalive {
                    let counter = self.incoming_since_send.lock().await.get(&cid).copied().unwrap_or(0);
                    if counter >= KEEPALIVE_INCOMING_THRESHOLD {
                        self.send_kick.notify_one();
                    }
                }
            }
        }
        Ok(())
    }

    async fn accept_x3dh(
        &self,
        init: &gipny_libcore::crypto::X3dhInitial,
        ad: &[u8],
    ) -> Result<(RatchetState, Vec<u8>)> {
        let signed_id = i64::from_be_bytes(
            self.db.get_setting(SETTING_SIGNED_PREKEY_ID)?.ok_or(CoreError::State)?
                .try_into().map_err(|_| CoreError::State)?);
        let signed = self.db.get_prekey(signed_id)?.ok_or(CoreError::State)?;
        let signed_pair = PreKeyPair::from_secret(to_arr32(signed.private.clone())?);
        let opk_pair = if let Some(opk_id) = init.one_time_id {
            let p = self.db.get_prekey(opk_id)?;
            if p.is_none() { return Err(CoreError::StaleOpk); }
            if let Some(ref pk) = p { let _ = self.db.delete_prekey(pk.id); }
            p.map(|p| Ok::<PreKeyPair, CoreError>(PreKeyPair::from_secret(to_arr32(p.private.clone())?))).transpose()?
        } else { None };
        let (state, pt) = crypto::x3dh_respond(&self.identity, &signed_pair, opk_pair.as_ref(), init, ad)?;
        Ok((state, pt))
    }

    /// The sender's relay and name, which every payload may carry.
    /// Anything from a contact proves they can reach us, so the address
    /// announcement for them has arrived and stops repeating.
    /// Time one message's journey, from the moment it actually left to the
    /// moment its acknowledgement arrived.
    ///
    /// `last_attempt_at` is written by `mark_sent` at the point of
    /// transmission, which is the only honest start: `sent_at` is when the
    /// message was composed, and a message that waited for a tunnel or sat in
    /// the retry queue was composed long before it went anywhere.
    ///
    /// A retry muddies this — the clock restarts on each attempt, so what we
    /// time is the attempt that worked, which is the right thing to show.
    /// Anything implausible is dropped rather than smoothed: a device whose
    /// clock jumped would otherwise poison the number for the whole session.
    async fn note_round_trip(self: &Arc<Self>, contact_id: i64, msg: &gipny_libcore::db::Message) {
        let Some(sent_at) = msg.last_attempt_at else { return };
        let elapsed = now_ms() - sent_at;
        if elapsed <= 0 || elapsed > MAX_PLAUSIBLE_RTT_MS {
            return;
        }
        let ms = elapsed as u32;
        let median = {
            let mut all = self.rtt.lock().await;
            let entry = all.entry(contact_id).or_default();
            entry.push(ms);
            entry.median_ms()
        };
        if let Some(median) = median {
            eprintln!("[link] contact {contact_id} round trip {ms} ms (median {median} ms)");
            let _ = self.events.try_send(CoreEvent::LinkRtt { contact_id, ms: median });
        }
    }

    /// What the channel to this contact is doing right now, for the readout
    /// above the chat.
    ///
    /// There is no single «connection» to describe: a letter travels our
    /// outbound tunnel into their relay's inbound tunnel, and each side owns
    /// its own leg. So the readout names both, and says «архив» when their
    /// relay is silent and the network is holding the letter instead.
    pub async fn link_stats(self: &Arc<Self>, contact_id: i64) -> LinkStats {
        let route = match self.db.get_contact(contact_id).ok().flatten() {
            Some(contact) => match self.route_for(&contact).await {
                Some(Route::Relay(_)) => LinkRoute::Relay,
                Some(Route::Dht) => LinkRoute::Archive,
                None => LinkRoute::None,
            },
            None => LinkRoute::None,
        };
        LinkStats {
            our_hops: DEFAULT_TUNNEL_HOPS,
            their_hops: DEFAULT_TUNNEL_HOPS,
            padded: true,
            route,
            rtt_ms: self.rtt.lock().await.get(&contact_id).and_then(Rtt::median_ms),
        }
    }

    async fn note_heard_from(self: &Arc<Self>, contact_id: i64) {
        if self.announce_pending.lock().await.remove(&contact_id) {
            self.announce_sent_at.lock().await.remove(&contact_id);
            eprintln!("[relay-hosted] contact {contact_id} answered; they have our address");
        }
    }

    fn apply_contact_hints(&self, contact_id: i64, payload: &WirePayload) {
        if let Some(relay) = payload.relay_address.as_deref() {
            let trimmed = relay.trim();
            if !trimmed.is_empty() && gipny_libcore::card::is_valid_i2p_address(trimmed) {
                if let Ok(current) = self.db.contact_relay(contact_id) {
                    if current.as_deref() != Some(trimmed) {
                        eprintln!("[relay-discovery] updated relay for contact {} to {}", contact_id, &trimmed[..trimmed.len().min(16)]);
                        let _ = self.db.set_contact_relay(contact_id, Some(trimmed));
                    }
                }
            }
        }
        if let Some(name) = payload.sender_name.as_deref() {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                self.apply_peer_name(contact_id, trimmed);
            }
        }
    }

    /// Until a request is accepted, keep plain messages without showing or
    /// acknowledging them, and ignore everything else.
    async fn persist_from_requester(self: &Arc<Self>, contact_id: i64, payload: WirePayload) -> Result<()> {
        self.apply_contact_hints(contact_id, &payload);
        self.note_heard_from(contact_id).await;
        let plain = payload.group.is_none() && payload.typing.is_none() && payload.edit_of.is_none()
            && payload.pin.is_none() && payload.ack_for.is_none() && payload.callback_data.is_none()
            && payload.console.is_none() && payload.origin_msg_id > 0
            && (!payload.body.is_empty() || !payload.attachments.is_empty());
        if !plain || self.db.find_message_by_origin(contact_id, payload.origin_msg_id as i64)?.is_some() {
            return Ok(());
        }
        let expires_at = payload.ttl_ms.map(|t| payload.sent_at + t);
        let mut atts = Vec::with_capacity(payload.attachments.len());
        for a in &payload.attachments {
            let (key, path, size) = store_attachment_raw(&self.data_dir, &a.data)?;
            atts.push(NewAttachment { name: a.name.clone(), size: size as i64, key: key.to_vec(), path });
        }
        self.db.insert_message_with_origin(
            contact_id, Direction::In, &payload.body, payload.sent_at, expires_at, &atts,
            Some(payload.origin_msg_id as i64),
        )?;
        Ok(())
    }

    async fn persist_incoming(self: &Arc<Self>, contact_id: i64, payload: WirePayload) -> Result<()> {
        match self.db.get_contact(contact_id)?.map(|c| c.request_state) {
            Some(RequestState::Incoming) => return self.persist_from_requester(contact_id, payload).await,
            // Anything at all from them means our introduction arrived.
            Some(RequestState::Outgoing) => {
                self.db.set_contact_request_state(contact_id, RequestState::None)?;
                let _ = self.events.try_send(CoreEvent::ContactUpdated { contact_id });
            }
            _ => {}
        }
        if let Some(typing) = payload.typing {
            let group_id_hex = payload.group.as_ref().map(|g| hex_bytes(&g.id));
            let sender_sign_hex = self.db.get_contact(contact_id).ok().flatten()
                .map(|c| hex_bytes(&c.identity_sign));
            let _ = self.events.try_send(CoreEvent::Typing {
                contact_id: if payload.group.is_none() { Some(contact_id) } else { None },
                group_id: group_id_hex,
                sender_sign_pk: sender_sign_hex,
                typing,
            });
            return Ok(());
        }
        self.apply_contact_hints(contact_id, &payload);
        self.note_heard_from(contact_id).await;
        if payload.buttons.is_some() || payload.callback_data.is_some() {
            if self.db.set_contact_is_bot(contact_id, true).unwrap_or(false) {
                let _ = self.events.try_send(CoreEvent::ContactUpdated { contact_id });
            }
        }
        if let Some(gref) = &payload.group {
            ensure_group_from_wire(&self.db, &self.events, &self.identity, gref).await?;
        }

        if let Some(ack_local) = payload.ack_for {
            let local_id = ack_local as i64;
            if let Some(msg) = self.db.get_message(local_id)? {
                let belongs = match (msg.contact_id, &msg.group_id, &payload.group) {
                    (Some(cid), _, None) => cid == contact_id,
                    (_, Some(gid), Some(gref)) => gid == &gref.id,
                    _ => false,
                };
                if belongs && matches!(msg.direction, Direction::Out) {
                    let _ = self.db.pending_outbound_remove(local_id, contact_id);
                    self.db.mark_delivered(local_id)?;
                    self.note_round_trip(contact_id, &msg).await;
                    let _ = self.events.try_send(CoreEvent::MessageDelivered { message_id: local_id });
                }
            }
            return Ok(());
        }

        if payload.callback_data.is_some()
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.body.is_empty()
            && payload.attachments.is_empty()
        {
            return Ok(());
        }

        if let Some(edit_target_origin) = payload.edit_of {
            let lookup = if let Some(gref) = &payload.group {
                let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
                let self_sign = self.identity.card().sign_pk.to_vec();
                self.db.resolve_group_message(&gref.id, &contact.identity_sign, edit_target_origin as i64, &self_sign)?
            } else {
                self.db.find_message_by_origin(contact_id, edit_target_origin as i64)?
            };
            if let Some(local_id) = lookup {
                self.db.update_message_body(local_id, &payload.body)?;
                if let Some(btns) = &payload.buttons {
                    if let Ok(b) = bincode::serialize(btns) {
                        self.db.set_setting(&format!("buttons_{}", local_id), &b)?;
                    }
                } else {
                    let _ = self.db.delete_setting(&format!("buttons_{}", local_id));
                }
                let _ = self.events.try_send(CoreEvent::MessageEdited {
                    message_id: local_id,
                    body: payload.body.clone(),
                    buttons: payload.buttons.clone(),
                });
                return Ok(());
            }
        }

        if let Some(pin) = &payload.pin {
            let self_sign = self.identity.card().sign_pk.to_vec();
            let origin = pin.origin_msg_id as i64;
            let (resolved, target_cid, target_gid) = if let Some(gref) = &payload.group {
                (self.db.resolve_group_message(&gref.id, &pin.sender_sign_pk, origin, &self_sign)?,
                 None, Some(gref.id.clone()))
            } else {
                (self.db.resolve_contact_message(contact_id, origin)?,
                 Some(contact_id), None)
            };
            match resolved {
                Some(local) => {
                    match (&target_gid, pin.unpin) {
                        (Some(gid), true)  => { self.db.unpin_group_message(gid, local)?; }
                        (Some(gid), false) => { self.db.pin_group_message(gid, local)?; }
                        (None, true)       => { self.db.unpin_contact_message(contact_id, local)?; }
                        (None, false)      => { self.db.pin_contact_message(contact_id, local)?; }
                    }
                    let gid_hex = target_gid.as_deref().map(to_hex);
                    let _ = self.events.try_send(if pin.unpin {
                        CoreEvent::MessageUnpinned { contact_id: target_cid, group_id: gid_hex, message_id: local }
                    } else {
                        CoreEvent::MessagePinned { contact_id: target_cid, group_id: gid_hex, message_id: local }
                    });
                }
                None => match &target_gid {
                    Some(gid) => {
                        self.db.add_deferred_pin_group(gid, &pin.sender_sign_pk, origin, pin.unpin)?;
                        eprintln!("[relay-client] deferred pin for group {} origin={}", to_hex(gid), pin.origin_msg_id);
                    }
                    None => {
                        self.db.add_deferred_pin_contact(contact_id, &pin.sender_sign_pk, origin, pin.unpin)?;
                        eprintln!("[relay-client] deferred pin for contact {} origin={}", contact_id, pin.origin_msg_id);
                    }
                },
            }
            return Ok(());
        }

        let is_empty = payload.body.is_empty() && payload.attachments.is_empty() && payload.group.is_none() && payload.callback_data.is_none() && payload.console.is_none();
        if is_empty { return Ok(()); }

        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;

        if payload.origin_msg_id > 0 {
            let existing = if let Some(gref) = &payload.group {
                let self_sign = self.identity.card().sign_pk.to_vec();
                self.db.resolve_group_message(&gref.id, &contact.identity_sign, payload.origin_msg_id as i64, &self_sign)?
            } else {
                self.db.find_message_by_origin(contact_id, payload.origin_msg_id as i64)?
            };
            if existing.is_some() {
                eprintln!("[relay-client] duplicate origin={} from contact {}, re-acking", payload.origin_msg_id, contact_id);
                let _ = self.send_ack(&contact, &payload).await;
                return Ok(());
            }
        }

        let expires_at = payload.ttl_ms.map(|t| payload.sent_at + t);
        let mut atts = Vec::with_capacity(payload.attachments.len());
        for a in &payload.attachments {
            let (key, path, size) = store_attachment_raw(&self.data_dir, &a.data)?;
            atts.push(NewAttachment { name: a.name.clone(), size: size as i64, key: key.to_vec(), path });
        }
        let (mid, group_id_for_event) = if let Some(gref) = &payload.group {
            if payload.body.is_empty() && payload.attachments.is_empty() {
                let _ = self.events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(&gref.id) });
                return Ok(());
            }
            let mid = self.db.insert_group_message_with_origin(
                &gref.id, Some(&contact.identity_sign), Direction::In,
                &payload.body, payload.sent_at, expires_at, &atts,
                Some(payload.origin_msg_id as i64),
            )?;
            (mid, Some(gref.id.clone()))
        } else {
            let mid = self.db.insert_message_with_origin(
                contact_id, Direction::In, &payload.body, payload.sent_at, expires_at, &atts,
                Some(payload.origin_msg_id as i64),
            )?;
            (mid, None)
        };
        if let Some(btns) = &payload.buttons {
            if let Ok(b) = bincode::serialize(btns) {
                self.db.set_setting(&format!("buttons_{}", mid), &b)?;
            }
        }
        if let Some(c) = &payload.console {
            if let Ok(b) = bincode::serialize(c) {
                self.db.set_setting(&format!("console_{}", mid), &b)?;
            }
            // Console framing is a DM affair; in a group it is stored for
            // display and nothing more.
            if group_id_for_event.is_none() {
                let from_master = self.agent_master().is_some_and(|m| m.contact_id == contact_id);
                match c.kind {
                    CONSOLE_COMMAND => {
                        // Pending until run. Stays pending while the mode is
                        // off, and runs when it is switched on for this contact.
                        self.db.set_setting(&format!("console_pending_{}", mid), b"1")?;
                        if from_master {
                            let _ = self.agent_tx.send(mid);
                        }
                    }
                    CONSOLE_OFF => {
                        if from_master {
                            eprintln!("[agent] master switched agent mode off");
                            let _ = self.disable_agent_mode(true).await;
                        }
                    }
                    CONSOLE_GRANT | CONSOLE_REVOKE => {
                        let granted = c.kind == CONSOLE_GRANT;
                        if self.db.set_contact_agent_granted(contact_id, granted).unwrap_or(false) {
                            let _ = self.events.try_send(CoreEvent::ContactUpdated { contact_id });
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(rep) = &payload.reply_to {
            let self_sign = self.identity.card().sign_pk.to_vec();
            let local = if let Some(gid) = &group_id_for_event {
                self.db.resolve_group_message(gid, &rep.sender_sign_pk, rep.origin_msg_id as i64, &self_sign)?
            } else if rep.sender_sign_pk == self_sign {
                let cid = rep.origin_msg_id as i64;
                match self.db.get_message(cid)? {
                    Some(m) if m.contact_id == Some(contact_id) && matches!(m.direction, Direction::Out) => Some(cid),
                    _ => None,
                }
            } else {
                self.db.find_message_by_origin(contact_id, rep.origin_msg_id as i64)?
            };
            if let Some(rt) = local {
                self.db.set_reply_to(mid, Some(rt))?;
            }
        }
        self.db.touch_contact(contact_id)?;
        let deferred_unpin = match &group_id_for_event {
            Some(gid) => self.db.take_deferred_pin_group(gid, &contact.identity_sign, payload.origin_msg_id as i64)?,
            None => self.db.take_deferred_pin_contact(contact_id, &contact.identity_sign, payload.origin_msg_id as i64)?,
        };
        if let Some(unpin) = deferred_unpin {
            match (&group_id_for_event, unpin) {
                (Some(gid), true)  => { self.db.unpin_group_message(gid, mid)?; }
                (Some(gid), false) => { self.db.pin_group_message(gid, mid)?; }
                (None, true)       => { self.db.unpin_contact_message(contact_id, mid)?; }
                (None, false)      => { self.db.pin_contact_message(contact_id, mid)?; }
            }
            let (tcid, tgid) = match &group_id_for_event {
                Some(gid) => (None, Some(to_hex(gid))),
                None => (Some(contact_id), None),
            };
            let _ = self.events.try_send(if unpin {
                CoreEvent::MessageUnpinned { contact_id: tcid, group_id: tgid, message_id: mid }
            } else {
                CoreEvent::MessagePinned { contact_id: tcid, group_id: tgid, message_id: mid }
            });
        }
        eprintln!("[relay-client] received msg id={} from contact {}", mid, contact_id);
        let body_for_event = payload.body.clone();
        let _ = self.events.try_send(CoreEvent::IncomingMessage {
            contact_id: if group_id_for_event.is_none() { Some(contact_id) } else { None },
            group_id: group_id_for_event.as_deref().map(to_hex),
            sender_sign_pk: Some(to_hex(&contact.identity_sign)),
            message_id: mid,
            body: body_for_event, sent_at: payload.sent_at,
            notify_sound: payload.notify_sound.clone(),
            console_kind: payload.console.as_ref().map(|c| c.kind),
        });
        let _ = self.send_ack(&contact, &payload).await;
        Ok(())
    }

    pub async fn reset_contact_session(&self, contact_id: i64) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        self.request_resync(&contact).await
    }

    async fn request_resync(&self, contact: &gipny_libcore::db::Contact) -> Result<()> {
        let throttled = self.db.resync_recent(contact.id, 60_000)?;
        if !throttled {
            eprintln!("[relay-client] forcing resync for contact {}", contact.id);
            self.db.record_resync(contact.id)?;
            self.sessions.lock().await.remove(&contact.id);
            self.session_created_at.lock().await.remove(&contact.id);
            let _ = self.db.delete_session(contact.id);
            let mut w = self.tiebreaker_waits.lock().await;
            w.insert(contact.id, now_ms() - TIEBREAKER_TIMEOUT_MS - 1);
        }
        self.send_kick.notify_one();
        Ok(())
    }

    async fn send_ack(self: &Arc<Self>, contact: &gipny_libcore::db::Contact, original: &WirePayload) -> Result<()> {
        if original.origin_msg_id == 0 { return Ok(()); }
        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: original.group.clone(),
            buttons: None,
            callback_data: None,
            edit_of: None,
            pin: None,
            ack_for: Some(original.origin_msg_id),
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
        };
        let out = match self.route_for(contact).await {
            Some(x) => x,
            None => return Ok(()),
        };
        if self.ensure_session_for(contact, &out).await.is_err() {
            return Ok(());
        }
        let _ = self.send_payload_via_relay(contact, &mut payload, &out).await;
        Ok(())
    }

    fn spawn_send_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = this.send_kick.notified() => {}
                    _ = tick.tick() => {}
                }
                if let Err(e) = this.flush_all_pending().await {
                    eprintln!("[relay-client] flush err: {:?}", e);
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    async fn flush_all_pending(self: &Arc<Self>) -> Result<()> {
        // Sending needs *their* relay, not ours: a letter is deposited where the
        // recipient collects it. Ours is where replies come back to, and it takes
        // a minute or two of tunnel building — waiting for it held every outgoing
        // message hostage for no reason (owner, 2026-09-18). A contact who has no
        // relay of their own still needs ours as the fallback, and `relay_for`
        // already returns nothing in that case, so that contact is skipped rather
        // than everyone.
        //
        // Announcing our address obviously waits for it to exist.
        if self.relay_out.read().await.is_some() {
            self.flush_relay_announcements().await;
        }
        let contacts = self.db.list_contacts()?;
        let groups_by_id: HashMap<Vec<u8>, String> = self.db.list_groups()?
            .into_iter().map(|g| (g.id, g.name)).collect();
        let members_by_group: HashMap<Vec<u8>, Vec<GroupMember>> = self.db.list_all_group_members()?;
        for contact in contacts {
            if contact.trust == TrustLevel::Blocked { continue; }
            // Nothing goes to someone we have not accepted, not even an ack.
            if contact.request_state == RequestState::Incoming { continue; }
            let pending = self.db.list_unsent_outgoing(contact.id, 50)?;
            let unacked = self.db.list_unacked_outgoing(
                contact.id, now_ms(), RETRY_BASE_BACKOFF_MS, RETRY_MAX_BACKOFF_MS, 50,
            )?;
            let introducing = contact.request_state == RequestState::Outgoing
                && self.db.get_session(contact.id)?.is_none();
            let needs_session = (introducing || self.db.resync_recent(contact.id, 120_000).unwrap_or(false))
                && !self.sessions.lock().await.contains_key(&contact.id);
            let needs_keepalive = self.incoming_since_send.lock().await.get(&contact.id).copied().unwrap_or(0) >= KEEPALIVE_INCOMING_THRESHOLD
                && self.sessions.lock().await.contains_key(&contact.id);
            if pending.is_empty() && unacked.is_empty() && !needs_session && !needs_keepalive { continue; }
            // Deposit on the relay this contact collects from, not on ours.
            let has_mail = !pending.is_empty() || !unacked.is_empty();
            eprintln!("[send] contact {} \"{}\": {} new, {} unacked, session={}, relay={}",
                contact.id,
                contact.display_name,
                pending.len(),
                unacked.len(),
                if self.db.get_session(contact.id).ok().flatten().is_some() { "yes" } else { "no" },
                contact.relay_address.as_deref().map(|r| &r[..16.min(r.len())]).unwrap_or("(none)"));
            let out = match self.relay_for(&contact).await {
                Some(tx) => {
                    self.note_reachability(&contact, true, has_mail).await;
                    Route::Relay(tx)
                }
                // Their relay is away. Leave it in the network, where it waits
                // for them — and keep the contact marked unreachable, because
                // nothing has been handed over yet.
                None => {
                    self.note_reachability(&contact, false, has_mail).await;
                    if self.dht.peer_count() == 0 {
                        eprintln!("[send] contact {}: relay silent and the network is empty — mail stays queued", contact.id);
                        continue;
                    }
                    eprintln!("[send] contact {}: relay silent, going through the network", contact.id);
                    // They may simply have restarted onto a new address; ask
                    // the network in the background, and meanwhile leave the
                    // letter where they will find it either way.
                    self.maybe_look_up_address(&contact).await;
                    Route::Dht
                }
            };
            if self.ensure_session_for(&contact, &out).await.is_err() { continue; }
            if needs_keepalive && pending.is_empty() && unacked.is_empty() {
                let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
                payload.ack_for = Some(0);
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    eprintln!("[relay-client] keepalive err to contact {}: {:?}", contact.id, e);
                } else {
                    eprintln!("[relay-client] keepalive sent to contact {} (DH-roll forced)", contact.id);
                }
            }
            for msg in pending {
                let mut payload = self.build_payload_from_db(&msg)?;
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    eprintln!("[relay-client] send err to contact {}: {:?}", contact.id, e);
                    break;
                }
            }
            for msg in unacked {
                let mut payload = self.build_payload_from_db(&msg)?;
                eprintln!("[relay-client] retry unacked msg {} to contact {} (attempt {})",
                    msg.id, contact.id, msg.send_attempts + 1);
                // Record before sending, like the group path below: without it
                // last_attempt_at stays NULL, the backoff in
                // list_unacked_outgoing is always satisfied, and every
                // undelivered message goes out on every tick of the relay loop.
                self.db.record_send_attempt(msg.id)?;
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    eprintln!("[relay-client] retry err to contact {}: {:?}", contact.id, e);
                    break;
                }
            }
            let group_pending = self.db.pending_outbound_for_recipient(
                contact.id, now_ms(), RETRY_BASE_BACKOFF_MS, RETRY_MAX_BACKOFF_MS, 50,
            )?;
            for msg_id in group_pending {
                let msg = match self.db.get_message(msg_id)? { Some(m) => m, None => {
                    let _ = self.db.pending_outbound_remove(msg_id, contact.id);
                    continue;
                } };
                let mut payload = self.build_payload_from_db(&msg)?;
                if let Some(gid) = &msg.group_id {
                    let gname = groups_by_id.get(gid).cloned().unwrap_or_default();
                    let gref_members: Vec<WireMember> = members_by_group.get(gid).map(|v| v.iter().map(|m| WireMember {
                        sign_pk: m.sign_pk.clone(), dh_pk: m.dh_pk.clone(),
                        onion: m.onion.clone(), name: m.display_name.clone(),
                    }).collect()).unwrap_or_default();
                    payload.group = Some(WireGroupRef { id: gid.clone(), name: gname, members: gref_members });
                }
                eprintln!("[relay-client] retry group msg {} to contact {}", msg_id, contact.id);
                self.db.pending_outbound_record_attempt(msg_id, contact.id)?;
                match self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    Ok(()) => { let _ = self.db.pending_outbound_remove(msg_id, contact.id); }
                    Err(e) => {
                        eprintln!("[relay-client] retry group err to contact {}: {:?}", contact.id, e);
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    async fn send_to_contact(self: &Arc<Self>, contact_id: i64, payload: &mut WirePayload) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        let out = self.route_for(&contact).await.ok_or(CoreError::State)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, payload, &out).await
    }

    pub async fn send_typing_dm(self: &Arc<Self>, contact_id: i64, typing: bool) -> Result<()> {
        if !self.sessions.lock().await.contains_key(&contact_id) { return Ok(()); }
        let mut payload = make_typing_payload(None, typing);
        let _ = self.send_to_contact(contact_id, &mut payload).await;
        Ok(())
    }

    pub async fn send_typing_group(self: &Arc<Self>, group_id: &[u8], typing: bool) -> Result<()> {
        let members = self.db.list_group_members(group_id)?;
        let gref = WireGroupRef {
            id: group_id.to_vec(),
            name: self.db.get_group_name(group_id)?.unwrap_or_default(),
            members: members.iter().map(|m| WireMember {
                sign_pk: m.sign_pk.clone(), dh_pk: m.dh_pk.clone(),
                onion: m.onion.clone(), name: m.display_name.clone(),
            }).collect(),
        };
        let self_sign = self.identity.card().sign_pk;
        for m in &members {
            if m.sign_pk == self_sign.as_slice() { continue; }
            if let Some(c) = self.db.find_contact_by_sign_pk(&m.sign_pk)? {
                if !self.sessions.lock().await.contains_key(&c.id) { continue; }
                let mut payload = make_typing_payload(Some(gref.clone()), typing);
                let _ = self.send_to_contact(c.id, &mut payload).await;
            }
        }
        Ok(())
    }

    async fn ensure_session_for(
        &self,
        contact: &gipny_libcore::db::Contact,
        route: &Route,
    ) -> Result<()> {
        if self.sessions.lock().await.contains_key(&contact.id) {
            eprintln!("[relay-client] ensure_session: contact {} already in cache, no-op", contact.id);
            return Ok(());
        }
        if let Some(blob) = self.db.get_session(contact.id)? {
            eprintln!("[relay-client] ensure_session: contact {} found in DB, loading", contact.id);
            let state = RatchetState::from_bytes(&blob)?;
            self.sessions.lock().await.insert(contact.id, state);
            return Ok(());
        }
        eprintln!("[relay-client] ensure_session: contact {} has NO session, will initiate X3DH", contact.id);

        let me_sign = self.identity.card().sign_pk;
        let should_initiate = me_sign.as_slice() < contact.identity_sign.as_slice();
        if !should_initiate {
            let waited_ms = {
                let mut w = self.tiebreaker_waits.lock().await;
                let now = now_ms();
                let started = *w.entry(contact.id).or_insert(now);
                now - started
            };
            if waited_ms < TIEBREAKER_TIMEOUT_MS {
                eprintln!("[relay-client] tiebreaker: waiting for X3dhInit from contact {} ({}ms)", contact.id, waited_ms);
                return Err(CoreError::State);
            }
            eprintln!("[relay-client] tiebreaker timeout, initiating anyway for contact {}", contact.id);
        }
        self.tiebreaker_waits.lock().await.remove(&contact.id);

        let mut pk = [0u8; 32];
        pk.copy_from_slice(&contact.identity_sign);
        let bundle_bytes = match route {
            Route::Relay(out) => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                self.bundle_waiters.lock().await.entry(pk).or_default().push(tx);
                if out.send(ClientToRelay::GetBundle { pk }).await.is_err() {
                    eprintln!("[relay-client] get_bundle send failed for contact {} (relay channel closed)", contact.id);
                    return Err(CoreError::State);
                }
                match tokio::time::timeout(Duration::from_millis(PENDING_REQ_TIMEOUT_MS), rx).await {
                    Err(_) => {
                        eprintln!("[relay-client] get_bundle timeout for contact {} after {}ms — relay not responding", contact.id, PENDING_REQ_TIMEOUT_MS);
                        return Err(CoreError::State);
                    }
                    Ok(Err(_)) => {
                        eprintln!("[relay-client] get_bundle channel dropped for contact {}", contact.id);
                        return Err(CoreError::State);
                    }
                    Ok(Ok(v)) => v,
                }
            }
            // Their relay is away, so their bundle comes from the network,
            // where they publish it for exactly this case.
            Route::Dht => {
                let their_dh = to_arr32(contact.identity_dh.clone())?;
                dht_client::find_bundle(&self.dht, &pk, &their_dh).await
            }
        };
        let bundle_bytes = match bundle_bytes {
            Some(b) => b,
            None => {
                eprintln!("[relay-client] get_bundle: relay has NO bundle for contact {} — peer never published or relay dropped it", contact.id);
                return Err(CoreError::NotFound);
            }
        };
        let bundle: PreKeyBundle = bincode::deserialize(&bundle_bytes)?;

        if bundle.identity.sign_pk != contact.identity_sign.as_slice()
            || bundle.identity.dh_pk != contact.identity_dh.as_slice()
        {
            eprintln!("[relay-client] bundle identity mismatch for contact {} — bundle from someone else?", contact.id);
            return Err(CoreError::State);
        }
        let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
        let mut empty_payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
        empty_payload.sender_name = self.outgoing_sender_name();
        // A contact created from this init would otherwise have no relay to
        // answer to until a later message brings one.
        let relay = self.relay_onion();
        if !relay.is_empty() {
            empty_payload.relay_address = Some(relay);
        }
        let pt = pad_payload(&encode_payload(&empty_payload)?);
        let (state, init) = crypto::x3dh_initiate(&self.identity, &bundle, &pt, &ad)?;
        self.db.put_session(contact.id, &state.to_bytes()?)?;
        self.sessions.lock().await.insert(contact.id, state);
        self.session_created_at.lock().await.insert(contact.id, now_ms());

        let envelope = EnvelopeBlob::X3dhInit(init);
        let blob = bincode::serialize(&envelope)?;
        self.deliver(contact, blob, route, true).await?;
        eprintln!("[relay-client] x3dh sent to contact {}", contact.id);
        Ok(())
    }

    async fn send_payload_via_relay(
        &self,
        contact: &gipny_libcore::db::Contact,
        payload: &mut WirePayload,
        route: &Route,
    ) -> Result<()> {
        if payload.sender_name.is_none() {
            payload.sender_name = self.outgoing_sender_name();
        }
        // Our relay rides along so the contact learns where we collect, but
        // not on typing notices: they are the most frequent payload by far and
        // a full destination is ~520 bytes of padding-bucket every keystroke.
        if payload.relay_address.is_none() && payload.typing.is_none() {
            let r = self.relay_onion();
            if !r.is_empty() {
                payload.relay_address = Some(r);
            }
        }
        let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
        let raw = encode_payload(payload)?;
        if raw.len() > MAX_PAYLOAD_BYTES {
            eprintln!("[relay-client] payload too large ({}B), dropping msg id={}", raw.len(), payload.origin_msg_id);
            if payload.origin_msg_id > 0 {
                let _ = self.db.mark_sent(payload.origin_msg_id as i64);
                let _ = self.events.try_send(CoreEvent::MessageSent { message_id: payload.origin_msg_id as i64 });
            }
            return Err(CoreError::State);
        }
        let pt = pad_payload(&raw);
        let (header, ct) = {
            let mut sess = self.sessions.lock().await;
            let state = sess.get_mut(&contact.id).ok_or(CoreError::State)?;
            let r = state.encrypt(&pt, &ad)?;
            self.db.put_session(contact.id, &state.to_bytes()?)?;
            r
        };
        let envelope = EnvelopeBlob::Ratchet { header, ciphertext: ct };
        let blob = bincode::serialize(&envelope)?;
        self.deliver(contact, blob, route, false).await?;

        self.incoming_since_send.lock().await.insert(contact.id, 0);

        if payload.origin_msg_id > 0
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.callback_data.is_none()
            && payload.ack_for.is_none()
        {
            self.db.mark_sent(payload.origin_msg_id as i64)?;
            let _ = self.events.try_send(CoreEvent::MessageSent { message_id: payload.origin_msg_id as i64 });
        }
        Ok(())
    }

    fn build_payload_from_db(&self, msg: &gipny_libcore::db::Message) -> Result<WirePayload> {
        let atts = self.db.list_attachments(msg.id)?;
        let mut wire_atts = Vec::with_capacity(atts.len());
        for a in atts {
            let key = to_arr32(a.key.clone())?;
            let full = self.data_dir.join(ATTACHMENTS_DIR).join(&a.path);
            let enc = std::fs::read(&full)?;
            let data = AttachmentCipher::from_key(key).decrypt_chunk(0, &[], &enc)?;
            wire_atts.push(WireAttachment { name: a.name, data });
        }
        let ttl_ms = msg.expires_at.map(|e| e - msg.sent_at);
        let mut p = WirePayload::simple(msg.id as u64, msg.body.clone(), wire_atts, msg.sent_at, ttl_ms);
        if let Some(rt) = msg.reply_to {
            p.reply_to = self.build_wire_reply(rt)?;
        }
        p.console = self.db.get_setting(&format!("console_{}", msg.id))
            .ok().flatten()
            .and_then(|b| bincode::deserialize::<WireConsole>(&b).ok());
        Ok(p)
    }

    fn spawn_purge_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(PURGE_INTERVAL_SECS));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            tick.tick().await;
            let mut bundle_refresh = tokio::time::interval(Duration::from_secs(BUNDLE_REFRESH_SECS));
            bundle_refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
            bundle_refresh.tick().await;
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        let _ = this.db.purge_expired(now_ms());
                        let _ = this.db.purge_old_deferred_pins(now_ms() - 7 * 24 * 3600 * 1000);
                    }
                    _ = bundle_refresh.tick() => {
                        this.republish_bundle().await;
                    }
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// Relay-network upkeep; joining happens when the built-in relay is up.
    fn spawn_dht_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(dht_client::MAINTAIN_EVERY);
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            tick.tick().await;
            let mut collect = tokio::time::interval(DHT_COLLECT_EVERY);
            collect.set_missed_tick_behavior(MissedTickBehavior::Skip);
            collect.tick().await;
            let mut days = DHT_COLLECT_DAYS_FIRST;
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        let address = match &*this.hosted_state.read().unwrap_or_else(|p| p.into_inner()) {
                            HostedRelayState::Ready { address } => Some(address.clone()),
                            _ => None,
                        };
                        dht_client::maintain(&this.dht, &this.db, &this.identity, address.as_deref()).await;
                        this.publish_bundle_to_dht().await;
                        let _ = this.db.dht_seen_purge(now_ms() - DHT_SEEN_TTL_MS);
                    }
                    _ = collect.tick() => {
                        this.collect_from_dht(days).await;
                        days = DHT_COLLECT_DAYS;
                    }
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// Collect what the network holds for us and hand it to the ordinary
    /// incoming path. `days` is how far back to look: a week on the first pass
    /// after a launch, then today and yesterday.
    async fn collect_from_dht(self: &Arc<Self>, days: u32) {
        if self.dht.peer_count() == 0 {
            return;
        }
        let Ok(contacts) = self.db.list_contacts() else { return };
        let mut letters = Vec::new();
        for c in &contacts {
            if c.trust == TrustLevel::Blocked || c.request_state == RequestState::Incoming {
                continue;
            }
            let (Ok(their_sign), Ok(their_dh)) = (to_arr32(c.identity_sign.clone()), to_arr32(c.identity_dh.clone())) else {
                continue;
            };
            letters.extend(dht_client::collect_mail(&self.dht, &self.identity, &their_sign, &their_dh, days).await);
        }
        // First letters from people who may not know us yet; an unknown sender
        // becomes a contact request, exactly as over a relay.
        letters.extend(dht_client::collect_intros(&self.dht, &self.identity, days).await);

        for letter in letters {
            let hash = dht_client::letter_hash(&letter.envelope);
            // A ratchet envelope decrypted twice looks like a broken session.
            match self.db.dht_seen_mark(&hash, now_ms()) {
                Ok(true) => {}
                _ => continue,
            }
            match self.handle_incoming_envelope(&[0u8; 32], &letter.envelope).await {
                Ok(()) | Err(CoreError::StaleOpk) | Err(CoreError::SealedDrop) => {
                    // Ours and handled: take it out of the network so nobody
                    // holds it for the rest of its week.
                    dht_client::drop_letter(&self.dht, &letter).await;
                }
                Err(e) => {
                    eprintln!("[dht] letter from the network did not open: {e:?}");
                    // Not handled: let a later pass try again.
                    let _ = self.db.dht_seen_forget(&hash);
                }
            }
        }
        // The answers (acks among them) go out on the next send tick.
        self.send_kick.notify_one();
    }

    /// Ask at most once every `DHT_ADDRESS_LOOKUP_EVERY`.
    async fn maybe_look_up_address(self: &Arc<Self>, contact: &gipny_libcore::db::Contact) {
        if self.dht.peer_count() == 0 {
            return;
        }
        {
            let mut asked = self.dht_addr_asked.lock().await;
            let now = Instant::now();
            match asked.get(&contact.id) {
                Some(t) if now.duration_since(*t) < DHT_ADDRESS_LOOKUP_EVERY => return,
                _ => asked.insert(contact.id, now),
            };
        }
        self.look_up_address(contact);
    }

    /// Ask the network where a contact collects now. Runs in the background:
    /// a lookup takes tens of seconds over i2p, and the send loop must not
    /// wait for it.
    fn look_up_address(self: &Arc<Self>, contact: &gipny_libcore::db::Contact) {
        let (Ok(their_sign), Ok(their_dh)) = (to_arr32(contact.identity_sign.clone()), to_arr32(contact.identity_dh.clone())) else {
            return;
        };
        let (this, id, known) = (self.clone(), contact.id, contact.relay_address.clone());
        tokio::spawn(async move {
            let Some((relay, _issued)) = dht_client::find_address(&this.dht, &this.identity, &their_sign, &their_dh).await else {
                return;
            };
            if known.as_deref() == Some(relay.as_str()) || relay.trim().is_empty() {
                return;
            }
            eprintln!("[dht] contact {id} moved to a new relay; taking the address from the network");
            if this.db.set_contact_relay(id, Some(&relay)).is_ok() {
                this.send_kick.notify_one();
                let _ = this.events.try_send(CoreEvent::ContactUpdated { contact_id: id });
            }
        });
    }

    /// Put our prekey bundle in the network, so a contact can open a session
    /// with us while our relay is down. It is the same bundle the relay hands
    /// out, and anyone holding our card may read it.
    async fn publish_bundle_to_dht(self: &Arc<Self>) {
        if self.dht.peer_count() == 0 {
            return;
        }
        let Ok(bundle) = self.my_bundle() else { return };
        let Ok(bytes) = bincode::serialize(&bundle) else { return };
        dht_client::publish_bundle(&self.dht, &self.identity, &bytes).await;
    }

    pub fn dht_status(&self) -> dht_client::DhtStatus {
        dht_client::status(&self.dht)
    }

    async fn republish_bundle(&self) {
        let _ = self.ensure_prekeys().await;
        if let Some(tx) = self.relay_out.read().await.clone() {
            if let Ok(b) = self.my_bundle() {
                if let Ok(bytes) = bincode::serialize(&b) {
                    let _ = tx.send(ClientToRelay::Publish { bundle: bytes }).await;
                }
            }
        }
    }

    fn spawn_update_loop(self: Arc<Self>) {
        // No local HTTP proxy this run (Android, or attached to a router we
        // don't own) — nothing to dial, so don't even try.
        if !self.updater.is_configured() {
            eprintln!("[update] no local HTTP proxy this run — auto-update disabled");
            return;
        }
        let this = self.clone();
        let handle = tokio::spawn(async move {
            use gipny_libcore::update::{UPDATE_CHECK_INITIAL_SECS, UPDATE_CHECK_INTERVAL_SECS};
            tokio::time::sleep(Duration::from_secs(UPDATE_CHECK_INITIAL_SECS)).await;
            loop {
                let _ = this.clone().check_and_emit_update().await;
                tokio::time::sleep(Duration::from_secs(UPDATE_CHECK_INTERVAL_SECS)).await;
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// Whether auto-update can work at all (see [`Updater::is_configured`]).
    pub fn update_configured(&self) -> bool {
        self.updater.is_configured()
    }
}

async fn ensure_group_from_wire(
    db: &Arc<Db>,
    events: &mpsc::Sender<CoreEvent>,
    identity: &Arc<Identity>,
    gref: &WireGroupRef,
) -> Result<()> {
    if db.get_group_name(&gref.id)?.is_none() {
        db.create_group(&gref.id, &gref.name)?;
    }
    let my_sign = identity.card().sign_pk;
    for m in &gref.members {
        let is_self = m.sign_pk.as_slice() == my_sign.as_slice();
        if db.is_group_member(&gref.id, &m.sign_pk)? { continue; }
        let name = if m.name.is_empty() { hex_short(&m.sign_pk) } else { m.name.clone() };
        db.add_group_member(&gref.id, &m.sign_pk, &m.dh_pk, &m.onion, &name, is_self)?;
        if !is_self && db.find_contact_by_identity(&m.dh_pk)?.is_none() {
            let _ = db.add_contact(&m.sign_pk, &m.dh_pk, &m.onion, &name, None)?;
        }
    }
    let _ = events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(&gref.id) });
    Ok(())
}

fn store_attachment_raw(data_dir: &PathBuf, data: &[u8]) -> Result<([u8; 32], String, u64)> {
    let cipher = AttachmentCipher::generate();
    let encrypted = cipher.encrypt_chunk(0, &[], data)?;
    let mut name = [0u8; 24];
    fill_random(&mut name);
    let hex = to_hex(&name);
    let path = data_dir.join(ATTACHMENTS_DIR).join(&hex);
    std::fs::write(&path, &encrypted)?;
    Ok((*cipher.key(), hex, data.len() as u64))
}

fn build_ad(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    if a <= b { out.extend_from_slice(a); out.extend_from_slice(b); }
    else      { out.extend_from_slice(b); out.extend_from_slice(a); }
    out
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn make_typing_payload(group: Option<WireGroupRef>, typing: bool) -> WirePayload {
    WirePayload {
        origin_msg_id: 0, body: String::new(), attachments: vec![], sent_at: now_ms(),
        ttl_ms: None, group, buttons: None, callback_data: None,
        edit_of: None, pin: None, ack_for: None, sender_name: None,
        reply_to: None, typing: Some(typing), notify_sound: None, console: None, relay_address: None,
    }
}

fn to_arr32(v: Vec<u8>) -> Result<[u8; 32]> {
    v.try_into().map(|a: Vec<u8>| {
        let mut out = [0u8; 32];
        out.copy_from_slice(&a);
        out
    }).map_err(|_| CoreError::State)
}

fn to_hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b { s.push_str(&format!("{:02x}", x)); }
    s
}

fn hex_short(b: &[u8]) -> String {
    let mut s = String::new();
    for x in &b[..8.min(b.len())] { s.push_str(&format!("{:02x}", x)); }
    s
}

#[cfg(test)]
mod relay_mode_tests {
    use super::{resolve_relay_mode, RelayMode};

    #[test]
    fn a_fresh_profile_gets_the_built_in_relay() {
        assert_eq!(resolve_relay_mode(None, ""), RelayMode::Builtin);
        assert_eq!(resolve_relay_mode(None, "  "), RelayMode::Builtin);
    }

    #[test]
    fn a_profile_that_already_names_a_relay_keeps_it() {
        // Upgrading must not move anyone off the relay their contacts know.
        assert_eq!(resolve_relay_mode(None, "abc.b32.i2p"), RelayMode::External);
    }

    #[test]
    fn an_explicit_choice_wins_either_way() {
        assert_eq!(resolve_relay_mode(Some(b"builtin"), "abc.b32.i2p"), RelayMode::Builtin);
        assert_eq!(resolve_relay_mode(Some(b"external"), ""), RelayMode::External);
        // Garbage in the setting is no choice at all.
        assert_eq!(resolve_relay_mode(Some(b"???"), ""), RelayMode::Builtin);
    }
}
