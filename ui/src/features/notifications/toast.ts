import { h } from '../../app/view';

export type ToastMessage = { text: string; err: boolean };

/** Allow time to read the complete message. Ordinary notices pass pointer input;
 * only a card whose text exceeds the available height accepts scrolling. */
export function toastLifetime(text: string): number {
  return Math.max(5500, 2000 + Array.from(text).length * 55);
}

export class ToastLayer {
  readonly el = h('div', { class: 'toast-layer', role: 'status', 'aria-live': 'polite', 'aria-atomic': 'true' });
  private observer: ResizeObserver | null = null;
  private measure = (): void => {
    const card = this.el.firstElementChild as HTMLElement | null;
    if (!card) return;
    const overflow = card.scrollHeight > card.clientHeight + 1;
    card.classList.toggle('toast-scrollable', overflow);
    // Only overflow needs interaction. Never move focus automatically; keyboard
    // users can tab to a long notice and scroll it without losing any text.
    if (overflow) card.tabIndex = 0;
    else card.removeAttribute('tabindex');
  };
  constructor() {
    document.body.appendChild(this.el);
    if (typeof ResizeObserver !== 'undefined') {
      this.observer = new ResizeObserver(this.measure);
      this.observer.observe(this.el);
    }
    window.addEventListener('resize', this.measure);
    window.visualViewport?.addEventListener('resize', this.measure);
  }
  show(message: ToastMessage | null): void {
    if (this.el.firstElementChild) this.observer?.unobserve(this.el.firstElementChild);
    this.el.replaceChildren();
    if (!message) return;
    this.el.setAttribute('aria-live', message.err ? 'assertive' : 'polite');
    // One centered card replaces the previous notice; no fixed-height stack
    // can cover another message or put its final lines outside a small phone.
    const card = h('div', { class: message.err ? 'toast err' : 'toast' }, message.text);
    this.el.appendChild(card);
    this.observer?.observe(card);
    this.measure();
  }
  destroy(): void {
    this.observer?.disconnect(); this.observer = null;
    window.removeEventListener('resize', this.measure);
    window.visualViewport?.removeEventListener('resize', this.measure);
    this.el.replaceChildren(); this.el.remove();
  }
}
