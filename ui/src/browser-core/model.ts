import type { Attachment, Contact, Group, GroupMember, Message } from '../api';

export type PublicIdentity = { card: string; sign_pk: string; dh_pk: string; destination: string; onion: string; fingerprint: string };
export type WireMember = { sign_pk: number[]; dh_pk: number[]; onion: string; name: string };
export type WireGroup = { id: number[]; name: string; members: WireMember[] };
export type WireFileOffer = { file_id: number[]; name: string; size: number; sha256: number[]; chunk_size: number };
export type WireFileChunk = { file_id: number[]; index: number; data: number[] };
export type WireFileAck = { file_id: number[]; received_up_to: number; seen_to: number; missing: number[]; resumed: boolean; echo_index: number; echo_sent_at: number };
export type FileTransfer = WireFileOffer & {
  peer: string; message_id: number; incoming: boolean;
  status: 'offered' | 'sending' | 'receiving' | 'verifying' | 'complete' | 'failed' | 'cancelled';
  /** Base64 bitmap of parts the receiver holds: written here (incoming) or
   * acknowledged by the peer (outgoing). Empty once finished or not started. */
  held: string;
  attachment_id: number | null;
  storage: StoredBlob | null; final_index: number; final_sent_at: number;
  /** Outgoing: part index -> local time the relay stored its letter. Bounded
   * by the per-recipient window; a lost part is encrypted again, never resent. */
  flight?: Record<string, number>;
  /** Outgoing: last receipt (or creation), last stored part, smoothed RTT. */
  progress_at?: number; deposited_at?: number; rtt?: number;
};
/** A finished or forgotten transfer. Late parts must not resurrect it:
 * complete incoming files repeat their final receipt, others answer cancel. */
export type ClosedFile = { peer: string; id: string; incoming: boolean; message_id: number; parts: number; ok: boolean };
export type EarlyPart = { peer: string; file_id: string; index: number; data: string; sent_at: number; at: number; reference?: NonNullable<WirePayload['dm']>['reference'] };
export type StoredBlob = { id: string; key: string; size: number; chunk_size: number };
export type WirePayload = {
  origin_msg_id: number; body: string; attachments: { name: string; data: number[] }[];
  sent_at: number; ttl_ms: number | null; group: WireGroup | null;
  buttons: Message['buttons']; callback_data: string | null; edit_of: number | null;
  pin: { sender_sign_pk: number[]; origin_msg_id: number; unpin: boolean } | null;
  ack_for: number | null; sender_name: string | null;
  reply_to: { sender_sign_pk: number[]; origin_msg_id: number } | null;
  typing: boolean | null; notify_sound: string | null; console: unknown;
  relay_address: string | null; wipe: boolean | null;
  files: WireFileOffer[]; file_chunk: WireFileChunk | null; file_ack: WireFileAck | null; file_cancel: number[] | null;
  dm: { kind: number; reference: { author: number[]; origin: number; sent_at: number } } | null;
};
export type StoredMessage = Message & { origin: number; author: string; deletion?: {
  local_at: number | null; remote_at: number | null; local_seen: boolean; remote_seen: boolean; ack_pending: boolean;
};
  /** Reports (kinds 5/6/7) still to send to the contact about this message. */
  peer_reports?: number[];
};
export type StoredAttachment = Attachment & { data?: string; storage?: StoredBlob };
/** One queued letter. `blob` is ratchet ciphertext (base64) that the peer's
 * relay has not confirmed; it is resent byte-for-byte only until a deposit is
 * confirmed, then dropped. A letter that still needs an answer afterwards
 * keeps `payload` and is encrypted again (a replayed ciphertext cannot be
 * decrypted by its receiver and makes native clients reset the session).
 * A file part is only a reference (`file`) until it is encrypted. */
