import { BrowserEngine, AttachmentHasher, attachment_passed_name, attachment_passes_through, clean_attachment, contact_qr_svg, envelope_is_initial, public_network_metrics, type BrowserNetwork } from '../../../browser-router/pkg/gipny_browser_router.js';
import type { BootStatus, Contact, CoreEvent, GroupMember, Message, PendingAttachment, RelayInfo } from '../api';
import { isValidI2pAddress } from '../contact-card';
import { ProfileStorage, listProfiles, removeProfile, profileName } from './storage';
import { BrowserRelay } from './relay';
import { acknowledgeBrowserDeletion, deletionEvent, forgetRequestObligations, queuePeerReport, queueDeletionControls, receiveDeletionControl } from './deletion';
import {
  acceptFileOffers, acceptPendingFiles, browserFileProgress, buildChunkLetter, cancelBrowserFiles, chunkSource, failOutgoingFile, failStalledFiles, FILE_CHUNK, fileReservations,
  fileWorkDue, finishFileVerification, forgetFlight, inlinePart, isRequestContent, offerOutgoingFile, pendingInlineAllowed, pruneFiles, queueFileChunks, receiveFileControl,
  repeatFileReceipt, resumeFiles, settleDeposit, validateFilePayload,
} from './files';
import { FileStorage, FILE_PREVIEW_LIMIT, MAX_FILE_BYTES, StorageBusy } from './file-storage';
import { ActionLog, LOG_LOCATION } from './log';
import {
  allocate, base64, canSendControls, clearHistory, dueLetters, expireHistory, expireResends, fromBase64, hex, inHistory, redactMessage, loadState, newState, payload, pruneMessageOutbox,
  queueLetter, queueReceipt, recentEnvelope, rememberEnvelope, rememberInitial, reopenForResend, replayedInitial, sameScope, unbase64, unhex, visible,
  type BrowserState, type Outbox, type PublicIdentity, type SeenEnvelope, type StoredBlob, type StoredMessage, type WireGroup, type WirePayload,
} from './model';

type Args = Record<string, unknown>;
type EventSink = (event: string, payload: unknown) => void;
type PreparedFile = { name: string; size: number; sha256: number[]; storage: StoredBlob };
const text = (args: Args, key: string): string => { if (typeof args[key] !== 'string') throw new Error(`Missing ${key}`); return args[key] as string; };
const number = (args: Args, key: string): number => { const n = args[key]; if (!Number.isSafeInteger(n) || (n as number) < 0) throw new Error(`Invalid ${key}`); return n as number; };
/** What the interface keeps for itself in the profile — the same keys as the
 *  native core's `UI_DATA_KEYS`, and nothing else. */
const UI_DATA_KEYS = new Set(['contact_folders', 'avatars', 'chat_prefs']);
const uiKey = (args: Args): string => { const key = text(args, 'key'); if (!UI_DATA_KEYS.has(key)) throw new Error('Unknown UI preference'); return key; };
/** Concurrent deposits; each holds at most one file part in memory. */
const SEND_STREAMS = 4;
/** Longer than any single network deadline: work this old is reported, at
 * most once per STALL_REPORT_MS. */
const STALL_MS = 120_000;
const STALL_REPORT_MS = 60_000;
const reason = (error: unknown): string => String(error instanceof Error ? error.message : error).slice(0, 500);

/** The worker's public state and command adapter. Private material stays in
 * BrowserEngine. Only a completed OPFS flush advances a crypto transaction. */
export class BrowserBackend {
  private engine: BrowserEngine | null = null;
  private storage: ProfileStorage | null = null;
  private files: FileStorage | null = null;
  private preparingFiles = false;
  private verifyingFiles = new Set<string>();
  private state: BrowserState | null = null;
  private identity: PublicIdentity | null = null;
  private queue: Promise<unknown> = Promise.resolve();
  private relay: BrowserRelay | null = null;
  private network: BrowserNetwork | null = null;
  private generation = 0;
  private closing = false;
  private pumping = false;
  /** Outbox keys being sent (at most SEND_STREAMS) and the step each is in. */
  private sending = new Map<string, { stage: string; at: number }>();
  /** When the running profile transaction started; for stall diagnostics. */
  private transactSince: number | null = null;
  private stallReported = 0;
  /** Reports in the current episode; a deposit starts a new one. */
  private stallReports = 0;
  private lastDeposit = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private sweepTimer: ReturnType<typeof setInterval> | null = null;
  private hosted: RelayInfo['hosted'] = { state: 'off' };
  private networkConfig: Record<string, string> | null = null;
  private relayProbes = { attempted: 0, completed: 0, failed: 0 };
  private relayAnswered = false;
  private verificationRetry = new Map<string, number>();
  private log = new ActionLog();
  constructor(private emit: EventSink) {
    this.log.info('worker_init', 'browser core worker created');
  }

  configure(config: Record<string, string> | null): void { this.networkConfig = config; }
  private require(): { engine: BrowserEngine; state: BrowserState; identity: PublicIdentity } {
    if (this.closing || !this.engine || !this.state || !this.identity) throw new Error('Profile is locked');
    return { engine: this.engine, state: this.state, identity: this.identity };
  }
  private event(value: CoreEvent): void { this.emit('core_event', value); }
  /** Log subjects are local contact ids: no names, keys or addresses. */
  private who(peer: string): string {
    const id = this.state?.contacts.find(contact => contact.sign_pk === peer)?.id;
    return id === undefined ? 'unknown sender' : `contact ${id}`;
  }
  private fileIds(state: BrowserState): Set<string> {
    return new Set([...state.attachments.flatMap(file => file.storage ? [file.storage.id] : []), ...state.transfers.flatMap(file => file.storage ? [file.storage.id] : [])]);
  }

  private async storeBytes(data: Uint8Array): Promise<StoredBlob> {
    const store = this.files!;
    const file = await store.create(data.length, FILE_CHUNK, fileReservations(this.require().state));
    try {
      for (let index = 0; index < Math.max(1, Math.ceil(data.length / FILE_CHUNK)); index++) await store.write(file, index, data.subarray(index * FILE_CHUNK, (index + 1) * FILE_CHUNK));
      return file;
    } catch (error) { await store.remove(file.id).catch(() => {}); throw error; }
    finally { data.fill(0); }
  }

  private async prepareFiles(args: Args): Promise<PreparedFile[]> {
    const inline = (args.attachments ?? []) as PendingAttachment[];
    const picked = (args.browserFiles ?? []) as File[];
    if (!Array.isArray(inline) || !Array.isArray(picked) || inline.length + picked.length > 16 || picked.some(file => !(file instanceof File))) throw new Error('Invalid attachments');
    if (this.preparingFiles && (inline.length || picked.length)) throw new Error('Another file is being prepared');
    if (!inline.length && !picked.length) return [];
    const { engine, state } = this.require();
    const store = this.files!;
    const sanitize = state.ui.attachment_privacy !== 'false';
    this.preparingFiles = true;
    const result: PreparedFile[] = [];
    const created: string[] = [];
    const active = (): void => { if (this.engine !== engine || this.closing) throw new Error('Profile is locked'); };
    try {
      const files: File[] = [...picked, ...inline.map(file => new File([new Uint8Array(unbase64(file.data, FILE_PREVIEW_LIMIT))], file.name))];
      for (let source of files) {
        active();
        if (source.size > MAX_FILE_BYTES || new TextEncoder().encode(source.name).length > 1024) throw new Error('File size limit is 2 GiB');
        let name = source.name;
        if (sanitize) {
          const head = new Uint8Array(await source.slice(0, 1024).arrayBuffer()); active();
          if (attachment_passes_through(name, head)) name = attachment_passed_name(name);
          else {
            if (source.size > FILE_PREVIEW_LIMIT) throw new Error('Metadata cleaning is limited to 32 MiB; convert this file or explicitly turn off attachment metadata cleaning');
            const bytes = new Uint8Array(await source.arrayBuffer()); active();
            let data: Uint8Array;
            try {
              const clean = clean_attachment(name, bytes);
              try { name = clean.name; data = clean.take_data(); } finally { clean.free(); }
            } finally { bytes.fill(0); }
            source = new File([data as BlobPart], name); data.fill(0);
          }
          head.fill(0);
        }
        const storage = await store.create(source.size, FILE_CHUNK, fileReservations(this.require().state)); created.push(storage.id);
        let hash: AttachmentHasher | null = new AttachmentHasher();
        try {
          for (let index = 0; index < Math.max(1, Math.ceil(source.size / FILE_CHUNK)); index++) {
            active();
            const data = new Uint8Array(await source.slice(index * FILE_CHUNK, (index + 1) * FILE_CHUNK).arrayBuffer()); active();
            try { hash.update(data); await store.write(storage, index, data); } finally { data.fill(0); }
          }
          const sha256 = [...hash.finish()]; hash = null;
          result.push({ name, size: source.size, sha256, storage });
        } finally { hash?.free(); }
      }
      active(); return result;
    } catch (error) {
      for (const id of created) await store.remove(id).catch(() => {});
      throw error;
    } finally { this.preparingFiles = false; }
  }

