import type { Contact } from '../../app/api';
import { isValidI2pAddress } from '../../features/contacts/contact-card';
import type { ActionLog } from '../diagnostics/log';
import { allocate, unhex, type BrowserState, type PublicIdentity } from '../profiles/model';

type Args = Record<string, unknown>;
type ContactLog = Pick<ActionLog, 'info'>;
const text = (args: Args, key: string): string => { if (typeof args[key] !== 'string') throw new Error(`Missing ${key}`); return args[key] as string; };

/** Contact identity pinning and local history removal; persistence is owned by
 * the backend transaction, not these synchronous state operations. */
export function addContact(state: BrowserState, identity: PublicIdentity, log: ContactLog, args: Args, request: Contact['request'] = 'outgoing'): Contact {
  const sign = text(args, 'signPk').toLowerCase(); const dh = text(args, 'dhPk').toLowerCase();
  unhex(sign); unhex(dh);
  if (sign === identity.sign_pk) throw new Error('Cannot add your own identity as a contact');
  const existing = state.contacts.find(value => value.sign_pk === sign);
  if (existing) {
    if (existing.dh_pk !== dh) throw new Error('Contact identity key changed');
    if (typeof args.relay === 'string') { if (!isValidI2pAddress(args.relay)) throw new Error('Invalid I2P destination'); existing.relay = args.relay; }
    return existing;
  }
  if (state.contacts.length >= 512) throw new Error('Contact limit reached');
  const onion = text(args, 'onion'); const relay = typeof args.relay === 'string' ? args.relay : null;
  if (!isValidI2pAddress(onion) || (relay !== null && !isValidI2pAddress(relay))) throw new Error('Invalid I2P destination');
  const contact: Contact = { id: allocate(state), sign_pk: sign, dh_pk: dh, onion, relay, name: text(args, 'name').slice(0, 96), trust: 0, created_at: Date.now(), last_seen: null, is_bot: false, pinned_at: null, last_message_at: null, agent_granted: false, request };
  state.contacts.push(contact);
  log.info('contact_add', `contact ${contact.id} (${request})`);
  return contact;
}

export function getContact(state: BrowserState, id: number): Contact {
  const contact = state.contacts.find(value => value.id === id); if (!contact) throw new Error('Contact not found'); return contact;
}

export function removeContact(state: BrowserState, log: ContactLog, id: number): void {
  const contact = getContact(state, id);
  log.info('contact_remove', `contact ${contact.id}`);
  const ids = new Set(state.messages.filter(value => value.contact_id === id).map(value => value.id));
  state.contacts = state.contacts.filter(value => value.id !== id);
  state.messages = state.messages.filter(value => value.contact_id !== id);
  state.attachments = state.attachments.filter(value => !ids.has(value.message_id));
  state.outbox = state.outbox.filter(value => value.peer !== contact.sign_pk);
}