export type Outbox = {
  key: string; peer: string; row: number | null; payload: WirePayload | null; blob: string | null;
  at: number; attempts: number; deposited: boolean;
  /** The ciphertext opened an X3DH session: deliver it, never drop it. */
  initial?: boolean;
  /** New encryptions of a deposited letter still waiting for its answer. */
  resends?: number;
  file?: { id: string; index: number; message_id: number };
};
export type SeenEnvelope = { hash: string; peer: string; ack: WirePayload | null; group?: string; file?: { id: string; index: number; sent_at: number } };
export type BrowserState = {
  version: 1; nextId: number; displayName: string; contacts: Contact[];
  messages: StoredMessage[]; attachments: StoredAttachment[];
  groups: (Group & { members: GroupMember[] })[];
  /** `seen`: recent envelopes only (duplicate deliveries get their receipt
   * again). `initials`: lifetime X3DH hashes per peer, checked before decrypt. */
  outbox: Outbox[]; seen: SeenEnvelope[]; initials: Record<string, string[]>; tombstones: string[];
  transfers: FileTransfer[]; closedFiles: ClosedFile[];
  earlyParts: EarlyPart[];
  ui: Record<string, string>; muted: string[]; pinnedMessages: number[];
};
export function newState(displayName: string): BrowserState {
  return { version: 1, nextId: 1, displayName, contacts: [], messages: [], attachments: [], groups: [], outbox: [], seen: [], initials: {}, tombstones: [], transfers: [], closedFiles: [], earlyParts: [], ui: {}, muted: [], pinnedMessages: [] };
}
export function loadState(json: string): BrowserState {
  const value = JSON.parse(json) as BrowserState;
  if (value.version !== 1 || !Number.isSafeInteger(value.nextId) || value.nextId < 1 || !Array.isArray(value.contacts) || !Array.isArray(value.messages) || !Array.isArray(value.outbox) || !Array.isArray(value.seen) || !Array.isArray(value.groups) || !Array.isArray(value.attachments) || !Array.isArray(value.tombstones)) throw new Error('Unsupported browser profile state');
  value.transfers ??= []; value.closedFiles ??= []; value.initials ??= {};
  // An earlier build kept copied-text fragments here; peer events replaced it.
  delete value.ui.exfiltration_receipts;
  coerceEarlierSnapshot(value);
  return value;
}

/** rc1/rc2 profiles: number-array ciphertext, boolean/index arrays for file
 * progress, plaintext chunk letters and an unbounded replay list. Converted in
 * place; no further migration is kept for older shapes. */
function coerceEarlierSnapshot(state: BrowserState): void {
  type Old = { parts?: unknown; acked?: unknown };
  for (const file of state.transfers as (FileTransfer & Old)[]) {
    const total = Math.max(1, Math.ceil(file.size / file.chunk_size));
    if (Array.isArray(file.parts) || Array.isArray(file.acked)) {
      const bits = new Uint8Array(Math.ceil(total / 8));
      if (Array.isArray(file.parts)) file.parts.forEach((held, i) => { if (held === true && i < total) setBit(bits, i); });
      if (Array.isArray(file.acked)) for (const i of file.acked) if (Number.isSafeInteger(i) && i >= 0 && i < total) setBit(bits, i as number);
      file.held = file.parts !== undefined && !(file.parts as unknown[]).length && !Array.isArray(file.acked) ? '' : base64(bits);
      delete file.parts; delete file.acked;
    }
    file.held ??= '';
    if (!file.incoming && file.status === 'sending') { file.flight ??= {}; file.progress_at ??= Date.now(); file.deposited_at ??= 0; }
  }
  state.outbox = state.outbox.filter(item => {
    const old = item as Outbox & { blob: unknown };
    if (Array.isArray(old.blob)) item.blob = base64(old.blob as number[]);
    // A deposited ciphertext is never sent again; a deposited part is
    // treated as lost and encrypted afresh.
    if (item.deposited && item.file) return false;
    if (item.deposited) item.blob = null;
    if (item.file && item.blob === null) item.payload = null;
    return true;
  });
  state.earlyParts = (state.earlyParts ?? []).filter(part => typeof (part as EarlyPart).data === 'string');
  if (state.seen.length > RECENT_ENVELOPES) state.seen.splice(0, state.seen.length - RECENT_ENVELOPES);
}

// ---- Compact byte encodings kept inside the JSON snapshot ----

