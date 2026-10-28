import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir, mkdtemp, rm } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { messages, resolveLanguage, applyLanguage, initialize } from '../main.js';
import { build, publicFiles } from '../build.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const html = await readFile(resolve(root, 'index.html'), 'utf8');
const css = await readFile(resolve(root, 'styles.css'), 'utf8');
const js = await readFile(resolve(root, 'main.js'), 'utf8');
const translationKeys = [...html.matchAll(/data-i18n(?:-aria)?="([^"]+)"/g)].map(match => match[1]);

class Element {
  constructor(dataset = {}) { this.dataset = dataset; this.attributes = {}; this.listeners = {}; this.hidden = true; }
  setAttribute(key, value) { this.attributes[key] = value; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
}

function documentFixture() {
  const text = [...html.matchAll(/data-i18n="([^"]+)"/g)].map(match => new Element({ i18n: match[1] }));
  const aria = [...html.matchAll(/data-i18n-aria="([^"]+)"/g)].map(match => new Element({ i18nAria: match[1] }));
  const buttons = ['en', 'ru'].map(language => new Element({ language }));
  const description = new Element();
  const languageSwitch = new Element();
  return {
    documentElement: {}, title: '', text, aria, buttons, description, languageSwitch,
    querySelectorAll(selector) { return { '[data-i18n]': text, '[data-i18n-aria]': aria, '[data-language]': buttons }[selector]; },
    querySelector(selector) { return { 'meta[name="description"]': description, '.language-switch': languageSwitch }[selector]; },
  };
}

test('both languages cover every rendered string and accessible label', () => {
  const expected = [...new Set([...translationKeys, 'title', 'description'])].sort();
  assert.deepEqual(Object.keys(messages.en).sort(), expected);
  assert.deepEqual(Object.keys(messages.ru).sort(), expected);
  for (const key of expected) {
    assert.ok(messages.en[key].trim(), `empty English ${key}`);
    assert.ok(messages.ru[key].trim(), `empty Russian ${key}`);
    assert.notEqual(messages.ru[key], messages.en[key], `untranslated ${key}`);
  }
  for (const match of html.matchAll(/<([a-z][\w-]*)\b[^>]*data-i18n="([^"]+)"[^>]*>([^<]*)<\/\1>/g)) {
    assert.equal(match[3], messages.en[match[2]], `English fallback differs for ${match[2]}`);
  }
  assert.match(html, /<html lang="en">/);
  assert.match(messages.en.browserStatus, /not available/i);
  assert.match(messages.ru.browserStatus, /недоступно/i);
  assert.match(messages.en.browserBoundary, /does not send or receive messages/);
  assert.match(messages.ru.browserBoundary, /не отправляет и не принимает сообщения/);
});

test('language preference honors explicit links, supported browser locales and English fallback', () => {
  assert.equal(resolveLanguage('', ['ru-RU', 'en-US']), 'ru');
  assert.equal(resolveLanguage('', ['en-GB', 'ru-RU']), 'en');
  assert.equal(resolveLanguage('', ['fr-FR', 'ru']), 'ru');
  assert.equal(resolveLanguage('', ['fr-FR']), 'en');
  assert.equal(resolveLanguage(), 'en');
  assert.equal(resolveLanguage('?lang=en', ['ru-RU']), 'en');
  assert.equal(resolveLanguage('?lang=ru', ['en-US']), 'ru');
  assert.equal(resolveLanguage('?lang=__proto__', ['ru-RU']), 'ru');
});

test('language switch updates content, metadata and pressed state without HTML injection', () => {
  const doc = documentFixture();
  assert.equal(applyLanguage('ru', doc), 'ru');
  assert.equal(doc.documentElement.lang, 'ru');
  assert.equal(doc.title, messages.ru.title);
  assert.equal(doc.description.attributes.content, messages.ru.description);
  for (const node of doc.text) assert.equal(node.textContent, messages.ru[node.dataset.i18n]);
  for (const node of doc.aria) assert.equal(node.attributes['aria-label'], messages.ru[node.dataset.i18nAria]);
  assert.equal(doc.buttons[0].attributes['aria-pressed'], 'false');
  assert.equal(doc.buttons[1].attributes['aria-pressed'], 'true');
  assert.equal(applyLanguage('unknown', doc), 'en');
  assert.equal(doc.documentElement.lang, 'en');
  assert.doesNotMatch(js, /innerHTML|eval\(|new Function|document\.write/);
});

test('initialization and native button clicks preserve the project Pages base path', () => {
  const doc = documentFixture();
  const browser = {
    location: new URL('https://example.test/gipny-i2p/?campaign=local#browser'),
    navigator: { languages: ['ru-RU'] },
    history: { replaceState(_state, _title, url) { browser.location = new URL(url); } },
    addEventListener() {},
  };
  initialize(doc, browser);
  assert.equal(doc.documentElement.lang, 'ru');
  assert.equal(doc.languageSwitch.hidden, false);
  doc.buttons[0].listeners.click();
  assert.equal(doc.documentElement.lang, 'en');
  assert.equal(browser.location.pathname, '/gipny-i2p/');
  assert.equal(browser.location.searchParams.get('lang'), 'en');
  assert.equal(browser.location.searchParams.get('campaign'), 'local');
  assert.equal(browser.location.hash, '#browser');
});

test('content policy allows only local resources and denies application connections', () => {
  const policy = html.match(/http-equiv="Content-Security-Policy" content="([^"]+)"/)[1];
  const directives = new Map(policy.split(';').map(value => value.trim().split(/\s+/)).filter(value => value[0]).map(([name, ...values]) => [name, values]));
  for (const name of ['default-src', 'connect-src', 'object-src', 'base-uri', 'form-action']) assert.deepEqual(directives.get(name), ["'none'"]);
  for (const name of ['script-src', 'style-src', 'img-src', 'font-src']) assert.deepEqual(directives.get(name), ["'self'"]);
  assert.doesNotMatch(html, /\son\w+\s*=|<form\b|<iframe\b|<input\b|<textarea\b|\sstyle\s*=/i);
  assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)[^>]*>/i);
  assert.doesNotMatch(js, /\bfetch\s*\(|\bWebSocket\b|\bXMLHttpRequest\b|sendBeacon|localStorage|sessionStorage|document\.cookie/);
  assert.doesNotMatch(css, /@import|url\(/i);
});

