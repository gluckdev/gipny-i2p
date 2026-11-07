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
  outDir: dir, emptyOutDir: false, rolldownOptions: { output: { banner: 'const Event = window.Event;' } },
  lib: { entry: { entry: 'tests/entry.ts', telex: 'src/osk-telex.ts' }, formats: ['es'], fileName: (_, name) => `${name}.mjs` },
} });
const win = new Window({ url: 'http://localhost/' });
for (const key of ['window', 'document', 'localStorage', 'navigator', 'Node', 'Element', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement',
  'KeyboardEvent', 'InputEvent', 'FocusEvent', 'MouseEvent', 'MutationObserver', 'File', 'requestAnimationFrame', 'getComputedStyle', 'ResizeObserver']) {
  if (win[key] !== undefined) Object.defineProperty(globalThis, key, { value: key === 'window' ? win : win[key], configurable: true });
}
win.__TAURI_INTERNALS__ = {};
const ui = await import(pathToFileURL(join(dir, 'entry.mjs')).href);
const { telexCompose } = await import(pathToFileURL(join(dir, 'telex.mjs')).href);
const { LAYOUTS, LAYOUT_IDS, SYMBOL_PAGES, DIGIT_ROWS, EMOJI, shifted, isLayoutId,
  insertText, replaceBeforeCaret, deleteBackward, moveCaret, graphemeStart,
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
      // Khmer keys may carry a vowel with its sign (two code points); any
      // other layout is one character per key on both levels.
      const limit = layout.id === 'km' ? 2 : 1;
      assert.ok([...key].length >= 1 && [...key].length <= limit, `${layout.id}: ${key} is at most ${limit} code point(s)`);
      assert.ok([...shifted(layout, key)].length >= 1 && [...shifted(layout, key)].length <= limit, `${layout.id}: shift(${key}) is at most ${limit} code point(s)`);
    }
  }
  assert.equal(shifted(LAYOUTS.en, 'q'), 'Q');
  assert.equal(shifted(LAYOUTS.ru, 'ё'), 'Ё');
  assert.equal(shifted(LAYOUTS.de, 'ß'), 'ß', 'ß does not become SS');
  assert.ok(LAYOUTS.ru.rows.flat().includes('ё') && LAYOUTS.ru.rows.flat().includes('ъ'));
  assert.ok(['ä', 'ö', 'ü', 'ß'].every((c) => LAYOUTS.de.rows.flat().includes(c)));
  assert.ok(isLayoutId('ru') && isLayoutId('vi') && isLayoutId('km') && !isLayoutId('xx'));
  assert.deepEqual(LAYOUTS.vi.rows.map((row) => row.join('')), ['qwertyuiop', 'asdfghjkl', 'zxcvbnm']);
  assert.equal(LAYOUTS.vi.label, 'VI');
  assert.equal(LAYOUTS.vi.compose, 'telex');
  assert.equal(shifted(LAYOUTS.vi, 'a'), 'A');
  for (const page of SYMBOL_PAGES) {
    const keys = page.flat();
    assert.equal(new Set(keys).size, keys.length, 'symbols are unique within a page');
  }
  assert.deepEqual(DIGIT_ROWS.flat().filter((c) => /\d/.test(c)).sort(), [...'0123456789']);
  assert.equal(new Set(EMOJI).size, EMOJI.length, 'emoji are unique');
});

function typeTelex(keys, before = '') {
  for (const key of keys) {
    const composed = telexCompose(before, key);
    before = composed ? before.slice(0, before.length - composed.replace) + composed.text : before + key;
  }
  return before;
}

