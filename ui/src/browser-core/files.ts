import type { CoreEvent, FileProgress } from '../api';
import { MAX_FILE_BYTES, type FileStorage } from './file-storage';
import {
  allocate, base64, canSendControls, countBits, firstClear, fromBase64, hasBit, hex, lastSet, OUTBOX_LIMIT, payload, queueReceipt,
  readBits, RECEIPT_RESERVE, retained, setBit, unbase64, unhex, visible, writeBits,
  type BrowserState, type ClosedFile, type FileTransfer, type Outbox, type PublicIdentity, type StoredAttachment,
  type WireFileAck, type WireFileOffer, type WirePayload,
} from './model';

// Attachments are streamed to ciphertext-only OPFS files, independent of the
// small profile snapshot. The wire/file size bound matches native clients.
export const BROWSER_FILE_LIMIT = MAX_FILE_BYTES;
export const FILE_INLINE = 128 * 1024;
export const FILE_CHUNK = 192 * 1024;
const MAX_WIRE_BYTES = 2 * 1024 * 1024 * 1024;
const MAX_PARTS = Math.ceil(MAX_WIRE_BYTES / FILE_CHUNK);
/** Parts queued or in flight to one recipient, across all of its files. One
 * slow, offline or blocked recipient never holds another recipient's window. */
export const FILE_WINDOW = 8;
/** Part references in the outbox across all recipients (no bytes; bounded). */
const FILE_QUEUE_LIMIT = 256;
/** A part counts as lost only well past the measured round trip. */
export const FILE_RTO_FIRST_MS = 60_000;
const FILE_RTO_MIN_MS = 30_000;
const FILE_RTO_MAX_MS = 120_000;
/** The peer's relay keeps storing parts but no receipt comes: give up. */
export const FILE_SILENT_MS = 24 * 3600 * 1000;
/** No receipt at all, reachable or not. */
export const FILE_ABANDON_MS = 7 * 24 * 3600 * 1000;
/** Finished transfers are forgotten beyond this; their ids stay closed. */
export const TRANSFER_SOFT_LIMIT = 1024;
const TRANSFER_LIMIT = 4096;
const CLOSED_LIMIT = 2048;
/** Inline attachments an unaccepted contact may place in OPFS. */
const PENDING_INLINE_FILES = 16;
const PENDING_INLINE_BYTES = 4 * 1024 * 1024;
const PENDING_INLINE_TOTAL = 16 * 1024 * 1024;

export const partCount = (file: WireFileOffer): number => Math.max(1, Math.ceil(file.size / file.chunk_size));
const idOf = (file: WireFileOffer): string => hex(file.file_id);
const uint = (n: number): boolean => Number.isSafeInteger(n) && n >= 0;
const bytes = (a: number[], size: number): boolean => Array.isArray(a) && a.length === size && a.every(n => Number.isInteger(n) && n >= 0 && n <= 255);
const active = (file: FileTransfer): boolean => ['offered', 'sending', 'receiving', 'verifying'].includes(file.status);
const visibleIds = (state: BrowserState): Set<number> => new Set(state.messages.filter(visible).map(row => row.id));

export function validateFilePayload(p: WirePayload): void {
  if (!Array.isArray(p.files) || p.files.length + p.attachments.length > 16 || new Set(p.files.map(idOf)).size !== p.files.length) throw new Error('Invalid file offers');
  for (const f of p.files) {
    if (!bytes(f.file_id, 16) || !bytes(f.sha256, 32) || typeof f.name !== 'string' || new TextEncoder().encode(f.name).length > 1024 || !uint(f.size) || f.size > MAX_WIRE_BYTES || !uint(f.chunk_size) || f.chunk_size < 1 || f.chunk_size > FILE_CHUNK || partCount(f) > MAX_PARTS) throw new Error('Invalid file offer');
  }
  const controls = Number(p.file_chunk !== null) + Number(p.file_ack !== null) + Number(p.file_cancel !== null);
  if (controls && (controls !== 1 || p.origin_msg_id !== 0 || p.body !== '' || p.attachments.length || p.files.length || p.group || p.ack_for !== null || p.typing !== null || p.edit_of !== null || p.pin !== null || p.wipe || p.callback_data !== null || p.console !== null)) throw new Error('Mixed file control');
  if (controls && p.dm && (!p.file_chunk || p.dm.kind !== 0 || !bytes(p.dm.reference.author, 32) || !uint(p.dm.reference.origin) || p.dm.reference.origin === 0 || !Number.isSafeInteger(p.dm.reference.sent_at))) throw new Error('Invalid file message reference');
  const c = p.file_chunk;
  if (c && (!bytes(c.file_id, 16) || !uint(c.index) || c.index >= MAX_PARTS || !Array.isArray(c.data) || c.data.length > FILE_CHUNK || !c.data.every(n => Number.isInteger(n) && n >= 0 && n <= 255))) throw new Error('Invalid file part');
  const a = p.file_ack;
  if (a && (!bytes(a.file_id, 16) || !uint(a.received_up_to) || !uint(a.seen_to) || a.received_up_to > a.seen_to || a.seen_to > MAX_PARTS || !Array.isArray(a.missing) || a.missing.length > 64 || new Set(a.missing).size !== a.missing.length || a.missing.some(n => !uint(n) || n < a.received_up_to || n >= a.seen_to) || !uint(a.echo_index) || !Number.isSafeInteger(a.echo_sent_at) || typeof a.resumed !== 'boolean')) throw new Error('Invalid file receipt');
  if (p.file_cancel && !bytes(p.file_cancel, 16)) throw new Error('Invalid file cancellation');
}

