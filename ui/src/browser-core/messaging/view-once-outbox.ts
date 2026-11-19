import type { BrowserEngine } from '../../../../browser-router/pkg/gipny_browser_router.js';
import { hex, retryDelay } from '../profiles/model';
import { VIEW_ONCE_BODY_LIMIT } from './view-once';

type Cipher = Pick<BrowserEngine, 'new_attachment_key' | 'seal_attachment_part' | 'open_attachment_part'>;
export type WaitingLetter = {
  id: string; contactId: number; peer: string; dh: string; expiresAt: number; deadline: number;
  storageId: string; key: Uint8Array; parts: Uint8Array[]; size: number;
  committed: boolean; sending: boolean; attempts: number; next: number;
};

/** Owned exclusively by the unlocked worker. No storage adapter or snapshot references these parts. */
export class ViewOnceOutbox {
  private letters = new Map<string, WaitingLetter>();

  add(engine: Cipher, id: string, contactId: number, peer: string, dh: string, expiresAt: number, deadline: number, body: Uint8Array): void {
    if (!body.length || body.length > VIEW_ONCE_BODY_LIMIT || this.letters.size >= 64 || this.letters.has(id)) throw new Error('View-once queue limit');
    const storageId = hex(crypto.getRandomValues(new Uint8Array(32)));
    const key = engine.new_attachment_key(storageId);
    const parts: Uint8Array[] = [];
    try {
      for (let offset = 0; offset < body.length; offset += 4096) {
        parts.push(engine.seal_attachment_part(storageId, key, parts.length, body.subarray(offset, offset + 4096)));
      }
      this.letters.set(id, { id, contactId, peer, dh, expiresAt, deadline, storageId, key, parts, size: body.length, committed: false, sending: false, attempts: 0, next: 0 });
    } catch (error) {
      key.fill(0);
      for (const part of parts) part.fill(0);
      throw error;
    }
  }

  activate(id: string): void {
    const entry = this.letters.get(id);
    if (entry) entry.committed = true;
  }

  remove(id: string): void {
    const letter = this.letters.get(id);
    if (!letter) return;
    this.letters.delete(id);
    letter.key.fill(0);
    for (const part of letter.parts) part.fill(0);
  }

  clear(): void { for (const id of this.letters.keys()) this.remove(id); }

  prune(keep: (entry: WaitingLetter) => boolean): void {
    for (const entry of this.letters.values()) if (!keep(entry)) this.remove(entry.id);
  }

  wake(peer: string): boolean {
    let waiting = false;
    for (const entry of this.letters.values()) if (entry.peer === peer && !entry.sending) {
      entry.next = 0;
      waiting = true;
    }
    return waiting;
  }

  due(now: number): WaitingLetter | undefined {
    // A silent terminal may consume a stream until its handshake timeout.
    // Keep other recipients eligible while limiting concurrent dials.
    const active = new Set([...this.letters.values()].filter(entry => entry.sending).map(entry => entry.peer));
    if (active.size >= 4) return;
    let selected: WaitingLetter | undefined;
    for (const entry of this.letters.values()) {
      if (active.has(entry.peer)) continue;
      if (entry.committed && entry.deadline > now && entry.next <= now && (!selected || entry.next < selected.next)) selected = entry;
    }
    if (selected) {
      selected.sending = true;
      selected.next = now + retryDelay(++selected.attempts);
    }
    return selected;
  }

  open(engine: Cipher, entry: WaitingLetter): Uint8Array {
    if (this.letters.get(entry.id) !== entry || Date.now() >= entry.deadline) throw new Error('View-once letter expired');
    const body = new Uint8Array(entry.size);
    let offset = 0;
    try {
      for (const [index, part] of entry.parts.entries()) {
        const plain = engine.open_attachment_part(entry.storageId, entry.key, index, part);
        try { body.set(plain, offset); offset += plain.length; } finally { plain.fill(0); }
      }
      return body;
    } catch (error) { body.fill(0); throw error; }
  }
}
