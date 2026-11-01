/** Ciphertext-only OPFS storage. Run inside a Dedicated Worker. Exclusive sync
 * handles and Web Locks protect a profile across tabs/workers; each transaction
 * replaces the older slot and flushes before the Rust core may send or acknowledge data. */
const ROOT = 'gipny-browser-v1';
const LIMIT = 16 * 1024 * 1024;
const MAX_PROFILES = 32;

type SyncFile = {
  getSize(): number;
  read(buffer: Uint8Array, options?: { at?: number }): number;
  write(buffer: Uint8Array, options?: { at?: number }): number;
  truncate(size: number): void;
  flush(): void;
  close(): void;
};

type OpfsFile = FileSystemFileHandle & { createSyncAccessHandle(): Promise<SyncFile> };
type Directory = FileSystemDirectoryHandle & { entries(): AsyncIterableIterator<[string, FileSystemHandle]> };

export function profileName(raw: string): string {
  const name = raw.normalize('NFC').trim();
  if (!name || new TextEncoder().encode(name).length > 128 || /[\u0000-\u001f\u007f]/.test(name)) {
    throw new Error('Invalid profile name');
  }
  return name;
}

async function directoryId(name: string): Promise<string> {
  const bytes = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(`gipny/browser/profile/v1\0${name}`));
  return Array.from(new Uint8Array(bytes), value => value.toString(16).padStart(2, '0')).join('');
}

function lockKey(id: string): string {
  return `gipny/browser/profile-lock/v1:${id}`;
}

async function profiles(): Promise<Directory> {
  if (!navigator.storage?.getDirectory) {
    throw new Error('This browser does not support private profile storage');
  }
  return await (await navigator.storage.getDirectory()).getDirectoryHandle(ROOT, { create: true }) as Directory;
}

