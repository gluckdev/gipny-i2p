#!/usr/bin/env node
// GHA-only real-browser checks; no packages, browser downloads or app builds.
import assert from 'node:assert/strict';
import { constants } from 'node:fs';
import { access, readFile, writeFile, mkdir, mkdtemp, realpath, rm, stat } from 'node:fs/promises';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { tmpdir } from 'node:os';
import { resolve, join, sep, extname } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const prefix = '/gipny-i2p/';
const report = { schema: 1, commit: process.env.GITHUB_SHA ?? null, checks: [], runtimeErrors: [] };
const output = resolve(process.argv[3] ?? 'site-browser-results');
let browser, browserClosed, server, cdp, profile, browserLog = '';
const pathMotion = `(() => [...document.querySelectorAll('.message-path .route-endpoint, .message-path .route-network, .message-path .route-connection i')]
  .flatMap(node => [null, '::before', '::after'].map(pseudo => {
    const css = getComputedStyle(node, pseudo);
    return { node: node.className || node.tagName, pseudo, name: css.animationName,
      duration: css.animationDuration, state: css.animationPlayState };
  })))()`;

async function until(test, label, timeout = 20_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await test();
    if (value) return value;
    await delay(100);
  }
  throw new Error(`Timed out: ${label}`);
}

class CDP {
  constructor(socket) {
    this.socket = socket;
    this.sequence = 0;
    this.pending = new Map();
    this.loaded = new Set();
    socket.addEventListener('message', ({ data }) => {
      let message;
      try { message = JSON.parse(data); } catch { return; }
      if (message.id) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        this.pending.delete(message.id);
        clearTimeout(pending.timer);
        if (message.error) pending.reject(new Error(JSON.stringify(message.error)));
        else pending.resolve(message.result);
      } else if (message.method === 'Page.lifecycleEvent' && message.params.name === 'load') {
        this.loaded.add(message.params.loaderId);
      } else if (message.method === 'Runtime.exceptionThrown') {
        const error = message.params.exceptionDetails;
        report.runtimeErrors.push(error.exception?.description ?? error.text);
      } else if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
        report.runtimeErrors.push(message.params.args.map(arg => arg.description ?? String(arg.value)).join(' '));
      }
    });
    socket.addEventListener('close', () => {
      for (const pending of this.pending.values()) {
        clearTimeout(pending.timer);
        pending.reject(new Error('Chrome DevTools connection closed'));
      }
      this.pending.clear();
    });
  }
  call(method, params = {}) {
    return new Promise((resolveCall, reject) => {
      if (this.socket.readyState !== WebSocket.OPEN) return reject(new Error('Chrome is unavailable'));
      const id = ++this.sequence;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP timeout: ${method}`));
      }, 15_000);
      this.pending.set(id, { resolve: resolveCall, reject, timer });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
  async evaluate(expression) {
    const result = await this.call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    assert(!result.exceptionDetails, result.exceptionDetails?.text ?? 'Browser evaluation failed');
    return result.result.value;
  }
}

async function startServer(root) {
  const mime = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
    '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml', '.png': 'image/png',
    '.webp': 'image/webp', '.jpg': 'image/jpeg', '.json': 'application/json' };
  server = createServer(async (request, response) => {
    try {
      const path = decodeURIComponent(new URL(request.url, 'http://127.0.0.1').pathname);
      if (!['GET', 'HEAD'].includes(request.method) || !path.startsWith(prefix)) {
        response.writeHead(404).end(); return;
      }
      const relative = path.slice(prefix.length) || 'index.html';
      const file = await realpath(resolve(root, relative));
      if (!file.startsWith(root + sep) || !(await stat(file)).isFile()) {
        response.writeHead(404).end(); return;
      }
      const content = await readFile(file);
      response.writeHead(200, { 'Content-Type': mime[extname(file)] ?? 'application/octet-stream',
        'Content-Length': content.length, 'Cache-Control': 'no-store' });
      response.end(request.method === 'HEAD' ? undefined : content);
    } catch { response.writeHead(404).end(); }
  });
  await new Promise((yes, no) => { server.once('error', no); server.listen(0, '127.0.0.1', yes); });
  return `http://127.0.0.1:${server.address().port}${prefix}`;
}

async function startChrome() {
  const candidates = [process.env.CHROME_BIN, process.env.CHROME_PATH, process.env.GOOGLE_CHROME_BIN,
    '/usr/bin/google-chrome', '/usr/bin/google-chrome-stable', '/usr/bin/chromium', '/usr/bin/chromium-browser'];
  let executable;
  for (const candidate of candidates.filter(Boolean)) {
    try { await access(candidate, constants.X_OK); executable = candidate; break; } catch { /* next installed browser */ }
  }
  assert(executable, 'GitHub runner must provide Chrome/Chromium; this helper never downloads a browser');
  profile = await mkdtemp(join(tmpdir(), 'gipny-site-browser-'));
  browser = spawn(executable, ['--headless=new', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage',
    '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-component-update',
    '--disable-sync', '--no-pings', '--remote-debugging-address=127.0.0.1', '--remote-debugging-port=0',
    '--remote-allow-origins=*', // Ephemeral loopback CDP endpoint, only in this GHA fixture.
    `--user-data-dir=${profile}`, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] });
  browserClosed = new Promise(yes => browser.once('close', yes));
  let spawnError;
  browser.once('error', error => { spawnError = error; });
  browser.stderr.on('data', chunk => { browserLog = (browserLog + chunk).slice(-32_768); });
  const port = await until(async () => {
    if (spawnError) throw spawnError;
    if (browser.exitCode !== null) throw new Error(`Chrome exited: ${browserLog}`);
    try { return Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]) || false; }
    catch { return false; }
  }, 'Chrome debugging port');
  const target = await until(async () => {
    const response = await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(3_000) });
    assert(response.ok, 'Chrome target discovery failed');
    return (await response.json()).find(item => item.type === 'page' && item.webSocketDebuggerUrl);
  }, 'Chrome page target');
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((yes, no) => {
    const timer = setTimeout(() => { socket.close(); no(new Error('Chrome WebSocket timeout')); }, 10_000);
    socket.addEventListener('open', () => { clearTimeout(timer); yes(); }, { once: true });
    socket.addEventListener('error', () => { clearTimeout(timer); no(new Error('Chrome WebSocket failed')); }, { once: true });
  });
  cdp = new CDP(socket);
  await cdp.call('Page.enable');
  await cdp.call('Page.setLifecycleEventsEnabled', { enabled: true });
  await cdp.call('Runtime.enable');
  await cdp.call('Emulation.setLocaleOverride', { locale: 'en-US' });
}

