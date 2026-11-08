/**
 * The app's own on-screen keyboard.
 *
 * On a phone the system keyboard never appears (every text field carries
 * `inputmode="none"`, see osk-mode.ts and `h()` in view.ts); this panel takes
 * its place. It sits fixed at the bottom of the window, comes up when a text
 * field gains focus and goes away when focus leaves text fields. Tapping it
 * never moves focus: `pointerdown` is cancelled, keys act on `click`.
 *
 * What it changes in the field goes through osk-edit.ts and ends in an
 * `input` event, so drafts, the composer's auto-height and the typing
 * indicator behave exactly as with a system keyboard. Its height is published
 * as `--kbd-offset`, the variable the layout already shrinks by when a system
 * keyboard is up (main.ts feeds the visualViewport side of that).
 *
 * Plan and the slices still to come: docs/plans/2026-11-08-own-keyboard.md.
 */
import { h } from './view';
import { t as tr, getLang, onLangChange } from './i18n';
import { isTextField, oskActive, isTextLikeType } from './osk-mode';
import { LAYOUTS, SYMBOL_PAGES, DIGIT_ROWS, EMOJI, shifted, isLayoutId } from './osk-layouts';
import type { LayoutId } from './osk-layouts';
import { insertText, replaceBeforeCaret, deleteBackward } from './osk-edit';
import type { TextState } from './osk-edit';
import { telexCompose } from './osk-telex';

type Page = 'letters' | 'symbols' | 'symbols2' | 'emoji' | 'digits';
type Shift = 'off' | 'once' | 'caps';
type Field = HTMLInputElement | HTMLTextAreaElement;

const EXTRA_LAYOUTS_KEY = 'gipny.osk.layouts';
const REPEAT_DELAY_MS = 400;
const REPEAT_EVERY_MS = 60;
const DOUBLE_TAP_MS = 350;

/** The layouts the layout key cycles through: the UI language, English,
 *  then whatever was added by hand (`gipny.osk.layouts`, comma-separated;
 *  the contact's language joins here in the next slice). */
export function enabledLayouts(): LayoutId[] {
  const list: LayoutId[] = [];
  const add = (id: string): void => { if (isLayoutId(id) && list.indexOf(id) === -1) list.push(id); };
  add(getLang());
  add('en');
  try {
    for (const id of (localStorage.getItem(EXTRA_LAYOUTS_KEY) ?? '').split(',')) add(id.trim());
  } catch {
    // Storage can be unavailable; the two defaults stay.
  }
  return list;
}

// --kbd-offset is owned here: the larger of what the viewport says (a system
// keyboard, on a device where one still appears) and this panel.
let viewportOffset = 0;
export function setViewportOffset(px: number): void {
  viewportOffset = Math.max(0, px);
  applyKbdOffset();
}
export function applyKbdOffset(): void {
  if (typeof document === 'undefined') return;
  const px = Math.max(viewportOffset, osk?.height() ?? 0);
  document.body.style.setProperty('--kbd-offset', px + 'px');
  if (px > 0) {
    const log = document.querySelector<HTMLElement>('.chat-log');
    if (log) requestAnimationFrame(() => { log.scrollTop = log.scrollHeight; });
  }
}

export class Osk {
  readonly el: HTMLElement;
  private target: Field | null = null;
  private layouts: LayoutId[] = enabledLayouts();
  private layoutIndex = 0;
  private page: Page = 'letters';
  private shift: Shift = 'off';
  private lastShiftTap = 0;
  private repeatTimer: ReturnType<typeof setTimeout> | null = null;
  private repeatKey: HTMLElement | null = null;
  private backspaceByPointer = false;
  private hideTimer: ReturnType<typeof setTimeout> | null = null;
  private shown = false;
  private readonly rows: HTMLElement;

