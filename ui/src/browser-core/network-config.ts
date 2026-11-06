import { rtcSettings, type RtcSettings } from './rtc-transport';

const CACHE = 'gipny-public-network-v1';
const LIMIT = 65536;
type NetworkConfiguration = { config: Record<string, string>; rtc: RtcSettings | null; publicValue: Record<string, unknown> };

/** This cache holds the public deployment configuration, never profile data. */
export function validateNetworkConfiguration(value: unknown): NetworkConfiguration {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid browser configuration');
  const raw = value as Record<string, unknown>;
  const fields = ['snapshot_url', 'snapshot_sha256', 'forwarder_url', 'capability'];
  if (fields.some(field => typeof raw[field] !== 'string' || (raw[field] as string).length > 4096)) throw new Error('Invalid browser network configuration');
  const config = Object.fromEntries(fields.map(field => [field, raw[field] as string]));
  const snapshot = new URL(config.snapshot_url!);
  const forwarder = new URL(config.forwarder_url!);
  const loopback = (url: URL): boolean => ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
  if ((snapshot.protocol !== 'https:' && !(snapshot.protocol === 'http:' && loopback(snapshot))) || snapshot.username || snapshot.password || snapshot.hash) throw new Error('Invalid snapshot URL');
  if ((forwarder.protocol !== 'wss:' && !(forwarder.protocol === 'ws:' && loopback(forwarder))) || forwarder.username || forwarder.password || forwarder.hash || forwarder.search) throw new Error('Invalid forwarder URL');
  if (!/^[0-9a-f]{64}$/i.test(config.snapshot_sha256!) || !/^[A-Za-z0-9_-]{43}$/.test(config.capability!)) throw new Error('Invalid network configuration digest/token');
  const transport = raw.transport ?? 'wss';
  if (transport !== 'wss' && transport !== 'webrtc') throw new Error('Invalid browser transport');
  config.transport = transport;
  const rtc = transport === 'webrtc' ? rtcSettings(raw) : null;
  const publicValue: Record<string, unknown> = { ...config };
  if (rtc) {
    publicValue.rtc_ice_policy = rtc.iceTransportPolicy;
    publicValue.rtc_ice_servers = rtc.iceServers;
    if (rtc.fixturePrivate) publicValue.rtc_fixture_private = true;
  }
  return { config, rtc, publicValue };
}

async function readConfiguration(response: Response): Promise<NetworkConfiguration> {
  if (response.status !== 200 || response.redirected || !response.body) throw new Error('Missing browser configuration');
  const reader = response.body.getReader();
  const bytes = new Uint8Array(LIMIT);
  let length = 0;
  try {
    while (true) {
      const chunk = await reader.read();
      if (chunk.done) break;
      if (length + chunk.value.byteLength > LIMIT) throw new Error('Oversize browser configuration');
      bytes.set(chunk.value, length); length += chunk.value.byteLength;
    }
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
  return validateNetworkConfiguration(JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(0, length))));
}

export async function loadNetworkConfiguration(source: URL, fetcher: typeof fetch = fetch, storage: CacheStorage | undefined = globalThis.caches): Promise<NetworkConfiguration | null> {
  let cache: Cache | undefined;
  try { cache = await storage?.open(CACHE); } catch { /* Cache denial does not prevent an online start. */ }
  const drop = async (): Promise<void> => { try { await cache?.delete(source.href); } catch { /* Optional public cache. */ } };
  const fallback = async (): Promise<NetworkConfiguration | null> => {
    try {
      const saved = await cache?.match(source.href);
      if (saved) return await readConfiguration(saved);
    } catch { await drop(); }
    return null;
  };
  const abort = new AbortController();
  const deadline = setTimeout(() => abort.abort(), 2500);
  let response: Response;
  try {
    response = await fetcher(source, { cache: 'no-store', credentials: 'omit', redirect: 'error', referrerPolicy: 'no-referrer', signal: abort.signal });
  } catch { clearTimeout(deadline); return fallback(); }
  try {
    // Losing the code host (including a removed repository, denial or rate
    // limit) must not revoke an installed client's public network settings.
    // A valid new configuration replaces them; an invalid cached copy is
    // discarded by fallback. Bridge authorization and RI freshness are still
    // checked independently and never bypassed by this cache.
    const current = await readConfiguration(response);
    try {
      await cache?.put(source.href, new Response(JSON.stringify(current.publicValue), { headers: { 'Content-Type': 'application/json' } }));
    } catch { /* Quota/persistence is best effort; in-memory configuration remains valid. */ }
    return current;
  } catch { return await fallback(); }
  finally { clearTimeout(deadline); }
}
