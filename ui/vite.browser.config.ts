import { defineConfig } from 'vite';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

export default defineConfig({
  base: process.env.VITE_BASE || '/',
  worker: { format: 'es' },
  build: { target: 'es2022', outDir: 'browser-dist', emptyOutDir: true, sourcemap: false, rollupOptions: { input: 'browser.html' } },
  plugins: [{
    name: 'gipny-offline-shell',
    generateBundle(_options, bundle) {
      this.emitFile({ type: 'asset', fileName: 'browser-icon.svg', source: readFileSync(new URL('../core/icons/icon.svg', import.meta.url)) });
      this.emitFile({ type: 'asset', fileName: 'browser-icon-256.png', source: readFileSync(new URL('../core/icons/128x128@2x.png', import.meta.url)) });
      this.emitFile({ type: 'asset', fileName: 'browser.webmanifest', source: JSON.stringify({ id: './browser.html', name: 'Gipny', short_name: 'Gipny', start_url: './browser.html', scope: './', display: 'standalone', background_color: '#eef3f8', theme_color: '#eef3f8', icons: [{ src: 'browser-icon.svg', sizes: 'any', type: 'image/svg+xml', purpose: 'any' }, { src: 'browser-icon-256.png', sizes: '256x256', type: 'image/png', purpose: 'any' }] }) });
      const files = [...Object.keys(bundle), 'browser-icon.svg', 'browser-icon-256.png', 'browser.webmanifest', 'theme-init.js', 'avatars.webp', 'notify.wav', 'sounds/success.wav'].filter(name => !name.endsWith('.map') && name !== 'browser-config.json');
      const version = createHash('sha256').update(files.map(name => {
        const file = bundle[name];
        return file ? `${name}:${file.type === 'chunk' ? file.code : file.source}` : name;
      }).join('\n')).digest('hex').slice(0, 20);
      this.emitFile({ type: 'asset', fileName: 'browser-sw.js', source: `
const CACHE = 'gipny-shell-${version}';
const URLS = ${JSON.stringify(files)}.map(path => new URL(path, self.registration.scope).href);
const ALLOWED = new Set(URLS);
self.addEventListener('install', event => event.waitUntil(caches.open(CACHE).then(cache => cache.addAll(URLS))));
// A waiting update activates when old windows close. Never replace a running
// worker or reload an unlocked profile behind the person's back.
self.addEventListener('activate', event => event.waitUntil((async () => {
  for (const key of await caches.keys()) if (key.startsWith('gipny-shell-') && key !== CACHE) await caches.delete(key);
  await self.clients.claim();
})()));
self.addEventListener('fetch', event => {
  if (event.request.method !== 'GET' || !ALLOWED.has(event.request.url)) return;
  event.respondWith(caches.open(CACHE).then(async cache => (await cache.match(event.request)) || fetch(event.request)));
});
` });
    },
  }],
});
