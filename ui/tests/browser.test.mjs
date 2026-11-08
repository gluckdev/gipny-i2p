import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';
import { build } from 'vite';
import { Window } from 'happy-dom';

const dir = await mkdtemp(join(tmpdir(), 'gipny-browser-'));
await build({ configFile: false, logLevel: 'silent', build: {
  outDir: dir, emptyOutDir: false, rolldownOptions: { output: { banner: 'const Event = window.Event;' } },
  lib: { entry: 'tests/browser-entry.ts', formats: ['es'], fileName: () => 'entry.mjs' },
} });
const win = new Window({ url: 'https://gluckdev.github.io/gipny-i2p/app/' });
for (const key of ['window', 'document', 'localStorage', 'navigator', 'Node', 'HTMLElement', 'HTMLInputElement', 'File', 'MessageEvent', 'CloseEvent']) {
  Object.defineProperty(globalThis, key, { value: key === 'window' ? win : win[key], configurable: true });
}
const ui = await import(pathToFileURL(join(dir, 'entry.mjs')).href);
after(async () => { await win.happyDOM.abort(); await rm(dir, { recursive: true, force: true }); });

test('automatic browser history clearing preserves saved chats, files, pins and pending deliveries', () => {
  const state = ui.newBrowserState('Alice');
  state.ui.chat_prefs = JSON.stringify({ saveHistory: { 'contact:1': true, 'contact:2': false, 'group:team': true } });
  state.messages = [
    { id: 1, contact_id: 1, group_id: null, author: 'alice', origin: 1, sent_at: 100, body: 'saved direct' },
    { id: 2, contact_id: 2, group_id: null, author: 'bob', origin: 2, sent_at: 100, body: 'clear direct' },
    { id: 3, contact_id: null, group_id: 'team', author: 'alice', origin: 3, sent_at: 100, body: 'saved group' },
    { id: 4, contact_id: 3, group_id: null, author: 'alice', origin: 4, sent_at: 100, body: 'no choice' },
  ];
  state.attachments = state.messages.map(message => ({ id: message.id, message_id: message.id, data: 'attachment' }));
  state.pinnedMessages = [1, 2, 3, 4];
  state.outbox = [
    { key: 'saved', row: 1, blob: null, payload: { body: 'saved direct' } },
    { key: 'clear', row: 2, blob: null, payload: { body: 'clear direct' } },
    { key: 'prepared', row: 4, blob: [1, 2, 3], payload: { body: 'no choice' } },
  ];
  assert.equal(ui.clearHistory(state, { preserveSaved: true }), 2);
  assert.deepEqual(state.messages.filter(message => message.deleted_at == null).map(message => message.id), [1, 3]);
  assert.equal(state.messages.find(message => message.id === 2).deletion_notice.can_ack, false);
  assert.deepEqual(state.attachments.map(file => file.message_id), [1, 3]);
  assert.deepEqual(state.pinnedMessages, [1, 3]);
  assert.deepEqual(state.outbox.map(item => item.key), ['saved', 'prepared']);
  assert.equal(state.outbox[1].payload, null);
  assert.equal(state.outbox[0].payload.body, 'saved direct');
  assert.equal(ui.clearHistory(state), 2, 'an explicit clear-all ignores retention choices');
  assert.equal(state.messages.filter(message => message.deleted_at == null).length, 0); assert.equal(state.attachments.length, 0); assert.equal(state.pinnedMessages.length, 0);
});

test('missing and malformed browser history preferences never imply consent to save', () => {
  for (const raw of [undefined, '{', 'null', '[]', JSON.stringify({ saveHistory: { 'contact:1': 'true' } })]) {
    const state = ui.newBrowserState('Alice');
    if (raw !== undefined) state.ui.chat_prefs = raw;
    state.messages = [{ id: 1, contact_id: 1, group_id: null, author: 'alice', origin: 1, sent_at: 100, body: 'clear' }];
    assert.equal(ui.clearHistory(state, { preserveSaved: true }), 1);
  }
});

test('browser expiry removes plaintext, attachments and pins while retaining unexpired history', () => {
  const state = ui.newBrowserState('Alice');
  state.messages = [
    { id: 1, author: 'alice', origin: 42, sent_at: 100, expires_at: 999, deleted_at: null, group_id: null, body: 'expired secret', buttons: [[{text:'secret'}]], console: {kind:0}, reply_to: 7 },
    { id: 2, author: 'bob', origin: 5, sent_at: 900, expires_at: 1001, deleted_at: null, group_id: null, body: 'still live' },
  ];
  state.attachments = [{ id: 11, message_id: 1, data: 'expired attachment' }, { id: 12, message_id: 2, data: 'live attachment' }];
  state.pinnedMessages = [1, 2];
  assert.equal(ui.expireHistory(state, 1000), true);
  assert.equal(state.messages[0].body, '');
  assert.equal(state.messages[0].buttons, null);
  assert.equal(state.messages[0].console, null);
  assert.equal(state.messages[0].reply_to, null);
  assert.equal(state.messages[0].deleted_at, 1000);
  assert.equal(state.messages[1].body, 'still live');
  assert.deepEqual(state.attachments.map(file => file.id), [12]);
  assert.deepEqual(state.pinnedMessages, [2]);
  assert.deepEqual(state.tombstones, ['alice:42:100']);
  assert.equal(ui.expireHistory(state, 1000), false, 'no repeated expiry transaction');
});

test('browser expiry preserves prepared ratchet ciphertext and discards unencrypted expired letters', () => {
  const state = ui.newBrowserState('Alice');
  state.messages = [{ id: 1, author: 'alice', origin: 1, sent_at: 1, expires_at: 10, deleted_at: null, group_id: null, body: 'expire' }];
  const ciphertext = [1, 3, 5];
  state.outbox = [
    { key: 'prepared', row: 1, blob: ciphertext, payload: { body: 'expire' } },
    { key: 'unprepared', row: 1, blob: null, payload: { body: 'expire' } },
    { key: 'other', row: 2, blob: null, payload: { body: 'live' } },
  ];
  assert.equal(ui.expireHistory(state, 10), true);
  assert.deepEqual(state.outbox.map(item => item.key), ['prepared', 'other']);
  assert.equal(state.outbox[0].blob, ciphertext, 'exact ciphertext still completes the existing ratchet chain');
  assert.equal(state.outbox[0].payload, null);
  assert.equal(state.outbox[0].row, null);
  assert.equal(state.outbox[1].payload.body, 'live');
});

test('clearing local history removes pending plaintext without abandoning a prepared ratchet chain', () => {
  const state = ui.newBrowserState('Alice');
  const ciphertext = [7, 8, 9];
  state.outbox = [
    { key: 'prepared', row: 1, blob: ciphertext, payload: { body: 'removed history' } },
    { key: 'unprepared', row: 2, blob: null, payload: { body: 'cancelled history' } },
    { key: 'control', row: null, blob: null, payload: { ack_for: 10 } },
    { key: 'retained', row: 3, blob: null, payload: { body: 'retained history' } },
  ];
  ui.pruneMessageOutbox(state, new Set([1, 2]));
  assert.deepEqual(state.outbox.map(item => item.key), ['prepared', 'control', 'retained']);
  assert.equal(state.outbox[0].blob, ciphertext);
  assert.equal(state.outbox[0].row, null);
  assert.equal(state.outbox[0].payload, null);
  assert.equal(state.outbox[1].payload.ack_for, 10);
  assert.equal(state.outbox[2].payload.body, 'retained history');
});

test('browser file handles pass immutable File objects without reading complete attachments', async () => {
  const file = new win.File(['hello'], 'example.txt');
  const handle = ui.registerBrowserFile(file);
  assert.equal(ui.selectedBrowserFiles([handle])[0], file);
  ui.forgetBrowserFiles([handle]);
  assert.throws(() => ui.selectedBrowserFiles([handle]));
});


test('browser notification bootstrap is quiet and permission is requested on first send', async () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'Notification');
  const send = ui.Api.sendMessagePaths;
  let requests = 0, answer = 'default';
  class Notifications {
    static permission = 'default';
    static async requestPermission() { requests++; return answer; }
  }
  Object.defineProperty(globalThis, 'Notification', { configurable: true, value: Notifications });
  ui.Api.sendMessagePaths = async () => 1;
  const toasts = [];
  const store = new ui.Store();
  store.loadMessages = async () => {};
  store.showToast = (message) => toasts.push(message);
  try {
    await store.ensureNotifyPermission();
    assert.equal(requests, 0); assert.deepEqual(toasts, []);
    await store.sendMessage({ kind: 'contact', id: 1 }, 'first send');
    assert.equal(requests, 1); assert.deepEqual(toasts, [], 'dismissed/default permission stays quiet');
    await store.sendMessage({ kind: 'contact', id: 1 }, 'second send');
    assert.equal(requests, 1, 'do not prompt again on every send');
    const denied = new ui.Store();
    denied.loadMessages = async () => {};
    denied.showToast = (message) => toasts.push(message);
    answer = 'denied';
    await denied.sendMessage({ kind: 'contact', id: 1 }, 'ask from a send');
    assert.equal(requests, 2); assert.deepEqual(toasts, [ui.t('web.notifications')]);
  } finally {
    ui.Api.sendMessagePaths = send;
    if (previous) Object.defineProperty(globalThis, 'Notification', previous);
    else delete globalThis.Notification;
  }
});


