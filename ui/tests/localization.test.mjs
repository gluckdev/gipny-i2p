import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { build } from 'vite';
import { Window } from 'happy-dom';

// Bundle the real TS modules with the same compiler as production. No router,
// account, WebSocket connection or browser binary is needed by these tests.
const dir = await mkdtemp(join(tmpdir(), 'gipny-i18n-'));
await build({ configFile: false, logLevel: 'silent', build: {
  outDir: dir, emptyOutDir: false, rolldownOptions: { output: { banner: 'const Event = window.Event;' } }, lib: { entry: 'tests/entry.ts', formats: ['es'], fileName: () => 'entry.mjs' },
} });
const win = new Window({ url: 'http://localhost/' });
for (const key of ['window', 'document', 'localStorage', 'navigator', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'File', 'requestAnimationFrame', 'cancelAnimationFrame']) {
  Object.defineProperty(globalThis, key, { value: key === 'window' ? win : win[key], configurable: true });
}
win.__TAURI_INTERNALS__ = {};
const ui = await import(pathToFileURL(join(dir, 'entry.mjs')).href);
const cardFixtures = JSON.parse(await readFile(new URL('../../tests/fixtures/contact-cards-v3.json', import.meta.url), 'utf8'));
const { t, setLang, getLang, translations, App, Store, Api } = ui;

const calls = [];
let logSettings = { available: true, enabled: false, path: '/tmp/example.log' };
ui.mockIPC((cmd, args) => {
  calls.push(cmd);
  if (cmd === 'message_files') return Object.fromEntries((args?.messageIds ?? []).map(id => [id, { attachments: [], progress: [] }]));
  if (cmd === 'log_settings' && logSettings instanceof Error) return Promise.reject(logSettings);
  const responses = {
    get_contact_folders: [], get_avatar_choices: {}, get_chat_prefs: { ttlSecs: null, ttlChosen: true },
    list_attachments: [], files_progress: [], list_group_members: [], list_media_contact: [], list_media_group: [],
    link_stats: { lane: 'normal', our_hops: 3, their_hops: 3, padded: true, route: 'relay', rtt_ms: null }, get_relay_info: null, get_relay_address: '', my_b32: '',
    update_configured: false, list_apk_artifacts: { version: '0.4.18', artifacts: [] },
    current_version: '0.4.18', get_attachment_privacy: true, get_clear_on_lock: false,
    get_agent_mode: null, get_router_settings: { transit: 'balanced', upnp: 'on' },
    log_settings: logSettings,
    my_bundle: { sign_pk: 'a'.repeat(64), dh_pk: 'b'.repeat(64), signed_prekey: 'c'.repeat(64), signed_prekey_sig: 'd'.repeat(128), one_time_prekey: null },
  };
  return responses[cmd] ?? null;
});
const tick = () => new Promise((resolve) => setTimeout(resolve, 10));
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
after(async () => { await win.happyDOM.abort(); await rm(dir, { recursive: true, force: true }); });

test('v3 cards match Rust and Python vectors and reject malformed canonical encodings', () => {
  for (const vector of cardFixtures.valid) {
    const source = vector.input;
    const encoded = ui.encodeCard(source.onion, source.sign_pk, source.dh_pk, source.name, source.relay);
    assert.equal(encoded, vector.encoded, vector.id);
    const decoded = ui.decodeCard(encoded);
    assert.deepEqual(decoded, {
      onion: vector.decoded.onion, signPk: source.sign_pk, dhPk: source.dh_pk,
      ...(source.relay ? { relay: vector.decoded.relay } : {}),
      ...(source.name ? { name: source.name } : {}),
    }, vector.id);
    assert.equal(ui.encodeCard(decoded.onion, decoded.signPk, decoded.dhPk, decoded.name, decoded.relay), encoded);
  }
  for (const vector of cardFixtures.invalid) assert.equal(ui.decodeCard(vector.encoded), null, vector.id);
  const sample = cardFixtures.valid[0].input;
  assert.throws(() => ui.encodeCard(sample.onion, sample.sign_pk, sample.dh_pk, 'я'.repeat(49), sample.relay));
  assert.throws(() => ui.encodeCard(sample.onion, sample.sign_pk, sample.dh_pk, '\ud800', sample.relay));
  assert.throws(() => ui.encodeCard(sample.onion, sample.sign_pk, sample.dh_pk, undefined, ''));
  assert.throws(() => ui.encodeCard(sample.onion, sample.sign_pk.slice(1), sample.dh_pk));
  assert.throws(() => ui.encodeCard(sample.onion, sample.sign_pk, sample.dh_pk + '00'));
  assert.throws(() => ui.encodeCard('a'.repeat(51) + 'b.b32.i2p', sample.sign_pk, sample.dh_pk));
});

test('valid legacy v1/v2 cards still import and exports migrate to v3', () => {
  const input = cardFixtures.valid[0].input;
  const v1 = `gipny:v1:${input.onion}:${input.sign_pk}:${input.dh_pk}:Мурка`;
  const v2 = `gipny:v2:${input.onion}:${input.sign_pk}:${input.dh_pk}:${input.relay}:${encodeURIComponent('Мурка:1')}`;
  assert.equal(ui.decodeCard(v1).name, 'Мурка');
  const decoded = ui.decodeCard(v2);
  assert.equal(decoded.name, 'Мурка:1');
  assert.match(ui.encodeCard(decoded.onion, decoded.signPk, decoded.dhPk, decoded.name, decoded.relay), /^gipny:v3:/);
  assert.equal(ui.decodeCard(v2.replace(encodeURIComponent('Мурка:1'), '%FF')), null);
});

