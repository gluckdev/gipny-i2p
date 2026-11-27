// Browser archive v1: magic, LE u32 vault length, password-sealed WASM vault,
// then the authenticated parts of each distinct file in metadata order. The
// sealed metadata binds the file keys, sizes, chunk layout and partial bitmaps.
import { BrowserEngine } from '../../../../browser-router/pkg/gipny_browser_router.js';
import { FILE_PART_OVERHEAD, FILE_PREVIEW_LIMIT, FileStorage, MAX_FILE_BYTES, STORED_CHUNK } from '../files/storage';
import { fromBase64, hasBit, hex, loadState, unhex, type BrowserState, type StoredBlob } from './model';
import { publicIdentity } from './public-identity';
import { ProfileStorage, VAULT_RECORD_LIMIT, profileName } from './storage';

const MAGIC = 'GIPNYBA1';
const PREFIX = 12;
const ARCHIVE_PROFILE = 'gipny-browser-backup/v1';
const invalid = (): Error => new Error('Invalid or incomplete browser backup');
type Reference = { storage?: StoredBlob | null };
type FileEntry = { file: StoredBlob; references: Reference[]; held: Uint8Array | null; count: number };

function filesIn(state: BrowserState): FileEntry[] {
  const entries = new Map<string, FileEntry>();
  const add = (ref: Reference, size: number, held?: string): void => {
    const file = ref.storage;
    if (!file || !/^[0-9a-f]{64}$/.test(file.id) || !/^[0-9a-f]{144}$/.test(file.key)
        || file.size !== size || !Number.isSafeInteger(size) || size < 0 || size > MAX_FILE_BYTES
        || !Number.isSafeInteger(file.chunk_size) || file.chunk_size < 1 || file.chunk_size > STORED_CHUNK) throw invalid();
    const count = Math.max(1, Math.ceil(size / file.chunk_size));
    if (count > 10923) throw invalid();
    const bitBytes = Math.ceil(count / 8);
    if (held && held.length !== 4 * Math.ceil(bitBytes / 3)) throw invalid();
    const bits = held === undefined ? null : held ? fromBase64(held) : new Uint8Array(bitBytes);
    if (bits && (bits.length !== bitBytes || (count % 8 && bits[bitBytes - 1]! >> (count % 8)))) throw invalid();
    const existing = entries.get(file.id);
    if (!existing) { entries.set(file.id, { file, references: [ref], held: bits, count }); return; }
    if (existing.file.key !== file.key || existing.file.size !== size || existing.file.chunk_size !== file.chunk_size) throw invalid();
    existing.references.push(ref);
    if (!bits) existing.held = null;
    else if (existing.held) bits.forEach((byte, i) => { existing.held![i] = existing.held![i]! | byte; });
  };
  for (const attachment of state.attachments) {
    if (attachment.storage) add(attachment, attachment.size);
    else {
      if (typeof attachment.data !== 'string' || attachment.size > FILE_PREVIEW_LIMIT || attachment.data.length > 4 * Math.ceil(FILE_PREVIEW_LIMIT / 3)) throw invalid();
      const bytes = fromBase64(attachment.data);
      bytes.fill(0);
      if (bytes.length !== attachment.size) throw invalid();
    }
  }
  for (const transfer of state.transfers) {
    if (!transfer.storage) continue;
    if (transfer.storage.chunk_size !== transfer.chunk_size) throw invalid();
    add(transfer, transfer.size, transfer.incoming && transfer.status === 'receiving' ? transfer.held : undefined);
  }
  return [...entries.values()];
}

function* parts(entry: FileEntry): Generator<{ index: number; size: number }> {
  const { file, held, count } = entry;
  for (let index = 0; index < count; index++) {
    if (!held || hasBit(held, index)) yield { index, size: Math.min(file.chunk_size, file.size - index * file.chunk_size) };
  }
}

function close(engine: BrowserEngine): void {
  try { engine.close(); } finally { engine.free(); }
}

export async function exportBackup(
  engine: BrowserEngine, current: BrowserState, files: FileStorage, password: string, active: () => void,
): Promise<Blob> {
  const archive = engine.fork_for_backup(ARCHIVE_PROFILE, password);
  try {
    const state = structuredClone(current);
    const ciphertext: Blob[] = [];
    for (const entry of filesIn(state)) {
      const { file } = entry;
      const token = archive.new_attachment_key(file.id);
      for (const { index } of parts(entry)) {
        active();
        const bytes = await files.read(file, index);
        try {
          active();
          const sealed = archive.seal_attachment_part(file.id, token, index, bytes);
          ciphertext.push(new Blob([new Uint8Array(sealed)]));
        } finally { bytes.fill(0); }
      }
      const storage = { ...file, key: hex(token) };
      for (const ref of entry.references) ref.storage = storage;
    }
    active();
    archive.begin();
    const record = archive.seal(JSON.stringify(state));
    const prefix = new Uint8Array(PREFIX);
    prefix.set(new TextEncoder().encode(MAGIC));
    new DataView(prefix.buffer).setUint32(8, record.length, true);
    return new Blob([prefix, new Uint8Array(record), ...ciphertext], { type: 'application/octet-stream' });
  } finally { close(archive); }
}

export async function importBackup(
  input: Blob, password: string, rawName: string, vaultPass: string, active: () => void,
): Promise<void> {
  const name = profileName(rawName);
  const header = new Uint8Array(await input.slice(0, PREFIX).arrayBuffer());
  if (header.length !== PREFIX || new TextDecoder().decode(header.subarray(0, 8)) !== MAGIC) throw new Error('Expected a gipny browser backup');
  const length = new DataView(header.buffer).getUint32(8, true);
  if (!length || length > VAULT_RECORD_LIMIT || PREFIX + length > input.size) throw invalid();
  const record = new Uint8Array(await input.slice(PREFIX, PREFIX + length).arrayBuffer());
  active();
  const archive = BrowserEngine.open_password(ARCHIVE_PROFILE, password, record, new Uint8Array());
  try {
    const state = loadState(archive.take_metadata());
    publicIdentity(archive.public_info());
    const entries = filesIn(state);
    let offset = PREFIX + length;
    let expected = offset;
    for (const entry of entries) {
      for (const part of parts(entry)) expected += part.size + FILE_PART_OVERHEAD;
      if (expected > input.size) throw invalid();
    }
    if (expected !== input.size) throw invalid();

    await ProfileStorage.restore(name, async storage => {
      active();
      const destination = archive.fork_for_backup(name, vaultPass);
      try {
        const files = await FileStorage.open(storage.directory, destination);
        try {
          for (const entry of entries) {
            active();
            const file = entry.file;
            const target = await files.create(file.size, file.chunk_size);
            const token = new Uint8Array(unhex(file.key, 72));
            for (const { index, size } of parts(entry)) {
              const end = offset + size + FILE_PART_OVERHEAD;
              const sealed = new Uint8Array(await input.slice(offset, end).arrayBuffer());
              active();
              const bytes = archive.open_attachment_part(file.id, token, index, sealed);
              try { await files.write(target, index, bytes); } finally { bytes.fill(0); }
              offset = end;
            }
            for (const ref of entry.references) ref.storage = target;
          }
          active();
          destination.begin();
          const sealed = destination.seal(JSON.stringify(state));
          await storage.persist(sealed, destination.pending_generation(), true);
          destination.commit();
        } finally { await files.close(); }
      } finally { close(destination); }
    }, active);
  } finally { close(archive); }
}