async function ready(language) {
  await until(() => cdp.evaluate(`document.readyState === 'complete' && document.documentElement.lang === ${JSON.stringify(language)}
    && !document.querySelector('.language-switch')?.hidden`), `page ready (${language})`);
  await cdp.evaluate(`(async () => { await document.fonts.ready;
    await Promise.all([...document.images].map(image => image.complete ? Promise.resolve() : new Promise((yes, no) => {
      image.addEventListener('load', yes, {once:true}); image.addEventListener('error', () => no(new Error('Image failed')), {once:true});
    })));
    return true; })()`);
}

async function snapshot(width, language) {
  const name = `${width}-${language}`;
  const data = await cdp.evaluate(`(() => {
    const visible = node => { const rect = node.getBoundingClientRect(); const css = getComputedStyle(node);
      return rect.width > 0 && rect.height > 0 && css.visibility !== 'hidden' && css.display !== 'none'; };
    const links = [...document.querySelectorAll('a[data-download-asset]')].map(node => ({
      name: node.dataset.downloadAsset, href: node.href, visible: visible(node),
      advanced: !!node.closest('details:not([open])'), text: node.textContent.trim() }));
    return { language: document.documentElement.lang, title: document.title, url: location.href,
      viewport: innerWidth, scrollWidth: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth),
      fonts: document.fonts.status, brokenImages: [...document.images].filter(image => !image.complete || image.naturalWidth === 0).length,
      downloads: links, hero: document.querySelector('h1')?.textContent.trim() };
  })()`);
  report.checks.push({ name, ...data });
  // Keep visual evidence even when the layout assertion below fails.
  await cdp.evaluate('scrollTo(0, 0)');
  const layout = await cdp.call('Page.getLayoutMetrics');
  const size = layout.cssContentSize ?? layout.contentSize;
  const screenshot = await cdp.call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true,
    clip: { x: 0, y: 0, width, height: Math.max(1, Math.min(20_000, Math.ceil(size.height))), scale: 1 } });
  await writeFile(join(output, `${name}.png`), Buffer.from(screenshot.data, 'base64'));
  assert(size.height <= 20_000, `${name}: unexpected page height`);
  assert.equal(data.language, language);
  assert(data.hero && data.fonts === 'loaded' && data.brokenImages === 0, `${name}: content/fonts/images are not ready`);
  assert(data.scrollWidth <= data.viewport + 1, `${name}: horizontal overflow (${data.scrollWidth} > ${data.viewport})`);
  const normalMotion = await cdp.evaluate(pathMotion);
  report.checks.at(-1).normalMotion = normalMotion;
  assert(normalMotion.some(item => item.name !== 'none' && item.duration.split(',').some(value => parseFloat(value) > 0)
    && item.state !== 'paused'), `${name}: message path has no active CSS animation`);
  for (const link of data.downloads) {
    const url = new URL(link.href);
    assert(url.protocol === 'https:' && url.hostname === 'github.com'
      && /^\/gluckdev\/gipny-i2p\/releases\/download\//.test(url.pathname), `${name}: invalid download URL ${link.href}`);
    if (!link.advanced) assert(link.visible && link.text, `${name}: hidden/empty download link ${link.name}`);
  }
  await cdp.call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
  await cdp.evaluate('new Promise(yes => requestAnimationFrame(() => requestAnimationFrame(yes)))');
  const motion = await cdp.evaluate(`({ requested: matchMedia('(prefers-reduced-motion: reduce)').matches,
    active: document.getAnimations().filter(animation => animation instanceof CSSAnimation && animation.playState === 'running').length })`);
  motion.computedPath = await cdp.evaluate(pathMotion);
  report.checks.at(-1).reducedMotion = motion;
  assert(motion.requested && motion.active === 0, `${name}: reduced motion left CSS animations running`);
  assert(motion.computedPath.every(item => item.name.split(',').every(value => value.trim() === 'none')
    || item.duration.split(',').every(value => parseFloat(value) === 0)), `${name}: reduced motion retained a path animation`);
  assert.equal(report.runtimeErrors.length, 0, JSON.stringify(report.runtimeErrors));
  await cdp.call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'no-preference' }] });
  return data;
}

