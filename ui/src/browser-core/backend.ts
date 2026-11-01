import { BrowserEngine, contact_qr_svg, type BrowserNetwork } from '../../../browser-router/pkg/gipny_browser_router.js';
import type { BootStatus, Contact, CoreEvent, GroupMember, Message, PendingAttachment, RelayInfo } from '../api';
import { isValidI2pAddress } from '../contact-card';
import { ProfileStorage, listProfiles, removeProfile, profileName } from './storage';
import { BrowserRelay } from './relay';
import {
  allocate, base64, hex, loadState, newState, payload, sameScope, unbase64, unhex, visible,
  type BrowserState, type Outbox, type PublicIdentity, type StoredMessage, type WireGroup, type WirePayload,
} from './model';

type Args = Record<string, unknown>;
type EventSink = (event: string, payload: unknown) => void;
const text = (args: Args, key: string): string => { if (typeof args[key] !== 'string') throw new Error(`Missing ${key}`); return args[key] as string; };
const number = (args: Args, key: string): number => { const n = args[key]; if (!Number.isSafeInteger(n) || (n as number) < 0) throw new Error(`Invalid ${key}`); return n as number; };
const reason = (error: unknown): string => String(error instanceof Error ? error.message : error).slice(0, 500);

/** The worker's public state and command adapter. Private material stays in
 * BrowserEngine. Only a completed OPFS flush advances a crypto transaction. */
export class BrowserBackend {
  private engine: BrowserEngine | null = null;
  private storage: ProfileStorage | null = null;
  private state: BrowserState | null = null;
  private identity: PublicIdentity | null = null;
  private queue: Promise<unknown> = Promise.resolve();
  private relay: BrowserRelay | null = null;
  private network: BrowserNetwork | null = null;
  private generation = 0;
  private closing = false;
  private pumping = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private hosted: RelayInfo['hosted'] = { state: 'off' };
  private networkConfig: Record<string, string> | null = null;
  constructor(private emit: EventSink) {}

  configure(config: Record<string, string> | null): void { this.networkConfig = config; }
  private require(): { engine: BrowserEngine; state: BrowserState; identity: PublicIdentity } {
    if (this.closing || !this.engine || !this.state || !this.identity) throw new Error('Profile is locked');
    return { engine: this.engine, state: this.state, identity: this.identity };
  }
  private event(value: CoreEvent): void { this.emit('core_event', value); }
  private boot(stage: BootStatus['stage'], state: BootStatus['state'], detail: string): void { this.emit('boot_status', { stage, state, detail }); }
  private info(): RelayInfo { return { mode: 'builtin', external: '', hosted: this.hosted, dial: { attempts: 0, last_error: this.hosted.state === 'failed' ? this.hosted.reason : null, unreachable_checks: 0, unreachable_error: null } }; }
  private relayChanged(): void { this.event({ RelayInfoChanged: { info: this.info() } }); }

  private transact<T>(action: (state: BrowserState, engine: BrowserEngine) => T): Promise<T> {
    const run = async (): Promise<T> => {
      const { engine, state } = this.require();
      const storage = this.storage!;
      const backup = structuredClone(state);
      engine.begin();
      let writing = false;
      try {
        const result = action(state, engine);
        const ciphertext = engine.seal(JSON.stringify(state));
        const next = engine.pending_generation();
        writing = true;
        await storage.persist(ciphertext, next);
        engine.commit();
        return result;
      } catch (error) {
        if (writing) {
          // The new slot may already be durable. Continuing the old ratchet
          // would reuse message keys; require a fresh read of the two slots.
          this.closeNow();
          this.emit('browser_fault', 'Profile storage failed; unlock again to recover the last complete transaction');
        } else {
          engine.rollback();
          this.state = backup;
        }
        throw error;
      }
    };
    const pending = this.queue.then(run);
    this.queue = pending.catch(() => {});
    return pending;
  }

  /** Stop network immediately. In-progress writes are synchronous flushes in
   * this worker; no long network operation is held inside the state queue. */
  closeNow(): void {
    this.closing = true;
    this.generation++;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.relay?.close(); this.relay = null;
    this.network?.close(); this.network = null;
    this.engine?.close(); this.engine?.free(); this.engine = null;
    this.storage?.close(); this.storage = null;
    this.state = null; this.identity = null;
    this.hosted = { state: 'off' };
  }

