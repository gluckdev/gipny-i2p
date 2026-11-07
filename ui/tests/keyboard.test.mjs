import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { build } from 'vite';
import { Window } from 'happy-dom';

// The app's own on-screen keyboard (ui/src/osk*.ts): layouts, the editing
// arithmetic, and the panel driving a real field in happy-dom. Built with the
// same compiler as production, like the other UI tests.
const dir = await mkdtemp(join(tmpdir(), 'gipny-osk-'));
await build({ configFile: false, logLevel: 'silent', build: {
  outDir: dir, emptyOutDir: false, rolldownOptions: { output: { banner: 'const Event = window.Event;' } }, lib: { entry: 'tests/entry.ts', formats: ['es'], fileName: () => 'entry.mjs' },
} });
const win = new Window({ url: 'http://localhost/' });
for (const key of ['window', 'document', 'localStorage', 'navigator', 'Node', 'Element', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement',
  'KeyboardEvent', 'InputEvent', 'FocusEvent', 'MouseEvent', 'MutationObserver', 'File', 'requestAnimationFrame', 'getComputedStyle', 'ResizeObserver']) {
  if (win[key] !== undefined) Object.defineProperty(globalThis, key, { value: key === 'window' ? win : win[key], configurable: true });
}
win.__TAURI_INTERNALS__ = {};
const ui = await import(pathToFileURL(join(dir, 'entry.mjs')).href);
const { LAYOUTS, LAYOUT_IDS, SYMBOL_PAGES, DIGIT_ROWS, EMOJI, shifted, isLayoutId,
  insertText, deleteBackward, moveCaret, graphemeStart,
  isTextLikeType, isTextField, oskActive, resetOskActive, getOskMode, setOskMode,
  installOsk, uninstallOsk, enabledLayouts, setViewportOffset, h, setLang, translations } = ui;

after(async () => { await rm(dir, { recursive: true, force: true }); });

const tick = () => new Promise((resolve) => setTimeout(resolve, 5));
const fire = (el, type, init = {}) => el.dispatchEvent(new win.Event(type, { bubbles: true, cancelable: true, ...init }));
const keyButton = (panel, selector) => {
  const el = panel.el.querySelector(selector);
  assert.ok(el, `key ${selector} is drawn`);
  return el;
};
const press = (panel, selector) => { const k = keyButton(panel, selector); fire(k, 'pointerdown'); fire(k, 'pointerup'); fire(k, 'click'); };
const charKey = (c) => `.osk-key[data-c="${c}"]`;
const fnKey = (k) => `.osk-key[data-k="${k}"]`;

test('every layout has three unique rows and a shifted form for each key', () => {
  assert.deepEqual(Object.keys(LAYOUTS).sort(), [...LAYOUT_IDS].sort());
  for (const layout of Object.values(LAYOUTS)) {
    assert.equal(layout.rows.length, 3, layout.id);
    const keys = layout.rows.flat();
    assert.equal(new Set(keys).size, keys.length, `${layout.id} has no duplicate keys`);
    for (const key of keys) {
      assert.equal([...key].length, 1, `${layout.id}: ${key} is one character`);
      assert.equal([...shifted(layout, key)].length, 1, `${layout.id}: shift(${key}) is one character`);
    }
  }
  assert.equal(shifted(LAYOUTS.en, 'q'), 'Q');
  assert.equal(shifted(LAYOUTS.ru, 'ё'), 'Ё');
  assert.equal(shifted(LAYOUTS.de, 'ß'), 'ß', 'ß does not become SS');
  assert.ok(LAYOUTS.ru.rows.flat().includes('ё') && LAYOUTS.ru.rows.flat().includes('ъ'));
  assert.ok(['ä', 'ö', 'ü', 'ß'].every((c) => LAYOUTS.de.rows.flat().includes(c)));
  assert.ok(isLayoutId('ru') && !isLayoutId('xx'));
  for (const page of SYMBOL_PAGES) {
    const keys = page.flat();
    assert.equal(new Set(keys).size, keys.length, 'symbols are unique within a page');
  }
  assert.deepEqual(DIGIT_ROWS.flat().filter((c) => /\d/.test(c)).sort(), [...'0123456789']);
  assert.equal(new Set(EMOJI).size, EMOJI.length, 'emoji are unique');
});

