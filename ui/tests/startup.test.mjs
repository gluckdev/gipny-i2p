import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { build } from 'vite';
import { Window } from 'happy-dom';

const dir = await mkdtemp(join(tmpdir(), 'gipny-startup-'));
await build({ configFile: false, logLevel: 'silent', plugins: [{
  name: 'unit-worker-boundary', enforce: 'pre',
  transform(source, id) {
    if (!id.replaceAll('\\', '/').endsWith('/src/browser-core/client.ts')) return;
    // WorkerFixture owns this unit-test boundary. Do not bundle the real WASM
    // worker on native CI, where no generated pkg exists. Production worker
    // emission and initialization are covered by worker-startup.test.mjs.
    const workerUrl = "new URL('./worker.ts', import.meta.url)";
    assert.equal(source.split(workerUrl).length - 1, 1, 'the worker fixture must replace exactly one module URL');
    return source.replace(workerUrl, "'fixture:worker'");
  },
}], build: {
  outDir: dir, emptyOutDir: false,
  lib: { entry: 'tests/startup-entry.ts', formats: ['es'], fileName: () => 'entry.mjs' },
} });
const win = new Window({ url: 'https://app.example.test/browser.html' });
for (const key of ['window', 'document', 'navigator', 'Event', 'CustomEvent']) {
  Object.defineProperty(globalThis, key, { value: key === 'window' ? win : win[key], configurable: true });
}
const { BrowserCoreClient, PublicPrewarm, loadNetworkConfiguration, initializeWorkerWasm, ToastLayer, toastLifetime } = await import(pathToFileURL(join(dir, 'entry.mjs')));
const originalFetch = globalThis.fetch;
const config = { snapshot_url: 'https://seeds.example.test/current.tar.gz', snapshot_sha256: 'a'.repeat(64), forwarder_url: 'wss://bridge.example.test/stream', capability: 'b'.repeat(43) };
const deferred = () => { let resolve, reject; const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; }); return { promise, resolve, reject }; };
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

class WorkerFixture {
  static all = [];
  static error = null;
  messages = []; stopped = false; hosted = 'unconfigured';
  constructor() { this.error = WorkerFixture.error; WorkerFixture.all.push(this); }
  postMessage(message) {
    this.messages.push(message);
    let result = {};
    if (message.command === 'list_profiles') result = ['Alice', 'Bob'];
    if (message.command === 'get_relay_info') result = { hosted: { state: this.hosted } };
    queueMicrotask(() => this.onmessage?.({ data: { id: message.id, ...(this.error ? { error: this.error } : { result }) } }));
  }
  terminate() { this.stopped = true; }
}
globalThis.Worker = WorkerFixture;
after(async () => { globalThis.fetch = originalFetch; await win.happyDOM.abort(); await rm(dir, { recursive: true, force: true }); });

test('profile list and worker initialization proceed while config waits; unlock waits for the config barrier', async () => {
  const request = deferred(); globalThis.fetch = () => request.promise;
  const client = new BrowserCoreClient();
  try {
    const preparation = client.prepare();
    assert.deepEqual(await client.invoke('list_profiles'), ['Alice', 'Bob']);
    const worker = WorkerFixture.all.at(-1);
    const opening = client.invoke('vault_unlock', { profile: 'Alice', pass: 'test' });
    await tick();
    assert.ok(!worker.messages.some(row => row.command === 'vault_unlock'));
    request.resolve(new Response(JSON.stringify(config)));
    await Promise.all([preparation, opening]);
    const configured = worker.messages.findIndex(row => row.command === 'init' && row.args?.config?.capability);
    assert.ok(configured >= 0);
    assert.ok(configured < worker.messages.findIndex(row => row.command === 'vault_unlock'));
    const stats = await client.invoke('browser_diagnostics');
    assert.ok(stats.page_startup_ms.config_loaded >= 0 && stats.page_startup_ms.worker_wasm_ready >= 0);
  } finally { client.terminate(); }
});

