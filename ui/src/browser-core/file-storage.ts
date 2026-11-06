import type { SyncFile } from './storage';
import { hex, unhex, type StoredBlob } from './model';

export const MAX_FILE_BYTES = 2 * 1024 * 1024 * 1024;
export const STORED_CHUNK = 192 * 1024;
export const FILE_PREVIEW_LIMIT = 32 * 1024 * 1024;
const OVERHEAD = 40;
const RESERVE = 64 * 1024 * 1024;
type Directory = FileSystemDirectoryHandle & { entries(): AsyncIterableIterator<[string, FileSystemHandle]> };
export interface FileCrypto {
  new_attachment_key(id: string): Uint8Array;
  seal_attachment_part(id: string, key: Uint8Array, index: number, data: Uint8Array): Uint8Array;
  open_attachment_part(id: string, key: Uint8Array, index: number, data: Uint8Array): Uint8Array;
}
export function storedSize(size: number, chunk = STORED_CHUNK): number { return size + Math.max(1, Math.ceil(size / chunk)) * OVERHEAD; }

/** Opening an OPFS access handle did not finish in time (live runs showed it
 * hanging): the operation fails as retryable instead of blocking every later
 * read and write of that file, and the late handle is closed on arrival. */
export class StorageBusy extends Error {
  constructor() { super('Attachment storage is busy'); }
}
export type StorageTiming = { open: number; idle: number };
/** A handle stays open between parts and closes after `idle` ms unused. */
const TIMING: StorageTiming = { open: 10_000, idle: 10_000 };

/** estimate() is advisory. Every actual write still checks/propagates quota
 * failure. Leave room for both vault slots, receipts and the offline shell. */
export function availableFileBytes(estimate: StorageEstimate, pending = 0): number {
  if (!Number.isFinite(estimate.quota) || !Number.isFinite(estimate.usage)) throw new Error('Browser storage quota is unavailable');
  return Math.max(0, estimate.quota! - estimate.usage! - RESERVE - pending);
}

/** Fixed slots contain nonce + AEAD ciphertext. No plaintext file, thumbnail,
 * key, filename or incoming temporary fragment is written to OPFS. */