test('WebRTC requires an explicit relay service and filters LAN / mDNS / IPv6 candidates', () => {
  assert.throws(() => ui.rtcSettings({}), /TURN/);
  const settings = ui.rtcSettings({ rtc_ice_servers: [{ urls: 'turns:turn.example:5349?transport=tcp', username: 'short-lived', credential: 'temporary' }] });
  assert.equal(settings.iceTransportPolicy, 'relay');
  assert.equal(ui.rtcSettings({ rtc_ice_policy: 'all' }).iceTransportPolicy, 'all');
  assert.throws(() => ui.rtcSettings({ rtc_ice_policy: 'automatic' }));
  const prefix = 'v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n';
  const candidate = address => `a=candidate:1 1 UDP 123 ${address} 5678 typ host\r\n`;
  const output = ui.filteredSdp(prefix + candidate('8.8.8.8') + candidate('10.0.0.1') + candidate('127.0.0.1') + candidate('peer.local') + candidate('::1'));
  assert.match(output, /8\.8\.8\.8/);
  assert.doesNotMatch(output, /10\.0\.0\.1|127\.0\.0\.1|peer\.local|::1/);
  assert.throws(() => ui.filteredSdp(prefix + candidate('169.254.169.254')));
  assert.match(ui.filteredSdp(prefix + candidate('127.0.0.1'), true), /127\.0\.0\.1/);
});

test('Worker RTC adapter applies credits, rejects overflow and closes its port on cancellation', async () => {
  let remote;
  const socket = new ui.WorkerRtcSocket('wss://bridge.example/rtc?target=8.8.8.8:1234', 'test', (_endpoint, _protocol, port) => { remote = port; });
  const opened = new Promise(resolve => { socket.onopen = resolve; });
  remote.postMessage({ kind: 'open' }); await opened;
  const written = new Promise(resolve => { remote.onmessage = event => { if (event.data.kind === 'write') resolve(event.data); }; });
  socket.send(new Uint8Array([1, 2, 3]));
  assert.equal(socket.bufferedAmount, 3);
  const packet = await written;
  assert.deepEqual([...new Uint8Array(packet.data)], [1, 2, 3]);
  remote.postMessage({ kind: 'written', size: 3 });
  await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(socket.bufferedAmount, 0);
  const failed = new Promise(resolve => { socket.onclose = resolve; });
  remote.postMessage({ kind: 'data', data: new ArrayBuffer(16385) });
  await failed;
  assert.equal(socket.readyState, 3);
  assert.throws(() => socket.send(new Uint8Array([1])));
  remote.close();
  let cancelled;
  const opening = new ui.WorkerRtcSocket('wss://bridge.example/rtc', 'test', (_url, _protocol, port) => { cancelled = port; });
  const closed = new Promise(resolve => { cancelled.onmessage = event => resolve(event.data.kind); });
  opening.close();
  assert.equal(await closed, 'close');
  cancelled.close();
});

const aliceSign = '01'.repeat(32), bobSign = '02'.repeat(32);
const aliceIdentity = { sign_pk: aliceSign, dh_pk: '03'.repeat(32), onion: 'alice', destination: 'alice', card: '', fingerprint: '' };
const bobIdentity = { ...aliceIdentity, sign_pk: bobSign, onion: 'bob' };
function deletionFixture() {
  const alice = ui.newBrowserState('Alice'), bob = ui.newBrowserState('Bob');
  alice.contacts = [{ id: 1, sign_pk: bobSign }]; bob.contacts = [{ id: 7, sign_pk: aliceSign }];
  const original = { id: 2, contact_id: 1, group_id: null, author: aliceSign, origin: 42, outgoing: true, sent_at: 100, body: 'secret', dm_seq: 1, deleted_at: null, expires_at: null };
  alice.messages = [original]; bob.messages = [{ ...original, id: 8, contact_id: 7, outgoing: false }];
  bob.nextId = 9;
  return { alice, bob, original };
}

test('a copy is reported to the author only, confirmed, and deleted with its message', () => {
  const { alice, bob, original } = deletionFixture();
  // Bob copies Alice's text; he cannot report on his own messages.
  ui.queuePeerReport(bob, 8, 5);
  bob.messages.push({ ...original, id: 9, author: bobSign, origin: 7, outgoing: true });
  assert.throws(() => ui.queuePeerReport(bob, 9, 5));
  assert.throws(() => ui.queuePeerReport(bob, 8, 4));
  ui.queueDeletionControls(bob, bobIdentity);
  const report = bob.outbox.find(item => item.payload?.dm?.kind === 5).payload;
  assert.equal(report.body, '', 'no content travels');
  // Alice sees it under her message, counted; the receipt is kind 8.
  const events = [];
  const receipt = ui.receiveDeletionControl(alice, aliceIdentity, 1, bobSign, report, events);
  assert.equal(receipt.dm.kind, 8);
  ui.receiveDeletionControl(alice, aliceIdentity, 1, bobSign, report, []);
  assert.deepEqual(original.peer_events.map(({ kind, count }) => ({ kind, count })), [{ kind: 5, count: 2 }]);
  assert.equal(events[0].PeerEvent.message_id, 2);
  // Bob stops repeating it once confirmed.
  ui.receiveDeletionControl(bob, bobIdentity, 7, aliceSign, receipt, []);
  assert.deepEqual(bob.messages[0].peer_reports, []);
  assert.ok(!bob.outbox.some(item => item.payload?.dm?.kind === 5));
  // Deleting the message removes what was recorded, and a later report is ignored.
  ui.redactMessage(alice, original, true, 300);
  assert.deepEqual(original.peer_events, []);
  ui.receiveDeletionControl(alice, aliceIdentity, 1, bobSign, report, []);
  assert.deepEqual(original.peer_events, []);
  // A report not yet encrypted goes with Bob's own deletion of the message.
  ui.queuePeerReport(bob, 8, 6);
  ui.queueDeletionControls(bob, bobIdentity);
  ui.redactMessage(bob, bob.messages[0], true, 400);
  assert.ok(!bob.outbox.some(item => item.payload?.dm?.kind === 6));
});

test('browser deletion supports both authors, survives restart, and requires a human OK', () => {
  const { alice, bob, original } = deletionFixture();
  ui.redactMessage(alice, original, true, 200);
  ui.queueDeletionControls(alice, aliceIdentity);
  const deletion = structuredClone(alice.outbox[0].payload);
  assert.equal(deletion.sent_at, 200);
  const events = [];
  const automatic = ui.receiveDeletionControl(bob, bobIdentity, 7, aliceSign, deletion, events);
  ui.receiveDeletionControl(alice, aliceIdentity, 1, bobSign, automatic, []);
  assert.equal(original.delete_pending, false);
  assert.equal(original.deletion_notice.can_ack, false);
  assert.equal(bob.messages[0].deletion_notice.can_ack, true);
  assert.throws(() => ui.acknowledgeBrowserDeletion(alice, 1, [2]));
  assert.throws(() => ui.acknowledgeBrowserDeletion(bob, 999, [8]));
  assert.throws(() => ui.acknowledgeBrowserDeletion(bob, 7, [8, 99]));
  assert.equal(bob.messages[0].deletion.remote_seen, false);
  const restored = JSON.parse(JSON.stringify(bob));
  ui.acknowledgeBrowserDeletion(restored, 7, [8]);
  assert.equal(restored.messages[0].deletion_notice, null);
  ui.queueDeletionControls(restored, bobIdentity);
  const human = restored.outbox.find(item => item.payload.dm?.kind === 3).payload;
  const receipt = ui.receiveDeletionControl(alice, aliceIdentity, 1, bobSign, human, []);
  assert.equal(original.deletion_notice, null);
  assert.equal(ui.receiveDeletionControl(alice, aliceIdentity, 1, bobSign, human, []).dm.kind, 4, 'lost receipt can be repeated');
  ui.receiveDeletionControl(restored, bobIdentity, 7, aliceSign, receipt, []);
  assert.equal(restored.messages[0].deletion.ack_pending, false);
  ui.receiveDeletionControl(restored, bobIdentity, 7, aliceSign, deletion, []);
  assert.equal(restored.messages[0].deletion_notice, null, 'a repeated delete must not resurrect the notice');
  assert.equal(restored.messages[0].body, '');
  assert.equal(restored.tombstones.length, 1);
  assert.equal(ui.inHistory(restored.messages[0]), true, 'dismissal does not remove the anti-replay row');
});

test('browser accepts recipient deletion of an existing outgoing message but rejects forged/mixed controls', () => {
  const { alice, bob } = deletionFixture();
  ui.redactMessage(bob, bob.messages[0], true, 300);
  ui.queueDeletionControls(bob, bobIdentity);
  const letter = bob.outbox[0].payload;
  assert.throws(() => ui.receiveDeletionControl(alice, aliceIdentity, 1, bobSign, { ...letter, body: 'mixed' }, []));
  ui.receiveDeletionControl(alice, aliceIdentity, 1, bobSign, letter, []);
  assert.deepEqual(alice.messages[0].deletion_notice, { deleted_at: 300, by_sender: false, can_ack: true });
  const forged = structuredClone(letter); forged.dm.reference.author = Array(32).fill(9);
  assert.throws(() => ui.receiveDeletionControl(alice, aliceIdentity, 1, bobSign, forged, []));
});

test('five adjacent deletions collapse; live, dismissed and sequence gaps separate notices', () => {
  const rows = Array.from({ length: 5 }, (_, i) => ({ id: i + 1, dm_seq: i + 1, deleted_at: 200, sent_at: 100 + i, deletion_notice: { deleted_at: 200 + i, by_sender: true, can_ack: true } }));
  assert.deepEqual(ui.deletionRuns(rows).map(run => run.members.length), [5]);
  assert.deepEqual(ui.deletionRuns([rows[0], { id: 20, dm_seq: 2, deleted_at: null }, { ...rows[2], dm_seq: 3 }]).map(run => run.members.length), [1, 1, 1]);
  assert.deepEqual(ui.deletionRuns([rows[0], { ...rows[1], deletion_notice: null }, rows[2]]).map(run => run.members.length), [1, 1]);
  assert.deepEqual(ui.deletionRuns([rows[0], rows[2]]).map(run => run.members.length), [1, 1]);
  // One participant's deletions stay one notice whoever wrote each message;
  // the other participant's deletions start a new one.
  const mixed = rows.map((row, i) => ({ ...row, deletion_notice: { ...row.deletion_notice, by_sender: i % 2 === 0 } }));
  assert.deepEqual(ui.deletionRuns(mixed).map(run => run.members.length), [5]);
  const ours = { ...rows[2], deletion_notice: { ...rows[2].deletion_notice, can_ack: false } };
  assert.deepEqual(ui.deletionRuns([rows[0], rows[1], ours, rows[3]]).map(run => run.members.length), [2, 1, 1]);
});