export function isRequestContent(p: WirePayload): boolean {
  return p.group === null && p.typing === null && p.edit_of === null && p.pin === null
    && p.ack_for === null && p.callback_data === null && p.console === null
    && p.file_ack === null && p.file_chunk === null && p.file_cancel === null && p.wipe === null
    && p.origin_msg_id > 0 && (!p.dm || p.dm.kind === 0)
    && (p.body.length > 0 || p.attachments.length > 0 || p.files.length > 0);
}

/** FileStorage reserves the full ciphertext length before accepting an offer;
 * estimate().usage already includes incomplete files, so do not count twice. */
export function fileReservations(state: BrowserState): number {
  void state;
  return 0;
}

/** Whether part `index` is held (incoming) or acknowledged (outgoing). */
export function partHeld(file: FileTransfer, index: number): boolean {
  return file.status === 'complete' || hasBit(readBits(file.held, partCount(file)), index);
}
function doneParts(file: FileTransfer): number {
  if (file.status === 'complete') return partCount(file);
  if (!file.held) return 0;
  return countBits(readBits(file.held, partCount(file))) - Number(file.incoming && file.status === 'verifying');
}
/** Retransmission timeout from the measured round trip (part sent -> receipt). */
export function fileRto(file: FileTransfer): number {
  return file.rtt ? Math.min(FILE_RTO_MAX_MS, Math.max(FILE_RTO_MIN_MS, 4 * file.rtt)) : FILE_RTO_FIRST_MS;
}

// ---- Closed transfers ----

export function closedFile(state: BrowserState, peer: string, id: string, incoming: boolean): ClosedFile | undefined {
  return state.closedFiles.find(entry => entry.peer === peer && entry.id === id && entry.incoming === incoming);
}
function closeTransfers(state: BrowserState, files: ReadonlySet<FileTransfer>): void {
  if (!files.size) return;
  const known = new Set(state.closedFiles.map(entry => `${entry.peer}:${entry.id}:${entry.incoming}`));
  for (const file of files) {
    if (known.has(`${file.peer}:${idOf(file)}:${file.incoming}`)) continue;
    state.closedFiles.push({ peer: file.peer, id: idOf(file), incoming: file.incoming, message_id: file.message_id, parts: partCount(file), ok: file.status === 'complete' });
  }
  if (state.closedFiles.length > CLOSED_LIMIT) state.closedFiles.splice(0, state.closedFiles.length - CLOSED_LIMIT);
  state.transfers = state.transfers.filter(file => !files.has(file));
}
/** Keep the transfer list bounded: forget complete transfers first (their
 * attachment remains), then the oldest finished ones at the hard limit. */
function makeRoom(state: BrowserState): void {
  if (state.transfers.length < TRANSFER_SOFT_LIMIT) return;
  const excess = (limit: number): number => state.transfers.length - limit + 1;
  closeTransfers(state, new Set(state.transfers.filter(f => f.status === 'complete').slice(0, Math.max(0, excess(TRANSFER_SOFT_LIMIT)))));
  if (state.transfers.length >= TRANSFER_LIMIT) closeTransfers(state, new Set(state.transfers.filter(f => !active(f)).slice(0, excess(TRANSFER_LIMIT))));
}

