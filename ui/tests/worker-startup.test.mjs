// Actual module-worker/WASM loading through the production CSP. No
// external bridge is needed. Run after build:browser; needs Chromium.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { extname, resolve } from 'node:path';
import { chromium } from 'playwright';

const dist = resolve(new URL('../browser-dist/', import.meta.url).pathname);
const mime = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css', '.json': 'application/json', '.webmanifest': 'application/manifest+json' };
let fault = '';
let publishConfig = false;
const server = createServer((req, res) => {
  const path = new URL(req.url, 'http://local').pathname;
  const file = resolve(dist, '.' + path);
  if (path === '/browser-config.json' && publishConfig) {
    const origin = '127.0.0.1:' + server.address().port;
    res.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
    res.end(JSON.stringify({ snapshot_url: 'http://' + origin + '/test-snapshot', snapshot_sha256: 'a'.repeat(64),
      forwarder_url: 'ws://' + origin + '/stream', capability: 'b'.repeat(43) }));
    return;
  }
  if (!file.startsWith(dist + '/') || path === '/browser-config.json') { res.writeHead(404).end(); return; }
  try {
    let bytes = readFileSync(file);
    if (/\/worker-[^/]+\.js$/.test(path)) {
      const observation = "self.addEventListener('unhandledrejection', e => console.error('__UNHANDLED_WASM__' + String(e.reason)));\n";
      const injection = fault === 'worker-missing' ? 'globalThis.WebAssembly = undefined;\n'
        : fault === 'compile-failed' ? "WebAssembly.instantiateStreaming = async () => { throw new WebAssembly.CompileError('test unsupported module instruction'); };\n" : '';
      bytes = Buffer.concat([Buffer.from(observation + injection), bytes]);
    }
    res.writeHead(200, { 'Content-Type': mime[extname(file)] || 'application/octet-stream', 'Content-Length': bytes.length, 'Cache-Control': 'no-store' }); res.end(bytes);
  } catch { res.writeHead(404).end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
let browser;
try {
  browser = await chromium.launch({ headless: true, executablePath: process.env.GIPNY_CHROMIUM || '/usr/bin/google-chrome' });
  for (const mode of ['normal', 'online-before-open', 'window-missing', 'worker-missing', 'compile-failed']) {
    fault = mode;
    publishConfig = false;
    const context = await browser.newContext({ serviceWorkers: 'block' });
    try {
      const page = await context.newPage();
      await page.addInitScript(() => { localStorage.setItem('gipny.setup', 'done'); });
      const errors = [], unhandled = [], external = [];
      page.on('pageerror', error => errors.push(error.message));
      page.on('console', message => { if (message.text().includes('__UNHANDLED_WASM__')) unhandled.push(message.text()); });
      page.on('request', request => { if (!request.url().startsWith('http://127.0.0.1:')) external.push(request.url()); });
      if (mode === 'window-missing') await page.addInitScript(() => { globalThis.WebAssembly = undefined; });
      await page.goto(`http://127.0.0.1:${server.address().port}/browser.html`);
      if (mode === 'worker-missing' || mode === 'compile-failed') await page.waitForSelector('#app .auth.err', { timeout: 30_000 });
      else await page.waitForSelector('#app #create-name', { timeout: 30_000 });
      const text = await page.locator('#app').innerText();
      if (mode === 'worker-missing') assert.match(text, /WASM_API_UNAVAILABLE/);
      else if (mode === 'compile-failed') assert.match(text, /WASM_INIT_FAILED, CompileError.*test unsupported module instruction/);
      else assert.equal(await page.locator('#app .auth.err').count(), 0, text);
      if (mode === 'normal' || mode === 'online-before-open') {
        const advance = page.locator('.salon-create .auth-actions .btn:not(.btn-ghost)');
        await page.locator('#create-name').fill('recovery');
        await advance.click();
        await page.locator('#create-password').fill('local recovery test password');
        await page.locator('#create-confirm').fill('local recovery test password');
        await advance.click();
        // A local snapshot request proves the real worker started bootstrap.
        const bootstrap = page.context().waitForEvent('request', {
          predicate: req => new URL(req.url()).pathname === '/test-snapshot', timeout: 20_000,
        });
        const goOnline = async () => {
          publishConfig = true;
          await page.evaluate(() => window.dispatchEvent(new Event('online')));
        };
        if (mode === 'online-before-open') await goOnline();
        await advance.click();
        await page.waitForSelector('.main');
        if (mode === 'normal') await goOnline();
        await bootstrap;
      }
      assert.doesNotMatch(text, /Lockdown Mode|WebAssembly is disabled/);
      assert.deepEqual(errors, []); assert.deepEqual(unhandled, []);
      assert.deepEqual(external, [], 'disabled hosting must not start a bridge or bootstrap download');
      console.log(`PASS ${mode}: ${mode.includes('worker') || mode.includes('failed') ? 'neutral error, settled RPC' : 'actual worker/WASM ready'}`);
    } finally { await context.close(); }
  }
} finally { await browser?.close(); await new Promise(resolve => server.close(resolve)); }
