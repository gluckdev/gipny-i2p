// Embedded in browser-sw.js at build time. Download tokens are random,
// single-use capabilities that live only in memory and end with the window
// that requested them. Never put them in Cache.
const downloads = new Map();
let activeDownloads = 0;
self.addEventListener('message', event => {
  const p = event.data, port = event.ports[0];
  if (p?.kind !== 'gipny-download' || !port) return;
  const owner = event.source;
  if (!owner?.id || !owner.url?.startsWith(self.registration.scope) || !/^[0-9a-f-]{36}$/.test(p.token) || typeof p.name !== 'string' || p.name.length > 1024 || !Number.isSafeInteger(p.size) || p.size < 0 || p.size > 2147483648 || !Number.isInteger(p.parts) || p.parts < 1 || p.parts > 10923 || activeDownloads >= 4 || downloads.has(p.token)) {
    port.postMessage({ error: 'Invalid download request' }); port.close(); return;
  }
  let finish;
  event.waitUntil(new Promise(resolve => { finish = resolve; }));
  let active = true, index = 0, received = 0, pending = null, controller = null, timer;
  activeDownloads++;
  const close = (error) => {
    if (!active) return;
    active = false; activeDownloads--; clearTimeout(timer); downloads.delete(p.token);
    if (error && controller) controller.error(new Error(error));
    pending?.(); pending = null;
    port.postMessage(error ? { error } : { done: true }); port.close(); finish();
  };
  const arm = () => { clearTimeout(timer); timer = setTimeout(() => close('Download timed out'), 120000); };
  port.onmessage = event => {
    const m = event.data;
    if (!active) return;
    if (m?.error) { close('Download interrupted'); return; }
    if (!pending || m?.index !== index || !(m.bytes instanceof Uint8Array) || m.bytes.length > 196608 || received + m.bytes.length > p.size || (m.bytes.length === 0 && p.size !== 0)) { close('Invalid download part'); return; }
    arm(); received += m.bytes.length; index++;
    controller.enqueue(m.bytes);
    const resolve = pending; pending = null; resolve();
    if (index === p.parts) {
      if (received !== p.size) { close('Incomplete download'); return; }
      controller.close(); close();
    }
  };
  const stream = new ReadableStream({
    start(value) { controller = value; },
    pull() {
      if (!active) return;
      return new Promise(resolve => { pending = resolve; arm(); port.postMessage({ pull: index }); });
    },
    cancel() { close('Download cancelled'); },
  }, { highWaterMark: 0 });
  downloads.set(p.token, { owner: owner.id, stream, name: p.name, size: p.size, close });
  arm(); port.postMessage({ ready: p.token });
});
self.addEventListener('fetch', event => {
  const url = new URL(event.request.url);
  const base = new URL('__gipny_download/', self.registration.scope).href;
  if (!url.href.startsWith(base)) return;
  const token = url.href.slice(base.length), item = downloads.get(token);
  const unavailable = () => new Response('Download unavailable', { status: 404, headers: { 'Cache-Control': 'no-store' } });
  // A subresource fetch names its window and must be the owner's. An
  // `<a download>` click is a navigation, which has no client id yet, so it
  // cannot be bound to a window: the random single-use token is the guarantee,
  // and the owning window must still be open.
  if (event.request.method !== 'GET' || !item || (event.clientId ? event.clientId !== item.owner : event.request.mode !== 'navigate')) {
    event.respondWith(unavailable()); return;
  }
  downloads.delete(token); // Single-use capability; no reload/range replay.
  const name = encodeURIComponent(item.name.replace(/[\r\n\u0000]/g, '').split(/[\\/]/).pop() || 'attachment').replace(/[!'()*]/g, ch => '%' + ch.charCodeAt(0).toString(16));
  event.respondWith(self.clients.get(item.owner).then(owner => {
    if (!owner) { item.close('Download window closed'); return unavailable(); }
    return new Response(item.stream, { headers: {
      'Content-Type': 'application/octet-stream',
      'Content-Disposition': "attachment; filename*=UTF-8''" + name,
      'Content-Length': String(item.size),
      'Cache-Control': 'no-store',
      'X-Content-Type-Options': 'nosniff',
      'Content-Security-Policy': "default-src 'none'; sandbox",
    } });
  }));
});
