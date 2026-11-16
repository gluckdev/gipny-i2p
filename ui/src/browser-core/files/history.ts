import type { MessageFiles } from '../../app/api';
import { visible, type BrowserState } from '../profiles/model';
import { browserFileProgress } from './transfers';

/** Referenced encrypted blobs in a profile snapshot (attachments and transfers). */
export function fileIds(state: BrowserState): Set<string> {
  return new Set([...state.attachments.flatMap(file => file.storage ? [file.storage.id] : []), ...state.transfers.flatMap(file => file.storage ? [file.storage.id] : [])]);
}

/** Public attachment/progress projections; never expose OPFS keys or blob ids. */
export function messageFiles(state: BrowserState, ids: number[]): Record<number, MessageFiles> {
  if (!Array.isArray(ids) || ids.length > 500 || ids.some(id => !Number.isSafeInteger(id) || id <= 0)) throw new Error('Invalid file history window');
  const result: Record<number, MessageFiles> = {};
  for (const id of ids) {
    const row = state.messages.find(message => message.id === id && visible(message));
    const attachments = row ? state.attachments.filter(file => file.message_id === id).map(file => ({ id: file.id, message_id: file.message_id, name: file.name, size: file.size })) : [];
    const transfers = row ? browserFileProgress(state, id) : [];
    const chunked = new Set(state.transfers.filter(file => file.message_id === id).map(file => file.attachment_id));
    // A forgotten (closed) outgoing transfer that failed must not look complete.
    const failed = state.closedFiles.some(file => file.message_id === id && !file.incoming && !file.ok);
    result[id] = { attachments, progress: [...transfers, ...(row?.outgoing ? attachments.filter(file => !chunked.has(file.id)).map(file => ({ name: file.name, size: file.size, done: row.delivered && !failed ? 1 : 0, total: 1, incoming: false, cancelable: false, status: failed ? 'failed' as const : row.delivered ? 'complete' as const : 'sending' as const })) : [])] };
  }
  return result;
}