async function run() {
  assert.equal(process.env.GITHUB_ACTIONS, 'true', 'This browser helper runs only inside GitHub Actions');
  assert(Number(process.versions.node.split('.')[0]) >= 24, 'Node 24+ is required');
  const root = await realpath(resolve(process.argv[2] ?? '_site'));
  await access(join(root, 'index.html'));
  const base = await startServer(root);
  await startChrome();
  for (const width of [320, 360, 390, 768, 1440]) {
    await cdp.call('Emulation.setDeviceMetricsOverride', { width, height: width < 768 ? 844 : 1000,
      deviceScaleFactor: 1, mobile: width < 768 });
    const url = `${base}?browser-check=keep#downloads`;
    const navigation = await cdp.call('Page.navigate', { url });
    assert(!navigation.errorText, navigation.errorText);
    assert(navigation.loaderId, 'Expected a fresh document navigation');
    await until(() => cdp.loaded.has(navigation.loaderId), 'document load event');
    await ready('ru');
    const russian = await snapshot(width, 'ru');
    await cdp.evaluate(`document.querySelector('[data-language="en"]').scrollIntoView({block:'center'})`);
    const point = await cdp.evaluate(`(() => { const rect = document.querySelector('[data-language="en"]').getBoundingClientRect();
      return {x:rect.x + rect.width / 2, y:rect.y + rect.height / 2}; })()`);
    await cdp.call('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...point });
    await cdp.call('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...point });
    await ready('en');
    const english = await snapshot(width, 'en');
    const preserved = new URL(english.url);
    assert.equal(preserved.searchParams.get('lang'), 'en');
    assert.equal(preserved.searchParams.get('browser-check'), 'keep');
    assert.equal(preserved.hash, '#downloads');
    assert.notEqual(english.hero, russian.hero, `${width}: English toggle did not translate the hero`);
    assert.notEqual(english.title, russian.title, `${width}: English title did not change`);
    const previousDocument = await cdp.evaluate('performance.timeOrigin');
    await cdp.call('Page.reload', { ignoreCache: true });
    await until(() => cdp.evaluate(`performance.timeOrigin !== ${JSON.stringify(previousDocument)}`), 'fresh reloaded document');
    await ready('en'); // The explicit URL must retain the language after reload.
  }
  assert.equal(report.runtimeErrors.length, 0, JSON.stringify(report.runtimeErrors));
}

async function cleanup() {
  const waitForBrowser = async () => {
    let timer;
    try {
      return await Promise.race([browserClosed.then(() => true),
        new Promise(yes => { timer = setTimeout(() => yes(false), 2_000); })]);
    } finally { clearTimeout(timer); }
  };
  let unclosed = false;
  if (browser) {
    // Let Chrome stop its profile writers before removing the owned directory.
    // Closing Chrome can close CDP before its command response; that is expected.
    if (cdp?.socket.readyState === WebSocket.OPEN) cdp.call('Browser.close').catch(() => {});
    if (!await waitForBrowser()) {
      browser.kill('SIGTERM');
      if (!await waitForBrowser()) {
        browser.kill('SIGKILL');
        unclosed = !await waitForBrowser();
      }
    }
  }
  cdp?.socket.close();
  if (server) { server.closeAllConnections(); await new Promise(yes => server.close(yes)); }
  if (profile) await rm(profile, { recursive: true, force: true, maxRetries: 6, retryDelay: 100 });
  assert(!unclosed, 'Chrome did not close after its bounded shutdown');
}

await mkdir(output, { recursive: true });
let deadline;
try {
  await Promise.race([run(), new Promise((_, no) => { deadline = setTimeout(() => no(new Error('Overall browser-check timeout')), 240_000); })]);
  report.success = true;
} catch (error) {
  report.success = false;
  report.error = error.stack ?? String(error);
  report.browserLog = browserLog;
  process.exitCode = 1;
} finally {
  clearTimeout(deadline);
  try { await cleanup(); } catch (error) {
    report.cleanupError = error.stack ?? String(error);
    // Keep the original assertion/setup failure when cleanup also fails.
    if (report.success) { report.success = false; report.error = 'Browser cleanup failed'; }
    process.exitCode = 1;
  }
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(`Browser checks: ${report.success ? 'PASS' : 'FAIL'} (${report.checks.length} viewport/language cases); ${output}`);
}