test('card export keeps copying disabled until ready and never copies an encoding error', async () => {
  const previousLang = getLang();
  const previous = { address: Api.getRelayAddress, info: Api.getRelayInfo, qr: Api.qrSvg,
    clipboard: Object.getOwnPropertyDescriptor(navigator, 'clipboard') };
  const source = cardFixtures.valid[0].input;
  const copied = [], qr = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text) => { copied.push(text); } } });
  try {
    Api.qrSvg = async (card) => { qr.push(card); return '<svg></svg>'; };
    for (const lang of ['en', 'ru']) {
      setLang(lang);
      const relay = deferred();
      Api.getRelayAddress = () => relay.promise;
      Api.getRelayInfo = async () => null;
      const store = new Store();
      store.identity.set({ onion: source.onion, card: { sign_pk: source.sign_pk, dh_pk: source.dh_pk }, fingerprint: 'a'.repeat(64) });
      store.displayName.set('я'.repeat(49));
      const modal = new ui.IdentityModal(store, () => {});
      document.body.append(modal.el);
      const copy = [...modal.el.querySelectorAll('button')].find((button) => button.textContent === t('identity.copy_card'));
      assert.equal(copy.disabled, true, 'relay has not become available');
      const before = copied.length;
      copy.click(); await tick();
      assert.equal(copied.length, before);
      relay.resolve(source.relay); await tick();
      assert.equal(copy.disabled, true, 'overlong UTF-8 name cannot produce a card');
      assert.ok(modal.el.textContent.includes(t('card.encode_error')));
      copy.click(); await tick(); assert.equal(copied.length, before);
      const name = modal.el.querySelector('input.input');
      name.value = 'Мурка'; name.dispatchEvent(new win.Event('input'));
      assert.equal(copy.disabled, false);
      copy.click(); await tick();
      assert.equal(copied.length, before + 1);
      assert.equal(ui.decodeCard(copied.at(-1)).name, 'Мурка');
      assert.ok(qr.includes(copied.at(-1)), 'QR and clipboard carry the same valid card');
      modal.el.remove();
    }
  } finally {
    setLang(previousLang);
    Api.getRelayAddress = previous.address; Api.getRelayInfo = previous.info; Api.qrSvg = previous.qr;
    if (previous.clipboard) Object.defineProperty(navigator, 'clipboard', previous.clipboard);
    else delete navigator.clipboard;
  }
});

test('locale choice respects saved preferences and handles unavailable storage', () => {
  assert.equal(ui.detectLang('ru', ['en-US']), 'ru');
  assert.equal(ui.detectLang('en', ['ru-RU']), 'en');
  assert.equal(ui.detectLang(null, ['ru-RU']), 'ru');
  assert.equal(ui.detectLang(null, ['de-DE']), 'en');
  assert.equal(ui.detectLang('invalid', []), 'en');
  assert.equal(ui.detectLang(null, ['en-US', 'ru-RU']), 'ru', 'any preferred Russian locale');
  assert.equal(ui.detectLang(null, ['uk-UA', 'be-BY']), 'en', 'other Cyrillic locales are not Russian');
  assert.equal(ui.detectLang(null, ['en-US'], true), 'ru', 'a Russian keyboard layout');
  assert.equal(ui.detectLang('en', ['ru-RU'], true), 'en', 'an explicit choice wins');
  assert.equal(ui.isRussianLayout('йцукенгшщзхъфывапролджэячсмитьбю'.split('')), true);
  assert.equal(ui.isRussianLayout('йцукенгшщзхїфівапролджєячсмитьбю'.split('')), false);
  const old = win.localStorage.setItem;
  win.localStorage.setItem = () => { throw new Error('storage disabled'); };
  assert.doesNotThrow(() => setLang('ru'));
  assert.equal(getLang(), 'ru');
  assert.equal(document.documentElement.lang, 'ru');
  win.localStorage.setItem = old;
  setLang('en');
  setLang('en');
  assert.equal(localStorage.getItem('gipny.lang'), 'en');
});

test('both dictionaries contain the same nonempty messages and placeholders', () => {
  assert.deepEqual(Object.keys(translations.en).sort(), Object.keys(translations.ru).sort());
  const placeholders = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
  for (const [key, en] of Object.entries(translations.en)) {
    assert.ok(en.trim(), key);
    assert.ok(translations.ru[key].trim(), key);
    assert.deepEqual(placeholders(en), placeholders(translations.ru[key]), key);
    if (key !== 'settings.lang_ru') assert.doesNotMatch(en, /[А-Яа-яЁё]/, key);
  }
});

test('interpolation preserves user data, replacement tokens and braces literally', () => {
  for (const lang of ['en', 'ru']) {
    setLang(lang);
    const name = '$& $` $\' $$ {error} <script>alert(1)</script>';
    const text = t('profile.wiped_toast', { name });
    assert.ok(text.includes(name));
    assert.ok(t('chat.send_failed', { error: '{name}' }).includes('{name}'));
  }
});

test('error presentation translates known native errors without changing filenames', () => {
  const error = 'файл слишком большой: Отчёт.pdf (20 МБ, лимит 16 МБ)';
  setLang('en');
  assert.equal(ui.errorText('unsupported: установка отменена'), 'Installation cancelled');
  assert.equal(ui.logText('(журнал пуст)'), '(log is empty)');
  assert.equal(ui.logText('Строка диагностики'), 'Строка диагностики');
  assert.equal(ui.errorText(error), 'File too large: Отчёт.pdf (20 MB; limit 16 MB)');
  assert.match(ui.errorText('приватность вложений: не удалось разобрать PDF (файл зашифрован) — подсказка'), /PDF \(the file is encrypted\)/);
  assert.equal(ui.errorText('unexpected diagnostic: Отчёт.pdf'), 'unexpected diagnostic: Отчёт.pdf');
  setLang('ru');
  assert.equal(ui.errorText(error), error);
});

test('device access errors have actionable EN/RU text without changing unknown errors or password markers', () => {
  const cases = [
    ['OS credential provider unavailable: hardware Android Keystore operation failed; unlock or provision protected OS credentials before opening GIPNY', 'auth.os_credentials_unavailable'],
    ['OS credential provider unavailable: Linux Secret Service is locked or did not respond; unlock or provision protected OS credentials before opening GIPNY', 'auth.os_credentials_unavailable'],
    ['Android protected credential provider could not initialize', 'auth.os_credentials_unavailable'],
    ['Android credential initialization closed', 'auth.os_credentials_unavailable'],
    ['Android protected credential provider is not ready', 'auth.os_credentials_unavailable'],
    ['device credential missing or inaccessible; raw profile copies cannot be opened; use an explicitly encrypted export/import', 'auth.device_credentials_missing'],
    ['profile belongs to another OS credential provider; use an explicitly encrypted export/import', 'auth.profile_other_device'],
    ['legacy unbound profile is unsupported; data was not deleted', 'auth.profile_legacy_unsupported'],
    ['legacy plaintext headless database is unsupported; data was not deleted', 'auth.profile_legacy_unsupported'],
    ['invalid device-binding metadata', 'auth.profile_binding_invalid'],
    ['device credential does not authenticate this profile; data and password attempt counter were not changed', 'auth.profile_binding_invalid'],
  ];
  for (const lang of ['en', 'ru']) {
    setLang(lang);
    for (const [marker, key] of cases) {
      assert.equal(ui.errorText(marker), t(key), `${lang}: ${marker}`);
      assert.equal(ui.errorText(new Error(marker)), t(key), `${lang}: Error wrapper`);
    }
    const unavailable = ui.errorText(cases[0][0]);
    assert.match(unavailable, lang === 'en' ? /protected key store/ : /хранилищу ключей/);
    assert.doesNotMatch(unavailable, /hardware|JNI|Secret Service/);
    assert.equal(ui.errorText('invalid passphrase'), 'invalid passphrase');
    assert.equal(ui.errorText('unexpected diagnostic: vault.bin'), 'unexpected diagnostic: vault.bin');
    assert.equal(ui.errorText('OS credential provider unavailable: unexpected diagnostic'), 'OS credential provider unavailable: unexpected diagnostic');
  }
});