test('profile selection and boot retain accessible hooks in the shared console design', () => {
  const store = new ui.Store(); store.profiles.set(['Alice']);
  const profiles = new ui.ProfileSelect(store, {});
  assert.equal(profiles.el.classList.contains('profile-salon'), true);
  assert.ok(profiles.el.querySelector('.salon-profile'));
  assert.equal(profiles.el.querySelector('.salon-backdrop').getAttribute('aria-hidden'), 'true');
  profiles.destroy();
  const boot = new ui.AuthBooting(store);
  assert.equal(boot.el.classList.contains('boot-cyber'), true);
  assert.equal(boot.el.querySelector('[role="log"]'), null);
  assert.ok(boot.el.querySelector('[role="progressbar"]'));
  boot.destroy();
});

function fileFixture(size = 2 * 196608 + 17) {
  const own = { sign_pk: '01'.repeat(32), dh_pk: '03'.repeat(32), onion: 'own.i2p' };
  const peer = '02'.repeat(32), state = ui.newBrowserState('Alice');
  state.contacts.push({ id: 1, sign_pk: peer, trust: 1 });
  state.messages.push({ id: 10, contact_id: 1, group_id: null, outgoing: false, author: peer, origin: 10, sent_at: 1, expires_at: null, deleted_at: null, body: '' });
  state.nextId = 11;
  const data = Buffer.alloc(size, 73);
  const offer = { file_id: Array(16).fill(5), name: 'archive.bin', size, sha256: [...createHash('sha256').update(data).digest()], chunk_size: 196608 };
  const parts = new Map();
  let sequence = 0;
  const storage = {
    async create(size, chunk_size) { return { id: String(++sequence), key: 'opaque wrapped key', size, chunk_size }; },
    async write(file, index, value) { parts.set(`${file.id}:${index}`, Buffer.from(value)); },
    async read(file, index) { return new Uint8Array(parts.get(`${file.id}:${index}`)); },
  };
  const chunk = index => ({ ...ui.payload(own, 'Bob'), file_chunk: { file_id: offer.file_id, index, data: [...data.subarray(index * offer.chunk_size, (index + 1) * offer.chunk_size)] } });
  return { own, peer, state, data, offer, storage, chunk };
}

test('browser chunk reception survives restart, checks whole-file hash, and repeats a lost final ACK', async () => {
  const f = fileFixture(), events = [];
  await ui.acceptFileOffers(f.state, f.own, f.peer, 10, [f.offer], f.storage, events);
  const second = await ui.receiveFileControl(f.state, f.own, f.peer, f.chunk(1), f.storage, events);
  assert.equal(second.file_ack.received_up_to, 0);
  assert.deepEqual(second.file_ack.missing, [0]);
  const restored = ui.loadBrowserState(JSON.stringify(f.state));
  assert.deepEqual([0, 1, 2].map(i => ui.partHeld(restored.transfers[0], i)), [false, true, false]);
  assert.equal(typeof restored.transfers[0].held, 'string', 'progress is a compact bitmap');
  assert.equal(JSON.stringify(restored).includes(Buffer.alloc(100, 73).toString('base64')), false, 'file bytes stay outside the snapshot');
  await ui.receiveFileControl(restored, f.own, f.peer, f.chunk(0), f.storage, events);
  const last = await ui.receiveFileControl(restored, f.own, f.peer, f.chunk(2), f.storage, events);
  assert.equal(last, null, 'no receipt names the last part while the whole-file hash runs');
  assert.equal(await ui.receiveFileControl(restored, f.own, f.peer, f.chunk(0), f.storage, events), null, 'nor a duplicate during verification');
  assert.equal(ui.repeatFileReceipt(restored, f.own, f.peer, ui.hex(f.offer.file_id), 0, 0), null);
  assert.equal(restored.attachments.length, 0);
  ui.finishFileVerification(restored, f.own, f.peer, ui.hex(f.offer.file_id), ui.hex(f.offer.sha256), events);
  assert.equal(restored.outbox.at(-1).payload.file_ack.received_up_to, 3, 'the final receipt follows the hash');
  assert.equal(restored.attachments.length, 1);
  assert.equal(restored.attachments[0].data, undefined);
  assert.equal(restored.transfers[0].status, 'complete');
  const again = ui.loadBrowserState(JSON.stringify(restored));
  const ack = await ui.receiveFileControl(again, f.own, f.peer, f.chunk(2), f.storage, events);
  assert.equal(ack.file_ack.received_up_to, 3);
  assert.equal(again.attachments.length, 1, 'duplicate does not create another file');
});

test('a checksum mismatch fails a browser transfer and never acknowledges its final part', async () => {
  const f = fileFixture(10), events = [];
  await ui.acceptFileOffers(f.state, f.own, f.peer, 10, [f.offer], f.storage, events);
  await ui.receiveFileControl(f.state, f.own, f.peer, f.chunk(0), f.storage, events);
  ui.finishFileVerification(f.state, f.own, f.peer, ui.hex(f.offer.file_id), '00'.repeat(32), events);
  assert.equal(f.state.attachments.length, 0);
  assert.equal(f.state.transfers[0].status, 'failed');
  assert.equal(f.state.transfers[0].storage, null, 'the wrapped file key is erased');
  const duplicate = await ui.receiveFileControl(f.state, f.own, f.peer, f.chunk(0), f.storage, events);
  assert.deepEqual(duplicate.file_cancel, f.offer.file_id);
  assert.equal(duplicate.file_ack, null);
});

test('native-style file packets carry the original DM identity, reject forgeries and respect deletion', async () => {
  const f = fileFixture(10), events = [];
  const packet = f.chunk(0);
  packet.dm = { kind: 0, reference: { author: Array(32).fill(2), origin: 10, sent_at: 1 } };
  ui.validateFilePayload(packet);
  await ui.acceptFileOffers(f.state, f.own, f.peer, 10, [f.offer], f.storage, events);
  const forged = structuredClone(packet); forged.dm.reference.author.fill(9);
  await assert.rejects(() => ui.receiveFileControl(f.state, f.own, f.peer, forged, f.storage, events), /author/);
  const misplaced = structuredClone(packet); misplaced.dm.reference.origin++;
  await assert.rejects(() => ui.receiveFileControl(f.state, f.own, f.peer, misplaced, f.storage, events), /another message/);
  await ui.receiveFileControl(f.state, f.own, f.peer, packet, f.storage, events);
  assert.equal(f.state.transfers[0].status, 'verifying');
  f.state.tombstones.push(`${f.peer}:10:1`);
  assert.equal(await ui.receiveFileControl(f.state, f.own, f.peer, packet, f.storage, events), null);
  const mixed = { ...packet, dm: { ...packet.dm, kind: 1 } };
  assert.throws(() => ui.validateFilePayload(mixed), /reference/);
});

test('file-only introductions keep offers through reopening and receive bytes only after acceptance', async () => {
  const f = fileFixture(10), events = [];
  f.state.contacts[0].request = 'incoming';
  const introduction = { ...ui.payload(f.own, ''), origin_msg_id: 10, files: [f.offer] };
  assert.equal(ui.isRequestContent(introduction), true);
  assert.equal(ui.isRequestContent({ ...introduction, file_chunk: f.chunk(0).file_chunk }), false);
  let allocations = 0;
  const storage = { ...f.storage, async create(...args) { allocations++; return f.storage.create(...args); } };
  await ui.receiveFileControl(f.state, f.own, f.peer, f.chunk(0), storage, events);
  assert.equal(f.state.earlyParts.length, 0, 'unknown request cannot stage early file parts');
  await ui.acceptFileOffers(f.state, f.own, f.peer, 10, [f.offer], storage, events);
  assert.equal(f.state.transfers[0].status, 'offered');
  assert.equal(allocations, 0);
  assert.equal(await ui.receiveFileControl(f.state, f.own, f.peer, f.chunk(0), storage, events), null);
  const restored = ui.loadBrowserState(JSON.stringify(f.state));
  restored.contacts[0].request = 'none';
  await ui.acceptPendingFiles(restored, f.own, f.peer, storage, events);
  assert.equal(allocations, 1);
  assert.ok(restored.outbox.some(item => item.payload?.file_ack?.resumed));
  await ui.receiveFileControl(restored, f.own, f.peer, f.chunk(0), storage, events);
  ui.finishFileVerification(restored, f.own, f.peer, ui.hex(f.offer.file_id), ui.hex(f.offer.sha256), events);
  assert.equal(restored.transfers[0].status, 'complete');
});

test('an early file part cannot be rebound to a different message on offer arrival', async () => {
  const f = fileFixture(10), events = [];
  const packet = f.chunk(0);
  packet.dm = { kind: 0, reference: { author: Array(32).fill(2), origin: 999, sent_at: 1 } };
  await ui.receiveFileControl(f.state, f.own, f.peer, packet, f.storage, events);
  assert.equal(f.state.earlyParts.length, 1);
  await ui.acceptFileOffers(f.state, f.own, f.peer, 10, [f.offer], f.storage, events);
  assert.equal(ui.partHeld(f.state.transfers[0], 0), false);
  assert.equal(f.state.earlyParts.length, 0);
});

