import { defineConfig } from 'vite';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { buildDefinitions } from './build-policy';

// The oldest browsers the WASM messenger can honestly run in. The profile
// vault lives in OPFS behind synchronous access handles; Safari 15.2–16.3 and
// Chromium 102–107 shipped getSize/truncate/flush/close returning Promises,
// which this client would read as a corrupt profile. Safari 16.4 made them
// synchronous (and first accepted the CSP 'wasm-unsafe-eval' browser.html
// relies on, in 16.0); Chromium did in 108, Firefox shipped them synchronous
// in 111. Every iOS browser is WebKit, so ios16.4 is the real phone floor.
// Vite's own default (baseline-widely-available) is already safari16.4 — the
// earlier 'es2022' setting was looser than that, not stricter. Top-level
// await stays: the entry relies on it (Safari 15, Chrome 89, Firefox 89).
// ui/tests/browser-compat.test.mjs holds the build to this floor and
// src/browser-core/storage.ts names it when an older Safari opens a profile.
export const BROWSER_FLOOR = ['safari16.4', 'ios16.4', 'chrome108', 'edge108', 'firefox111'];

export default defineConfig(({ command }) => ({
  define: buildDefinitions(command === 'serve'),
  base: process.env.VITE_BASE || '/',
  worker: { format: 'es' },
  build: { target: BROWSER_FLOOR, cssTarget: BROWSER_FLOOR, outDir: 'browser-dist', emptyOutDir: true, sourcemap: false, rollupOptions: { input: 'browser.html' } },
  plugins: [{
    name: 'gipny-offline-shell',
    generateBundle: {
      // Vite emits transformed HTML/CSS after ordinary user hooks.
      order: 'post',
      handler(_options, bundle) {
      this.emitFile({ type: 'asset', fileName: 'browser-icon.svg', source: readFileSync(new URL('../core/icons/icon.svg', import.meta.url)) });
      this.emitFile({ type: 'asset', fileName: 'browser-icon-256.png', source: readFileSync(new URL('../core/icons/128x128@2x.png', import.meta.url)) });
      this.emitFile({ type: 'asset', fileName: 'browser.webmanifest', source: JSON.stringify({ id: './browser.html', name: 'Gipny', short_name: 'Gipny', start_url: './browser.html', scope: './', display: 'standalone', background_color: '#eef3f8', theme_color: '#eef3f8', icons: [{ src: 'browser-icon.svg', sizes: 'any', type: 'image/svg+xml', purpose: 'any' }, { src: 'browser-icon-256.png', sizes: '256x256', type: 'image/png', purpose: 'any' }] }) });
      const files = [...new Set([...Object.keys(bundle), 'browser.html', 'browser-icon.svg', 'browser-icon-256.png', 'browser.webmanifest', 'theme-init.js', 'avatars.webp', 'notify.wav', 'sounds/success.wav'])].filter(name => !name.endsWith('.map') && name !== 'browser-config.json');
      if (!files.some(name => name.endsWith('.wasm')) || !files.some(name => /worker.*\.js$/.test(name))) throw new Error('Offline shell is missing the emitted WASM worker assets');
      const digest = createHash('sha256');
      const downloadWorker = readFileSync(new URL('./src/browser-download-sw.js', import.meta.url), 'utf8');
      digest.update(downloadWorker);
      for (const name of files) {
        const file = bundle[name];
        digest.update(name);
        if (file) digest.update(file.type === 'chunk' ? file.code : file.source);
      }
      const version = digest.digest('hex').slice(0, 20);
      this.emitFile({ type: 'asset', fileName: 'browser-sw.js', source: `
const CACHE = 'gipny-shell-${version}';
const URLS = ${JSON.stringify(files)}.map(path => new URL(path, self.registration.scope).href);
const ALLOWED = new Set(URLS);
${downloadWorker}
self.addEventListener('install', event => event.waitUntil(caches.open(CACHE).then(cache => cache.addAll(URLS))));
// A waiting update activates when old windows close. Never replace a running
// worker or reload an unlocked profile behind the person's back.
self.addEventListener('activate', event => event.waitUntil((async () => {
  for (const key of await caches.keys()) if (key.startsWith('gipny-shell-') && key !== CACHE) await caches.delete(key);
  await self.clients.claim();
})()));
self.addEventListener('fetch', event => {
  if (event.request.method !== 'GET') return;
  const path = new URL(event.request.url);
  const shell = new URL('browser.html', self.registration.scope).href;
  if (event.request.mode === 'navigate' && path.origin === self.location.origin && (path.href === self.registration.scope || path.pathname === new URL(shell).pathname)) {
    event.respondWith(caches.open(CACHE).then(async cache => (await cache.match(shell)) || fetch(event.request)));
    return;
  }
  if (!ALLOWED.has(event.request.url)) return;
  event.respondWith(caches.open(CACHE).then(async cache => (await cache.match(event.request)) || fetch(event.request)));
});
` });
      },
    },
  }],
}));
