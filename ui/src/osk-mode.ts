/**
 * Whether the app's own on-screen keyboard is in charge of text entry.
 *
 * The owner's rule (2026-11-07): on a phone the system keyboard (IME) must
 * never appear — it sees every character before encryption and may send it
 * to a cloud for suggestions. Every text field gets `inputmode="none"`, and
 * the app draws its own keyboard (osk.ts). A hardware keyboard keeps working:
 * key events do not depend on the IME.
 *
 * Kept apart from osk.ts so view.ts can mark fields without a dependency
 * cycle. The preference lives in localStorage like the theme and language:
 * it is not a secret, and it has to apply on the sign-in screens, before any
 * vault is unlocked.
 */
export type OskMode = 'auto' | 'on' | 'off';

const KEY = 'gipny.osk';

export function getOskMode(): OskMode {
  try {
    const v = localStorage.getItem(KEY);
    if (v === 'auto' || v === 'on' || v === 'off') return v;
  } catch {
    // Storage can be unavailable; fall through to the default.
  }
  return 'auto';
}

export function setOskMode(mode: OskMode): void {
  try { localStorage.setItem(KEY, mode); } catch { /* keep working in memory */ }
}

/** A device whose only pointer is a finger, or the Android app: there the
 *  system keyboard would come up on every field. */
export function phoneLike(): boolean {
  if (typeof window === 'undefined') return false;
  const android = typeof navigator !== 'undefined' && /Android/.test(navigator.userAgent ?? '');
  const tauri = '__TAURI_INTERNALS__' in window;
  if (android && tauri) return true;
  return typeof window.matchMedia === 'function' && window.matchMedia('(hover: none) and (pointer: coarse)').matches;
}

/** Cached once per page: fields are created before and after sign-in, and
 *  the answer must be the same for all of them. */
let active: boolean | null = null;
export function oskActive(): boolean {
  if (active === null) {
    const mode = getOskMode();
    active = mode === 'on' || (mode === 'auto' && phoneLike());
  }
  return active;
}

/** Test seam: the page-wide decision is recomputed on the next call. */
export function resetOskActive(): void { active = null; }

/** The input types that take text — and so would bring up the IME. */
const TEXT_TYPES = new Set(['text', 'password', 'search', 'email', 'url', 'tel', 'number', '']);

export function isTextLikeType(tag: string, type: string | null | undefined): boolean {
  if (tag === 'textarea') return true;
  return tag === 'input' && TEXT_TYPES.has((type ?? '').toLowerCase());
}

export function isTextField(el: unknown): el is HTMLInputElement | HTMLTextAreaElement {
  if (!el || typeof el !== 'object' || !('tagName' in el)) return false;
  const node = el as HTMLElement;
  const tag = node.tagName.toLowerCase();
  if (tag === 'textarea') return !(node as HTMLTextAreaElement).readOnly && !(node as HTMLTextAreaElement).disabled;
  if (tag !== 'input') return false;
  const input = node as HTMLInputElement;
  return isTextLikeType('input', input.getAttribute('type')) && !input.readOnly && !input.disabled;
}