test('local resources and section links work below a GitHub project prefix', async () => {
  const ids = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]));
  for (const match of html.matchAll(/\b(?:href|src)="([^"]+)"/g)) {
    const target = match[1];
    if (target.startsWith('#')) {
      assert.ok(ids.has(target.slice(1)), `missing anchor ${target}`);
    } else if (target.startsWith('https://')) {
      assert.equal(new URL(target).hostname, 'github.com');
    } else {
      assert.ok(target.startsWith('./'), `resource must be relative: ${target}`);
      const url = new URL(target, 'https://example.test/gipny-i2p/');
      assert.ok(url.pathname.startsWith('/gipny-i2p/'));
      await readFile(resolve(root, target));
    }
  }
});

test('build publishes only the four static public files, preserving their bytes', async () => {
  const output = await mkdtemp(resolve(tmpdir(), 'gipny-site-test-'));
  try {
    await build(output);
    assert.deepEqual((await readdir(output)).sort(), ['assets', 'index.html', 'main.js', 'styles.css']);
    assert.deepEqual(await readdir(resolve(output, 'assets')), ['logo.svg']);
    for (const file of publicFiles) assert.deepEqual(await readFile(resolve(output, file)), await readFile(resolve(root, file)));
    await assert.rejects(build(root), /must differ/);
  } finally { await rm(output, { recursive: true, force: true }); }
});

function contrast(first, second) {
  const luminance = value => {
    const values = value.match(/[0-9a-f]{2}/g).map(channel => parseInt(channel, 16) / 255)
      .map(channel => channel <= .04045 ? channel / 12.92 : ((channel + .055) / 1.055) ** 2.4);
    return values[0] * .2126 + values[1] * .7152 + values[2] * .0722;
  };
  const values = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (values[0] + .05) / (values[1] + .05);
}

test('text contrast, keyboard focus and reduced motion remain explicit', () => {
  const tokens = Object.fromEntries([...css.matchAll(/--([a-z-]+): (#[0-9a-f]{6});/g)].map(match => [match[1], match[2]]));
  for (const background of ['background', 'surface', 'surface-lifted']) {
    for (const text of ['text', 'muted', 'mint']) assert.ok(contrast(tokens[text], tokens[background]) >= 4.5, `${text}/${background} contrast`);
  }
  assert.match(css, /:focus-visible/);
  assert.match(css, /prefers-reduced-motion: reduce/);
  assert.match(css, /@media \(max-width: 740px\)/);
  assert.match(html, /class="skip-link" href="#main"/);
  assert.match(html, /<button type="button" data-language="en"/);
  assert.match(html, /<button type="button" data-language="ru"/);
});

test('Pages workflow validates PRs and only deploys the reviewed main branch', async () => {
  const workflow = await readFile(resolve(root, '../.github/workflows/pages.yml'), 'utf8');
  assert.match(workflow, /pull_request:/);
  assert.doesNotMatch(workflow, /pull_request_target/);
  const deployment = workflow.split('\n  deploy:')[1];
  assert.match(deployment, /github\.ref == 'refs\/heads\/main'/);
  assert.match(deployment, /github\.event_name == 'push' \|\| github\.event_name == 'workflow_dispatch'/);
  assert.match(deployment, /enablement: false/);
  assert.match(workflow, /path: _site/);
  assert.match(workflow, /actions\/configure-pages@v6/);
  assert.match(workflow, /actions\/upload-pages-artifact@v5/);
  assert.match(workflow, /actions\/deploy-pages@v5/);
  assert.doesNotMatch(workflow.split('\n  deploy:')[0], /pages: write|id-token: write/);
});
