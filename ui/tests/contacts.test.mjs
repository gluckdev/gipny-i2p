import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { build } from 'vite';
import { Window } from 'happy-dom';

const dir = await mkdtemp(join(tmpdir(), 'gipny-contacts-'));
await build({ configFile: false, logLevel: 'silent', build: {
  outDir: dir, emptyOutDir: false,
  lib: { entry: 'tests/contacts-entry.ts', formats: ['es'], fileName: () => 'entry.mjs' },
} });
const win = new Window({ url: 'http://localhost/' });
for (const key of ['window', 'document', 'localStorage', 'navigator', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'File', 'requestAnimationFrame', 'cancelAnimationFrame']) {
  Object.defineProperty(globalThis, key, { value: key === 'window' ? win : win[key], configurable: true });
}
const ui = await import(pathToFileURL(join(dir, 'entry.mjs')).href);
ui.configureBrowserBackend({ invoke: async () => null, listen: async () => () => {} });
ui.setLang('en');
after(async () => { await win.happyDOM.abort(); await rm(dir, { recursive: true, force: true }); });
const card = { onion: 'a'.repeat(52) + '.b32.i2p', signPk: '12'.repeat(32), dhPk: '34'.repeat(32), name: 'Alice' };
const uri = ui.encodeCard(card.onion, card.signPk, card.dhPk, card.name);
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const tick = () => new Promise(resolve => setTimeout(resolve, 10));
function fixture() {
  const store = new ui.Store(); store.currentProfile.set('owner'); store.view.set('main');
  let saved = [], calls = [], refreshes = 0, closes = 0;
  store.refreshContacts = async () => { refreshes++; store.contacts.set(saved); };
  store.showToast = () => {};
  ui.Api.listContacts = async () => saved;
  ui.Api.addContact = async (...args) => { calls.push(args); saved.push({ sign_pk: args[1], dh_pk: args[2], trust: 0 }); return 1; };
  const modal = new ui.AddContactModal(store, () => { closes++; });
  document.body.replaceChildren(modal.el);
  const paste = (value, inputType = 'insertFromPaste') => {
    const field = modal.el.querySelector('textarea'); field.value = value;
    field.dispatchEvent(new win.InputEvent('input', { bubbles: true, inputType }));
  };
  return { store, modal, paste, calls, saved, refreshes: () => refreshes, closes: () => closes,
    close: () => modal.el.dispatchEvent(new win.Event('gipny-modal-close')) };
}

test('only a complete valid contact URI is parsed, including strict legacy destination validation', () => {
  assert.deepEqual(ui.parseContactImport(uri), card);
  assert.equal(ui.parseContactImport(`gipny:v1:${card.onion}:${card.signPk}:${card.dhPk}:Alice`).name, 'Alice');
  for (const text of ['', 'Alice', uri.slice(0, -1), uri + 'x', 'https://example.invalid/?card=' + uri,
    `gipny:v1:wrong:${card.signPk}:${card.dhPk}`, `gipny:v1:${'b'.repeat(52)}.b32.i2p:${card.signPk}:${card.dhPk}`,
    `gipny:v2:${card.onion}:${card.signPk}:${card.dhPk}:wrong`, uri.replace(/.$/, '!')]) {
    assert.equal(ui.parseContactImport(text), null, text.slice(0, 40));
  }
});

test('paste auto-adds once as unverified; arbitrary input and a typed legacy URI do not submit', async () => {
  const h = fixture();
  for (const text of ['not a card', uri.slice(0, -1)]) h.paste(text);
  h.paste(`gipny:v1:${card.onion}:${card.signPk}:${card.dhPk}`, 'insertText');
  await tick(); assert.equal(h.calls.length, 0);
  h.paste(uri); h.paste(uri); await tick();
  assert.equal(h.calls.length, 1); assert.deepEqual(h.calls[0], [card.onion, card.signPk, card.dhPk, card.name, undefined]);
  assert.equal(h.saved[0].trust, 0); assert.equal(h.closes(), 1);
  h.close();
});

