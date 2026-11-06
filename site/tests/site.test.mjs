import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir, mkdtemp, rm } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { messages, resolveLanguage, applyLanguage, initialize, detectDevice, chooseDownload } from '../main.js';
import { build, publicFiles, catalogKeys, releaseCatalog, renderCatalog, nativeChoices } from '../build.mjs';

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

function documentFixture(source = html) {
  const text = [...source.matchAll(/data-i18n="([^"]+)"/g)].map(match => new Element({ i18n: match[1] }));
  const aria = [...source.matchAll(/data-i18n-aria="([^"]+)"/g)].map(match => new Element({ i18nAria: match[1] }));
  const buttons = ['en', 'ru'].map(language => new Element({ language }));
  const description = new Element();
  const openGraph = Object.fromEntries(['title', 'description', 'locale', 'locale:alternate'].map(key => [key, new Element()]));
  const languageSwitch = new Element();
  return {
    documentElement: {}, title: '', text, aria, buttons, description, openGraph, languageSwitch,
    querySelectorAll(selector) { return { '[data-i18n]': text, '[data-i18n-aria]': aria, '[data-language]': buttons }[selector]; },
    querySelector(selector) {
      return { 'meta[name="description"]': description, '.language-switch': languageSwitch,
        ...Object.fromEntries(Object.entries(openGraph).map(([key, node]) => [`meta[property="og:${key}"]`, node])),
      }[selector];
    },
  };
}

test('both languages cover every rendered string and accessible label', () => {
  const expected = [...new Set([...translationKeys, ...catalogKeys, 'title', 'description'])].sort();
  assert.deepEqual(Object.keys(messages.en).sort(), expected);
  assert.deepEqual(Object.keys(messages.ru).sort(), expected);
  for (const key of expected) {
    assert.ok(messages.en[key].trim(), `empty English ${key}`);
    assert.ok(messages.ru[key].trim(), `empty Russian ${key}`);
    assert.notEqual(messages.ru[key], messages.en[key], `untranslated ${key}`);
  }
  for (const match of html.matchAll(/<([a-z][\w-]*)\b[^>]*data-i18n="([^"]+)"[^>]*>([^<]*)<\/\1>/g)) {
    assert.equal(match[3], messages.ru[match[2]], `Russian fallback differs for ${match[2]}`);
  }
  for (const match of html.matchAll(/<[^>]+aria-label="([^"]*)"[^>]+data-i18n-aria="([^"]+)"[^>]*>/g)) {
    assert.equal(match[1], messages.ru[match[2]], `Russian accessible name differs for ${match[2]}`);
  }
  assert.match(html, /<html lang="ru">/);
  assert.match(messages.en.browserStatus, /Experimental/i);
  assert.match(messages.ru.browserStatus, /Эксперимент/i);
  assert.match(messages.en.browserBoundary, /does not send or receive messages/);
  assert.match(messages.ru.browserBoundary, /не отправляет и не принимает сообщения/);
  assert.match(messages.ru.browserLocal, /HTTPS.*WebAssembly.*OPFS/);
  assert.match(messages.ru.browserTimeline, /проходит проверку/);
  assert.match(messages.en.browserTimeline, /under validation/);
  assert.match(messages.ru.browserStandaloneBody, /OPFS.*WASM/);
  assert.match(messages.en.browserStandaloneBody, /OPFS.*WASM/);
  assert.match(messages.ru.i2pProTunnelsTitle, /Чесночная маршрутизация/);
  assert.match(messages.en.i2pProTunnelsTitle, /Garlic routing/);
  assert.match(messages.ru.i2pConBootstrapTitle, /холодный старт/i);
  assert.match(messages.en.i2pConBootstrapTitle, /cold start/i);
});

test('roadmap distinguishes future features and transport availability from privacy guarantees', () => {
  assert.match(messages.ru.plansIntro, /не означает готовую функцию/);
  assert.match(messages.en.plansIntro, /not an available feature/);
  assert.match(messages.ru.planNameBody, /требуют доработки/);
  assert.match(messages.en.planCallsBody, /does not by itself provide calls/);
  assert.match(messages.ru.i2pConVisibilityBody, /WSS.*WebRTC.*не гарантируют/);
  assert.match(messages.ru.plansBoundary, /сохранить текст.*снимок экрана/);
});

