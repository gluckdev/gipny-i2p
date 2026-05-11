import type { Signal } from './state';

export type Child = Node | string | number | null | undefined | false | Child[];

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props?: Record<string, unknown> | null,
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (props) {
    for (const [k, v] of Object.entries(props)) {
      if (v == null || v === false) continue;
      if (k.startsWith('on') && typeof v === 'function') {
        el.addEventListener(k.slice(2).toLowerCase(), v as EventListener);
      } else if (k === 'class') {
        el.className = String(v);
      } else if (k === 'style' && typeof v === 'object') {
        Object.assign(el.style, v as Record<string, string>);
      } else if (k === 'checked' || k === 'disabled' || k === 'autofocus') {
        (el as unknown as Record<string, unknown>)[k] = Boolean(v);
      } else if (k === 'value') {
        (el as unknown as Record<string, unknown>)[k] = String(v);
      } else {
        el.setAttribute(k, String(v));
      }
    }
  }
  appendAll(el, children);
  return el;
}

export function appendAll(parent: Node, children: Child[]): void {
  for (const c of children) {
    if (c == null || c === false) continue;
    if (Array.isArray(c)) appendAll(parent, c);
    else if (c instanceof Node) parent.appendChild(c);
    else parent.appendChild(document.createTextNode(String(c)));
  }
}

export function fmtTime(ts: number): string {
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function fmtDate(ts: number): string {
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

export function fmtFp(hex: string): string {
  return (hex.match(/.{1,4}/g) ?? []).join(' ').toUpperCase();
}

export function trustLabel(t: number): string {
  return ['UNVERIFIED', 'VERIFIED', 'BLOCKED'][t] ?? 'UNKNOWN';
}

export async function busy(btn: HTMLButtonElement, fn: () => Promise<void>): Promise<void> {
  if (btn.disabled) return;
  const orig = btn.textContent ?? '';
  btn.disabled = true;
  btn.textContent = '[ ... ]';
  try { await fn(); }
  finally {
    btn.disabled = false;
    btn.textContent = orig;
  }
}

export function short(s: string, n = 22): string {
  if (s.length <= n) return s;
  return s.slice(0, 10) + '…' + s.slice(-8);
}

export function humanSize(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)}MB`;
}

export function isImageName(name: string): boolean {
  return /\.(png|jpe?g|gif|webp|bmp|svg)$/i.test(name);
}

export function mimeFromName(name: string): string {
  const m = name.toLowerCase().match(/\.([a-z0-9]+)$/);
  const ext = m?.[1] ?? '';
  const map: Record<string, string> = {
    png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg',
    gif: 'image/gif', webp: 'image/webp', bmp: 'image/bmp', svg: 'image/svg+xml',
  };
  return map[ext] ?? 'application/octet-stream';
}

export const LOGO = String.raw`
 ┌─┐┬┌─┐┌┐┌┬ ┬
 │ ┬│├─┘│││└┬┘
 └─┘┴┴  ┘└┘ ┴ `;

export abstract class View {
  abstract el: HTMLElement;
  protected subs: Array<() => void> = [];
  protected sub<T>(signal: Signal<T>, fn: (v: T) => void, fire = true): void {
    this.subs.push(signal.subscribe(fn, fire));
  }
  destroy(): void {
    for (const u of this.subs) u();
    this.subs = [];
    this.el.remove();
  }
}