  private verifyReadyFiles(): void {
    if (!this.files || !this.engine || !this.state || this.closing) return;
    const engine = this.engine, store = this.files;
    for (const file of this.state.transfers) {
      if (this.verifyingFiles.size >= 2) break;
      const key = `${file.peer}:${hex(file.file_id)}`;
      if (file.status !== 'verifying' || !file.storage || this.verifyingFiles.has(key) || (this.verificationRetry.get(key) ?? 0) > Date.now()) continue;
      this.verifyingFiles.add(key);
      const storage = file.storage;
      const active = (): boolean => this.engine === engine && !this.closing && !!this.state?.transfers.some(f => f.peer === file.peer && hex(f.file_id) === hex(file.file_id) && f.status === 'verifying');
      void (async () => {
        let digest: string | null = null;
        let hash: AttachmentHasher | null = new AttachmentHasher();
        try {
          for (let index = 0; index < Math.max(1, Math.ceil(storage.size / storage.chunk_size)); index++) {
            if (!active()) return;
            const data = await store.read(storage, index);
            try { if (!active()) return; hash.update(data); } finally { data.fill(0); }
          }
          digest = hex(hash.finish()); hash = null;
        } catch { /* A corrupt/missing authenticated part fails the whole file. */ }
        finally { hash?.free(); }
        if (!active()) return;
        const events: CoreEvent[] = [];
        await this.transact(state => finishFileVerification(state, this.identity!, file.peer, hex(file.file_id), digest, events));
        if (this.engine !== engine) return;
        this.verificationRetry.delete(key);
        events.forEach(event => this.event(event)); this.schedule(0);
      })().catch(error => {
        if (this.engine === engine && !this.closing) {
          this.verificationRetry.set(key, Date.now() + 30_000);
          this.log.info('file_verification_failed', reason(error));
        }
      })
        .finally(() => { this.verifyingFiles.delete(key); if (this.engine === engine) this.verifyReadyFiles(); });
    }
  }

  private async attachmentPart(id: number, index: number): Promise<Uint8Array> {
    const { state, engine } = this.require();
    const attachment = state.attachments.find(file => file.id === id);
    if (!attachment || !state.messages.some(row => row.id === attachment.message_id && visible(row))) throw new Error('Attachment not found');
    const data = attachment.storage ? await this.files!.read(attachment.storage, index) : index === 0 ? new Uint8Array(unbase64(attachment.data ?? '', FILE_PREVIEW_LIMIT)) : new Uint8Array();
    if (this.engine !== engine || !this.state?.attachments.some(file => file.id === id) || !this.state.messages.some(row => row.id === attachment.message_id && visible(row))) { data.fill(0); throw new Error('Attachment is no longer available'); }
    return data;
  }

  private async previewAttachment(id: number): Promise<string> {
    const attachment = this.require().state.attachments.find(file => file.id === id);
    if (!attachment) throw new Error('Attachment not found');
    if (attachment.size > FILE_PREVIEW_LIMIT) throw new Error('This file is too large for a preview; use Save');
    if (!attachment.storage) return base64(await this.attachmentPart(id, 0));
    const data = new Uint8Array(attachment.size);
    try {
      for (let index = 0; index < Math.max(1, Math.ceil(data.length / attachment.storage.chunk_size)); index++) {
        const part = await this.attachmentPart(id, index);
        data.set(part, index * attachment.storage.chunk_size); part.fill(0);
      }
      return base64(data);
    } finally { data.fill(0); }
  }

  private async forwardFiles(messageId: number): Promise<PreparedFile[]> {
    const { state, engine } = this.require(), store = this.files!;
    const files = state.attachments.filter(file => file.message_id === messageId);
    const prepared: PreparedFile[] = [];
    const original = this.fileIds(state);
    try {
      for (const file of files) {
        if (!file.storage) { prepared.push(...await this.prepareFiles({ attachments: [{ name: file.name, data: file.data ?? '' }] })); continue; }
        const head = await this.attachmentPart(file.id, 0);
        const sanitize = this.require().state.ui.attachment_privacy !== 'false';
        const pass = attachment_passes_through(file.name, head.subarray(0, 1024)); head.fill(0);
        if (sanitize && !pass) { prepared.push(...await this.prepareFiles({ attachments: [{ name: file.name, data: await this.previewAttachment(file.id) }] })); continue; }
        let hash: AttachmentHasher | null = new AttachmentHasher();
        try {
          for (let index = 0; index < Math.max(1, Math.ceil(file.size / file.storage.chunk_size)); index++) {
            const data = await this.attachmentPart(file.id, index); try { hash.update(data); } finally { data.fill(0); }
          }
          const sha256 = [...hash.finish()]; hash = null;
          prepared.push({ name: sanitize ? attachment_passed_name(file.name) : file.name, size: file.size, sha256, storage: file.storage });
        } finally { hash?.free(); }
      }
      if (this.engine !== engine) throw new Error('Profile is locked');
      return prepared;
    } catch (error) { for (const file of prepared) if (!original.has(file.storage.id)) await store.remove(file.storage.id).catch(() => {}); throw error; }
  }
  private boot(stage: BootStatus['stage'], state: BootStatus['state'], detail: string): void {
    this.emit('boot_status', { stage, state, detail });
    this.log.debug(`boot:${stage}`, `${state}${detail ? ' · ' + detail : ''}`);
  }
  private info(): RelayInfo { return { mode: 'builtin', external: '', hosted: this.hosted, dial: { attempts: 0, last_error: this.hosted.state === 'failed' ? this.hosted.reason : null, unreachable_checks: 0, unreachable_error: null } }; }
  private relayChanged(): void { this.event({ RelayInfoChanged: { info: this.info() } }); }

  private transact<T>(action: (state: BrowserState, engine: BrowserEngine) => T | Promise<T>, mirrorRecovery: boolean | (() => boolean) = false): Promise<T> {
    const run = async (): Promise<T> => {
      const { engine, state } = this.require();
      const storage = this.storage!;
      const backup = structuredClone(state);
      engine.begin();
      let writing = false;
      try {
        const result = await action(state, engine);
        if (this.engine !== engine || this.closing) throw new Error('Profile is locked');
        const now = Date.now();
        const expired = expireHistory(state, now);
        queueDeletionControls(state, this.identity!);
        pruneFiles(state, this.identity!, now);
        // Refill per-recipient file windows in this same snapshot write.
        queueFileChunks(state, this.identity!, now);
        expireResends(state, now);
        const oldFiles = this.fileIds(backup), newFiles = this.fileIds(state);
        const removedFiles = [...oldFiles].filter(id => !newFiles.has(id));
        const ciphertext = engine.seal(JSON.stringify(state));
        const next = engine.pending_generation();
        writing = true;
        await storage.persist(ciphertext, next, expired || removedFiles.length > 0 || (typeof mirrorRecovery === 'function' ? mirrorRecovery() : mirrorRecovery));
        if (this.engine !== engine || this.closing) throw new Error('Profile is locked');
        engine.commit();
        for (const id of removedFiles) void this.files?.remove(id).catch(() => {});
        return result;
      } catch (error) {
        if (this.engine !== engine || this.closing) throw error;
        if (writing) {
          // The new slot may already be durable. Continuing the old ratchet
          // would reuse message keys; require a fresh read of the two slots.
          this.log.info('storage_fault', 'profile storage failed during a transaction');
          this.closeNow();
          this.emit('browser_fault', 'Profile storage failed; unlock again to recover the last complete transaction');
        } else {
          const retained = this.fileIds(backup);
          for (const id of this.fileIds(state)) if (!retained.has(id)) void this.files?.remove(id).catch(() => {});
          engine.rollback();
          this.state = backup;
        }
        throw error;
      }
    };
    const pending = this.queue.then(async () => {
      this.transactSince = Date.now();
      try { return await run(); } finally { this.transactSince = null; }
    });
    this.queue = pending.catch(() => {});
    return pending;
  }

