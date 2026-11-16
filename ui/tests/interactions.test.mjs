// Real computed styles and input handling, shared by native/browser UI. The
// layout matrix covers geometry; these checks prove visible effect policies.
import assert from 'node:assert/strict';
import { createServer } from 'vite';
const engines = await import(process.env.GIPNY_PLAYWRIGHT ?? 'playwright');
const engine = process.env.GIPNY_UI_ENGINE ?? 'chromium';
const server = await createServer({ logLevel: 'error', server: { host: '127.0.0.1', port: 0, strictPort: false } });
await server.listen();
const base = `http://127.0.0.1:${server.httpServer.address().port}`;
let browser;
try {
  browser = await engines[engine].launch({ headless: true,
    ...(engine === 'chromium' && process.env.GIPNY_CHROMIUM ? { executablePath: process.env.GIPNY_CHROMIUM } : {}) });
  const page = await browser.newPage({ viewport: { width: 360, height: 780 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'hardwareConcurrency', { value: 8, configurable: true });
    Object.defineProperty(navigator, 'deviceMemory', { value: 8, configurable: true });
    Object.defineProperty(navigator, 'connection', { value: undefined, configurable: true });
  });
  for (const theme of ['light', 'dark']) {
    await page.goto(`${base}/dev/frame.html?scene=list&theme=${theme}&lang=en`);
    await page.waitForSelector('[data-preview-ready="list"]');
    const me = page.locator('.sidebar-me');
    assert.equal(await me.locator('.sidebar-me-name + .sidebar-me-qr svg').count(), 1);
    const styles = () => me.evaluate(el => ({ color: getComputedStyle(el).backgroundColor,
      transition: getComputedStyle(el).transitionDuration, outline: getComputedStyle(el).outlineStyle }));
    const idle = await styles();
    await me.hover(); await page.waitForTimeout(200);
    assert.notEqual((await styles()).color, idle.color, `${theme}: real hover changes background`);
    await page.mouse.move(359, 779); await page.keyboard.press('Tab'); await me.focus();
    await page.waitForTimeout(200);
    const focused = await styles();
    assert.notEqual(focused.color, idle.color, `${theme}: keyboard focus changes background`);
    assert.notEqual(focused.outline, 'none'); assert.match(focused.transition, /0\.16s/);
    await me.click(); await page.waitForSelector('.modal-backdrop .card-qr');
    const filter = () => page.locator('.modal-backdrop').evaluate(el => {
      const css = getComputedStyle(el); return css.backdropFilter || css.webkitBackdropFilter;
    });
    assert.equal(await filter(), 'blur(2px)', `${theme}: light modal backdrop ${JSON.stringify(await page.evaluate(() => ({
      classes: document.documentElement.className, blur: getComputedStyle(document.documentElement).getPropertyValue('--modal-blur'),
      motion: matchMedia('(prefers-reduced-motion: reduce)').matches, transparency: matchMedia('(prefers-reduced-transparency: reduce)').matches,
      slow: matchMedia('(update: slow)').matches,
    })))}`);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    assert.equal(await filter(), 'none');
    assert.match((await styles()).transition, /^0s(?:, 0s)*$/);
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await page.evaluate(() => document.documentElement.classList.add('reduced-effects'));
    assert.equal(await filter(), 'none');
    await page.locator('.modal-header .icon-btn').click();
    assert.equal(await page.locator('.modal-backdrop').count(), 0, 'modal actions remain usable');
    await page.evaluate(() => document.documentElement.classList.remove('reduced-effects'));
  }
  assert.deepEqual(errors, []);
  console.log(`${engine}: light/dark hover, keyboard focus, QR action, blur, reduced-motion/resource fallback PASS`);
} finally { await browser?.close(); await server.close(); }