test('language is available before login and preserves values while changing forms', () => {
  const store = new Store();
  store.view.set('auth-create');
  setLang('ru');
  const app = new App(store); document.body.append(app.el);
  const inputs = [...app.el.querySelectorAll('input')];
  inputs[0].value = 'alice'; inputs[2].value = 'private-passphrase';
  const picker = app.el.querySelector('.auth-language select');
  picker.value = 'en'; picker.dispatchEvent(new win.Event('change'));
  assert.match(app.el.textContent, /Create profile/);
  const current = [...app.el.querySelectorAll('input')];
  assert.equal(current[0].value, 'alice'); assert.equal(current[2].value, 'private-passphrase');
  assert.ok(!JSON.stringify(localStorage).includes('private-passphrase'));
  store.view.set('auth-booting');
  assert.match(app.el.textContent, /SYSTEM INITIALIZATION/);
  setLang('ru'); assert.match(app.el.textContent, /ЗАПУСК СИСТЕМЫ/);
  app.destroy();
});

test('unlock requires the advertised TOTP code for both password and biometric requests', async () => {
  const original = { status: Api.vaultStatus, password: Api.vaultUnlock, biometric: Api.vaultUnlockBiometric, boot: Api.onBootStatus };
  const requests = [];
  Api.vaultStatus = async () => ({ exists: true, unlocked: false, totp_enabled: true, biometric_available: true });
  Api.vaultUnlock = async (...args) => { requests.push(['password', ...args]); return null; };
  Api.vaultUnlockBiometric = async (...args) => { requests.push(['biometric', ...args]); return null; };
  Api.onBootStatus = async () => () => {};
  try {
    for (const lang of ['en', 'ru']) for (const method of ['password', 'biometric']) {
      setLang(lang);
      const store = new Store(); store.currentProfile.set('alice'); store.view.set('auth-unlock');
      let opened = 0; store.onUnlocked = async () => { opened++; };
      const auth = new ui.AuthUnlock(store); document.body.append(auth.el);
      try {
        await tick();
        const password = auth.el.querySelector('input[type=password]');
        const code = auth.el.querySelector('input[autocomplete=one-time-code]');
        assert.equal(code.closest('.field').classList.contains('hidden'), false);
        assert.equal(code.inputMode, 'numeric');
        assert.equal(code.required, true);
        assert.ok(auth.el.textContent.includes(t('auth.biometric_unlock')));
        password.value = 'secret-password';
        for (const value of ['', '12345', '12a456', '1234567']) {
          const before = requests.length;
          code.value = value; await auth.unlock(method);
          assert.equal(requests.length, before, `${lang}/${method}: ${value} must not reach the backend`);
          assert.equal(store.view.get(), 'auth-unlock');
          assert.equal(auth.el.querySelector('.err').textContent, t('auth.totp_required'));
        }
        code.value = '012345'; await auth.unlock(method);
        assert.deepEqual(requests.at(-1), method === 'password'
          ? ['password', 'alice', 'secret-password', '012345'] : ['biometric', 'alice', '012345']);
        assert.equal(opened, 1);
      } finally { auth.destroy(); store.endBoot(); }
    }
  } finally { Api.vaultStatus = original.status; Api.vaultUnlock = original.password; Api.vaultUnlockBiometric = original.biometric; Api.onBootStatus = original.boot; }
});

test('unlock options are optional, failed capability checks retry, and competing unlock methods share one request', async () => {
  const original = { status: Api.vaultStatus, password: Api.vaultUnlock, biometric: Api.vaultUnlockBiometric, boot: Api.onBootStatus };
  const store = new Store(); store.currentProfile.set('alice'); store.view.set('auth-unlock');
  store.onUnlocked = async () => {};
  Api.onBootStatus = async () => () => {};
  let checks = 0, passwords = 0, biometrics = 0;
  Api.vaultStatus = async () => { if (++checks === 1) throw new Error('unavailable'); return { exists: true, unlocked: false }; };
  const pending = deferred();
  Api.vaultUnlock = async () => { passwords++; return pending.promise; };
  Api.vaultUnlockBiometric = async () => { biometrics++; return null; };
  const auth = new ui.AuthUnlock(store); document.body.append(auth.el);
  try {
    await tick();
    assert.equal(auth.el.querySelector('.err').textContent, t('auth.security_status_failed'));
    assert.equal(auth.el.querySelector('#unlock-totp').closest('.field').classList.contains('hidden'), true);
    assert.equal(auth.biometricBtn.classList.contains('hidden'), true);
    auth.el.querySelector('input[type=password]').value = 'secret-password';
    const opening = auth.unlock('password'); await tick();
    assert.equal(checks, 2, 'retry the failed capability request before unlocking');
    assert.equal(passwords, 1);
    await auth.unlock('biometric');
    await auth.unlock('password');
    assert.equal(biometrics, 0); assert.equal(passwords, 1);
    assert.equal(auth.openBtn.disabled, true);
    pending.resolve(null); await opening;
    auth.destroy();
    assert.equal(auth.passI.value, ''); assert.equal(auth.totpI.value, '');
  } finally { auth.destroy(); store.endBoot(); Api.vaultStatus = original.status; Api.vaultUnlock = original.password; Api.vaultUnlockBiometric = original.biometric; Api.onBootStatus = original.boot; }
});