test('two GiB offers use a bounded bitmap; browser quota replaces the old four MiB ceiling', async () => {
  const f = fileFixture(1);
  f.offer.size = 2 * 1024 * 1024 * 1024;
  const letter = { ...ui.payload(f.own, ''), origin_msg_id: 1, files: [f.offer] };
  ui.validateFilePayload(letter);
  await ui.acceptFileOffers(f.state, f.own, f.peer, 10, [f.offer], f.storage, []);
  assert.equal(f.state.transfers[0].status, 'receiving');
  assert.equal(ui.readBits(f.state.transfers[0].held, 10923).length, 1366);
  assert.ok(JSON.stringify(f.state).length < 8 * 1024, 'a 2 GiB bitmap costs under 2 KiB of snapshot');
  assert.equal(ui.availableFileBytes({ quota: 8 * 1024 ** 3, usage: 1024 ** 3 }), 7 * 1024 ** 3 - 64 * 1024 ** 2);
  assert.ok(ui.storedSize(f.offer.size) < f.offer.size + 1024 * 1024);
  assert.throws(() => ui.validateFilePayload({ ...letter, files: [{ ...f.offer, size: f.offer.size + 1 }] }));
});

test('quota refusal leaves an explicit failed file in history and cancels the remote sender', async () => {
  const f = fileFixture(1);
  const storage = { ...f.storage, async create() { throw new DOMException('quota', 'QuotaExceededError'); } };
  await ui.acceptFileOffers(f.state, f.own, f.peer, 10, [f.offer], storage, []);
  assert.equal(ui.browserFileProgress(f.state, 10)[0].status, 'failed');
  assert.ok(f.state.outbox.some(item => item.payload?.file_cancel));
  assert.equal(f.state.messages.length, 1);
});

test('browser cancellation and deletion remove queued plaintext and preserve prepared ratchet ciphertext', async () => {
  const f = fileFixture(10);
  f.state.messages[0].outgoing = true;
  f.state.attachments.push({ id: 11, message_id: 10, name: f.offer.name, size: 10, storage: { id: '1', key: 'wrapped', size: 10, chunk_size: 196608 } });
  ui.offerOutgoingFile(f.state, 10, f.peer, f.offer, 11);
  f.state.outbox.push({ key: 'prepared', peer: f.peer, row: null, blob: ui.base64([1, 2, 3]), payload: null, initial: true, at: 0, attempts: 1, deposited: false, file: { id: ui.hex(f.offer.file_id), index: 0, message_id: 10 } });
  f.state.outbox.push({ key: 'not-prepared', peer: f.peer, row: null, blob: null, payload: null, at: 0, attempts: 0, deposited: false, file: { id: ui.hex(f.offer.file_id), index: 0, message_id: 10 } });
  ui.cancelBrowserFiles(f.state, f.own, 10, []);
  assert.ok(!f.state.outbox.some(item => item.key === 'not-prepared'));
  assert.deepEqual([...ui.fromBase64(f.state.outbox.find(item => item.key === 'prepared').blob)], [1, 2, 3]);
  assert.equal(f.state.outbox.find(item => item.key === 'prepared').file, undefined);
  assert.equal(ui.browserFileProgress(f.state, 10)[0].status, 'cancelled');
});

test('public network configuration survives an unavailable or removed host without trusting invalid replacements', async () => {
  const source = new URL('https://app.example.test/browser-config.json');
  const config = { snapshot_url: 'https://peers.example.test/current.tar.gz', snapshot_sha256: 'a'.repeat(64), forwarder_url: 'wss://bridge.example.test/stream', capability: 'b'.repeat(43), transport: 'wss' };
  const entries = new Map();
  const storage = { async open() { return {
    async match(key) { return entries.get(key)?.clone(); },
    async put(key, value) { entries.set(key, value.clone()); },
    async delete(key) { return entries.delete(key); },
  }; } };
  const live = async () => new Response(JSON.stringify({ ...config, private_field: 'must not persist' }));
  const offline = async () => { throw new Error('Application host unavailable'); };
  assert.equal((await ui.loadNetworkConfiguration(source, live, storage)).config.forwarder_url, config.forwarder_url);
  assert.equal((await entries.get(source.href).clone().json()).private_field, undefined);
  assert.equal((await ui.loadNetworkConfiguration(source, offline, storage)).config.forwarder_url, config.forwarder_url);
  for (const status of [403, 404, 410, 429, 503]) {
    assert.equal((await ui.loadNetworkConfiguration(source, async () => new Response('', { status }), storage)).config.forwarder_url, config.forwarder_url);
  }
  assert.equal((await ui.loadNetworkConfiguration(source, async () => new Response('{"transport":"direct"}'), storage)).config.transport, 'wss');
  assert.equal(entries.size, 1);
  assert.ok(await ui.loadNetworkConfiguration(source, async () => new Response(' '.repeat(65537)), storage));
  const replacement = { ...config, forwarder_url: 'wss://replacement.example.test/stream' };
  await ui.loadNetworkConfiguration(source, async () => new Response(JSON.stringify(replacement)), storage);
  assert.equal((await ui.loadNetworkConfiguration(source, offline, storage)).config.forwarder_url, replacement.forwarder_url);
  entries.set(source.href, new Response('{"transport":"direct"}'));
  assert.equal(await ui.loadNetworkConfiguration(source, offline, storage), null, 'corrupt local data is not trusted');
  assert.equal(entries.size, 0);
  assert.equal(await ui.loadNetworkConfiguration(source, async () => new Response(' '.repeat(65537)), storage), null);
  assert.equal(await ui.loadNetworkConfiguration(source, offline, { async open() { throw new Error('Cache denied'); } }), null);
  assert.ok(await ui.loadNetworkConfiguration(source, live, { async open() { throw new Error('Cache denied'); } }));
});

test('persisted network configuration is revalidated, including transport policy and endpoint boundaries', () => {
  const good = { snapshot_url: 'https://peers.example.test/current.tar.gz', snapshot_sha256: 'a'.repeat(64), forwarder_url: 'wss://bridge.example.test/stream', capability: 'b'.repeat(43) };
  assert.equal(ui.validateNetworkConfiguration(good).config.transport, 'wss');
  for (const bad of [
    { forwarder_url: 'ws://bridge.example.test/stream' }, { forwarder_url: 'wss://user:pass@bridge.example.test/stream' },
    { forwarder_url: 'wss://bridge.example.test/stream?target=127.0.0.1' }, { snapshot_url: 'http://peers.example.test/current.tar.gz' },
    { snapshot_sha256: 'a'.repeat(63) }, { capability: 'bad' }, { transport: 'direct' }, { transport: 'webrtc' },
  ]) assert.throws(() => ui.validateNetworkConfiguration({ ...good, ...bad }));
});

// ---- Large-file delivery, replay records and automatic letters ----

function outgoingFixture(parts = 20, peers = ['02'.repeat(32)]) {
  const own = { sign_pk: '01'.repeat(32), dh_pk: '03'.repeat(32), onion: 'own.i2p' };
  const state = ui.newBrowserState('Alice');
  peers.forEach((peer, i) => state.contacts.push({ id: i + 1, sign_pk: peer, trust: 1, request: 'none' }));
  state.messages.push({ id: 10, contact_id: 1, group_id: null, outgoing: true, author: own.sign_pk, origin: 10, sent_at: 1, expires_at: null, deleted_at: null, body: '' });
  state.attachments.push({ id: 11, message_id: 10, name: 'big.bin', size: parts * 196608, storage: { id: 'a'.repeat(64), key: 'k', size: parts * 196608, chunk_size: 196608 } });
  const offer = { file_id: Array(16).fill(7), name: 'big.bin', size: parts * 196608, sha256: Array(32).fill(1), chunk_size: 196608 };
  for (const peer of peers) ui.offerOutgoingFile(state, 10, peer, offer, 11, 1000);
  state.nextId = 12;
  const receipt = (fields) => ({ ...ui.payload({ onion: 'peer.i2p' }, 'Bob'), file_ack: { file_id: offer.file_id, received_up_to: 0, seen_to: 0, missing: [], resumed: false, echo_index: 0, echo_sent_at: 0, ...fields } });
  const sealAndStore = (item, at) => { item.blob = ui.base64([9, 9, 9]); item.attempts = 1; item.at = at; return ui.settleDeposit(state, item.key, at); };
  return { own, state, offer, peers, peer: peers[0], receipt, sealAndStore };
}
const refs = (state, peer) => state.outbox.filter(item => item.file && (!peer || item.peer === peer));

test('a part the relay stored is never resent; a lost part becomes a new encryption only when overdue or after a restart', async () => {
  const f = outgoingFixture();
  ui.queueFileChunks(f.state, f.own, 1000);
  assert.equal(refs(f.state).length, ui.FILE_WINDOW);
  assert.ok(refs(f.state).every(item => item.blob === null && item.payload === null), 'queued parts are references without bytes');
  const [zero, one] = refs(f.state);
  f.sealAndStore(zero, 2000); f.sealAndStore(one, 2000);
  assert.ok(!f.state.outbox.some(item => item.key === zero.key || item.key === one.key), 'stored ciphertext leaves the snapshot');
  assert.deepEqual(f.state.transfers[0].flight, { 0: 2000, 1: 2000 });
  assert.ok(!ui.dueLetters(f.state, 10 ** 13).some(item => item.file && item.file.index < 2));
  // Part 1 arrived first: part 0 is a hole, but not yet overdue.
  await ui.receiveFileControl(f.state, f.own, f.peer, f.receipt({ seen_to: 2, missing: [0], echo_sent_at: 1500 }), {}, [], 3000);
  assert.equal(f.state.transfers[0].flight[0], 2000, 'a hole is a part still on its way');
  assert.ok(!refs(f.state).some(item => item.file.index === 0));
  assert.ok(ui.partHeld(f.state.transfers[0], 1));
  // The same hole past the timeout: one new reference (a new encryption).
  const late = 2000 + ui.fileRto(f.state.transfers[0]);
  await ui.receiveFileControl(f.state, f.own, f.peer, f.receipt({ seen_to: 2, missing: [0] }), {}, [], late);
  ui.queueFileChunks(f.state, f.own, late);
  assert.equal(f.state.transfers[0].flight[0], undefined);
  const again = refs(f.state).filter(item => item.file.index === 0);
  assert.equal(again.length, 1); assert.equal(again[0].blob, null);
  // A restarted receiver: everything in flight it does not hold goes again now.
  const next = refs(f.state).find(item => item.file.index === 2);
  f.sealAndStore(next, late + 10);
  await ui.receiveFileControl(f.state, f.own, f.peer, f.receipt({ received_up_to: 2, seen_to: 2, resumed: true }), {}, [], late + 20);
  ui.queueFileChunks(f.state, f.own, late + 20);
  assert.equal(refs(f.state).filter(item => item.file.index === 2).length, 1);
  assert.ok(ui.partHeld(f.state.transfers[0], 0), 'the cumulative prefix is held');
});

