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
const quietCore = { invoke: async () => null, listen: async () => () => {} };
ui.configureBrowserBackend(quietCore);
ui.setLang('en');
after(async () => { await win.happyDOM.abort(); await rm(dir, { recursive: true, force: true }); });

const { Api, Store, t } = ui;
const tick = () => new Promise((resolve) => setTimeout(resolve, 10));
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const UNCONFIGURED = 'Network service is not configured; saved chats are available offline';
const dial = { attempts: 0, last_error: null, unreachable_checks: 0, unreachable_error: null };
const relayInfo = (hosted) => ({ hosted, dial });
const step = (id, state, detail = '') => ({ id, state, startedAt: 0, ms: 0, detail });

/** Stub what onUnlocked needs, with the core answering `info` about its relay. */
function bootHarness(info) {
  const original = { boot: Api.onBootStatus, card: Api.myCard, i2p_address: Api.myI2pAddress, fingerprint: Api.myFingerprint, name: Api.getDisplayName,
    events: Api.onEvent, relayInfo: Api.getRelayInfo, relayAddress: Api.getRelayAddress, unreachable: Api.listUnreachableContacts, timeout: Store.BOOT_RELAY_TIMEOUT_MS };
  let boot = () => {};
  Api.onBootStatus = async (handler) => { boot = handler; return () => {}; };
  Api.myCard = async () => ({ sign_pk: 'a', dh_pk: 'b' }); Api.myI2pAddress = async () => 'address';
  Api.myFingerprint = async () => 'fingerprint'; Api.getDisplayName = async () => 'Alice';
  Api.onEvent = async () => () => {};
  Api.getRelayInfo = async () => info; Api.getRelayAddress = async () => ''; Api.listUnreachableContacts = async () => [];
  const store = new Store();
  store.refreshAll = async () => {}; store.ensureNotifyPermission = async () => {}; store.startWatchdog = () => {};
  const restore = () => { store.endBoot(); Object.assign(Api, { onBootStatus: original.boot, myCard: original.card, myI2pAddress: original.i2p_address, myFingerprint: original.fingerprint,
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
  const saved = localStorage.getItem('gipny:last-profile');
  localStorage.setItem('gipny:last-profile', 'last-success');
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
    assert.equal(localStorage.getItem('gipny:last-profile'), 'last-success', 'a failed password never remembers a selection');
    store.view.set('profile-select');
    assert.equal(store.authError.get(), null, 'the profile list does not carry another form\'s failure');
  } finally {
    app.destroy(); store.endBoot(); Api.vaultStatus = original.status; Api.vaultUnlock = original.unlock; Api.onBootStatus = original.boot;
    if (saved === null) localStorage.removeItem('gipny:last-profile'); else localStorage.setItem('gipny:last-profile', saved);
  }
});

test('startup selects the last successfully used profile but never unlocks or creates it', async () => {
  const original = { listProfiles: Api.listProfiles, vaultUnlock: Api.vaultUnlock, vaultCreate: Api.vaultCreate };
  const saved = localStorage.getItem('gipny:last-profile');
  let authenticationCalls = 0;
  Api.vaultUnlock = Api.vaultCreate = async () => { authenticationCalls++; };
  try {
    for (const [profiles, remembered, expected] of [
      [[], null, null],
      [['alice'], null, 'alice'],
      [['alice', 'bob'], null, 'alice'],
      [['alice', 'bob'], 'bob', 'bob'],
      [['alice', 'bob'], 'removed', 'alice'],
      [[], 'removed', null],
    ]) {
      if (remembered) localStorage.setItem('gipny:last-profile', remembered);
      else localStorage.removeItem('gipny:last-profile');
      Api.listProfiles = async () => [...profiles];
      const store = new Store();
      const prewarmed = [];
      store.beginPrewarm = (profile) => prewarmed.push(profile);
      await store.bootstrap();
      assert.equal(store.currentProfile.get(), expected);
      assert.equal(store.view.get(), expected ? 'auth-unlock' : 'auth-create');
      assert.equal(store.identity.get(), null);
      assert.equal(store.bootCanEnter.get(), false);
      assert.deepEqual(prewarmed, expected ? [expected] : ['']);
      assert.equal(localStorage.getItem('gipny:last-profile'), remembered && profiles.includes(remembered) ? remembered : null,
        'a fallback selection is not recorded as a successful login');
      store.endBoot();
    }
    assert.equal(authenticationCalls, 0);
  } finally {
    Object.assign(Api, original);
    if (saved === null) localStorage.removeItem('gipny:last-profile'); else localStorage.setItem('gipny:last-profile', saved);
  }
});

test('only successful authentication updates the remembered identifier; lock returns to its empty password form', async () => {
  const saved = localStorage.getItem('gipny:last-profile');
  const original = { listProfiles: Api.listProfiles, vaultLock: Api.vaultLock };
  const h = bootHarness(relayInfo({ state: 'unconfigured', reason: UNCONFIGURED }));
  const store = h.store;
  store.profiles.set(['alice', 'bob']);
  store.beginPrewarm = () => {};
  store.clearHistoryIfAsked = async () => {};
  Api.listProfiles = async () => ['alice', 'bob'];
  Api.vaultLock = async () => {};
  try {
    localStorage.setItem('gipny:last-profile', 'alice');
    store.selectProfileForUnlock('bob');
    assert.equal(localStorage.getItem('gipny:last-profile'), 'alice');
    await store.beginBoot('bob');
    await store.onUnlocked('bob');
    assert.equal(localStorage.getItem('gipny:last-profile'), 'bob');
    await store.lock();
    assert.equal(store.view.get(), 'auth-unlock');
    assert.equal(store.currentProfile.get(), 'bob');
    assert.equal(store.identity.get(), null);
    assert.equal(store.bootCanEnter.get(), false);
    const auth = new ui.AuthUnlock(store);
    try { assert.equal(auth.passI.value, ''); } finally { auth.destroy(); }
  } finally {
    h.restore(); Object.assign(Api, original);
    if (saved === null) localStorage.removeItem('gipny:last-profile'); else localStorage.setItem('gipny:last-profile', saved);
  }
});

test('deleting a remembered profile forgets it without remembering an unverified replacement', async () => {
  const saved = localStorage.getItem('gipny:last-profile');
  const original = { listProfiles: Api.listProfiles, deleteProfile: Api.deleteProfile };
  const store = new Store(); store.profiles.set(['alice', 'bob']); store.beginPrewarm = () => {};
  Api.deleteProfile = async () => {};
  Api.listProfiles = async () => ['alice'];
  try {
    localStorage.setItem('gipny:last-profile', 'bob');
    store.selectProfileForUnlock('bob');
    await store.deleteProfile('bob');
    assert.equal(localStorage.getItem('gipny:last-profile'), null);
    assert.equal(store.currentProfile.get(), 'alice');
    assert.equal(store.view.get(), 'auth-unlock');
    Api.listProfiles = async () => [];
    await store.deleteProfile('alice');
    assert.equal(store.currentProfile.get(), null);
    assert.equal(store.view.get(), 'auth-create');
  } finally {
    store.endBoot(); Object.assign(Api, original);
    if (saved === null) localStorage.removeItem('gipny:last-profile'); else localStorage.setItem('gipny:last-profile', saved);
  }
});

test('the bottom profile selector link counts OTHER profiles, including zero', async () => {
  const original = Api.vaultStatus;
  Api.vaultStatus = async () => ({ exists: true, unlocked: false });
  const store = new Store(); store.profiles.set(['alice', 'bob', 'charlie']); store.currentProfile.set('bob');
  store.view.set('auth-unlock');
  let opened = false;
  store.cancelToProfileSelect = async () => { opened = true; store.view.set('profile-select'); };
  const auth = new ui.AuthUnlock(store); document.body.append(auth.el);
  try {
    const link = auth.el.querySelector('.auth-other-profiles');
    assert.ok(link);
    assert.equal(link.textContent, t('auth.other_profiles', { p0: 2 }));
    assert.equal(auth.passI.value, '');
    assert.equal(store.identity.get(), null);
    store.profiles.set(['bob']);
    assert.equal(link.textContent, t('auth.other_profiles', { p0: 0 }));
    link.click();
    assert.equal(opened, true);
    assert.equal(store.view.get(), 'profile-select');
  } finally { auth.destroy(); Api.vaultStatus = original; }
});

test('switching a selected locked profile discards the previous password form', async () => {
  const original = Api.vaultStatus;
  Api.vaultStatus = async () => ({ exists: true, unlocked: false });
  const store = new Store(); store.profiles.set(['alice', 'bob']); store.beginPrewarm = () => {};
  store.currentProfile.set('alice'); store.view.set('auth-unlock');
  const app = new ui.App(store); document.body.append(app.el);
  try {
    const previous = app.current;
    previous.passI.value = 'must not cross profiles';
    store.selectProfileForUnlock('bob');
    assert.notEqual(app.current, previous);
    assert.equal(previous.passI.value, '');
    assert.equal(app.current.passI.value, '');
    assert.equal(app.current.el.querySelector('.auth-title').textContent, 'bob');
    assert.equal(store.identity.get(), null);
  } finally { app.destroy(); store.endBoot(); Api.vaultStatus = original; }
});

test('a destructive unlock refreshes profiles and removes a wiped remembered identifier', async () => {
  const saved = localStorage.getItem('gipny:last-profile');
  const original = { vaultStatus: Api.vaultStatus, vaultUnlock: Api.vaultUnlock, listProfiles: Api.listProfiles, onBootStatus: Api.onBootStatus };
  Api.vaultStatus = async () => ({ exists: true, unlocked: false });
  Api.vaultUnlock = async () => { throw new Error('wiped'); };
  Api.listProfiles = async () => [];
  Api.onBootStatus = async () => () => {};
  const store = new Store(); store.profiles.set(['alice']); store.currentProfile.set('alice'); store.view.set('auth-unlock');
  const auth = new ui.AuthUnlock(store); document.body.append(auth.el);
  try {
    localStorage.setItem('gipny:last-profile', 'alice');
    auth.passI.value = 'duress password';
    await auth.unlock('password');
    assert.equal(store.view.get(), 'auth-create');
    assert.equal(store.currentProfile.get(), null);
    assert.equal(localStorage.getItem('gipny:last-profile'), null);
    assert.equal(store.toast.get().text, t('auth.profile_wiped'));
  } finally {
    auth.destroy(); store.endBoot(); Object.assign(Api, original);
    if (saved === null) localStorage.removeItem('gipny:last-profile'); else localStorage.setItem('gipny:last-profile', saved);
  }
});

test('registration progresses from one name to password confirmation to truthful browser limitations', async () => {
  const original = Api.vaultCreate;
  const calls = [];
  Api.vaultCreate = async (...args) => { calls.push(args); };
  const store = new Store(); store.view.set('auth-create');
  store.beginBoot = async (profile) => { store.currentProfile.set(profile); store.view.set('auth-booting'); };
  store.isBooting = (profile) => store.currentProfile.get() === profile && store.view.get() === 'auth-booting';
  store.onUnlocked = async () => {};
  const auth = new ui.AuthCreate(store); document.body.append(auth.el);
  const visibleSteps = () => Array.from(auth.el.querySelectorAll('.auth-create-step')).filter((el) => !el.hidden);
  try {
    assert.deepEqual(visibleSteps().map((el) => el.dataset.step), ['1']);
    assert.equal(visibleSteps()[0].querySelectorAll('input').length, 1);
    assert.equal(auth.nextBtn.textContent, t('auth.next'));
    await auth.advance();
    assert.equal(auth.step, 1, 'name is required');
    auth.displayI.value = 'Alice';
    await auth.advance();
    assert.equal(auth.step, 2);
    assert.equal(visibleSteps()[0].querySelectorAll('input[type=password]').length, 2);
    assert.equal(auth.el.querySelector('.auth-local-data').textContent, t('auth.local_data_hint'));
    auth.passI.value = auth.confirmI.value = 'short';
    await auth.advance();
    assert.equal(auth.step, 2);
    auth.passI.value = 'password with spaces ';
    auth.confirmI.value = 'password with spaces';
    await auth.advance();
    assert.equal(auth.step, 2, 'confirmation compares exact password bytes');
    auth.confirmI.value = auth.passI.value;
    await auth.advance();
    assert.equal(auth.step, 3);
    assert.equal(calls.length, 0, 'Next never creates a profile');
    assert.equal(auth.nextBtn.textContent, t('auth.create_btn'));
    assert.equal(visibleSteps()[0].querySelector('input'), null, 'browser does not offer fake duress or wipe controls');
    assert.equal(auth.el.querySelector('.auth-duress-explanation').textContent, t('auth.duress_unavailable_browser'));
    // Even an injected unsupported draft must not reach the browser backend.
    auth.duressI.value = 'unsupported secret'; auth.attemptsI.value = '10'; auth.wipeC.checked = true;
    await auth.advance();
    assert.deepEqual(calls, [['alice', 'password with spaces ', 'Alice', null, false, 0]]);
  } finally { auth.destroy(); store.endBoot(); Api.vaultCreate = original; }
});

test('registration draft restores the active step in memory and clears the destroyed form', async () => {
  const before = Array.from({ length: localStorage.length }, (_, i) => [localStorage.key(i), localStorage.getItem(localStorage.key(i))]);
  const store = new Store(); store.view.set('auth-create');
  const first = new ui.AuthCreate(store); document.body.append(first.el);
  let second;
  try {
    first.displayI.value = 'Анна';
    await first.advance();
    first.passI.value = first.confirmI.value = 'memory-only password';
    await first.advance();
    first.advanced.open = true;
    const draft = first.captureDraft();
    first.destroy();
    assert.equal(first.passI.value, ''); assert.equal(first.confirmI.value, ''); assert.equal(first.duressI.value, '');
    ui.setLang('ru');
    second = new ui.AuthCreate(store); document.body.append(second.el);
    second.restoreDraft(draft);
    assert.equal(second.step, 3);
    assert.equal(second.passI.value, 'memory-only password');
    assert.equal(second.confirmI.value, 'memory-only password');
    assert.equal(second.advanced.open, true);
    assert.equal(second.nextBtn.textContent, t('auth.create_btn'));
    assert.match(second.nameHint.textContent, /anna/);
    assert.equal(second.el.querySelector('[data-step="3"]').hidden, false);
    second.backBtn.click();
    assert.equal(second.step, 2);
    assert.equal(second.passI.value, 'memory-only password');
    second.confirmI.value = 'changed';
    await second.advance();
    assert.equal(second.step, 2, 'restoring never bypasses validation');
    assert.equal(second.err.textContent, t('auth.passphrases_do_not_match'));
    // Language itself may persist; neither the draft nor its secrets may do so.
    for (let i = 0; i < localStorage.length; i++) assert.ok(!localStorage.getItem(localStorage.key(i)).includes('memory-only password'));
    assert.equal(JSON.stringify(store).includes('memory-only password'), false);
    assert.ok(before.every(([key, value]) => key === 'gipny.lang' || localStorage.getItem(key) === value));
  } finally { first.destroy(); second?.destroy(); ui.setLang('en'); }
});

test('native registration keeps duress optional and preserves exact passwords, wipe mode and attempt limits', async () => {
  const original = Api.vaultCreate;
  const calls = [];
  // Exercise the same shared view in native/preview capability mode; no native
  // process is started and every backend command used here is stubbed.
  ui.configureBrowserBackend(null);
  Api.vaultCreate = async (...args) => { calls.push(args); };
  const store = new Store(); store.view.set('auth-create');
  store.beginBoot = async (profile) => { store.currentProfile.set(profile); store.view.set('auth-booting'); };
  store.isBooting = (profile) => store.currentProfile.get() === profile && store.view.get() === 'auth-booting';
  store.onUnlocked = async () => {};
  const auth = new ui.AuthCreate(store); document.body.append(auth.el);
  let optional;
  try {
    auth.displayI.value = 'Alice';
    await auth.advance();
    auth.passI.value = auth.confirmI.value = 'primary password';
    await auth.advance();
    assert.equal(auth.step, 3);
    assert.equal(auth.el.querySelector('[data-step="3"] input[type=password]'), auth.duressI);
    assert.equal(auth.duressI.required, false);
    assert.equal(auth.el.querySelector('.auth-duress-explanation').textContent, t('auth.duress_setup_hint'));
    assert.equal(auth.advanced.open, false, 'advanced options start collapsed');
    assert.equal(auth.attemptsI.value, '10');
    assert.equal(auth.wipeC.checked, true);
    auth.duressI.value = auth.passI.value;
    await auth.advance();
    assert.equal(calls.length, 0);
    assert.equal(auth.err.textContent, t('auth.duress_must_differ_from_primary'));
    auth.duressI.value = 'primary password ';
    auth.attemptsI.value = '1.5';
    await auth.advance();
    assert.equal(calls.length, 0);
    assert.equal(auth.advanced.open, true);
    assert.equal(auth.err.textContent, t('auth.invalid_max_attempts'));
    auth.attemptsI.value = '0'; auth.wipeC.checked = false;
    await auth.advance();
    assert.deepEqual(calls, [['alice', 'primary password', 'Alice', 'primary password ', false, 0]]);
    auth.destroy();
    store.view.set('auth-create');
    optional = new ui.AuthCreate(store); document.body.append(optional.el);
    optional.displayI.value = 'Bob';
    await optional.advance();
    optional.passI.value = optional.confirmI.value = 'another password';
    await optional.advance();
    await optional.advance();
    assert.deepEqual(calls[1], ['bob', 'another password', 'Bob', null, true, 10], 'blank duress is optional and existing native defaults remain intact');
  } finally {
    auth.destroy(); optional?.destroy(); store.endBoot(); Api.vaultCreate = original; ui.configureBrowserBackend(quietCore);
  }
});

test('browser Settings exports a named encrypted backup and allows retry after an error', async ctx => {
  const store = new Store();
  const app = new ui.App(store);
  ctx.after(() => app.destroy());
  document.body.append(app.el);
  const close = app.openModal(dismiss => new ui.SettingsModal(store, app, dismiss).el);
  ctx.after(close);
  const button = [...document.querySelectorAll('.modal button')].find(el => el.textContent === t('settings.export_backup'));
  assert.ok(button);
  const { parentElement: section } = button;
  const pass = section.querySelector('input[type=password]'), error = section.querySelector('[role=alert]');
  const calls = [];
  let finish;
  ctx.mock.method(Api, 'exportIdentity', async (...args) => {
    calls.push(args);
    if (calls.length === 1) throw new Error('export interrupted');
    return new Promise(resolve => { finish = resolve; });
  });
  pass.value = 'short';
  button.click();
  await tick();
  assert.equal(error.textContent, t('settings.too_short'));
  assert.equal(calls.length, 0);
  pass.value = 'archive password';
  button.click();
  await tick();
  assert.match(error.textContent, /export interrupted/);
  assert.equal(pass.value, 'archive password');
  button.click(); button.click();
  assert.equal(button.disabled, true);
  assert.equal(calls.length, 2);
  assert.equal(calls[1][0], pass.value);
  assert.match(calls[1][1], /^gipny-browser-backup-\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}\.bin$/);
  finish();
  await tick();
  assert.equal(error.textContent, '');
  assert.equal(pass.value, '');
  assert.equal(button.disabled, false);
  assert.equal(store.toast.get()?.text, t('settings.backup_exported'));
});