test('resumed startup hides step rows and waits for profile data before reporting ready', async () => {
  const original = { boot: Api.onBootStatus, card: Api.myCard, onion: Api.myOnion, fingerprint: Api.myFingerprint, name: Api.getDisplayName, events: Api.onEvent,
    relayInfo: Api.getRelayInfo, relayAddress: Api.getRelayAddress,
    hidden: Object.getOwnPropertyDescriptor(document, 'hidden'), focus: document.hasFocus };
  const views = [], stores = [];
  let boot;
  Api.onBootStatus = async (handler) => { boot = handler; return () => {}; };
  Api.myCard = async () => ({ sign_pk: 'a', dh_pk: 'b' }); Api.myOnion = async () => 'address';
  Api.myFingerprint = async () => 'fingerprint'; Api.getDisplayName = async () => 'Alice';
  Api.onEvent = async () => () => {};
  let hidden = false;
  Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden }); document.hasFocus = () => !hidden;
  try {
    for (const fresh of [false, true]) {
      const store = new Store(); stores.push(store);
      const loaded = deferred(); store.refreshAll = () => loaded.promise;
      const relay = deferred(); Api.getRelayInfo = () => relay.promise; Api.getRelayAddress = async () => '';
      store.ensureNotifyPermission = async () => {}; store.startWatchdog = () => {};
      await store.beginBoot('alice', fresh);
      const view = new ui.AuthBooting(store); views.push(view); document.body.append(view.el);
      boot({ stage: 'vault', state: 'done', detail: 'opened' });
      hidden = true; document.dispatchEvent(new win.Event('visibilitychange'));
      hidden = false; document.dispatchEvent(new win.Event('visibilitychange'));
      assert.equal(store.bootResuming.get(), !fresh);
      assert.equal(view.el.querySelector('.boot-details').classList.contains('hidden'), !fresh);
      boot({ stage: 'core', state: 'done', detail: 'core running' });
      boot({ stage: 'relay', state: 'done', detail: 'relay ready' });
      store.handleEvent('RelayConnected'); store.enterMain();
      assert.equal(store.view.get(), 'auth-booting', 'network readiness cannot bypass profile loading');
      assert.equal(store.bootCanEnter.get(), false);
      const opening = store.onUnlocked('alice'); await tick();
      assert.equal(store.view.get(), 'auth-booting');
      const before = store.bootSteps.get();
      await store.beginBoot('alice', fresh);
      assert.equal(store.bootSteps.get(), before, 'resuming an in-flight boot never resets progress');
      loaded.resolve(); await tick();
      assert.equal(store.view.get(), 'auth-booting', 'relay status is part of loading the profile');
      assert.equal(store.bootCanEnter.get(), false);
      relay.resolve(null); await opening;
      assert.equal(store.view.get(), 'main');
      assert.equal(store.displayName.get(), 'Alice');
      if (!fresh) assert.equal(view.el.querySelector('.boot-status').textContent, t('boot.ready'));
      view.destroy(); store.endBoot();
    }
  } finally {
    for (const view of views) view.destroy(); for (const store of stores) store.endBoot();
    Api.onBootStatus = original.boot; Api.myCard = original.card; Api.myOnion = original.onion; Api.myFingerprint = original.fingerprint; Api.getDisplayName = original.name; Api.onEvent = original.events;
    Api.getRelayInfo = original.relayInfo; Api.getRelayAddress = original.relayAddress;
    if (original.hidden) Object.defineProperty(document, 'hidden', original.hidden); else delete document.hidden;
    document.hasFocus = original.focus;
  }
});

test('a cancelled boot ignores and releases a late progress subscription', async () => {
  const original = Api.onBootStatus, subscription = deferred();
  let handler, released = 0;
  Api.onBootStatus = (next) => { handler = next; return subscription.promise; };
  const store = new Store();
  try {
    const opening = store.beginBoot('alice');
    store.endBoot(); store.view.set('profile-select');
    subscription.resolve(() => { released++; }); await opening;
    handler({ stage: 'relay', state: 'done', detail: 'stale' });
    assert.equal(released, 1);
    assert.ok(store.bootSteps.get().every(step => step.state === 'idle'));
    assert.equal(store.view.get(), 'profile-select');
  } finally { store.endBoot(); Api.onBootStatus = original; }
});

test('a cancelled profile refresh cannot restore contact or unread data', async () => {
  const original = { contacts: Api.listContacts, groups: Api.listGroups, muted: Api.listMuted, agent: Api.getAgentMode };
  const contacts = deferred(), store = new Store();
  Api.listContacts = () => contacts.promise; Api.listGroups = async () => [];
  Api.listMuted = async () => []; Api.getAgentMode = async () => null;
  try {
    const refreshing = store.refreshAll();
    store.endBoot();
    contacts.resolve([{ id: 1, name: 'private contact', request: 'none' }]);
    await refreshing;
    assert.deepEqual(store.contacts.get(), []);
    assert.equal(store.unread.get().size, 0);
  } finally { Api.listContacts = original.contacts; Api.listGroups = original.groups; Api.listMuted = original.muted; Api.getAgentMode = original.agent; }
});

test('a rejected TOTP code never loads the profile and reports a localized retry', async () => {
  const original = { status: Api.vaultStatus, password: Api.vaultUnlock, biometric: Api.vaultUnlockBiometric, boot: Api.onBootStatus };
  Api.vaultStatus = async () => ({ exists: true, unlocked: false, totp_enabled: true, biometric_available: true });
  Api.vaultUnlock = Api.vaultUnlockBiometric = async () => { throw new Error('invalid totp code'); };
  Api.onBootStatus = async () => () => {};
  try {
    for (const lang of ['en', 'ru']) for (const method of ['password', 'biometric']) {
      setLang(lang);
      const store = new Store(); store.currentProfile.set('alice'); store.view.set('auth-unlock');
      let opened = false; store.onUnlocked = async () => { opened = true; };
      const auth = new ui.AuthUnlock(store);
      try {
        await tick(); auth.passI.value = 'secret-password'; auth.totpI.value = '123456';
        await auth.unlock(method);
        assert.equal(opened, false); assert.equal(store.view.get(), 'auth-unlock');
        assert.equal(store.toast.get().text, t('auth.totp_invalid'));
      } finally { auth.destroy(); store.endBoot(); }
    }
  } finally { Api.vaultStatus = original.status; Api.vaultUnlock = original.password; Api.vaultUnlockBiometric = original.biometric; Api.onBootStatus = original.boot; }
});

function chatStore() {
  const store = new Store();
  store.currentProfile.set('alice');
  store.contacts.set([{ id: 1, name: 'Alice <script>', sign_pk: 'a'.repeat(64), dh_pk: 'b'.repeat(64), onion: 'example.b32.i2p', trust: 1, request: 'none', created_at: Date.now(), last_seen: null, last_message_at: null, agent_granted: true }]);
  store.groups.set([{ id: 'team', name: 'Team', created_at: Date.now() }]);
  store.groupMembers.set(new Map([['team', []]]));
  store.selectedChat.set({ kind: 'contact', id: 1 });
  store.messages.set(new Map([['contact:1', [{ id: 7, body: 'Original', outgoing: true, sent_at: Date.now(), console: null }]]]));
  store.view.set('main');
  return store;
}