  constructor() {
    this.rows = h('div', { class: 'osk-rows' });
    this.el = h('div', { class: 'osk', role: 'group', 'aria-label': tr('osk.keyboard') }, this.rows);
    this.el.hidden = true;
    // Keep focus in the field and the text unselected: cancel the press, let click through.
    this.el.addEventListener('pointerdown', (e) => this.onPointerDown(e));
    this.el.addEventListener('mousedown', (e) => e.preventDefault());
    this.el.addEventListener('click', (e) => this.onClick(e));
    for (const type of ['pointerup', 'pointercancel', 'pointerleave']) {
      this.el.addEventListener(type, () => this.stopRepeat());
    }
    // A finger that slides off Backspace onto a neighbour stops the repeat.
    this.el.addEventListener('pointermove', (e) => { if (this.repeatTimer && this.keyOf(e) !== this.repeatKey) this.stopRepeat(); });
    this.el.addEventListener('contextmenu', (e) => e.preventDefault());
    onLangChange(() => {
      this.el.setAttribute('aria-label', tr('osk.keyboard'));
      this.layouts = enabledLayouts();
      this.layoutIndex = 0;
      if (this.shown) this.render();
    });
  }

  get visible(): boolean { return this.shown; }
  get field(): Field | null { return this.target; }
  /** What the layout key shows right now. */
  get layout(): LayoutId { return this.layouts[this.layoutIndex] ?? 'en'; }
  get currentPage(): Page { return this.page; }

  height(): number { return this.shown ? this.el.offsetHeight : 0; }

  show(field: Field): void {
    if (this.hideTimer) { clearTimeout(this.hideTimer); this.hideTimer = null; }
    if (field !== this.target) {
      this.target = field;
      this.page = wantsDigits(field) ? 'digits' : 'letters';
      this.shift = 'off';
    }
    if (!this.shown) {
      this.shown = true;
      this.el.hidden = false;
      document.documentElement.setAttribute('data-osk', 'shown');
    }
    this.render();
    applyKbdOffset();
    // The field may sit where the panel now is; bring it up above the keys.
    if (typeof field.scrollIntoView === 'function') {
      requestAnimationFrame(() => { try { field.scrollIntoView({ block: 'center' }); } catch { field.scrollIntoView(); } });
    }
  }

  /** After focus leaves a field: hide unless another field took it. */
  scheduleHide(): void {
    if (this.hideTimer) clearTimeout(this.hideTimer);
    this.hideTimer = setTimeout(() => {
      this.hideTimer = null;
      const active = document.activeElement;
      if (!isTextField(active) && !this.el.contains(active)) this.hide();
    }, 0);
  }

  hide(): void {
    if (this.hideTimer) { clearTimeout(this.hideTimer); this.hideTimer = null; }
    this.stopRepeat();
    this.target = null;
    if (!this.shown) return;
    this.shown = false;
    this.el.hidden = true;
    document.documentElement.removeAttribute('data-osk');
    applyKbdOffset();
  }

  // --- keys -----------------------------------------------------------------

  private keyOf(e: Event): HTMLElement | null {
    const node = e.target as HTMLElement | null;
    return node && typeof node.closest === 'function' ? node.closest<HTMLElement>('.osk-key') : null;
  }

  private onPointerDown(e: Event): void {
    e.preventDefault();
    const key = this.keyOf(e);
    if (key?.dataset.k === 'backspace') {
      this.backspaceByPointer = true;
      this.repeatKey = key;
      this.backspace();
      this.stopRepeat();
      this.repeatTimer = setTimeout(() => {
        const tick = (): void => { this.backspace(); this.repeatTimer = setTimeout(tick, REPEAT_EVERY_MS); };
        tick();
      }, REPEAT_DELAY_MS);
    }
  }

  private stopRepeat(): void {
    if (this.repeatTimer) { clearTimeout(this.repeatTimer); this.repeatTimer = null; }
    this.repeatKey = null;
  }

  private onClick(e: Event): void {
    const key = this.keyOf(e);
    if (!key) return;
    e.preventDefault();
    // A touch tap can still hand focus to the key itself (Chromium 83 did,
    // despite the cancelled pointerdown): give it back to the field first, or
    // the focus bounce re-shows the panel with its state reset.
    if (this.target && document.activeElement !== this.target) this.target.focus();
    const c = key.dataset.c;
    if (c !== undefined) { this.type(c); return; }
    switch (key.dataset.k) {
      case 'backspace':
        // A pointer press already deleted (and repeats while held); a click
        // without one comes from a keyboard or assistive technology.
        if (!this.backspaceByPointer) this.backspace();
        this.backspaceByPointer = false;
        return;
      case 'shift': this.toggleShift(); return;
      case 'space': this.type(' '); return;
      case 'enter': this.enter(); return;
      case 'symbols': this.page = 'symbols'; this.render(); return;
      case 'symbols2': this.page = 'symbols2'; this.render(); return;
      case 'letters': this.page = 'letters'; this.render(); return;
      case 'emoji': this.page = 'emoji'; this.render(); return;
      case 'layout':
        this.layoutIndex = (this.layoutIndex + 1) % Math.max(1, this.layouts.length);
        this.page = 'letters';
        this.render();
        return;
      case 'hide':
        this.target?.blur();
        this.hide();
        return;
      default: return;
    }
  }

