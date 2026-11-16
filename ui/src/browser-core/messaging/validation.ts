import { validateFilePayload } from '../files/transfers';
import type { WirePayload } from '../profiles/model';

/** Validate the decrypted wire shape before changing contacts or message state. */
export function validatePayload(value: WirePayload): void {
  if (!Number.isSafeInteger(value.origin_msg_id) || value.origin_msg_id < 0 || !Number.isSafeInteger(value.sent_at) || typeof value.body !== 'string' || value.body.length > 128 * 1024 || !Array.isArray(value.attachments) || value.attachments.length > 16 || value.attachments.reduce((n, file) => n + file.data.length, 0) > 1024 * 1024 || value.attachments.some(file => new TextEncoder().encode(file.name).length > 1024) || (value.ttl_ms !== null && (!Number.isSafeInteger(value.ttl_ms) || value.ttl_ms < 0 || value.ttl_ms > 365 * 86400000))) throw new Error('Unsupported message payload');
  validateFilePayload(value);
  if (value.console !== null || value.callback_data !== null) throw new Error('This browser does not support this message control');
  if (value.edit_of !== null && (!Number.isSafeInteger(value.edit_of) || value.edit_of <= 0 || value.attachments.length > 0)) throw new Error('Invalid edit payload');
  if (value.ack_for !== null && (!Number.isSafeInteger(value.ack_for) || value.ack_for < 0)) throw new Error('Invalid receipt');
  if (value.pin !== null && (value.pin.sender_sign_pk.length !== 32 || !Number.isSafeInteger(value.pin.origin_msg_id) || value.pin.origin_msg_id <= 0)) throw new Error('Invalid pinned-message reference');
}