test('a fresh browser can select a backup, retry its password and release its file on close', async ctx => {
  const store = new Store();
  store.beginPrewarm = () => {};
  let restored = false;
  ctx.mock.method(Api, 'listProfiles', async () => restored ? ['restored'] : []);
  ctx.mock.method(Api, 'vaultStatus', async () => ({ exists: true, unlocked: false }));
  const app = new ui.App(store);
  ctx.after(() => app.destroy());
  document.body.append(app.el);
  await store.bootstrap();
  const findButton = (root, key) => [...root.querySelectorAll('button')].find(el => el.textContent === t(key));
  const select = findButton(app.el, 'profile.select_title');
  assert.ok(select && !select.classList.contains('hidden'));
  select.click();
  const choose = findButton(app.el, 'profile.import_backup');
  const file = new File(['archive'], 'my-backup.bin');
  let picker;
  ctx.mock.method(HTMLInputElement.prototype, 'click', function () { picker = this; });
  const pick = () => {
    choose.click();
    assert.equal(picker.type, 'file');
    assert.equal(picker.accept, '.bin');
    Object.defineProperty(picker, 'files', { value: [file] });
    picker.dispatchEvent(new win.Event('change'));
    return document.querySelector('.modal');
  };
  choose.click();
  picker.dispatchEvent(new win.Event('cancel'));
  assert.equal(document.querySelector('.modal'), null);
  let token;
  ctx.mock.method(Api, 'importIdentityToProfile', async (name, vaultPass, backupPath, backupPass) => {
    assert.equal(name, 'restored');
    assert.equal(vaultPass, 'new vault password');
    assert.equal(ui.selectedBrowserFile(backupPath), file);
    token = backupPath;
    if (backupPass !== 'backup password') throw new Error('incorrect backup password');
    restored = true;
  });
  const submitWrongPassword = async modal => {
    assert.ok(modal.textContent.includes(file.name));
    assert.ok(!modal.textContent.includes('browser-file:'));
    const [name, vaultPass, archivePass] = modal.querySelectorAll('input');
    name.value = 'restored'; vaultPass.value = 'new vault password'; archivePass.value = 'incorrect';
    findButton(modal, 'profile.import').click();
    await tick();
    assert.match(modal.querySelector('.err').textContent, /incorrect backup password/);
    assert.equal(ui.selectedBrowserFile(token), file);
    return archivePass;
  };
  const cancelled = pick();
  await submitWrongPassword(cancelled);
  findButton(cancelled, 'common.cancel').click();
  assert.throws(() => ui.selectedBrowserFile(token), /select the file again/);
  const modal = pick();
  const archivePass = await submitWrongPassword(modal);
  archivePass.value = 'backup password';
  findButton(modal, 'profile.import').click();
  await tick();
  assert.throws(() => ui.selectedBrowserFile(token), /select the file again/);
  assert.deepEqual(store.profiles.get(), ['restored']);
  assert.equal(store.view.get(), 'auth-unlock');
  assert.equal(store.currentProfile.get(), 'restored');
});

