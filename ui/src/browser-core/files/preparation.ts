import { AttachmentHasher, attachment_passed_name, attachment_passes_through, clean_attachment } from '../../../../browser-router/pkg/gipny_browser_router.js';
import type { PendingAttachment } from '../../app/api';
import { unbase64, type BrowserState, type StoredBlob } from '../profiles/model';
import { FILE_PREVIEW_LIMIT, MAX_FILE_BYTES, type FileStorage } from './storage';
import { FILE_CHUNK, fileReservations } from './transfers';

export type PreparedFile = { name: string; size: number; sha256: number[]; storage: StoredBlob };

/** Stage inline bytes in encrypted OPFS; wipe the buffer after writing parts. */
export async function storeBytes(store: FileStorage, data: Uint8Array, state: BrowserState): Promise<StoredBlob> {
  const file = await store.create(data.length, FILE_CHUNK, fileReservations(state));
  try {
    for (let index = 0; index < Math.max(1, Math.ceil(data.length / FILE_CHUNK)); index++) await store.write(file, index, data.subarray(index * FILE_CHUNK, (index + 1) * FILE_CHUNK));
    return file;
  } catch (error) { await store.remove(file.id).catch(() => {}); throw error; }
  finally { data.fill(0); }
}

/** Worker-only staging. The backend owns the preparation gate and supplies live
 * profile checks/reservations so lock and rollback cannot use a stale snapshot. */
export async function prepareFiles(
  store: FileStorage,
  inline: PendingAttachment[],
  picked: File[],
  sanitize: boolean,
  active: () => void,
  state: () => BrowserState,
): Promise<PreparedFile[]> {
  const result: PreparedFile[] = [];
  const created: string[] = [];
  try {
    const files: File[] = [...picked, ...inline.map(file => new File([new Uint8Array(unbase64(file.data, FILE_PREVIEW_LIMIT))], file.name))];
    for (let source of files) {
      active();
      if (source.size > MAX_FILE_BYTES || new TextEncoder().encode(source.name).length > 1024) throw new Error('File size limit is 2 GiB');
      let name = source.name;
      if (sanitize) {
        const head = new Uint8Array(await source.slice(0, 1024).arrayBuffer()); active();
        if (attachment_passes_through(name, head)) name = attachment_passed_name(name);
        else {
          if (source.size > FILE_PREVIEW_LIMIT) throw new Error('Metadata cleaning is limited to 32 MiB; convert this file or explicitly turn off attachment metadata cleaning');
          const bytes = new Uint8Array(await source.arrayBuffer()); active();
          let data: Uint8Array;
          try {
            const clean = clean_attachment(name, bytes);
            try { name = clean.name; data = clean.take_data(); } finally { clean.free(); }
          } finally { bytes.fill(0); }
          source = new File([data as BlobPart], name); data.fill(0);
        }
        head.fill(0);
      }
      const storage = await store.create(source.size, FILE_CHUNK, fileReservations(state())); created.push(storage.id);
      let hash: AttachmentHasher | null = new AttachmentHasher();
      try {
        for (let index = 0; index < Math.max(1, Math.ceil(source.size / FILE_CHUNK)); index++) {
          active();
          const data = new Uint8Array(await source.slice(index * FILE_CHUNK, (index + 1) * FILE_CHUNK).arrayBuffer()); active();
          try { hash.update(data); await store.write(storage, index, data); } finally { data.fill(0); }
        }
        const sha256 = [...hash.finish()]; hash = null;
        result.push({ name, size: source.size, sha256, storage });
      } finally { hash?.free(); }
    }
    active(); return result;
  } catch (error) {
    for (const id of created) await store.remove(id).catch(() => {});
    throw error;
  }
}
