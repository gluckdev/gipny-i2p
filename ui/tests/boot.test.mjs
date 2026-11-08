// Opening a profile in the browser client: what blocks the way in, what the
// boot screen and the main view say about a missing network service, and how
// a failed create/unlock is reported. Same harness as browser.test.mjs.
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { build } from 'vite';
import { Window } from 'happy-dom';

const dir = await mkdtemp(join(tmpdir(), 'gipny-boot-'));
await build({ configFile: false, logLevel: 'silent', build: {
  outDir: dir, emptyOutDir: false, rolldownOptions: { output: { banner: 'const Event = window.Event;' } },
  lib: { entry: 'tests/browser-entry.ts', formats: ['es'], fileName: () => 'entry.mjs' },
} });
const win = new Window({ url: 'https://gluckdev.github.io/gipny-i2p/app/' });
for (const key of ['window', 'document', 'localStorage', 'navigator', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'File', 'MessageEvent', 'CloseEvent', 'requestAnimationFrame', 'cancelAnimationFrame']) {
  Object.defineProperty(globalThis, key, { value: key === 'window' ? win : win[key], configurable: true });
}
const ui = await import(pathToFileURL(join(dir, 'entry.mjs')).href);
// Browser mode: every command the views send goes to a quiet core.
ui.configureBrowserBackend({ invoke: async () => null, listen: async () => () => {} });
ui.setLang('en');
after(async () => { await win.happyDOM.abort(); await rm(dir, { recursive: true, force: true }); });

const { Api, Store, t } = ui;
const tick = () => new Promise((resolve) => setTimeout(resolve, 10));
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const UNCONFIGURED = 'Network service is not configured; saved chats are available offline';
const dial = { attempts: 0, last_error: null, unreachable_checks: 0, unreachable_error: null };
const relayInfo = (hosted) => ({ mode: 'builtin', external: '', hosted, dial });
const step = (id, state, detail = '') => ({ id, state, startedAt: 0, ms: 0, detail });

/** Stub what onUnlocked needs, with the core answering `info` about its relay. */
function bootHarness(info) {
  const original = { boot: Api.onBootStatus, card: Api.myCard, onion: Api.myOnion, fingerprint: Api.myFingerprint, name: Api.getDisplayName,
    events: Api.onEvent, relayInfo: Api.getRelayInfo, relayAddress: Api.getRelayAddress, unreachable: Api.listUnreachableContacts, timeout: Store.BOOT_RELAY_TIMEOUT_MS };
  let boot = () => {};
  Api.onBootStatus = async (handler) => { boot = handler; return () => {}; };
  Api.myCard = async () => ({ sign_pk: 'a', dh_pk: 'b' }); Api.myOnion = async () => 'address';
  Api.myFingerprint = async () => 'fingerprint'; Api.getDisplayName = async () => 'Alice';
  Api.onEvent = async () => () => {};
  Api.getRelayInfo = async () => info; Api.getRelayAddress = async () => ''; Api.listUnreachableContacts = async () => [];
  const store = new Store();
  store.refreshAll = async () => {}; store.ensureNotifyPermission = async () => {}; store.startWatchdog = () => {};
  const restore = () => { store.endBoot(); Object.assign(Api, { onBootStatus: original.boot, myCard: original.card, myOnion: original.onion, myFingerprint: original.fingerprint,
    getDisplayName: original.name, onEvent: original.events, getRelayInfo: original.relayInfo, getRelayAddress: original.relayAddress, listUnreachableContacts: original.unreachable });
    Store.BOOT_RELAY_TIMEOUT_MS = original.timeout; };
  return { store, boot: (...args) => boot(...args), restore };
}