export function offerOutgoingFile(state: BrowserState, messageId: number, peer: string, offer: WireFileOffer, attachmentId: number, now = Date.now()): void {
  makeRoom(state);
  if (state.transfers.length >= TRANSFER_LIMIT) throw new Error('File history limit reached');
  state.transfers.push({ ...offer, peer, message_id: messageId, incoming: false, status: 'sending', held: '', attachment_id: attachmentId, storage: null, final_index: 0, final_sent_at: 0, flight: {}, progress_at: now, deposited_at: 0 });
}

// ---- Receiver ----

function fullReceipt(fileId: number[], total: number, index: number, sentAt: number, resumed = false): WireFileAck {
  return { file_id: fileId, received_up_to: total, seen_to: total, missing: [], resumed, echo_index: index, echo_sent_at: sentAt };
}

/** Native semantics: prefix held, highest seen, at most 64 holes between. */
function receipt(file: FileTransfer, index: number, sentAt: number, resumed = false): WireFileAck {
  const total = partCount(file);
  if (file.status === 'complete') return fullReceipt(file.file_id, total, index, sentAt, resumed);
  const bits = readBits(file.held, total);
  const upTo = firstClear(bits, total);
  let seenTo = Math.max(upTo, lastSet(bits, total) + 1);
  const missing: number[] = [];
  for (let i = upTo; i < seenTo; i++) {
    if (hasBit(bits, i)) continue;
    if (missing.length === 64) { seenTo = i; break; }
    missing.push(i);
  }
  return { file_id: file.file_id, received_up_to: upTo, seen_to: seenTo, missing, resumed, echo_index: index, echo_sent_at: sentAt };
}

function closedAnswer(state: BrowserState, own: PublicIdentity, closed: ClosedFile, fileId: number[], index: number, sentAt: number): WirePayload {
  const answer = payload(own, state.displayName);
  if (closed.ok) answer.file_ack = fullReceipt(fileId, closed.parts, index, sentAt);
  else answer.file_cancel = fileId;
  return answer;
}

/** Answer to an exact duplicate envelope. None while the whole-file hash runs:
 * `finishFileVerification` sends the final receipt or the cancellation. */
export function repeatFileReceipt(state: BrowserState, own: PublicIdentity, peer: string, id: string, index: number, sentAt: number): WirePayload | null {
  const file = state.transfers.find(f => f.peer === peer && f.incoming && idOf(f) === id);
  if (!file) {
    const closed = closedFile(state, peer, id, true);
    return closed ? closedAnswer(state, own, closed, unhex(id, 16), index, sentAt) : null;
  }
  if (file.status === 'offered' || file.status === 'verifying') return null;
  const ack = payload(own, state.displayName);
  if (file.status === 'failed' || file.status === 'cancelled') ack.file_cancel = file.file_id;
  else ack.file_ack = receipt(file, index, sentAt);
  return ack;
}

function progress(file: FileTransfer, state: BrowserState, events: CoreEvent[]): void {
  const contact = state.contacts.find(c => c.sign_pk === file.peer);
  if (!contact) return;
  events.push({ FileProgress: { message_id: file.message_id, contact_id: contact.id, incoming: file.incoming, done: doneParts(file), total: partCount(file) } });
}

/** Stop a transfer. A cancellation goes only to a contact that may receive
 * controls (never to a pending or blocked one), as on native clients. */
function stop(state: BrowserState, own: PublicIdentity, file: FileTransfer, status: 'failed' | 'cancelled', events: CoreEvent[], notify: boolean): void {
  const wasActive = active(file);
  file.status = status; file.held = ''; file.storage = null; file.flight = {};
  const id = idOf(file);
  state.outbox = state.outbox.filter(item => {
    if (item.peer !== file.peer || item.file?.id !== id) return true;
    if (item.blob === null) return false; // A reference only: nothing committed.
    // Ciphertext already on the ratchet (possibly an X3DH initial): deliver it,
    // then forget it like any receipt.
    item.file = undefined;
    return true;
  });
  if (notify && wasActive) { const letter = payload(own, state.displayName); letter.file_cancel = file.file_id; queueReceipt(state, file.peer, letter); }
  const contact = state.contacts.find(c => c.sign_pk === file.peer);
  if (contact) events.push({ FileFailed: { message_id: file.message_id, contact_id: contact.id, reason: status === 'cancelled' ? 'Transfer cancelled' : 'File verification or browser storage limit' } });
}