// Bypass only the native file picker: the modal, App lifecycle, Store and
// delayed backend replies below exercise the real shared import interaction.
for (const [action, duringList] of [
  ['cancel', false], ['destroy', false], ['language', false],
  ['navigate', false], ['boot', false], ['new-import', false],
  ['cancel', true], ['language', true], ['navigate', true], ['boot', true],
  ['cancel-error', false], ['success', true],
]) test(`delayed profile import: ${action}${duringList ? ' during list refresh' : ''}`, async () => {
  const original = { importIdentityToProfile: Api.importIdentityToProfile, listProfiles: Api.listProfiles,
    vaultStatus: Api.vaultStatus, onBootStatus: Api.onBootStatus };
  const deferred = () => {
    let resolve, reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    return { promise, resolve, reject };
  };
  const imported = deferred(), listed = deferred();
  let importCalls = 0, listCalls = 0;
  Api.importIdentityToProfile = async () => { importCalls++; return imported.promise; };
  Api.listProfiles = async () => { listCalls++; return listed.promise; };
  Api.vaultStatus = async () => ({ exists: true, unlocked: false });
  Api.onBootStatus = async () => () => {};
  const store = new Store(); store.profiles.set(['alice']); store.beginPrewarm = () => {};
  const selected = [];
  const select = store.selectProfileForUnlock.bind(store);
  store.selectProfileForUnlock = (profile) => { selected.push(profile); select(profile); };
  const app = new ui.App(store); document.body.append(app.el);
  try {
    const selector = app.current;
    assert.ok(selector instanceof ui.ProfileSelect);
    selector.showImportModal('/backup.bin');
    const modal = document.querySelector('.modal');
    const inputs = modal.querySelectorAll('input');
    inputs[0].value = 'imported'; inputs[1].value = 'new vault password'; inputs[2].value = 'backup password';
    const error = modal.querySelector('.err');
    const submit = modal.querySelector('.modal-footer .btn:not(.btn-ghost)');
    submit.click(); submit.click();
    assert.equal(importCalls, 1, 'repeated clicks must not launch competing imports');
    if (duringList) {
      imported.resolve(); await tick();
      assert.equal(listCalls, 1);
    }
    if (action === 'cancel' || action === 'cancel-error') app.closeModals();
    else if (action === 'destroy') selector.destroy();
    else if (action === 'language') ui.setLang('ru');
    else if (action === 'navigate' || action === 'boot') {
      store.selectProfileForUnlock('alice');
      app.current.passI.value = 'password in the active form';
      if (action === 'boot') await store.beginBoot('alice');
    } else if (action === 'new-import') {
      selector.showImportModal('/another-backup.bin');
      document.querySelector('.modal input').value = 'new interaction draft';
    }
    const before = { view: store.view.get(), profile: store.currentProfile.get(), current: app.current,
      profiles: store.profiles.get(), toast: store.toast.get(), error: store.authError.get(),
      modal: document.querySelector('.modal'), selections: [...selected], attempt: store.bootAttempt };
    if (action === 'cancel-error') imported.reject(new Error('late import failure'));
    else imported.resolve();
    await tick();
    listed.resolve(['alice', 'imported']);
    await tick();
    if (action === 'success') {
      assert.deepEqual(selected, ['imported'], 'a live import still selects after its own close invalidates the operation');
      assert.equal(store.currentProfile.get(), 'imported');
      assert.equal(store.view.get(), 'auth-unlock');
      assert.equal(app.current.passI.value, '', 'selection is not authentication');
      assert.equal(document.querySelector('.modal'), null);
      assert.deepEqual(store.profiles.get(), ['alice', 'imported']);
    } else {
      assert.deepEqual(selected, before.selections, 'stale completion must not select a profile');
      assert.equal(store.view.get(), before.view);
      assert.equal(store.currentProfile.get(), before.profile);
      assert.equal(app.current, before.current, 'do not replace the active form or startup view');
      assert.equal(store.profiles.get(), before.profiles, 'do not publish a stale list into the current interaction');
      assert.equal(store.toast.get(), before.toast, 'no stale success toast');
      assert.equal(store.authError.get(), before.error);
      assert.equal(store.bootAttempt, before.attempt, 'ongoing authentication must not be interrupted');
      assert.equal(document.querySelector('.modal'), before.modal, 'do not close a newer modal');
      assert.equal(error.textContent, '', 'no stale error writes into the detached form');
      if (action === 'navigate') assert.equal(app.current.passI.value, 'password in the active form');
      if (action === 'new-import') assert.equal(before.modal.querySelector('input').value, 'new interaction draft');
      assert.equal(listCalls, duringList ? 1 : 0, 'cancelled imports do not start a follow-up request');
    }
    assert.equal(inputs[1].value, '', 'closing/destroying the interaction clears its password fields');
    assert.equal(inputs[2].value, '');
  } finally {
    app.destroy(); store.endBoot(); Object.assign(Api, original); ui.setLang('en');
  }
});