test('manual fields require Add and use the same validation', async () => {
  const h = fixture();
  const fields = h.modal.el.querySelectorAll('input.input');
  for (const [index, value] of [card.onion, '', card.signPk, card.dhPk].entries()) fields[index].value = value;
  await tick(); assert.equal(h.calls.length, 0);
  h.modal.el.querySelector('.modal-footer .btn:last-child').click(); await tick();
  assert.equal(h.calls.length, 1); h.close();
});

test('camera callbacks use the import path, stop scanning and ignore late results after close', async () => {
  const original = ui.QrScanner.prototype.start;
  let scanner;
  ui.QrScanner.prototype.start = async function () { scanner = this; };
  try {
    const h = fixture();
    [...h.modal.el.querySelectorAll('button')].find(button => button.textContent === ui.t('contact.scan_qr')).click();
    scanner.onResult(uri); scanner.onResult(uri); await tick();
    assert.equal(scanner.stopped, true); assert.equal(h.calls.length, 1);
    h.close(); scanner.onResult(uri); await tick(); assert.equal(h.calls.length, 1);
  } finally { ui.QrScanner.prototype.start = original; }
});

test('existing identity is never upserted, so verified trust/relay remain intact', async () => {
  const h = fixture(); h.saved.push({ sign_pk: card.signPk, dh_pk: card.dhPk, trust: 1, relay: 'saved-relay' });
  h.paste(uri); await tick();
  assert.equal(h.calls.length, 0); assert.equal(h.closes(), 1);
  assert.deepEqual(h.saved[0], { sign_pk: card.signPk, dh_pk: card.dhPk, trust: 1, relay: 'saved-relay' }); h.close();
});

test('closing the scanner before camera permission resolves releases the late stream', async () => {
  const descriptor = Object.getOwnPropertyDescriptor(navigator, 'mediaDevices');
  const permission = deferred(); let stopped = 0, callbacks = 0;
  Object.defineProperty(navigator, 'mediaDevices', { configurable: true, value: { getUserMedia: () => permission.promise } });
  try {
    const scanner = new ui.QrScanner(() => { callbacks++; }, () => { callbacks++; });
    const starting = scanner.start(); scanner.stop();
    permission.resolve({ getTracks: () => [{ stop: () => { stopped++; } }] }); await starting;
    assert.equal(stopped, 1); assert.equal(scanner.video.srcObject, null);
    assert.equal(scanner.worker, null); assert.equal(scanner.frame, null); assert.equal(callbacks, 0);
  } finally { if (descriptor) Object.defineProperty(navigator, 'mediaDevices', descriptor); else delete navigator.mediaDevices; }
});

test('closing while video.play waits does not start the decoder after playback resolves', async () => {
  const descriptor = Object.getOwnPropertyDescriptor(navigator, 'mediaDevices');
  const playback = deferred(); let stopped = 0;
  Object.defineProperty(navigator, 'mediaDevices', { configurable: true, value: {
    getUserMedia: async () => ({ getTracks: () => [{ stop: () => { stopped++; } }] }),
  } });
  try {
    const scanner = new ui.QrScanner(() => assert.fail('late result'), () => assert.fail('late error'));
    // The fixture supplies track ownership, not an actual camera device.
    Object.defineProperty(scanner.video, 'srcObject', { configurable: true, writable: true, value: null });
    scanner.video.play = () => playback.promise;
    const starting = scanner.start(); await tick(); scanner.stop(); playback.resolve(); await starting;
    assert.equal(stopped, 1); assert.equal(scanner.video.srcObject, null);
    assert.equal(scanner.worker, null); assert.equal(scanner.frame, null);
  } finally { if (descriptor) Object.defineProperty(navigator, 'mediaDevices', descriptor); else delete navigator.mediaDevices; }
});

test('a permission rejection after close is silent and a stopped scanner cannot restart', async () => {
  const descriptor = Object.getOwnPropertyDescriptor(navigator, 'mediaDevices');
  let reject, calls = 0;
  const permission = new Promise((_, fail) => { reject = fail; });
  Object.defineProperty(navigator, 'mediaDevices', { configurable: true, value: { getUserMedia: () => { calls++; return permission; } } });
  try {
    const scanner = new ui.QrScanner(() => assert.fail('late result'), () => assert.fail('late error'));
    const starting = scanner.start(); scanner.stop(); reject(new Error('permission denied')); await starting;
    await scanner.start(); assert.equal(calls, 1);
  } finally { if (descriptor) Object.defineProperty(navigator, 'mediaDevices', descriptor); else delete navigator.mediaDevices; }
});