async function applyPart(state: BrowserState, own: PublicIdentity, file: FileTransfer, index: number, data: Uint8Array, sentAt: number, storage: FileStorage, events: CoreEvent[]): Promise<WirePayload | null> {
  const answer = payload(own, state.displayName);
  if (!state.messages.some(row => row.id === file.message_id && visible(row)) || file.status === 'failed' || file.status === 'cancelled') { answer.file_cancel = file.file_id; return answer; }
  const total = partCount(file);
  if (index >= total || data.length !== Math.min(file.chunk_size, Math.max(0, file.size - index * file.chunk_size))) throw new Error('File part size mismatch');
  if (file.status === 'verifying') return null;
  if (file.status === 'receiving') {
    const bits = readBits(file.held, total);
    if (!hasBit(bits, index)) {
      if (!file.storage) throw new Error('Missing attachment storage');
      await storage.write(file.storage, index, data);
      setBit(bits, index); file.held = writeBits(bits);
      if (countBits(bits) === total) {
        // Hold the final receipt until a separate streaming hash pass
        // succeeds; reporting this part held or missing would both be wrong.
        file.status = 'verifying'; file.final_index = index; file.final_sent_at = sentAt;
        progress(file, state, events);
        return null;
      }
    }
  }
  progress(file, state, events);
  answer.file_ack = receipt(file, index, sentAt);
  return answer;
}

export function finishFileVerification(state: BrowserState, own: PublicIdentity, peer: string, fileId: string, digest: string | null, events: CoreEvent[]): void {
  const file = state.transfers.find(f => f.peer === peer && idOf(f) === fileId && f.status === 'verifying');
  if (!file) return;
  if (!file.storage || digest !== hex(file.sha256)) { stop(state, own, file, 'failed', events, true); return; }
  file.attachment_id = allocate(state);
  state.attachments.push({ id: file.attachment_id, message_id: file.message_id, name: file.name, size: file.size, storage: file.storage });
  file.held = ''; file.status = 'complete';
  events.push({ FileReceived: { message_id: file.message_id, attachment_id: file.attachment_id } });
  progress(file, state, events);
  const ack = payload(own, state.displayName); ack.file_ack = receipt(file, file.final_index, file.final_sent_at);
  queueReceipt(state, peer, ack);
}

export async function acceptFileOffers(state: BrowserState, own: PublicIdentity, peer: string, messageId: number, offers: WireFileOffer[], storage: FileStorage, events: CoreEvent[]): Promise<void> {
  const refuse = (offer: WireFileOffer): void => { const letter = payload(own, state.displayName); letter.file_cancel = offer.file_id; queueReceipt(state, peer, letter); };
  for (const offer of offers) {
    const existing = state.transfers.find(f => f.peer === peer && idOf(f) === idOf(offer));
    if (existing) {
      if (!existing.incoming || existing.message_id !== messageId || existing.size !== offer.size || hex(existing.sha256) !== hex(offer.sha256) || existing.chunk_size !== offer.chunk_size) throw new Error('Conflicting file offer');
      continue;
    }
    if (closedFile(state, peer, idOf(offer), true)) continue;
    makeRoom(state);
    // Capacity never drops the message text: the file alone is refused.
    if (state.transfers.length >= TRANSFER_LIMIT) { refuse(offer); continue; }
    const total = partCount(offer);
    const file: FileTransfer = { ...offer, peer, message_id: messageId, incoming: true, status: 'receiving', held: writeBits(new Uint8Array(Math.ceil(total / 8))), attachment_id: null, storage: null, final_index: 0, final_sent_at: 0 };
    if (state.contacts.some(c => c.sign_pk === peer && c.request === 'incoming')) {
      // An unaccepted contact may send a bounded offer, not reserve gigabytes.
      const waiting = state.transfers.filter(f => f.incoming && active(f));
      if (waiting.length >= 64 || waiting.filter(f => f.peer === peer).length >= 16) continue;
      file.status = 'offered'; file.held = ''; state.transfers.push(file);
      state.earlyParts = state.earlyParts.filter(p => p.peer !== peer || p.file_id !== idOf(file));
      continue;
    }
    const receiving = state.transfers.filter(f => f.incoming && (f.status === 'receiving' || f.status === 'verifying'));
    let refused = receiving.length >= 64 || receiving.filter(f => f.peer === peer).length >= 16;
    if (!refused) { try { file.storage = await storage.create(offer.size, offer.chunk_size, fileReservations(state)); } catch { refused = true; } }
    state.transfers.push(file);
    if (refused) stop(state, own, file, 'failed', events, true);
    else {
      const row = state.messages.find(row => row.id === messageId);
      const early = state.earlyParts.filter(p => p.peer === peer && p.file_id === idOf(file)
        && (!p.reference || (row?.group_id === null && row.author === hex(p.reference.author) && row.origin === p.reference.origin && row.sent_at === p.reference.sent_at)));
      let answered = false;
      for (const part of early) {
        const data = fromBase64(part.data);
        try {
          const ack = await applyPart(state, own, file, part.index, data, part.sent_at, storage, events);
          if (ack) { queueReceipt(state, peer, ack); answered = true; }
        } finally { data.fill(0); }
      }
      if (!answered && file.status === 'receiving') { const ready = payload(own, state.displayName); ready.file_ack = receipt(file, 0, 0, true); queueReceipt(state, peer, ready); }
    }
    state.earlyParts = state.earlyParts.filter(p => p.peer !== peer || p.file_id !== idOf(file));
  }
}

