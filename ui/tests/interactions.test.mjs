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
  if (process.env.GIPNY_LAYOUT_TIMEOUT_MS) page.setDefaultTimeout(Number(process.env.GIPNY_LAYOUT_TIMEOUT_MS));
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
  await page.setViewportSize({ width: 320, height: 740 });
  await page.goto(`${base}/dev/frame.html?scene=list&theme=light&lang=en&demo=1`);
  await page.waitForSelector('[data-preview-ready="list"]');
  const groups = page.getByRole('button', { name: /^Groups \d/ });
  await groups.focus();
  await page.keyboard.press('Space');
  assert.equal(await groups.getAttribute('aria-expanded'), 'false');
  assert.equal(await groups.evaluate(el => el === document.activeElement), true, 'folding retains keyboard focus');
  await page.keyboard.press('Enter');
  assert.equal(await groups.getAttribute('aria-expanded'), 'true');

  const search = page.getByRole('searchbox');
  await search.fill('no such contact');
  assert.equal(await page.locator('.list-empty [class]').innerText(), 'Nothing found');
  await search.fill('');
  const contact = page.getByRole('button', { name: /^Anna\b/ });
  await contact.focus();
  await page.keyboard.press('Enter');
  const title = page.getByRole('heading', { name: 'Anna' });
  assert.equal(await title.evaluate(el => el === document.activeElement), true);
  assert.ok((await page.locator('.chat-input textarea').boundingBox()).width >= 200, 'composer fits a line of text at 320px');
  const send = page.getByRole('button', { name: 'Send', exact: true });
  assert.equal(await send.isDisabled(), true);
  await page.locator('.chat-input textarea').fill('Draft');
  assert.equal(await send.isEnabled(), true);
  await page.locator('.chat-input textarea').fill('');
  assert.equal(await send.isDisabled(), true);
  const status = await page.locator('.network-ambient').boundingBox();
  if (status) assert.ok(status.y + status.height <= (await page.locator('.chat-header').boundingBox()).y + 1);
  await page.keyboard.press('Escape');
  assert.equal(await contact.evaluate(el => el === document.activeElement), true);

  const newChat = page.locator('.sidebar-actions .icon-btn-accent');
  await newChat.focus();
  await page.keyboard.press('Enter');
  await page.keyboard.press('ArrowDown');
  assert.equal(await page.getByRole('menuitem').nth(1).evaluate(el => el === document.activeElement), true);
  await page.keyboard.press('Escape');
  assert.equal(await newChat.evaluate(el => el === document.activeElement), true);

  const settings = page.getByRole('button', { name: 'Settings', exact: true });
  await settings.focus();
  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  assert.equal(await dialog.evaluate(el => el === document.activeElement), true);
  await page.keyboard.press('Shift+Tab');
  assert.equal(await dialog.locator('.modal-footer button').evaluate(el => el === document.activeElement), true);
  await page.keyboard.press('Tab');
  assert.equal(await dialog.locator('.modal-header button').evaluate(el => el === document.activeElement), true);
  await page.keyboard.press('Escape');
  assert.equal(await settings.evaluate(el => el === document.activeElement), true);
  assert.deepEqual(errors, []);
  console.log(`${engine}: theme and motion preferences, 320px composer, keyboard chat navigation, menus and dialog focus passed`);
} finally { await browser?.close(); await server.close(); }