test('Telex composes Vietnamese letters and tones', () => {
  for (const [keys, expected] of [
    ['vieetj', 'việt'], ['ddawng', 'đăng'], ['tieengs', 'tiếng'], ['tiengs', 'tiéng'],
    ['hoaf', 'hoà'], ['xoong', 'xông'], ['quas', 'quá'], ['thuyr', 'thuỷ'],
    ['aaa', 'aa'], ['toans', 'toán'], ['toansz', 'toan'], ['VIEETJ', 'VIỆT'],
    ['aa', 'â'], ['ee', 'ê'], ['oo', 'ô'], ['aw', 'ă'], ['ow', 'ơ'], ['uw', 'ư'], ['dd', 'đ'],
    ['AA', 'Â'], ['Aa', 'Â'], ['aA', 'â'], ['Vieetj', 'Việt'],
    ['aass', 'âs'], ['ass', 'as'], ['aff', 'af'], ['arr', 'ar'], ['axx', 'ax'], ['ajj', 'aj'],
    ['eee', 'ee'], ['ooo', 'oo'], ['aww', 'aw'], ['oww', 'ow'], ['uww', 'uw'], ['ddd', 'dd'],
    ['asf', 'à'], ['aasz', 'â'], ['asa', 'ấ'], ['AAa', 'Aa'], ['ASs', 'As'],
    ['hoef', 'hoè'], ['hoanf', 'hoàn'], ['laauj', 'lậu'], ['thuwowngr', 'thưởng'],
    ['ddangw', 'đăng'], ['tieengs hoaf', 'tiếng hoà'], ['vieetj\ns', 'việt\ns'],
  ]) {
    const actual = typeTelex(keys);
    assert.equal(actual, expected, keys);
    assert.equal(actual.normalize('NFC'), actual, `${keys} is NFC`);
  }
  assert.equal(typeTelex('a', typeTelex('aa')), 'aa', 'the third a undoes â');
  assert.equal(typeTelex('z', typeTelex('toans')), 'toan', 'z removes only the tone');
  assert.deepEqual(telexCompose('prefix 😀 viêt', 'j'), { replace: 4, text: 'việt' }, 'only the trailing syllable is replaced');
  assert.equal(typeTelex('s', 'ă'), 'ắ');
  for (const [before, key] of [
    ['a', 'k'], ['a', '.'], ['a', '1'], ['a', '😀'], ['a', 'ss'], ['a', ''],
    ['', 'w'], ['tr', 'w'], ['', 's'], ['tr', 's'], ['a', 'z'],
    ['a ', 's'], ['a1', 's'], ['a!', 's'], ['a😀', 's'], ['aЖ', 's'],
    ['a\n', 's'], ['a\r', 's'], ['a\r\n', 's'], ['a\u2028', 's'], ['a\u2029', 's'],
  ]) assert.equal(telexCompose(before, key), null, `${JSON.stringify(before)} + ${JSON.stringify(key)} is literal`);
});

test('Telex emits and clears all five NFC tones for each vowel in both cases', () => {
  for (const [base, tones] of [
    ['a', 'áàảãạ'], ['ă', 'ắằẳẵặ'], ['â', 'ấầẩẫậ'], ['e', 'éèẻẽẹ'], ['ê', 'ếềểễệ'], ['i', 'íìỉĩị'],
    ['o', 'óòỏõọ'], ['ô', 'ốồổỗộ'], ['ơ', 'ớờởỡợ'], ['u', 'úùủũụ'], ['ư', 'ứừửữự'], ['y', 'ýỳỷỹỵ'],
  ]) {
    for (const upper of [false, true]) {
      const vowel = upper ? base.toUpperCase() : base;
      for (let i = 0; i < 5; i++) {
        const key = upper ? 'SFRXJ'[i] : 'sfrxj'[i];
        const expected = upper ? tones[i].toUpperCase() : tones[i];
        assert.equal(typeTelex(key, vowel), expected, `${vowel} + ${key}`);
        assert.equal(expected.normalize('NFC'), expected);
        assert.equal(typeTelex(key, expected), vowel + key, 'repeating a tone emits the literal modifier');
        assert.equal(typeTelex('z', expected), vowel, 'clearing a tone preserves the vowel shape and case');
      }
    }
  }
});

