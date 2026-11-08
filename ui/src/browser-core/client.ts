import type { BrowserBackend, UnlistenFn } from '../api';
import { enrollBrowserPasskey, unlockBrowserPasskey } from './passkeys';
import { RtcBridge, rtcSettings } from './rtc-transport';
import { loadNetworkConfiguration } from './network-config';

type Pending = { resolve(value: unknown): void; reject(error: Error): void; command: string };
export class BrowserCoreClient implements BrowserBackend {
  private worker: Worker | null = null;
  private init: Promise<unknown> | null = null;
  private sequence = 0;
  private pending = new Map<number, Pending>();
  private listeners = new Map<string, Set<(payload: unknown) => void>>();
  private config: Record<string, string> | null = null;
  private unlocked = false;
  private rtc: RtcBridge | null = null;
  private rtcConfig: ReturnType<typeof rtcSettings> | null = null;
  async configure(): Promise<void> {
    const loaded = await loadNetworkConfiguration(new URL('browser-config.json', document.baseURI));
    this.config = loaded?.config ?? null;
    this.rtcConfig = loaded?.rtc ?? null;
  }
  private ensureWorker(): Promise<unknown> {
    if (this.init) return this.init;
    // iOS Lockdown Mode and some hardened browsers remove the WebAssembly
    // global; the Rust core cannot start and a worker would only die quietly.
    if (typeof WebAssembly === 'undefined') throw new Error('WebAssembly is disabled in this browser (Lockdown Mode or a restrictive policy), so the messenger cannot run here');
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
    this.init = this.send('init', { config: this.config }).catch(error => {
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
      if (!this.worker) return null as T;
      const emergency = setTimeout(() => this.terminate('Profile locked'), 2000);
      try { await this.send(command); }
      finally { clearTimeout(emergency); this.terminate('Profile locked'); }
      return null as T;
    }
    await this.ensureWorker();
    const opening = command === 'vault_create' || command === 'vault_unlock' || command === 'vault_unlock_passkey';
    try {
      const result = await this.send(command, args) as T;
      if (opening) this.unlocked = true;
      if (command === 'vault_create') void navigator.storage?.persist?.().catch(() => false);
      return result;
    } catch (error) {
      if (opening) this.terminate('Profile opening failed');
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
    this.rtc?.close(); this.rtc = null;
    this.worker?.terminate(); this.worker = null; this.init = null;
    this.unlocked = false;
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
    if (!this.unlocked) return;
    await this.configure();
    if (!this.unlocked) return;
    this.rtc?.close();
    this.rtc = this.rtcConfig ? new RtcBridge(this.rtcConfig) : null;
    await this.invoke('init', { config: this.config });
    await this.invoke('retry_network');
  }
  async unlockPasskey(profile: string, totpCode?: string): Promise<void> {
    const entries = await this.invoke<{ credentialId: number[]; rpId: string; salt: number[] }[]>('passkey_entries', { profile });
    const result = await unlockBrowserPasskey(entries.map(entry => ({ credentialId: new Uint8Array(entry.credentialId), rpId: entry.rpId, salt: new Uint8Array(entry.salt) })));
    const prf = [...result.prf];
    try { await this.invoke('vault_unlock_passkey', { profile, credentialId: [...result.credentialId], prf, ...(totpCode ? { totpCode } : {}) }); }
    finally { prf.fill(0); result.prf.fill(0); }
  }
}