export function fromBase64(value: string): Uint8Array {
  const raw = atob(value);
  const out = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
}
/** Bitmap of `total` parts, decoded once per operation (never per part). */
export function readBits(value: string, total: number): Uint8Array {
  const out = new Uint8Array(Math.ceil(total / 8));
  if (value) out.set(fromBase64(value).subarray(0, out.length));
  if (total % 8 && out.length) out[out.length - 1]! &= (1 << (total % 8)) - 1;
  return out;
}
export const writeBits = (bits: Uint8Array): string => base64(bits);
export const hasBit = (bits: Uint8Array, i: number): boolean => i >= 0 && (bits[i >> 3]! & (1 << (i & 7))) !== 0;
export function setBit(bits: Uint8Array, i: number): boolean {
  if (hasBit(bits, i)) return false;
  bits[i >> 3]! |= 1 << (i & 7);
  return true;
}
export function countBits(bits: Uint8Array): number {
  let n = 0;
  for (const byte of bits) { let b = byte; while (b) { b &= b - 1; n++; } }
  return n;
}
/** First part at or after `from` that is not held (or `total`). */
export function firstClear(bits: Uint8Array, total: number, from = 0): number {
  let i = Math.max(0, from);
  while (i < total) {
    if ((i & 7) === 0 && bits[i >> 3] === 0xff) { i += 8; continue; }
    if (!hasBit(bits, i)) return i;
    i++;
  }
  return total;
}
/** Last held part, or -1. */
export function lastSet(bits: Uint8Array, total: number): number {
  for (let byte = bits.length - 1; byte >= 0; byte--) {
    if (!bits[byte]) continue;
    for (let i = Math.min(total - 1, byte * 8 + 7); i >= byte * 8; i--) if (hasBit(bits, i)) return i;
  }
  return -1;
}

// ---- Replay records ----

/** Recent envelopes: enough to answer a sender that re-deposits the same
 * ciphertext after a lost `Deposited` frame. Older entries are forgotten; the
 * ratchet itself refuses a second decryption of an ordinary letter. */
export const RECENT_ENVELOPES = 1024;
/** Lifetime X3DH initials per peer. Only the authenticated peer can deposit
 * its initials, so per-peer eviction cannot be used against other contacts. */
export const INITIALS_PER_PEER = 64;
const INITIAL_PEERS = 1024;
export const recentEnvelope = (state: BrowserState, peer: string, hash: string): SeenEnvelope | undefined =>
  state.seen.find(value => value.hash === hash && value.peer === peer);
export function rememberEnvelope(state: BrowserState, entry: SeenEnvelope): void {
  state.seen.push(entry);
  if (state.seen.length > RECENT_ENVELOPES) state.seen.splice(0, state.seen.length - RECENT_ENVELOPES);
}
export const replayedInitial = (state: BrowserState, peer: string, hash: string): boolean =>
  state.initials[peer]?.includes(hash.slice(0, 32)) ?? false;
export function rememberInitial(state: BrowserState, peer: string, hash: string): void {
  const list = state.initials[peer] ??= [];
  if (!list.includes(hash.slice(0, 32))) list.push(hash.slice(0, 32));
  if (list.length > INITIALS_PER_PEER) list.splice(0, list.length - INITIALS_PER_PEER);
  const peers = Object.keys(state.initials);
  if (peers.length > INITIAL_PEERS) {
    // Declined strangers cannot grow the profile: keep current contacts first.
    const known = new Set(state.contacts.map(contact => contact.sign_pk));
    for (const key of peers) if (!known.has(key) && key !== peer && Object.keys(state.initials).length > INITIAL_PEERS) delete state.initials[key];
  }
}

// ---- Outbox policy ----

export const OUTBOX_LIMIT = 2048;
/** Receipts may use the last slots, so a full outbox never stops receiving. */
export const RECEIPT_RESERVE = 256;
/** Encrypted file parts waiting for their relay, across all recipients. */
export const SEALED_PART_LIMIT = 16;
/** A deposited ordinary message is re-encrypted for 7 days without a receipt. */
export const RESEND_TTL_MS = 7 * 24 * 3600 * 1000;
export const retryDelay = (attempts: number): number => Math.min(60_000, 2000 * 2 ** Math.min(attempts, 5));
export const resendDelay = (resends: number): number => Math.min(3_600_000, 120_000 * 2 ** Math.min(resends, 5));