test('history prompts persist explicit choices per direct chat and group in both languages', async () => {
  const original = { get: Api.getChatPrefs, update: Api.updateChatPrefs };
  try {
    for (const lang of ['en', 'ru']) for (const target of [{ kind: 'contact', id: 1 }, { kind: 'group', id: 'team' }]) for (const save of [false, true]) {
      setLang(lang);
      const key = `${target.kind}:${target.id}`;
      let prefs = { ttlSecs: null, ttlChosen: true, clearOnLock: true, saveHistory: { 'contact:2': true } };
      Api.getChatPrefs = async () => prefs;
      Api.updateChatPrefs = async (patch) => { prefs = { ...prefs, ...patch, saveHistory: { ...prefs.saveHistory, ...patch.saveHistory } }; };
      const store = chatStore(); store.selectedChat.set(target);
      const app = new App(store); document.body.append(app.el);
      try {
        await tick();
        const prompt = app.el.querySelector('.history-prompt');
        assert.equal(prompt.classList.contains('hidden'), false);
        assert.ok(prompt.textContent.includes(t('chat.save_history_prompt')));
        const button = [...prompt.querySelectorAll('button')].find(button => button.textContent === t(save ? 'chat.history_save' : 'chat.history_dont_save'));
        button.click(); await tick();
        assert.equal(prefs.saveHistory[key], save);
        assert.equal(prefs.saveHistory['contact:2'], true);
        assert.equal(prompt.classList.contains('hidden'), true);
        setLang(lang === 'en' ? 'ru' : 'en'); await tick();
        assert.equal(app.el.querySelector('.history-prompt').classList.contains('hidden'), true, 'both explicit answers survive a fresh chat render');
        store.selectedChat.set({ kind: 'contact', id: 3 }); await tick();
        assert.equal(app.el.querySelector('.history-prompt').classList.contains('hidden'), false, 'another chat still needs its own answer');
      } finally { app.destroy(); }
    }
  } finally { Api.getChatPrefs = original.get; Api.updateChatPrefs = original.update; }
});

test('history prompts stay hidden when clearing is disabled and retry failed preference writes', async () => {
  const original = { get: Api.getChatPrefs, update: Api.updateChatPrefs };
  const store = chatStore(); let app;
  try {
    Api.getChatPrefs = async () => ({ ttlSecs: null, ttlChosen: true, clearOnLock: false });
    app = new App(store); document.body.append(app.el); await tick();
    assert.equal(app.el.querySelector('.history-prompt').classList.contains('hidden'), true);
    app.destroy();
    Api.getChatPrefs = async () => ({ ttlSecs: null, ttlChosen: true, clearOnLock: true });
    Api.updateChatPrefs = async () => { throw new Error('write failed'); };
    app = new App(store); document.body.append(app.el); await tick();
    const prompt = app.el.querySelector('.history-prompt'), button = prompt.querySelector('button');
    button.click(); await tick();
    assert.equal(prompt.classList.contains('hidden'), false);
    assert.equal(button.disabled, false);
    assert.equal(store.toast.get().text, t('chat.history_save_failed'));
    Api.updateChatPrefs = async () => {};
    button.click(); await tick();
    assert.equal(prompt.classList.contains('hidden'), true);
  } finally { app?.destroy(); Api.getChatPrefs = original.get; Api.updateChatPrefs = original.update; }
});

test('history prompts react to saved settings while the chat stays open and release their subscription', async () => {
  const listeners = Api.chatPrefsListeners.size;
  const store = chatStore(), app = new App(store); document.body.append(app.el);
  const prefs = { ttlSecs: null, ttlChosen: true, clearOnLock: false };
  try {
    await tick();
    const prompt = app.el.querySelector('.history-prompt');
    await Api.setChatPrefs(prefs);
    assert.equal(prompt.classList.contains('hidden'), true);
    await Api.setChatPrefs({ ...prefs, clearOnLock: true });
    assert.equal(prompt.classList.contains('hidden'), false);
    await Api.setChatPrefs({ ...prefs, clearOnLock: true, saveHistory: { 'contact:1': false } });
    assert.equal(prompt.classList.contains('hidden'), true);
  } finally { app.destroy(); }
  assert.equal(Api.chatPrefsListeners.size, listeners);
});

test('peer history banners use explicit live preferences independently of message expiry', async () => {
  const store = chatStore(), app = new App(store); document.body.append(app.el);
  try {
    for (const lang of ['en', 'ru']) {
      setLang(lang); await tick();
      for (const choice of [true, false, null]) {
        store.handleEvent({ PeerHistorySaving: { contact_id: 1, save_history: choice } });
        for (const expires_at of [null, Date.now() + 3600_000]) {
          store.messages.set(new Map([['contact:1', [{ id: 9, outgoing: false, body: 'hello', sent_at: Date.now(), group_id: null, console: null, expires_at }]]]));
          const note = app.el.querySelector('.peer-history-note');
          if (choice == null) { assert.equal(note.classList.contains('hidden'), true, 'an unreported preference shows nothing'); continue; }
          assert.equal(note.textContent, t(choice ? 'chat.peer_saves_history' : 'chat.peer_doesnt_save_history'));
          assert.equal(note.classList.contains('hidden'), false);
        }
      }
      store.selectedChat.set({ kind: 'group', id: 'team' }); await tick();
      assert.equal(app.el.querySelector('.peer-history-note').classList.contains('hidden'), true);
      store.selectedChat.set({ kind: 'contact', id: 1 }); await tick();
    }
  } finally { app.destroy(); }
});

test('overlapping history, timer and clear-on-lock writes merge without losing another choice', async () => {
  const original = { get: Api.getChatPrefs, set: Api.setChatPrefs };
  let prefs = { ttlSecs: null, ttlChosen: false, clearOnLock: true, saveHistory: {} };
  Api.getChatPrefs = async () => ({ ...prefs });
  Api.setChatPrefs = async (value) => { await tick(); prefs = value; };
  try {
    await Promise.all([
      Api.updateChatPrefs({ saveHistory: { 'contact:1': true } }),
      Api.updateChatPrefs({ ttlSecs: 300, ttlChosen: true }),
      Api.updateChatPrefs({ saveHistory: { 'group:team': false } }),
      Api.updateChatPrefs({ clearOnLock: false }),
    ]);
    assert.deepEqual(prefs, { ttlSecs: 300, ttlChosen: true, clearOnLock: false, saveHistory: { 'contact:1': true, 'group:team': false } });
  } finally { Api.getChatPrefs = original.get; Api.setChatPrefs = original.set; }
});

