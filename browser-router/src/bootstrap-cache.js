// Only public RouterInfo archives enter this cache. Rust verifies the digest,
// every peer signature and freshness on every read, before creating the router.
const NAME = 'gipny-public-netdb-v1';
const LIMIT = 4 * 1024 * 1024;
const key = digest => new URL('/__gipny-public-netdb/v1/' + digest, globalThis.location.href).href;

export async function loadBootstrap(digest) {
  if (!/^[0-9a-f]{64}$/i.test(digest)) return null;
  try {
    const cache = await caches.open(NAME);
    const response = await cache.match(key(digest));
    if (!response?.body) return null;
    const reader = response.body.getReader();
    const bytes = new Uint8Array(LIMIT);
    let length = 0;
    try {
      while (true) {
        const part = await reader.read();
        if (part.done) break;
        if (length + part.value.byteLength > LIMIT) throw new Error('Oversize cached bootstrap');
        bytes.set(part.value, length); length += part.value.byteLength;
      }
    } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
    return bytes.slice(0, length);
  } catch { return null; }
}

export async function saveBootstrap(digest, bytes) {
  if (!/^[0-9a-f]{64}$/i.test(digest) || !bytes.length || bytes.length > LIMIT) return;
  const owned = bytes.slice(); // A WASM memory view may detach while cache.open awaits.
  try {
    const cache = await caches.open(NAME);
    const target = key(digest);
    await cache.put(target, new Response(owned, { headers: { 'Content-Type': 'application/octet-stream' } }));
    for (const request of await cache.keys()) if (request.url !== target) await cache.delete(request);
  } catch { /* Optional public cache: quota denial does not break online use. */ }
}

export async function removeBootstrap(digest) {
  try { await (await caches.open(NAME)).delete(key(digest)); } catch { /* Optional cache. */ }
}