/** Pending (unaccepted) and blocked contacts receive no automatic letters. */
export function canSendControls(state: BrowserState, peer: string): boolean {
  const contact = state.contacts.find(value => value.sign_pk === peer);
  return !!contact && contact.trust !== 2 && contact.request !== 'incoming';
}
const deletionKind = (letter: WirePayload | null): number => letter?.dm?.kind ?? -1;
/** Letters that need an answer keep their payload after deposit. */
export const retained = (item: Outbox): boolean => item.payload !== null && (item.row !== null || [1, 3].includes(deletionKind(item.payload)));
/** Newer receipts carry everything older unencrypted ones did. */
function supersedes(letter: WirePayload): string | null {
  if (letter.file_ack) return `ack:${hex(letter.file_ack.file_id)}`;
  if (letter.file_cancel) return `cancel:${hex(letter.file_cancel)}`;
  if (letter.typing !== null) return 'typing';
  return null;
}

export function queueLetter(state: BrowserState, peer: string, letter: WirePayload | null, row: number | null, file?: Outbox['file']): Outbox {
  if (state.outbox.length >= OUTBOX_LIMIT - RECEIPT_RESERVE) throw new Error('Outbox is full');
  const item: Outbox = { key: crypto.randomUUID(), peer, row, payload: letter, blob: null, at: 0, attempts: 0, deposited: false };
  if (file) item.file = file;
  state.outbox.push(item);
  return item;
}

/** Receipts, cancellations and typing: gated by contact state, coalesced
 * while unencrypted, dropped (never thrown) when even the reserve is full. */
export function queueReceipt(state: BrowserState, peer: string, letter: WirePayload): boolean {
  if (!canSendControls(state, peer)) return false;
  const key = supersedes(letter);
  if (key) {
    const old = state.outbox.find(item => item.peer === peer && item.blob === null && !item.deposited && item.payload && supersedes(item.payload) === key);
    if (old) {
      if (old.payload!.file_ack?.resumed && letter.file_ack) letter.file_ack.resumed = true;
      old.payload = letter;
      return true;
    }
  }
  if (state.outbox.length >= OUTBOX_LIMIT) return false;
  state.outbox.push({ key: crypto.randomUUID(), peer, row: null, payload: letter, blob: null, at: 0, attempts: 0, deposited: false });
  return true;
}

/** Choose letters to send now. Never returns a deposited ciphertext: a
 * deposited letter that still waits for its answer is returned only when a
 * new encryption is due (`reopenForResend`). File parts are encrypted only
 * for reachable peers and within the snapshot-wide cap. Letters in `busy` are
 * being sent: never picked twice, not yet failures, and their parts keep their
 * per-peer slots. */
export function dueLetters(state: BrowserState, now: number, limit = 8, busy: { has(key: string): boolean } = new Set<string>()): Outbox[] {
  const silent = new Set(state.contacts.filter(contact => contact.trust === 2 || contact.request === 'incoming').map(contact => contact.sign_pk));
  const failing = new Set(state.outbox.filter(item => !busy.has(item.key) && item.blob !== null && !item.deposited && item.attempts > 0).map(item => item.peer));
  let sealed = state.outbox.filter(item => item.file && item.blob !== null).length;
  const perPeer = new Map<string, number>();
  for (const item of state.outbox) if (item.file && busy.has(item.key)) perPeer.set(item.peer, (perPeer.get(item.peer) ?? 0) + 1);
  const picked: Outbox[] = [];
  const order = [...state.outbox].sort((a, b) => Number(!!a.file) - Number(!!b.file));
  for (const item of order) {
    if (picked.length >= limit) break;
    if (silent.has(item.peer) || busy.has(item.key)) continue;
    if (item.deposited) {
      if (!retained(item) || now - item.at < resendDelay(item.resends ?? 0)) continue;
    } else if (now - item.at < retryDelay(item.attempts)) continue;
    if (item.file) {
      if ((perPeer.get(item.peer) ?? 0) >= 4) continue;
      if (item.blob === null) {
        if (failing.has(item.peer) || sealed >= SEALED_PART_LIMIT) continue;
        sealed++;
      }
      perPeer.set(item.peer, (perPeer.get(item.peer) ?? 0) + 1);
    }
    picked.push(item);
  }
  return picked;
}