test('switching languages preserves unsent text, attachments, reply, editor and console history', async () => {
  const store = chatStore(); setLang('ru');
  const app = new App(store); document.body.append(app.el); await tick();
  // This is the same state captured for a locale refresh, including non-DOM
  // attachment references and replies which would otherwise silently vanish.
  const main = app.current;
  const draft = main.captureDraft();
  Object.assign(draft, { body: 'Черновик $& <b>unchanged</b>', pending: [{ name: 'Отчёт.pdf', path: '/tmp/report', size: 42 }],
    editingId: 7, editBody: 'Edited draft', replyTo: { id: 5, body: 'Original reply', outgoing: false }, history: ['uname -a'], consoleDraft: 'pwd', historyIndex: 0 });
  main.restoreDraft(draft);
  const search = app.el.querySelector('.sidebar-search');
  search.value = 'Alice'; search.dispatchEvent(new win.Event('input'));
  const before = main.captureDraft();
  const beforeCalls = calls.length;
  setLang('en'); await tick();
  assert.equal(app.current.captureDraft().body, before.body);
  assert.equal(app.el.querySelector('.sidebar-search').value, 'Alice');
  assert.deepEqual(app.current.captureDraft().pending, before.pending);
  assert.deepEqual(app.current.captureDraft().replyTo, before.replyTo);
  assert.deepEqual(app.current.captureDraft().history, before.history);
  assert.equal(app.current.captureDraft().consoleDraft, 'pwd');
  assert.equal(app.current.captureDraft().editingId, 7);
  assert.equal(app.current.captureDraft().editBody, 'Edited draft');
  assert.match(app.el.querySelector('.chat-send').textContent, /Send/);
  assert.match(app.el.querySelector('.sidebar-footer').textContent, /security/);
  assert.equal(app.el.querySelectorAll('script').length, 0);
  assert.ok(!calls.slice(beforeCalls).some((cmd) => /vault_lock|send_message|send_console_command/.test(cmd)));
  setLang('ru'); await tick();
  assert.match(app.el.querySelector('.chat-send').textContent, /Отправить/);
  assert.equal(app.current.captureDraft().body, before.body);
  const messagesListeners = store.messages.subs.size;
  for (let i = 0; i < 4; i++) { setLang(i % 2 ? 'ru' : 'en'); await tick(); }
  assert.equal(store.messages.subs.size, messagesListeners, 'chat subscriptions must not accumulate');
  app.destroy(); assert.equal(store.messages.subs.size, 0);
});

test('all modal families render English and Russian without translating user content', async () => {
  const store = chatStore(); const app = new App(store); document.body.append(app.el);
  const noop = () => {};
  for (const lang of ['en', 'ru']) {
    setLang(lang);
    const modals = [new ui.AboutModal(noop), new ui.ContactModal(store, app, 1, noop), new ui.AddContactModal(store, noop),
      new ui.GroupModal(store, app, 'team', noop), new ui.CreateGroupModal(store, noop), new ui.IdentityModal(store, noop), new ui.SettingsModal(store, app, noop),
      new ui.SearchModal(store, noop), new ui.ForwardModal(store, 1, noop), new ui.MediaModal(store, { kind: 'contact', id: 1 }, noop)];
    for (const modal of modals) {
      document.body.append(modal.el); await tick();
      assert.ok(modal.el.querySelector('.modal-title')?.textContent);
      if (lang === 'en') {
        assert.doesNotMatch(modal.el.textContent.replaceAll('Русский', ''), /[А-Яа-яЁё]/, modal.constructor.name);
        for (const el of modal.el.querySelectorAll('[title], [placeholder], [aria-label]')) {
          assert.doesNotMatch([el.title, el.getAttribute('placeholder'), el.getAttribute('aria-label')].join(' '), /[А-Яа-яЁё]/);
        }
      }
      modal.el.remove();
    }
    assert.equal(ui.avatarNames().length, 24);
    assert.equal(ui.fmtMs(123), lang === 'en' ? '123 ms' : '123 мс');
    assert.equal(ui.fmtAgo(Date.now()), lang === 'en' ? 'just now' : 'только что');
  }
  app.destroy();
});

test('system installer handoff preserves retry and never offers an installed-version restart', async () => {
  for (const lang of ['en', 'ru']) {
    setLang(lang);
    const store = new Store();
    const app = new App(store); document.body.append(app.el);
    const info = { version: '0.4.20', notes: 'Release notes', size: 123 };
    store.handleEvent({ UpdateAvailable: info });
    const before = calls.length;
    store.handleEvent({ UpdateInstallerOpened: { version: info.version, path: '/cache/verified.apk' } });
    const modal = document.querySelector('.update-installer-handoff');
    assert.ok(modal, 'handoff has a visible status even without an existing update dialog');
    assert.ok(modal.textContent.includes(t('app.complete_update_in_system', { version: info.version })));
    assert.equal(store.updateStaged.get(), null);
    assert.equal(store.updateAvailable.get()?.version, info.version, 'cancelled install can be retried');
    assert.ok(![...modal.querySelectorAll('button')].some((button) => button.textContent === t('app.restart')));
    modal.querySelector('button').click(); await tick();
    assert.ok(!document.querySelector('.update-installer-handoff'));
    assert.ok(!calls.slice(before).some((cmd) => /dismiss_update|restart_app/.test(cmd)));
    store.handleEvent({ UpdateStaged: { version: info.version } });
    const restart = [...document.querySelectorAll('.modal button')].find((button) => button.textContent === t('app.restart'));
    assert.ok(restart, 'completed preparation still offers restart');
    [...restart.parentElement.querySelectorAll('button')].find((button) => button !== restart).click();
    app.destroy();
  }
});