  private async open(args: Args, create: boolean, passkey = false): Promise<null> {
    if (this.engine || this.storage) throw new Error('Lock the current profile first');
    this.closing = false;
    const opening = this.generation;
    const profile = profileName(text(args, 'profile'));
    if (create && (args.duressPass || args.maxAttempts)) throw new Error('Browser profiles do not support duress passwords or destructive attempt counters');
    this.boot('vault', 'active', create ? 'Creating encrypted browser profile' : 'Opening encrypted browser profile');
    const storage = await ProfileStorage.open(profile, create);
    if (opening !== this.generation || this.closing) { storage.close(); throw new Error('Profile opening was cancelled'); }
    let engine: BrowserEngine | null = null;
    try {
      const [a, b] = storage.readSlots();
      if (create) engine = BrowserEngine.create(profile, text(args, 'pass'));
      else if (passkey) engine = BrowserEngine.open_passkey(profile, new Uint8Array(args.credentialId as number[]), new Uint8Array(args.prf as number[]), a, b);
      else engine = BrowserEngine.open_password(profile, text(args, 'pass'), a, b);
      const state = create ? newState(text(args, 'displayName')) : loadState(engine.take_metadata());
      this.engine = engine; this.storage = storage; this.state = state;
      this.identity = JSON.parse(engine.public_info()) as PublicIdentity;
      if (create) await this.transact(() => {});
      this.boot('vault', 'done', 'Profile opened; keys remain in the WASM worker');
      const attempt = ++this.generation;
      void this.startNetwork(attempt);
      return null;
    } catch (error) {
      if (this.engine === engine && engine) this.closeNow();
      else { engine?.close(); engine?.free(); storage.close(); }
      this.boot('vault', 'failed', reason(error));
      throw error;
    } finally {
      if (Array.isArray(args.prf)) (args.prf as number[]).fill(0);
    }
  }