  private toggleShift(): void {
    const now = Date.now();
    if (this.shift === 'off') this.shift = 'once';
    else if (this.shift === 'once') this.shift = now - this.lastShiftTap < DOUBLE_TAP_MS ? 'caps' : 'off';
    else this.shift = 'off';
    this.lastShiftTap = now;
    this.render();
  }

  private state(): TextState | null {
    const f = this.target;
    if (!f) return null;
    const start = f.selectionStart ?? f.value.length;
    const end = f.selectionEnd ?? start;
    return { value: f.value, start, end };
  }

  private apply(next: TextState, inputType: string, data: string | null): void {
    const f = this.target;
    if (!f || f.readOnly || f.disabled) return;
    // maxlength: what a physical keyboard could not type, this one does not either.
    if (f.maxLength >= 0 && next.value.length > f.maxLength && next.value.length > f.value.length) return;
    if (document.activeElement !== f) f.focus();
    f.value = next.value;
    try { f.setSelectionRange(next.start, next.end); } catch { /* number fields have no selection API */ }
    let event: Event;
    try {
      // A password never travels in a bubbling event's payload.
      event = new InputEvent('input', { bubbles: true, inputType, data: f.type === 'password' ? null : data });
    } catch {
      event = new Event('input', { bubbles: true });
    }
    f.dispatchEvent(event);
  }

  private type(text: string): void {
    const s = this.state();
    if (!s) return;
    const composed = this.page === 'letters' && LAYOUTS[this.layout].compose === 'telex'
      ? telexCompose(s.value.slice(0, s.start), text) : null;
    const next = composed ? replaceBeforeCaret(s, composed.replace, composed.text) : insertText(s, text);
    this.apply(next, 'insertText', composed?.text ?? text);
    if (this.shift === 'once') { this.shift = 'off'; this.render(); }
  }

  private backspace(): void {
    const s = this.state();
    if (!s) return;
    this.apply(deleteBackward(s), 'deleteContentBackward', null);
  }

  private enter(): void {
    const f = this.target;
    if (!f) return;
    if (f.tagName.toLowerCase() === 'textarea') {
      const s = this.state();
      if (s) this.apply(insertText(s, '\n'), 'insertLineBreak', null);
      return;
    }
    // Forms listen for Enter on their fields; give them the key they expect.
    for (const type of ['keydown', 'keyup']) {
      f.dispatchEvent(new KeyboardEvent(type, { key: 'Enter', code: 'Enter', bubbles: true, cancelable: true }));
    }
  }

  // --- drawing --------------------------------------------------------------