test('an online event reloads missing settings once and leaves a live router connected', async () => {
  const client = new BrowserCoreClient();
  let fetches = 0;
  const published = deferred();
  globalThis.fetch = async () => ++fetches === 1 ? new Response(null, { status: 404 }) : published.promise;
  try {
    await client.invoke('vault_unlock', { profile: 'Alice', pass: 'test' });
    const worker = WorkerFixture.all.at(-1);
    const first = client.reconnect(), second = client.reconnect();
    await tick();
    assert.equal(fetches, 2);
    published.resolve(new Response(JSON.stringify(config)));
    await Promise.all([first, second]);
    const configured = worker.messages.findIndex(row => row.command === 'init' && row.args?.config?.capability);
    assert.ok(configured >= 0 && worker.messages.findIndex(row => row.command === 'retry_network') > configured);
    worker.hosted = 'ready';
    const count = worker.messages.length;
    await client.reconnect();
    assert.equal(fetches, 2);
    assert.deepEqual(worker.messages.slice(count).map(row => row.command), ['get_relay_info']);
  } finally { client.terminate(); }
});

test('an online event during sign-in recovers a missing configuration after unlock', async () => {
  const client = new BrowserCoreClient();
  const offline = deferred();
  let fetches = 0;
  globalThis.fetch = async () => ++fetches === 1 ? offline.promise : new Response(JSON.stringify(config));
  try {
    const opening = client.invoke('vault_unlock', { profile: 'Alice', pass: 'test' });
    await tick();
    await client.reconnect();
    await client.reconnect();
    assert.equal(fetches, 1);
    offline.resolve(new Response(null, { status: 404 }));
    await opening;
    await tick();
    const messages = WorkerFixture.all.at(-1).messages;
    assert.equal(fetches, 2);
    const unlock = messages.findIndex(row => row.command === 'vault_unlock');
    const configured = messages.findIndex(row => row.command === 'init' && row.args?.config?.capability);
    assert.ok(configured > unlock && messages.findIndex(row => row.command === 'retry_network') > configured);
    assert.equal(messages.filter(row => row.command === 'retry_network').length, 1);
  } finally { client.terminate(); }
});

test('lock cancels pending configuration and an old unlock cannot initialize or terminate the next profile worker', async () => {
  const requests = [];
  globalThis.fetch = (_url, options) => { const request = deferred(); requests.push({ ...request, signal: options.signal }); return request.promise; };
  const client = new BrowserCoreClient();
  try {
    const preparing = client.prepare().catch(error => error);
    const opening = client.invoke('vault_unlock', { profile: 'Alice', pass: 'old' }).catch(error => error);
    await tick();
    const first = WorkerFixture.all.at(-1);
    await client.invoke('vault_lock');
    assert.ok(first.stopped && requests[0].signal.aborted);
    const secondPreparation = client.prepare();
    const secondOpening = client.invoke('vault_unlock', { profile: 'Bob', pass: 'new' });
    await tick();
    const second = WorkerFixture.all.at(-1); assert.notEqual(first, second);
    requests[0].resolve(new Response(JSON.stringify({ ...config, capability: 'c'.repeat(43) })));
    assert.match(String(await preparing), /cancelled/); assert.match(String(await opening), /cancelled/);
    assert.equal(second.stopped, false, 'stale open failure must not terminate the new worker');
    requests[1].resolve(new Response(JSON.stringify(config)));
    await Promise.all([secondPreparation, secondOpening]);
    assert.equal(second.messages.filter(row => row.command === 'vault_unlock').length, 1);
    assert.equal(second.messages.find(row => row.command === 'vault_unlock').args.profile, 'Bob');
    assert.ok(!second.messages.some(row => row.args?.config?.capability === 'c'.repeat(43)));
  } finally { client.terminate(); }
});

test('freeze/pagehide termination cancels a pending preparation without recreating a worker', async () => {
  const request = deferred(); globalThis.fetch = () => request.promise;
  const client = new BrowserCoreClient();
  const preparing = client.prepare().catch(error => error);
  await tick(); const count = WorkerFixture.all.length;
  client.terminate();
  request.resolve(new Response(JSON.stringify(config)));
  assert.match(String(await preparing), /cancelled/);
  assert.equal(WorkerFixture.all.length, count);
  assert.equal(WorkerFixture.all.at(-1).stopped, true);
});