test('display lock removes message views, retains the session and requires fresh authentication', async () => {
  const store = new Store();
  store.currentProfile.set('alice');
  store.view.set('main');
  const app = new ui.App(store);
  document.body.appendChild(app.el);
  let refreshes = 0;
  store.refreshAll = async () => { refreshes++; };
  const originalLock = Api.vaultLock;
  let vaultLocks = 0;
  Api.vaultLock = async () => { vaultLocks++; };
  try {
    store.coverScreen();
    assert.equal(store.screenLocked.get(), true);
    assert.ok(app.el.querySelector('#screen-pass'));
    assert.equal(app.el.querySelector('.sidebar'), null);
    assert.equal(store.currentProfile.get(), 'alice');
    assert.equal(vaultLocks, 0);
    await assert.rejects(store.revealScreen(async () => { throw new Error('wrong password'); }), /wrong password/);
    assert.equal(store.screenLocked.get(), true);

    let accept;
    const pending = store.revealScreen(() => new Promise(resolve => { accept = resolve; }));
    store.coverScreen(true);
    accept();
    await pending;
    assert.equal(store.screenLocked.get(), true, 'a fresh screen-off invalidates the pending password check');

    await store.revealScreen(async profile => { assert.equal(profile, 'alice'); });
    assert.equal(store.screenLocked.get(), false);
    assert.equal(app.el.querySelector('#screen-pass'), null);
    assert.equal(refreshes, 1);
    assert.equal(vaultLocks, 0);
  } finally {
    Api.vaultLock = originalLock;
    app.destroy();
  }
});