test('Russian is the default regardless of browser locale; explicit language links keep their choice', () => {
  assert.equal(resolveLanguage('', ['ru-RU', 'en-US']), 'ru');
  assert.equal(resolveLanguage('', ['en-GB', 'ru-RU']), 'ru');
  assert.equal(resolveLanguage('', ['fr-FR', 'ru']), 'ru');
  assert.equal(resolveLanguage('', ['fr-FR']), 'ru');
  assert.equal(resolveLanguage(), 'ru');
  assert.equal(resolveLanguage('?lang=en', ['ru-RU']), 'en');
  assert.equal(resolveLanguage('?lang=ru', ['en-US']), 'ru');
  assert.equal(resolveLanguage('?lang=__proto__', ['ru-RU']), 'ru');
  assert.equal(resolveLanguage('?lang=unknown', ['en-US']), 'ru');
  assert.equal(resolveLanguage('?lang=en-US', ['en-US']), 'ru');
});

test('static title, description, Open Graph and pressed language agree with the Russian no-script page', () => {
  assert.equal(html.match(/<title>([^<]+)<\/title>/)[1], messages.ru.title);
  assert.equal(html.match(/<meta name="description" content="([^"]+)"/)[1], messages.ru.description);
  for (const key of ['title', 'description']) {
    assert.equal(html.match(new RegExp(`<meta property="og:${key}" content="([^"]+)"`))[1], messages.ru[key]);
  }
  assert.match(html, /property="og:locale" content="ru_RU"/);
  assert.match(html, /property="og:locale:alternate" content="en_US"/);
  assert.match(html, /data-language="ru" lang="ru" aria-pressed="true"/);
  assert.match(html, /data-language="en" lang="en" aria-pressed="false"/);
});