/** Inside the encryption transaction: a deposited letter is encrypted anew. */
export function reopenForResend(item: Outbox): boolean {
  if (!item.deposited) return true;
  if (!retained(item)) return false;
  item.deposited = false; item.blob = null; item.initial = undefined;
  item.resends = (item.resends ?? 0) + 1; item.attempts = 0;
  return true;
}

/** Stop re-encrypting ordinary messages that never got a receipt. Deletion
 * requests stay: they are durable obligations until kind 2/4. */
export function expireResends(state: BrowserState, now: number): void {
  state.outbox = state.outbox.filter(item => !(item.deposited && item.row !== null && item.payload && now - item.payload.sent_at > RESEND_TTL_MS));
}
export function allocate(state: BrowserState): number {
  if (state.nextId >= Number.MAX_SAFE_INTEGER) throw new Error('Profile sequence exhausted');
  return state.nextId++;
}
export function hex(bytes: ArrayLike<number>): string { return Array.from(bytes, n => n.toString(16).padStart(2, '0')).join(''); }
export function unhex(value: string, length = 32): number[] {
  if (value.length !== length * 2 || !/^[0-9a-f]+$/i.test(value)) throw new Error('Invalid identity key');
  return Array.from({ length }, (_, i) => parseInt(value.slice(i * 2, i * 2 + 2), 16));
}
export function base64(bytes: ArrayLike<number>): string {
  let text = '';
  for (let i = 0; i < bytes.length; i += 8192) text += String.fromCharCode(...Array.from({ length: Math.min(8192, bytes.length - i) }, (_, j) => bytes[i + j]!));
  return btoa(text);
}
export function unbase64(value: string, limit = 1024 * 1024): number[] {
  if (value.length > Math.ceil(limit / 3) * 4 + 4) throw new Error('Attachment is too large');
  const raw = atob(value);
  if (raw.length > limit) throw new Error('Attachment is too large');
  return Array.from(raw, ch => ch.charCodeAt(0));
}
export function payload(identity: PublicIdentity, name: string): WirePayload {
  return { origin_msg_id: 0, body: '', attachments: [], sent_at: Date.now(), ttl_ms: null, group: null, buttons: null, callback_data: null, edit_of: null, pin: null, ack_for: null, sender_name: name, reply_to: null, typing: null, notify_sound: null, console: null, relay_address: identity.onion, wipe: null, files: [], file_chunk: null, file_ack: null, file_cancel: null, dm: null };
}
export function sameScope(message: StoredMessage, contactId: number | null, groupId: string | null): boolean { return message.contact_id === contactId && message.group_id === groupId; }
export function visible(message: StoredMessage): boolean { return message.deleted_at == null && (message.expires_at == null || message.expires_at > Date.now()); }

/** Keep notices until a person acknowledges; retain rows after dismissal too. */
export function inHistory(message: StoredMessage): boolean {
  return message.deleted_at != null || visible(message);
}

export function refreshDeletionNotice(row: StoredMessage): void {
  const d = row.deletion;
  row.deletion_notice = d?.remote_at != null && !d.remote_seen
    ? { deleted_at: d.remote_at, by_sender: !row.outgoing, can_ack: true }
    : d?.local_at != null && !d.local_seen
      ? { deleted_at: d.local_at, by_sender: row.outgoing, can_ack: false } : null;
}

