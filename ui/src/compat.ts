// Runtime shims for the oldest WebView the Android build supports (Chromium
// 83, see WEBVIEW_FLOOR in vite.config.ts). The compiler lowers syntax to that
// floor; these cover the few DOM/JS APIs the UI uses that arrived later. Each
// shim installs only when the native one is missing, so modern engines are
// untouched. Imported first by main.ts, before any module that can call them.

function replaceChildrenShim(this: Node, ...nodes: (Node | string)[]): void {
  while (this.firstChild) this.removeChild(this.firstChild);
  for (const node of nodes) this.appendChild(typeof node === 'string' ? document.createTextNode(node) : node);
}

// Chromium 86.
for (const proto of [Element.prototype, Document.prototype, DocumentFragment.prototype]) {
  if (typeof (proto as { replaceChildren?: unknown }).replaceChildren !== 'function') {
    Object.defineProperty(proto, 'replaceChildren', { value: replaceChildrenShim, writable: true, configurable: true });
  }
}

// Chromium 93.
if (typeof (Object as { hasOwn?: unknown }).hasOwn !== 'function') {
  Object.defineProperty(Object, 'hasOwn', {
    value: (target: object, key: PropertyKey): boolean => Object.prototype.hasOwnProperty.call(target, key),
    writable: true, configurable: true,
  });
}

// Chromium 92. RFC 4122 v4 from the same CSPRNG the native one uses.
if (typeof crypto !== 'undefined' && typeof (crypto as { randomUUID?: unknown }).randomUUID !== 'function') {
  Object.defineProperty(crypto, 'randomUUID', {
    value: (): string => {
      const b = crypto.getRandomValues(new Uint8Array(16));
      b[6] = (b[6]! & 0x0f) | 0x40;
      b[8] = (b[8]! & 0x3f) | 0x80;
      const hex = Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
      return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
    },
    writable: true, configurable: true,
  });
}

export {};