  private async startNetwork(attempt: number): Promise<void> {
    const engine = this.engine!;
    this.hosted = { state: 'starting' }; this.relayChanged();
    try {
      if (!this.networkConfig) throw new Error('Network service is not configured; saved chats are available offline');
      const network = await engine.start_network(JSON.stringify(this.networkConfig), (raw: string) => {
        if (attempt !== this.generation) return;
        try { this.emit('boot_status', JSON.parse(raw)); } catch { /* Reject malformed progress, never invent readiness. */ }
      });
      if (attempt !== this.generation || this.closing) { network.close(); network.free(); return; }
      this.network = network;
      this.relay = new BrowserRelay(engine, network, this.identity!, (peer, blob) => this.receive(peer, blob), error => this.networkFailed(error));
      this.relay.start();
      this.hosted = { state: 'ready', address: this.identity!.onion };
      this.boot('core', 'done', 'In-process client destination is ready');
      this.boot('relay', 'done', 'Encrypted owner-only mailbox is accepting I2P streams');
      this.boot('dht', 'skipped', 'Contact cards supply relay destinations');
      this.relayChanged();
      this.schedule(0);
    } catch (error) { if (attempt === this.generation && !this.closing) this.networkFailed(reason(error)); }
  }
  private networkFailed(error: string): void {
    this.relay?.close(); this.relay = null;
    this.network?.close(); this.network = null;
    this.hosted = { state: 'failed', reason: error };
    this.boot('relay', 'failed', error); this.relayChanged();
  }
  private schedule(delay = 1000): void {
    if (this.closing || !this.relay) return;
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => { this.timer = null; void this.pump(); }, delay);
  }
  private enqueue(state: BrowserState, peer: string, message: WirePayload, row: number | null): void {
    if (state.outbox.length >= 2048) throw new Error('Outbox is full');
    state.outbox.push({ key: crypto.randomUUID(), peer, row, payload: message, blob: null, at: 0, attempts: 0, deposited: false });
  }
  private async pump(): Promise<void> {
    if (this.pumping || !this.relay || !this.engine || !this.state) return;
    this.pumping = true;
    const attempt = this.generation;
    try {
      const eligible = this.state.outbox.filter(item => Date.now() - item.at >= Math.min(60000, 2000 * 2 ** Math.min(item.attempts, 5))).slice(0, 8);
      for (const item of eligible) {
        if (attempt !== this.generation || !this.relay) break;
        const contact = this.state?.contacts.find(value => value.sign_pk === item.peer);
        if (!contact) continue;
        const address = contact.relay || contact.onion;
        try {
          // Bundle lookup and connect run outside the persistence queue.
          const bundle = !item.blob && !this.engine!.has_session(item.peer) ? await this.relay.bundle(address, item.peer) : new Uint8Array();
          await this.transact((state, engine) => {
            const current = state.outbox.find(value => value.key === item.key);
            if (!current) return;
            if (!current.blob) {
              current.blob = [...engine.encrypt(contact.sign_pk, contact.dh_pk, bundle, JSON.stringify(current.payload))];
              current.payload = null;
            }
            current.at = Date.now(); current.attempts++;
          });
          const current = this.state?.outbox.find(value => value.key === item.key);
          if (!current?.blob || !this.relay || attempt !== this.generation) continue;
          await this.relay.deposit(address, current.peer, current.blob);
          let sent: number | null = null;
          await this.transact(state => {
            const deposited = state.outbox.find(value => value.key === item.key);
            if (!deposited) return;
            deposited.deposited = true;
            if (deposited.row === null) state.outbox = state.outbox.filter(value => value.key !== item.key);
            else if (state.outbox.filter(value => value.row === deposited.row).every(value => value.deposited)) {
              const row = state.messages.find(value => value.id === deposited.row);
              if (row) { row.sent = true; sent = row.id; }
            }
          });
          if (sent !== null) this.event({ MessageSent: { message_id: sent } });
        } catch (error) {
          if (attempt !== this.generation || !this.state) break;
          await this.transact(state => { const current = state.outbox.find(value => value.key === item.key); if (current) { current.at = Date.now(); current.attempts = Math.max(1, current.attempts); } });
          this.event({ ContactReachability: { contact_id: contact.id, unreachable: true } });
          // Keep the exact ciphertext for a later retry, including restart.
          if (item.row !== null) this.event({ MessageFailed: { message_id: item.row, reason: reason(error) } });
        }
      }
    } finally { this.pumping = false; this.schedule(1000); }
  }

  private async receive(peer: string, bytes: Uint8Array): Promise<void> {
    const digestInput = new Uint8Array(32 + bytes.length); digestInput.set(unhex(peer)); digestInput.set(bytes, 32);
    const hash = hex(new Uint8Array(await crypto.subtle.digest('SHA-256', digestInput)));
    const events: CoreEvent[] = [];
    await this.transact((state, engine) => {
      const duplicate = state.seen.find(value => value.hash === hash && value.peer === peer);
      if (duplicate) { if (duplicate.ack) this.enqueue(state, peer, duplicate.ack, null); return; }
      let contact = state.contacts.find(value => value.sign_pk === peer);
      const decoded = JSON.parse(engine.decrypt(peer, contact?.dh_pk || '', bytes)) as { sign_pk: string; dh_pk: string; payload: WirePayload };
      if (decoded.sign_pk !== peer) throw new Error('Envelope sender mismatch');
      const incoming = decoded.payload;
      this.validatePayload(incoming);
      if (!contact) {
        if (!incoming.relay_address || !isValidI2pAddress(incoming.relay_address)) throw new Error('New contact has no valid reply address');
        contact = this.addContact(state, { signPk: peer, dhPk: decoded.dh_pk, onion: incoming.relay_address, relay: incoming.relay_address, name: incoming.sender_name || peer.slice(0, 12) }, 'incoming');
        events.push({ ContactRequest: { contact_id: contact.id } });
      }
      if (incoming.relay_address && isValidI2pAddress(incoming.relay_address)) contact.relay = incoming.relay_address;
      contact.last_seen = incoming.sent_at;
      if (contact.request === 'outgoing') { contact.request = 'none'; events.push({ ContactUpdated: { contact_id: contact.id } }); }
      let groupId: string | null = null;
      if (incoming.group) groupId = this.ingestGroup(state, incoming.group, peer);
      const contactId = groupId ? null : contact.id;
      const own = this.identity!;
      const ack = incoming.origin_msg_id > 0 && !incoming.ack_for ? payload(own, state.displayName) : null;
      if (ack) { ack.ack_for = incoming.origin_msg_id; ack.group = incoming.group; }
      if (incoming.ack_for !== null) {
        const row = state.messages.find(value => value.outgoing && value.origin === incoming.ack_for && sameScope(value, contactId, groupId));
        if (row) {
          state.outbox = state.outbox.filter(value => !(value.row === row.id && value.peer === peer));
          if (!state.outbox.some(value => value.row === row.id)) { row.sent = true; row.delivered = true; events.push({ MessageDelivered: { message_id: row.id } }); }
        }
      } else if (incoming.typing !== null) {
        if (contact.request !== 'incoming') events.push({ Typing: { contact_id: contactId, group_id: groupId, sender_sign_pk: groupId ? peer : null, typing: incoming.typing } });
      } else if (incoming.dm?.kind === 1 && groupId === null) {
        this.applyDeletion(state, contact, incoming, peer, events);
      } else if (incoming.dm?.kind === 2) {
        // An authenticated deletion acknowledgement never creates chat text.
      } else if (incoming.wipe === true && groupId === null) {
        this.removeContact(state, contact.id); events.push({ ContactWiped: { contact_id: contact.id, name: contact.name } });
      } else if (incoming.edit_of !== null) {
        const row = state.messages.find(value => !value.outgoing && value.author === peer && value.origin === incoming.edit_of && sameScope(value, contactId, groupId));
        if (row && visible(row)) { row.body = incoming.body; row.buttons = incoming.buttons; events.push({ MessageEdited: { message_id: row.id, body: row.body, buttons: row.buttons || null } }); }
      } else if (incoming.pin) {
        const row = state.messages.find(value => value.author === hex(incoming.pin!.sender_sign_pk) && value.origin === incoming.pin!.origin_msg_id && sameScope(value, contactId, groupId));
        if (row && visible(row)) {
          state.pinnedMessages = state.pinnedMessages.filter(value => value !== row.id);
          if (!incoming.pin.unpin) state.pinnedMessages.push(row.id);
          events.push(incoming.pin.unpin ? { MessageUnpinned: { contact_id: contactId, group_id: groupId, message_id: row.id } } : { MessagePinned: { contact_id: contactId, group_id: groupId, message_id: row.id } });
        }
      } else if (incoming.origin_msg_id > 0 && !incoming.callback_data && !incoming.console && !incoming.files.length && incoming.file_chunk === null) {
        const existing = state.messages.some(value => value.author === peer && value.origin === incoming.origin_msg_id && sameScope(value, contactId, groupId));
        const deleted = state.tombstones.includes(`${peer}:${incoming.origin_msg_id}:${incoming.sent_at}`);
        if (!existing && !deleted) {
          const row: StoredMessage = { id: allocate(state), origin: incoming.origin_msg_id, author: peer, contact_id: contactId, group_id: groupId, sender_sign_pk: groupId ? peer : null, outgoing: false, body: incoming.body, sent_at: incoming.sent_at, sent: true, delivered: true, read: false, expires_at: incoming.ttl_ms === null ? null : incoming.sent_at + incoming.ttl_ms, reply_to: null, buttons: incoming.buttons, dm_seq: state.messages.filter(value => sameScope(value, contactId, groupId)).length + 1 };
          if (incoming.reply_to) row.reply_to = state.messages.find(value => value.author === hex(incoming.reply_to!.sender_sign_pk) && value.origin === incoming.reply_to!.origin_msg_id && sameScope(value, contactId, groupId))?.id || null;
          state.messages.push(row); contact.last_message_at = row.sent_at;
          for (const attachment of incoming.attachments) state.attachments.push({ id: allocate(state), message_id: row.id, name: attachment.name, size: attachment.data.length, data: base64(attachment.data) });
          if (contact.request !== 'incoming' && visible(row)) events.push({ IncomingMessage: { contact_id: contactId, group_id: groupId, sender_sign_pk: row.sender_sign_pk, message_id: row.id, body: row.body, sent_at: row.sent_at, notify_sound: null, console_kind: null } });
        }
      }
      if (ack) this.enqueue(state, peer, ack, null);
      state.seen.push({ hash, peer, ack });
      // Preserve replay hashes for the profile's lifetime. Silently evicting
      // them could replay an old X3DH initial after its OTK-free fallback.
      if (state.seen.length > 50000) throw new Error('Profile replay history limit reached');
      events.push({ PeerSeen: { contact_id: contact.id, at_ms: incoming.sent_at } });
    });
    events.forEach(event => this.event(event));
    this.schedule(0);
  }

  private validatePayload(value: WirePayload): void {
    if (!Number.isSafeInteger(value.origin_msg_id) || value.origin_msg_id < 0 || !Number.isSafeInteger(value.sent_at) || typeof value.body !== 'string' || value.body.length > 128 * 1024 || !Array.isArray(value.attachments) || value.attachments.length > 16 || value.attachments.reduce((n, file) => n + file.data.length, 0) > 1024 * 1024 || value.attachments.some(file => file.name.length > 255) || (value.ttl_ms !== null && (!Number.isSafeInteger(value.ttl_ms) || value.ttl_ms < 0 || value.ttl_ms > 365 * 86400000))) throw new Error('Unsupported message payload');
    if (value.files.length || value.file_chunk !== null || value.file_ack !== null || value.file_cancel !== null || value.console !== null || value.callback_data !== null) throw new Error('This browser does not support this message control');
  }
  private addContact(state: BrowserState, args: Args, request: Contact['request'] = 'outgoing'): Contact {
    const sign = text(args, 'signPk').toLowerCase(); const dh = text(args, 'dhPk').toLowerCase();
    unhex(sign); unhex(dh);
    if (sign === this.identity!.sign_pk || state.contacts.some(value => value.sign_pk === sign)) throw new Error('Contact already exists');
    if (state.contacts.length >= 512) throw new Error('Contact limit reached');
    const onion = text(args, 'onion'); const relay = typeof args.relay === 'string' ? args.relay : null;
    if (!isValidI2pAddress(onion) || (relay !== null && !isValidI2pAddress(relay))) throw new Error('Invalid I2P destination');
    const contact: Contact = { id: allocate(state), sign_pk: sign, dh_pk: dh, onion, relay, name: text(args, 'name').slice(0, 96), trust: 0, created_at: Date.now(), last_seen: null, is_bot: false, pinned_at: null, last_message_at: null, agent_granted: false, request };
    state.contacts.push(contact); return contact;
  }
  private getContact(state: BrowserState, id: number): Contact { const contact = state.contacts.find(value => value.id === id); if (!contact) throw new Error('Contact not found'); return contact; }
  private removeContact(state: BrowserState, id: number): void {
    const contact = this.getContact(state, id);
    const ids = new Set(state.messages.filter(value => value.contact_id === id).map(value => value.id));
    state.contacts = state.contacts.filter(value => value.id !== id);
    state.messages = state.messages.filter(value => value.contact_id !== id);
    state.attachments = state.attachments.filter(value => !ids.has(value.message_id));
    state.outbox = state.outbox.filter(value => value.peer !== contact.sign_pk);
  }
  private ingestGroup(state: BrowserState, wire: WireGroup, peer: string): string {
    if (wire.id.length !== 32 || wire.name.length > 128 || wire.members.length > 64 || wire.members.length < 2) throw new Error('Invalid group');
    const id = hex(wire.id);
    const members: GroupMember[] = wire.members.map(member => {
      if (member.sign_pk.length !== 32 || member.dh_pk.length !== 32 || !isValidI2pAddress(member.onion) || member.name.length > 96) throw new Error('Invalid group member');
      return { sign_pk: hex(member.sign_pk), dh_pk: hex(member.dh_pk), onion: member.onion, name: member.name, is_self: hex(member.sign_pk) === this.identity!.sign_pk };
    });
    if (new Set(members.map(member => member.sign_pk)).size !== members.length || !members.some(member => member.is_self) || !members.some(member => member.sign_pk === peer)) throw new Error('Group sender is not a member');
    let group = state.groups.find(value => value.id === id);
    if (group && !group.members.some(member => member.sign_pk === peer)) throw new Error('Uninvited group sender');
    if (!group) { group = { id, name: wire.name, members, created_at: Date.now(), pinned_at: null, last_message_at: Date.now() }; state.groups.push(group); }
    // Existing membership is pinned locally. A message cannot silently replace
    // known identity keys or invite a new sender into an established group.
    return id;
  }
  private groupWire(state: BrowserState, id: string): WireGroup {
    const group = state.groups.find(value => value.id === id); if (!group) throw new Error('Group not found');
    return { id: unhex(id), name: group.name, members: group.members.map(member => ({ sign_pk: unhex(member.sign_pk), dh_pk: unhex(member.dh_pk), onion: member.onion, name: member.name })) };
  }
  private applyDeletion(state: BrowserState, contact: Contact, incoming: WirePayload, peer: string, events: CoreEvent[]): void {
    const reference = incoming.dm!.reference;
    if (incoming.origin_msg_id !== 0 || incoming.body || incoming.attachments.length || incoming.typing !== null || incoming.edit_of !== null || incoming.pin !== null || incoming.wipe !== null || incoming.ack_for !== null || incoming.reply_to !== null || hex(reference.author) !== peer) throw new Error('Invalid deletion control');
    const key = `${peer}:${reference.origin}:${reference.sent_at}`;
    if (!state.tombstones.includes(key)) state.tombstones.push(key);
    const row = state.messages.find(value => value.contact_id === contact.id && !value.outgoing && value.author === peer && value.origin === reference.origin && value.sent_at === reference.sent_at);
    if (row) { row.body = ''; row.deleted_at = Date.now(); state.attachments = state.attachments.filter(value => value.message_id !== row.id); events.push({ MessageDeleted: { contact_id: contact.id, message_id: row.id, deleted_at: row.deleted_at, pending: false } }); }
    const ack = payload(this.identity!, state.displayName); ack.dm = { kind: 2, reference };
    this.enqueue(state, peer, ack, null);
  }

  private send(state: BrowserState, args: Args, group: boolean): number {
    const contact = group ? null : this.getContact(state, number(args, 'contactId'));
    if (contact?.request === 'incoming') throw new Error('Accept the contact request first');
    const groupId = group ? text(args, 'groupId') : null;
    const body = text(args, 'body');
    const attachments = (args.attachments || []) as PendingAttachment[];
    if (body.length > 128 * 1024 || attachments.length > 16) throw new Error('Message size limit');
    const letter = payload(this.identity!, state.displayName);
    letter.attachments = attachments.map(file => ({ name: file.name.slice(0, 255), data: unbase64(file.data) }));
    if (letter.attachments.reduce((n, file) => n + file.data.length, 0) > 1024 * 1024) throw new Error('Browser attachments are limited to 1 MiB per message');
    letter.origin_msg_id = allocate(state); letter.body = body;
    const ttl = args.ttlSecs;
    if (ttl != null && (!Number.isSafeInteger(ttl) || (ttl as number) < 0 || (ttl as number) > 365 * 86400)) throw new Error('Invalid message lifetime');
    letter.ttl_ms = typeof ttl === 'number' ? ttl * 1000 : null;
    if (groupId) letter.group = this.groupWire(state, groupId);
    if (typeof args.replyTo === 'number') {
      const reply = state.messages.find(value => value.id === args.replyTo && sameScope(value, contact?.id || null, groupId));
      if (!reply) throw new Error('Reply target is not in this conversation');
      letter.reply_to = { sender_sign_pk: unhex(reply.author), origin_msg_id: reply.origin };
    }
    if (!groupId) letter.dm = { kind: 0, reference: { author: unhex(this.identity!.sign_pk), origin: letter.origin_msg_id, sent_at: letter.sent_at } };
    const row: StoredMessage = { id: letter.origin_msg_id, origin: letter.origin_msg_id, author: this.identity!.sign_pk, contact_id: contact?.id || null, group_id: groupId, sender_sign_pk: groupId ? this.identity!.sign_pk : null, outgoing: true, body, sent_at: letter.sent_at, sent: false, delivered: false, read: true, expires_at: letter.ttl_ms === null ? null : letter.sent_at + letter.ttl_ms, reply_to: typeof args.replyTo === 'number' ? args.replyTo : null, dm_seq: state.messages.filter(value => sameScope(value, contact?.id || null, groupId)).length + 1 };
    state.messages.push(row);
    for (const attachment of attachments) { const data = unbase64(attachment.data); state.attachments.push({ id: allocate(state), message_id: row.id, name: attachment.name.slice(0, 255), size: data.length, data: attachment.data }); }
    const peers = groupId ? state.groups.find(value => value.id === groupId)!.members.filter(member => !member.is_self).map(member => {
      let recipient = state.contacts.find(value => value.sign_pk === member.sign_pk);
      if (!recipient) recipient = this.addContact(state, { signPk: member.sign_pk, dhPk: member.dh_pk, onion: member.onion, name: member.name }, 'none');
      return recipient;
    }) : [contact!];
    for (const peer of peers) { this.enqueue(state, peer.sign_pk, structuredClone(letter), row.id); peer.last_message_at = row.sent_at; }
    if (groupId) state.groups.find(value => value.id === groupId)!.last_message_at = row.sent_at;
    return row.id;
  }

  async invoke(command: string, args: Args = {}): Promise<unknown> {
    if (command === 'init') { this.configure(args.config as Record<string, string> | null); return null; }
    if (command === 'list_profiles') return listProfiles();
    if (command === 'delete_profile') { if (this.engine) throw new Error('Lock the profile first'); await removeProfile(text(args, 'profile')); return null; }
    if (command === 'vault_status') return { exists: this.storage?.name === args.profile || (await listProfiles()).includes(text(args, 'profile')), unlocked: this.storage?.name === args.profile && this.engine !== null };
    if (command === 'vault_create') return this.open(args, true);
    if (command === 'vault_unlock') return this.open(args, false);
    if (command === 'vault_unlock_passkey') return this.open(args, false, true);
    if (command === 'vault_lock') { this.closeNow(); return null; }
    if (command === 'passkey_entries') {
      const storage = await ProfileStorage.open(text(args, 'profile'), false);
      try { const [a, b] = storage.readSlots(); return JSON.parse(BrowserEngine.passkey_entries(a, b)); }
      finally { storage.close(); }
    }
    if (command === 'prewarm_network') return null;
    if (command === 'prewarm_status') return 'off';
    if (command === 'current_version') return '0.4.19-browser';
    if (command === 'update_configured' || command === 'get_auto_update' || command === 'update_installs_itself' || command === 'update_asks_for_root') return false;
    if (command === 'check_update' || command === 'get_agent_mode') return null;
    if (command === 'get_relay_info') return this.info();
    if (command === 'get_relay_address') return this.identity?.onion || '';
    if (command === 'get_router_settings') return { transit: 'frugal', yggdrasil: 'off' };
    if (command === 'get_dht_status') return { peers: 0, items: 0, bytes: 0, joined: false, stores: false, seeds: 0 };
    if (command === 'list_unreachable_contacts') return [];
    if (command === 'get_attachment_privacy') return true;
    const { state, engine, identity } = this.require();
    if (command === 'my_card') return { sign_pk: identity.sign_pk, dh_pk: identity.dh_pk };
    if (command === 'my_bundle') return JSON.parse(engine.bundle_info());
    if (command === 'qr_svg') return contact_qr_svg(text(args, 'text'));
    if (command === 'my_onion') return identity.destination;
    if (command === 'my_b32') return identity.onion;
    if (command === 'my_fingerprint') return identity.fingerprint;
    if (command === 'get_display_name') return state.displayName;
    if (command === 'link_stats') return { lane: 'normal', our_hops: 2, their_hops: 2, padded: true, route: this.relay ? 'relay' : 'none', rtt_ms: null };
    if (command === 'verify_passphrase') { engine.verify_password(text(args, 'pass')); return null; }
    if (command === 'list_contacts') return state.contacts;
    if (command === 'get_contact') return this.getContact(state, number(args, 'id'));
    if (command === 'list_groups') return state.groups;
    if (command === 'list_group_members') return state.groups.find(value => value.id === args.groupId)?.members || [];
    if (command === 'list_muted') return state.muted;
    if (command === 'get_ui_data') return state.ui[text(args, 'key')] || null;
    if (command === 'list_messages' || command === 'list_group_messages') {
      const group = command === 'list_group_messages';
      const rows = state.messages.filter(value => sameScope(value, group ? null : number(args, 'contactId'), group ? text(args, 'groupId') : null) && visible(value) && (typeof args.beforeId !== 'number' || value.id < args.beforeId));
      return rows.slice(-Math.min(500, typeof args.limit === 'number' ? Math.max(1, args.limit) : 100));
    }
    if (command === 'unread_count' || command === 'group_unread_count') return state.messages.filter(value => !value.read && visible(value) && sameScope(value, command === 'unread_count' ? number(args, 'contactId') : null, command === 'group_unread_count' ? text(args, 'groupId') : null)).length;
    if (command === 'list_attachments') return state.attachments.filter(value => value.message_id === args.messageId).map(({ data: _, ...attachment }) => attachment);
    if (command === 'load_attachment') { const attachment = state.attachments.find(value => value.id === args.attachmentId); if (!attachment) throw new Error('Attachment not found'); return attachment.data; }
    if (command === 'files_progress') return [];
    if (command === 'list_pinned_contact' || command === 'list_pinned_group') return state.messages.filter(value => state.pinnedMessages.includes(value.id) && visible(value) && sameScope(value, command === 'list_pinned_contact' ? number(args, 'contactId') : null, command === 'list_pinned_group' ? text(args, 'groupId') : null));
    if (command === 'search_messages') {
      const query = text(args, 'query').toLocaleLowerCase();
      return state.messages.filter(value => visible(value) && value.body.toLocaleLowerCase().includes(query) && (args.contactId == null || value.contact_id === args.contactId) && (args.groupId == null || value.group_id === args.groupId)).slice(-Math.min(500, typeof args.limit === 'number' ? args.limit : 100)).reverse().map(message => ({ message, contact_id: message.contact_id, group_id: message.group_id, contact_name: state.contacts.find(value => value.id === message.contact_id)?.name || null, group_name: state.groups.find(value => value.id === message.group_id)?.name || null }));
    }
    if (command === 'list_media_contact' || command === 'list_media_group') {
      const messages = state.messages.filter(value => visible(value) && sameScope(value, command === 'list_media_contact' ? number(args, 'contactId') : null, command === 'list_media_group' ? text(args, 'groupId') : null));
      return state.attachments.filter(value => messages.some(message => message.id === value.message_id)).slice(-100).map(({ data: _, ...attachment }) => ({ ...attachment, sent_at: messages.find(message => message.id === attachment.message_id)!.sent_at }));
    }
    if (command === 'message_position') { const index = state.messages.filter(value => visible(value) && sameScope(value, typeof args.contactId === 'number' ? args.contactId : null, typeof args.groupId === 'string' ? args.groupId : null)).findIndex(value => value.id === args.messageId); return index < 0 ? null : index; }
    const result = await this.transact((next, core) => this.mutate(command, args, next, core));
    this.schedule(0);
    return result;
  }

  private mutate(command: string, args: Args, state: BrowserState, engine: BrowserEngine): unknown {
    if (command === 'change_passphrase') { engine.change_password(text(args, 'old'), text(args, 'new')); return null; }
    if (command === 'enroll_passkey') { try { engine.enroll_passkey(new Uint8Array(args.credentialId as number[]), text(args, 'rpId'), new Uint8Array(args.salt as number[]), new Uint8Array(args.prf as number[])); } finally { if (Array.isArray(args.prf)) (args.prf as number[]).fill(0); } return null; }
    if (command === 'set_display_name') { state.displayName = text(args, 'name').slice(0, 64); return null; }
    if (command === 'set_ui_data') { const json = text(args, 'json'); if (json.length > 256 * 1024) throw new Error('UI preference size limit'); JSON.parse(json); state.ui[text(args, 'key')] = json; return null; }
    if (command === 'set_muted') { const key = text(args, 'targetKey'); state.muted = state.muted.filter(value => value !== key); if (args.muted === true) state.muted.push(key); return null; }
    if (command === 'add_contact') return this.addContact(state, args);
    if (command === 'update_contact') { const contact = this.getContact(state, number(args, 'id')); contact.name = text(args, 'name').slice(0, 96); contact.trust = number(args, 'trust'); return null; }
    if (command === 'accept_contact_request') { this.getContact(state, number(args, 'id')).request = 'none'; return null; }
    if (command === 'decline_contact_request' || command === 'delete_contact') {
      if (args.forBoth) throw new Error('Browser contact removal is local; delete messages individually to notify the peer');
      this.removeContact(state, number(args, 'id')); return null;
    }
    if (command === 'send_message' || command === 'send_group_message') return this.send(state, args, command === 'send_group_message');
    if (command === 'mark_read' || command === 'mark_group_read') { for (const row of state.messages) if (sameScope(row, command === 'mark_read' ? number(args, 'contactId') : null, command === 'mark_group_read' ? text(args, 'groupId') : null)) row.read = true; return null; }
    if (command === 'send_edit' || command === 'send_edit_group') {
      const group = command === 'send_edit_group';
      const row = state.messages.find(value => value.id === args.messageId && value.outgoing && visible(value) && sameScope(value, group ? null : number(args, 'contactId'), group ? text(args, 'groupId') : null));
      if (!row) throw new Error('Message cannot be edited');
      const letter = payload(this.identity!, state.displayName); letter.edit_of = row.origin; letter.body = text(args, 'newBody');
      if (letter.body.length > 128 * 1024) throw new Error('Message size limit');
      if (row.group_id) letter.group = this.groupWire(state, row.group_id);
      const recipients = row.group_id ? state.groups.find(value => value.id === row.group_id)!.members.filter(value => !value.is_self).map(value => value.sign_pk) : [this.getContact(state, row.contact_id!).sign_pk];
      recipients.forEach(peer => this.enqueue(state, peer, structuredClone(letter), null));
      row.body = letter.body; return null;
    }
    if (command === 'delete_message') {
      const row = state.messages.find(value => value.id === args.id); if (!row) return null;
      if (state.outbox.some(value => value.row === row.id) && !row.sent) throw new Error('Wait for this message to reach the recipient before deleting it');
      if (row.outgoing && row.contact_id !== null) {
        const letter = payload(this.identity!, state.displayName); letter.dm = { kind: 1, reference: { author: unhex(row.author), origin: row.origin, sent_at: row.sent_at } };
        this.enqueue(state, this.getContact(state, row.contact_id).sign_pk, letter, null);
      }
      row.body = ''; row.deleted_at = Date.now(); row.delete_pending = false;
      state.attachments = state.attachments.filter(value => value.message_id !== row.id);
      // A locally cancelled, never deposited message must not later send.
      state.outbox = state.outbox.filter(value => value.row !== row.id);
      return null;
    }
    if (command === 'clear_dm_chat' || command === 'clear_message_history') {
      const ids = new Set(state.messages.filter(value => command === 'clear_message_history' || value.contact_id === args.contactId).map(value => value.id));
      state.messages = state.messages.filter(value => !ids.has(value.id)); state.attachments = state.attachments.filter(value => !ids.has(value.message_id)); for (const letter of state.outbox) if (letter.row !== null && ids.has(letter.row)) letter.row = null; return null;
    }
    if (command === 'create_group') {
      const ids = args.memberContactIds as number[];
      if (!Array.isArray(ids) || ids.length < 1 || ids.length > 63 || new Set(ids).size !== ids.length) throw new Error('Invalid group members');
      const members: GroupMember[] = ids.map(id => { const contact = this.getContact(state, id); return { sign_pk: contact.sign_pk, dh_pk: contact.dh_pk, onion: contact.onion, name: contact.name, is_self: false }; });
      members.push({ sign_pk: this.identity!.sign_pk, dh_pk: this.identity!.dh_pk, onion: this.identity!.onion, name: state.displayName, is_self: true });
      const id = hex(crypto.getRandomValues(new Uint8Array(32))); state.groups.push({ id, name: text(args, 'name').slice(0, 128), members, created_at: Date.now(), pinned_at: null, last_message_at: null }); return id;
    }
    if (command === 'delete_group') { const id = text(args, 'groupId'); const rows = new Set(state.messages.filter(value => value.group_id === id).map(value => value.id)); state.groups = state.groups.filter(value => value.id !== id); state.messages = state.messages.filter(value => value.group_id !== id); state.outbox = state.outbox.filter(value => value.row === null || !rows.has(value.row)); state.attachments = state.attachments.filter(value => !rows.has(value.message_id)); return null; }
    if (command === 'pin_chat' || command === 'unpin_chat') { const target = args.contactId != null ? this.getContact(state, number(args, 'contactId')) : state.groups.find(value => value.id === args.groupId); if (!target) throw new Error('Conversation not found'); target.pinned_at = command === 'pin_chat' ? Date.now() : null; return null; }
    if (command === 'set_attachment_privacy') { if (!args.enabled) throw new Error('Attachment privacy cannot be disabled in the browser'); return null; }
    if (command === 'set_relay_mode') { if (args.mode !== 'builtin') throw new Error('This browser uses its own I2P destination'); return null; }
    if (command === 'set_auto_update' && args.enabled === false) return null;
    if (command === 'send_typing') {
      if (typeof args.contactId !== 'number') return null;
      const contact = this.getContact(state, args.contactId);
      if (contact.request !== 'none') return null;
      const letter = payload(this.identity!, state.displayName); letter.typing = args.typing === true;
      this.enqueue(state, contact.sign_pk, letter, null); return null;
    }
    throw new Error(`The browser core does not support ${command}`);
  }
}
