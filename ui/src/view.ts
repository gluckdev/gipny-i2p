import type { Signal } from './state';
import { avatarIndex, paintAvatar } from './avatars';

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

/** A round avatar. For a person (`seed` = their signing key) it is their
 * character from the avatar sprite; with `picture` false (groups) it is the
 * name's initials in a colour that stays the same for the same `seed`. */
export function avatar(name: string, seed: string, extra = '', picture = true): HTMLElement {
  if (picture) {
    const pic = h('div', { class: 'avatar' + (extra ? ' ' + extra : ''), role: 'img', 'aria-label': name });
    paintAvatar(pic, avatarIndex(seed));
    return pic;
  }
  const words = name.trim().split(/[\s._@-]+/).filter(Boolean);
  const [a, b] = words;
  const initials = (a && b ? a.charAt(0) + b.charAt(0) : (a ?? '?').slice(0, 2)).toUpperCase();
  let hash = 0;
  for (const ch of seed || name) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  const el = h('div', { class: 'avatar' + (extra ? ' ' + extra : '') }, initials);
  el.style.setProperty('--avatar-hue', String(hash % 360));
  return el;
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
  return ['Не проверен', 'Проверен', 'Заблокирован'][t] ?? 'Неизвестно';
}

export async function busy(btn: HTMLButtonElement, fn: () => Promise<void>): Promise<void> {
  if (btn.disabled) return;
  const orig = btn.textContent ?? '';
  btn.disabled = true;
  btn.textContent = '...';
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

export function fmtAgo(ts: number): string {
  const d = Math.max(0, Date.now() - ts);
  if (d < 60_000) return 'только что';
  const m = Math.floor(d / 60_000);
  if (m < 60) return `${m} мин назад`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} ч назад`;
  const days = Math.floor(h / 24);
  if (days < 30) return `${days} дн назад`;
  return new Date(ts).toISOString().slice(0, 10);
}

export function humanSize(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)}MB`;
}

export function isImageName(name: string): boolean {
  return /\.(png|jpe?g|gif|webp|bmp|svg)$/i.test(name);
}

export function isAudioName(name: string): boolean {
  return /\.(wav|ogg|opus|mp3|m4a|aac|flac)$/i.test(name);
}

export function mimeFromName(name: string): string {
  const m = name.toLowerCase().match(/\.([a-z0-9]+)$/);
  const ext = m?.[1] ?? '';
  const map: Record<string, string> = {
    png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg',
    gif: 'image/gif', webp: 'image/webp', bmp: 'image/bmp', svg: 'image/svg+xml',
    wav: 'audio/wav', ogg: 'audio/ogg', opus: 'audio/opus', mp3: 'audio/mpeg', m4a: 'audio/mp4',
  };
  return map[ext] ?? 'application/octet-stream';
}

/**
 * The app mark: the same chevron-and-cursor as the launcher icon
 * (core/icons/icon.svg), so the thing a person clicks and the thing that greets
 * them are recognisably one product.
 *
 * This replaced box-drawing ASCII art. That art needed a monospace font to hold
 * its shape, and the app's font is now a proportional sans — the letters pulled
 * apart into an unreadable smear. A drawn mark cannot break that way.
 */
export function logoMark(): SVGSVGElement {
  const NS = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(NS, 'svg');
  svg.setAttribute('viewBox', '0 0 512 512');
  svg.setAttribute('class', 'logo-mark');
  svg.setAttribute('aria-hidden', 'true');

  const chevron = document.createElementNS(NS, 'path');
  chevron.setAttribute('d', 'M168 148 L280 256 L168 364');
  chevron.setAttribute('fill', 'none');
  chevron.setAttribute('stroke', 'currentColor');
  chevron.setAttribute('stroke-width', '46');
  chevron.setAttribute('stroke-linecap', 'round');
  chevron.setAttribute('stroke-linejoin', 'round');

  const cursor = document.createElementNS(NS, 'rect');
  cursor.setAttribute('x', '300');
  cursor.setAttribute('y', '318');
  cursor.setAttribute('width', '120');
  cursor.setAttribute('height', '40');
  cursor.setAttribute('rx', '20');
  cursor.setAttribute('fill', 'var(--amber)');

  svg.append(chevron, cursor);
  return svg;
}

/**
 * Mark plus wordmark, as shown on the auth screens.
 *
 * One lockup rather than a mark and a separate badge. "i2P" is part of the
 * name — this is the i2p fork, installed beside the original as its own app —
 * so it is drawn as a layer of the logo itself: in front, at three times the
 * wordmark's size, at a third of full strength. Strong enough to read as the
 * brand, faint enough that "gipny" stays the first thing the eye lands on.
 *
 * The overlay is aria-hidden and the lockup carries a label, so assistive tech
 * reads the name once instead of reading "gipny" and then "i2P" as noise.
 */
export function logo(): HTMLElement {
  return h('div', { class: 'logo', role: 'img', 'aria-label': 'gipny i2P' },
    logoMark(),
    h('span', { class: 'logo-word', 'aria-hidden': 'true' }, 'gipny'),
    h('span', { class: 'logo-net', 'aria-hidden': 'true' }, 'i2P'),
  );
}

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
