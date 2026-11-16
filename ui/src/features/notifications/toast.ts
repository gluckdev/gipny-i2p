import { h } from '../../app/view';

export type ToastMessage = { text: string; err: boolean };

/** Allow time to read the complete message. The layer never steals focus or
 * pointer input, including when a long diagnostic needs more time. */
export function toastLifetime(text: string): number {
  return Math.max(5500, 2000 + Array.from(text).length * 55);
}

export class ToastLayer {
  readonly el = h('div', { class: 'toast-layer', role: 'status', 'aria-live': 'polite', 'aria-atomic': 'true' });
  constructor() { document.body.appendChild(this.el); }
  show(message: ToastMessage | null): void {
    this.el.replaceChildren();
    if (!message) return;
    this.el.setAttribute('aria-live', message.err ? 'assertive' : 'polite');
    // One centered card replaces the previous notice; no fixed-height stack
    // can cover another message or put its final lines outside a small phone.
    this.el.appendChild(h('div', { class: message.err ? 'toast err' : 'toast' }, message.text));
  }
  destroy(): void { this.el.replaceChildren(); this.el.remove(); }
}
