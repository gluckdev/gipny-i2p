// Build browser-router/pkg with wasm-pack first. Exercise sparse file references
// and continuation of an established encrypted session without network I/O.
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { pathToFileURL } from 'node:url';
import { build } from 'vite';

test('backup preserves the peer ratchet, shared blobs and partial incoming files', async t => {
  const wasm = new URL('../../browser-router/pkg/gipny_browser_router.js', import.meta.url);
  const dir = await mkdtemp(join(tmpdir(), 'gipny-backup-state-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  await build({ configFile: false, logLevel: 'silent', plugins: [{ name: 'real-wasm', enforce: 'pre',
    resolveId(id) { if (id.endsWith('/pkg/gipny_browser_router.js')) return { id: wasm.href, external: true }; },
  }], build: { outDir: dir, lib: { entry: 'tests/profile-state-entry.ts', formats: ['es'], fileName: () => 'entry.mjs' } } });
  const { BrowserEngine, BrowserBackend, FileStorage, ProfileStorage, newState } = await import(pathToFileURL(join(dir, 'entry.mjs')));
  const { default: init } = await import(wasm);
  await init({ module_or_path: await readFile(new URL('./gipny_browser_router_bg.wasm', wasm)) });
  const own = BrowserEngine.create('source', 'local source pass'), peer = BrowserEngine.create('peer', 'local peer pass');
  t.after(() => { own.close(); own.free(); peer.close(); peer.free(); });
  const ownId = JSON.parse(own.public_info()), peerId = JSON.parse(peer.public_info());
  const message = { origin_msg_id: 1, sent_at: Date.now(), body: 'hello', attachments: [], files: [] };
  own.begin();
  const initLetter = own.encrypt(peerId.sign_pk, peerId.dh_pk, peer.my_bundle(), JSON.stringify(message));
  own.seal('{}'); own.commit();
  peer.begin();
  assert.equal(JSON.parse(peer.decrypt(ownId.sign_pk, ownId.dh_pk, initLetter)).payload.body, 'hello');
  peer.seal('{}'); peer.commit();

  const state = newState('owner');
  function file(byte) {
    const id = byte.repeat(64);
    return { id, key: Buffer.from(own.new_attachment_key(id)).toString('hex'), size: 9, chunk_size: 4 };
  }
  const complete = file('a'), partial = file('b');
  state.attachments.push({ id: 7, size: 9, storage: complete }, { id: 8, size: 9, storage: { ...complete } });
  state.transfers.push({ size: 9, chunk_size: 4, storage: partial, incoming: true, status: 'receiving', held: 'BQ==' });
  const reads = [];
  const sender = new BrowserBackend(() => {});
  sender.engine = own; sender.identity = ownId; sender.state = state;
  sender.files = { async read(file, index) {
    reads.push([file.id[0], index]);
    return new Uint8Array(Math.min(4, 9 - 4 * index)).fill(index + 1);
  } };
  const backup = await sender.invoke('export_identity', { passphrase: 'backup password' });
  assert.deepEqual(reads, [['a', 0], ['a', 1], ['a', 2], ['b', 0], ['b', 2]]);
  assert.deepEqual(sender.state.attachments[0].storage, complete);

  let saved, next = 0;
  const writes = [];
  t.mock.method(ProfileStorage, 'restore', async (_name, populate) => {
    await populate({ directory: {}, persist: async (bytes, gen, mirror) => {
      assert.equal(gen, '1'); assert.equal(mirror, true); saved = bytes;
    } });
  });
  t.mock.method(FileStorage, 'open', async (_dir, engine) => ({
    async create(size, chunk_size) {
      const id = (++next).toString(16).repeat(64);
      return { id, key: Buffer.from(engine.new_attachment_key(id)).toString('hex'), size, chunk_size };
    },
    async write(file, index, bytes) { writes.push([file.id[0], index, [...bytes]]); },
    async close() {},
  }));
  await new BrowserBackend(() => {}).invoke('import_identity_to_profile', { backupFile: backup, backupPass: 'backup password', profile: 'destination', vaultPass: 'new vault password' });
  assert.deepEqual(writes, [['1', 0, [1,1,1,1]], ['1', 1, [2,2,2,2]], ['1', 2, [3]], ['2', 0, [1,1,1,1]], ['2', 2, [3]]]);
  const restored = BrowserEngine.open_password('destination', 'new vault password', saved, new Uint8Array());
  t.after(() => { restored.close(); restored.free(); });
  const metadata = JSON.parse(restored.take_metadata());
  assert.deepEqual(metadata.attachments[0].storage, metadata.attachments[1].storage);
  assert.notEqual(metadata.attachments[0].storage.key, complete.key);
  assert.equal(metadata.transfers[0].held, 'BQ==');
  assert.equal(restored.has_session(peerId.sign_pk), true);
  peer.begin();
  const reply = peer.encrypt(ownId.sign_pk, ownId.dh_pk, new Uint8Array(), JSON.stringify({ ...message, body: 'continued' }));
  restored.begin();
  assert.equal(JSON.parse(restored.decrypt(peerId.sign_pk, peerId.dh_pk, reply)).payload.body, 'continued');
});