test('DM notices survive automatic receipts and disappear only after explicit human confirmation', async () => {
  const previousList = Api.listMessages;
  const previousUnread = Api.unreadCount;
  try {
    for (const lang of ['en', 'ru']) {
      setLang(lang);
      const store = chatStore();
      const message = (id, sequence, outgoing) => ({ id, dm_seq: sequence, contact_id: 1, group_id: null,
        sender_sign_pk: 'a'.repeat(64), outgoing, body: `private ${id}`, sent_at: Date.now(),
        sent: true, delivered: true, read: false, expires_at: null, reply_to: null, console: null,
        deleted_at: null, delete_pending: false });
      let server = [message(101, 1, true), message(202, 2, false), message(303, 3, true)];
      Api.listMessages = async () => server.map((item) => ({ ...item })).reverse();
      Api.unreadCount = async () => 0;
      store.messages.set(new Map([['contact:1', server.map((item) => ({ ...item }))]]));
      let sounds = 0, notifications = 0;
      store.playNotify = () => { sounds++; };
      store.nativeNotify = () => { notifications++; };
      store.fileProgress.set(new Map([[202, { done: 1, total: 3, incoming: true }]]));
      store.pinned.set(new Map([['contact:1', [server[1]]]]));
      const app = new App(store); document.body.append(app.el); await tick();
      for (const outgoing of [true, false]) {
        let invoked = 0;
        const menu = ui.messageMenuItems(store, { kind: 'contact', id: 1 }, { ...server[0], outgoing },
          () => {}, () => {}, () => {}, () => { invoked++; });
        const remove = menu.find((item) => item.label === t('chat.delete_for_both'));
        assert.ok(remove, 'both participants can delete either authorship');
        remove.onClick(); assert.equal(invoked, 1);
      }
      server = server.map((item) => item.id === 202 ? { ...item, body: '', deleted_at: Date.now(), delete_pending: true, deletion_notice: { deleted_at: Date.now(), by_sender: false, can_ack: false } } : item);
      const oldResponse = deferred();
      const staleRows = [message(303, 3, true), message(202, 2, false), message(101, 1, true)];
      Api.listMessages = () => oldResponse.promise;
      const staleLoad = store.loadMessages({ kind: 'contact', id: 1 });
      Api.listMessages = async () => server.map((item) => ({ ...item })).reverse();
      store.handleEvent({ MessageDeleted: { contact_id: 1, message_id: 202, deleted_at: server[1].deleted_at, pending: true, deletion_notice: server[1].deletion_notice } });
      await tick();
      oldResponse.resolve(staleRows); await staleLoad;
      const gap = app.el.querySelector('[data-mid="202"]');
      assert.ok(gap.classList.contains('msg-deleted'));
      assert.ok(gap.textContent.includes(t('chat.deleted_by_you')));
      assert.ok(gap.textContent.includes(t('chat.deletion_waiting_ok')));
      assert.equal(gap.querySelector('.deletion-ok'), null, 'the initiator cannot dismiss their own notice');
      assert.ok(!gap.textContent.includes('private'));
      assert.equal(gap.querySelectorAll('button, img, video, .attachment').length, 0);
      assert.ok(app.el.querySelector('[data-mid="101"] .msg-number').textContent.includes('#1'));
      assert.ok(app.el.querySelector('[data-mid="303"] .msg-number').textContent.includes('#3'));
      assert.equal(ui.messageMenuItems(store, { kind: 'contact', id: 1 }, server[1], () => {}, () => {}, () => {}, () => {}).length, 0);
      assert.equal(store.fileProgress.get().has(202), false);
      assert.equal(store.pinned.get().get('contact:1').length, 0);
      assert.equal(store.unread.get().get('contact:1'), 0);
      assert.equal(sounds, 0); assert.equal(notifications, 0);
      store.handleEvent({ MessageEdited: { message_id: 202, body: 'late private edit', buttons: null } });
      store.handleEvent({ FileProgress: { message_id: 202, contact_id: 1, incoming: true, done: 2, total: 3 } });
      assert.equal(store.messages.get().get('contact:1').find((item) => item.id === 202).body, '');
      assert.equal(store.fileProgress.get().has(202), false);
      server = server.map((item) => item.id === 202 ? { ...item, delete_pending: false } : item);
      store.handleEvent({ MessageDeleted: { contact_id: 1, message_id: 202, deleted_at: server[1].deleted_at, pending: false, deletion_notice: server[1].deletion_notice } });
      await tick();
      assert.ok(app.el.querySelector('[data-mid="202"]'), 'automatic ACK keeps the notice');
      server = server.map(item => item.id === 202 ? { ...item, deletion_notice: null } : item);
      store.handleEvent({ MessageDeleted: { contact_id: 1, message_id: 202, deleted_at: server[1].deleted_at, pending: false, deletion_notice: null } });
      await tick();
      assert.equal(app.el.querySelector('[data-mid="202"]'), null, 'human confirmation dismisses it');
      assert.equal(store.messages.get().get('contact:1').find((item) => item.id === 202).dm_seq, 2);
      assert.equal(sounds, 0); assert.equal(notifications, 0);
      app.destroy();
    }
  } finally { Api.listMessages = previousList; Api.unreadCount = previousUnread; }
});

test('only active chat content is cached and stale list/pinned replies cannot return after switch or lock', async () => {
  const previous = { list: Api.listMessages, pinned: Api.listPinnedContact, lock: Api.vaultLock };
  const store = chatStore();
  const target = { kind: 'contact', id: 1 };
  try {
    const oldList = deferred(), oldPins = deferred();
    let fetches = 0;
    Api.listMessages = () => { fetches++; return oldList.promise; };
    Api.listPinnedContact = () => oldPins.promise;
    const loading = store.loadMessages(target), pinning = store.loadPinned(target);
    store.selectedChat.set({ kind: 'contact', id: 2 });
    oldList.resolve([{ id: 7, body: 'old private body' }]);
    oldPins.resolve([{ id: 7, body: 'old pinned body' }]);
    await Promise.all([loading, pinning]);
    assert.equal(store.messages.get().size, 0);
    assert.equal(store.pinned.get().size, 0);
    const before = fetches;
    store.handleEvent({ IncomingMessage: { message_id: 8, contact_id: 1, group_id: null,
      body: 'background secret', sent_at: Date.now(), sender_sign_pk: null, buttons: null,
      console_kind: null, notify_sound: null } });
    assert.equal(fetches, before, 'background event must not fetch message bodies');
    assert.equal(store.messages.get().has('contact:1'), false);
    assert.equal(store.unread.get().get('contact:1'), 1);

    store.selectedChat.set(target);
    const afterLock = deferred(), afterLockPins = deferred(), vault = deferred();
    Api.listMessages = () => afterLock.promise;
    Api.listPinnedContact = () => afterLockPins.promise;
    Api.vaultLock = () => vault.promise;
    store.cancelToProfileSelect = async () => { store.view.set('profile-select'); };
    store.beginPrewarm = () => {};
    const lateList = store.loadMessages(target), latePins = store.loadPinned(target);
    const locking = store.lock();
    assert.equal(store.selectedChat.get(), null, 'plaintext view closes before native vault await');
    afterLock.resolve([{ id: 7, body: 'locked private body' }]);
    afterLockPins.resolve([{ id: 7, body: 'locked pinned body' }]);
    await Promise.all([lateList, latePins]);
    assert.equal(store.messages.get().size, 0); assert.equal(store.pinned.get().size, 0);
    vault.resolve(); await locking;
    assert.equal(store.messages.get().size, 0); assert.equal(store.pinned.get().size, 0);
  } finally { Api.listMessages = previous.list; Api.listPinnedContact = previous.pinned; Api.vaultLock = previous.lock; }
});

