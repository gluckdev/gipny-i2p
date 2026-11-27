// The command adapter is the unit under test. Storage and WASM transactions
// are observed here; browser-core-backup.test.mjs exercises their real APIs.
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, test } from 'node:test';
import { pathToFileURL } from 'node:url';
import { build } from 'vite';

const dir = await mkdtemp(join(tmpdir(), 'gipny-browser-commands-'));
const declaration = await readFile('src/browser-core/wasm.d.ts', 'utf8');
const stubs = { application_tunnel_hops: 'return [2, 2]', envelope_is_initial: 'return false' };
const exports = [...declaration.matchAll(/export (class|function) (\w+)/g)].map(([, kind, name]) => {
  if (kind === 'class') return `export class ${name} {}`;
  return `export function ${name}() { ${stubs[name] ?? "throw Error('Unexpected WASM call')"} }`;
}).join('\n');
await build({ configFile: false, logLevel: 'silent', plugins: [{ name: 'test-wasm', enforce: 'pre',
  resolveId(id) { if (id.endsWith('/pkg/gipny_browser_router.js')) return '\0test-wasm'; },
  load(id) { if (id === '\0test-wasm') return exports; },
}], build: { outDir: dir, lib: { entry: 'tests/profile-state-entry.ts', formats: ['es'], fileName: () => 'entry.mjs' } } });
const { BrowserBackend, newState } = await import(pathToFileURL(join(dir, 'entry.mjs')));
after(() => rm(dir, { recursive: true, force: true }));

function setup() {
  const backend = new BrowserBackend(() => {});
  const state = newState('example');
  const peer = { id: 1, name: 'peer', sign_pk: '11'.repeat(32), dh_pk: '22'.repeat(32),
    i2p_address: 'a'.repeat(52) + '.b32.i2p', request: 'none', trust: 1 };
  const own = { ...peer, sign_pk: '33'.repeat(32), dh_pk: '44'.repeat(32), is_self: true };
  const group = '99'.repeat(32);
  state.contacts.push(peer);
  state.groups.push({ id: group, name: 'group', members: [peer, own] });
  const makeRow = (id, group_id) => ({ id, contact_id: group_id ? null : peer.id, group_id, author: peer.sign_pk,
    origin: id + 100, sent_at: Date.now(), expires_at: null, outgoing: false, body: 'reply', buttons: [[{ callback_data: 'next' }]] });
  state.messages.push(makeRow(2, null), makeRow(3, group));
  backend.state = state; backend.identity = own;
  const forgotten = [];
  backend.engine = { begin() {}, commit() {}, rollback() {}, seal() { return Uint8Array.of(1); },
    pending_generation() { return '1'; }, forget_peer(key) { forgotten.push(key); },
    has_session() { return true; }, encrypt() { return Uint8Array.of(9); } };
  backend.storage = { persist: async () => {} };
  return { backend, peer, group, forgotten, rpc: (cmd, args) => backend.invoke(cmd, args) };
}

test('callbacks use the original ID and send group responses only to the author', async () => {
  const { backend, rpc, peer, group } = setup();
  await rpc('press_button', { contactId: 1, messageId: 2, callbackData: 'next' });
  await rpc('press_group_button', { groupId: group, messageId: 3, callbackData: 'next' });
  const [direct, groupReply] = backend.state.outbox;
  assert.equal(direct.peer, peer.sign_pk); assert.equal(direct.payload.group, null);
  assert.equal(direct.payload.origin_msg_id, 102); assert.equal(direct.payload.callback_data, 'next');
  assert.equal(groupReply.peer, peer.sign_pk); assert.equal(groupReply.payload.origin_msg_id, 103);
  assert.equal(Buffer.from(groupReply.payload.group.id).toString('hex'), group);
  for (const args of [{ contactId: 1, messageId: 3, callbackData: 'next' }, { contactId: 1, messageId: 2, callbackData: 'forged' }]) {
    await assert.rejects(rpc('press_button', args), /Button is unavailable/);
  }
  backend.state.contacts[0].trust = 2;
  await assert.rejects(rpc('press_button', { contactId: 1, messageId: 2, callbackData: 'next' }), /sender is unavailable/);
  assert.equal(backend.state.outbox.length, 2);
});