  /** Diagnostics only: once a minute, name work stalled far past the network
   * deadlines — a profile transaction, a send slot and its step, or an outbox
   * that has letters but deposited nothing (counts only, no content). */
  private reportStalls(now = Date.now()): void {
    if (now - this.stallReported < STALL_REPORT_MS || this.stallReports >= 5 || !this.state) return;
    const age = (since: number): string => `${Math.round((now - since) / 1000)} s`;
    const stalled: string[] = [];
    if (this.transactSince !== null && now - this.transactSince >= STALL_MS) stalled.push(`transaction ${age(this.transactSince)}`);
    for (const slot of this.sending.values()) if (now - slot.at >= STALL_MS) stalled.push(`send ${slot.stage} ${age(slot.at)}`);
    const outbox = this.state.outbox;
    const waiting = outbox.some(item => !item.deposited && canSendControls(this.state!, item.peer));
    if (!stalled.length && !(waiting && now - this.lastDeposit >= STALL_MS)) return;
    const parts = outbox.filter(item => item.file);
    const flight = this.state.transfers.reduce((n, file) => n + (!file.incoming && file.status === 'sending' ? Object.keys(file.flight ?? {}).length : 0), 0);
    stalled.push(`slots ${this.sending.size}/${SEND_STREAMS}`, `outbox ${outbox.length} (parts ${parts.length}, sealed ${parts.filter(item => item.blob !== null).length})`,
      `due ${dueLetters(this.state, now, 8, this.sending).length}`, `parts in flight ${flight}`, `no deposit for ${age(this.lastDeposit)}`);
    this.stallReported = now; this.stallReports++;
    this.log.info('stall', `${stalled.join(' · ')}${this.pumping ? ' · outbox pass running' : ''}`);
  }