test('insert, backspace and caret moves respect graphemes and selections', () => {
  assert.deepEqual(insertText({ value: 'ab', start: 1, end: 1 }, 'X'), { value: 'aXb', start: 2, end: 2 });
  assert.deepEqual(insertText({ value: 'hello', start: 1, end: 4 }, '-'), { value: 'h-o', start: 2, end: 2 });
  assert.deepEqual(insertText({ value: 'ab', start: 9, end: 9 }, 'c'), { value: 'abc', start: 3, end: 3 }, 'an out-of-range caret clamps');
  assert.deepEqual(deleteBackward({ value: 'abc', start: 3, end: 3 }), { value: 'ab', start: 2, end: 2 });
  assert.deepEqual(deleteBackward({ value: 'abc', start: 0, end: 0 }), { value: 'abc', start: 0, end: 0 });
  assert.deepEqual(deleteBackward({ value: 'abcd', start: 1, end: 3 }), { value: 'ad', start: 1, end: 1 }, 'a selection goes as a whole');
  assert.deepEqual(deleteBackward({ value: 'a😀', start: 3, end: 3 }), { value: 'a', start: 1, end: 1 }, 'a surrogate pair goes together');
  assert.deepEqual(deleteBackward({ value: 'a✌️', start: 3, end: 3 }), { value: 'a', start: 1, end: 1 }, 'a variation selector goes with its base');
  assert.deepEqual(deleteBackward({ value: 'a👍🏽', start: 5, end: 5 }), { value: 'a', start: 1, end: 1 }, 'a skin tone goes with its base');
  const family = 'x👨‍👩‍👧';
  assert.deepEqual(deleteBackward({ value: family, start: family.length, end: family.length }), { value: 'x', start: 1, end: 1 }, 'a ZWJ sequence goes as one');
  assert.deepEqual(deleteBackward({ value: 'é!', start: 2, end: 2 }), { value: '!', start: 0, end: 0 }, 'a combining mark goes with its letter');
  assert.equal(graphemeStart('ab', 0), 0);
  assert.deepEqual(moveCaret({ value: 'a😀b', start: 3, end: 3 }, -1), { value: 'a😀b', start: 1, end: 1 });
  assert.deepEqual(moveCaret({ value: 'a😀b', start: 1, end: 1 }, 1), { value: 'a😀b', start: 3, end: 3 });
  assert.deepEqual(moveCaret({ value: 'abc', start: 1, end: 2 }, 1), { value: 'abc', start: 2, end: 2 }, 'a selection collapses to its edge');
  assert.deepEqual(moveCaret({ value: 'abc', start: 3, end: 3 }, 1), { value: 'abc', start: 3, end: 3 });
});

test('text-like fields are the ones that would bring up a system keyboard', () => {
  assert.ok(isTextLikeType('textarea', null));
  for (const type of ['text', 'password', 'search', 'email', 'url', 'tel', 'number', '', null]) assert.ok(isTextLikeType('input', type), String(type));
  for (const type of ['checkbox', 'radio', 'file', 'range', 'submit', 'button']) assert.ok(!isTextLikeType('input', type), type);
  assert.ok(!isTextLikeType('div', null));
  const input = document.createElement('input');
  assert.ok(isTextField(input));
  input.readOnly = true;
  assert.ok(!isTextField(input), 'a read-only field needs no keyboard');
  assert.ok(!isTextField(document.createElement('button')));
  assert.ok(!isTextField(null));
});

