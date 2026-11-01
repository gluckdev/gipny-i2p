import type { Attachment, Contact, Group, GroupMember, Message } from '../api';

export type PublicIdentity = { card: string; sign_pk: string; dh_pk: string; destination: string; onion: string; fingerprint: string };
export type WireMember = { sign_pk: number[]; dh_pk: number[]; onion: string; name: string };
export type WireGroup = { id: number[]; name: string; members: WireMember[] };
export type WirePayload = {
  origin_msg_id: number; body: string; attachments: { name: string; data: number[] }[];
  sent_at: number; ttl_ms: number | null; group: WireGroup | null;
  buttons: Message['buttons']; callback_data: string | null; edit_of: number | null;
  pin: { sender_sign_pk: number[]; origin_msg_id: number; unpin: boolean } | null;
  ack_for: number | null; sender_name: string | null;
  reply_to: { sender_sign_pk: number[]; origin_msg_id: number } | null;
  typing: boolean | null; notify_sound: string | null; console: unknown;
  relay_address: string | null; wipe: boolean | null;
  files: unknown[]; file_chunk: unknown; file_ack: unknown; file_cancel: unknown;
  dm: { kind: number; reference: { author: number[]; origin: number; sent_at: number } } | null;
};
export type StoredMessage = Message & { origin: number; author: string };
export type StoredAttachment = Attachment & { data: string };
export type Outbox = { key: string; peer: string; row: number | null; payload: WirePayload | null; blob: number[] | null; at: number; attempts: number; deposited: boolean };
export type SeenEnvelope = { hash: string; peer: string; ack: WirePayload | null };
export type BrowserState = {
  version: 1; nextId: number; displayName: string; contacts: Contact[];
  messages: StoredMessage[]; attachments: StoredAttachment[];
  groups: (Group & { members: GroupMember[] })[];
  outbox: Outbox[]; seen: SeenEnvelope[]; tombstones: string[];
  ui: Record<string, string>; muted: string[]; pinnedMessages: number[];
};
export function newState(displayName: string): BrowserState {
  return { version: 1, nextId: 1, displayName, contacts: [], messages: [], attachments: [], groups: [], outbox: [], seen: [], tombstones: [], ui: {}, muted: [], pinnedMessages: [] };
}
export function loadState(json: string): BrowserState {
  const value = JSON.parse(json) as BrowserState;
  if (value.version !== 1 || !Number.isSafeInteger(value.nextId) || value.nextId < 1 || !Array.isArray(value.contacts) || !Array.isArray(value.messages) || !Array.isArray(value.outbox) || !Array.isArray(value.seen) || !Array.isArray(value.groups) || !Array.isArray(value.attachments) || !Array.isArray(value.tombstones)) throw new Error('Unsupported browser profile state');
  return value;
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

/** Remove expired plaintext, keeping a prepared ciphertext only long enough
 * to finish its ratchet chain. Unencrypted queued letters can be discarded. */
export function expireHistory(state: BrowserState, now = Date.now()): boolean {
  const expired = new Set<number>();
  for (const row of state.messages) {
    if (row.deleted_at != null || row.expires_at === null || row.expires_at > now) continue;
    expired.add(row.id);
    row.body = ''; row.buttons = null; row.console = null; row.reply_to = null;
    row.deleted_at = now; row.read = true;
    if (row.group_id === null) {
      const reference = `${row.author}:${row.origin}:${row.sent_at}`;
      if (!state.tombstones.includes(reference)) state.tombstones.push(reference);
    }
  }
  if (!expired.size) return false;
  state.attachments = state.attachments.filter(file => !expired.has(file.message_id));
  state.pinnedMessages = state.pinnedMessages.filter(id => !expired.has(id));
  state.outbox = state.outbox.filter(item => {
    if (item.row === null || !expired.has(item.row)) return true;
    if (item.blob === null) return false;
    item.payload = null; item.row = null;
    return true;
  });
  return true;
}