export function redactMessage(state: BrowserState, row: StoredMessage, local: boolean, at: number): void {
  row.deletion ??= { local_at: null, remote_at: null, local_seen: false, remote_seen: false, ack_pending: false };
  if (local && row.deletion.local_at === null) { row.deletion.local_at = at; row.delete_pending = true; }
  if (!local && row.deletion.remote_at === null) row.deletion.remote_at = at;
  // `at` is the peer's clock for a remote deletion: the notice shows it, but
  // the row's own deletion time stays local, as in the native database.
  row.deleted_at ??= local ? at : Date.now(); row.read = true;
  row.body = ''; row.buttons = null; row.console = null; row.reply_to = null; row.expires_at = null;
  // What the contact reported doing with it goes with it, and so do our own
  // reports about it that have not been encrypted yet.
  row.peer_events = []; delete row.peer_reports;
  const reference = (item: Outbox): boolean => !!item.payload?.dm && item.payload.dm.kind >= 5 && item.payload.dm.kind <= 7
    && hex(item.payload.dm.reference.author) === row.author && item.payload.dm.reference.origin === row.origin && item.payload.dm.reference.sent_at === row.sent_at;
  state.outbox = state.outbox.filter(item => !(reference(item) && item.blob === null));
  const key = `${row.author}:${row.origin}:${row.sent_at}`;
  if (row.group_id === null && !state.tombstones.includes(key)) state.tombstones.push(key);
  state.attachments = state.attachments.filter(file => file.message_id !== row.id);
  state.pinnedMessages = state.pinnedMessages.filter(id => id !== row.id);
  pruneMessageOutbox(state, new Set([row.id]));
  refreshDeletionNotice(row);
}

/** Local history removal must not retain plaintext or tear a prepared ratchet
 * chain. A ciphertext already committed to that chain still needs delivery. */
export function pruneMessageOutbox(state: BrowserState, ids: ReadonlySet<number>): void {
  state.outbox = state.outbox.filter(item => {
    if (item.row === null || !ids.has(item.row)) return true;
    if (item.blob === null) return false;
    item.payload = null; item.row = null;
    return true;
  });
}

/** Automatic clearing preserves saved chats, including files and pins. */
export function clearHistory(state: BrowserState, options: { contactId?: number; preserveSaved?: boolean } = {}): number {
  let saved: Record<string, unknown> = {};
  if (options.preserveSaved) {
    try {
      const value: unknown = JSON.parse(state.ui.chat_prefs ?? '{}');
      const choices = value && typeof value === 'object' ? (value as Record<string, unknown>).saveHistory : null;
      if (choices && typeof choices === 'object' && !Array.isArray(choices)) saved = choices as Record<string, unknown>;
    } catch { /* Invalid preferences do not opt a chat into retention. */ }
  }
  const ids = new Set(state.messages.filter(row => {
    if (row.deleted_at != null) return false;
    if (options.contactId !== undefined && (row.contact_id !== options.contactId || row.group_id !== null)) return false;
    const key = row.group_id !== null ? `group:${row.group_id}` : `contact:${row.contact_id}`;
    return !Object.hasOwn(saved, key) || saved[key] !== true;
  }).map(row => row.id));
  for (const row of state.messages) if (ids.has(row.id) && row.group_id === null) redactMessage(state, row, true, Date.now());
  state.messages = state.messages.filter(row => row.group_id === null || !ids.has(row.id));
  state.attachments = state.attachments.filter(file => !ids.has(file.message_id));
  state.pinnedMessages = state.pinnedMessages.filter(id => !ids.has(id));
  pruneMessageOutbox(state, ids);
  return ids.size;
}

/** Remove expired plaintext, keeping a prepared ciphertext only long enough
 * to finish its ratchet chain. Unencrypted queued letters can be discarded. */
export function expireHistory(state: BrowserState, now = Date.now()): boolean {
  const expired = new Set<number>();
  for (const row of state.messages) {
    if (row.deleted_at != null || row.expires_at === null || row.expires_at > now) continue;
    expired.add(row.id);
    if (row.group_id === null) redactMessage(state, row, true, now);
    else { row.body = ''; row.buttons = null; row.console = null; row.reply_to = null; row.deleted_at = now; row.read = true; }
  }
  if (!expired.size) return false;
  state.attachments = state.attachments.filter(file => !expired.has(file.message_id));
  state.pinnedMessages = state.pinnedMessages.filter(id => !expired.has(id));
  pruneMessageOutbox(state, expired);
  return true;
}