test('public prewarm is idempotent, bounded to one job and never claims ready tunnels', async () => {
  const calls = [];
  const prewarm = new PublicPrewarm((url, digest) => {
    const result = deferred(); const job = { ...result, url, digest, closed: 0, freed: 0, ready() { return result.promise; }, close() { this.closed++; result.reject(Error('cancelled')); }, free() { this.freed++; } };
    calls.push(job); return job;
  });
  await prewarm.start(null, 'Alice'); assert.equal(calls.length, 0, 'hosting disabled cannot fetch seeds');
  const first = prewarm.start(config);
  assert.equal(first, prewarm.start(config, 'Alice'));
  assert.equal(first, prewarm.start(config, 'Alice')); assert.equal(calls.length, 1);
  assert.equal(prewarm.status(), 'building');
  calls[0].resolve(JSON.stringify({ elapsed_ms: 12, snapshot: { source: 'learned-cache' } }));
  await first;
  assert.equal(prewarm.status(), 'off'); assert.equal(prewarm.diagnostics().state, 'ready');
  assert.equal(prewarm.start(config, 'Alice'), first, 'finished public work also coalesces');
  assert.deepEqual([calls[0].url, calls[0].digest], [config.snapshot_url, config.snapshot_sha256]);
  prewarm.cancel(); assert.equal(prewarm.diagnostics().state, 'off');
});

test('profile/config switch and lock abort public prewarm and discard late completion', async () => {
  const jobs = [];
  const prewarm = new PublicPrewarm(() => {
    const result = deferred(); const job = { ...result, closed: 0, freed: 0, ready: () => result.promise, close() { this.closed++; }, free() { this.freed++; } }; jobs.push(job); return job;
  });
  const alice = prewarm.start(config, 'Alice');
  const bob = prewarm.start(config, 'Bob');
  assert.equal(jobs[0].closed, 1); assert.equal(jobs[0].freed, 1);
  jobs[0].resolve('{"snapshot":{"source":"old"}}'); await alice;
  assert.equal(prewarm.status(), 'building'); assert.equal(prewarm.diagnostics().receipt, null);
  const changed = prewarm.start({ ...config, snapshot_sha256: 'd'.repeat(64) }, 'Bob');
  assert.equal(jobs[1].closed, 1);
  jobs[1].resolve('{}'); await bob;
  prewarm.cancel(); assert.equal(jobs[2].closed, 1);
  jobs[2].resolve('{}'); await changed;
  assert.deepEqual(prewarm.diagnostics(), { state: 'off', receipt: null });
  assert.ok(jobs.every(job => job.freed === 1));
});

test('aborting config never falls back to an old capability', async () => {
  const abort = new AbortController(); let matches = 0;
  const result = loadNetworkConfiguration(new URL('https://app.example.test/browser-config.json'), async (_url, options) => {
    await new Promise(resolve => options.signal.addEventListener('abort', resolve, { once: true })); throw Error('aborted');
  }, { async open() { return { async match() { matches++; return new Response(JSON.stringify(config)); } }; } }, abort.signal);
  await tick(); abort.abort(); await assert.rejects(result, /cancelled/);
  assert.equal(matches, 0);
});

test('a never-settling public cache cannot hold online config or a lock barrier', async () => {
  const unavailable = { open: () => new Promise(() => {}) };
  const source = new URL('https://app.example.test/browser-config.json');
  let requests = 0;
  const live = async () => { requests++; return new Response(JSON.stringify(config)); };
  assert.equal((await loadNetworkConfiguration(source, live, unavailable)).config.capability, config.capability);
  assert.equal(requests, 1);
  const abort = new AbortController();
  const pending = loadNetworkConfiguration(source, live, unavailable, abort.signal);
  abort.abort();
  await assert.rejects(pending, /cancelled/);
  await new Promise(resolve => setTimeout(resolve, 275));
  assert.equal(requests, 1, 'cancelled cache continuation must not start an online request');
});

test('both public cache readers stop streaming at 4 MiB without arrayBuffer allocation', async () => {
  const source = await readFile(new URL('../../browser-router/src/bootstrap-cache.js', import.meta.url), 'utf8');
  const cache = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
  globalThis.location = { href: 'https://app.example.test/browser.html' };
  let cancelled = 0, read = 0;
  const oversized = () => ({ body: { getReader() { return {
    async read() { read++; return { done: false, value: new Uint8Array(1024 * 1024) }; },
    async cancel() { cancelled++; }, releaseLock() {},
  }; } }, arrayBuffer() { throw Error('unbounded allocation'); } });
  const previous = globalThis.caches;
  try {
    globalThis.caches = { async open() { return { async match() { return oversized(); } }; }, async match() { return oversized(); } };
    assert.equal(await cache.loadLiveNetDb(), null);
    assert.equal(await cache.loadBootstrap('a'.repeat(64)), null);
    assert.equal(read, 10); assert.equal(cancelled, 2);
    globalThis.caches = { async match() { return new Response(new Uint8Array([1, 2, 3])); } };
    assert.deepEqual([...await cache.loadLiveNetDb()], [1, 2, 3]);
  } finally { globalThis.caches = previous; }
});