  /** Stop network immediately. In-progress writes are synchronous flushes in
   * this worker; no long network operation is held inside the state queue. */
  closeNow(): void {
    this.closing = true;
    this.verificationRetry.clear();
    if (this.engine || this.storage) {
      this.log.info('vault_lock', 'keys wiped from worker memory');
      this.log.snapshot();
    }
    this.generation++;
    if (this.timer) clearTimeout(this.timer);
    if (this.retryTimer) clearTimeout(this.retryTimer);
    if (this.sweepTimer) clearInterval(this.sweepTimer);
    this.timer = null;
    this.retryTimer = null;
    this.sweepTimer = null;
    this.relay?.close(); this.relay = null;
    this.network?.close(); this.network?.free(); this.network = null;
    this.engine?.close(); this.engine?.free(); this.engine = null;
    this.files?.close(); this.files = null; this.verifyingFiles.clear();
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
    this.log.info(create ? 'vault_create' : 'vault_unlock', passkey ? 'passkey' : 'password');
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
      this.files = await FileStorage.open(storage.directory, engine);
      if (this.engine !== engine || this.closing) throw new Error('Profile opening was cancelled');
      // No transfer or upload is running during open. Remove only unreferenced
      // encrypted remnants; a normal transaction removes named obsolete files.
      await this.files.collect(this.fileIds(state));
      const needsExpiry = (): boolean => !!this.state?.messages.some(row => row.deleted_at == null && row.expires_at !== null && row.expires_at <= Date.now());
      const inlineHistory = state.attachments.some(file => file.data !== undefined);
      await this.transact(async next => {
        for (const file of next.attachments) if (file.data !== undefined) {
          file.storage = await this.storeBytes(new Uint8Array(unbase64(file.data, FILE_PREVIEW_LIMIT)));
          delete file.data;
        }
        resumeFiles(next, this.identity!, Date.now());
      }, inlineHistory);
      this.verifyReadyFiles();
      this.sweepTimer = setInterval(() => { this.reportStalls(); if (needsExpiry() && !this.closing) void this.transact(() => {}).catch(() => {}); }, 10000);
      this.boot('vault', 'done', 'Profile opened; keys remain in the WASM worker');
      const attempt = ++this.generation;
      // "No deposit" stalls count from the network start, not the epoch.
      this.lastDeposit = Date.now(); this.stallReports = 0;
      void this.startNetwork(attempt);
      return null;
    } catch (error) {
      if (this.engine === engine && engine) this.closeNow();
      else { engine?.close(); engine?.free(); storage.close(); }
      this.boot('vault', 'failed', reason(error));
      this.log.info('vault_failed', reason(error));
      throw error;
    } finally {
      if (Array.isArray(args.prf)) (args.prf as number[]).fill(0);
    }
  }

  private async startNetwork(attempt: number): Promise<void> {
    const engine = this.engine!;
    this.relayAnswered = false;
    if (!this.networkConfig) { this.networkUnconfigured(); return; }
    this.hosted = { state: 'starting' }; this.relayChanged();
    try {
      this.log.info('network_start', `bootstrap attempt ${attempt}`);
      const network = await engine.start_network(JSON.stringify(this.networkConfig), (raw: string) => {
        if (attempt !== this.generation) return;
        try {
          const status = JSON.parse(raw) as BootStatus;
          this.boot(status.stage, status.state, status.detail);
        } catch { /* Reject malformed progress, never invent readiness. */ }
      });
      if (attempt !== this.generation || this.closing) {
        this.log.debug('network_abort', 'superseded by a newer attempt');
        network.close(); network.free(); return;
      }
      this.network = network;
      this.relay = new BrowserRelay(engine, network, this.identity!, (peer, blob) => this.receive(peer, blob), error => this.networkFailed(error));
      this.relay.start();
      this.hosted = { state: 'ready', address: this.identity!.onion };
      this.boot('core', 'done', 'In-process client destination is ready');
      this.boot('relay', 'done', 'Encrypted owner-only mailbox is accepting I2P streams');
      this.event('RelayConnected');
      this.relayChanged();
      this.schedule(0);
      this.boot('dht', 'active', 'Connecting saved contact relays');
      void this.warmPeerRelays(attempt).then(() => {
        if (attempt !== this.generation || this.closing) return;
        const found = !this.state?.contacts.some(peer => peer.trust !== 2 && peer.request !== 'incoming') || this.relayProbes.completed > 0 || this.relayAnswered;
        this.boot('dht', found ? 'skipped' : 'failed', found ? 'Contact relay discovery finished; browser does not use DHT' : 'Contact relays are unavailable; messages stay queued');
      });
    } catch (error) { if (attempt === this.generation && !this.closing) this.networkFailed(reason(error)); }
  }

  private async warmPeerRelays(attempt: number): Promise<void> {
    // A restored destination has fresh I2P encryption keys and inbound leases.
    // Establish outbound streams so peers learn its current signed LeaseSet
    // without waiting for their cached pre-restart address to expire.
    const peers = this.state?.contacts.filter(peer => peer.trust !== 2 && peer.request !== 'incoming')
      .map(peer => ({ key: peer.sign_pk, address: peer.relay || peer.onion })) || [];
    this.relayProbes = { attempted: 0, completed: 0, failed: 0 };
    let next = 0;
    const active = (): boolean => attempt === this.generation && !this.closing && this.relay !== null;
    const worker = async (): Promise<void> => {
      while (active() && next < peers.length) {
        const peer = peers[next++]!;
        for (let retry = 0; retry < 3 && active(); retry++) {
          if (!this.state?.contacts.some(contact => contact.sign_pk === peer.key && contact.trust !== 2 && contact.request !== 'incoming')) break;
          this.relayProbes.attempted++;
          try {
            this.log.debug('relay_probe', `${this.who(peer.key)}${peer.address === this.state?.contacts.find(contact => contact.sign_pk === peer.key)?.onion ? '' : ' via its relay'} · try ${retry + 1}/3`);
            await this.relay!.ping(peer.address);
            if (active()) {
              this.relayProbes.completed++;
              this.log.info('relay_probe_ok', this.who(peer.key));
            }
            break;
          } catch {
            if (!active()) return;
            this.relayProbes.failed++;
            this.log.debug('relay_probe_failed', `${this.who(peer.key)} · try ${retry + 1}/3`);
            if (retry < 2) await new Promise(resolve => setTimeout(resolve, 10000));
          }
        }
      }
    };
    await Promise.all([worker(), worker()]);
  }
  /** This hosting serves the client without a network service. Not a failure
   * a retry would mend — none is armed — so it is its own relay state rather
   * than `failed`, and the screens say what is missing instead of promising
   * to keep trying. The boot event keeps the usual shape. */
  private networkUnconfigured(): void {
    const reason = 'Network service is not configured; saved chats are available offline';
    this.log.info('network_offline', 'no network service configured; offline only');
    this.hosted = { state: 'unconfigured', reason };
    this.boot('relay', 'failed', reason); this.relayChanged();
  }
  private networkFailed(error: string): void {
    this.log.info('relay_failed', error);
    this.event('RelayDisconnected');
    this.relay?.close(); this.relay = null;
    this.network?.close(); this.network?.free(); this.network = null;
    this.hosted = { state: 'failed', reason: error };
    this.boot('relay', 'failed', error); this.relayChanged();
    if (this.networkConfig && !this.closing && this.engine) {
      if (this.retryTimer) clearTimeout(this.retryTimer);
      this.retryTimer = setTimeout(() => {
        this.retryTimer = null;
        this.log.info('network_retry', '30 s elapsed; restarting bootstrap');
        if (!this.closing && this.engine && this.hosted.state === 'failed') void this.startNetwork(++this.generation);
      }, 30000);
    }
  }
  private schedule(delay = 1000): void {
    if (this.closing || !this.relay) return;
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => { this.timer = null; void this.pump().catch(error => { if (!this.closing) this.log.info('outbox_fault', reason(error)); }); }, delay);
  }
  /** Plaintext of a part, read outside the vault queue. */
  private async outgoingPart(item: Outbox): Promise<Uint8Array | null> {
    const source = this.state ? chunkSource(this.state, item) : null;
    if (!source) return null;
    const { attachment, file, index } = source;
    return attachment.storage ? this.files!.read(attachment.storage, index) : inlinePart(attachment, file.chunk_size, index);
  }
  private async pump(): Promise<void> {
    if (this.pumping || !this.relay || !this.engine || !this.state) return;
    this.verifyReadyFiles();
    this.pumping = true;
    const attempt = this.generation;
    try {
      if (fileWorkDue(this.state, Date.now())) {
        // Overdue parts and abandoned transfers; one failure never stops sending.
        const events: CoreEvent[] = [];
        try {
          await this.transact(state => failStalledFiles(state, this.identity!, Date.now(), events));
          events.forEach(event => this.event(event));
        } catch (error) { if (!this.closing) this.log.info('file_queue_fault', reason(error)); }
      }
      if (attempt !== this.generation || !this.state) return;
      // Network waits overlap, but encryption and persistence still pass through
      // the single vault queue. Controls sort first; four streams bound memory.
      // One slow stream (connect, write and read deadlines add up to minutes)
      // holds only its own slot: a freed slot refills at once, so receipts and
      // further parts never wait for the slowest send.
      const free = SEND_STREAMS - this.sending.size;
      if (free <= 0) return;
      for (const item of dueLetters(this.state, Date.now(), free, this.sending)) {
        this.sending.set(item.key, { stage: 'start', at: Date.now() });
        void this.sendItem(item, attempt)
          .catch(error => { if (!this.closing) this.log.info('outbox_fault', reason(error)); })
          .finally(() => { this.sending.delete(item.key); if (attempt === this.generation) this.schedule(0); });
      }
    } finally { this.pumping = false; this.schedule(1000); }
  }

  /** One letter: encrypt (a new encryption for a part or a resend), deposit,
   * then record the deposit before anything could send it again. */
  private async sendItem(item: Outbox, attempt: number): Promise<void> {
    if (attempt !== this.generation || !this.relay || !this.state) return;
    const contact = this.state.contacts.find(value => value.sign_pk === item.peer);
    if (!contact || !canSendControls(this.state, item.peer)) return;
    const address = contact.relay || contact.onion;
    const step = (stage: string): void => { if (this.sending.has(item.key)) this.sending.set(item.key, { stage, at: Date.now() }); };
    let part: Uint8Array | null = null;
    try {
      if (item.file && item.blob === null) {
        step('read');
        try { part = await this.outgoingPart(item); } catch (error) {
          if (attempt !== this.generation) return;
          // A storage that did not open in time is retried, not a lost file.
          if (error instanceof StorageBusy) throw error;
          const events: CoreEvent[] = [];
          await this.transact(state => failOutgoingFile(state, this.identity!, item.peer, item.file!.id, events));
          events.forEach(event => this.event(event));
          this.log.info('file_part_unreadable', `${this.who(item.peer)} · ${reason(error)}`);
          return;
        }
        if (!part) {
          // No longer wanted (held, stopped or erased): drop the reference.
          await this.transact(state => { state.outbox = state.outbox.filter(value => value.key !== item.key || value.blob !== null); });
          return;
        }
      }
      // Bundle lookup and connect run outside the persistence queue.
      const encrypting = item.blob === null || item.deposited;
      step('bundle');
      const bundle = encrypting && !this.engine!.has_session(item.peer) ? await this.relay.bundle(address, item.peer) : new Uint8Array();
      if (attempt !== this.generation || !this.relay) return;
      step('encrypt');
      await this.transact((state, engine) => {
        if (attempt !== this.generation) return;
        const current = state.outbox.find(value => value.key === item.key);
        if (!current || !canSendControls(state, item.peer) || !reopenForResend(current)) return;
        if (current.blob === null) {
          const letter = current.file ? buildChunkLetter(state, this.identity!, current, part) : current.payload;
          if (!letter) { state.outbox = state.outbox.filter(value => value !== current); return; }
          const initial = !engine.has_session(contact.sign_pk);
          current.blob = base64(engine.encrypt(contact.sign_pk, contact.dh_pk, bundle, JSON.stringify(letter)));
          if (initial) current.initial = true;
          if (current.file) current.payload = null;
        }
        current.at = Date.now(); current.attempts++;
      });
      part?.fill(0); part = null;
      const current = this.state?.outbox.find(value => value.key === item.key);
      if (!current?.blob || !this.relay || attempt !== this.generation || !this.state || !canSendControls(this.state, item.peer)) return;
      step('deposit');
      await this.relay.deposit(address, current.peer, fromBase64(current.blob));
      if (attempt !== this.generation || !this.relay) return;
      this.lastDeposit = Date.now(); this.stallReports = 0;
      this.relayAnswered = true;
      this.event({ ContactReachability: { contact_id: contact.id, unreachable: false } });
      this.boot('dht', 'skipped', 'Authenticated peer relay answered');
      this.log.debug('outbox_deposited', `to ${this.who(current.peer)} · attempts ${current.attempts}${current.resends ? ` · resend ${current.resends}` : ''}`);
      let sent: number | null = null;
      step('settle');
      // The relay stored this ciphertext: from now on it is never sent again.
      await this.transact(state => { sent = settleDeposit(state, item.key, Date.now()); });
      if (sent !== null) this.event({ MessageSent: { message_id: sent } });
    } catch (error) {
      if (attempt !== this.generation || !this.state) return;
      const first = !item.deposited && !item.resends;
      step('record failure');
      try {
        await this.transact(state => {
          const current = state.outbox.find(value => value.key === item.key);
          if (!current) return;
          current.at = Date.now();
          if (!current.deposited) current.attempts = Math.max(1, current.attempts);
        });
      } catch { /* Locked or storage fault: nothing more to record. */ }
      this.event({ ContactReachability: { contact_id: contact.id, unreachable: true } });
      this.log.debug('outbox_retry', `to ${this.who(item.peer)} · attempts ${(this.state?.outbox.find(value => value.key === item.key))?.attempts ?? item.attempts} · ${reason(error)}`);
      // Keep the exact ciphertext for a later retry (until the relay confirms it), including restart.
      if (item.row !== null && first) this.event({ MessageFailed: { message_id: item.row, reason: reason(error) } });
    } finally { part?.fill(0); }
  }

  /** The receipt stored with a recent envelope; a group receipt is rebuilt
   * from current membership instead of keeping a full roster per entry. */
  private repeatedAck(state: BrowserState, entry: SeenEnvelope): WirePayload | null {
    if (entry.file) return repeatFileReceipt(state, this.identity!, entry.peer, entry.file.id, entry.file.index, entry.file.sent_at);
    if (!entry.ack) return null;
    const ack = structuredClone(entry.ack);
    if (entry.group) { if (!state.groups.some(group => group.id === entry.group)) return null; ack.group = this.groupWire(state, entry.group); }
    return ack;
  }

  private async receive(peer: string, bytes: Uint8Array): Promise<void> {
    const receivingEngine = this.engine;
    if (this.state?.contacts.some(contact => contact.sign_pk === peer && contact.trust === 2)) return;
    const initial = envelope_is_initial(bytes);
    this.log.debug('message_rx', `envelope ${bytes.length} B from ${this.who(peer)}${initial ? ' · new session' : ''}`);
    const digestInput = new Uint8Array(32 + bytes.length); digestInput.set(unhex(peer)); digestInput.set(bytes, 32);
    const hash = hex(new Uint8Array(await crypto.subtle.digest('SHA-256', digestInput)));
    if (this.engine !== receivingEngine || this.closing) throw new Error('Profile is locked');
    const events: CoreEvent[] = [];
    let erasure = false;
    await this.transact(async (state, engine) => {
      if (engine !== receivingEngine) throw new Error('Profile is locked');
      if (state.contacts.some(contact => contact.sign_pk === peer && contact.trust === 2)) return;
      const duplicate = recentEnvelope(state, peer, hash);
      if (duplicate) {
        // The same ciphertext again (a lost `Deposited` frame): repeat the
        // receipt; decrypting it a second time is impossible.
        const ack = this.repeatedAck(state, duplicate);
        if (ack) queueReceipt(state, peer, ack);
        return;
      }
      // An X3DH initial is never accepted twice, however old: a replay could
      // otherwise re-open a superseded session through the OTK-free fallback.
      if (initial && replayedInitial(state, peer, hash)) { this.log.info('replay_dropped', `initial letter from ${this.who(peer)}`); return; }
      let contact = state.contacts.find(value => value.sign_pk === peer);
      const decoded = JSON.parse(engine.decrypt(peer, contact?.dh_pk || '', bytes)) as { sign_pk: string; dh_pk: string; payload: WirePayload };
      if (decoded.sign_pk !== peer) throw new Error('Envelope sender mismatch');
      if (initial) rememberInitial(state, peer, hash);
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
      if (contact.request === 'incoming' && !isRequestContent(incoming) && !(incoming.dm && incoming.dm.kind !== 0) && incoming.wipe !== true) {
        rememberEnvelope(state, { hash, peer, ack: null });
        return;
      }
      let groupId: string | null = null;
      if (incoming.group) { groupId = this.ingestGroup(state, incoming.group, peer); events.push({ GroupUpdated: { group_id: groupId } }); }
      const contactId = groupId ? null : contact.id;
      const own = this.identity!;
      let ack = incoming.origin_msg_id > 0 && !incoming.ack_for && contact.request !== 'incoming' ? payload(own, state.displayName) : null;
      if (ack) { ack.ack_for = incoming.origin_msg_id; ack.group = incoming.group; }
      if (incoming.file_chunk || incoming.file_ack || incoming.file_cancel) {
        ack = await receiveFileControl(state, own, peer, incoming, this.files!, events);
      } else if (incoming.dm && incoming.dm.kind !== 0) {
        erasure = incoming.dm.kind === 1;
        ack = receiveDeletionControl(state, own, contact.id, peer, incoming, events);
      } else if (incoming.ack_for !== null) {
        const row = state.messages.find(value => value.outgoing && value.origin === incoming.ack_for && sameScope(value, contactId, groupId));
        if (row) {
          state.outbox = state.outbox.filter(value => {
            if (value.row !== row.id || value.peer !== peer) return true;
            if (value.blob === null || !value.initial) return false;
            value.row = null; value.payload = null; // Deliver the session opener only.
            return true;
          });
          if (!state.outbox.some(value => value.row === row.id)) {
            row.sent = true; row.delivered = true;
            events.push({ MessageDelivered: { message_id: row.id } });
            this.log.info('message_ack', `message ${row.id} delivered to ${this.who(peer)}`);
          }
        }
      } else if (incoming.typing !== null) {
        if (contact.request !== 'incoming') events.push({ Typing: { contact_id: contactId, group_id: groupId, sender_sign_pk: groupId ? peer : null, typing: incoming.typing } });
      } else if (incoming.wipe === true && groupId === null) {
        erasure = true;
        engine.forget_peer(peer); this.removeContact(state, contact.id); events.push({ ContactWiped: { contact_id: contact.id, name: contact.name } });
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
      } else if (incoming.origin_msg_id > 0 && !incoming.callback_data && !incoming.console) {
        if (incoming.dm && (incoming.dm.kind !== 0 || hex(incoming.dm.reference.author) !== peer || incoming.dm.reference.origin !== incoming.origin_msg_id || incoming.dm.reference.sent_at !== incoming.sent_at || groupId !== null)) throw new Error('Forged direct-message reference');
        const existing = state.messages.some(value => value.author === peer && value.origin === incoming.origin_msg_id && sameScope(value, contactId, groupId));
        const deleted = state.tombstones.includes(`${peer}:${incoming.origin_msg_id}:${incoming.sent_at}`);
        if (!existing && !deleted) {
          const row: StoredMessage = { id: allocate(state), origin: incoming.origin_msg_id, author: peer, contact_id: contactId, group_id: groupId, sender_sign_pk: groupId ? peer : null, outgoing: false, body: incoming.body, sent_at: incoming.sent_at, sent: true, delivered: true, read: false, expires_at: incoming.ttl_ms === null ? null : incoming.sent_at + incoming.ttl_ms, reply_to: null, buttons: incoming.buttons, dm_seq: state.messages.filter(value => sameScope(value, contactId, groupId)).length + 1 };
          if (incoming.reply_to) row.reply_to = state.messages.find(value => value.author === hex(incoming.reply_to!.sender_sign_pk) && value.origin === incoming.reply_to!.origin_msg_id && sameScope(value, contactId, groupId))?.id || null;
          state.messages.push(row); contact.last_message_at = row.sent_at;
          this.log.info('message_in', `message ${row.id} from ${this.who(peer)}`);
          // An unaccepted contact gets a bounded share of OPFS, like its file offers.
          const stored = contact.request !== 'incoming' || pendingInlineAllowed(state, contact.id, incoming.attachments);
          if (!stored) this.log.info('request_attachments_dropped', `${incoming.attachments.length} inline file(s) from ${this.who(peer)}`);
          if (stored) for (const attachment of incoming.attachments) {
            const storage = await this.storeBytes(new Uint8Array(attachment.data));
            state.attachments.push({ id: allocate(state), message_id: row.id, name: attachment.name, size: attachment.data.length, storage });
          }
          await acceptFileOffers(state, own, peer, row.id, incoming.files, this.files!, events);
          if (contact.request !== 'incoming' && visible(row)) events.push({ IncomingMessage: { contact_id: contactId, group_id: groupId, sender_sign_pk: row.sender_sign_pk, message_id: row.id, body: row.body, sent_at: row.sent_at, notify_sound: null, console_kind: null } });
        }
      }
      // Receipts never fail the letter they answer: gated (no pending or
      // blocked contact), coalesced, and dropped if even the reserve is full.
      if (ack && queueReceipt(state, peer, ack)) this.log.debug('ack_tx', `receipt queued for ${this.who(peer)}`);
      rememberEnvelope(state, incoming.file_chunk
        ? { hash, peer, ack: null, file: { id: hex(incoming.file_chunk.file_id), index: incoming.file_chunk.index, sent_at: incoming.sent_at } }
        : { hash, peer, ack: ack && !ack.file_ack ? { ...ack, group: null } : null, ...(ack?.group && groupId ? { group: groupId } : {}) });
      events.push({ PeerSeen: { contact_id: contact.id, at_ms: incoming.sent_at } });
    }, () => erasure);
    if (this.engine !== receivingEngine) return;
    events.forEach(event => this.event(event));
    this.verifyReadyFiles();
    this.schedule(0);
  }

  private validatePayload(value: WirePayload): void {
    if (!Number.isSafeInteger(value.origin_msg_id) || value.origin_msg_id < 0 || !Number.isSafeInteger(value.sent_at) || typeof value.body !== 'string' || value.body.length > 128 * 1024 || !Array.isArray(value.attachments) || value.attachments.length > 16 || value.attachments.reduce((n, file) => n + file.data.length, 0) > 1024 * 1024 || value.attachments.some(file => new TextEncoder().encode(file.name).length > 1024) || (value.ttl_ms !== null && (!Number.isSafeInteger(value.ttl_ms) || value.ttl_ms < 0 || value.ttl_ms > 365 * 86400000))) throw new Error('Unsupported message payload');
    validateFilePayload(value);
    if (value.console !== null || value.callback_data !== null) throw new Error('This browser does not support this message control');
    if (value.edit_of !== null && (!Number.isSafeInteger(value.edit_of) || value.edit_of <= 0 || value.attachments.length > 0)) throw new Error('Invalid edit payload');
    if (value.ack_for !== null && (!Number.isSafeInteger(value.ack_for) || value.ack_for < 0)) throw new Error('Invalid receipt');
    if (value.pin !== null && (value.pin.sender_sign_pk.length !== 32 || !Number.isSafeInteger(value.pin.origin_msg_id) || value.pin.origin_msg_id <= 0)) throw new Error('Invalid pinned-message reference');
  }
  private addContact(state: BrowserState, args: Args, request: Contact['request'] = 'outgoing'): Contact {
    const sign = text(args, 'signPk').toLowerCase(); const dh = text(args, 'dhPk').toLowerCase();
    unhex(sign); unhex(dh);
    if (sign === this.identity!.sign_pk) throw new Error('Cannot add your own identity as a contact');
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
    this.log.info('contact_add', `contact ${contact.id} (${request})`);
    return contact;
  }
  private getContact(state: BrowserState, id: number): Contact { const contact = state.contacts.find(value => value.id === id); if (!contact) throw new Error('Contact not found'); return contact; }
  private removeContact(state: BrowserState, id: number): void {
    const contact = this.getContact(state, id);
    this.log.info('contact_remove', `contact ${contact.id}`);
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
    for (const member of members) {
      const pinned = member.is_self ? this.identity : state.contacts.find(value => value.sign_pk === member.sign_pk);
      if (pinned && pinned.dh_pk !== member.dh_pk) throw new Error('Group member identity changed');
    }
    if (new Set(members.map(member => member.sign_pk)).size !== members.length || !members.some(member => member.is_self) || !members.some(member => member.sign_pk === peer)) throw new Error('Group sender is not a member');
    let group = state.groups.find(value => value.id === id);
    if (group && !group.members.some(member => member.sign_pk === peer)) throw new Error('Uninvited group sender');
    if (!group) { group = { id, name: wire.name, members, created_at: Date.now(), pinned_at: null, last_message_at: Date.now() }; state.groups.push(group); }
    else for (const member of members) {
      const known = group.members.find(value => value.sign_pk === member.sign_pk) || state.contacts.find(value => value.sign_pk === member.sign_pk);
      if (known && known.dh_pk !== member.dh_pk) throw new Error('Group member identity changed');
      if (!group.members.some(value => value.sign_pk === member.sign_pk)) { if (group.members.length >= 64) throw new Error('Group member limit reached'); group.members.push(member); }
    }
    // An established member can introduce a new member. Existing identity keys
    // stay pinned, and a stale message cannot remove later additions.
    return id;
  }
  private groupWire(state: BrowserState, id: string): WireGroup {
    const group = state.groups.find(value => value.id === id); if (!group) throw new Error('Group not found');
    return { id: unhex(id), name: group.name, members: group.members.map(member => ({ sign_pk: unhex(member.sign_pk), dh_pk: unhex(member.dh_pk), onion: member.onion, name: member.name })) };
  }
  private send(state: BrowserState, args: Args, group: boolean, attachments: PreparedFile[]): number {
    const contact = group ? null : this.getContact(state, number(args, 'contactId'));
    if (contact?.trust === 2) throw new Error('Contact is blocked');
    if (contact?.request === 'incoming') throw new Error('Accept the contact request first');
    const groupId = group ? text(args, 'groupId') : null;
    const body = text(args, 'body');
    if (body.length > 128 * 1024 || attachments.length > 16) throw new Error('Message size limit');
    const letter = payload(this.identity!, state.displayName);
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
    const chunked: { offer: import('./model').WireFileOffer; attachment: number }[] = [];
    for (const attachment of attachments) {
      const id = allocate(state);
      state.attachments.push({ id, message_id: row.id, name: attachment.name, size: attachment.size, storage: attachment.storage });
      const offer = { file_id: [...crypto.getRandomValues(new Uint8Array(16))], name: attachment.name, size: attachment.size, sha256: attachment.sha256, chunk_size: attachment.storage.chunk_size };
      letter.files.push(offer); chunked.push({ offer, attachment: id });
    }
    const peers = groupId ? state.groups.find(value => value.id === groupId)!.members.filter(member => !member.is_self).map(member => {
      let recipient = state.contacts.find(value => value.sign_pk === member.sign_pk);
      if (!recipient) recipient = this.addContact(state, { signPk: member.sign_pk, dhPk: member.dh_pk, onion: member.onion, name: member.name }, 'none');
      return recipient;
    }) : [contact!];
    const recipients = peers.filter(peer => peer.trust !== 2);
    if (!recipients.length) throw new Error('No active recipients');
    for (const peer of recipients) {
      queueLetter(state, peer.sign_pk, structuredClone(letter), row.id); peer.last_message_at = row.sent_at;
      for (const file of chunked) offerOutgoingFile(state, row.id, peer.sign_pk, file.offer, file.attachment);
    }
    if (groupId) state.groups.find(value => value.id === groupId)!.last_message_at = row.sent_at;
    this.log.info('message_send', `message ${row.id} to ${recipients.length} recipient(s)`);
    return row.id;
  }

  async invoke(command: string, args: Args = {}): Promise<unknown> {
    if (command === 'init') {
      this.configure(args.config as Record<string, string> | null);
      this.log.info('init', this.networkConfig ? 'network config received' : 'started without network config');
      return null;
    }
    if (command === 'retry_network') {
      if (this.engine && this.hosted.state === 'failed' && this.networkConfig) {
        if (this.retryTimer) clearTimeout(this.retryTimer); this.retryTimer = null;
        void this.startNetwork(++this.generation);
      }
      return null;
    }
    if (command === 'list_profiles') return listProfiles();
    if (command === 'delete_profile') {
      if (this.engine) throw new Error('Lock the profile first');
      await removeProfile(text(args, 'profile'));
      await this.log.clear(); // The stored previous session may describe that profile.
      return null;
    }
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
    if (command === 'current_version') return typeof __GIPNY_VERSION__ === 'undefined' ? 'unknown-browser' : `${__GIPNY_VERSION__}-browser`;
    if (command === 'update_configured' || command === 'get_auto_update' || command === 'update_installs_itself' || command === 'update_asks_for_root') return false;
    if (command === 'check_update' || command === 'get_agent_mode') return null;
    if (command === 'browser_diagnostics') return { ...JSON.parse(public_network_metrics()), relay_probes: { ...this.relayProbes } };
    if (command === 'get_relay_info') return this.info();
    if (command === 'get_relay_address') return this.identity?.onion || '';
    if (command === 'get_router_settings') return { transit: 'frugal', upnp: 'off' };
    if (command === 'get_dht_status') return { peers: 0, items: 0, bytes: 0, joined: false, stores: false, seeds: 0 };
    if (command === 'list_unreachable_contacts') return [];
    // The application log viewer works without an unlocked profile.
    if (command === 'log_settings') return { available: this.log.isEnabled, enabled: this.log.isEnabled, path: LOG_LOCATION };
    if (command === 'read_debug_log') return this.log.text();
    if (command === 'read_previous_log') return this.log.previous();
    if (command === 'clear_debug_log') { await this.log.clear(); this.log.info('log_cleared', 'action log and its stored copy erased'); return null; }
    const { state, engine, identity } = this.require();
    if (command === 'get_attachment_privacy') return state.ui.attachment_privacy !== 'false';
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
    if (command === 'get_ui_data') return state.ui[uiKey(args)] || null;
    if (command === 'list_messages' || command === 'list_group_messages') {
      const group = command === 'list_group_messages';
      const rows = state.messages.filter(value => sameScope(value, group ? null : number(args, 'contactId'), group ? text(args, 'groupId') : null) && inHistory(value) && (typeof args.beforeId !== 'number' || value.id < args.beforeId));
      return rows.slice(-Math.min(500, typeof args.limit === 'number' ? Math.max(1, args.limit) : 100)).reverse();
    }
    if (command === 'unread_count' || command === 'group_unread_count') return state.messages.filter(value => !value.read && visible(value) && sameScope(value, command === 'unread_count' ? number(args, 'contactId') : null, command === 'group_unread_count' ? text(args, 'groupId') : null)).length;
    if (command === 'list_attachments') return state.messages.some(row => row.id === args.messageId && visible(row)) ? state.attachments.filter(value => value.message_id === args.messageId).map(file => ({ id: file.id, message_id: file.message_id, name: file.name, size: file.size })) : [];
    if (command === 'load_attachment') return this.previewAttachment(number(args, 'attachmentId'));
    if (command === 'attachment_part') return this.attachmentPart(number(args, 'attachmentId'), number(args, 'index'));
    if (command === 'attachment_download_info') {
      const file = state.attachments.find(f => f.id === args.attachmentId);
      if (!file || !state.messages.some(row => row.id === file.message_id && visible(row))) throw new Error('Attachment not found');
      return { name: file.name, size: file.size, parts: file.storage ? Math.max(1, Math.ceil(file.size / file.storage.chunk_size)) : 1 };
    }
    if (command === 'files_progress') return this.messageFiles(state, [number(args, 'messageId')])[number(args, 'messageId')]!.progress;
    if (command === 'message_files') return this.messageFiles(state, args.messageIds as number[]);
    if (command === 'list_pinned_contact' || command === 'list_pinned_group') return state.messages.filter(value => state.pinnedMessages.includes(value.id) && visible(value) && sameScope(value, command === 'list_pinned_contact' ? number(args, 'contactId') : null, command === 'list_pinned_group' ? text(args, 'groupId') : null));
    if (command === 'search_messages') {
      const query = text(args, 'query').toLocaleLowerCase();
      return state.messages.filter(value => visible(value) && value.body.toLocaleLowerCase().includes(query) && (args.contactId == null || value.contact_id === args.contactId) && (args.groupId == null || value.group_id === args.groupId)).slice(-Math.min(500, typeof args.limit === 'number' ? args.limit : 100)).reverse().map(message => ({ message, contact_id: message.contact_id, group_id: message.group_id, contact_name: state.contacts.find(value => value.id === message.contact_id)?.name || null, group_name: state.groups.find(value => value.id === message.group_id)?.name || null }));
    }
    if (command === 'list_media_contact' || command === 'list_media_group') {
      const messages = state.messages.filter(value => visible(value) && sameScope(value, command === 'list_media_contact' ? number(args, 'contactId') : null, command === 'list_media_group' ? text(args, 'groupId') : null));
      return state.attachments.filter(value => messages.some(message => message.id === value.message_id)).slice(-100).map(file => ({ id: file.id, message_id: file.message_id, name: file.name, size: file.size, sent_at: messages.find(message => message.id === file.message_id)!.sent_at }));
    }
    if (command === 'message_position') { const index = state.messages.filter(value => visible(value) && sameScope(value, typeof args.contactId === 'number' ? args.contactId : null, typeof args.groupId === 'string' ? args.groupId : null)).reverse().findIndex(value => value.id === args.messageId); return index < 0 ? null : index; }
    if (command === 'send_message' || command === 'send_group_message' || command === 'forward_message') {
      const currentEngine = engine, store = this.files!;
      const originalFiles = this.fileIds(state);
      const prepared = command === 'forward_message' ? await this.forwardFiles(number(args, 'sourceMessageId')) : await this.prepareFiles(args);
      try {
        if (this.engine !== currentEngine || this.closing) throw new Error('Profile is locked');
        const id = await this.transact(next => {
          if (command === 'forward_message') {
            const source = next.messages.find(row => row.id === args.sourceMessageId && visible(row));
            if (!source) throw new Error('Message not found');
            return this.send(next, { ...args, body: source.body, ttlSecs: null, replyTo: null }, typeof args.groupId === 'string', prepared);
          }
          return this.send(next, args, command === 'send_group_message', prepared);
        });
        this.schedule(0); return id;
      } catch (error) {
        // A storage fault may have committed the newest slot. Keep ciphertext
        // until recovery can determine ownership; otherwise remove staging now.
        if (this.engine === currentEngine) for (const file of prepared) if (!originalFiles.has(file.storage.id)) await store.remove(file.storage.id).catch(() => {});
        throw error;
      }
    }
    const replaceRecovery = ['change_passphrase', 'enroll_passkey', 'delete_message', 'delete_contact', 'decline_contact_request', 'delete_group', 'clear_dm_chat', 'clear_message_history'].includes(command);
    const fileEvents: CoreEvent[] = [];
    const result = await this.transact(async (next, core) => {
      const value = this.mutate(command, args, next, core);
      if (command === 'accept_contact_request') {
        const contact = this.getContact(next, number(args, 'id'));
        forgetRequestObligations(next, contact.id);
        await acceptPendingFiles(next, this.identity!, contact.sign_pk, this.files!, fileEvents);
        for (const row of next.messages) if (row.contact_id === contact.id && row.group_id === null && !row.outgoing && visible(row)) {
          const ack = payload(this.identity!, next.displayName); ack.ack_for = row.origin;
          queueReceipt(next, contact.sign_pk, ack);
        }
      }
      return value;
    }, replaceRecovery);
    fileEvents.forEach(event => this.event(event));
    if (command === 'delete_message' || command === 'acknowledge_deletion_notices' || command === 'clear_dm_chat' || command === 'clear_message_history') {
      for (const row of this.state?.messages ?? []) {
        if (row.contact_id !== null && row.deleted_at != null && (command !== 'delete_message' || row.id === args.id)
          && (command !== 'acknowledge_deletion_notices' || (args.messageIds as number[]).includes(row.id))) this.event(deletionEvent(row));
      }
    }
    if (command === 'add_contact' && typeof result === 'number') this.event({ ContactAdded: { contact_id: result } });
    if (command === 'update_contact' || command === 'set_contact_bot' || command === 'accept_contact_request') this.event({ ContactUpdated: { contact_id: number(args, 'id') } });
    if (command === 'create_group' && typeof result === 'string') this.event({ GroupUpdated: { group_id: result } });
    if (command === 'add_group_member') this.event({ GroupUpdated: { group_id: text(args, 'groupId') } });
    this.schedule(0);
    return result;
  }

  private messageFiles(state: BrowserState, ids: number[]): Record<number, import('../api').MessageFiles> {
    if (!Array.isArray(ids) || ids.length > 500 || ids.some(id => !Number.isSafeInteger(id) || id <= 0)) throw new Error('Invalid file history window');
    const result: Record<number, import('../api').MessageFiles> = {};
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

  private mutate(command: string, args: Args, state: BrowserState, engine: BrowserEngine): unknown {
    if (command === 'change_passphrase') { engine.change_password(text(args, 'old'), text(args, 'new')); return null; }
    if (command === 'enroll_passkey') { try { engine.enroll_passkey(new Uint8Array(args.credentialId as number[]), text(args, 'rpId'), new Uint8Array(args.salt as number[]), new Uint8Array(args.prf as number[])); } finally { if (Array.isArray(args.prf)) (args.prf as number[]).fill(0); } return null; }
    if (command === 'set_display_name') { state.displayName = text(args, 'name').slice(0, 64); return null; }
    if (command === 'set_ui_data') { const json = text(args, 'json'); if (json.length > 256 * 1024) throw new Error('UI preference size limit'); JSON.parse(json); state.ui[uiKey(args)] = json; this.log.debug('settings_update', `ui:${text(args, 'key')}`); return null; }
    if (command === 'set_muted') { const key = text(args, 'targetKey'); state.muted = state.muted.filter(value => value !== key); if (args.muted === true) state.muted.push(key); this.log.debug('settings_update', `mute:${key}`); return null; }
    if (command === 'add_contact') return this.addContact(state, args).id;
    if (command === 'update_contact') { const contact = this.getContact(state, number(args, 'id')); const trust = number(args, 'trust'); if (trust > 2) throw new Error('Invalid contact trust'); contact.name = text(args, 'name').slice(0, 96); contact.trust = trust; return null; }
    if (command === 'set_contact_bot') { this.getContact(state, number(args, 'id')).is_bot = args.isBot === true; return null; }
    if (command === 'accept_contact_request') { this.getContact(state, number(args, 'id')).request = 'none'; return null; }
    if (command === 'decline_contact_request' || command === 'delete_contact') {
      if (args.forBoth) throw new Error('Browser contact removal is local; delete messages individually to notify the peer');
      engine.forget_peer(this.getContact(state, number(args, 'id')).sign_pk); this.removeContact(state, number(args, 'id')); return null;
    }
    if (command === 'reset_contact_session') {
      const contact = this.getContact(state, number(args, 'id'));
      this.log.info('contact_session_reset', `contact ${contact.id}`);
      // File parts and one-shot controls are rebuilt from current state.
      state.outbox = state.outbox.filter(value => value.peer !== contact.sign_pk || value.row !== null || value.payload !== null);
      const pending = state.outbox.filter(value => value.peer === contact.sign_pk);
      if (pending.some(value => !value.payload)) throw new Error('An older queued letter cannot be re-encrypted');
      for (const item of pending) { item.key = crypto.randomUUID(); item.blob = null; item.initial = undefined; item.deposited = false; item.at = 0; item.attempts = 0; }
      forgetFlight(state, contact.sign_pk);
      engine.forget_peer(contact.sign_pk); return null;
    }
    if (command === 'cancel_files') { cancelBrowserFiles(state, this.identity!, number(args, 'messageId'), []); return null; }
    if (command === 'mark_read' || command === 'mark_group_read') { for (const row of state.messages) if (sameScope(row, command === 'mark_read' ? number(args, 'contactId') : null, command === 'mark_group_read' ? text(args, 'groupId') : null)) row.read = true; return null; }
    if (command === 'send_edit' || command === 'send_edit_group') {
      const group = command === 'send_edit_group';
      const row = state.messages.find(value => value.id === args.messageId && value.outgoing && visible(value) && sameScope(value, group ? null : number(args, 'contactId'), group ? text(args, 'groupId') : null));
      if (!row) throw new Error('Message cannot be edited');
      const letter = payload(this.identity!, state.displayName); letter.edit_of = row.origin; letter.body = text(args, 'newBody');
      if (letter.body.length > 128 * 1024) throw new Error('Message size limit');
      if (row.group_id) letter.group = this.groupWire(state, row.group_id);
      const recipients = row.group_id ? state.groups.find(value => value.id === row.group_id)!.members.filter(value => !value.is_self).map(value => value.sign_pk) : [this.getContact(state, row.contact_id!).sign_pk];
      recipients.forEach(peer => queueLetter(state, peer, structuredClone(letter), null));
      row.body = letter.body; return null;
    }
    if (command === 'delete_message') {
      const row = state.messages.find(value => value.id === args.id); if (!row || row.deleted_at != null) return null;
      if (row.contact_id !== null && row.group_id === null) redactMessage(state, row, true, Date.now());
      else {
        pruneMessageOutbox(state, new Set([row.id]));
        state.messages = state.messages.filter(value => value.id !== row.id);
        state.attachments = state.attachments.filter(value => value.message_id !== row.id);
        state.pinnedMessages = state.pinnedMessages.filter(id => id !== row.id);
      }
      return null;
    }
    if (command === 'acknowledge_deletion_notices') {
      acknowledgeBrowserDeletion(state, number(args, 'contactId'), args.messageIds as number[]);
      return null;
    }
    if (command === 'report_peer_event') {
      queuePeerReport(state, number(args, 'messageId'), number(args, 'kind'));
      return null;
    }
    if (command === 'clear_dm_chat' || command === 'clear_message_history') {
      return clearHistory(state, command === 'clear_dm_chat'
        ? { contactId: number(args, 'contactId') } : { preserveSaved: args.preserveSaved === true });
    }
    if (command === 'create_group') {
      const ids = args.memberContactIds as number[];
      if (!Array.isArray(ids) || ids.length < 1 || ids.length > 63 || new Set(ids).size !== ids.length) throw new Error('Invalid group members');
      const members: GroupMember[] = ids.map(id => { const contact = this.getContact(state, id); return { sign_pk: contact.sign_pk, dh_pk: contact.dh_pk, onion: contact.onion, name: contact.name, is_self: false }; });
      members.push({ sign_pk: this.identity!.sign_pk, dh_pk: this.identity!.dh_pk, onion: this.identity!.onion, name: state.displayName, is_self: true });
      const id = hex(crypto.getRandomValues(new Uint8Array(32))); state.groups.push({ id, name: text(args, 'name').slice(0, 128), members, created_at: Date.now(), pinned_at: null, last_message_at: null }); return id;
    }
    if (command === 'add_group_member') {
      const group = state.groups.find(value => value.id === args.groupId); if (!group) throw new Error('Group not found');
      const contact = this.getContact(state, number(args, 'contactId'));
      if (group.members.length >= 64 || group.members.some(value => value.sign_pk === contact.sign_pk)) throw new Error('Group member already exists or group is full');
      group.members.push({ sign_pk: contact.sign_pk, dh_pk: contact.dh_pk, onion: contact.onion, name: contact.name, is_self: false }); return null;
    }
    if (['pin_contact_message', 'unpin_contact_message', 'pin_group_message', 'unpin_group_message'].includes(command)) {
      const groupId = typeof args.groupId === 'string' ? args.groupId : null;
      const contactId = typeof args.contactId === 'number' ? args.contactId : null;
      const row = state.messages.find(value => value.id === args.messageId && visible(value) && sameScope(value, contactId, groupId));
      if (!row) throw new Error('Message not found');
      const unpin = command.startsWith('unpin_');
      state.pinnedMessages = state.pinnedMessages.filter(value => value !== row.id);
      if (!unpin) state.pinnedMessages.push(row.id);
      const letter = payload(this.identity!, state.displayName);
      letter.pin = { sender_sign_pk: unhex(row.author), origin_msg_id: row.origin, unpin };
      if (groupId) letter.group = this.groupWire(state, groupId);
      const peers = groupId ? state.groups.find(value => value.id === groupId)!.members.filter(value => !value.is_self).map(value => value.sign_pk) : [this.getContact(state, contactId!).sign_pk];
      for (const peer of peers) queueLetter(state, peer, structuredClone(letter), null);
      return null;
    }
    if (command === 'delete_group') { const id = text(args, 'groupId'); const rows = new Set(state.messages.filter(value => value.group_id === id).map(value => value.id)); state.groups = state.groups.filter(value => value.id !== id); state.messages = state.messages.filter(value => value.group_id !== id); pruneMessageOutbox(state, rows); state.pinnedMessages = state.pinnedMessages.filter(id => !rows.has(id)); state.attachments = state.attachments.filter(value => !rows.has(value.message_id)); return null; }
    if (command === 'pin_chat' || command === 'unpin_chat') { const target = args.contactId != null ? this.getContact(state, number(args, 'contactId')) : state.groups.find(value => value.id === args.groupId); if (!target) throw new Error('Conversation not found'); target.pinned_at = command === 'pin_chat' ? Date.now() : null; return null; }
    if (command === 'set_attachment_privacy') { state.ui.attachment_privacy = args.enabled === false ? 'false' : 'true'; this.log.debug('settings_update', 'attachment_privacy'); return null; }
    if (command === 'set_relay_mode') { if (args.mode !== 'builtin') throw new Error('This browser uses its own I2P destination'); return null; }
    if (command === 'set_auto_update' && args.enabled === false) return null;
    if (command === 'send_typing') {
      if (typeof args.contactId !== 'number') return null;
      const contact = this.getContact(state, args.contactId);
      if (contact.request !== 'none' || contact.trust === 2) return null;
      const letter = payload(this.identity!, state.displayName); letter.typing = args.typing === true;
      queueReceipt(state, contact.sign_pk, letter); return null;
    }
    throw new Error(`The browser core does not support ${command}`);
  }
}
