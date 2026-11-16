/** Line icons drawn for gipny: 24×24, stroked with the current text colour, so
 * they follow the theme and the button's hover colour. */
const PATHS = {
  plus: '<path d="M12 5v14M5 12h14"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>',
  settings: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>',
  lock: '<rect x="4" y="11" width="16" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>',
  attach: '<path d="m21 11.5-8.6 8.6a5.5 5.5 0 0 1-7.8-7.8l8.6-8.6a3.7 3.7 0 0 1 5.2 5.2l-8.6 8.6a1.8 1.8 0 0 1-2.6-2.6l8-8"/>',
  send: '<path d="M22 2 11 13"/><path d="m22 2-7 20-4-9-9-4 20-7z"/>',
  image: '<rect x="3" y="3" width="18" height="18" rx="3"/><circle cx="8.5" cy="8.5" r="1.5"/><path d="m21 15-5-5L5 21"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 16v-4M12 8h.01"/>',
  close: '<path d="M18 6 6 18M6 6l12 12"/>',
  back: '<path d="M15 18 9 12l6-6"/>',
  chevronLeft: '<path d="M15 18 9 12l6-6"/>',
  chevronRight: '<path d="m9 18 6-6-6-6"/>',
  more: '<circle cx="5" cy="12" r="1.3"/><circle cx="12" cy="12" r="1.3"/><circle cx="19" cy="12" r="1.3"/>',
  block: '<circle cx="12" cy="12" r="9"/><path d="m5.7 5.7 12.6 12.6"/>',
  shield: '<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/><path d="m9 12 2 2 4-4"/>',
} as const;

export type IconName = keyof typeof PATHS;

export function icon(name: IconName, size = 20): SVGSVGElement {
  const wrap = document.createElement('span');
  wrap.innerHTML = `<svg class="ico" width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${PATHS[name]}</svg>`;
  return wrap.firstElementChild as SVGSVGElement;
}

/** The empty chat pane: two chat bubbles passing through a tunnel of rings —
 * messages that travel inside the i2p network. */
export function emptyChatArt(): HTMLElement {
  const el = document.createElement('div');
  el.className = 'art art-empty';
  el.innerHTML = `<svg viewBox="0 0 240 160" width="240" height="160" aria-hidden="true">
  <defs>
    <linearGradient id="ga" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#2f6fed"/><stop offset="1" stop-color="#7b5cf0"/></linearGradient>
    <linearGradient id="gb" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#34c3a0"/><stop offset="1" stop-color="#2f9bed"/></linearGradient>
  </defs>
  <g fill="none" stroke="currentColor" stroke-opacity=".14" stroke-width="2">
    <ellipse cx="120" cy="80" rx="96" ry="58"/><ellipse cx="120" cy="80" rx="72" ry="43"/><ellipse cx="120" cy="80" rx="48" ry="29"/>
  </g>
  <g stroke="currentColor" stroke-opacity=".22" stroke-width="2" stroke-dasharray="3 6" fill="none"><path d="M60 58c30-26 90-26 120 0"/><path d="M180 102c-30 26-90 26-120 0"/></g>
  <rect x="26" y="40" width="72" height="42" rx="16" fill="url(#ga)"/><path d="M40 82l-6 12 16-12z" fill="#5a66ef"/>
  <rect x="38" y="54" width="44" height="5" rx="2.5" fill="#fff" fill-opacity=".9"/><rect x="38" y="64" width="30" height="5" rx="2.5" fill="#fff" fill-opacity=".65"/>
  <rect x="142" y="80" width="72" height="42" rx="16" fill="url(#gb)"/><path d="M200 122l6 12-16-12z" fill="#2fa6d8"/>
  <rect x="154" y="94" width="48" height="5" rx="2.5" fill="#fff" fill-opacity=".9"/><rect x="154" y="104" width="26" height="5" rx="2.5" fill="#fff" fill-opacity=".65"/>
  <g transform="translate(108 64)"><rect x="2" y="12" width="20" height="16" rx="4" fill="url(#ga)"/><path d="M6 12V9a6 6 0 0 1 12 0v3" fill="none" stroke="url(#ga)" stroke-width="3"/><circle cx="12" cy="20" r="2.4" fill="#fff"/></g>
</svg>`;
  return el;
}

/** Decorative network for the "about" header: nodes joined by links. */
export function networkArt(): HTMLElement {
  const el = document.createElement('div');
  el.className = 'art art-network';
  const nodes: [number, number][] = [[20, 30], [70, 12], [120, 40], [170, 16], [60, 70], [150, 78], [210, 50]];
  const links: [number, number][] = [[0, 1], [1, 2], [2, 3], [0, 4], [4, 2], [2, 5], [3, 6], [5, 6], [1, 4]];
  el.innerHTML = `<svg viewBox="0 0 230 96" width="230" height="96" aria-hidden="true">
  <g stroke="#fff" stroke-opacity=".35" stroke-width="1.5">${links.map(([a, b]) => `<line x1="${nodes[a]![0]}" y1="${nodes[a]![1]}" x2="${nodes[b]![0]}" y2="${nodes[b]![1]}"/>`).join('')}</g>
  <g fill="#fff">${nodes.map(([x, y], i) => `<circle cx="${x}" cy="${y}" r="${i === 2 ? 7 : 4.5}" fill-opacity="${i === 2 ? 1 : 0.8}"/>`).join('')}</g>
</svg>`;
  return el;
}