test('the tail times out without a receipt; restart restarts in-flight timers instead of a burst', () => {
  const f = outgoingFixture(3);
  ui.queueFileChunks(f.state, f.own, 1000);
  for (const item of refs(f.state)) f.sealAndStore(item, 1000);
  assert.equal(ui.fileWorkDue(f.state, 1000 + 1000), false);
  const restored = ui.loadBrowserState(JSON.stringify(f.state));
  ui.resumeFiles(restored, f.own, 50_000);
  assert.deepEqual(Object.values(restored.transfers[0].flight), [50_000, 50_000, 50_000]);
  assert.equal(ui.fileWorkDue(restored, 50_000 + ui.FILE_RTO_FIRST_MS), true);
  ui.queueFileChunks(restored, f.own, 50_000 + ui.FILE_RTO_FIRST_MS);
  assert.equal(refs(restored).length, 3, 'all three tail parts go again, encrypted anew');
});

test('ordinary messages and deletion requests are re-encrypted after deposit, never replayed', () => {
  const f = outgoingFixture(1);
  const letter = { ...ui.payload(f.own, 'Alice'), origin_msg_id: 10, body: 'hello', sent_at: 5000 };
  const message = ui.queueLetter(f.state, f.peer, letter, 10);
  message.blob = ui.base64([1]); message.attempts = 1; message.at = 5000;
  assert.equal(ui.settleDeposit(f.state, message.key, 6000), 10, 'MessageSent once all recipients stored it');
  assert.equal(message.blob, null); assert.equal(message.deposited, true); assert.equal(message.payload.body, 'hello');
  assert.ok(!ui.dueLetters(f.state, 6000 + ui.resendDelay(0) - 1).includes(message));
  assert.ok(ui.dueLetters(f.state, 6000 + ui.resendDelay(0)).includes(message));
  assert.equal(ui.reopenForResend(message), true);
  assert.equal(message.blob, null); assert.equal(message.deposited, false); assert.equal(message.resends, 1);
  const deletion = ui.queueLetter(f.state, f.peer, { ...ui.payload(f.own, 'Alice'), dm: { kind: 1, reference: { author: Array(32).fill(1), origin: 10, sent_at: 1 } } }, null);
  deletion.blob = ui.base64([2]);
  ui.settleDeposit(f.state, deletion.key, 7000);
  assert.ok(f.state.outbox.includes(deletion), 'a deletion request waits for kind 2');
  const typing = ui.queueLetter(f.state, f.peer, { ...ui.payload(f.own, 'Alice'), typing: true }, null);
  typing.blob = ui.base64([3]);
  ui.settleDeposit(f.state, typing.key, 7000);
  assert.ok(!f.state.outbox.includes(typing), 'one-shot letters are finished once stored');
  ui.pruneMessageOutbox(f.state, new Set([10]));
  assert.ok(!f.state.outbox.includes(message), 'a stored letter for erased history is simply dropped');
  const old = ui.queueLetter(f.state, f.peer, { ...letter, sent_at: 1 }, 10); old.deposited = true;
  ui.expireResends(f.state, ui.RESEND_TTL_MS + 2);
  assert.ok(!f.state.outbox.includes(old), 'no receipt for a week: stop re-encrypting');
});

test('replay records stay bounded through a synthetic 12,000-part transfer; X3DH initials are kept per peer', async () => {
  const f = fileFixture(12_000);
  f.offer.chunk_size = 1;
  const chunk = index => ({ ...ui.payload(f.own, 'Bob'), sent_at: index, file_chunk: { file_id: f.offer.file_id, index, data: [73] } });
  await ui.acceptFileOffers(f.state, f.own, f.peer, 10, [f.offer], f.storage, []);
  for (let index = 0; index < 12_000; index++) {
    const ack = await ui.receiveFileControl(f.state, f.own, f.peer, chunk(index), f.storage, []);
    if (ack) ui.queueReceipt(f.state, f.peer, ack);
    ui.rememberEnvelope(f.state, { hash: index.toString(16).padStart(64, '0'), peer: f.peer, ack: null, file: { id: ui.hex(f.offer.file_id), index, sent_at: index } });
  }
  assert.equal(f.state.transfers[0].status, 'verifying');
  assert.equal(f.state.seen.length, ui.RECENT_ENVELOPES);
  assert.equal(f.state.outbox.length, 1, 'unsent receipts for one file coalesce');
  for (let i = 0; i < 60_000; i++) ui.rememberEnvelope(f.state, { hash: `r${i}`, peer: f.peer, ack: null });
  assert.equal(f.state.seen.length, ui.RECENT_ENVELOPES, 'never throws, never grows past the window');
  const initial = h => h.repeat(64).slice(0, 64);
  ui.rememberInitial(f.state, f.peer, initial('a'));
  assert.equal(ui.replayedInitial(f.state, f.peer, initial('a')), true);
  assert.equal(ui.replayedInitial(f.state, '09'.repeat(32), initial('a')), false, 'initial records are per sender');
  for (let i = 0; i < ui.INITIALS_PER_PEER; i++) ui.rememberInitial(f.state, f.peer, createHash('sha256').update(String(i)).digest('hex'));
  assert.equal(f.state.initials[f.peer].length, ui.INITIALS_PER_PEER);
  assert.equal(ui.replayedInitial(f.state, f.peer, initial('a')), false, 'the oldest of this peer is evicted, never another peer\'s');
  assert.ok(JSON.stringify(f.state).length < 256 * 1024, 'the snapshot stays small');
});

test('a letter being sent is not picked twice and does not stall its peer', () => {
  const a = '0a'.repeat(32);
  const f = outgoingFixture(40, [a]);
  ui.queueFileChunks(f.state, f.own, 1000);
  // Encrypted and attempted, its deposit still under way.
  const busy = refs(f.state, a)[0]; busy.blob = ui.base64([1]); busy.attempts = 1; busy.at = 1000;
  const later = 1000 + ui.retryDelay(1);
  const due = ui.dueLetters(f.state, later, 8, new Set([busy.key]));
  assert.ok(!due.includes(busy), 'never sent twice at once');
  assert.ok(due.some(item => item.peer === a && item.blob === null), 'further parts are encrypted meanwhile');
  assert.equal(due.filter(item => item.file && item.peer === a).length, 3, 'the busy part keeps its per-peer slot');
  // Not marked busy, the same state is a failing relay.
  assert.ok(!ui.dueLetters(f.state, later).some(item => item.peer === a && item.blob === null));
});

test('a silent relay stream is given up well before the transport timeouts', async () => {
  assert.ok(ui.RELAY_ANSWER_MS <= 60_000, 'shorter than write (60 s) plus read (120 s)');
  assert.equal(await ui.answerWithin(Promise.resolve('Deposited'), 50), 'Deposited');
  await assert.rejects(ui.answerWithin(new Promise(() => {}), 20), /did not answer in time/);
  await assert.rejects(ui.answerWithin(Promise.reject(new Error('closed')), 50), /closed/);
});

test('one offline or blocked recipient does not hold the others; silent transfers fail', async () => {
  const [a, b, c] = ['0a'.repeat(32), '0b'.repeat(32), '0c'.repeat(32)];
  const f = outgoingFixture(40, [a, b, c]);
  f.state.contacts[2].trust = 2;
  ui.queueFileChunks(f.state, f.own, 1000);
  assert.equal(refs(f.state, a).length, ui.FILE_WINDOW);
  assert.equal(refs(f.state, b).length, ui.FILE_WINDOW, 'windows are per recipient');
  assert.equal(refs(f.state, c).length, 0, 'nothing for a blocked contact');
  // A's relay keeps failing with a part already encrypted: no further A parts
  // are encrypted, while B's still are.
  const stuck = refs(f.state, a)[0]; stuck.blob = ui.base64([1]); stuck.attempts = 3; stuck.at = 1000;
  const due = ui.dueLetters(f.state, 1000 + ui.retryDelay(3));
  assert.ok(due.includes(stuck), 'the stored ciphertext itself is retried byte for byte (not yet deposited)');
  assert.ok(!due.some(item => item.peer === a && item.blob === null));
  assert.ok(due.some(item => item.peer === b));
  assert.ok(due.filter(item => item.file && item.blob === null).length <= ui.SEALED_PART_LIMIT);
  // B stored parts but never answered for a day: give up, tell B once.
  const toB = f.state.transfers.find(file => file.peer === b);
  toB.deposited_at = 2000;
  const day = 1000 + ui.FILE_SILENT_MS + 1;
  assert.deepEqual(ui.stalledFiles(f.state, day), [toB]);
  const events = [];
  ui.failStalledFiles(f.state, f.own, day, events);
  assert.equal(toB.status, 'failed');
  assert.ok(f.state.outbox.some(item => item.peer === b && item.payload?.file_cancel));
  assert.equal(refs(f.state, b).length, 0);
  assert.equal(events[0].FileFailed.contact_id, 2);
  // A part that cannot be read fails only its own transfer.
  ui.failOutgoingFile(f.state, f.own, a, ui.hex(f.offer.file_id), []);
  assert.equal(f.state.transfers.find(file => file.peer === a).status, 'failed');
  assert.equal(f.state.transfers.find(file => file.peer === c).status, 'sending');
});

