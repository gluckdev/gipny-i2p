import assert from 'node:assert/strict';
import { createServer } from 'vite';
import { chromium } from 'playwright';
const server = await createServer({ logLevel: 'error', server: { host: '127.0.0.1', port: 0 } });
await server.listen();
let browser;
try {
  browser = await chromium.launch({ headless: true, executablePath: process.env.GIPNY_CHROMIUM || '/usr/bin/google-chrome' });
  const context = await browser.newContext({ hasTouch: true });
  const page = await context.newPage(), errors = [];
  page.on('pageerror', error => errors.push(error.message));
  let checked = 0;
  for (const viewport of [{ width: 360, height: 780 }, { width: 780, height: 360 }]) for (const lang of ['en', 'ru', 'uk', 'de', 'vi', 'hi', 'km', 'zh']) for (const scene of ['chat', 'auth']) {
    await page.setViewportSize(viewport);
    await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/dev/frame.html?scene=${scene}&theme=dark&lang=${lang}&osk=1`);
    await page.waitForSelector(`[data-preview-ready="${scene}"]`);
    const field = page.locator(scene === 'chat' ? '.chat-input textarea' : '#create-name');
    await field.focus(); await page.waitForSelector('.osk:not([hidden])');
    for (let attempt = 0; await page.locator('.osk').getAttribute('data-layout') !== lang && attempt < 8; attempt++) await page.locator('[data-k="layout"]').click();
    assert.equal(await page.locator('.osk').getAttribute('data-layout'), lang);
    const numbers = page.locator('.osk-number-row .osk-key');
    assert.deepEqual(await numbers.allTextContents(), [...'1234567890']);
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const geometry = await page.evaluate(() => {
      const keyboard = document.querySelector('.osk').getBoundingClientRect(), field = document.activeElement.getBoundingClientRect();
      return { keyboard: { top: keyboard.top, bottom: keyboard.bottom }, field: { top: field.top, bottom: field.bottom },
        overflow: document.documentElement.scrollWidth - innerWidth,
        keys: [...document.querySelectorAll('.osk-number-row button')].map(button => {
          const r = button.getBoundingClientRect(); return { x: r.x, right: r.right, width: r.width, height: r.height,
            hit: button.contains(document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2)) };
        }) };
    });
    assert.ok(geometry.overflow <= 1, JSON.stringify(geometry));
    assert.ok(geometry.keyboard.top >= 0 && geometry.keyboard.bottom <= viewport.height + 1);
    assert.ok(geometry.field.top >= -1 && geometry.field.bottom <= geometry.keyboard.top + 1, `${lang}/${scene}: ${JSON.stringify(geometry)}`);
    assert.ok(geometry.keys.every(key => key.x >= 0 && key.right <= viewport.width + 1 && key.width >= 24 && key.height >= (viewport.height > 500 ? 42 : 28) && key.hit), JSON.stringify(geometry));
    const before = await field.inputValue(); await numbers.first().click(); assert.equal(await field.inputValue(), before + '1');
    await page.locator('[data-k="shift"]').click(); const symbol = await numbers.first().textContent(); assert.notEqual(symbol, '1');
    if (lang === 'en' && scene === 'chat') await page.screenshot({ path: `/tmp/gipny-beta-keyboard-${viewport.width}.png` });
    await numbers.first().click(); assert.equal(await field.inputValue(), before + '1' + symbol);
    assert.deepEqual(await numbers.allTextContents(), [...'1234567890']);
    await page.locator('[data-k="symbols"]').click(); assert.equal(await page.locator('.osk').getAttribute('data-page'), 'symbols');
    assert.equal(await page.locator('.osk-number-row').count(), 0);
    await page.locator('[data-k="letters"]').click(); assert.equal(await numbers.count(), 10);
    const styling = await page.evaluate((currentLang) => {
      const qKey = document.querySelector('.osk-number-row button') || document.querySelector('.osk-key');
      const wKey = document.querySelector('.osk-key[data-c="w"]');
      const parseRgb = (c) => {
        const m = c && c.match(/rgb\((\d+),\s*(\d+),\s*(\d+)\)/);
        return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
      };
      const lum = (rgb) => rgb ? rgb[0] * 0.299 + rgb[1] * 0.587 + rgb[2] * 0.114 : 0;
      return {
        qBgLum: lum(parseRgb(getComputedStyle(qKey).backgroundColor)),
        qColorLum: lum(parseRgb(getComputedStyle(qKey).color)),
        hasW: Boolean(wKey),
        wBgLum: wKey ? lum(parseRgb(getComputedStyle(wKey).backgroundColor)) : 0,
        wColorLum: wKey ? lum(parseRgb(getComputedStyle(wKey).color)) : 0,
        wHasClass: wKey ? wKey.classList.contains('osk-wasd') : false,
      };
    }, lang);
    assert.ok(styling.qBgLum < 100, `key background is dark (${styling.qBgLum})`);
    assert.ok(styling.qColorLum > 180, `key text is light (${styling.qColorLum})`);
    if (lang === 'en') {
      assert.ok(styling.hasW && styling.wHasClass, 'en layout has osk-wasd class on w');
      assert.ok(styling.wBgLum > 180, `en WASD key background is light (${styling.wBgLum})`);
      assert.ok(styling.wColorLum < 100, `en WASD key text is dark (${styling.wColorLum})`);
    } else if (styling.hasW) {
      assert.equal(styling.wHasClass, false, `${lang} layout must NOT have osk-wasd on w`);
      assert.ok(styling.wBgLum < 100, `${lang} w key background is dark (${styling.wBgLum})`);
      assert.ok(styling.wColorLum > 180, `${lang} w key text is light (${styling.wColorLum})`);
    }
    checked++;
  }

  // Explicit check across light and dark themes: keyboard keys remain dark, WASD inverted only in en
  for (const theme of ['light', 'dark']) for (const testLang of ['en', 'de', 'ru']) {
    await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/dev/frame.html?scene=chat&theme=${theme}&lang=${testLang}&osk=1`);
    await page.waitForSelector('[data-preview-ready="chat"]');
    const f = page.locator('.chat-input textarea');
    await f.focus();
    await page.waitForSelector('.osk:not([hidden])');
    for (let attempt = 0; await page.locator('.osk').getAttribute('data-layout') !== testLang && attempt < 8; attempt++) await page.locator('[data-k="layout"]').click();
    assert.equal(await page.locator('.osk').getAttribute('data-layout'), testLang);
    const themeStyling = await page.evaluate(() => {
      const normalKey = document.querySelector('.osk-number-row button');
      const wKey = document.querySelector('.osk-key[data-c="w"]');
      const parseRgb = (c) => {
        const m = c && c.match(/rgb\((\d+),\s*(\d+),\s*(\d+)\)/);
        return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
      };
      const lum = (rgb) => rgb ? rgb[0] * 0.299 + rgb[1] * 0.587 + rgb[2] * 0.114 : 0;
      return {
        normalBgLum: lum(parseRgb(getComputedStyle(normalKey).backgroundColor)),
        normalColorLum: lum(parseRgb(getComputedStyle(normalKey).color)),
        hasW: Boolean(wKey),
        wBgLum: wKey ? lum(parseRgb(getComputedStyle(wKey).backgroundColor)) : 0,
        wColorLum: wKey ? lum(parseRgb(getComputedStyle(wKey).color)) : 0,
        wHasClass: wKey ? wKey.classList.contains('osk-wasd') : false,
      };
    });
    assert.ok(themeStyling.normalBgLum < 100, `${theme}/${testLang}: normal key has dark background`);
    assert.ok(themeStyling.normalColorLum > 180, `${theme}/${testLang}: normal key has light text`);
    if (testLang === 'en') {
      assert.ok(themeStyling.wHasClass && themeStyling.wBgLum > 180 && themeStyling.wColorLum < 100,
        `${theme}/en: WASD has inverted light key and dark text`);
    } else if (themeStyling.hasW) {
      assert.ok(!themeStyling.wHasClass && themeStyling.wBgLum < 100 && themeStyling.wColorLum > 180,
        `${theme}/${testLang}: w key is not light`);
    }
  }
  assert.deepEqual(errors, []);
  console.log(`keyboard-layout: ${checked} language/scene/portrait-landscape cases passed; digits, Shift, real clicks, targets, focus and viewport bounds`);
} finally { await browser?.close(); await server.close(); }