test('provider sheet may suspend the view but an intervening hardware screen-off still wins', async () => {
  const store = new Store();
  store.currentProfile.set('alice');
  store.refreshAll = async () => {};
  store.view.set('main');
  store.coverScreen();
  await store.revealScreen(async () => { store.coverScreen(true); }, true);
  assert.equal(store.screenLocked.get(), true);
  await store.revealScreen(async () => { store.coverScreen(); }, true);
  assert.equal(store.screenLocked.get(), false);
});

test('returning from display lock reloads messages and pins changed in the background', async () => {
  const store = new Store(), target = { kind: 'contact', id: 12 }, key = 'contact:12';
  const former = { id: 1, body: 'expired while locked' }, current = { id: 2, body: 'arrived while locked' };
  const original = { listMessages: Api.listMessages, listPinnedContact: Api.listPinnedContact };
  store.currentProfile.set('alice');
  store.refreshAll = async () => {};
  store.view.set('main');
  store.selectedChat.set(target);
  store.messages.set(new Map([[key, [former]]]));
  store.pinned.set(new Map([[key, [former]]]));
  Api.listMessages = async () => [current];
  Api.listPinnedContact = async () => [];
  try {
    store.coverScreen();
    assert.equal(store.messages.get().size, 0);
    assert.equal(store.pinned.get().size, 0);
    await store.revealScreen(async () => {});
    assert.deepEqual(store.messages.get().get(key), [current]);
    assert.deepEqual(store.pinned.get().get(key), []);
  } finally { Object.assign(Api, original); }
});