// ---- Sender ----

/** Apply a receipt to an outgoing transfer. Returns the parts to encrypt
 * again now: a hole is usually a part still on its way, so only one past its
 * timeout counts as lost — or everything unheld when the receiver restarted. */
export function applyFileReceipt(file: FileTransfer, a: WireFileAck, now: number): number[] {
  const total = partCount(file);
  if (a.seen_to > total) throw new Error('File receipt beyond end');
  const bits = readBits(file.held, total);
  const missing = new Set(a.missing);
  for (let i = firstClear(bits, total); i < a.received_up_to; i = firstClear(bits, total, i + 1)) setBit(bits, i);
  for (let i = a.received_up_to; i < a.seen_to; i++) if (!missing.has(i)) setBit(bits, i);
  file.held = writeBits(bits);
  if (a.echo_sent_at > 0 && a.echo_sent_at <= now && now - a.echo_sent_at < 30 * 60 * 1000) {
    const sample = now - a.echo_sent_at;
    file.rtt = file.rtt ? Math.round(0.875 * file.rtt + 0.125 * sample) : sample;
  }
  file.progress_at = now;
  const flight = file.flight ??= {};
  const rto = fileRto(file);
  const again: number[] = [];
  for (const [key, at] of Object.entries(flight)) {
    const index = Number(key);
    if (hasBit(bits, index)) { delete flight[key]; continue; }
    if (a.resumed || (missing.has(index) && now - at >= rto)) { delete flight[key]; again.push(index); }
  }
  return again.sort((x, y) => x - y);
}

/** Inside the receive transaction: drop work the receipt made unnecessary. */
function dropHeldParts(state: BrowserState, file: FileTransfer): void {
  const id = idOf(file), total = partCount(file);
  const bits = readBits(file.held, total);
  const done = file.status !== 'sending';
  state.outbox = state.outbox.filter(item => {
    if (item.peer !== file.peer || item.file?.id !== id || (!done && !hasBit(bits, item.file.index))) return true;
    if (item.blob === null || !item.initial) return false; // Unsent ratchet letter: a gap is harmless.
    item.file = undefined;
    return true;
  });
}

/** Called inside the same vault transaction as decrypt; its returned ACK is
 * sent only after the profile (parts and replay record included) was flushed. */
