import type { UnlistenFn } from '@tauri-apps/api/event';

export interface WebCredentials { port?: string; token: string; }
type PageLocation = Pick<Location, 'hostname' | 'host' | 'hash'>;
const KEY = 'gipny.web.pairing';
const loopback = (hostname: string): boolean => hostname === '127.0.0.1' || hostname === 'localhost';
const pagePort = (page: PageLocation): string => page.host.includes(':') ? page.host.split(':').pop()! : '80';
const validPort = (port: string): boolean => /^\d{1,5}$/.test(port) && Number(port) > 0 && Number(port) <= 65535;

function credentials(page: PageLocation, saved: WebCredentials | null): WebCredentials | null {
  const hash = new URLSearchParams(page.hash.replace(/^#/, ''));
  const value = hash.has('token') || hash.has('port')
    ? { token: hash.get('token') ?? '', port: hash.has('port') ? hash.get('port')! : loopback(page.hostname) ? pagePort(page) : undefined }
    : saved;
  if (!value || typeof value.token !== 'string' || !/^[A-Za-z0-9_-]+$/.test(value.token)) return null;
  if (value.port !== undefined && (typeof value.port !== 'string' || !validPort(value.port))) return null;
  if (!loopback(page.hostname) && !value.port) return null;
  return value;
}

/** Pure endpoint resolution. Credentials only ever enter a loopback WS URL. */
export function resolveWebEndpoint(page: PageLocation, saved: WebCredentials | null = null): string | null {
  const value = credentials(page, saved);
  if (!value) return null;
  const explicitPort = new URLSearchParams(page.hash.replace(/^#/, '')).has('port');
  const host = loopback(page.hostname) && !explicitPort && (!value.port || value.port === pagePort(page))
    ? page.host : `127.0.0.1:${value.port}`;
  return `ws://${host}/ws?token=${encodeURIComponent(value.token)}`;
}

/** Scrub launch fragments without persisting unverified credentials. */
export function readWebEndpoint(browser: Pick<Window, 'location' | 'history' | 'sessionStorage'>): string | null {
  let saved: WebCredentials | null = null;
  try { saved = JSON.parse(browser.sessionStorage.getItem(KEY) ?? 'null'); } catch { /* unavailable or invalid storage */ }
  const value = credentials(browser.location, saved);
  const endpoint = resolveWebEndpoint(browser.location, saved);
  const hash = new URLSearchParams(browser.location.hash.replace(/^#/, ''));
  if (hash.has('token') || hash.has('port')) {
    browser.history.replaceState(null, '', browser.location.pathname + browser.location.search);
  }
  try {
    if (!value || !endpoint || hash.has('token') || hash.has('port')) browser.sessionStorage.removeItem(KEY);
  } catch { /* current connection can still use credentials in memory */ }
  return endpoint;
}

/** Call only after web_info has confirmed the connection. */
export function saveWebPairing(port: string, token: string, browser: Pick<Window, 'location' | 'sessionStorage'> = window): boolean {
  const value = { port: port.trim(), token: token.trim() };
  if (!resolveWebEndpoint(browser.location, value)) return false;
  try { browser.sessionStorage.setItem(KEY, JSON.stringify(value)); } catch { /* memory-only pairing */ }
  return true;
}

export function clearWebPairing(): void {
  try { window.sessionStorage.removeItem(KEY); } catch { /* storage may be blocked */ }
}

export class WebTransportClient {
  private ws: WebSocket | null = null;
  private reqId = 0;
  private pending = new Map<number, { resolve: (val: any) => void; reject: (err: any) => void }>();
  private listeners = new Map<string, Set<(event: { payload: any }) => void>>();
  private connectPromise: Promise<void> | null = null;
  private endpoint: string | null = null;
  private failures = 0;
  private retry: ReturnType<typeof setTimeout> | null = null;
  private stopped = false;

  configure(endpoint: string | null): void {
    if (this.retry) clearTimeout(this.retry);
    this.retry = null;
    const previous = this.ws;
    this.ws = null;
    this.endpoint = endpoint;
    this.connectPromise = null;
    this.failures = 0;
    this.stopped = false;
    this.pending.forEach(({ reject }) => reject(new Error('Connection replaced')));
    this.pending.clear();
    previous?.close();
  }

  connect(): Promise<void> {
    if (this.ws?.readyState === WebSocket.OPEN) return Promise.resolve();
    if (this.connectPromise) return this.connectPromise;
    if (this.stopped || !this.endpoint) return Promise.reject(new Error('Pairing required'));
    const endpoint = this.endpoint;
    this.connectPromise = new Promise((resolve, reject) => {
      let openedAt = 0;
      let socket: WebSocket;
      try { socket = new WebSocket(endpoint); }
      catch {
        this.stopped = true;
        window.dispatchEvent(new Event('gipny-pairing-required'));
        reject(new Error('Local connection refused')); return;
      }
      this.ws = socket;
      const timeout = setTimeout(() => socket.close(), 5000);
      socket.onopen = () => { clearTimeout(timeout); openedAt = Date.now(); resolve(); };
      socket.onmessage = (ev) => {
        try {
          const msg = JSON.parse(ev.data);
          const pending = this.pending.get(msg.id);
          if (pending) {
            this.pending.delete(msg.id);
            if (msg.error) pending.reject(msg.error); else pending.resolve(msg.result);
          } else if (msg.event) {
            this.listeners.get(msg.event)?.forEach((handler) => handler({ payload: msg.payload }));
          }
        } catch { /* ignore malformed frames */ }
      };
      socket.onerror = () => { /* close handles errors without exposing capabilities */ };
      socket.onclose = () => {
        clearTimeout(timeout);
        reject(new Error('Local connection refused'));
        if (this.ws !== socket) return;
        this.ws = null;
        this.connectPromise = null;
        this.pending.forEach(({ reject }) => reject(new Error('Local connection closed')));
        this.pending.clear();
        this.failures = openedAt && Date.now() - openedAt > 5000 ? 0 : this.failures + 1;
        if (this.failures >= 3) {
          this.stopped = true;
          window.dispatchEvent(new Event('gipny-pairing-required'));
          return;
        }
        this.retry = setTimeout(() => { this.retry = null; void this.connect().catch(() => {}); }, 2000);
      };
    });
    return this.connectPromise;
  }

  async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    await this.connect();
    return new Promise((resolve, reject) => {
      const id = ++this.reqId;
      this.pending.set(id, { resolve, reject });
      if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ id, method: cmd, params: args ?? {} }));
      else { this.pending.delete(id); reject(new Error('Local connection closed')); }
    });
  }

  async listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<UnlistenFn> {
    if (!this.listeners.has(event)) this.listeners.set(event, new Set());
    const set = this.listeners.get(event)!;
    set.add(handler as any);
    void this.connect().catch(() => {});
    return () => { set.delete(handler as any); };
  }
}