test('explicit sign out and close waits for the vault to stop', async () => {
  const store = new Store();
  store.currentProfile.set('alice');
  store.view.set('main');
  let finish;
  store.lock = async () => {
    assert.equal(store.screenLocked.get(), true);
    await new Promise(resolve => { finish = resolve; });
  };
  let done = false;
  const closed = store.lockAndClose().then(() => { done = true; });
  assert.equal(done, false);
  finish();
  await closed;
  assert.equal(done, true);
});

test('the delivery notice remains pending if the view is covered while it is being read', async () => {
  const original = { list: Api.listDeliveryExpiryNotices, ack: Api.acknowledgeDeliveryExpiryNotices };
  const notice = { message_id: 12, contact_id: 1, group_id: null, expired_at: 100 };
  let finish;
  let shown = 0;
  let acked = 0;
  Api.listDeliveryExpiryNotices = () => new Promise(resolve => { finish = () => resolve([notice]); });
  Api.acknowledgeDeliveryExpiryNotices = async (_profile, ids) => { assert.deepEqual(ids, [12]); acked++; };
  const store = new Store();
  store.currentProfile.set('alice');
  store.showToast = () => { shown++; };
  store.refreshAll = async () => {};
  try {
    store.view.set('main');
    store.coverScreen();
    finish();
    await tick();
    assert.equal(acked, 0);
    assert.equal(shown, 0);
    await store.revealScreen(async () => {});
    finish();
    await tick();
    assert.equal(shown, 1);
    assert.equal(acked, 1);
  } finally {
    Api.listDeliveryExpiryNotices = original.list;
    Api.acknowledgeDeliveryExpiryNotices = original.ack;
  }
});
