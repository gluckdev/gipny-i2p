/** Ring-buffer action log for the browser core. Every line is written to the
 * worker console (`DevTools → Consoles → [worker]`, Debug/Verbose levels) and
 * kept in memory so `read_debug_log` / `read_previous_log` can serve the
 * in-app viewer. Message bodies never enter the log — ids, key prefixes and
 * byte counts only. */
const CAP = 2048;
const EMPTY = '(журнал пуст)';

export const shortKey = (key: string): string => key.slice(0, 12);
export const shortAddress = (address: string): string => address.slice(0, 24);

function stamp(date = new Date()): string {
  const two = (n: number): string => String(n).padStart(2, '0');
  return `${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}.${String(date.getMilliseconds()).padStart(3, '0')}`;
}

type Level = 'debug' | 'info';

function readStore(key: string): Promise<string> {
  return new Promise(resolve => {
    if (!('indexedDB' in globalThis)) return resolve('');
    const open = indexedDB.open('gipny-browser-log', 1);
    open.onupgradeneeded = () => open.result.createObjectStore('logs');
    open.onerror = () => resolve('');
    open.onsuccess = () => {
      const db = open.result;
      const row = db.transaction('logs').objectStore('logs').get(key);
      row.onerror = () => { db.close(); resolve(''); };
      row.onsuccess = () => { db.close(); resolve(typeof row.result === 'string' ? row.result : ''); };
    };
  });
}

function writeStore(key: string, value: string): Promise<void> {
  return new Promise(resolve => {
    if (!('indexedDB' in globalThis)) return resolve();
    const open = indexedDB.open('gipny-browser-log', 1);
    open.onupgradeneeded = () => open.result.createObjectStore('logs');
    open.onerror = () => resolve();
    open.onsuccess = () => {
      const db = open.result;
      const row = db.transaction('logs', 'readwrite').objectStore('logs').put(value, key);
      row.onerror = () => { db.close(); resolve(); };
      row.onsuccess = () => { db.close(); resolve(); };
    };
  });
}

export class ActionLog {
  private enabled = true;
  private rows: string[] = [];

  debug(action: string, detail = ''): void { this.write('debug', action, detail); }
  info(action: string, detail = ''): void { this.write('info', action, detail); }

  private write(level: Level, action: string, detail: string): void {
    if (!this.enabled) return;
    const line = `[${stamp()}] [gipny-browser] ${action}${detail ? ' · ' + detail : ''}`;
    console[level](line);
    this.rows.push(line);
    if (this.rows.length > CAP) this.rows.splice(0, this.rows.length - CAP);
  }

  get isEnabled(): boolean { return this.enabled; }

  /** Matches the Tauri core's toggle semantics: switching off erases the log. */
  setEnabled(on: boolean): void { this.enabled = on; if (!on) this.rows = []; }
  clear(): void { this.rows = []; }
  text(): string { return this.rows.join('\n') || EMPTY; }

  /** The ring buffer dies with the worker, so the viewer needs a copy of the
   * current session in IndexedDB to reset it to the previous session. */
  snapshot(): void { void writeStore('previous', this.rows.join('\n')); }
  previous(): Promise<string> { return readStore('previous').then(copy => copy || EMPTY); }
}
