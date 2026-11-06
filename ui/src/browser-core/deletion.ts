import type { CoreEvent } from '../api';
import { allocate, hex, OUTBOX_LIMIT, payload, RECEIPT_RESERVE, redactMessage, refreshDeletionNotice, unhex, type BrowserState, type PublicIdentity, type StoredMessage, type WirePayload } from './model';

/** Deletion requests (kind 1) and human acknowledgements (kind 3) queued per
 * contact at once. Clearing a long chat marks every row; the rest follow as
 * receipts (kinds 2/4) free slots, so the outbox never fills with them. */
export const DELETION_WINDOW = 16;
/** Controls that stay queued until their answer, re-encrypted on resend:
 * deletion (1→2), human OK (3→4), and reports of what we did with the
 * contact's message — copied (5→8), saved (6→9), screenshot keys (7→10). */
const REQUESTS = [1, 3, 5, 6, 7];
export const PEER_EVENT_KINDS = [5, 6, 7];
const RECEIPT_OFFSET = 3;

const referenceKey = (wire: WirePayload): string | null => wire.dm
  ? `${wire.dm.kind}:${hex(wire.dm.reference.author)}:${wire.dm.reference.origin}:${wire.dm.reference.sent_at}` : null;

function forgetControl(state: BrowserState, peer: string, incoming: WirePayload, kind: number): void {
  const key = referenceKey({ ...incoming, dm: { ...incoming.dm!, kind } });
  state.outbox = state.outbox.filter(item => {
    if (item.peer !== peer || !item.payload || referenceKey(item.payload) !== key) return true;
    if (item.blob === null) return false;
    // Encrypted but not yet stored by the relay: deliver it once, then forget.
    item.payload = null;
    return true;
  });
}

export function deletionEvent(row: StoredMessage): CoreEvent {
  return { MessageDeleted: { contact_id: row.contact_id!, message_id: row.id, deleted_at: row.deleted_at!, pending: row.delete_pending === true, deletion_notice: row.deletion_notice ?? null } };
}

/** Retry durable obligations until kind 2/4, even after relay deposit/restart.
 * Pending (unaccepted) and blocked contacts receive none; the flags survive. */
export function queueDeletionControls(state: BrowserState, identity: PublicIdentity): void {
  const contacts = new Map(state.contacts.map(contact => [contact.id, contact]));
  const queued = new Set<string>();
  const perPeer = new Map<string, number>();
  for (const item of state.outbox) {
    const key = item.payload && REQUESTS.includes(item.payload.dm?.kind ?? -1) ? referenceKey(item.payload) : null;
    if (!key) continue;
    queued.add(`${item.peer}|${key}`);
    perPeer.set(item.peer, (perPeer.get(item.peer) ?? 0) + 1);
  }
  for (const row of state.messages) {
    const reports = row.deleted_at == null ? row.peer_reports ?? [] : [];
    if (row.contact_id === null || row.group_id !== null || ((!row.deletion || (!row.delete_pending && !row.deletion.ack_pending)) && !reports.length)) continue;
    const contact = contacts.get(row.contact_id);
    if (!contact || contact.trust === 2 || contact.request === 'incoming') continue;
    for (const kind of REQUESTS) {
      const due = kind === 1 ? row.delete_pending === true : kind === 3 ? row.deletion?.ack_pending === true : reports.includes(kind);
      if (!due) continue;
      if ((perPeer.get(contact.sign_pk) ?? 0) >= DELETION_WINDOW) break;
      const letter = payload(identity, state.displayName);
      letter.dm = { kind, reference: { author: unhex(row.author), origin: row.origin, sent_at: row.sent_at } };
      if (kind === 1) letter.sent_at = row.deletion!.local_at!;
      const key = `${contact.sign_pk}|${referenceKey(letter)}`;
      if (queued.has(key)) continue;
      if (state.outbox.length >= OUTBOX_LIMIT - RECEIPT_RESERVE) return; // Flags survive; retry as slots free.
      state.outbox.push({ key: crypto.randomUUID(), peer: contact.sign_pk, row: null, payload: letter, blob: null, at: 0, attempts: 0, deposited: false });
      queued.add(key); perPeer.set(contact.sign_pk, (perPeer.get(contact.sign_pk) ?? 0) + 1);
    }
  }
}

/** On accepting a contact request: obligations that arose while it was
 * pending (e.g. a stranger's message that expired before anyone saw it) are
 * dropped instead of being sent as the first letters to the new contact. */
export function forgetRequestObligations(state: BrowserState, contactId: number): void {
  for (const row of state.messages) {
    if (row.contact_id !== contactId || row.group_id !== null || !row.deletion) continue;
    if (row.delete_pending) { row.delete_pending = false; row.deletion.local_seen = true; }
    row.deletion.ack_pending = false;
    refreshDeletionNotice(row);
  }
}

export function acknowledgeBrowserDeletion(state: BrowserState, contactId: number, ids: number[]): void {
  if (!Array.isArray(ids) || !ids.length || ids.length > 500 || ids.some(id => !Number.isSafeInteger(id))) throw new Error('Invalid notice selection');
  const rows = ids.map(id => state.messages.find(row => row.id === id && row.contact_id === contactId && row.group_id === null && row.deletion?.remote_at != null));
  if (rows.some(row => !row)) throw new Error('Only the other participant can acknowledge a deletion');
  for (const row of rows as StoredMessage[]) {
    if (!row.deletion!.remote_seen) { row.deletion!.remote_seen = true; row.deletion!.ack_pending = true; }
    refreshDeletionNotice(row);
  }
}