test('our typed client patch applies the same explicit hop policy exported to LinkStats', async () => {
  const patch = await readFile(new URL('../../browser-router/patches/client-only-ntcp2.patch', import.meta.url), 'utf8');
  assert.match(patch, /\+        pub const INBOUND_HOPS: usize = 3;/);
  assert.match(patch, /\+        pub const OUTBOUND_HOPS: usize = 3;/);
  assert.match(patch, /\+            let config = TunnelPoolConfig \{[^\n]*num_inbound_hops: Self::INBOUND_HOPS, num_outbound_hops: Self::OUTBOUND_HOPS/);
  const backend = await readFile(new URL('../src/browser-core/backend.ts', import.meta.url), 'utf8');
  assert.match(backend, /const hops = application_tunnel_hops\(\)/);
});

test('missing Window.WebAssembly does not prevent an available worker from loading', async () => {
  const wasm = globalThis.WebAssembly;
  const client = new BrowserCoreClient();
  try {
    globalThis.WebAssembly = undefined;
    assert.deepEqual(await client.invoke('list_profiles'), ['Alice', 'Bob']);
    assert.equal(WorkerFixture.all.at(-1).stopped, false);
  } finally { globalThis.WebAssembly = wasm; client.terminate(); }
});

test('worker capability and initialization failures settle pending RPCs with neutral exact causes', async () => {
  const unhandled = [];
  const collect = error => unhandled.push(error);
  process.on('unhandledRejection', collect);
  let called = 0;
  try {
    const missing = await initializeWorkerWasm(async () => { called++; }, {});
    assert.equal(called, 0);
    assert.equal(missing.ok, false); assert.match(missing.error, /WASM_API_UNAVAILABLE/);
    assert.doesNotMatch(missing.error, /Lockdown|disabled|CSP/);
    const compile = new WebAssembly.CompileError('unsupported module instruction');
    const failure = initializeWorkerWasm(async () => { throw compile; });
    await tick(); // Initialization may finish before the first RPC subscribes.
    const rejected = await failure;
    assert.equal(rejected.ok, false); assert.match(rejected.error, /CompileError.*unsupported module instruction/);
    assert.doesNotMatch(rejected.error, /Lockdown|CSP/);
    for (const result of [missing, rejected]) {
      WorkerFixture.error = result.error;
      const client = new BrowserCoreClient();
      const first = client.invoke('list_profiles');
      const second = client.invoke('vault_status', { profile: 'Alice' });
      const settled = await Promise.allSettled([first, second]);
      assert.ok(settled.every(value => value.status === 'rejected' && value.reason.message === result.error));
      assert.equal(WorkerFixture.all.at(-1).stopped, true);
      client.terminate();
    }
    await tick(); assert.deepEqual(unhandled, []);
  } finally { process.off('unhandledRejection', collect); WorkerFixture.error = null; }
});

test('centered notices retain full multiline text and live-region semantics without interactive controls', () => {
  const layer = new ToastLayer();
  const text = 'Не удалось загрузить модуль.\n' + 'Длинная причина без сокращения. '.repeat(24) + '\nКонец сообщения.';
  try {
    layer.show({ text, err: true });
    assert.equal(layer.el.textContent, text);
    assert.equal(layer.el.getAttribute('aria-live'), 'assertive');
    assert.equal(layer.el.getAttribute('role'), 'status');
    assert.equal(layer.el.querySelectorAll('button,a,input,[tabindex]').length, 0);
    assert.ok(toastLifetime(text) > toastLifetime('Done'));
    layer.show({ text: 'Готово', err: false });
    assert.equal(layer.el.children.length, 1, 'messages never overlap in a fixed-height stack');
    assert.equal(layer.el.textContent, 'Готово');
    assert.equal(layer.el.getAttribute('aria-live'), 'polite');
    layer.show(null); assert.equal(layer.el.textContent, '');
  } finally { layer.destroy(); }
  assert.equal(layer.el.isConnected, false);
});