export class FileStorage {
  private closed = false;
  private handles = new Map<string, Promise<SyncFile>>();
  private idle = new Map<string, ReturnType<typeof setTimeout>>();
  private queue = new Map<string, Promise<unknown>>();
  private constructor(private directory: Directory, private engine: FileCrypto, private timing: StorageTiming) {}
  static async open(profile: FileSystemDirectoryHandle, engine: FileCrypto, timing: StorageTiming = TIMING): Promise<FileStorage> {
    return new FileStorage(await profile.getDirectoryHandle('attachments', { create: true }) as Directory, engine, timing);
  }
  private check(): void { if (this.closed) throw new Error('Profile is locked'); }
  private serial<T>(id: string, action: () => Promise<T>): Promise<T> {
    const result = (this.queue.get(id) ?? Promise.resolve()).catch(() => {}).then(() => { this.check(); return action(); });
    this.queue.set(id, result);
    void result.finally(() => { if (this.queue.get(id) === result) this.queue.delete(id); }).catch(() => {});
    return result;
  }
  private async handle(id: string, create: boolean): Promise<SyncFile> {
    this.check();
    if (!/^[0-9a-f]{64}$/.test(id)) throw new Error('Invalid attachment storage id');
    clearTimeout(this.idle.get(id)); this.idle.delete(id);
    let pending = this.handles.get(id);
    if (!pending) {
      let late = false, timer: ReturnType<typeof setTimeout> | undefined;
      const opening = (async () => {
        const file = await this.directory.getFileHandle(id, { create }) as FileSystemFileHandle & { createSyncAccessHandle(): Promise<SyncFile> };
        const handle = await file.createSyncAccessHandle().catch((error: unknown) => {
          throw error instanceof DOMException && error.name === 'NoModificationAllowedError' ? new StorageBusy() : error;
        });
        if (this.closed || late) { handle.close(); throw new Error(late ? 'Abandoned attachment handle' : 'Profile is locked'); }
        return handle;
      })();
      const expired = new Promise<never>((_, reject) => { timer = setTimeout(() => { late = true; reject(new StorageBusy()); }, this.timing.open); });
      pending = Promise.race([opening, expired]).finally(() => clearTimeout(timer));
      opening.catch(() => {});
      this.handles.set(id, pending);
      pending.catch(() => { if (this.handles.get(id) === pending) this.handles.delete(id); });
    }
    return pending;
  }
  /** After an operation: a good handle stays open for the next part. */
  private keep(id: string): void {
    clearTimeout(this.idle.get(id));
    this.idle.set(id, setTimeout(() => {
      this.idle.delete(id);
      void this.serial(id, () => this.release(id)).catch(() => {});
    }, this.timing.idle));
  }
  private async release(id: string): Promise<void> {
    clearTimeout(this.idle.get(id)); this.idle.delete(id);
    const pending = this.handles.get(id); this.handles.delete(id);
    if (pending) { try { (await pending).close(); } catch { /* A failed open has no handle. */ } }
  }
  async create(size: number, chunk = STORED_CHUNK, pending = 0): Promise<StoredBlob> {
    this.check();
    if (!Number.isSafeInteger(size) || size < 0 || size > MAX_FILE_BYTES || !Number.isSafeInteger(chunk) || chunk < 1 || chunk > STORED_CHUNK || Math.ceil(size / chunk) > 10923) throw new Error('File size limit is 2 GiB');
    if (storedSize(size, chunk) > availableFileBytes(await navigator.storage.estimate(), pending)) throw new Error('Not enough browser storage for this file');
    this.check();
    const id = hex(crypto.getRandomValues(new Uint8Array(32)));
    const key = hex(this.engine.new_attachment_key(id));
    try {
      // Reserve the complete ciphertext length in OPFS now. A sender cannot
      // promise the same free quota to several simultaneous incoming files.
      await this.serial(id, () => this.using(id, true, h => { h.truncate(storedSize(size, chunk)); h.flush(); }));
    } catch (error) { await this.remove(id).catch(() => {}); throw error; }
    return { id, key, size, chunk_size: chunk };
  }
  private length(file: StoredBlob, index: number): number {
    if (!Number.isSafeInteger(index) || index < 0 || index >= Math.max(1, Math.ceil(file.size / file.chunk_size))) throw new Error('File part outside attachment');
    return Math.min(file.chunk_size, Math.max(0, file.size - index * file.chunk_size));
  }
  async write(file: StoredBlob, index: number, data: Uint8Array): Promise<void> {
    return this.serial(file.id, () => this.writePart(file, index, data));
  }
  private async writePart(file: StoredBlob, index: number, data: Uint8Array): Promise<void> {
    this.check();
    if (data.length !== this.length(file, index)) throw new Error('File part size mismatch');
    const sealed = this.engine.seal_attachment_part(file.id, new Uint8Array(unhex(file.key, 72)), index, data);
    await this.using(file.id, true, handle => {
      let offset = 0;
      while (offset < sealed.length) {
        const n = handle.write(sealed.subarray(offset), { at: index * (file.chunk_size + OVERHEAD) + offset });
        if (!Number.isSafeInteger(n) || n <= 0 || n > sealed.length - offset) throw new Error('Incomplete attachment write');
        offset += n;
      }
      handle.flush();
    });
  }
  /** One synchronous step on the file's handle (inside `serial`). */
  private async using<T>(id: string, create: boolean, step: (handle: SyncFile) => T): Promise<T> {
    const handle = await this.handle(id, create);
    let ok = false;
    try { const value = step(handle); ok = true; return value; }
    finally { if (ok && !this.closed) this.keep(id); else await this.release(id); }
  }
  async read(file: StoredBlob, index: number): Promise<Uint8Array> {
    return this.serial(file.id, () => this.readPart(file, index));
  }
  private async readPart(file: StoredBlob, index: number): Promise<Uint8Array> {
    this.check();
    const bytes = new Uint8Array(this.length(file, index) + OVERHEAD);
    await this.using(file.id, false, handle => {
      let offset = 0;
      while (offset < bytes.length) {
        const n = handle.read(bytes.subarray(offset), { at: index * (file.chunk_size + OVERHEAD) + offset });
        if (!Number.isSafeInteger(n) || n <= 0 || n > bytes.length - offset) throw new Error('Incomplete attachment read');
        offset += n;
      }
    });
    this.check();
    return this.engine.open_attachment_part(file.id, new Uint8Array(unhex(file.key, 72)), index, bytes);
  }
  async remove(id: string): Promise<void> {
    return this.serial(id, async () => {
      this.check(); await this.release(id);
      try { await this.directory.removeEntry(id); }
      catch (error) { if (!(error instanceof DOMException && error.name === 'NotFoundError')) throw error; }
    });
  }
  async collect(keep: ReadonlySet<string>): Promise<void> {
    for await (const [name, entry] of this.directory.entries()) {
      this.check();
      if (entry.kind === 'file' && /^[0-9a-f]{64}$/.test(name) && !keep.has(name)) await this.remove(name);
    }
  }
  close(): void {
    this.closed = true;
    for (const timer of this.idle.values()) clearTimeout(timer);
    this.idle.clear();
    for (const id of [...this.handles.keys()]) void this.release(id);
  }
}