test('the keyboard is off on a desktop and on when asked or on a phone', () => {
  localStorage.removeItem('gipny.osk');
  resetOskActive();
  assert.equal(getOskMode(), 'auto');
  assert.equal(oskActive(), false, 'happy-dom has a fine pointer: no keyboard');
  const plain = h('input', { type: 'text' });
  assert.equal(plain.getAttribute('inputmode'), null, 'desktop fields keep the system keyboard');
  setOskMode('on'); resetOskActive();
  assert.equal(oskActive(), true);
  assert.equal(h('input', { class: 'input' }).getAttribute('inputmode'), 'none');
  assert.equal(h('input', { type: 'password' }).getAttribute('inputmode'), 'none');
  assert.equal(h('textarea', { rows: '1' }).getAttribute('inputmode'), 'none');
  assert.equal(h('input', { type: 'checkbox' }).getAttribute('inputmode'), null);
  const totp = h('input', { type: 'text', inputmode: 'numeric' });
  assert.equal(totp.getAttribute('inputmode'), 'none', 'a numeric hint would still be the system keyboard');
  assert.equal(totp.getAttribute('data-osk-page'), 'digits');
  setOskMode('off'); resetOskActive();
  assert.equal(oskActive(), false);
  assert.equal(h('input', {}).getAttribute('inputmode'), null);
});

test('the layout key cycles the UI language, English and hand-added layouts', () => {
  localStorage.removeItem('gipny.osk.layouts');
  setLang('ru');
  assert.deepEqual(enabledLayouts(), ['ru', 'en']);
  setLang('en');
  assert.deepEqual(enabledLayouts(), ['en']);
  localStorage.setItem('gipny.osk.layouts', 'de, xx ,ru');
  assert.deepEqual(enabledLayouts(), ['en', 'de', 'ru'], 'unknown ids are dropped, order kept');
  localStorage.removeItem('gipny.osk.layouts');
  assert.deepEqual(Object.keys(translations.en).filter((k) => k.startsWith('osk.')).length, 10);
});