test('receipts have reserved outbox room, coalesce, and never fail the letter they answer', () => {
  const f = outgoingFixture(1);
  while (f.state.outbox.length < ui.OUTBOX_LIMIT - ui.RECEIPT_RESERVE) ui.queueLetter(f.state, f.peer, ui.payload(f.own, 'Alice'), null);
  assert.throws(() => ui.queueLetter(f.state, f.peer, ui.payload(f.own, 'Alice'), null), /Outbox is full/);
  const ack = (resumed, upTo) => ({ ...ui.payload(f.own, 'Alice'), file_ack: { file_id: f.offer.file_id, received_up_to: upTo, seen_to: upTo, missing: [], resumed, echo_index: 0, echo_sent_at: 0 } });
  assert.equal(ui.queueReceipt(f.state, f.peer, ack(true, 0)), true);
  assert.equal(ui.queueReceipt(f.state, f.peer, ack(false, 5)), true);
  const receipts = f.state.outbox.filter(item => item.payload?.file_ack);
  assert.equal(receipts.length, 1);
  assert.equal(receipts[0].payload.file_ack.received_up_to, 5);
  assert.equal(receipts[0].payload.file_ack.resumed, true, 'a restart signal is never lost by coalescing');
  while (f.state.outbox.length < ui.OUTBOX_LIMIT) ui.queueReceipt(f.state, f.peer, { ...ui.payload(f.own, 'Alice'), ack_for: f.state.outbox.length });
  assert.equal(ui.queueReceipt(f.state, f.peer, { ...ui.payload(f.own, 'Alice'), ack_for: 1 }), false, 'dropped, not thrown');
});

test('clearing a long chat queues a bounded number of deletion requests per contact', () => {
  const state = ui.newBrowserState('Alice');
  const peers = ['0a'.repeat(32), '0b'.repeat(32), '0c'.repeat(32)];
  state.contacts = [{ id: 1, sign_pk: peers[0], trust: 1, request: 'none' }, { id: 2, sign_pk: peers[1], trust: 2, request: 'none' }, { id: 3, sign_pk: peers[2], trust: 0, request: 'incoming' }];
  for (let i = 0; i < 300; i++) state.messages.push({ id: 100 + i, contact_id: 1 + (i % 3), group_id: null, author: peers[i % 3], origin: i + 1, outgoing: false, sent_at: i, body: 'x', deleted_at: null, expires_at: null });
  ui.clearHistory(state);
  ui.queueDeletionControls(state, { sign_pk: '01'.repeat(32), onion: 'own' });
  assert.equal(state.outbox.filter(item => item.peer === peers[0]).length, ui.DELETION_WINDOW);
  assert.equal(state.outbox.filter(item => item.peer !== peers[0]).length, 0, 'no controls to blocked or pending contacts');
  ui.queueDeletionControls(state, { sign_pk: '01'.repeat(32), onion: 'own' });
  assert.equal(state.outbox.length, ui.DELETION_WINDOW, 'idempotent');
});

test('a stranger gets no automatic letters, even for an instantly expiring file offer; accepting drops those obligations', async () => {
  const f = fileFixture(10);
  f.state.contacts[0].request = 'incoming';
  f.state.messages[0].expires_at = 1; // ttl_ms: 0
  await ui.acceptFileOffers(f.state, f.own, f.peer, 10, [f.offer], f.storage, []);
  ui.expireHistory(f.state, 2);
  ui.queueDeletionControls(f.state, f.own);
  ui.pruneFiles(f.state, f.own);
  ui.cancelBrowserFiles(f.state, f.own, 10, []);
  assert.deepEqual(f.state.outbox, [], 'no cancel, no deletion request, no receipt');
  assert.equal(f.state.messages[0].delete_pending, true);
  assert.equal(ui.canSendControls(f.state, f.peer), false);
  ui.forgetRequestObligations(f.state, 1);
  f.state.contacts[0].request = 'none';
  ui.queueDeletionControls(f.state, f.own);
  assert.deepEqual(f.state.outbox, []);
  assert.equal(f.state.messages[0].deletion_notice, null);
  // Completed transfers are forgotten without a cancellation.
  const g = fileFixture(10);
  await ui.acceptFileOffers(g.state, g.own, g.peer, 10, [g.offer], g.storage, []);
  await ui.receiveFileControl(g.state, g.own, g.peer, g.chunk(0), g.storage, []);
  ui.finishFileVerification(g.state, g.own, g.peer, ui.hex(g.offer.file_id), ui.hex(g.offer.sha256), []);
  g.state.outbox = [];
  g.state.messages[0].expires_at = 1;
  ui.expireHistory(g.state, 2);
  ui.pruneFiles(g.state, g.own);
  assert.ok(!g.state.outbox.some(item => item.payload?.file_cancel), 'no stray cancel for a completed file');
  assert.equal(g.state.transfers.length, 0);
  assert.equal(g.state.closedFiles[0].ok, true);
  const referenced = { ...g.chunk(0), dm: { kind: 0, reference: { author: [...Buffer.from(g.peer, 'hex')], origin: 10, sent_at: 1 } } };
  assert.equal(await ui.receiveFileControl(g.state, g.own, g.peer, referenced, g.storage, []), null, 'deleted message: the tombstone stops delayed parts');
  const unreferenced = await ui.receiveFileControl(g.state, g.own, g.peer, g.chunk(0), g.storage, []);
  assert.equal(unreferenced.file_ack.received_up_to, 1, 'a closed complete file only repeats its final receipt');
  assert.equal(g.state.attachments.length, 0, 'and is never resurrected');
});

test('finished transfers are pruned into compact closed ids; capacity refuses a file, never the message', async () => {
  const f = fileFixture(10);
  for (let i = 0; i < ui.TRANSFER_SOFT_LIMIT + 50; i++) {
    f.state.transfers.push({ ...f.offer, file_id: [...Buffer.from(i.toString(16).padStart(32, '0'), 'hex')], peer: f.peer, message_id: 10, incoming: true, status: 'complete', held: '', attachment_id: null, storage: null, final_index: 0, final_sent_at: 0 });
  }
  ui.pruneFiles(f.state, f.own);
  assert.ok(f.state.transfers.length < ui.TRANSFER_SOFT_LIMIT);
  assert.equal(f.state.closedFiles.length, 51);
  const closed = f.state.closedFiles[0];
  const chunk = { ...ui.payload(f.own, 'Bob'), file_chunk: { file_id: [...Buffer.from(closed.id, 'hex')], index: 0, data: [...Buffer.alloc(10, 73)] } };
  const answer = await ui.receiveFileControl(f.state, f.own, f.peer, chunk, f.storage, []);
  assert.equal(answer.file_ack.received_up_to, 1, 'a late part of a forgotten complete file gets the final receipt');
  // Every slot active: the new offer is refused with a cancellation, no throw.
  const g = fileFixture(10);
  for (let i = 0; i < 4096; i++) g.state.transfers.push({ ...g.offer, file_id: [...Buffer.from(i.toString(16).padStart(32, '0'), 'hex')], peer: '0f'.repeat(32), message_id: 10, incoming: false, status: 'sending', held: '', attachment_id: null, storage: null, final_index: 0, final_sent_at: 0, flight: {}, progress_at: 0 });
  await ui.acceptFileOffers(g.state, g.own, g.peer, 10, [g.offer], g.storage, []);
  assert.ok(g.state.outbox.some(item => item.payload?.file_cancel));
});

test('rc2 snapshots load: number-array ciphertext, boolean progress and plaintext parts are converted', () => {
  const f = fileFixture(3 * 196608);
  const old = JSON.parse(JSON.stringify(f.state));
  old.transfers = [
    { ...f.offer, peer: f.peer, message_id: 10, incoming: true, status: 'receiving', parts: [true, false, true], acked: [], attachment_id: null, storage: null, final_index: 0, final_sent_at: 0 },
    { ...f.offer, file_id: Array(16).fill(6), peer: f.peer, message_id: 10, incoming: false, status: 'sending', parts: [], acked: [0, 2], attachment_id: 1, storage: null, final_index: 0, final_sent_at: 0 },
  ];
  old.outbox = [
    { key: 'cipher', peer: f.peer, row: 10, payload: { body: 'x' }, blob: [1, 2, 3], at: 0, attempts: 1, deposited: false },
    { key: 'stored-part', peer: f.peer, row: null, payload: null, blob: [4], at: 0, attempts: 1, deposited: true, file: { id: '06'.repeat(16), index: 1, message_id: 10 } },
    { key: 'plain-part', peer: f.peer, row: null, payload: f.chunk(1), blob: null, at: 0, attempts: 0, deposited: false, file: { id: '06'.repeat(16), index: 1, message_id: 10 } },
  ];
  old.seen = Array.from({ length: 5000 }, (_, i) => ({ hash: String(i), peer: f.peer, ack: null }));
  delete old.closedFiles; delete old.initials;
  const state = ui.loadBrowserState(JSON.stringify(old));
  assert.deepEqual([0, 1, 2].map(i => ui.partHeld(state.transfers[0], i)), [true, false, true]);
  assert.deepEqual([0, 1, 2].map(i => ui.partHeld(state.transfers[1], i)), [true, false, true]);
  assert.deepEqual([...ui.fromBase64(state.outbox[0].blob)], [1, 2, 3]);
  assert.deepEqual(state.outbox.map(item => item.key), ['cipher', 'plain-part']);
  assert.equal(state.outbox[1].payload, null, 'no plaintext part survives in the snapshot');
  assert.equal(state.seen.length, ui.RECENT_ENVELOPES);
  assert.deepEqual(state.closedFiles, []); assert.deepEqual(state.initials, {});
});

