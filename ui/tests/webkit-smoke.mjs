// The built browser client in WebKit, the engine behind Safari and every iOS
// browser. Playwright's Linux WebKit runs the whole flow — module worker, WASM,
// OPFS sync access handles, Web Locks — so this catches what Chrome cannot:
// a WebKit-only API gap or a message that leaves a phone user guessing.
//
// The page is served the way GitHub Pages serves it today: no
// browser-config.json. A profile is created, the chats must open without a
// network service, and the main view must say that this hosting has none.
// The profile must then still be there after a reload (OPFS persisted).
//
// Not part of `npm test`: it needs the browser (`npx playwright install
// --with-deps webkit`). CI runs it in the wasm job; locally:
//   node tests/webkit-smoke.mjs [--dist browser-dist] [--out <dir>]
// The storage lives in a persistent context on purpose: WebKit refuses OPFS
// in an ephemeral one (navigator.storage.getDirectory() rejects).
import { createServer } from 'node:http';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { extname, join, resolve } from 'node:path';
import { webkit } from 'playwright';

const args = Object.fromEntries(process.argv.slice(2).map((a, i, all) => a.startsWith('--') ? [a.slice(2), all[i + 1]] : []).filter(Boolean));
const dist = resolve(args.dist ?? new URL('../browser-dist/', import.meta.url).pathname);
const out = args.out ? resolve(args.out) : null;
if (out) mkdirSync(out, { recursive: true });
statSync(join(dist, 'browser.html'));

const MIME = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.css': 'text/css', '.wasm': 'application/wasm', '.json': 'application/json', '.webmanifest': 'application/manifest+json', '.svg': 'image/svg+xml', '.png': 'image/png', '.webp': 'image/webp', '.wav': 'audio/wav' };
const server = createServer((req, res) => {
  const path = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  const file = resolve(dist, '.' + path);
  // Pages without the operator's variable: the client must cope with a 404 here.
  if (path === '/browser-config.json' || !file.startsWith(dist)) { res.writeHead(404); res.end(); return; }
  let body;
  try { body = readFileSync(file); } catch { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { 'Content-Type': MIME[extname(file)] ?? 'application/octet-stream', 'Content-Length': body.length, 'Cache-Control': 'no-store' });
  res.end(body);
});
await new Promise(r => server.listen(0, '127.0.0.1', r));
const url = `http://127.0.0.1:${server.address().port}/browser.html`;

const log = [];
const say = (line) => { log.push(line); console.log(line); };
const udd = mkdtempSync(join(tmpdir(), 'gipny-webkit-'));
const context = await webkit.launchPersistentContext(udd, { headless: true, viewport: { width: 390, height: 844 } });
const page = await context.newPage();
const pageErrors = [];
page.on('pageerror', e => { pageErrors.push(e.message); say(`  [pageerror] ${e.message}`); });
page.on('console', m => { const t = m.text(); if (/gipny-browser|error/i.test(t)) say(`  [console.${m.type()}] ${t.slice(0, 240)}`); });
say(`webkit ${context.browser()?.version() ?? '?'} · ${url}`);

const snapshot = () => page.evaluate(() => {
  const app = document.getElementById('app');
  const banner = document.querySelector('.network-ambient');
  return {
    view: app?.firstElementChild?.className ?? null,
    main: !!app?.querySelector('.main'),
    text: (document.body.innerText || '').replace(/\s+/g, ' ').trim(),
    inputs: [...document.querySelectorAll('input')].map(i => i.type || 'text'),
    steps: [...document.querySelectorAll('.boot-step')].map(e => e.className.replace('boot-step ', '') + ': ' + e.innerText.replace(/\s+/g, ' ').trim()),
    banner: banner ? { className: banner.className, text: banner.innerText.replace(/\s+/g, ' ').trim() } : null,
    errors: [...document.querySelectorAll('.err')].map(e => e.textContent.trim()).filter(Boolean),
  };
});
const until = async (what, predicate, ms) => {
  const started = Date.now();
  let last = '';
  for (;;) {
    const st = await snapshot();
    const brief = JSON.stringify({ v: st.view, s: st.steps, b: st.banner, e: st.errors });
    if (brief !== last) { last = brief; say(`[${((Date.now() - started) / 1000).toFixed(1)}s] view=${st.view} · ${st.text.slice(0, 160)}`); for (const s of st.steps) say(`    ${s}`); if (st.banner) say(`    banner ${st.banner.className}: ${st.banner.text}`); if (st.errors.length) say(`    form error: ${st.errors.join(' | ')}`); }
    if (predicate(st)) return st;
    if (Date.now() - started > ms) throw new Error(`${what}: not reached in ${ms / 1000} s; last view=${st.view}, text=${st.text.slice(0, 300)}`);
    await new Promise(r => setTimeout(r, 500));
  }
};

const PROFILE = 'probe', PASS = 'correct horse battery staple 42';
let code = 0;
try {
  await page.goto(url, { waitUntil: 'load', timeout: 30_000 });
  await until('create form', st => st.inputs.filter(t => t === 'password').length >= 2, 30_000);
  await page.evaluate(([name, pass]) => {
    const inputs = [...document.querySelectorAll('input')];
    const set = (el, v) => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, v); el.dispatchEvent(new Event('input', { bubbles: true })); };
    for (const t of inputs.filter(i => (i.type || 'text') === 'text')) set(t, name);
    const [a, b] = inputs.filter(i => i.type === 'password'); set(a, pass); set(b, pass);
    [...document.querySelectorAll('button')].find(b => /create|созда/i.test(b.innerText)).click();
  }, [PROFILE, PASS]);
  const created = Date.now();
  // Nothing is on its way without a network service: the chats open at once.
  const main = await until('main view', st => st.main, 90_000);
  say(`main view ${((Date.now() - created) / 1000).toFixed(1)} s after Create`);
  if (!main.banner || !/is-offline/.test(main.banner.className) || !/no network service on this hosting/i.test(main.banner.text)) throw new Error(`main view banner does not name the missing network service: ${JSON.stringify(main.banner)}`);
  if (/is-retrying/.test(main.banner.className)) throw new Error('banner promises a retry that nothing arms');
  // The profile survives a reload: OPFS and the vault slots persisted in WebKit.
  await page.reload({ waitUntil: 'load' });
  const list = await until('profile list', st => st.text.includes(PROFILE) && !/\bauth err\b/.test(st.view ?? ''), 30_000);
  if (list.errors.length) throw new Error(`profile list shows an error: ${list.errors.join(' | ')}`);
  if (pageErrors.length) throw new Error(`uncaught page errors: ${pageErrors.join(' | ')}`);
  say('webkit-smoke: profile created, chats opened offline with the hosting notice, profile listed after reload');
} catch (error) {
  code = 1;
  say(`webkit-smoke: FAILED — ${error.message}`);
} finally {
  if (out) writeFileSync(join(out, 'webkit-smoke.log'), log.join('\n') + '\n');
  await context.close().catch(() => {});
  rmSync(udd, { recursive: true, force: true });
  server.close();
}
process.exit(code);