test('language switch updates content, metadata and pressed state without HTML injection', () => {
  const doc = documentFixture();
  assert.equal(applyLanguage('ru', doc), 'ru');
  assert.equal(doc.documentElement.lang, 'ru');
  assert.equal(doc.title, messages.ru.title);
  assert.equal(doc.description.attributes.content, messages.ru.description);
  assert.equal(doc.openGraph.title.attributes.content, messages.ru.title);
  assert.equal(doc.openGraph.description.attributes.content, messages.ru.description);
  assert.equal(doc.openGraph.locale.attributes.content, 'ru_RU');
  assert.equal(doc.openGraph['locale:alternate'].attributes.content, 'en_US');
  for (const node of doc.text) assert.equal(node.textContent, messages.ru[node.dataset.i18n]);
  for (const node of doc.aria) assert.equal(node.attributes['aria-label'], messages.ru[node.dataset.i18nAria]);
  assert.equal(doc.buttons[0].attributes['aria-pressed'], 'false');
  assert.equal(doc.buttons[1].attributes['aria-pressed'], 'true');
  assert.equal(applyLanguage('en', doc), 'en');
  assert.equal(doc.documentElement.lang, 'en');
  assert.equal(doc.title, messages.en.title);
  assert.equal(doc.description.attributes.content, messages.en.description);
  assert.equal(doc.openGraph.title.attributes.content, messages.en.title);
  assert.equal(doc.openGraph.description.attributes.content, messages.en.description);
  assert.equal(doc.openGraph.locale.attributes.content, 'en_US');
  assert.equal(doc.openGraph['locale:alternate'].attributes.content, 'ru_RU');
  for (const node of doc.text) assert.equal(node.textContent, messages.en[node.dataset.i18n]);
  for (const node of doc.aria) assert.equal(node.attributes['aria-label'], messages.en[node.dataset.i18nAria]);
  assert.equal(doc.buttons[0].attributes['aria-pressed'], 'true');
  assert.equal(doc.buttons[1].attributes['aria-pressed'], 'false');
  assert.equal(applyLanguage('unknown', doc), 'ru');
  assert.doesNotMatch(js, /innerHTML|eval\(|new Function|document\.write/);
});

test('English selection survives initialization and URL history without losing the project prefix', () => {
  const doc = documentFixture();
  const listeners = {};
  const browser = {
    location: new URL('https://example.test/gipny-i2p/?campaign=local#browser'),
    navigator: { languages: ['en-US'] },
    history: { replaceState(_state, _title, url) { browser.location = new URL(url); } },
    addEventListener(name, callback) { listeners[name] = callback; },
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
  const reloaded = documentFixture();
  initialize(reloaded, browser);
  assert.equal(reloaded.documentElement.lang, 'en');
  browser.location.searchParams.set('lang', 'ru');
  listeners.popstate();
  assert.equal(reloaded.documentElement.lang, 'ru');
  browser.location.searchParams.delete('lang');
  listeners.popstate();
  assert.equal(reloaded.documentElement.lang, 'ru');
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
      // app/ is supplied by the UI build in Pages CI, not the landing builder.
      if (target === './app/browser.html') assert.equal(url.pathname, '/gipny-i2p/app/browser.html');
      else await readFile(resolve(root, target));
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

function releaseFixture(version = '0.4.18', extended = false) {
  const names = [
    ...['x64-setup.exe', 'amd64.AppImage', 'aarch64.AppImage', 'amd64.deb', 'arm64.deb', 'x64.dmg', 'aarch64.dmg', 'android-arm64.apk', 'android-armv7.apk'].map(suffix => 'gipny-i2p_' + version + '_' + suffix),
    ...['agent', 'relay'].flatMap(component => ['amd64', 'arm64'].map(arch => 'gipny-' + component + '_' + version + '_linux-' + arch + '.tar.gz')),
    ...(extended ? [
      ...['darwin-amd64', 'darwin-arm64', 'windows-amd64', 'aarch64-unknown-linux-musl', 'armv7-unknown-linux-musleabi', 'i686-unknown-linux-musl', 'x86_64-unknown-linux-musl'].map(target => 'gipny-agent_' + version + '_' + target + '.tar.gz'),
      'gipny-source_' + version + '_' + 'a'.repeat(40) + '.tar.gz', 'BUILD-MANIFEST.json', 'SOURCE-MANIFEST.json',
    ] : []),
    'SHA256SUMS.txt',
  ];
  const base = 'https://github.com/gluckdev/gipny-i2p/releases/';
  return {
    id: 400014206, url: 'https://api.github.com/repos/gluckdev/gipny-i2p/releases/400014206',
    html_url: base + 'tag/v' + version, tag_name: 'v' + version,
    published_at: '2026-10-29T00:00:00Z', draft: false, prerelease: false,
    body: '<script>alert("unused release prose")</script>',
    assets: names.map((name, index) => ({ name, state: 'uploaded', size: 5_000_000 + index * 250_000, browser_download_url: base + 'download/v' + version + '/' + name })),
  };
}

test('catalog offers one native download control and a GitHub Releases link, keeping other packages off the landing', async () => {
  for (const release of [releaseFixture(), releaseFixture('0.4.19', true)]) {
    const output = await mkdtemp(resolve(tmpdir(), 'gipny-catalog-test-'));
    try {
      await build(output, release);
      const rendered = await readFile(resolve(output, 'index.html'), 'utf8');
      const catalog = releaseCatalog(release), choices = nativeChoices(catalog);
      assert.equal(choices.length, 7);
      assert.equal([...rendered.matchAll(/id="download-primary"/g)].length, 1);
      assert.equal([...rendered.matchAll(/data-download-asset="/g)].length, choices.length);
      for (const asset of choices) {
        assert.equal(rendered.split('value="' + asset.url + '"').length - 1, 1, asset.name);
        assert.equal(rendered.split('href="' + asset.url + '"').length - 1, 0, 'button is selected on-device');
      }
      assert.ok(choices.filter(asset => asset.platform === 'Linux').every(asset => asset.format === 'AppImage'));
      assert.match(rendered, /data-i18n="catalogAll"/);
      assert.match(rendered, /Apple Silicon ARM64/);
      assert.doesNotMatch(rendered, /download-advanced|<script>alert|unused release prose|data:image|<iframe/);
      assert.deepEqual((await readdir(output)).sort(), ['assets', 'index.html', 'main.js', 'styles.css']);
      assert.deepEqual(await readFile(resolve(output, 'main.js')), await readFile(resolve(root, 'main.js')));
      const doc = documentFixture(rendered);
      applyLanguage('en', doc);
      for (const node of doc.text) assert.equal(node.textContent, messages.en[node.dataset.i18n]);
    } finally { await rm(output, { recursive: true, force: true }); }
  }
});

test('a smaller release never invents an absent native architecture; full Releases remains available', () => {
  const release = releaseFixture();
  release.assets = release.assets.filter(asset => asset.name.endsWith('x64-setup.exe'));
  const rendered = renderCatalog(releaseCatalog(release));
  assert.equal([...rendered.matchAll(/data-download-asset="/g)].length, 1);
  assert.match(rendered, /data-platform="Windows"/);
  assert.doesNotMatch(rendered, /data-platform="macOS"|data-platform="Linux"|data-platform="Android"/);
});

test('device selection respects real ARM hints, iPad desktop mode and missing architecture information', () => {
  const choices = [
    { platform: 'macOS', architecture: 'arm64', value: 'mac-arm' }, { platform: 'macOS', architecture: 'x64', value: 'mac-intel' },
    { platform: 'Windows', architecture: 'x64', value: 'windows' },
    { platform: 'Android', architecture: 'arm64', value: 'android' }, { platform: 'Linux', architecture: 'x64', value: 'linux' },
  ];
  const mac = { platform: 'MacIntel', userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', maxTouchPoints: 0 };
  assert.equal(chooseDownload(choices, detectDevice(mac)), null, 'Safari does not expose Intel vs Apple Silicon reliably');
  assert.equal(chooseDownload(choices, detectDevice(mac, { platform: 'macOS', architecture: 'arm', bitness: '64' })).value, 'mac-arm');
  assert.equal(chooseDownload(choices, detectDevice(mac, { platform: 'macOS', architecture: 'x86', bitness: '64' })).value, 'mac-intel');
  assert.equal(detectDevice({ ...mac, maxTouchPoints: 5 }).platform, 'iOS');
  assert.equal(chooseDownload(choices, detectDevice({ userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' })), null);
  assert.equal(chooseDownload(choices, detectDevice({}, { platform: 'Windows', architecture: 'x86', bitness: '64' })).value, 'windows');
  assert.equal(chooseDownload(choices, detectDevice({}, { platform: 'Windows', architecture: 'arm', bitness: '64' })), null, 'no native Windows ARM package exists');
  assert.equal(chooseDownload(choices, detectDevice({ userAgent: 'Mozilla/5.0 (X11; Linux x86_64)' })).value, 'linux');
  assert.equal(chooseDownload(choices, detectDevice({ userAgent: 'Mozilla/5.0 (Linux; Android 10; K)' })), null);
  assert.equal(chooseDownload(choices, detectDevice({ userAgent: 'Mozilla/5.0 (Linux; Android 10; K)' }, { architecture: 'arm', bitness: '64' })).value, 'android');
  assert.equal(detectDevice({ userAgent: 'Mozilla/5.0 (X11; CrOS x86_64)' }).platform, 'ChromeOS');
  assert.equal(chooseDownload(choices, detectDevice({})), null);
});

test('catalog rejects unpublished, foreign, mismatched, incomplete or injected release assets before writing output', async () => {
  const changes = [
    value => { value.draft = true; },
    value => { value.prerelease = true; },
    value => { value.published_at = null; },
    value => { value.tag_name = 'v0.4.18"><script>'; },
    value => { value.html_url = 'https://github.com/attacker/gipny-i2p/releases/tag/v0.4.18'; },
    value => { value.url = value.url.replace('gluckdev', 'attacker'); },
    value => { value.assets[0].state = 'new'; },
    value => { value.assets[0].size = 0; },
    value => { value.assets[0].size = Number.MAX_SAFE_INTEGER + 1; },
    value => { value.assets.push(structuredClone(value.assets[0])); },
    value => { value.assets[0].browser_download_url = 'javascript:alert(1)'; },
    value => { value.assets[0].browser_download_url += '?redirect=evil'; },
    value => { value.assets[0].browser_download_url = value.assets[0].browser_download_url.replace('/v0.4.18/', '/v0.4.17/'); },
    value => { value.assets[0].name = 'bad"><img src=x onerror=alert(1)>.exe'; },
    value => { value.assets[0].name = 'gipny-i2p_0.4.18_mips.apk'; value.assets[0].browser_download_url = 'https://github.com/gluckdev/gipny-i2p/releases/download/v0.4.18/' + value.assets[0].name; },
  ];
  const parent = await mkdtemp(resolve(tmpdir(), 'gipny-catalog-invalid-'));
  try {
    for (const change of changes) {
      const release = releaseFixture();
      change(release);
      await assert.rejects(build(resolve(parent, 'public'), release), /release|asset/i);
      assert.deepEqual(await readdir(parent), []);
    }
  } finally { await rm(parent, { recursive: true, force: true }); }
});

test('catalog markup escapes text and attribute metadata instead of interpreting HTML', () => {
  const catalog = releaseCatalog(releaseFixture());
  catalog.assets[0].arch = 'x86_64"><img src=x onerror=alert(1)>';
  const rendered = renderCatalog(catalog);
  assert.doesNotMatch(rendered, /<img|onerror="/);
  assert.match(rendered, /&lt;img/);
  assert.match(rendered, /&quot;/);
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

function blocks(source, expression) {
  return [...source.matchAll(expression)].map(match => {
    const start = source.indexOf('{', match.index);
    let depth = 1, end = start + 1;
    while (depth && end < source.length) {
      if (source[end] === '{') depth++;
      if (source[end] === '}') depth--;
      end++;
    }
    assert.equal(depth, 0, 'unbalanced CSS block');
    return { start, end, body: source.slice(start + 1, end - 1) };
  });
}

test('decorative motion requires opt-in and animates no layout or flashing color properties', () => {
  const allowed = blocks(css, /@media\s*\(prefers-reduced-motion:\s*no-preference\)/g);
  const declarations = [...css.matchAll(/\banimation\s*:\s*([^;]+)/g)].filter(match => !match[1].startsWith('none'));
  assert.ok(declarations.length > 0);
  for (const declaration of declarations) {
    assert.ok(allowed.some(block => declaration.index > block.start && declaration.index < block.end), 'motion outside no-preference');
  }
  for (const frame of blocks(css, /@keyframes\s+[\w-]+/g)) {
    for (const property of frame.body.matchAll(/([\w-]+)\s*:/g)) {
      assert.ok(['opacity', 'transform'].includes(property[1]), 'motion changes ' + property[1]);
    }
  }
  const reduced = blocks(css, /@media\s*\(prefers-reduced-motion:\s*reduce\)/g);
  assert.ok(reduced.some(block => /\*::before/.test(block.body) && /animation:\s*none\s*!important/.test(block.body)), 'reduced motion must disable pseudo-element animation too');
});

test('Pages workflow validates PRs and only deploys the reviewed main branch', async () => {
  const workflow = await readFile(resolve(root, '../.github/workflows/pages.yml'), 'utf8');
  assert.match(workflow, /pull_request:/);
  assert.match(workflow, /workflow_run:\s*\n\s+workflows: \[release\]\s*\n\s+types: \[completed\]\s*\n\s+branches: \[main\]/);
  assert.doesNotMatch(workflow, /pull_request_target/);
  const deployment = workflow.split('\n  deploy:')[1];
  assert.match(deployment, /github\.ref == 'refs\/heads\/main'/);
  assert.match(deployment, /github\.event_name == 'push' \|\| github\.event_name == 'workflow_dispatch'/);
  assert.match(deployment, /github\.event_name == 'workflow_run' && github\.event\.workflow_run\.conclusion == 'success' && github\.event\.workflow_run\.head_repository\.full_name == github\.repository/);
  const preparation = workflow.split('\n  deploy:')[0];
  assert.match(preparation, /github\.event_name != 'workflow_run' \|\| \(github\.event\.workflow_run\.conclusion == 'success' && github\.event\.workflow_run\.head_repository\.full_name == github\.repository\)/);
  assert.match(preparation, /github\.event_name == 'workflow_run' && 'refs\/heads\/main'/);
  assert.match(preparation, /releases\?per_page=100/);
  assert.match(preparation, /node site\/build\.mjs _site "\$RUNNER_TEMP\/gipny-release\.json"/);
  assert.doesNotMatch(preparation, /workflow_run\.head_sha|workflow_run\.head_branch|download-artifact/);
  assert.match(deployment, /enablement: false/);
  assert.match(workflow, /path: _site/);
  assert.match(workflow, /actions\/configure-pages@v6/);
  assert.match(workflow, /actions\/upload-pages-artifact@v5/);
  assert.match(workflow, /actions\/deploy-pages@v5/);
  assert.doesNotMatch(workflow.split('\n  deploy:')[0], /pages: write|id-token: write/);
});


test('WASM browser app is packaged separately without changing the landing CSP', async () => {
  assert.match(html, /href="\.\/app\/browser\.html"/);
  for (const lang of ['ru', 'en']) {
    assert.match(messages[lang].browserBody, /Pages/);
    assert.match(messages[lang].browserLocal, /OPFS/);
  }
  const workflow = await readFile(resolve(root, '../.github/workflows/pages.yml'), 'utf8');
  assert.match(workflow, /VITE_BASE: \/gipny-i2p\/app\//);
  assert.match(workflow, /cp -R ui\/browser-dist\/\. _site\/app\//);
  assert.match(workflow, /npm test --prefix ui/);
});
