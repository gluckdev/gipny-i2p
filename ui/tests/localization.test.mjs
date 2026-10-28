import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
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
const { t, setLang, getLang, translations, App, Store, Api } = ui;
const calls = [];
ui.mockIPC((cmd) => {
  calls.push(cmd);
  const responses = {
    get_contact_folders: [], get_avatar_choices: {}, get_chat_prefs: { ttlSecs: null, ttlChosen: true },
    list_attachments: [], list_group_members: [], list_media_contact: [], list_media_group: [],
    link_stats: { lane: 'normal', our_hops: 3, their_hops: 3, padded: true, route: 'relay', rtt_ms: null }, get_relay_info: null, get_relay_address: '', my_b32: '',
    update_configured: false, list_apk_artifacts: { version: '0.4.18', artifacts: [] },
    current_version: '0.4.18', get_attachment_privacy: true, get_clear_on_lock: false,
    get_agent_mode: null, get_router_settings: { transit: 'balanced', yggdrasil: 'off' },
    log_settings: { enabled: false, path: '/tmp/example.log' },
    my_bundle: { sign_pk: 'a'.repeat(64), dh_pk: 'b'.repeat(64), signed_prekey: 'c'.repeat(64), signed_prekey_sig: 'd'.repeat(128), one_time_prekey: null },
  };
  return responses[cmd] ?? null;
});
const tick = () => new Promise((resolve) => setTimeout(resolve, 10));
after(async () => { await win.happyDOM.abort(); await rm(dir, { recursive: true, force: true }); });

test('locale choice respects saved preferences and handles unavailable storage', () => {
  assert.equal(ui.detectLang('ru', ['en-US']), 'ru');
  assert.equal(ui.detectLang('en', ['ru-RU']), 'en');
  assert.equal(ui.detectLang(null, ['ru-RU']), 'ru');
  assert.equal(ui.detectLang(null, ['de-DE']), 'en');
  assert.equal(ui.detectLang('invalid', []), 'en');
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