test('a failed relay or relay network never blocks entry; a failed foreground step does', () => {
  assert.equal(ui.isBackgroundStep('relay'), true);
  assert.equal(ui.isBackgroundStep('dht'), true);
  for (const id of ['vault', 'router', 'session', 'inbound', 'outbound', 'tunnels', 'core']) assert.equal(ui.isBackgroundStep(id), false, id);
  assert.equal(ui.bootBlocked([step('vault', 'done'), step('core', 'done'), step('relay', 'failed'), step('dht', 'failed')]), false);
  assert.equal(ui.bootBlocked([step('vault', 'done'), step('core', 'failed'), step('relay', 'failed')]), true);
  assert.equal(ui.bootBlocked([step('vault', 'failed')]), true);
  assert.equal(ui.bootBlocked([step('vault', 'done'), step('relay', 'active')]), false);
});

test('the entry timeout lets the person in past a failed relay but not past a failed profile', async () => {
  assert.equal(typeof Store.BOOT_RELAY_TIMEOUT_MS, 'number');
  for (const blocked of [false, true]) {
    // A configured network whose relay failed: a transient failure that retries.
    const h = bootHarness(relayInfo({ state: 'failed', reason: 'forwarder refused the stream' }));
    Store.BOOT_RELAY_TIMEOUT_MS = 40;
    try {
      await h.store.beginBoot('alice', true);
      h.boot({ stage: 'vault', state: 'done', detail: 'opened' });
      if (blocked) h.boot({ stage: 'core', state: 'failed', detail: 'database is locked' });
      h.boot({ stage: 'relay', state: 'failed', detail: 'forwarder refused the stream' });
      await h.store.onUnlocked('alice');
      assert.equal(h.store.view.get(), 'auth-booting', 'a relay that may still come is waited for');
      assert.equal(h.store.bootCanEnter.get(), true, 'but the door is offered');
      await wait(80);
      assert.equal(h.store.view.get(), blocked ? 'auth-booting' : 'main');
    } finally { h.restore(); }
  }
});

test('without a network service the chats open at once and the relay row says so instead of "retrying"', async () => {
  const h = bootHarness(relayInfo({ state: 'unconfigured', reason: UNCONFIGURED }));
  const view = new ui.AuthBooting(h.store); document.body.append(view.el);
  try {
    await h.store.beginBoot('alice', true);
    h.boot({ stage: 'vault', state: 'done', detail: 'Profile opened' });
    h.boot({ stage: 'relay', state: 'failed', detail: UNCONFIGURED });
    await h.store.onUnlocked('alice');
    assert.equal(h.store.view.get(), 'main', 'nothing is on its way, so nothing is waited for');
    assert.equal(h.store.bootTimer, null, 'no entry timeout is armed');
    const row = view.rows.get('relay');
    assert.equal(row.querySelector('.boot-mark').textContent, t('auth.fail'));
    assert.equal(row.querySelector('.boot-step-hint').textContent, t('boot.network_unconfigured'));
    assert.notEqual(t('boot.network_unconfigured'), t('boot.retrying_in_background'));
    assert.equal(view.backBtn.classList.contains('hidden'), true, 'not a reason to sign in again');
    assert.equal(h.store.relayUnconfigured.get(), false, 'the native "no relay address" state is not borrowed');
  } finally { view.destroy(); h.restore(); }
});

test('a relay that failed with a network service configured keeps the background note', async () => {
  const h = bootHarness(relayInfo({ state: 'failed', reason: 'forwarder refused the stream' }));
  const view = new ui.AuthBooting(h.store); document.body.append(view.el);
  try {
    await h.store.beginBoot('alice', true);
    h.boot({ stage: 'relay', state: 'failed', detail: 'forwarder refused the stream' });
    await h.store.onUnlocked('alice');
    assert.equal(view.rows.get('relay').querySelector('.boot-step-hint').textContent, t('boot.retrying_in_background'));
    assert.equal(h.store.view.get(), 'auth-booting');
  } finally { view.destroy(); h.restore(); }
});