export async function receiveFileControl(state: BrowserState, own: PublicIdentity, peer: string, p: WirePayload, storage: FileStorage, events: CoreEvent[], now = Date.now()): Promise<WirePayload | null> {
  if (!canSendControls(state, peer)) return null;
  if (p.file_chunk) {
    const chunk = p.file_chunk;
    const id = hex(chunk.file_id);
    const reference = p.dm?.reference;
    if (reference && hex(reference.author) !== peer) throw new Error('Forged file author');
    if (reference && state.tombstones.includes(`${peer}:${reference.origin}:${reference.sent_at}`)) return null;
    const file = state.transfers.find(f => f.peer === peer && f.incoming && idOf(f) === id);
    if (reference && file) {
      const row = state.messages.find(row => row.id === file.message_id);
      if (!row || row.group_id !== null || row.author !== peer || row.origin !== reference.origin || row.sent_at !== reference.sent_at) throw new Error('File belongs to another message');
    }
    if (file?.status === 'offered') return null;
    if (file) {
      const data = new Uint8Array(chunk.data);
      try { return await applyPart(state, own, file, chunk.index, data, p.sent_at, storage, events); } finally { data.fill(0); }
    }
    const closed = closedFile(state, peer, id, true);
    if (closed) return closedAnswer(state, own, closed, chunk.file_id, chunk.index, p.sent_at);
    state.earlyParts = state.earlyParts.filter(part => part.at > now - 5 * 60 * 1000);
    const old = state.earlyParts.find(part => part.peer === peer && part.file_id === id && part.index === chunk.index);
    if (!old && state.earlyParts.length < 4 && state.earlyParts.filter(part => part.peer === peer).length < 2) {
      state.earlyParts.push({ peer, file_id: id, index: chunk.index, data: base64(chunk.data), at: now, sent_at: p.sent_at, reference });
    }
    // A reservation is not a receipt: the offer may never arrive.
    return null;
  }
  if (p.file_ack) {
    const a = p.file_ack;
    const file = state.transfers.find(f => f.peer === peer && !f.incoming && idOf(f) === hex(a.file_id));
    if (!file || file.status !== 'sending') return null;
    applyFileReceipt(file, a, now);
    if (countBits(readBits(file.held, partCount(file))) === partCount(file)) { file.status = 'complete'; file.held = ''; file.flight = {}; }
    dropHeldParts(state, file);
    progress(file, state, events);
    // Parts named lost have no flight entry now; `queueFileChunks` (run at
    // the end of this transaction) queues them as new references.
  }
  if (p.file_cancel) {
    const id = hex(p.file_cancel);
    const file = state.transfers.find(f => f.peer === peer && idOf(f) === id);
    if (file && active(file)) stop(state, own, file, 'cancelled', events, false);
    state.earlyParts = state.earlyParts.filter(part => part.peer !== peer || part.file_id !== id);
  }
  return null;
}

type Sendable = { file: FileTransfer; attachment: StoredAttachment };
function sendable(state: BrowserState, live: Set<number>): Sendable[] {
  const attachments = new Map(state.attachments.map(a => [a.id, a]));
  const contacts = new Map(state.contacts.map(c => [c.sign_pk, c]));
  // Parts follow the offer: wait until the message letter to that peer was answered.
  const offered = new Set(state.outbox.filter(item => item.row !== null).map(item => `${item.peer}:${item.row}`));
  const out: Sendable[] = [];
  for (const file of state.transfers) {
    if (file.incoming || file.status !== 'sending' || !live.has(file.message_id)) continue;
    const contact = contacts.get(file.peer);
    const attachment = file.attachment_id === null ? undefined : attachments.get(file.attachment_id);
    if (!contact || contact.trust === 2 || contact.request === 'incoming' || !attachment || offered.has(`${file.peer}:${file.message_id}`)) continue;
    out.push({ file, attachment });
  }
  return out;
}

/** Queue part references (no bytes) within each recipient's window. Run at
 * the end of every transaction, so a receipt refills the window in the same
 * snapshot write. Pure and per-transfer isolated: a broken transfer is marked
 * failed and the others continue. */
export function queueFileChunks(state: BrowserState, own: PublicIdentity, now = Date.now()): void {
  const load = new Map<string, number>();
  const queued = new Map<string, Set<number>>();
  let references = 0;
  for (const item of state.outbox) if (item.file) {
    const key = `${item.peer}:${item.file.id}`;
    (queued.get(key) ?? queued.set(key, new Set()).get(key)!).add(item.file.index);
    load.set(item.peer, (load.get(item.peer) ?? 0) + 1);
    references++;
  }
  const sending = state.transfers.filter(file => !file.incoming && file.status === 'sending');
  for (const file of sending) {
    const flight = file.flight ??= {};
    // Tail parts: nothing after them arrives, so no receipt names them.
    const rto = fileRto(file);
    for (const [key, at] of Object.entries(flight)) if (now - at >= rto) delete flight[key];
    load.set(file.peer, (load.get(file.peer) ?? 0) + Object.keys(flight).length);
  }
  for (const { file } of sendable(state, visibleIds(state))) {
    try {
      let room = FILE_WINDOW - (load.get(file.peer) ?? 0);
      if (room <= 0) continue;
      const total = partCount(file), id = idOf(file), bits = readBits(file.held, total);
      const flight = file.flight!, inQueue = queued.get(`${file.peer}:${id}`);
      for (let index = firstClear(bits, total); index < total && room > 0; index = firstClear(bits, total, index + 1)) {
        if (inQueue?.has(index) || flight[index] !== undefined) continue;
        if (references >= FILE_QUEUE_LIMIT || state.outbox.length >= OUTBOX_LIMIT - RECEIPT_RESERVE) return;
        state.outbox.push({ key: crypto.randomUUID(), peer: file.peer, row: null, payload: null, blob: null, at: 0, attempts: 0, deposited: false, file: { id, index, message_id: file.message_id } });
        room--; references++;
        load.set(file.peer, (load.get(file.peer) ?? 0) + 1);
      }
    } catch { stop(state, own, file, 'failed', [], true); }
  }
}

