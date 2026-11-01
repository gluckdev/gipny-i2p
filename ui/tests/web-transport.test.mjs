import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { build } from 'vite';
import { Window } from 'happy-dom';

const dir = await mkdtemp(join(tmpdir(), 'gipny-web-'));
await build({ configFile: false, logLevel: 'silent', build: {
  outDir: dir, emptyOutDir: false, rolldownOptions: { output: { banner: 'const Event = window.Event;' } },
  lib: { entry: 'tests/web-entry.ts', formats: ['es'], fileName: () => 'entry.mjs' },
} });
const win = new Window({ url: 'https://gluckdev.github.io/gipny-i2p/app/' });
for (const key of ['window', 'document', 'localStorage', 'navigator', 'Node', 'HTMLElement', 'HTMLInputElement', 'File']) {
  Object.defineProperty(globalThis, key, { value: key === 'window' ? win : win[key], configurable: true });
}
const ui = await import(pathToFileURL(join(dir, 'entry.mjs')).href);
after(async () => { await win.happyDOM.abort(); await rm(dir, { recursive: true, force: true }); });
const token = 'A'.repeat(43);
const page = (url) => new URL(url);

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

test('loopback token uses current host; Pages credentials always target native loopback', () => {
  assert.equal(ui.resolveWebEndpoint(page(`http://127.0.0.1:3456/#token=${token}`)), `ws://127.0.0.1:3456/ws?token=${token}`);
  assert.equal(ui.resolveWebEndpoint(page(`http://localhost:3456/#token=${token}`)), `ws://localhost:3456/ws?token=${token}`);
  assert.equal(ui.resolveWebEndpoint(page(`https://gluckdev.github.io/gipny-i2p/app/#port=4567&token=${token}`)), `ws://127.0.0.1:4567/ws?token=${token}`);
  assert.equal(ui.resolveWebEndpoint(page(`http://localhost:3456/#port=4567&token=${token}`)), `ws://127.0.0.1:4567/ws?token=${token}`);
});

test('missing, incomplete and invalid credentials require pairing', () => {
  for (const suffix of ['', '#token=abc', '#port=4567', '#port=0&token=abc', '#port=65536&token=abc', '#port=12evil&token=abc', '#port=80&token=a/b']) {
    assert.equal(ui.resolveWebEndpoint(page(`https://gluckdev.github.io/gipny-i2p/app/${suffix}`)), null, suffix);
  }
  assert.equal(ui.resolveWebEndpoint(page('http://localhost:3456/')), null);
  for (const value of [{}, 'invalid', { token: 42 }, { token, port: 123 }]) {
    assert.equal(ui.resolveWebEndpoint(page('https://example.test/'), value), null);
  }
});

test('fragment is scrubbed and per-tab storage supports reload without changing query/path', () => {
  for (const url of [`http://127.0.0.1:3456/?lang=en#token=${token}`, `https://gluckdev.github.io/gipny-i2p/app/?lang=en#port=4567&token=${token}`]) {
    const browser = new Window({ url });
    const first = ui.readWebEndpoint(browser);
    assert.ok(first);
    assert.equal(browser.location.hash, '');
    assert.equal(browser.location.search, '?lang=en');
    assert.equal(browser.location.pathname, new URL(url).pathname);
    assert.equal(browser.sessionStorage.getItem('gipny.web.pairing'), null, 'launch credentials are still unverified');
    const connection = new URL(first);
    // Simulate a successful web_info; the pairing regression below checks ordering.
    assert.ok(ui.saveWebPairing(connection.port, token, browser));
    assert.equal(ui.readWebEndpoint(browser), first);
    const stored = browser.sessionStorage.getItem('gipny.web.pairing');
    assert.equal(JSON.parse(stored).token, token);
    assert.equal(JSON.parse(stored).port, new URL(first).port);
    const newTab = new Window({ url: browser.location.href });
    assert.equal(ui.readWebEndpoint(newTab), null);
  }
});

test('invalid fragment is scrubbed and replaces stale credentials instead of silently reusing them', () => {
  const browser = new Window({ url: 'https://gluckdev.github.io/gipny-i2p/app/#port=0&token=bad' });
  browser.sessionStorage.setItem('gipny.web.pairing', JSON.stringify({ port: '4567', token }));
  assert.equal(ui.readWebEndpoint(browser), null);
  assert.equal(browser.location.hash, '');
  assert.equal(browser.sessionStorage.getItem('gipny.web.pairing'), null);
});