test('the panel comes up for a focused field, types into it and goes away', async () => {
  setLang('en');
  setOskMode('on'); resetOskActive();
  uninstallOsk();
  const panel = installOsk();
  assert.ok(panel, 'installed when the mode is on');
  assert.equal(installOsk(), panel, 'installed once');
  assert.equal(panel.visible, false);
  assert.equal(panel.el.hidden, true);

  const input = h('input', { type: 'text' });
  const events = [];
  input.addEventListener('input', (e) => events.push(['input', e.inputType ?? null, e.data ?? null]));
  input.addEventListener('keydown', (e) => events.push(['keydown', e.key]));
  document.body.append(input);
  input.focus();
  if (!panel.visible) fire(input, 'focusin');
  assert.equal(panel.visible, true, 'a text field brings the keyboard up');
  assert.equal(document.documentElement.getAttribute('data-osk'), 'shown');
  assert.equal(panel.layout, 'en');
  assert.equal(panel.currentPage, 'letters');
  assert.match(document.body.style.getPropertyValue('--kbd-offset'), /px$/, 'the layout learns the panel height');

  press(panel, charKey('q'));
  press(panel, charKey('w'));
  assert.equal(input.value, 'qw');
  assert.deepEqual(events.slice(0, 2), [['input', 'insertText', 'q'], ['input', 'insertText', 'w']]);
  assert.equal(document.activeElement, input, 'focus stays in the field');

  press(panel, fnKey('shift'));
  assert.ok(keyButton(panel, charKey('E')), 'shift draws capitals');
  press(panel, charKey('E'));
  assert.equal(input.value, 'qwE');
  assert.ok(keyButton(panel, charKey('r')), 'one-shot shift releases after a letter');

  press(panel, fnKey('shift')); press(panel, fnKey('shift'));
  assert.ok(keyButton(panel, charKey('R')), 'a double tap is caps lock');
  press(panel, charKey('R')); press(panel, charKey('T'));
  assert.equal(input.value, 'qwERT');
  assert.ok(keyButton(panel, charKey('Y')), 'caps stays on');
  press(panel, fnKey('shift'));
  assert.ok(keyButton(panel, charKey('y')));

  press(panel, fnKey('space'));
  assert.equal(input.value, 'qwERT ');
  const backspace = keyButton(panel, fnKey('backspace'));
  fire(backspace, 'pointerdown'); fire(backspace, 'pointerup'); fire(backspace, 'click');
  assert.equal(input.value, 'qwERT', 'backspace acts once on press, click adds nothing');
  assert.equal(events.filter((e) => e[1] === 'deleteContentBackward').length, 1);

  input.setSelectionRange(0, 2);
  press(panel, charKey('x'));
  assert.equal(input.value, 'xERT', 'a selection is replaced');
  assert.equal(input.selectionStart, 1, 'the caret follows the inserted text');
  input.setSelectionRange(4, 4);

  press(panel, fnKey('symbols'));
  assert.equal(panel.currentPage, 'symbols');
  press(panel, charKey('1'));
  press(panel, fnKey('symbols2'));
  assert.equal(panel.currentPage, 'symbols2');
  press(panel, charKey('€'));
  press(panel, fnKey('letters'));
  assert.equal(panel.currentPage, 'letters');
  press(panel, fnKey('emoji'));
  assert.equal(panel.currentPage, 'emoji');
  press(panel, charKey('😀'));
  assert.equal(input.value, 'xERT1€😀');
  press(panel, fnKey('letters'));

  localStorage.setItem('gipny.osk.layouts', 'ru');
  setLang('ru'); setLang('en'); // enabledLayouts is re-read on a language change
  press(panel, fnKey('layout'));
  assert.equal(panel.layout, 'ru');
  press(panel, charKey('й'));
  assert.equal(input.value, 'xERT1€😀й');
  press(panel, fnKey('layout'));
  assert.equal(panel.layout, 'en', 'the cycle wraps');
  localStorage.removeItem('gipny.osk.layouts');

  press(panel, fnKey('enter'));
  assert.deepEqual(events.filter((e) => e[0] === 'keydown'), [['keydown', 'Enter']], 'an input gets the Enter key forms listen for');
  assert.equal(input.value, 'xERT1€😀й', 'Enter adds no newline to an input');

  const area = h('textarea', {});
  document.body.append(area);
  area.focus();
  if (panel.field !== area) fire(area, 'focusin');
  assert.equal(panel.field, area, 'the panel follows focus to the next field');
  press(panel, charKey('a'));
  press(panel, fnKey('enter'));
  press(panel, charKey('b'));
  assert.equal(area.value, 'a\nb', 'Enter is a newline in a textarea');

  const number = h('input', { type: 'number' });
  document.body.append(number);
  number.focus();
  if (panel.field !== number) fire(number, 'focusin');
  assert.equal(panel.currentPage, 'digits', 'a number field opens the digit page');
  press(panel, charKey('4')); press(panel, charKey('2'));
  assert.equal(number.value, '42');
  assert.equal(panel.el.querySelector(fnKey('layout')), null, 'no letters to switch to');

  number.blur();
  fire(number, 'focusout');
  await tick();
  assert.equal(panel.visible, false, 'focus left text fields: the panel hides');
  assert.equal(document.documentElement.getAttribute('data-osk'), null);
  assert.equal(document.body.style.getPropertyValue('--kbd-offset'), '0px');

  input.focus(); if (!panel.visible) fire(input, 'focusin');
  assert.equal(panel.visible, true);
  press(panel, fnKey('hide'));
  assert.equal(panel.visible, false, 'the hide key closes the panel');

  setViewportOffset(120);
  assert.equal(document.body.style.getPropertyValue('--kbd-offset'), '120px', 'a system keyboard still reports through the same variable');
  setViewportOffset(0);

  uninstallOsk();
  input.remove(); area.remove(); number.remove();
  setOskMode('auto'); resetOskActive();
});

test('fields the panel did not create are marked when they enter the document', async () => {
  setOskMode('on'); resetOskActive();
  uninstallOsk();
  installOsk();
  const raw = document.createElement('input');
  raw.type = 'text';
  const box = document.createElement('div');
  box.append(raw);
  document.body.append(box);
  await tick();
  assert.equal(raw.getAttribute('inputmode'), 'none');
  box.remove();
  uninstallOsk();
  setOskMode('auto'); resetOskActive();
});
