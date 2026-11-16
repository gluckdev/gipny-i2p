//! Events emitted by the messenger core.

use super::*;

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
    MessageDeleted {
        contact_id: i64,
        message_id: i64,
        deleted_at: i64,
        pending: bool,
        deletion_notice: Option<gipny_libcore::db::DmDeletionNotice>,
    },
    /// The contact reports copying, saving or a screenshot shortcut on one of
    /// this chat's messages; `events` is that message's full current list.
    PeerEvent {
        contact_id: i64,
        message_id: i64,
        events: Vec<gipny_libcore::db::PeerEvent>,
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
    MessageSent {
        message_id: i64,
    },
    MessageDelivered {
        message_id: i64,
    },
    /// It will not go: too large for one letter (files go in parts; this is
    /// a letter that could not). Marked sent so it is not retried forever.
    MessageFailed {
        message_id: i64,
        reason: String,
    },
    /// A file in parts moved on: `done` of `total` parts, sent (acknowledged
    /// by `contact_id`) or received.
    FileProgress {
        message_id: i64,
        contact_id: i64,
        incoming: bool,
        done: u32,
        total: u32,
    },
    /// A file in parts is whole and is now attachment `attachment_id`.
    FileReceived {
        message_id: i64,
        attachment_id: i64,
    },
    /// A file in parts will not get there (not taken, cancelled, or broken).
    FileFailed {
        message_id: i64,
        contact_id: i64,
        reason: String,
    },
    /// Something arrived from this contact that they wrote at `at_ms`.
    ///
    /// The only presence signal there is: nobody announces that they are
    /// online, and nothing here asks. It carries *their* timestamp rather
    /// than ours precisely because it also fires for mail that sat on a
    /// relay for two hours — that is not someone being online, and the
    /// interface must be able to tell the difference.
    PeerSeen {
        contact_id: i64,
        at_ms: i64,
    },
    /// Median round trip to this contact, in milliseconds, after a fresh
    /// measurement. Drives the link readout above the chat.
    LinkRtt {
        contact_id: i64,
        ms: u32,
    },
    Typing {
        contact_id: Option<i64>,
        group_id: Option<String>,
        sender_sign_pk: Option<String>,
        typing: bool,
    },
    RelayConnected,
    RelayDisconnected,
    ContactAdded {
        contact_id: i64,
    },
    ContactUpdated {
        contact_id: i64,
    },
    /// A contact deleted us and asked for our chat with them to go too; both
    /// are gone now. `name` is what the chat was called here.
    ContactWiped {
        contact_id: i64,
        name: String,
    },
    /// Someone we do not know introduced themselves.
    ContactRequest {
        contact_id: i64,
    },
    /// Joined the relay network, or tried to: `reached` says whether anyone
    /// answered at all and `peers` how many we are in touch with. Both travel
    /// together because `peers: 0` is the only honest failure here and, sent
    /// alone, it read as a finished step on the unlock screen.
    DhtJoined {
        peers: usize,
        reached: bool,
    },
    GroupUpdated {
        group_id: String,
    },
    /// A newer version needs a manual start or system consent.
    UpdateAvailable {
        version: String,
        notes: String,
        size: u64,
    },
    UpdateProgress {
        downloaded: u64,
        total: u64,
        pct: u8,
    },
    /// Downloaded and installed (or staged for the next launch, on Windows) —
    /// nothing to click, just a notice.
    UpdateStaged {
        version: String,
    },
    /// A verified package was handed to the system; installation still needs
    /// confirmation there and cancellation must remain retryable.
    UpdateInstallerOpened {
        version: String,
        path: String,
    },
    /// Downloaded, but this build cannot install it automatically; `path` is
    /// where it landed.
    UpdateReady {
        path: String,
    },
    UpdateFailed {
        reason: String,
    },
    /// Agent mode switched on (with this master) or off, locally or remotely.
    AgentModeChanged {
        master: Option<AgentMaster>,
    },
    /// This client, in agent mode, produced console output for `contact_id`.
    ConsoleActivity {
        contact_id: i64,
    },
    /// The relay mode changed, or the built-in relay changed state.
    RelayInfoChanged {
        info: RelayInfo,
    },
    /// Mail for this contact is queued and their relay has not answered for a
    /// while (`unreachable`), or it answers again.
    ContactReachability {
        contact_id: i64,
        unreachable: bool,
    },
}