/** In-memory OPFS directory whose next handle open can be made to hang. */
function fakeOpfs() {
  const files = new Map(), stats = { opens: 0, open: 0 }, late = [];
  let hang = false;
  const access = name => {
    stats.opens++; stats.open++;
    const bytes = () => files.get(name) ?? new Uint8Array();
    return {
      read(buf, { at }) { const n = Math.max(0, Math.min(buf.length, bytes().length - at)); buf.set(bytes().subarray(at, at + n)); return n; },
      write(buf, { at }) { let data = bytes(); if (data.length < at + buf.length) { const grown = new Uint8Array(at + buf.length); grown.set(data); data = grown; } data.set(buf, at); files.set(name, data); return buf.length; },
      truncate(size) { const data = new Uint8Array(size); data.set(bytes().subarray(0, size)); files.set(name, data); },
      flush() {}, getSize: () => bytes().length, close() { stats.open--; },
    };
  };
  const directory = {
    async getFileHandle(name, { create } = {}) {
      if (!files.has(name)) { if (!create) throw new DOMException('missing', 'NotFoundError'); files.set(name, new Uint8Array()); }
      return { createSyncAccessHandle() {
        if (!hang) return Promise.resolve(access(name));
        hang = false;
        return new Promise(resolve => late.push(() => resolve(access(name))));
      } };
    },
    async removeEntry(name) { files.delete(name); },
    async *entries() { for (const name of files.keys()) yield [name, { kind: 'file' }]; },
  };
  return { profile: { getDirectoryHandle: async () => directory }, stats, late, hangNext: () => { hang = true; } };
}
const plainParts = {
  new_attachment_key: () => new Uint8Array(72),
  seal_attachment_part: (_id, _key, _index, data) => { const out = new Uint8Array(data.length + 40); out.set(data, 40); return out; },
  open_attachment_part: (_id, _key, _index, data) => data.slice(40),
};

test('attachment parts share one handle; a hung open is retryable and never blocks the file', async () => {
  const fake = fakeOpfs();
  const store = await ui.FileStorage.open(fake.profile, plainParts, { open: 30, idle: 40 });
  const file = { id: 'ab'.repeat(32), key: '00'.repeat(72), size: 10, chunk_size: 4 };
  await store.write(file, 0, new Uint8Array([1, 2, 3, 4]));
  await store.write(file, 1, new Uint8Array([5, 6, 7, 8]));
  await store.write(file, 2, new Uint8Array([9, 10]));
  assert.deepEqual([...await store.read(file, 1)], [5, 6, 7, 8]);
  assert.equal(fake.stats.opens, 1, 'consecutive parts reuse the open handle');
  await new Promise(resolve => setTimeout(resolve, 80));
  assert.equal(fake.stats.open, 0, 'an idle handle is closed');
  fake.hangNext();
  await assert.rejects(store.read(file, 0), error => error instanceof ui.StorageBusy);
  assert.deepEqual([...await store.read(file, 2)], [9, 10], 'later operations do not wait behind the hung open');
  fake.late.forEach(open => open());
  await new Promise(resolve => setTimeout(resolve, 0));
  store.close();
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(fake.stats.open, 0, 'the late handle and the live one are both closed');
});

test('log lines carry no keys or addresses; unaccepted contacts get bounded inline storage', () => {
  const line = ui.scrubLog(`peer ${'ab'.repeat(32)} via ${'a'.repeat(52)}.b32.i2p dest ${'A~'.repeat(260)} contact 7`);
  assert.doesNotMatch(line, /abab|aaaa|A~A~/);
  assert.match(line, /contact 7/);
  const state = ui.newBrowserState('Alice');
  state.contacts = [{ id: 1, sign_pk: '0a'.repeat(32), request: 'incoming' }, { id: 2, sign_pk: '0b'.repeat(32), request: 'none' }];
  state.messages = [{ id: 5, contact_id: 1 }, { id: 6, contact_id: 2 }];
  state.attachments = [{ id: 7, message_id: 5, size: 3.5 * 1024 * 1024 }, { id: 8, message_id: 6, size: 100 * 1024 * 1024 }];
  assert.equal(ui.pendingInlineAllowed(state, 1, [{ data: Array(256 * 1024) }]), true);
  assert.equal(ui.pendingInlineAllowed(state, 1, [{ data: Array(1024 * 1024) }]), false);
});

const settleUpdates = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
function updateFixture(overrides = {}) {
  const implementations = {
    currentVersion: async () => '0.5.0', getAttachmentPrivacy: async () => true,
    getChatPrefs: async () => ({ clearOnLock: false }), getRouterSettings: async () => ({ transit: 'balanced', upnp: 'on' }),
    updateConfigured: async () => true, updateAsksForRoot: async () => false, updateInstallsItself: async () => true,
    getAutoUpdate: async () => true, setAutoUpdate: async () => {}, installUpdate: async () => {},
    dismissUpdate: async () => {}, restartApp: async () => {},
    vaultLock: async () => {}, clearMessageHistory: async () => {}, listProfiles: async () => ['ops'],
    ...overrides,
  };
  const saved = Object.fromEntries(Object.keys(implementations).map(key => [key, ui.Api[key]]));
  Object.assign(ui.Api, implementations); ui.setLang('en');
  const store = new ui.Store(), app = new ui.App(store);
  document.body.appendChild(app.el);
  return { store, app, settings() {
    let modal;
    const close = app.openModal(dismiss => (modal = new ui.SettingsModal(store, app, dismiss)).el);
    return { el: modal.el, close };
  }, cleanup() { app.destroy(); Object.assign(ui.Api, saved); } };
}
const release = { version: '0.5.1', notes: 'Release notes', size: 2048 };

for (const [result, key, params] of [
  [{ status: 'current', latest: '0.5.0' }, 'settings.update_current', { version: '0.5.0' }],
  [{ status: 'no_release' }, 'settings.update_no_release', {}],
  [{ status: 'dismissed', version: '0.5.1' }, 'settings.update_dismissed', { version: '0.5.1' }],
  [{ status: 'unavailable' }, 'settings.update_unavailable', {}],
  [{ status: 'unsupported', latest: '0.5.1' }, 'settings.update_unsupported', { version: '0.5.1' }],
  [{ status: 'no_asset', latest: '0.5.1', wanted: 'arm64.apk' }, 'settings.update_no_asset', { version: '0.5.1', asset: 'arm64.apk' }],
]) test(`Settings manual update check explains ${result.status}`, async () => {
  let resolveCheck, checks = 0;
  const f = updateFixture({ checkUpdate: () => { checks++; return new Promise(resolve => { resolveCheck = resolve; }); } });
  try {
    const { el } = f.settings(), check = el.querySelector('[data-check-update]');
    check.click(); check.click();
    assert.equal(checks, 1); assert.equal(check.disabled, true);
    assert.equal(check.textContent, ui.t('settings.update_checking'));
    assert.equal(el.querySelector('[data-update-status]').textContent, ui.t('settings.update_checking'));
    resolveCheck(result); await settleUpdates();
    assert.equal(check.disabled, false); assert.equal(check.textContent, ui.t('settings.update_check'));
    assert.equal(el.querySelector('[data-update-status]').textContent, ui.t(key, params));
    assert.equal(el.querySelector('[data-install-update]').parentElement.classList.contains('hidden'), true);
  } finally { f.cleanup(); }
});

test('Settings offers the checked release immediately, installs and releases subscriptions on close', async () => {
  let installs = 0;
  const f = updateFixture({ checkUpdate: async () => ({ status: 'update', ...release }), installUpdate: async () => { installs++; } });
  try {
    f.store.updateError.set('old failure');
    const { el, close } = f.settings();
    el.querySelector('[data-check-update]').click(); await settleUpdates();
    assert.deepEqual(f.store.updateAvailable.get(), release); assert.equal(f.store.updateError.get(), null);
    assert.match(el.querySelector('[data-update-status]').textContent, /0\.5\.1.*2\.0KB/);
    assert.equal(document.querySelector('.update-modal'), null, 'Settings keeps its install control accessible');
    const install = el.querySelector('[data-install-update]');
    assert.equal(install.parentElement.classList.contains('hidden'), false);
    install.click(); install.click(); await settleUpdates(); assert.equal(installs, 1);
    f.store.handleEvent({ UpdateProgress: { pct: 50, downloaded: 1024, total: 2048 } });
    assert.match(el.querySelector('[data-update-status]').textContent, /50/); assert.equal(install.disabled, true);
    f.store.handleEvent({ UpdateStaged: { version: release.version } });
    assert.equal(install.parentElement.classList.contains('hidden'), true);
    assert.ok(f.app.el.querySelector('[data-restart-update]'));
    const status = el.querySelector('[data-update-status]'), previous = status.textContent;
    close(); f.store.updateAvailable.set({ ...release, version: '0.5.2' });
    assert.equal(status.textContent, previous, 'closed Settings no longer subscribes');
  } finally { f.cleanup(); }
});

test('Settings does not re-offer a release staged before the check resolves', async () => {
  const f = updateFixture({ checkUpdate: async () => {
    f.store.handleEvent({ UpdateStaged: { version: release.version } }); return { status: 'update', ...release };
  } });
  try {
    const { el } = f.settings(); el.querySelector('[data-check-update]').click(); await settleUpdates();
    assert.equal(f.store.updateAvailable.get(), null);
    assert.equal(el.querySelector('[data-install-update]').parentElement.classList.contains('hidden'), true);
    assert.match(el.querySelector('[data-update-status]').textContent, /0\.5\.1/);
  } finally { f.cleanup(); }
});