/** The part a reference needs, or null when it is no longer wanted. */
export function chunkSource(state: BrowserState, item: Outbox): { file: FileTransfer; attachment: StoredAttachment; index: number } | null {
  if (!item.file) return null;
  const file = state.transfers.find(f => f.peer === item.peer && !f.incoming && f.status === 'sending' && idOf(f) === item.file!.id);
  const attachment = file && state.attachments.find(a => a.id === file.attachment_id);
  if (!file || !attachment || partHeld(file, item.file.index) || !state.messages.some(row => row.id === file.message_id && visible(row))) return null;
  return { file, attachment, index: item.file.index };
}
/** Inline legacy attachments only; OPFS parts are read by FileStorage. */
export function inlinePart(attachment: StoredAttachment, chunkSize: number, index: number): Uint8Array {
  return new Uint8Array(unbase64(attachment.data ?? '', FILE_INLINE).slice(index * chunkSize, (index + 1) * chunkSize));
}

/** Inside the encryption transaction: the letter for a part read outside it. */
export function buildChunkLetter(state: BrowserState, own: PublicIdentity, item: Outbox, data: Uint8Array | null, now = Date.now()): WirePayload | null {
  const source = chunkSource(state, item);
  if (!source || !data) return null;
  const row = state.messages.find(row => row.id === source.file.message_id)!;
  const letter = payload(own, state.displayName);
  letter.sent_at = now;
  if (row.group_id === null) letter.dm = { kind: 0, reference: { author: unhex(row.author), origin: row.origin, sent_at: row.sent_at } };
  letter.file_chunk = { file_id: source.file.file_id, index: source.index, data: Array.from(data) };
  return letter;
}

/** After the peer's relay confirmed a deposit. The ciphertext is gone from
 * the snapshot either way; returns a message id that is now fully sent. */
export function settleDeposit(state: BrowserState, key: string, now = Date.now()): number | null {
  const item = state.outbox.find(value => value.key === key);
  if (!item) return null;
  if (item.file) {
    const file = state.transfers.find(f => f.peer === item.peer && !f.incoming && f.status === 'sending' && idOf(f) === item.file!.id);
    if (file && !partHeld(file, item.file.index)) { (file.flight ??= {})[item.file.index] = now; file.deposited_at = now; }
    state.outbox = state.outbox.filter(value => value !== item);
    return null;
  }
  item.deposited = true; item.blob = null; item.initial = undefined; item.at = now; item.attempts = 0;
  if (!retained(item)) { state.outbox = state.outbox.filter(value => value !== item); return null; }
  if (item.row === null || !state.outbox.filter(value => value.row === item.row).every(value => value.deposited)) return null;
  const row = state.messages.find(value => value.id === item.row);
  if (!row || row.sent) return null;
  row.sent = true;
  return row.id;
}

/** A part could not be read: only this transfer fails. */
export function failOutgoingFile(state: BrowserState, own: PublicIdentity, peer: string, id: string, events: CoreEvent[]): void {
  const file = state.transfers.find(f => f.peer === peer && !f.incoming && idOf(f) === id && f.status === 'sending');
  if (file) stop(state, own, file, 'failed', events, true);
}

/** Native gives up after a day of a live peer that never answers; the browser
 * also abandons a transfer after a week with no receipt at all. */
export function stalledFiles(state: BrowserState, now: number): FileTransfer[] {
  return state.transfers.filter(file => !file.incoming && file.status === 'sending' && file.progress_at !== undefined
    && ((file.deposited_at ?? 0) > file.progress_at && now - file.progress_at > FILE_SILENT_MS || now - file.progress_at > FILE_ABANDON_MS));
}
export function failStalledFiles(state: BrowserState, own: PublicIdentity, now: number, events: CoreEvent[]): void {
  for (const file of stalledFiles(state, now)) stop(state, own, file, 'failed', events, true);
}
/** Cheap pump check for time-driven file work no transaction triggered:
 * an overdue part (cleared by the transaction) or a transfer to give up. */
