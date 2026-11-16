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
  const state = value as unknown as BrowserState;
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
    return true;
  });
  return state;
}
