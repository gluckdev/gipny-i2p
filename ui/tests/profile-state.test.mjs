// Real backend open ownership/order, with crypto and storage as fault-observing
// boundaries. This does not replace actual WASM/OPFS browser smoke tests.
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { build } from 'vite';
const dir = await mkdtemp(join(tmpdir(), 'gipny-state-gate-'));
const declaration = await readFile('src/browser-core/wasm.d.ts', 'utf8');
const exports = [...declaration.matchAll(/export (class|function) (\w+)/g)].map(([, kind, name]) => kind === 'class'
  ? `export class ${name} {}` : `export function ${name}() { throw new Error('Unexpected WASM call ${name}'); }`).join('\n');
await build({ configFile: false, logLevel: 'silent', plugins: [{ name: 'state-gate-crypto-boundary', enforce: 'pre',
  resolveId(id) { if (id.endsWith('/pkg/gipny_browser_router.js')) return '\0state-gate-wasm'; },
  load(id) { if (id === '\0state-gate-wasm') return exports; },
}], build: { outDir: dir, emptyOutDir: false,
  lib: { entry: 'tests/profile-state-entry.ts', formats: ['es'], fileName: () => 'entry.mjs' },
} });
const ui = await import(pathToFileURL(join(dir, 'entry.mjs')).href);
after(async () => { await rm(dir, { recursive: true, force: true }); });

test('unsupported snapshots never collect, seal, persist or commit and release engine/storage ownership', async () => {
  for (const version of [1, 3]) for (const passkey of [false, true]) {
    const metadata = JSON.stringify({ ...ui.newState('Saved'), version });
    const saved = [Uint8Array.of(1, 2, 3), Uint8Array.of(4, 5, 6)], before = saved.map(bytes => bytes.slice());
    const calls = [];
    const fail = name => () => { calls.push(name); assert.fail(`Unsupported metadata reached ${name}`); };
    const engine = { take_metadata: () => metadata, close: () => calls.push('engine.close'), free: () => calls.push('engine.free'),
      begin: fail('begin'), seal: fail('seal'), commit: fail('commit'), public_info: fail('public_info') };
    ui.BrowserEngine.open_password = ui.BrowserEngine.open_passkey = () => engine;
    ui.ProfileStorage.open = async () => ({ readSlots: () => saved, close: () => calls.push('storage.close'), persist: fail('persist') });
    ui.FileStorage.open = fail('FileStorage.open');
    const backend = new ui.BrowserBackend(() => {});
    const args = { profile: 'saved', pass: 'test password', credentialId: [1], prf: [2, 3] };
    await assert.rejects(backend.invoke(passkey ? 'vault_unlock_passkey' : 'vault_unlock', args), /Unsupported browser profile format.*not changed/);
    assert.deepEqual(saved, before);
    assert.deepEqual(calls, ['engine.close', 'engine.free', 'storage.close']);
    await assert.rejects(backend.invoke('my_i2p_address'), /Profile is locked/);
    if (passkey) assert.deepEqual(args.prf, [0, 0]);
    backend.closeNow();
    assert.deepEqual(calls, ['engine.close', 'engine.free', 'storage.close'], 'cleanup does not retain or close the old owner twice');
  }
});

test('a mismatched WASM public identity is rejected before ownership or writes even with current metadata', async () => {
  const calls = [], metadata = JSON.stringify(ui.newState('Saved'));
  const engine = { take_metadata: () => metadata, public_info: () => JSON.stringify({ card: 'card', sign_pk: 'sign', dh_pk: 'dh', destination: 'full', fingerprint: 'fp' }),
    close: () => calls.push('engine.close'), free: () => calls.push('engine.free'),
    begin: () => assert.fail('no transaction'), seal: () => assert.fail('no seal') };
  ui.BrowserEngine.open_password = () => engine;
  ui.ProfileStorage.open = async () => ({ readSlots: () => [new Uint8Array(), new Uint8Array()], close: () => calls.push('storage.close'), persist: () => assert.fail('no write') });
  ui.FileStorage.open = () => assert.fail('no file collection');
  const backend = new ui.BrowserBackend(() => {});
  await assert.rejects(backend.invoke('vault_unlock', { profile: 'saved', pass: 'test' }), /Unsupported browser engine identity.*not changed/);
  assert.deepEqual(calls, ['engine.close', 'engine.free', 'storage.close']);
  await assert.rejects(backend.invoke('my_i2p_address'), /Profile is locked/);
  backend.closeNow();
});