test('direct and group typing coalesce separately and media respects scope, order and limit', async () => {
  const { backend, rpc, group } = setup();
  await rpc('send_typing', { contactId: 1, typing: true });
  await rpc('send_typing', { groupId: group, typing: true });
  await rpc('send_typing', { groupId: group, typing: false });
  assert.deepEqual(backend.state.outbox.map(row => [!!row.payload.group, row.payload.typing]), [[false, true], [true, false]]);
  backend.state.messages.push({ ...backend.state.messages[0], id: 6 });
  backend.state.attachments = [2, 3, 6].map(message_id => ({ id: message_id + 10, message_id, size: 4, name: 'test' }));
  assert.deepEqual((await rpc('list_media_contact', { contactId: 1, limit: 1 })).map(file => file.id), [16]);
  assert.deepEqual(await rpc('list_media_group', { groupId: group, limit: 0 }), []);
});

test('deleting for both hides local history immediately and forgets the session after the wipe is deposited', async () => {
  const { backend, rpc, peer, forgotten } = setup();
  await rpc('delete_contact', { id: 1, forBoth: true });
  assert.deepEqual(await rpc('list_contacts'), []);
  assert.deepEqual(await rpc('list_messages', { contactId: 1 }), []);
  const [item] = backend.state.outbox;
  assert.equal(item.wipeRoute.dh_pk, peer.dh_pk);
  assert.deepEqual(backend.terminalCandidates(peer.sign_pk), [peer.i2p_address]);
  assert.equal(item.payload.wipe, true);
  assert.equal(forgotten.length, 0);
  const deposits = [];
  backend.relay = { async deposit(route, key, bytes) { deposits.push([route, key, [...bytes]]); } };
  await backend.sendItem(item, backend.generation);
  assert.deepEqual(deposits, [[peer.i2p_address, peer.sign_pk, [9]]]);
  assert.deepEqual(forgotten, [peer.sign_pk]); assert.deepEqual(backend.state.outbox, []);
  assert.deepEqual(backend.terminalCandidates(peer.sign_pk), []);
});

test('incoming requests are removed locally and an offline wipe expires', async () => {
  const first = setup();
  first.backend.state.contacts[0].request = 'incoming';
  await first.rpc('delete_contact', { id: 1, forBoth: true });
  assert.deepEqual(first.backend.state.outbox, []);
  assert.deepEqual(first.forgotten, [first.peer.sign_pk]);
  const second = setup();
  await second.rpc('delete_contact', { id: 1, forBoth: true });
  second.backend.state.outbox[0].send_until = Date.now() - 1;
  await second.rpc('set_display_name', { name: 'test' });
  assert.deepEqual(second.backend.state.outbox, []);
  assert.deepEqual(second.forgotten, [second.peer.sign_pk]);
});

test('link statistics reflect an authenticated message receipt and a returning peer', async () => {
  const { backend, rpc, peer } = setup();
  await rpc('send_typing', { contactId: 1, typing: true });
  const [item] = backend.state.outbox;
  const ack = { ...item.payload, typing: null, ack_for: 20 };
  backend.state.messages.push({ ...backend.state.messages[0], id: 20, outgoing: true, origin: 20 });
  item.row = 20; item.at = Date.now() - 123; item.deposited = true;
  backend.engine.decrypt = () => JSON.stringify({ sign_pk: peer.sign_pk, dh_pk: peer.dh_pk, payload: ack });
  backend.unreachable.add(1);
  assert.deepEqual(await rpc('list_unreachable_contacts'), [1]);
  backend.resumePeerRetries(peer.sign_pk, backend.generation);
  assert.deepEqual(await rpc('list_unreachable_contacts'), []);
  await backend.receive(peer.sign_pk, Uint8Array.of(123));
  const stats = await rpc('link_stats', { contactId: 1 });
  assert.ok(stats.rtt_ms >= 123 && stats.rtt_ms < 600000);
  assert.equal((await rpc('link_stats')).rtt_ms, null);
});
