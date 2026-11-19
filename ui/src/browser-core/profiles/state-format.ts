import type { BrowserState } from './model';

const unsupported = (): never => {
  throw new Error('Unsupported browser profile format (expected version 2). The saved profile was not changed.');
};
const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const address = (value: unknown): boolean => record(value) && typeof value.i2p_address === 'string' && value.i2p_address.length > 0;

/** Metadata has its own version, separate from the encrypted vault container.
 * Reject before the backend owns the engine or collects/writes any files. No
 * aliasing or migration of previous profile snapshots is performed. */
export function loadState(json: string): BrowserState {
  let value: unknown;
  try { value = JSON.parse(json); } catch { return unsupported(); }
  if (!record(value) || value.version !== 2 || !Number.isSafeInteger(value.nextId) || (value.nextId as number) < 1 ||
      typeof value.displayName !== 'string' || !record(value.ui) || !record(value.initials)) return unsupported();
  for (const key of ['contacts', 'messages', 'outbox', 'seen', 'groups', 'attachments', 'tombstones', 'transfers', 'closedFiles', 'earlyParts', 'muted', 'pinnedMessages']) {
    if (!Array.isArray(value[key])) return unsupported();
  }
  const delivery = value.view_once_tracking;
  if (delivery !== undefined && (!record(delivery) || !Array.isArray(delivery.deliveries) || delivery.deliveries.length > 2048
      || !delivery.deliveries.every(row => record(row) && typeof row.id === 'string' && /^[a-f0-9]{32}$/.test(row.id)
        && Number.isSafeInteger(row.contact_id) && Number.isSafeInteger(row.deadline) && ['pending', 'relayed', 'received', 'unconfirmed'].includes(String(row.status)))
      || !record(delivery.receipts) || Object.keys(delivery.receipts).length > 2048 || !Object.entries(delivery.receipts).every(([id, row]) =>
        /^[a-f0-9]{32}$/.test(id) && record(row) && typeof row.peer === 'string' && /^[a-f0-9]{64}$/.test(row.peer)
        && Number.isSafeInteger(row.deadline) && Number.isSafeInteger(row.next)))) return unsupported();
  const state = value as unknown as BrowserState;
  state.delivery_expiry_notices ??= [];
  if (!Array.isArray(state.delivery_expiry_notices)) return unsupported();
  if (!state.contacts.every(address) || !state.groups.every(group => record(group) && Array.isArray(group.members) && group.members.every(address)) ||
      !state.transfers.every(file => record(file) && typeof file.held === 'string') ||
      !state.outbox.every(item => record(item) && (item.blob === null || typeof item.blob === 'string')) ||
      !state.earlyParts.every(part => record(part) && typeof part.data === 'string')) return unsupported();

  // Current-format restart recovery: confirmed file deposits are not resent,
  // and a file reference is materialized only when it is encrypted for sending.
  state.outbox = state.outbox.filter(item => {
    if (item.deposited && item.file) return false;
    if (item.deposited) item.blob = null;
    if (item.file && item.blob === null) item.payload = null;
    // Old erasures left only ciphertext, with neither the original deadline
    // nor any live delivery obligation. Retire it (and its X3DH session if
    // needed) in the opening transaction rather than replaying expired text.
    if (item.row === null && !item.payload && !item.file && item.blob && item.send_until === undefined) item.send_until = 0;
    return true;
  });
  // Snapshots written before explicit group recipient tracking still contain
  // one ordinary outbox item for each outstanding recipient. Capture these
  // before contact removal or queue pruning can erase that information.
  const pending = new Map<number, Set<string>>();
  for (const item of state.outbox) if (item.row !== null) {
    if (!pending.has(item.row)) pending.set(item.row, new Set());
    pending.get(item.row)!.add(item.peer);
  }
  for (const row of state.messages) {
    const peers = pending.get(row.id);
    if (row.outgoing && row.group_id != null && !row.delivered && row.deleted_at == null && !row.pending_recipients && peers) row.pending_recipients = [...peers];
  }
  return state;
}