test('conflicting identity and own card never submit or silently update', async () => {
  for (const own of [false, true]) {
    const h = fixture();
    if (own) h.store.identity.set({ card: { sign_pk: card.signPk } });
    else h.saved.push({ sign_pk: card.signPk, dh_pk: '56'.repeat(32), trust: 1 });
    h.paste(uri); await tick();
    assert.equal(h.calls.length, 0); assert.equal(h.closes(), 0);
    assert.match(h.modal.el.querySelector('.err').textContent, own ? /own contact/ : /different identity/); h.close();
  }
});

test('cross-modal in-flight guard blocks duplicate writes while the vault check is pending', async () => {
  const h = fixture(), waiting = deferred(); ui.Api.listContacts = () => waiting.promise;
  const other = new ui.ContactImport(h.store, () => {});
  h.paste(uri); assert.equal(h.modal.el.getAttribute('aria-busy'), 'true');
  assert.equal(await other.add(card), 'ignored');
  waiting.resolve([]); await tick(); assert.equal(h.calls.length, 1);
  other.close(); h.close();
});

test('close, lock and profile switch invalidate an in-flight check even if the old profile is selected again', async () => {
  for (const action of ['close', 'lock', 'switch']) {
    const h = fixture(), waiting = deferred(); ui.Api.listContacts = () => waiting.promise;
    h.paste(uri);
    if (action === 'close') h.close();
    if (action === 'lock') h.store.view.set('auth-unlock');
    if (action === 'switch') { h.store.currentProfile.set('other'); h.store.currentProfile.set('owner'); }
    waiting.resolve([]); await tick(); assert.equal(h.calls.length, 0, action); assert.equal(h.closes(), 0); h.close();
  }
});

test('an already dispatched add may finish, but a closed modal cannot refresh/toast the next profile', async () => {
  const h = fixture(), waiting = deferred(); ui.Api.addContact = () => waiting.promise;
  h.paste(uri); await tick(); h.close(); h.store.currentProfile.set('other');
  waiting.resolve(1); await tick(); assert.equal(h.refreshes(), 0); assert.equal(h.closes(), 0);
});

test('failure releases the guard and restores controls for an explicit retry', async () => {
  const h = fixture(); const add = ui.Api.addContact; ui.Api.addContact = async () => { throw new Error('vault busy'); };
  h.paste(uri); await tick(); assert.equal(h.closes(), 0);
  assert.match(h.modal.el.querySelector('.err').textContent, /vault busy/);
  assert.equal(h.modal.el.querySelector('textarea').readOnly, false);
  ui.Api.addContact = add; h.modal.el.querySelector('.modal-footer .btn:last-child').click(); await tick();
  assert.equal(h.calls.length, 1); assert.equal(h.closes(), 1); h.close();
});

test('optional local resource hints enable effects fallback; changes and disposal are observed', () => {
  for (const nav of [{}, { hardwareConcurrency: 8 }, { hardwareConcurrency: 0, deviceMemory: 0 }, { connection: { saveData: false } }]) assert.equal(ui.reducedEffects(nav), false);
  for (const nav of [{ hardwareConcurrency: 2 }, { deviceMemory: 1 }, { connection: { saveData: true } }]) assert.equal(ui.reducedEffects(nav), true);
  const connection = new win.EventTarget(); connection.saveData = false;
  const root = document.createElement('div'), dispose = ui.installEffects(root, { connection });
  assert.equal(root.classList.contains('reduced-effects'), false);
  connection.saveData = true; connection.dispatchEvent(new win.Event('change'));
  assert.equal(root.classList.contains('reduced-effects'), true);
  dispose(); connection.dispatchEvent(new win.Event('change'));
  assert.equal(root.classList.contains('reduced-effects'), false);
});
