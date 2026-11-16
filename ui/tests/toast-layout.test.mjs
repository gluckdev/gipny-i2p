import assert from 'node:assert/strict';
import { createServer } from 'vite';
import { chromium } from 'playwright';
import { join, parse } from 'node:path';

const server = await createServer({ logLevel: 'error', server: { host: '127.0.0.1', port: 0 } });
await server.listen();
let browser;
try {
  browser = await chromium.launch({ headless: true, executablePath: process.env.GIPNY_CHROMIUM || '/usr/bin/google-chrome' });
  const page = await browser.newPage({ viewport: { width: 360, height: 780 } });
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/dev/frame.html?scene=chat&theme=dark&lang=ru&osk=1`);
  await page.waitForSelector('[data-preview-ready="chat"]');
  await page.locator('.chat-input textarea').focus();
  await page.waitForSelector('.osk:not([hidden])');
  const text = 'Не удалось загрузить модуль.\nWASM_INIT_FAILED, CompileError: проверочная многострочная ошибка.\n\n' +
    'Это демонстрационный текст для узкого экрана. Сообщение переносится полностью и не закрывает доступ к полю ввода. ' +
    'Под уведомлением можно нажимать кнопки и продолжать печатать.\n\nПоследняя строка должна оставаться видимой.';
  await page.evaluate(async text => {
    const { ToastLayer } = await import('/src/features/notifications/toast.ts');
    document.querySelectorAll('.toast-layer').forEach(el => el.remove());
    document.body.style.setProperty('--safe-top', '20px'); document.body.style.setProperty('--safe-bottom', '12px');
    const layer = new ToastLayer(); layer.show({ text, err: true });
    window.testToast = layer;
  }, text);
  await page.waitForTimeout(250);
  const measured = await page.evaluate(() => {
    const el = document.querySelector('.toast'), layer = el.parentElement;
    const r = el.getBoundingClientRect(), area = layer.getBoundingClientRect();
    const css = getComputedStyle(el), layerCss = getComputedStyle(layer);
    const keyboard = document.querySelector('.osk:not([hidden])').getBoundingClientRect();
    return { text: el.textContent, left: r.left, right: r.right, top: r.top, bottom: r.bottom, center: (r.top + r.bottom) / 2,
      target: (area.top + area.bottom + parseFloat(layerCss.paddingTop) - parseFloat(layerCss.paddingBottom)) / 2,
      areaTop: area.top + parseFloat(layerCss.paddingTop), areaBottom: area.bottom - parseFloat(layerCss.paddingBottom), keyboard: keyboard.top,
      pointer: css.pointerEvents, parentPointer: layerCss.pointerEvents, background: css.backgroundColor,
      clipped: el.scrollHeight > el.clientHeight, focus: document.activeElement.tagName };
  });
  assert.equal(measured.text, text); assert.equal(measured.clipped, false);
  assert.ok(measured.left >= 12 && measured.right <= 348, JSON.stringify(measured));
  assert.ok(measured.top >= measured.areaTop && measured.bottom <= measured.areaBottom && measured.bottom <= measured.keyboard, JSON.stringify(measured));
  assert.ok(Math.abs(measured.center - measured.target) < 2);
  assert.equal(measured.pointer, 'none'); assert.equal(measured.parentPointer, 'none');
  assert.match(measured.background, /rgba\(.+, 0\.88\)/); assert.equal(measured.focus, 'TEXTAREA');
  const screenshot = process.env.GIPNY_TOAST_SCREENSHOT || '/tmp/gipny-toast-android-keyboard.png';
  await page.screenshot({ path: screenshot });
  const longText = 'WASM_INIT_FAILED: unsupported instruction and detailed error. '.repeat(20) + '\nПоследняя строка длинной ошибки.';
  await page.evaluate(text => {
    window.testToastHeight = document.body.style.getPropertyValue('--app-height');
    document.body.style.setProperty('--app-height', '430px');
    window.testToast.show({ text, err: true });
  }, longText);
  await page.waitForSelector('.toast-scrollable');
  await page.waitForTimeout(250);
  const longState = await page.locator('.toast').evaluate(el => {
    const r = el.getBoundingClientRect();
    return { text: el.textContent, top: r.top, bottom: r.bottom, height: r.height,
      scrollHeight: el.scrollHeight, pointer: getComputedStyle(el).pointerEvents, focused: document.activeElement.tagName };
  });
  assert.equal(longState.text, longText); assert.equal(longState.pointer, 'auto');
  assert.ok(longState.top >= 32 && longState.bottom <= 406 && longState.scrollHeight > longState.height, JSON.stringify(longState));
  assert.equal(longState.focused, 'TEXTAREA', 'long notice must not steal focus');
  await page.locator('.toast').hover(); await page.mouse.wheel(0, 10000);
  await page.waitForFunction(() => { const el = document.querySelector('.toast'); return el.scrollTop + el.clientHeight >= el.scrollHeight - 2; });
  assert.ok(await page.locator('.toast').evaluate(el => {
    const node = el.firstChild, length = node.textContent.length;
    const range = document.createRange(); range.setStart(node, length - 'Последняя строка длинной ошибки.'.length); range.setEnd(node, length);
    const end = range.getBoundingClientRect(), card = el.getBoundingClientRect();
    return end.top >= card.top && end.bottom <= card.bottom;
  }), 'the final line must be reachable and visible by ordinary scrolling');
  const screenshotPath = parse(screenshot);
  const longScreenshot = join(screenshotPath.dir, `${screenshotPath.name}-long${screenshotPath.ext || '.png'}`);
  await page.screenshot({ path: longScreenshot });
  await page.evaluate(text => {
    document.body.style.setProperty('--app-height', window.testToastHeight);
    window.testToast.show({ text, err: true });
  }, text);
  await page.waitForFunction(() => !document.querySelector('.toast').classList.contains('toast-scrollable'));
  await page.evaluate(() => {
    const target = document.createElement('button'); target.id = 'under-toast'; target.textContent = 'click through';
    const rect = document.querySelector('.toast').getBoundingClientRect();
    Object.assign(target.style, { position: 'fixed', zIndex: '100', left: '145px', top: `${rect.top + 20}px`, width: '70px', height: '30px' });
    target.onclick = () => { target.dataset.clicked = 'yes'; }; document.body.appendChild(target);
  });
  await page.locator('#under-toast').click(); assert.equal(await page.locator('#under-toast').getAttribute('data-clicked'), 'yes');
  await page.evaluate(() => {
    const modal = document.createElement('div'); modal.className = 'modal-backdrop'; modal.id = 'confirmation';
    modal.innerHTML = '<div class="modal"><button id="confirm-choice">Подтвердить выбор</button></div>';
    document.body.appendChild(modal);
    document.getElementById('confirm-choice').onclick = event => { event.target.dataset.clicked = 'yes'; };
  });
  assert.ok(await page.locator('#confirm-choice').evaluate(el => {
    const r = el.getBoundingClientRect();
    return document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2) === el &&
      Number(getComputedStyle(document.getElementById('confirmation')).zIndex) > Number(getComputedStyle(document.querySelector('.toast-layer')).zIndex);
  }));
  await page.locator('#confirm-choice').click(); assert.equal(await page.locator('#confirm-choice').getAttribute('data-clicked'), 'yes');
  console.log(`PASS centered multiline/1000+ character toast, keyboard/safe area, overflow scroll, ordinary click-through, modal controls: ${screenshot}`);
} finally { await browser?.close(); await server.close(); }