  private render(): void {
    const up = this.shift !== 'off';
    const layout = LAYOUTS[this.layout];
    const fn = (label: string, k: string, cls: string, aria: string, extra?: Record<string, unknown>): HTMLElement =>
      h('button', { type: 'button', class: 'osk-key osk-fn ' + cls, 'data-k': k, 'aria-label': aria, ...extra }, label);
    const ch = (c: string): HTMLElement => h('button', { type: 'button', class: 'osk-key', 'data-c': c }, c);
    const row = (...keys: HTMLElement[]): HTMLElement => h('div', { class: 'osk-row' }, ...keys);
    const backspace = (): HTMLElement => fn('⌫', 'backspace', 'osk-wide', tr('osk.backspace'));
    const rows: HTMLElement[] = [];

    if (this.page === 'letters') {
      layout.rows.forEach((letters, i) => {
        const keys = letters.map((c) => ch(up ? shifted(layout, c) : c));
        if (i === layout.rows.length - 1) {
          const shiftCls = 'osk-wide osk-shift' + (this.shift === 'caps' ? ' osk-caps' : up ? ' osk-on' : '');
          keys.unshift(fn(this.shift === 'caps' ? '⇪' : '⇧', 'shift', shiftCls, tr('osk.shift'), { 'aria-pressed': String(up) }));
          keys.push(backspace());
        }
        rows.push(row(...keys));
      });
    } else if (this.page === 'symbols' || this.page === 'symbols2') {
      const page = SYMBOL_PAGES[this.page === 'symbols' ? 0 : 1] ?? SYMBOL_PAGES[0]!;
      page.forEach((symbols, i) => {
        const keys = symbols.map(ch);
        if (i === page.length - 1) {
          const other = this.page === 'symbols' ? 'symbols2' : 'symbols';
          keys.unshift(fn(this.page === 'symbols' ? '=\\<' : '?123', other, 'osk-wide', tr('osk.symbols')));
          keys.push(backspace());
        }
        rows.push(row(...keys));
      });
    } else if (this.page === 'digits') {
      DIGIT_ROWS.forEach((digits, i) => {
        const keys = digits.map(ch);
        keys.push(i === 0 ? backspace() : i === DIGIT_ROWS.length - 1 ? fn('↵', 'enter', 'osk-wide', tr('osk.enter')) : h('span', { class: 'osk-key osk-blank', 'aria-hidden': 'true' }));
        rows.push(row(...keys));
      });
    } else {
      rows.push(h('div', { class: 'osk-row osk-emoji' }, ...EMOJI.map(ch)));
    }

    // The bottom row: pages, layout, space, enter, hide. A number field has
    // its own enter and no letters to switch to.
    if (this.page !== 'digits') {
      const letters = this.page !== 'letters';
      rows.push(row(
        letters ? fn('ABC', 'letters', 'osk-wide', tr('osk.letters')) : fn('?123', 'symbols', 'osk-wide', tr('osk.symbols')),
        fn(layout.label, 'layout', 'osk-layout', tr('osk.layout', { name: layout.label }), { title: tr('osk.layout', { name: layout.label }) }),
        this.page === 'emoji' ? fn('?123', 'symbols', '', tr('osk.symbols')) : fn('☺', 'emoji', '', tr('osk.emoji')),
        fn('', 'space', 'osk-space', tr('osk.space')),
        fn('↵', 'enter', 'osk-wide', tr('osk.enter')),
        fn('⌄', 'hide', '', tr('osk.hide')),
      ));
    } else {
      rows.push(row(fn('⌄', 'hide', 'osk-wide', tr('osk.hide'))));
    }

    this.rows.replaceChildren(...rows);
    this.el.setAttribute('data-page', this.page);
    // The emoji page is taller than the letter rows: the layout follows the panel.
    if (this.shown) applyKbdOffset();
    this.el.setAttribute('data-layout', this.layout);
  }
}

/** The one panel, once installOsk() decided the device needs it. */
export let osk: Osk | null = null;

function markField(el: Element): void {
  const tag = el.tagName.toLowerCase();
  if (!isTextLikeType(tag, el.getAttribute('type'))) return;
  if (el.getAttribute('inputmode') === 'numeric') (el as HTMLElement).dataset.oskPage = 'digits';
  el.setAttribute('inputmode', 'none');
}

function wantsDigits(field: Field): boolean {
  return field.type === 'number' || field.dataset.oskPage === 'digits';
}

/**
 * Mounts the keyboard when this device needs one, and keeps every text field
 * — including ones that do not come from `h()` — from calling the system
 * keyboard. Returns null on a desktop, where nothing changes.
 */
export function installOsk(): Osk | null {
  if (osk || typeof document === 'undefined' || !oskActive()) return osk;
  osk = new Osk();
  document.body.appendChild(osk.el);
  // Mark on focus too: a field that bypassed h() and arrived in the same
  // task as its focus() call would otherwise meet the IME before the observer.
  document.addEventListener('focusin', (e) => { if (isTextField(e.target)) { markField(e.target); osk!.show(e.target); } });
  document.addEventListener('focusout', (e) => { if (isTextField(e.target)) osk!.scheduleHide(); });
  for (const el of document.querySelectorAll('input, textarea')) markField(el);
  if (typeof MutationObserver !== 'undefined') {
    new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) {
          if (node.nodeType !== 1) continue;
          const el = node as Element;
          if (el.matches('input, textarea')) markField(el);
          for (const inner of el.querySelectorAll('input, textarea')) markField(inner);
        }
      }
    }).observe(document.documentElement, { childList: true, subtree: true });
  }
  return osk;
}

/** Test seam: forget the mounted panel so a test can install again. */
export function uninstallOsk(): void {
  osk?.hide();
  osk?.el.remove();
  osk = null;
}