/** Queue a report that we copied, saved or pressed screenshot keys over a
 * message of this direct chat. Copy/save only for the contact's messages. */
export function queuePeerReport(state: BrowserState, messageId: number, kind: number): void {
  if (!PEER_EVENT_KINDS.includes(kind)) throw new Error('Unknown peer event');
  const row = state.messages.find(value => value.id === messageId && value.contact_id !== null && value.group_id === null && value.deleted_at == null);
  if (!row || (kind !== 7 && row.outgoing)) throw new Error('Nothing to report about this message');
  row.peer_reports = [...new Set([...(row.peer_reports ?? []), kind])];
}

/** Called inside the encrypted profile transaction after E2E authentication. */
export function receiveDeletionControl(state: BrowserState, identity: PublicIdentity, contactId: number, peer: string, incoming: WirePayload, events: CoreEvent[]): WirePayload | null {
  const dm = incoming.dm!; const reference = dm.reference;
  const author = hex(reference.author);
  if (!(dm.kind >= 1 && dm.kind <= 10) || incoming.group !== null || incoming.origin_msg_id !== 0 || incoming.body || incoming.attachments.length || incoming.files.length || incoming.file_chunk !== null || incoming.file_ack !== null || incoming.file_cancel !== null || incoming.typing !== null || incoming.edit_of !== null || incoming.pin !== null || incoming.wipe !== null || incoming.ack_for !== null || incoming.reply_to !== null || incoming.ttl_ms !== null || incoming.buttons != null || incoming.notify_sound !== null || incoming.console !== null || incoming.callback_data !== null || reference.author.length !== 32 || (author !== peer && author !== identity.sign_pk) || !Number.isSafeInteger(reference.origin) || reference.origin <= 0 || !Number.isSafeInteger(reference.sent_at) || incoming.sent_at < 0 || incoming.sent_at > 8.64e15) throw new Error('Invalid deletion control');
  let row = state.messages.find(value => value.contact_id === contactId && value.group_id === null && value.author === author && value.origin === reference.origin && value.sent_at === reference.sent_at);
  if (PEER_EVENT_KINDS.includes(dm.kind)) {
    // The contact did this with a message of ours (copy, save) or of this chat
    // (screenshot keys). Recorded on a live row only; confirmed either way.
    if (row && row.deleted_at == null && (dm.kind === 7 || row.outgoing)) {
      const at = Math.min(Math.max(0, incoming.sent_at), Date.now());
      const list = row.peer_events ?? [];
      const known = list.find(event => event.kind === dm.kind);
      if (known) { known.at = Math.max(known.at, at); known.count = Math.min(known.count + 1, 1_000_000); }
      else list.push({ kind: dm.kind as 5 | 6 | 7, at, count: 1 });
      row.peer_events = list.sort((a, b) => a.kind - b.kind);
      events.push({ PeerEvent: { contact_id: contactId, message_id: row.id, events: row.peer_events } });
    }
    const receipt = payload(identity, state.displayName); receipt.dm = { kind: dm.kind + RECEIPT_OFFSET, reference };
    return receipt;
  }
  if (PEER_EVENT_KINDS.includes(dm.kind - RECEIPT_OFFSET)) {
    if (row?.peer_reports) row.peer_reports = row.peer_reports.filter(kind => kind !== dm.kind - RECEIPT_OFFSET);
    forgetControl(state, peer, incoming, dm.kind - RECEIPT_OFFSET);
    return null;
  }
  if (dm.kind === 1) {
    if (!row) {
      if (author !== peer) throw new Error('Cannot pre-delete a message by the other participant');
      row = { id: allocate(state), contact_id: contactId, group_id: null, sender_sign_pk: null, origin: reference.origin, author, outgoing: false, body: '', sent_at: reference.sent_at, sent: true, delivered: true, read: true, expires_at: null, reply_to: null, dm_seq: Math.max(0, ...state.messages.filter(m => m.contact_id === contactId).map(m => m.dm_seq ?? 0)) + 1 };
      state.messages.push(row);
    }
    redactMessage(state, row, false, incoming.sent_at);
  } else if (dm.kind === 2) {
    if (row?.deletion?.local_at != null) { row.delete_pending = false; forgetControl(state, peer, incoming, 1); }
  } else if (dm.kind === 3) {
    if (row?.deletion?.local_at == null) return null;
    row.deletion.local_seen = true; row.delete_pending = false;
    forgetControl(state, peer, incoming, 1); refreshDeletionNotice(row);
  } else if (dm.kind === 4 && row?.deletion?.remote_seen) {
    row.deletion.ack_pending = false; forgetControl(state, peer, incoming, 3);
  }
  if (row?.deleted_at != null) events.push(deletionEvent(row));
  if (dm.kind !== 1 && dm.kind !== 3) return null;
  const ack = payload(identity, state.displayName); ack.dm = { kind: dm.kind + 1, reference };
  return ack;
}
