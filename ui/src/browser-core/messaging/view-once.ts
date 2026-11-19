import type { BrowserEngine, ViewOnceLetter, ViewOnceOffer } from '../../../../browser-router/pkg/gipny_browser_router.js';
import type { ViewOnceNotice } from '../../app/api';
import type { BrowserState } from '../profiles/model';
import { EnvelopeParts, splitEnvelope } from '../networking/envelope-parts';
import { hex } from '../profiles/model';

export type ViewOnceRequest = { to: number[]; id: number[]; nonce: number[]; expires_at: number };
export type ReceivingViewOnce = { bytes: Uint8Array; accept(envelope: Uint8Array): Promise<void>; close(): void };
export const VIEW_ONCE_BODY_LIMIT = 16 * 1024;
const limits = { envelope: VIEW_ONCE_BODY_LIMIT + 2048, part: 4096 };
export const viewOnceParts = (envelope: Uint8Array) => splitEnvelope(envelope, limits);
export class ViewOnceParts extends EnvelopeParts { constructor() { super(limits); } }
export type ReplayMarker = number | { peer: string; expires_at: number };
export const markerEnd = (marker: ReplayMarker | undefined): number =>
  typeof marker === 'number' ? marker : marker?.expires_at ?? 0;
export type ViewOnceOfferReply = ReceivingViewOnce | { recorded: Promise<void> };
const INBOX_LIMIT = 64;
const OFFER_LIFETIME = 60_000;
const invalid = (): Error => new Error('View-once message is unavailable');

export function pruneViewOnceMarkers(state: BrowserState, now: number): void {
  if (!state.view_once_seen) return;
  for (const [id, end] of Object.entries(state.view_once_seen)) if (markerEnd(end) <= now) delete state.view_once_seen[id];
}

/** The worker owns unread envelopes and their opening keys as opaque WASM
 * handles. Only stable replay markers cross the profile storage boundary. */
export class ViewOnceInbox {
  private unread = new Map<string, { info: ViewOnceNotice; peer: string; dh: string; letter: ViewOnceLetter }>();
  private offers = new Map<string, { contactId: number; end: number; close(): void }>();

  offer(engine: BrowserEngine, peer: string, dh: string, contactId: number, request: ViewOnceRequest,
    record: (id: string, expiresAt: number) => Promise<void>, notify: (info: ViewOnceNotice) => void): ReceivingViewOnce {
    const now = Date.now(), id = hex(request.id);
    const end = Math.min(now + OFFER_LIFETIME, request.expires_at);
    this.prune(now);
    if (now >= end || this.unread.size + this.offers.size >= INBOX_LIMIT || this.unread.has(id) || this.offers.has(id)) throw invalid();
    let offer: ViewOnceOffer | null = engine.view_once_offer(peer, dh, new Uint8Array(request.id), new Uint8Array(request.nonce), request.expires_at);
    let accepted: ViewOnceLetter | null = null;
    const close = (): void => {
      offer?.free(); offer = null;
      accepted?.free(); accepted = null;
      if (this.offers.get(id)?.close === close) this.offers.delete(id);
    };
    this.offers.set(id, { contactId, end, close });
    try {
      return { bytes: offer.bytes(), close, accept: async envelope => {
        if (!offer || Date.now() >= end) throw invalid();
        const current = offer; offer = null; // WASM consumes the handle, including on error.
        accepted = engine.view_once_accept(current, envelope);
        const info = { id, contact_id: contactId, sent_at: Date.now(), expires_at: request.expires_at };
        await record(id, info.expires_at);
        if (!accepted || Date.now() >= info.expires_at) throw invalid();
        this.unread.set(id, { info, peer, dh, letter: accepted }); accepted = null;
        this.offers.delete(id);
        notify(info);
      } };
    } catch (error) { close(); throw error; }
  }

  list(contactId: number): ViewOnceNotice[] {
    this.prune(Date.now());
    return [...this.unread.values()].filter(value => value.info.contact_id === contactId).map(value => value.info);
  }

  open(engine: BrowserEngine, id: string, allowed: (contactId: number, peer: string, dh: string) => boolean): string {
    this.prune(Date.now());
    const entry = this.unread.get(id);
    if (!entry) throw invalid();
    if (!allowed(entry.info.contact_id, entry.peer, entry.dh)) throw invalid();
    this.unread.delete(id); // Consume only after trust preflight and before any plaintext release.
    const body = engine.view_once_open(entry.letter);
    try { return new TextDecoder('utf-8', { fatal: true }).decode(body); }
    finally { body.fill(0); }
  }

  prune(now: number, allowed?: (contactId: number) => boolean): void {
    for (const item of this.offers.values()) if (now >= item.end || (allowed && !allowed(item.contactId))) item.close();
    for (const [id, entry] of this.unread) if (now >= entry.info.expires_at || (allowed && !allowed(entry.info.contact_id))) {
      this.unread.delete(id); entry.letter.free();
    }
  }

  clear(): void {
    for (const item of this.offers.values()) item.close();
    for (const entry of this.unread.values()) entry.letter.free();
    this.unread.clear();
  }
}
