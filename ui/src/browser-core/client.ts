import type { BrowserBackend, RelayInfo, UnlistenFn } from '../app/api';
import { enrollBrowserPasskey, unlockBrowserPasskey } from './authorization/passkeys';
import { RtcBridge, rtcSettings } from './networking/rtc-transport';
import { loadNetworkConfiguration } from './networking/config';
import { StartupTimings } from './networking/startup';

type Pending = { resolve(value: unknown): void; reject(error: Error): void; command: string };
export class BrowserCoreClient implements BrowserBackend {
  private worker: Worker | null = null;
  private init: Promise<unknown> | null = null;
  private sequence = 0;
  private pending = new Map<number, Pending>();
  private listeners = new Map<string, Set<(payload: unknown) => void>>();
  private config: Record<string, string> | null = null;
  private unlocked = false;
  private reconnectAfterOpen = false;
  private rtc: RtcBridge | null = null;
  private rtcConfig: ReturnType<typeof rtcSettings> | null = null;
  private generation = 0;
  private locking = false;
  private configured: Promise<void> | null = null;
  private configAbort: AbortController | null = null;
  private timings = new StartupTimings();

  /** Public assets/configuration load beside the UI. No profile is opened. */
  async prepare(): Promise<void> {
    const generation = this.generation;
    await this.configure();
    this.active(generation);
    await this.send('prewarm_network');
  }
  private active(generation: number): void {
    if (generation !== this.generation || this.locking) throw new Error('Browser startup cancelled');
  }
  configure(): Promise<void> {
    if (this.configured) return this.configured;
    const generation = this.generation;
    const workerReady = this.ensureWorker();
    const abort = new AbortController(); this.configAbort = abort;
    const loading = loadNetworkConfiguration(new URL('browser-config.json', document.baseURI), fetch, globalThis.caches, abort.signal)
      .then(loaded => { this.active(generation); this.timings.mark('config_loaded'); return loaded; });
    const configured = Promise.all([workerReady, loading]).then(async ([, loaded]) => {
      this.active(generation);
      this.config = loaded?.config ?? null;
      const rtc = loaded?.rtc ?? null;
      if (JSON.stringify(rtc) !== JSON.stringify(this.rtcConfig)) {
        this.rtc?.close();
        this.rtcConfig = rtc;
        this.rtc = rtc ? new RtcBridge(rtc) : null;
      }
      await this.send('init', { config: this.config });
      this.active(generation); this.timings.mark('configured');
    }).finally(() => { if (this.configAbort === abort) this.configAbort = null; });
    this.configured = configured;
    return configured;
  }
  private ensureWorker(): Promise<unknown> {
    if (this.locking) throw new Error('Profile is locking');
    if (this.init) return this.init;
    // The worker checks its own WASM API and initialization result. Window's
    // WebAssembly availability is not a capability check for another realm.
    const worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module', name: 'gipny-core' });
    this.worker = worker;
    this.rtc = this.rtcConfig ? new RtcBridge(this.rtcConfig) : null;
    worker.onmessage = (event: MessageEvent): void => {
      if (this.worker !== worker) return;
      if (event.data?.event === 'rtc_connect') {
        const port = event.ports[0];
        const endpoint = event.data.endpoint;
        const protocol = event.data.protocol;
        try {
          if (!port || !this.rtc || typeof endpoint !== 'string' || endpoint.length > 8192 || protocol !== `gipny.ntcp2.v1.${this.config?.capability}`) throw new Error('Invalid RTC request');
          const url = new URL(endpoint), configured = new URL(this.config!.forwarder_url!);
          url.searchParams.delete('target'); configured.searchParams.delete('target');
          if (url.href !== configured.href) throw new Error('Unexpected RTC endpoint');
          this.rtc.connect(endpoint, protocol, port);
        } catch { port?.postMessage({ kind: 'error' }); port?.close(); }
        return;
      }
      const response = event.data as { id?: number; result?: unknown; error?: string; event?: string; payload?: unknown };
      if (response.event) {
        if (response.event === 'browser_fault') {
          this.terminate(String(response.payload));
          window.dispatchEvent(new CustomEvent('gipny-browser-locked', { detail: response.payload }));
          return;
        }
        for (const listener of this.listeners.get(response.event) || []) listener(response.payload);
        return;
      }
      if (typeof response.id !== 'number') return;
      const pending = this.pending.get(response.id);
      if (!pending) return;
      this.pending.delete(response.id);
      if (response.error) pending.reject(new Error(response.error));
      else pending.resolve(response.result);
    };
    worker.onerror = (): void => {
      if (this.worker !== worker) return;
      const wasUnlocked = this.unlocked;
      this.terminate('The browser core stopped; unlock the profile to recover');
      if (wasUnlocked) window.dispatchEvent(new Event('gipny-browser-locked'));
    };
    this.init = this.send('init', { config: this.config }).then(value => {
      if (this.worker === worker) this.timings.mark('worker_wasm_ready');
      return value;
    }).catch(error => {
      if (this.worker === worker) this.terminate('Browser core initialization failed');
      throw error;
    });
    return this.init;
  }
  private send(command: string, args?: Record<string, unknown>): Promise<unknown> {
    return new Promise((resolve, reject) => {
      if (!this.worker) { reject(new Error('Browser core is stopped')); return; }
      const id = ++this.sequence;
      this.pending.set(id, { resolve, reject, command });
      try { this.worker.postMessage({ id, command, args }); }
      catch (error) { this.pending.delete(id); reject(error); }
    });
  }
  async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    if (command === 'vault_lock') {
      const worker = this.worker;
      this.generation++; this.locking = true;
      this.configAbort?.abort(); this.configAbort = null; this.configured = null;
      if (!worker) { this.terminate(); return null as T; }
      const emergency = setTimeout(() => { if (this.worker === worker) this.terminate('Profile locked'); }, 2000);
      try { await this.send(command); }
      finally { clearTimeout(emergency); if (this.worker === worker) this.terminate('Profile locked'); }
      return null as T;
    }
    const generation = this.generation;
    const opening = command === 'vault_create' || command === 'vault_unlock' || command === 'vault_unlock_passkey';
    try {
      // Profile listing and UI rendering do not wait for a remote code host.
      // Any operation that can start networking must see the config barrier.
      if (opening || command === 'prewarm_network' || command === 'retry_network') await this.configure();
      else await this.ensureWorker();
      this.active(generation);
      const result = await this.send(command, args) as T;
      this.active(generation);
      if (opening) {
        this.unlocked = true;
        if (this.reconnectAfterOpen) {
          this.reconnectAfterOpen = false;
          void this.reconnect().catch(() => {});
        }
      }
      if (command === 'vault_create') void navigator.storage?.persist?.().catch(() => false);
      if (command === 'browser_diagnostics') return { ...result, page_startup_ms: this.timings.snapshot() } as T;
      return result;
    } catch (error) {
      if (opening && generation === this.generation) this.terminate('Profile opening failed');
      throw error;
    }
  }
  listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<UnlistenFn> {
    const callback = (payload: unknown): void => handler({ payload: payload as T });
    let listeners = this.listeners.get(event);
    if (!listeners) { listeners = new Set(); this.listeners.set(event, listeners); }
    listeners.add(callback);
    return Promise.resolve(() => { listeners!.delete(callback); });
  }
  terminate(message = 'Profile locked'): void {
    this.generation++; this.locking = false;
    this.configAbort?.abort(); this.configAbort = null; this.configured = null;
    this.config = null; this.rtcConfig = null;
    this.timings.reset();
    this.rtc?.close(); this.rtc = null;
    this.worker?.terminate(); this.worker = null; this.init = null;
    this.unlocked = false;
    this.reconnectAfterOpen = false;
    for (const pending of this.pending.values()) pending.reject(new Error(message));
    this.pending.clear();
  }
  async enrollPasskey(profile: string): Promise<void> {
    const result = await enrollBrowserPasskey(profile);
    const prf = [...result.prf];
    try {
      await this.invoke('enroll_passkey', { credentialId: [...result.credentialId], rpId: result.rpId, salt: [...result.salt], prf });
    } finally { prf.fill(0); result.prf.fill(0); }
  }
  async reconnect(): Promise<void> {
    if (!this.unlocked) {
      // Config can finish offline while the user is still on the sign-in screen.
      if (this.worker && !this.locking) this.reconnectAfterOpen = true;
      return;
    }
    const generation = this.generation;
    const info = await this.invoke<RelayInfo>('get_relay_info');
    if (!this.unlocked || generation !== this.generation ||
        (info.hosted.state !== 'failed' && info.hosted.state !== 'unconfigured')) return;
    if (!this.configAbort) this.configured = null;
    await this.configure();
    if (!this.unlocked || generation !== this.generation) return;
    await this.invoke('retry_network');
  }
  async unlockPasskey(profile: string, totpCode?: string, screenOnly = false): Promise<void> {
    const entries = await this.invoke<{ credentialId: number[]; rpId: string; salt: number[] }[]>('passkey_entries', { profile });
    const result = await unlockBrowserPasskey(entries.map(entry => ({ credentialId: new Uint8Array(entry.credentialId), rpId: entry.rpId, salt: new Uint8Array(entry.salt) })));
    const prf = [...result.prf];
    try { await this.invoke(screenOnly ? 'verify_screen_passkey' : 'vault_unlock_passkey', { profile, credentialId: [...result.credentialId], prf, ...(totpCode ? { totpCode } : {}) }); }
    finally { prf.fill(0); result.prf.fill(0); }
  }
}