test('Settings check and install errors preserve a usable retry action', async () => {
  const f = updateFixture({ checkUpdate: async () => { throw new Error('check unavailable'); }, installUpdate: async () => { f.store.handleEvent({ UpdateProgress: { downloaded: 1, total: 4, pct: 25 } }); throw new Error('install denied'); } });
  try {
    f.store.updateAvailable.set(release);
    const { el } = f.settings(), check = el.querySelector('[data-check-update]'), install = el.querySelector('[data-install-update]');
    check.click(); await settleUpdates(); assert.match(el.textContent, /check unavailable/); assert.equal(check.disabled, false);
    assert.equal(install.parentElement.classList.contains('hidden'), false);
    install.click(); await settleUpdates(); assert.match(el.textContent, /install denied/);
    assert.match(f.store.updateError.get(), /install denied/); assert.equal(install.disabled, false);
    assert.equal(f.store.updateProgress.get(), null, 'a command rejection cannot leave the controls stuck downloading');
  } finally { f.cleanup(); }
});

test('Settings reads and persists auto-update, disables concurrent writes and rolls back failures', async () => {
  let reads = 0, fail = false, saved = true; const writes = [];
  const f = updateFixture({ getAutoUpdate: async () => { reads++; return saved; }, setAutoUpdate: async enabled => {
    writes.push(enabled); if (fail) throw new Error('cannot save preference'); saved = enabled;
  } });
  try {
    let modal = f.settings(), auto = modal.el.querySelector('[data-auto-update]');
    assert.equal(auto.disabled, true); await settleUpdates();
    assert.equal(auto.checked, true); assert.equal(auto.disabled, false); assert.equal(reads, 1);
    auto.click(); assert.equal(auto.disabled, true); await settleUpdates();
    assert.equal(auto.checked, false); assert.deepEqual(writes, [false]);
    modal.close(); modal = f.settings(); auto = modal.el.querySelector('[data-auto-update]'); await settleUpdates();
    assert.equal(auto.checked, false, 'reopening reads the saved preference');
    auto.click(); await settleUpdates(); assert.deepEqual(writes, [false, true]);
    fail = true; auto.click(); await settleUpdates();
    assert.equal(auto.checked, true); assert.equal(auto.disabled, false); assert.match(modal.el.textContent, /cannot save preference/);
  } finally { f.cleanup(); }
});

test('Settings reports unreadable automatic-update preferences without inventing a default', async () => {
  const f = updateFixture({ getAutoUpdate: async () => { throw new Error('preference unavailable'); } });
  try {
    const { el } = f.settings(); await settleUpdates();
    assert.equal(el.querySelector('[data-auto-update]').disabled, true); assert.match(el.textContent, /preference unavailable/);
  } finally { f.cleanup(); }
});

test('App handles available, progress and staged events with a persistent restart action', async () => {
  let installs = 0, restarts = 0;
  const f = updateFixture({ installUpdate: async () => { installs++; }, restartApp: async () => { restarts++; } });
  try {
    f.store.handleEvent({ UpdateAvailable: { ...release, notes: '<b>plain notes</b>' } }); await settleUpdates();
    const modal = document.querySelector('.update-modal'); assert.ok(modal);
    assert.match(modal.textContent, /0\.5\.1/); assert.equal(modal.querySelector('b'), null);
    [...modal.querySelectorAll('button')].find(button => button.textContent === ui.t('app.update_now')).click();
    await settleUpdates(); assert.equal(installs, 1);
    f.store.handleEvent({ UpdateProgress: { downloaded: 1024, total: 2048, pct: 50 } });
    assert.equal(modal.querySelector('.update-progress').classList.contains('hidden'), false);
    assert.equal(modal.querySelector('.update-bar').style.width, '50%');
    assert.equal(modal.querySelector('[role="progressbar"]').getAttribute('aria-valuenow'), '50');
    f.store.handleEvent({ UpdateStaged: { version: release.version } });
    assert.equal(document.querySelector('.update-modal'), null);
    assert.equal(f.store.updateAvailable.get(), null); assert.equal(f.store.updateProgress.get(), null);
    const banner = f.app.el.querySelector('.update-banner');
    assert.equal(banner.classList.contains('hidden'), false); assert.match(banner.textContent, /0\.5\.1/);
    f.store.view.set('auth-create'); assert.equal(banner.isConnected, true);
    await f.store.lock();
    assert.equal(f.store.updateStaged.get(), release.version, 'staged process update survives profile lock');
    assert.equal(banner.classList.contains('hidden'), false);
    banner.querySelector('[data-restart-update]').click(); await settleUpdates(); assert.equal(restarts, 1);
  } finally { f.cleanup(); }
});

test('App shows automatic progress without a modal and preserves staging if restart fails', async () => {
  const f = updateFixture({ restartApp: async () => { throw new Error('restart blocked'); } });
  try {
    f.store.handleEvent({ UpdateProgress: { downloaded: 1, total: 4, pct: 25 } });
    const banner = f.app.el.querySelector('.update-banner');
    assert.equal(banner.classList.contains('hidden'), false); assert.match(banner.textContent, /25/);
    f.store.handleEvent({ UpdateStaged: { version: release.version } }); banner.querySelector('button').click(); await settleUpdates();
    assert.equal(f.store.updateStaged.get(), release.version); assert.equal(banner.querySelector('button').disabled, false);
    assert.match(document.querySelector('.toast').textContent, /restart blocked/);
  } finally { f.cleanup(); }
});

test('App dismisses offers and handles Android installer handoff without claiming a restart', async () => {
  const dismissed = [], f = updateFixture({ dismissUpdate: async version => { dismissed.push(version); } });
  try {
    f.store.handleEvent({ UpdateAvailable: release }); await settleUpdates();
    [...document.querySelectorAll('.update-modal button')].find(button => button.textContent === ui.t('app.later')).click();
    await settleUpdates(); assert.deepEqual(dismissed, [release.version]); assert.equal(document.querySelector('.update-modal'), null);
    f.store.handleEvent({ UpdateAvailable: release }); await settleUpdates();
    const modal = f.settings(); await settleUpdates();
    f.store.handleEvent({ UpdateProgress: { downloaded: 1024, total: 1024, pct: 100 } });
    assert.match(modal.el.querySelector('[data-update-status]').textContent, /100%/);
    f.store.handleEvent({ UpdateInstallerOpened: { version: release.version, path: '/downloads/gipny.apk' } });
    assert.match(modal.el.querySelector('[data-update-status]').textContent, /Installer opened/, 'Settings clears downloading status on installer handoff');
    assert.equal(document.querySelector('.update-modal'), null);
    const handoff = document.querySelector('.update-installer-handoff'); assert.ok(handoff);
    assert.match(handoff.textContent, /0\.5\.1/); assert.match(handoff.textContent, /system installer/);
    assert.equal(f.store.updateStaged.get(), null); handoff.querySelector('button').click();
    assert.equal(f.store.updateInstallerOpened.get(), null); assert.equal(document.querySelector('.update-installer-handoff'), null);
    assert.equal(f.store.updateAvailable.get().version, release.version, 'cancelled installation remains retryable');
    assert.match(modal.el.querySelector('[data-update-status]').textContent, /Ready to install/, 'closing handoff restores ready-to-install status in Settings');
    modal.close();
  } finally { f.cleanup(); }
});

test('App surfaces UpdateFailed in the offer and background toast', async () => {
  const f = updateFixture();
  try {
    f.store.handleEvent({ UpdateAvailable: release }); await settleUpdates();
    f.store.handleEvent({ UpdateProgress: { downloaded: 1, total: 4, pct: 25 } });
    f.store.handleEvent({ UpdateFailed: { reason: 'checksum mismatch' } });
    assert.match(document.querySelector('.update-modal').textContent, /checksum mismatch/);
    assert.match(document.querySelector('.toast.err').textContent, /checksum mismatch/); assert.equal(f.store.updateProgress.get(), null);
    document.querySelector('.update-modal button').click();
    f.store.handleEvent({ UpdateFailed: { reason: 'background download failed' } });
    assert.match(document.body.textContent, /background download failed/);
  } finally { f.cleanup(); }
});

test('preview backend covers update commands, statuses, staged application, Android and failures', async () => {
  for (const [mode, status] of [['available', 'update'], ['current', 'current'], ['no_release', 'no_release'], ['staged', 'current'], ['unavailable', 'unavailable'], ['unsupported', 'unsupported'], ['no_asset', 'no_asset'], ['dismissed', 'dismissed']]) {
    const backend = ui.previewUpdates(mode, async () => {}, false, async () => {});
    assert.equal((await backend.invoke('check_update')).status, status);
  }
  const events = [], backend = ui.previewUpdates('available', async event => { events.push(event); }, false, async () => {});
  assert.equal(await backend.invoke('get_auto_update'), true);
  await backend.invoke('set_auto_update', { enabled: false }); assert.equal(await backend.invoke('get_auto_update'), false);
  await backend.announce(); assert.equal(events[0].UpdateAvailable.version, release.version);
  await backend.invoke('install_update');
  assert.deepEqual(events.filter(event => event.UpdateProgress).map(event => event.UpdateProgress.pct), [0, 35, 75, 100]);
  assert.equal(events[events.length - 1].UpdateStaged.version, release.version);
  await backend.invoke('apply_staged_update'); assert.equal(await backend.invoke('current_version'), release.version);
  assert.equal((await backend.invoke('check_update')).status, 'current');
  const dismissed = ui.previewUpdates('available', async () => {}, false, async () => {});
  await dismissed.invoke('dismiss_update', { version: release.version }); assert.equal((await dismissed.invoke('check_update')).status, 'dismissed');
  const android = ui.previewUpdates('available', async event => { events.push(event); }, true, async () => {});
  await android.invoke('install_update'); assert.equal(events[events.length - 1].UpdateInstallerOpened.version, release.version);
  const failed = ui.previewUpdates('error', async event => { events.push(event); }, false, async () => {});
  await assert.rejects(failed.invoke('check_update'), /could not be reached/);
  await assert.rejects(failed.invoke('install_update'), /download failed/);
  assert.match(events[events.length - 1].UpdateFailed.reason, /download failed/);
});
