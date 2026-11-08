// Real layout regression matrix. Run with `node tests/layout.test.mjs` after
// installing Playwright's Chromium; DOM-only tests cannot measure overflow.
import assert from 'node:assert/strict';
import { createServer } from 'vite';
import { mkdir, writeFile } from 'node:fs/promises';
const { chromium } = await import(process.env.GIPNY_PLAYWRIGHT ?? 'playwright');
const server = await createServer({ logLevel: 'error', server: { host: '127.0.0.1', port: 0, strictPort: false } });
await server.listen();
const base = `http://127.0.0.1:${server.httpServer.address().port}`;
const browser = await chromium.launch({ headless: true, ...(process.env.GIPNY_CHROMIUM ? { executablePath: process.env.GIPNY_CHROMIUM } : {}) });
const sizes = [{ width: 360, height: 740 }, { width: 412, height: 892 }, { width: 768, height: 1024 }, { width: 1100, height: 720 }];
const scenes = ['list', 'chat', 'console', 'agent', 'settings', 'identity', 'add-contact', 'auth', 'select', 'unlock', 'boot'];
const artifacts = process.env.GIPNY_LAYOUT_ARTIFACTS;
if (artifacts) await mkdir(artifacts, { recursive: true });
const results = [];
const jobs = [];
for (const theme of ['light', 'dark']) for (const osk of [false, true]) for (const scene of scenes) for (const viewport of sizes) jobs.push({ scene, theme, osk, viewport, lang: 'en' });
for (const lang of ['ru', 'uk', 'de', 'km', 'vi', 'hi', 'zh']) for (const theme of ['light', 'dark']) for (const osk of [false, true]) for (const scene of ['auth', 'select', 'unlock', 'boot', 'settings']) jobs.push({ scene, theme, osk, viewport: sizes[0], lang });
function measure() {
  const root = document.documentElement, vw = root.clientWidth;
  const offenders = [];
  for (const el of document.body.querySelectorAll('*')) {
    const rect = el.getBoundingClientRect();
    if (rect.width === 0 || (rect.right <= vw + 1 && rect.left >= -1)) continue;
    let clipped = false;
    for (let parent = el.parentElement; parent && parent !== document.body; parent = parent.parentElement) {
      if (getComputedStyle(parent).overflowX !== 'visible') { clipped = true; break; }
    }
    if (!clipped) offenders.push(`${el.tagName}.${el.className}: ${rect.left}..${rect.right}`);
  }
  const card = document.querySelector('.auth-card');
  let need = null;
  if (card) {
    const css = getComputedStyle(card.parentElement);
    const keyboard = parseFloat(document.body.style.getPropertyValue('--kbd-offset')) || 0;
    need = Math.round(card.getBoundingClientRect().height + parseFloat(css.paddingTop) + parseFloat(css.paddingBottom) - keyboard);
  }
  const field = document.activeElement;
  const keyboard = document.querySelector('.osk:not([hidden])');
  const rect = field?.getBoundingClientRect();
  return { overflow: Math.max(root.scrollWidth, document.body.scrollWidth) - vw, offenders, need,
    clear: !keyboard || (rect && rect.top >= -1 && rect.bottom <= keyboard.getBoundingClientRect().top + 1),
    theme: getComputedStyle(root).getPropertyValue('--bg').trim(),
  };
}
async function run() {
  const context = await browser.newContext();
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    while (jobs.length) {
      const job = jobs.shift();
      const { scene, theme, osk, viewport, lang } = job;
      const label = `${scene}-${theme}-${lang}-${viewport.width}x${viewport.height}${osk ? '-osk' : ''}`;
      errors.length = 0;
      await page.setViewportSize(viewport);
      const params = new URLSearchParams({ scene, theme, lang, ...(osk ? { osk: '1' } : {}) });
      await page.goto(`${base}/dev/frame.html?${params}`);
      if (scene === 'boot') await page.waitForSelector('.boot-card');
      else await page.waitForSelector(`[data-preview-ready="${scene}"]`);
      const field = page.locator('.chat-input textarea, input.input:not([type="hidden"]), textarea').filter({ visible: true }).first();
      if (osk && await field.count()) {
        await field.focus();
        await page.waitForSelector('.osk:not([hidden])');
      }
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      await page.waitForTimeout(80);
      const result = await page.evaluate(measure);
      if (scene === 'auth') {
        await page.locator('.auth-security summary').click();
        const expanded = await page.evaluate(measure);
        result.expandedNeed = expanded.need;

      }
      results.push({ label, ...result, errors: [...errors] });
      const failed = result.overflow > 1 || result.offenders.length || (result.need && result.need > viewport.height) || (result.expandedNeed && result.expandedNeed > viewport.height) || !result.clear || errors.length;
      if (artifacts && (failed || (['auth', 'boot', 'console', 'settings'].includes(scene) && viewport.width === 360 && !osk && lang === 'en'))) {
        await page.screenshot({ path: `${artifacts}/${label}.png` });
      }
      if (failed) console.error(label, JSON.stringify(result), errors);
      if (results.length % 40 === 0) console.log(`layout: checked ${results.length} combinations`);
    }
  } finally { await context.close(); }
}
try {
  await Promise.all(Array.from({ length: 4 }, run));
  // The outer preview forwards update modes to every frame; exercise the
  // real IPC facade as well as the DOM-only event tests.
  const page = await browser.newPage({ viewport: sizes[0] });
  for (const mode of ['current', 'no_release', 'available', 'unavailable', 'unsupported', 'no_asset', 'dismissed', 'error', 'staged']) {
    await page.goto(`${base}/dev/frame.html?scene=settings&theme=dark&lang=en&update=${mode}`);
    await page.waitForSelector('[data-preview-ready="settings"]');
    const check = page.locator('[data-check-update]');
    await check.click();
    await page.waitForFunction(() => !document.querySelector('[data-check-update]').disabled);
    const status = await page.locator('[data-update-status]').textContent();
    const expected = { current: /Up to date/, no_release: /No release/, available: /0\.5\.1/, unavailable: /unavailable/, unsupported: /does not support/, no_asset: /no download/, dismissed: /dismissed/, staged: /0\.5\.1/ };
    if (mode === 'error') assert.match(await page.locator('[data-update-section]').textContent(), /could not be reached/);
    else assert.match(status, expected[mode], mode);
    if (mode === 'available') {
      const auto = page.locator('[data-auto-update]');
      assert.equal(await auto.isChecked(), true);
      await auto.uncheck(); await page.waitForFunction(() => !document.querySelector('[data-auto-update]').disabled);
      assert.equal(await auto.isChecked(), false);
      await page.locator('[data-install-update]').click();
      await page.waitForSelector('[data-settings-restart]:not(.hidden)');
      await page.locator('[data-settings-restart]').click();
      await page.waitForURL(/update=current/);
      await page.waitForSelector('[data-preview-ready="settings"]');
      assert.match(await page.locator('.modal .card-block').first().textContent(), /0\.5\.1/);
      assert.equal(await page.locator('.update-banner').evaluate(el => el.classList.contains('hidden')), true);
    }
  }
  // System theme must resolve to precisely the explicit palettes.
  for (const theme of ['light', 'dark']) {
    await page.emulateMedia({ colorScheme: theme });
    await page.goto(`${base}/dev/frame.html?scene=auth&theme=system`);
    await page.waitForSelector('[data-preview-ready="auth"]');
    assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()), theme === 'dark' ? '#10151d' : '#f3f5f8');
  }
  // Test preview.html's own card/OSK and overflow report, including forwarding.
  await page.goto(`${base}/dev/preview.html?scene=auth&theme=dark&osk=1`);
  await page.waitForFunction(() => [...document.querySelectorAll('iframe')].every(frame => frame.contentDocument?.querySelector('.auth-card')));
  const preview = await page.evaluate(() => window.checkOverflow());
  assert.equal(preview.bad, false, preview.lines.join('\n'));
  await page.goto(`${base}/dev/preview.html?scene=settings&update=no_release`);
  await page.waitForSelector('iframe');
  assert.ok(await page.locator('iframe').evaluateAll(frames => frames.every(frame => frame.src.includes('update=no_release'))));
  await page.close();
  if (artifacts) await writeFile(`${artifacts}/report.json`, JSON.stringify(results, null, 2));
  const failed = results.filter(r => r.overflow > 1 || r.offenders.length || (r.need && r.need > Number(r.label.match(/x(\d+)/)[1])) || (r.expandedNeed && r.expandedNeed > Number(r.label.match(/x(\d+)/)[1])) || !r.clear || r.errors.length);
  assert.equal(failed.length, 0, `${failed.length}/${results.length} layouts failed`);
  console.log(`layout: ${results.length} scene/theme/viewport/keyboard combinations passed; no horizontal overflow, auth cards fit, focused fields clear the keyboard`);
} finally { await browser.close(); await server.close(); }
