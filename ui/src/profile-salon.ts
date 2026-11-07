import { h } from './view';

/** Decorative only: no images, network requests, focus targets or fake status. */
export function salonBackdrop(): HTMLElement {
  return h('div', { class: 'salon-backdrop', 'aria-hidden': 'true' },
    h('div', { class: 'salon-wood salon-wood-left' }),
    h('div', { class: 'salon-wood salon-wood-right' }),
    h('div', { class: 'salon-light' }),
  );
}

export function salonEmblem(): HTMLElement {
  return h('div', { class: 'salon-emblem', 'aria-hidden': 'true' },
    h('span', { class: 'salon-wing' }),
    h('span', { class: 'salon-dial' }, h('span', { class: 'salon-monogram' }, 'G')),
    h('span', { class: 'salon-wing' }),
  );
}