test('Khmer keys cover the NiDA layout', () => {
  const layout = LAYOUTS.km;
  assert.equal(layout.label, 'KM');
  // NiDA's ANSI Q/A/Z rows from xkeyboard-config kh(basic); four keys emit
  // two code points on a level (A, semicolon, V, comma).
  const cps = (s) => [...s].map((c) => c.codePointAt(0));
  const base = [
    [0x1786, 0x17b9, 0x17c1, 0x179a, 0x178f, 0x1799, 0x17bb, 0x17b7, 0x17c4, 0x1795, 0x17c0, 0x17aa, 0x17ae],
    [0x17b6, 0x179f, 0x178a, 0x1790, 0x1784, 0x17a0, 0x17d2, 0x1780, 0x179b, 0x17be, 0x17cb],
    [0x178b, 0x1781, 0x1785, 0x179c, 0x1794, 0x1793, 0x1798, [0x17bb, 0x17c6], 0x17d4, 0x17ca],
  ];
  const shift = [
    [0x1788, 0x17ba, 0x17c2, 0x17ac, 0x1791, 0x17bd, 0x17bc, 0x17b8, 0x17c5, 0x1797, 0x17bf, 0x17a7, 0x17ad],
    [[0x17b6, 0x17c6], 0x17c3, 0x178c, 0x1792, 0x17a2, 0x17c7, 0x1789, 0x1782, 0x17a1, [0x17c4, 0x17c7], 0x17c9],
    [0x178d, 0x1783, 0x1787, [0x17c1, 0x17c7], 0x1796, 0x178e, 0x17c6, [0x17bb, 0x17c7], 0x17d5, 0x003f],
  ];
  const one = (list) => list.map((c) => (c.length === 1 ? c[0] : c));
  assert.deepEqual(layout.rows.map((row) => one(row.map(cps))), base);
  assert.deepEqual(layout.rows.map((row) => one(row.map((c) => cps(shifted(layout, c))))), shift);
  const keys = layout.rows.flat();
  assert.deepEqual(Object.keys(layout.shift).sort(), [...keys].sort(), 'every NiDA key changes on Shift');
  const levels = [...keys, ...keys.map((c) => shifted(layout, c))];
  assert.ok(levels.every((c) => !/[a-z]/i.test(c)), 'neither level has Latin letters');
  const coverage = new Set(levels.flatMap((s) => [...s]));
  const missing = (first, last) => Array.from({ length: last - first + 1 }, (_, i) => first + i)
    .filter((cp) => !coverage.has(String.fromCodePoint(cp)));
  // All 33 modern consonants; SHA/SSO (Pali/Sanskrit) live on NiDA's AltGr level.
  assert.deepEqual(missing(0x1780, 0x17a2), [0x179d, 0x179e]);
  assert.deepEqual(missing(0x17b6, 0x17c5), [], 'all 16 dependent vowel signs');
  const independentVowels = [...'ឥឦឧឩឪឫឬឭឮឯឰឱឲឳ'];
  assert.deepEqual(independentVowels.filter((c) => coverage.has(c)), [...'ឧឪឬឭឮ'], 'the other NiDA vowels sit on the number row or AltGr — next slice');
  for (const cp of [0x17c6, 0x17c7, 0x17c9, 0x17ca, 0x17cb, 0x17d2]) {
    assert.ok(coverage.has(String.fromCodePoint(cp)), `NiDA sign U+${cp.toString(16)} is available`);
  }
  assert.deepEqual(missing(0x17c6, 0x17d1), [0x17c8, 0x17cc, 0x17cd, 0x17ce, 0x17cf, 0x17d0, 0x17d1]);
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

test('composition replaces the suffix before the caret and any selection', () => {
  assert.deepEqual(replaceBeforeCaret({ value: '😀viêt!', start: 6, end: 6 }, 4, 'việt'), { value: '😀việt!', start: 6, end: 6 });
  assert.deepEqual(replaceBeforeCaret({ value: 'viêtOLD!', start: 4, end: 7 }, 4, 'việt'), { value: 'việt!', start: 4, end: 4 });
  assert.deepEqual(replaceBeforeCaret({ value: 'â!', start: 1, end: 1 }, 1, 'aa'), { value: 'aa!', start: 2, end: 2 });
  assert.deepEqual(replaceBeforeCaret({ value: 'ab', start: 9, end: 9 }, 1, 'X'), { value: 'aX', start: 2, end: 2 });
  assert.deepEqual(replaceBeforeCaret({ value: 'ab!', start: 2, end: 2 }, 9, 'X'), { value: 'X!', start: 1, end: 1 });
  assert.deepEqual(replaceBeforeCaret({ value: 'ab!', start: 2, end: 2 }, -1, 'X'), { value: 'abX!', start: 3, end: 3 });
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
  localStorage.setItem('gipny.osk.layouts', 'vi, en, vi');
  assert.deepEqual(enabledLayouts(), ['en', 'vi'], 'Vietnamese joins the cycle without duplicates');
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

test('the Vietnamese panel composes Telex in the focused field', () => {
  setOskMode('on'); resetOskActive();
  uninstallOsk();
  const panel = installOsk();
  assert.ok(panel);
  const input = h('input', { type: 'text' });
  const events = [];
  input.addEventListener('input', (e) => events.push([e.inputType, e.data]));
  try {
    localStorage.setItem('gipny.osk.layouts', 'vi');
    setLang('en'); setLang('ru'); setLang('en');
    document.body.append(input);
    input.focus();
    if (!panel.visible) fire(input, 'focusin');
    for (let i = 0; i < LAYOUT_IDS.length && panel.layout !== 'vi'; i++) press(panel, fnKey('layout'));
    assert.equal(panel.layout, 'vi');
    assert.equal(panel.currentPage, 'letters');

    for (const key of 'vieetj') press(panel, charKey(key));
    assert.equal(input.value, 'việt');
    assert.equal(input.selectionStart, 4);
    assert.equal(input.selectionEnd, 4);
    assert.equal(document.activeElement, input);
    assert.deepEqual(events, [
      ['insertText', 'v'], ['insertText', 'i'], ['insertText', 'e'],
      ['insertText', 'viê'], ['insertText', 't'], ['insertText', 'việt'],
    ], 'each key emits one input event with the replacement text');

    input.value = '😀 viêt tail';
    input.setSelectionRange(7, 7);
    press(panel, charKey('j'));
    assert.equal(input.value, '😀 việt tail', 'composition preserves text on both sides of the syllable');
    assert.equal(input.selectionStart, 7);

    input.value = 'viêtOLD!';
    input.setSelectionRange(4, 7);
    press(panel, charKey('j'));
    assert.equal(input.value, 'việt!', 'composition also replaces the selection');
    assert.equal(input.selectionStart, 4);
    assert.equal(input.selectionEnd, 4);

    input.value = 'â!';
    input.setSelectionRange(1, 1);
    press(panel, charKey('a'));
    assert.equal(input.value, 'aa!');
    assert.equal(input.selectionStart, 2, 'undo advances the caret past the literal modifier');

    input.value = '';
    input.setSelectionRange(0, 0);
    press(panel, fnKey('shift'));
    press(panel, charKey('V'));
    for (const key of 'ieetj') press(panel, charKey(key));
    assert.equal(input.value, 'Việt', 'one-shot shift releases during composition');

    input.value = '';
    input.setSelectionRange(0, 0);
    press(panel, fnKey('shift')); press(panel, fnKey('shift'));
    for (const key of 'VIEETJ') press(panel, charKey(key));
    assert.equal(input.value, 'VIỆT', 'caps lock composes uppercase vowels');
    press(panel, fnKey('shift'));

    press(panel, fnKey('symbols'));
    press(panel, charKey('1'));
    press(panel, fnKey('emoji'));
    press(panel, charKey('😀'));
    assert.equal(input.value, 'VIỆT1😀', 'other pages insert literally');

    press(panel, fnKey('layout'));
    assert.equal(panel.layout, 'en');
    input.value = '';
    input.setSelectionRange(0, 0);
    for (const key of 'vieetj') press(panel, charKey(key));
    assert.equal(input.value, 'vieetj', 'composition is limited to the Vietnamese layout');
  } finally {
    input.blur();
    input.remove();
    localStorage.removeItem('gipny.osk.layouts');
    uninstallOsk();
    setOskMode('auto'); resetOskActive();
  }
});

test('the Khmer panel inserts a consonant and dependent vowel in order', () => {
  setOskMode('on'); resetOskActive();
  uninstallOsk();
  const panel = installOsk();
  assert.ok(panel);
  const input = h('input', { type: 'text' });
  const events = [];
  input.addEventListener('input', (e) => events.push([e.inputType, e.data]));
  document.body.append(input);
  try {
    input.focus();
    if (!panel.visible) fire(input, 'focusin');
    localStorage.setItem('gipny.osk.layouts', 'km');
    setLang('ru'); setLang('en');
    for (let i = 0; i < enabledLayouts().length && panel.layout !== 'km'; i++) press(panel, fnKey('layout'));
    assert.equal(panel.layout, 'km');
    assert.equal(panel.el.dataset.layout, 'km');
    assert.equal(panel.currentPage, 'letters');
    press(panel, charKey('ក'));
    press(panel, charKey('ិ'));
    assert.deepEqual([...input.value], ['\u1780', '\u17b7']);
    assert.deepEqual(events, [['insertText', 'ក'], ['insertText', 'ិ']]);
    assert.equal(input.selectionStart, 2);
    assert.equal(document.activeElement, input);

    press(panel, fnKey('shift'));
    press(panel, charKey('គ'));
    assert.ok(keyButton(panel, charKey('ក')), 'one-shot Shift restores the Khmer base level');
    press(panel, fnKey('shift'));
    press(panel, charKey('ី'));
    assert.deepEqual([...input.value], ['\u1780', '\u17b7', '\u1782', '\u17b8']);
    assert.equal(input.selectionStart, 4);
    press(panel, fnKey('layout'));
    assert.equal(panel.el.dataset.layout, 'en', 'the Khmer font rule stops applying when the layout changes');
  } finally {
    uninstallOsk();
    input.remove();
    localStorage.removeItem('gipny.osk.layouts');
    setLang('ru'); setLang('en');
    setOskMode('auto'); resetOskActive();
  }
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
