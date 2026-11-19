import type { ViewOnceDelivery } from '../../app/api';
import { payload, queueReceipt, RESEND_TTL_MS, type BrowserState, type PublicIdentity, type WirePayload } from '../profiles/model';

const prefix = 'gipny.viewonce.';
export const isViewOnceControl = (value: unknown): boolean => typeof value === 'string' && value.startsWith(prefix);
type Receipt = { peer: string; deadline: number; next: number };
export type ViewOnceTracking = { deliveries: ViewOnceDelivery[]; receipts: Record<string, Receipt> };

function tracking(state: BrowserState): ViewOnceTracking {
  return state.view_once_tracking ??= { deliveries: [], receipts: {} };
}

export function trackViewOnce(state: BrowserState, contactId: number, id: string, expiresAt: number, now = Date.now()): ViewOnceDelivery {
  const rows = tracking(state).deliveries;
  if (rows.length >= 2048) throw new Error('View-once delivery limit reached');
  const record: ViewOnceDelivery = { id, contact_id: contactId, deadline: Math.min(expiresAt, now + RESEND_TTL_MS), status: 'pending' };
  rows.push(record);
  return record;
}

export function oweReceipt(state: BrowserState, peer: string, id: string, expiresAt: number): void {
  const receipts = tracking(state).receipts;
  if (!receipts[id] && Object.keys(receipts).length >= 2048) throw new Error('View-once receipt limit reached');
  receipts[id] = { peer, deadline: Math.min(expiresAt, Date.now() + RESEND_TTL_MS), next: 0 };
}

function enqueue(state: BrowserState, identity: PublicIdentity, peer: string, kind: 'received' | 'confirmed', id: string): void {
  const data = prefix + kind + '.v1/' + id;
  if (state.outbox.some(item => item.peer === peer && item.payload?.callback_data === data)) return;
  const p = payload(identity, state.displayName);
  p.ack_for = 0; p.callback_data = data;
  queueReceipt(state, peer, p);
}

export function viewOnceDue(state: BrowserState, now: number): boolean {
  const tracking = state.view_once_tracking;
  return !!tracking && (tracking.deliveries.some(row => row.deadline <= now && (row.status === 'pending' || row.status === 'relayed' || row.status === 'received'))
    || Object.values(tracking.receipts).some(row => now >= row.next || now >= row.deadline));
}

export function sweepViewOnce(state: BrowserState, identity: PublicIdentity, now: number): ViewOnceDelivery[] {
  const metadata = state.view_once_tracking;
  if (!metadata) return [];
  const changed: ViewOnceDelivery[] = [];
  for (const row of metadata.deliveries) if (row.deadline <= now && (row.status === 'pending' || row.status === 'relayed')) {
    row.status = 'unconfirmed'; changed.push(row);
  }
  metadata.deliveries = metadata.deliveries.filter(row => row.status !== 'received' || row.deadline > now);
  for (const [id, receipt] of Object.entries(metadata.receipts)) {
    if (receipt.deadline <= now) delete metadata.receipts[id];
    else if (receipt.next <= now) {
      enqueue(state, identity, receipt.peer, 'received', id);
      receipt.next = now + 120_000;
    }
  }
  return changed;
}

export function receiveViewOnceControl(state: BrowserState, identity: PublicIdentity, peer: string, incoming: WirePayload): ViewOnceDelivery | true | false {
  if (typeof incoming.callback_data !== 'string' || !incoming.callback_data.startsWith(prefix)) return false;
  const match = /^gipny\.viewonce\.(received|confirmed)\.v1\/([a-f0-9]{32})$/.exec(incoming.callback_data);
  if (!match || incoming.origin_msg_id !== 0 || incoming.ack_for !== 0 || incoming.body !== '' || incoming.group || incoming.ttl_ms != null ||
      incoming.attachments.length || incoming.files.length || incoming.file_chunk || incoming.file_cancel || incoming.file_ack || incoming.dm ||
      incoming.console || incoming.reply_to || incoming.notify_sound || incoming.pin || incoming.edit_of != null || incoming.typing != null || incoming.wipe != null || incoming.buttons != null) return true;
  const contact = state.contacts.find(c => c.sign_pk === peer && c.request === 'none' && c.trust !== 2);
  if (!contact) return true;
  const data = state.view_once_tracking;
  if (!data) return true;
  const id = match[2]!;
  if (match[1] === 'confirmed') {
    if (data.receipts[id]?.peer === peer) delete data.receipts[id];
    return true;
  }
  const row = data.deliveries.find(row => row.id === id && row.contact_id === contact.id);
  if (!row) return true;
  enqueue(state, identity, peer, 'confirmed', id);
  if (row.status === 'received') return true;
  row.status = 'received';
  return row;
}