test('destroy and deletion clear chat-owned plaintext/images and reject late attachment replies', async () => {
  const previous = { load: Api.loadAttachment, files: Api.messageFiles,
    messages: Api.listMessages, unread: Api.unreadCount,
    create: URL.createObjectURL, revoke: URL.revokeObjectURL };
  const created = [], revoked = [];
  URL.createObjectURL = () => { const url = `blob:test-${created.length}`; created.push(url); return url; };
  URL.revokeObjectURL = (url) => { revoked.push(url); };
  const store = chatStore();
  const app = new App(store); document.body.append(app.el); await tick();
  const chat = app.current.chat;
  try {
    Api.loadAttachment = async () => 'c2VjcmV0';
    await chat.openImageLazy(9, 'private.png', 7);
    assert.equal(created.length, 1); assert.ok(document.querySelector('.zoom-img'));
    chat.input.value = 'private draft';
    chat.pending = [{ name: 'private file', path: '/private/file', size: 1 }];
    chat.replyTo = { id: 7, body: 'private reply' };
    chat.consoleHistory = ['private command']; chat.consoleDraft = 'private console draft';
    const lateImage = deferred(), lateNames = deferred();
    Api.loadAttachment = () => lateImage.promise;
    Api.messageFiles = () => lateNames.promise;
    const opening = chat.openImageLazy(10, 'late.png', 7);
    chat.fileCache.clear(); chat.attachmentCache.clear();
    chat.queueFiles(7); await tick();
    store.selectedChat.set({ kind: 'contact', id: 2 });
    lateImage.resolve('c2VjcmV0'); lateNames.resolve({ 7: { attachments: [{ id: 10, name: 'late private name', size: 2 }], progress: [] } });
    await opening; await tick();
    assert.deepEqual(revoked, created);
    assert.equal(chat.input.value, ''); assert.equal(chat.replyTo, null);
    assert.deepEqual(chat.pending, []); assert.deepEqual(chat.consoleHistory, []); assert.equal(chat.consoleDraft, '');
    assert.equal(chat.imageDataCache.size, 0); assert.equal(chat.attachmentCache.size, 0);
    assert.equal(chat.renderedSignatures.size, 0); assert.equal(chat.el.textContent, '');
    assert.equal(document.querySelector('.zoom-img'), null);
    assert.equal(created.length, 1, 'late body must not create a new object URL');

    store.selectedChat.set({ kind: 'contact', id: 1 });
    store.messages.set(new Map([['contact:1', [{ id: 7, body: 'live', outgoing: true, sent_at: Date.now(), console: null }]]]));
    const current = app.current.chat;
    const deletedImage = deferred(); Api.loadAttachment = () => deletedImage.promise;
    const beforeDelete = current.openImageLazy(11, 'deleted.png', 7);
    // Keep this test on the event/cache boundary without a native database.
    Api.listMessages = async () => []; Api.unreadCount = async () => 0;
    store.handleEvent({ MessageDeleted: { contact_id: 1, message_id: 7, deleted_at: Date.now(), pending: true } });
    deletedImage.resolve('c2VjcmV0'); await beforeDelete; await tick();
    assert.equal(created.length, 1); assert.equal(current.imageDataCache.size, 0);
  } finally {
    app.destroy(); Api.loadAttachment = previous.load; Api.messageFiles = previous.files;
    Api.listMessages = previous.messages; Api.unreadCount = previous.unread;
    URL.createObjectURL = previous.create; URL.revokeObjectURL = previous.revoke;
  }
});

test('closed search/media modals reject late plaintext and image responses', async () => {
  const previous = { search: Api.searchMessages, media: Api.listMediaContact, load: Api.loadAttachment };
  const store = chatStore(), app = new App(store); document.body.append(app.el); await tick();
  try {
    const hits = deferred(), items = deferred(), image = deferred();
    Api.searchMessages = () => hits.promise;
    Api.listMediaContact = () => items.promise;
    Api.loadAttachment = () => image.promise;
    let search, media, searching;
    app.openModal((close) => {
      search = new ui.SearchModal(store, close); search.input.value = 'private query';
      searching = search.runSearch(); return search.el;
    });
    app.openModal((close) => { media = new ui.MediaModal(store, { kind: 'contact', id: 1 }, close); return media.el; });
    const preview = document.createElement('div'); media.grid.appendChild(preview);
    const filling = media.fillPreview(preview, 9, 'image/png');
    app.closeModals();
    hits.resolve([{ message: { id: 7, body: 'late search private body', sent_at: Date.now() }, contact_id: 1 }]);
    items.resolve([{ id: 9, message_id: 7, name: 'private media.png', size: 1, sent_at: Date.now() }]);
    image.resolve('c2VjcmV0'); await Promise.all([searching, filling]); await tick();
    assert.equal(search.input.value, ''); assert.equal(search.results.textContent, '');
    assert.equal(media.grid.textContent, ''); assert.equal(media.ownedUrls.size, 0);
    assert.equal(document.querySelector('.modal-backdrop'), null);
    assert.equal(document.querySelector('.search-snippet, .media-name, .media-preview img'), null);
  } finally {
    app.destroy(); Api.searchMessages = previous.search; Api.listMediaContact = previous.media; Api.loadAttachment = previous.load;
  }
});

test('closed context menus release document handlers and do not attach them after a delayed open', async () => {
  const add = document.addEventListener, remove = document.removeEventListener;
  const owned = new Map();
  document.addEventListener = function (type, listener, options) {
    if (type === 'pointerdown' || type === 'keydown') owned.set(listener, type);
    return add.call(this, type, listener, options);
  };
  document.removeEventListener = function (type, listener, options) {
    owned.delete(listener); return remove.call(this, type, listener, options);
  };
  try {
    ui.ContextMenu.open(0, 0, [{ label: 'private action', onClick: () => {} }]);
    await tick(); assert.equal(owned.size, 2);
    ui.ContextMenu.close(); assert.equal(owned.size, 0);
    assert.equal(document.querySelector('.ctx-menu'), null);
    ui.ContextMenu.open(0, 0, [{ label: 'private delayed action', onClick: () => {} }]);
    ui.ContextMenu.close(); await tick(); assert.equal(owned.size, 0);
    assert.equal(document.querySelector('.ctx-menu'), null);
    let built = 0;
    const row = document.createElement('div'); document.body.append(row);
    ui.attachContextMenu(row, () => { built++; return [{ label: 'detached secret', onClick: () => {} }]; });
    row.dispatchEvent(new win.PointerEvent('pointerdown', { pointerType: 'touch' })); row.remove();
    await new Promise((resolve) => setTimeout(resolve, 510));
    assert.equal(built, 0); assert.equal(document.querySelector('.ctx-menu'), null);
  } finally {
    ui.ContextMenu.close(); document.addEventListener = add; document.removeEventListener = remove;
  }
});

test('settings omit relay diagnostics, APK catalog and log controls in either build policy', async () => {
  const store = chatStore(); const app = new App(store); document.body.append(app.el);
  const previous = logSettings;
  try {
    for (const available of [false, true]) for (const lang of ['en', 'ru']) {
      logSettings = { available, enabled: available, path: available ? '/tmp/example.log' : '' };
      setLang(lang);
      const before = calls.length;
      const modal = new ui.SettingsModal(store, app, () => {});
      document.body.append(modal.el); await tick();
      assert.equal(modal.el.querySelector('.application-log'), null);
      for (const key of ['settings.application_log', 'settings.android_app', 'settings.relay', 'about.relay_network']) {
        assert.ok(![...modal.el.querySelectorAll('.divider-text')].some(node => node.textContent === t(key)));
      }
      assert.ok(!calls.slice(before).some(cmd => /log_settings|read_.*log|set_log_enabled|clear_debug_log|list_apk_artifacts/.test(cmd)));
      modal.el.remove();
    }
  } finally { logSettings = previous; app.destroy(); }
});