function readAll(file: SyncFile, limit = LIMIT): Uint8Array {
  const size = file.getSize();
  if (!Number.isSafeInteger(size) || size < 0 || size > limit) {
    throw new Error('Profile storage size limit');
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  while (offset < size) {
    const count = file.read(bytes.subarray(offset), { at: offset });
    if (!Number.isSafeInteger(count) || count <= 0 || count > size - offset) {
      throw new Error('Incomplete profile read');
    }
    offset += count;
  }
  return bytes;
}

function tryReadSlot(file: SyncFile, limit = LIMIT): Uint8Array {
  // Oversize records are structurally invalid. Actual I/O errors must surface:
  // treating an unreadable newer slot as absent could restore an old password.
  if (file.getSize() > limit) return new Uint8Array(0);
  return readAll(file, limit);
}

function writeAll(file: SyncFile, bytes: Uint8Array): void {
  if (bytes.byteLength > LIMIT) {
    throw new Error('Profile storage size limit');
  }
  try {
    file.truncate(0);
    let offset = 0;
    while (offset < bytes.byteLength) {
      const count = file.write(bytes.subarray(offset), { at: offset });
      if (!Number.isSafeInteger(count) || count <= 0 || count > bytes.byteLength - offset) {
        throw new Error('Incomplete profile write');
      }
      offset += count;
    }
    file.truncate(bytes.byteLength);
    file.flush();
    if (file.getSize() !== bytes.byteLength) {
      throw new Error('Incomplete profile write');
    }
  } catch (error) {
    if (error instanceof DOMException && error.name === 'QuotaExceededError') {
      throw new Error('Storage quota exceeded');
    }
    throw error;
  }
}

async function label(directory: FileSystemDirectoryHandle): Promise<string> {
  const file = await (await directory.getFileHandle('profile.json')).getFile();
  if (file.size > 1024) throw new Error('Invalid profile metadata');
  const value: unknown = JSON.parse(await file.text());
  if (!value || typeof value !== 'object' || typeof (value as { name?: unknown }).name !== 'string') {
    throw new Error('Invalid profile metadata');
  }
  return profileName((value as { name: string }).name);
}

export async function listProfiles(): Promise<string[]> {
  const root = await profiles();
  const result: string[] = [];
  let count = 0;
  for await (const [id, entry] of root.entries()) {
    if (entry.kind !== 'directory' || !/^[0-9a-f]{64}$/.test(id)) continue;
    if (++count > 128) throw new Error('Too many browser profile directories');
    try {
      const directory = await root.getDirectoryHandle(id);
      const name = await label(directory);
      const hasData = await Promise.all(['a', 'b'].map(async slot => {
        try { return (await (await directory.getFileHandle(`vault-${slot}.bin`)).getFile()).size > 0; }
        catch (error) { if (error instanceof DOMException && error.name === 'NotFoundError') return false; throw error; }
      }));
      if (await directoryId(name) === id && hasData.some(Boolean)) {
        result.push(name);
      }
    } catch {
      // Empty directories or interrupted initial creations lack valid metadata.
    }
  }
  if (result.length > MAX_PROFILES) throw new Error('Too many browser profiles');
  return result.sort((left, right) => left.localeCompare(right));
}

export async function removeProfile(raw: string): Promise<void> {
  const name = profileName(raw);
  const root = await profiles();
  const id = await directoryId(name);
  if (!navigator.locks?.request) {
    throw new Error('This browser does not support Web Locks');
  }

  await navigator.locks.request(lockKey(id), { mode: 'exclusive', ifAvailable: true }, async (lock) => {
    if (!lock) {
      throw new Error('Profile is already open in another tab');
    }

    let directory: FileSystemDirectoryHandle;
    try {
      directory = await root.getDirectoryHandle(id, { create: false });
    } catch (error) {
      if (error instanceof DOMException && error.name === 'NotFoundError') {
        throw new Error('Profile not found');
      }
      throw error;
    }

    if (await label(directory) !== name) {
      throw new Error('Profile metadata mismatch');
    }

    // Verify no exclusive sync handles are held across threads before deletion.
    const handles: SyncFile[] = [];
    try {
      for (const slot of ['a', 'b'] as const) {
        try {
          const file = await directory.getFileHandle(`vault-${slot}.bin`, { create: false }) as OpfsFile;
          if (file.createSyncAccessHandle) {
            handles.push(await file.createSyncAccessHandle());
          }
        } catch (error) {
          if (error instanceof DOMException && error.name === 'NotFoundError') {
            continue;
          }
          if (error instanceof DOMException && error.name === 'NoModificationAllowedError') {
            throw new Error('Profile is already open in another tab');
          }
          throw error;
        }
      }
    } finally {
      handles.forEach(h => {
        try { h.close(); } catch {}
      });
    }

    // Deletion executes while still holding the Web Lock, avoiding any race between close and remove.
    await root.removeEntry(id, { recursive: true });
  });
}

export class ProfileStorage {
  private closed = false;
  private constructor(
    readonly name: string,
    private slots: [SyncFile, SyncFile],
    private releaseLock: () => void,
  ) {}

  static async open(raw: string, create: boolean): Promise<ProfileStorage> {
    const name = profileName(raw);
    const root = await profiles();
    const id = await directoryId(name);
    if (!navigator.locks?.request) {
      throw new Error('This browser does not support Web Locks');
    }

    return new Promise<ProfileStorage>((resolve, reject) => {
      let lockAcquired = false;

      navigator.locks.request(lockKey(id), { mode: 'exclusive', ifAvailable: true }, async (lock) => {
        if (!lock) {
          reject(new Error('Profile is already open in another tab'));
          return;
        }
        lockAcquired = true;

        let releaseHold: () => void = () => {};
        const lockHold = new Promise<void>((res) => {
          releaseHold = res;
        });

        const handles: SyncFile[] = [];
        try {
          if (create) {
            if ((await listProfiles()).length >= MAX_PROFILES) {
              throw new Error('Too many browser profiles');
            }

            let existingDir: FileSystemDirectoryHandle | null = null;
            try {
              existingDir = await root.getDirectoryHandle(id, { create: false });
            } catch {}

            if (existingDir) {
              let isCreatedProfile = false;
              try {
                const existingName = await label(existingDir);
                if (existingName !== name) throw new Error('Profile metadata mismatch');
              } catch (error) { if (!(error instanceof DOMException && error.name === 'NotFoundError')) throw error; }
              if (!isCreatedProfile) {
                for (const slot of ['a', 'b'] as const) {
                  try {
                    const fh = await existingDir.getFileHandle(`vault-${slot}.bin`, { create: false }) as OpfsFile;
                    if (fh.createSyncAccessHandle) {
                      const h = await fh.createSyncAccessHandle();
                      try {
                        if (h.getSize() > 0) isCreatedProfile = true;
                      } finally {
                        h.close();
                      }
                    }
                  } catch (error) { if (!(error instanceof DOMException && error.name === 'NotFoundError')) throw error; }
                }
              }
              if (isCreatedProfile) {
                throw new Error('Profile already exists');
              }
            }

            const directory = existingDir ?? await root.getDirectoryHandle(id, { create: true });
            for (const slot of ['a', 'b'] as const) {
              const file = await directory.getFileHandle(`vault-${slot}.bin`, { create: true }) as OpfsFile;
              if (!file.createSyncAccessHandle) throw new Error('This browser does not support private profile storage');
              handles.push(await file.createSyncAccessHandle());
            }

            const meta = await directory.getFileHandle('profile.json', { create: true }) as OpfsFile;
            if (!meta.createSyncAccessHandle) throw new Error('This browser does not support private profile storage');
            const metaHandle = await meta.createSyncAccessHandle();
            try {
              writeAll(metaHandle, new TextEncoder().encode(JSON.stringify({ name })));
            } finally {
              metaHandle.close();
            }
          } else {
            let directory: FileSystemDirectoryHandle;
            try {
              directory = await root.getDirectoryHandle(id, { create: false });
            } catch (error) {
              if (error instanceof DOMException && error.name === 'NotFoundError') {
                throw new Error('Profile not found');
              }
              throw error;
            }

            // Verify metadata and directory hash before opening or recovering slots.
            if (await label(directory) !== name) {
              throw new Error('Profile metadata mismatch');
            }

            for (const slot of ['a', 'b'] as const) {
              let file: OpfsFile;
              try {
                file = await directory.getFileHandle(`vault-${slot}.bin`, { create: false }) as OpfsFile;
              } catch (error) {
                if (error instanceof DOMException && error.name === 'NotFoundError') {
                  // Recreate missing empty slot to allow recovery of the other intact slot.
                  file = await directory.getFileHandle(`vault-${slot}.bin`, { create: true }) as OpfsFile;
                } else {
                  throw error;
                }
              }
              if (!file.createSyncAccessHandle) throw new Error('This browser does not support private profile storage');
              handles.push(await file.createSyncAccessHandle());
            }

            const hasData = handles.some(h => h.getSize() > 0);
            if (!hasData) {
              throw new Error('Profile has no saved state');
            }
          }

          const slotA = handles[0];
          const slotB = handles[1];
          if (!slotA || !slotB) {
            throw new Error('Failed to open storage slots');
          }

          const storage = new ProfileStorage(name, [slotA, slotB], releaseHold);
          resolve(storage);
        } catch (error) {
          handles.forEach(h => {
            try { h.close(); } catch {}
          });
          if (error instanceof DOMException && error.name === 'NoModificationAllowedError') {
            reject(new Error('Profile is already open in another tab'));
          } else if (error instanceof DOMException && error.name === 'QuotaExceededError') {
            reject(new Error('Storage quota exceeded'));
          } else {
            reject(error);
          }
          return;
        }

        // Keep the Web Lock held until close() resolves the lockHold promise.
        await lockHold;
      }).catch((error) => {
        if (!lockAcquired) {
          reject(error);
        }
      });
    });
  }

  readSlots(): [Uint8Array, Uint8Array] {
    if (this.closed) throw new Error('Profile is locked');
    return [tryReadSlot(this.slots[0]), tryReadSlot(this.slots[1])];
  }

  /** Only ciphertext reaches this callback. A failed write leaves the other
   * slot intact; Rust must stop the transaction instead of advancing its state. */
  persist = async (bytes: Uint8Array, generation: string): Promise<void> => {
    if (this.closed) throw new Error('Profile is locked');
    if (!/^[1-9][0-9]{0,19}$/.test(generation)) throw new Error('Invalid profile generation');
    const value = BigInt(generation);
    if (value > 0xffffffffffffffffn) throw new Error('Invalid profile generation');
    const target = Number(value % 2n) === 0 ? this.slots[0] : this.slots[1];
    writeAll(target, bytes);
  };

  close(): void {
    if (this.closed) return;
    this.closed = true;
    try {
      this.slots[0].close();
    } finally {
      try {
        this.slots[1].close();
      } finally {
        this.releaseLock();
      }
    }
  }
}