export function fileWorkDue(state: BrowserState, now: number): boolean {
  return state.transfers.some(file => !file.incoming && file.status === 'sending'
    && Object.values(file.flight ?? {}).some(at => now - at >= fileRto(file))) || stalledFiles(state, now).length > 0;
}

/** On profile open: incoming transfers report their state (`resumed`); our
 * own in-flight parts restart their timers instead of all going again now. */
export function resumeFiles(state: BrowserState, own: PublicIdentity, now = Date.now()): void {
  for (const file of state.transfers) {
    if (file.incoming && file.status === 'receiving') { const ack = payload(own, state.displayName); ack.file_ack = receipt(file, 0, 0, true); queueReceipt(state, file.peer, ack); }
    if (!file.incoming && file.status === 'sending') for (const key of Object.keys(file.flight ?? {})) file.flight![key] = now;
  }
}

/** A session reset loses whatever was in flight on the old session. */
export function forgetFlight(state: BrowserState, peer: string): void {
  for (const file of state.transfers) if (file.peer === peer && !file.incoming) file.flight = {};
}

export function cancelBrowserFiles(state: BrowserState, own: PublicIdentity, messageId: number, events: CoreEvent[]): void {
  for (const file of state.transfers) if (file.message_id === messageId && active(file)) stop(state, own, file, 'cancelled', events, true);
}

export async function acceptPendingFiles(state: BrowserState, own: PublicIdentity, peer: string, storage: FileStorage, events: CoreEvent[]): Promise<void> {
  const waiting = state.transfers.filter(file => file.peer === peer && file.status === 'offered');
  for (const file of waiting) {
    if (!state.messages.some(row => row.id === file.message_id && visible(row))) continue;
    state.transfers = state.transfers.filter(f => f !== file);
    await acceptFileOffers(state, own, peer, file.message_id, [file], storage, events);
  }
}

/** Whether an unaccepted contact may still place these inline attachments in
 * OPFS (the message text is kept either way). */
export function pendingInlineAllowed(state: BrowserState, contactId: number, incoming: { data: number[] }[]): boolean {
  const pending = new Set(state.contacts.filter(c => c.request === 'incoming').map(c => c.id));
  const owner = new Map(state.messages.map(row => [row.id, row.contact_id]));
  let files = 0, bytes = 0, total = 0;
  for (const attachment of state.attachments) {
    const contact = owner.get(attachment.message_id);
    if (contact === null || contact === undefined || !pending.has(contact)) continue;
    total += attachment.size;
    if (contact === contactId) { files++; bytes += attachment.size; }
  }
  const adding = incoming.reduce((n, file) => n + file.data.length, 0);
  return files + incoming.length <= PENDING_INLINE_FILES && bytes + adding <= PENDING_INLINE_BYTES && total + adding <= PENDING_INLINE_TOTAL;
}

/** Erasure removes file plaintext and the transfer record; a compact closed
 * id keeps delayed parts from resurrecting it. Completed transfers are never
 * cancelled; nothing is sent to pending or blocked contacts. */
export function pruneFiles(state: BrowserState, own: PublicIdentity, now = Date.now()): void {
  const live = visibleIds(state);
  const gone = new Set(state.transfers.filter(file => !live.has(file.message_id)));
  for (const file of gone) if (active(file)) stop(state, own, file, 'cancelled', [], true);
  closeTransfers(state, gone);
  makeRoom(state);
  const contacts = new Set(state.contacts.map(c => c.sign_pk));
  state.earlyParts = state.earlyParts.filter(part => contacts.has(part.peer) && part.at > now - 5 * 60 * 1000);
}

export function browserFileProgress(state: BrowserState, messageId: number): FileProgress[] {
  // A group file has one transfer per recipient; show the least advanced peer.
  const byFile = new Map<string, FileTransfer[]>();
  for (const file of state.transfers) if (file.message_id === messageId) { const list = byFile.get(idOf(file)) ?? []; list.push(file); byFile.set(idOf(file), list); }
  return [...byFile.values()].map(files => {
    const first = files[0]!;
    const done = Math.min(...files.map(doneParts));
    const status: FileProgress['status'] = files.some(f => f.status === 'failed') ? 'failed' : files.some(f => f.status === 'cancelled') ? 'cancelled' : files.some(f => f.status === 'offered') ? 'waiting' : files.every(f => f.status === 'complete') ? 'complete' : first.incoming ? 'receiving' : 'sending';
    return { name: first.name, size: first.size, done, total: partCount(first), incoming: first.incoming, cancelable: true, status };
  });
}

