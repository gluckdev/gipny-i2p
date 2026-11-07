/** Ring-buffer action log for the browser core (prerelease builds only). Every
 * line is written to the worker console (`DevTools → Consoles → [worker]`,
 * Debug/Verbose levels) and kept in memory so `read_debug_log` /
 * `read_previous_log` can serve the in-app viewer. Lines name local contact
 * ids, sizes and states only: no message text, names, keys or I2P addresses.
 *
 * On lock the current session is copied to IndexedDB (`gipny-browser-log`) so
 * the viewer can show the previous session. That copy is NOT encrypted and is
 * shared by every profile of this origin; clearing the log deletes it, and a
 * stable build deletes any copy an earlier prerelease left behind. */
const CAP = 2048;
const EMPTY = '(журнал пуст)';
const DIAGNOSTICS = typeof __GIPNY_DIAGNOSTICS__ !== 'undefined' && __GIPNY_DIAGNOSTICS__;
export const LOG_DATABASE = 'gipny-browser-log';
export const LOG_LOCATION = DIAGNOSTICS
  ? `worker memory · DevTools console · IndexedDB "${LOG_DATABASE}" (previous session; not encrypted; shared by browser profiles)`
  : 'disabled in this build';

/** Last line of defence for error texts from lower layers: mask anything
 * shaped like a key, hash, I2P address or long base64 destination. */
export function scrubLog(text: string): string {
  return text
    .replace(/\b[a-z2-7]{52,}\.b32\.i2p\b/gi, '[i2p address]')
    .replace(/\b[0-9a-f]{24,}\b/gi, '[key]')
    .replace(/[A-Za-z0-9+/~=-]{80,}/g, '[destination]');
}

function stamp(date = new Date()): string {
  const two = (n: number): string => String(n).padStart(2, '0');
  return `${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}.${String(date.getMilliseconds()).padStart(3, '0')}`;
}

type Level = 'debug' | 'info';

function withStore<T>(fallback: T, action: (db: IDBDatabase, done: (value: T) => void) => void): Promise<T> {
  return new Promise(resolve => {
    if (!('indexedDB' in globalThis)) return resolve(fallback);
    try {
      const open = indexedDB.open(LOG_DATABASE, 1);
      open.onupgradeneeded = () => open.result.createObjectStore('logs');
      open.onerror = () => resolve(fallback);
      open.onsuccess = () => {
        const db = open.result;
        try { action(db, value => { db.close(); resolve(value); }); } catch { db.close(); resolve(fallback); }
      };
    } catch { resolve(fallback); }
  });
}

function readStore(key: string): Promise<string> {
  if (!DIAGNOSTICS) return Promise.resolve('');
  return withStore('', (db, done) => {
    const row = db.transaction('logs').objectStore('logs').get(key);
    row.onerror = () => done('');
    row.onsuccess = () => done(typeof row.result === 'string' ? row.result : '');
  });
}

function writeStore(key: string, value: string): Promise<void> {
  if (!DIAGNOSTICS) return Promise.resolve();
  return withStore(undefined, (db, done) => {
    const row = db.transaction('logs', 'readwrite').objectStore('logs').put(value, key);
    row.onerror = () => done(undefined);
    row.onsuccess = () => done(undefined);
  });
}

/** Remove the persisted copy (any build). Resolves even if it is absent. */
export function deleteLogStore(): Promise<void> {
  return new Promise(resolve => {
    if (!('indexedDB' in globalThis)) return resolve();
    try {
      const request = indexedDB.deleteDatabase(LOG_DATABASE);
      request.onsuccess = request.onerror = request.onblocked = () => resolve();
    } catch { resolve(); }
  });
}

export class ActionLog {
  private rows: string[] = [];

  constructor() {
    // A stable build must not keep an earlier prerelease's unencrypted copy.
    if (!DIAGNOSTICS) void deleteLogStore();
  }

  debug(action: string, detail = ''): void { this.write('debug', action, detail); }
  info(action: string, detail = ''): void { this.write('info', action, detail); }

  private write(level: Level, action: string, detail: string): void {
    if (!DIAGNOSTICS) return;
    const line = `[${stamp()}] [gipny-browser] ${action}${detail ? ' · ' + scrubLog(detail) : ''}`;
    console[level](line);
    this.rows.push(line);
    if (this.rows.length > CAP) this.rows.splice(0, this.rows.length - CAP);
  }

  get isEnabled(): boolean { return DIAGNOSTICS; }

  /** Erase the current session and the persisted previous-session copy. */
  async clear(): Promise<void> { this.rows = []; await deleteLogStore(); }
  text(): string { return this.rows.join('\n') || EMPTY; }

  /** The ring buffer dies with the worker, so the viewer needs a copy of the
   * current session in IndexedDB to reset it to the previous session. */
  snapshot(): void { if (DIAGNOSTICS) void writeStore('previous', this.rows.join('\n')); }
  previous(): Promise<string> { return readStore('previous').then(copy => copy || EMPTY); }
}