test('the main view banner explains the missing network service and never animates a retry for it', () => {
  const store = new Store(); store.currentProfile.set('alice'); store.view.set('main');
  const app = new ui.App(store); document.body.append(app.el);
  try {
    const banner = app.el.querySelector('.network-ambient'), label = app.el.querySelector('.network-ambient-label');
    store.relayInfo.set(relayInfo({ state: 'unconfigured', reason: UNCONFIGURED }));
    assert.equal(banner.classList.contains('hidden'), false);
    assert.equal(banner.classList.contains('is-offline'), true);
    assert.equal(banner.classList.contains('is-retrying'), false);
    assert.equal(label.textContent, t('app.network_unconfigured'));
    assert.notEqual(t('app.network_unconfigured'), t('app.network_offline'));
    store.relayInfo.set(relayInfo({ state: 'failed', reason: 'forwarder refused the stream' }));
    assert.equal(banner.classList.contains('is-retrying'), true);
    assert.equal(label.textContent, t('app.network_retry'));
    store.relayInfo.set(relayInfo({ state: 'ready', address: 'address' })); store.handleEvent('RelayConnected');
    assert.equal(banner.classList.contains('hidden'), true, 'a connected relay with no contacts to reach is ready');
  } finally { app.destroy(); }
});

test('a failed profile creation stays readable in the rebuilt form, not only in the toast', async () => {
  const original = { create: Api.vaultCreate, boot: Api.onBootStatus };
  Api.onBootStatus = async () => () => {};
  Api.vaultCreate = async () => { throw new Error('OPFS is unavailable in this browser'); };
  const store = new Store(); store.view.set('auth-create');
  const app = new ui.App(store); document.body.append(app.el);
  try {
    const form = app.current;
    form.displayI.value = 'Alice';
    form.passI.value = form.confirmI.value = 'correct horse battery';
    await form.create();
    assert.equal(store.view.get(), 'auth-create');
    assert.notEqual(app.current, form, 'the form is built anew on the way back from the boot screen');
    const shown = app.el.querySelector('.err').textContent;
    assert.match(shown, /OPFS is unavailable in this browser/);
    assert.equal(shown, store.toast.get().text, 'the toast and the form carry the same message');
    assert.equal(store.authError.get(), shown);
    // The next attempt starts clean.
    await store.beginBoot('alice', true);
    assert.equal(store.authError.get(), null);
    assert.equal(app.el.querySelector('.err'), null, 'the boot screen has no form');
    store.endBoot(); store.view.set('auth-create');
    assert.equal(app.el.querySelector('.err').textContent, '');
  } finally { app.destroy(); store.endBoot(); Api.vaultCreate = original.create; Api.onBootStatus = original.boot; }
});

test('a wrong passphrase is reported in the rebuilt unlock form and forgotten on leaving it', async () => {
  const original = { status: Api.vaultStatus, unlock: Api.vaultUnlock, boot: Api.onBootStatus };
  Api.vaultStatus = async () => ({ exists: true, unlocked: false });
  Api.vaultUnlock = async () => { throw new Error('invalid passphrase'); };
  Api.onBootStatus = async () => () => {};
  const store = new Store(); store.currentProfile.set('alice'); store.view.set('auth-unlock');
  const app = new ui.App(store); document.body.append(app.el);
  try {
    await tick();
    const form = app.current;
    form.passI.value = 'not the passphrase';
    await form.unlock('password');
    assert.equal(store.view.get(), 'auth-unlock');
    assert.notEqual(app.current, form);
    assert.equal(app.el.querySelector('.err').textContent, t('auth.wrong_pass'));
    assert.equal(store.toast.get().text, t('auth.wrong_pass'));
    store.view.set('profile-select');
    assert.equal(store.authError.get(), null, 'the profile list does not carry another form\'s failure');
  } finally { app.destroy(); store.endBoot(); Api.vaultStatus = original.status; Api.vaultUnlock = original.unlock; Api.onBootStatus = original.boot; }
});