test('failed handshakes stop reconnecting after three attempts and request pairing', async () => {
  const originalSocket = globalThis.WebSocket;
  const originalTimeout = globalThis.setTimeout;
  let attempts = 0, pairing = 0;
  class Socket {
    static OPEN = 1;
    readyState = 0;
    constructor() { attempts++; queueMicrotask(() => this.onclose?.()); }
    close() { this.onclose?.(); }
  }
  globalThis.WebSocket = Socket;
  // Accelerate only the reconnect interval, retaining the connection timeout.
  globalThis.setTimeout = (callback, delay, ...args) => originalTimeout(callback, delay === 2000 ? 1 : delay, ...args);
  const required = () => pairing++;
  win.addEventListener('gipny-pairing-required', required);
  const client = new ui.WebTransportClient();
  try {
    client.configure(`ws://127.0.0.1:4567/ws?token=${token}`);
    await assert.rejects(client.connect());
    await new Promise((resolve) => originalTimeout(resolve, 50));
    assert.equal(attempts, 3);
    assert.equal(pairing, 1);
    await assert.rejects(client.connect(), /Pairing required/);
    assert.equal(attempts, 3);
  } finally {
    client.configure(null); win.removeEventListener('gipny-pairing-required', required);
    globalThis.WebSocket = originalSocket; globalThis.setTimeout = originalTimeout;
  }
});

test('browser file handles become inline attachment bytes rather than native file paths', async () => {
  const handle = ui.registerBrowserFile(new win.File(['hello'], 'example.txt'));
  assert.deepEqual(await ui.browserAttachments([handle]), [{ name: 'example.txt', data: 'aGVsbG8=' }]);
  await assert.rejects(ui.browserAttachments([handle]));
});


test('pairing stores credentials only after successful web_info and preserves failed form input', async () => {
  const originalSocket = globalThis.WebSocket;
  const originalTimeout = globalThis.setTimeout;
  const sockets = [];
  let pairingEvents = 0;
  class Socket {
    static OPEN = 1;
    readyState = 0;
    constructor() {
      sockets.push(this);
      queueMicrotask(() => { this.readyState = 1; this.onopen?.(); });
    }
    send(data) { this.request = JSON.parse(data); }
    close() { this.readyState = 3; this.onclose?.(); }
    confirm() { this.onmessage?.({ data: JSON.stringify({ id: this.request.id, result: { mode: 'local', version: 'test' } }) }); }
  }
  const required = () => pairingEvents++;
  win.addEventListener('gipny-pairing-required', required);
  globalThis.WebSocket = Socket;
  globalThis.setTimeout = (callback, delay, ...args) => originalTimeout(callback, delay === 2000 ? 1 : delay, ...args);
  const tick = () => new Promise((resolve) => originalTimeout(resolve, 10));
  const root = document.createElement('div');
  win.sessionStorage.clear();
  win.location.hash = `#port=4567&token=${token}`;
  const pairing = ui.pairWeb(root);
  try {
    await tick();
    assert.equal(sockets[0].request.method, 'web_info');
    assert.equal(win.location.hash, '');
    assert.equal(win.sessionStorage.getItem('gipny.web.pairing'), null);
    sockets[0].close();
    await tick();
    assert.equal(root.querySelector('[role=status]').textContent, ui.t('web.failed'), 'initial failure is visible');
    const portInput = root.querySelector('input[name=port]');
    const tokenInput = root.querySelector('input[name=token]');
    portInput.value = '4567'; tokenInput.value = token;
    const submit = () => root.querySelector('form').dispatchEvent(new win.Event('submit', { cancelable: true }));
    submit(); await tick();
    assert.equal(win.sessionStorage.getItem('gipny.web.pairing'), null);
    sockets[1].close(); await tick();
    tokenInput.value = 'still-typing';
    await new Promise((resolve) => originalTimeout(resolve, 50));
    assert.equal(sockets.length, 2, 'a failed submit cancels background reconnects');
    assert.equal(pairingEvents, 0);
    assert.equal(tokenInput.value, 'still-typing');
    assert.equal(win.sessionStorage.getItem('gipny.web.pairing'), null);
    tokenInput.value = token;
    submit(); await tick();
    assert.equal(sockets[2].request.method, 'web_info');
    assert.equal(win.sessionStorage.getItem('gipny.web.pairing'), null);
    sockets[2].confirm(); await pairing;
    assert.deepEqual(JSON.parse(win.sessionStorage.getItem('gipny.web.pairing')), { port: '4567', token });
  } finally {
    ui.configureWebTransport(null);
    win.removeEventListener('gipny-pairing-required', required);
    globalThis.WebSocket = originalSocket; globalThis.setTimeout = originalTimeout;
    win.sessionStorage.clear(); win.location.hash = '';
  }
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
