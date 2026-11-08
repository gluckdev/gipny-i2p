import { h, logoMark } from './view';
import { languagePicker } from './language-picker';

/** Decorative only: no images, network requests, focus targets or fake status. */
export function salonBackdrop(): HTMLElement {
  // Keep the preview/test hook while sharing the application's theme.
  return h('div', { class: 'salon-backdrop', 'aria-hidden': 'true' });
}

/** The same compact window bar on every screen before unlocking. */
export function authHeader(): HTMLElement {
  return h('header', { class: 'auth-window-bar' },
    h('div', { class: 'auth-brand' }, logoMark(), h('span', null, 'gipny'), h('span', { class: 'auth-badge' }, 'i2p')),
    languagePicker(),
  );
}
