import {
  BrowserEngine, type BrowserNetwork, type BrowserStream,
  encode_client_frame, decode_client_frame, encode_relay_frame, decode_relay_frame,
} from '../../../../browser-router/pkg/gipny_browser_router.js';
import { hex, unhex, type PublicIdentity } from '../profiles/model';
import { answerWithin } from './deadline';
import { ViewOnceParts, viewOnceParts, type ViewOnceOfferReply, type ViewOnceRequest } from '../messaging/view-once';
import { DEPOSIT_LIMITS, EnvelopeParts, splitEnvelope, type EnvelopePart } from './envelope-parts';

type Login = { sign_pk: number[]; signature: number[] };
type ClientFrame = 'Ping' | { AuthV2: Login } | { AuthFramedV1: Login & { nonce: number[] } }
  | { GetBundle: { pk: number[] } } | { Send: { to: number[]; blob: number[] } }
  | { Publish: { bundle: number[] } } | { Ack: { id: number } }
  | { ViewOnceOfferV1: ViewOnceRequest } | { ViewOncePartV1: EnvelopePart } | { SendPartV1: EnvelopePart & { to: number[] } };
type RelayFrame = 'AuthOk' | { AuthFramedOkV1: { proof: number[] } } | 'AuthFail' | 'Pong' | { Challenge: number[] }
  | { Bundle: { pk: number[]; bundle: number[] | null } } | { Deposited: { id: number } }
  | { SendPartAckV1: { index: number } } | { Incoming: { id: number; from: number[]; blob: number[] } }
  | { Error: string } | { ViewOnceReadyV1: { offer: number[] } } | { ViewOncePartAckV1: { index: number } } | { ViewOnceStoredV1: { id: number[] } };

/** One process hosts only its owner. There is no open mailbox service or
 * plaintext connection to another local application. The native relay codec,
 * frame bounds and authentication context are supplied by Rust. */
export class BrowserRelay {
  private closed = false;
  private streams = new Set<BrowserStream>();
  private idle: { address: string; peer: string; stream: BrowserStream; timer: ReturnType<typeof setTimeout> }[] = [];
  private confirmed = new Map<string, string>();
  private serving = 0;
  private sequence = 1;
  constructor(private engine: BrowserEngine, private network: BrowserNetwork, private identity: PublicIdentity,
    private incoming: (from: string, blob: Uint8Array) => Promise<void>, private failure: (reason: string) => void,
    private peerPing: (peer: string) => void,
    private viewOnceOffer?: (peer: string, request: ViewOnceRequest) => ViewOnceOfferReply,
    private terminalCandidates?: (peer: string) => string[]) {}

  start(): void { void this.acceptLoop(); }
  close(): void {
    this.closed = true;
    for (const entry of this.idle) { clearTimeout(entry.timer); this.release(entry.stream); }
    this.idle = [];
    for (const stream of this.streams) stream.close();
    this.streams.clear();
    this.network.close();
  }
  private release(stream: BrowserStream): void {
    this.streams.delete(stream);
    stream.close();
    stream.free();
  }
  private async acceptLoop(): Promise<void> {
    try {
      while (!this.closed) {
        const stream = await this.network.accept();
        if (this.closed || this.serving >= 8) { stream.close(); stream.free(); continue; }
        this.streams.add(stream);
        this.serving++;
        void this.serve(stream).catch(() => { /* A malformed/aborted peer closes its stream only. */ })
          .finally(() => { this.serving--; this.release(stream); });
      }
    } catch (error) { if (!this.closed) this.failure(String(error)); }
  }
  private async readClient(stream: BrowserStream): Promise<ClientFrame> { return JSON.parse(decode_client_frame(await stream.read_frame())) as ClientFrame; }
  private async writeRelay(stream: BrowserStream, frame: RelayFrame): Promise<void> { await stream.write_frame(encode_relay_frame(JSON.stringify(frame))); }
  private async readRelay(stream: BrowserStream): Promise<RelayFrame> {
    const frame = JSON.parse(decode_relay_frame(await stream.read_frame())) as RelayFrame;
    if (typeof frame === 'object' && 'Error' in frame) throw new Error(frame.Error);
    return frame;
  }
  private async writeClient(stream: BrowserStream, frame: ClientFrame): Promise<void> { await stream.write_frame(encode_client_frame(JSON.stringify(frame))); }
  private async serve(stream: BrowserStream): Promise<void> {
    const challenge = crypto.getRandomValues(new Uint8Array(32));
    await this.writeRelay(stream, { Challenge: [...challenge] });
    const auth = await this.readClient(stream);
    const framed = typeof auth === 'object' && 'AuthFramedV1' in auth;
    const login = framed ? auth.AuthFramedV1 : typeof auth === 'object' && 'AuthV2' in auth ? auth.AuthV2 : null;
    const nonce = framed ? new Uint8Array(auth.AuthFramedV1.nonce) : null;
    const verified = login && (nonce
      ? BrowserEngine.verify_framed_auth(hex(login.sign_pk), this.identity.i2p_address, challenge, nonce, new Uint8Array(login.signature))
      : BrowserEngine.verify_auth(hex(login.sign_pk), this.identity.i2p_address, challenge, new Uint8Array(login.signature)));
    if (!login || !verified) {
      await this.writeRelay(stream, 'AuthFail');
      return;
    }
    const from = hex(login.sign_pk);
    await this.writeRelay(stream, nonce ? { AuthFramedOkV1: { proof: [...this.engine.terminal_proof(from, challenge, nonce)] } } : 'AuthOk');
    while (!this.closed) {
      const frame = await this.readClient(stream);
      if (frame === 'Ping') {
        await this.writeRelay(stream, 'Pong');
        if (!this.closed) this.peerPing(from);
        continue;
      }
      if (typeof frame !== 'object') throw new Error('Unexpected relay frame');
      if ('GetBundle' in frame) {
        const served = hex(frame.GetBundle.pk) === this.identity.sign_pk;
        await this.writeRelay(stream, { Bundle: { pk: frame.GetBundle.pk, bundle: served ? [...this.engine.my_bundle()] : null } });
      } else if ('SendPartV1' in frame) {
        const parts = new EnvelopeParts(DEPOSIT_LIMITS);
        const deadline = Date.now() + 120_000;
        let current = frame.SendPartV1;
        try {
          for (;;) {
            if (hex(current.to) !== this.identity.sign_pk || Date.now() >= deadline) throw new Error('Invalid fragmented deposit');
            const blob = parts.add(current);
            if (blob) {
              try { await this.incoming(from, blob); } finally { blob.fill(0); }
              await this.writeRelay(stream, { Deposited: { id: this.sequence++ } });
              break;
            }
            await this.writeRelay(stream, { SendPartAckV1: { index: current.index } });
            const next = await answerWithin(this.readClient(stream), deadline - Date.now());
            if (typeof next !== 'object' || !('SendPartV1' in next)) throw new Error('Interrupted fragmented deposit');
            current = next.SendPartV1;
          }
        } finally { parts.clear(); }
      } else if ('Send' in frame) {
        if (hex(frame.Send.to) !== this.identity.sign_pk) { await this.writeRelay(stream, { Error: 'this relay does not serve that recipient' }); continue; }
        if (frame.Send.blob.length > 4 * 1024 * 1024) throw new Error('Envelope size limit');
        // Decrypt, apply, seal, write and flush before Deposited. The sender
        // still needs the authenticated end-to-end receipt for Delivered.
        await this.incoming(from, new Uint8Array(frame.Send.blob));
        await this.writeRelay(stream, { Deposited: { id: this.sequence++ } });
      } else if ('ViewOnceOfferV1' in frame) {
        const request = frame.ViewOnceOfferV1;
        if (!this.viewOnceOffer || hex(request.to) !== this.identity.sign_pk) throw new Error('View-once recipient unavailable');
        const receive = this.viewOnceOffer(from, request);
        if ('recorded' in receive) {
          await receive.recorded;
          await this.writeRelay(stream, { ViewOnceStoredV1: { id: request.id } });
          continue;
        }
        const parts = new ViewOnceParts();
        const deadline = Math.min(Date.now() + 60_000, request.expires_at);
        try {
          await this.writeRelay(stream, { ViewOnceReadyV1: { offer: [...receive.bytes] } });
          while (Date.now() < deadline) {
            const send = await answerWithin(this.readClient(stream), deadline - Date.now());
            if (typeof send !== 'object' || !('ViewOncePartV1' in send)) throw new Error('Invalid view-once part');
            const envelope = parts.add(send.ViewOncePartV1);
            if (envelope) {
              try { await receive.accept(envelope); } finally { envelope.fill(0); }
              await this.writeRelay(stream, { ViewOnceStoredV1: { id: request.id } });
              break;
            }
            await this.writeRelay(stream, { ViewOncePartAckV1: { index: send.ViewOncePartV1.index } });
          }
        } finally { parts.clear(); receive.close(); }
      } else if ('Publish' in frame) {
        if (from !== this.identity.sign_pk) await this.writeRelay(stream, { Error: 'this relay does not serve that recipient' });
        // Our bundle lives in the Rust profile, not an unauthenticated cache.
      } else if (!('Ack' in frame)) throw new Error('Unexpected relay frame');
    }
  }

  private async authenticated(address: string, peer: string): Promise<BrowserStream> {
    if (this.closed) throw new Error('Router is locked');
    const index = this.idle.findIndex(entry => entry.address === address && entry.peer === peer);
    if (index >= 0) {
      const entry = this.idle.splice(index, 1)[0]!;
      clearTimeout(entry.timer);
      return entry.stream;
    }
    const stream = await this.network.connect(address, 443);
    if (this.closed) { stream.close(); stream.free(); throw new Error('Router is locked'); }
    this.streams.add(stream);
    try {
      await answerWithin((async () => {
        const challenge = await this.readRelay(stream);
        if (typeof challenge !== 'object' || !('Challenge' in challenge) || challenge.Challenge.length !== 32) throw new Error('Relay did not challenge');
        const nonce = crypto.getRandomValues(new Uint8Array(32));
        const remote = new Uint8Array(challenge.Challenge);
        const signature = this.engine.sign_framed_auth(address, remote, nonce);
        await this.writeClient(stream, { AuthFramedV1: { sign_pk: unhex(this.identity.sign_pk), nonce: [...nonce], signature: [...signature] } });
        const reply = await this.readRelay(stream);
        if (typeof reply !== 'object' || !('AuthFramedOkV1' in reply) ||
            !BrowserEngine.verify_terminal_proof(peer, address, this.identity.sign_pk, remote, nonce, new Uint8Array(reply.AuthFramedOkV1.proof))) throw new Error('Personal terminal ownership check failed');
      })());
      return stream;
    } catch (error) { this.release(stream); throw error; }
  }
  /** One request and its answer; a stream that fails or stays silent is closed. */
  private async exchange<T>(address: string, peer: string, run: (stream: BrowserStream) => Promise<T>, timeout?: number): Promise<T> {
    const candidates = [...new Set(this.terminalCandidates?.(peer) || [address])];
    const last = this.confirmed.get(peer);
    if (last && candidates.includes(last)) candidates.unshift(...candidates.splice(candidates.indexOf(last), 1));
    let stream: BrowserStream | undefined;
    let error: unknown = new Error('No personal terminal address');
    for (const candidate of candidates) {
      if (this.closed) throw new Error('Router is locked');
      try { stream = await this.authenticated(candidate, peer); address = candidate; break; }
      catch (cause) { error = cause; }
    }
    if (!stream) throw error;
    this.confirmed.set(peer, address);
    let ok = false;
    try {
      const value = await answerWithin(run(stream), timeout);
      ok = true;
      return value;
    } finally { if (ok) this.reuse(address, peer, stream); else this.release(stream); }
  }
  private reuse(address: string, peer: string, stream: BrowserStream): void {
    if (this.closed || this.idle.length >= 8 || this.idle.filter(entry => entry.address === address).length >= 4) { this.release(stream); return; }
    const timer = setTimeout(() => {
      const index = this.idle.findIndex(entry => entry.stream === stream);
      if (index >= 0) { this.idle.splice(index, 1); this.release(stream); }
    }, 15000);
    this.idle.push({ address, peer, stream, timer });
  }
  async bundle(address: string, peer: string): Promise<Uint8Array> {
    return this.exchange(address, peer, async stream => {
      await this.writeClient(stream, { GetBundle: { pk: unhex(peer) } });
      const frame = await this.readRelay(stream);
      if (typeof frame !== 'object' || !('Bundle' in frame) || hex(frame.Bundle.pk) !== peer || !frame.Bundle.bundle) throw new Error('Peer bundle is unavailable');
      return new Uint8Array(frame.Bundle.bundle);
    });
  }
  async ping(address: string, peer: string): Promise<void> {
    return this.exchange(address, peer, async stream => {
      await this.writeClient(stream, 'Ping');
      if (await this.readRelay(stream) !== 'Pong') throw new Error('Relay did not answer ping');
    });
  }
  /** The first frame carries no message. Only the recipient's signed offer
   * authorizes the encrypted body on this same stream. No mailbox fallback. */
  async sendViewOnce(address: string, peer: string, dh: string, body: Uint8Array | (() => Promise<Uint8Array>), expiresAt: number, stillAllowed: () => boolean, id = crypto.getRandomValues(new Uint8Array(16)), beforeUpload?: () => Promise<void>): Promise<string> {
    const nonce = crypto.getRandomValues(new Uint8Array(32));
    await this.exchange(address, peer, async stream => {
      if (this.closed || !stillAllowed() || expiresAt <= Date.now()) throw new Error('View-once message is unavailable');
      const deadline = Math.min(Date.now() + 60_000, expiresAt);
      await this.writeClient(stream, { ViewOnceOfferV1: { to: unhex(peer), id: [...id], nonce: [...nonce], expires_at: expiresAt } });
      const ready = await this.readRelay(stream);
      if (typeof ready === 'object' && 'ViewOnceStoredV1' in ready && hex(ready.ViewOnceStoredV1.id) === hex(id)) return;
      if (this.closed || Date.now() >= deadline || !stillAllowed() || typeof ready !== 'object' || !('ViewOnceReadyV1' in ready) ||
          ready.ViewOnceReadyV1.offer.length > 4096) throw new Error('Peer did not return a view-once offer');
      const opened = typeof body === 'function' ? await body() : body;
      let sealed: Uint8Array;
      try {
        if (this.closed || Date.now() >= deadline || !stillAllowed()) throw new Error('View-once message is unavailable');
        sealed = this.engine.view_once_seal(peer, dh, id, nonce, expiresAt, new Uint8Array(ready.ViewOnceReadyV1.offer), opened);
      } finally { if (typeof body === 'function') opened.fill(0); }
      try {
        await beforeUpload?.();
        for (const part of viewOnceParts(sealed)) {
          if (this.closed || Date.now() >= deadline || !stillAllowed()) throw new Error('View-once message is unavailable');
          await this.writeClient(stream, { ViewOncePartV1: part });
          const reply = await this.readRelay(stream);
          const valid = typeof reply === 'object' && (part.index === part.total - 1
            ? 'ViewOnceStoredV1' in reply && hex(reply.ViewOnceStoredV1.id) === hex(id)
            : 'ViewOncePartAckV1' in reply && reply.ViewOncePartAckV1.index === part.index);
          if (!valid) throw new Error('Peer did not confirm receipt of the view-once part');
        }
      } finally { sealed.fill(0); }
    });
    return hex(id);
  }
  async deposit(address: string, peer: string, blob: Uint8Array, stillPending?: () => boolean): Promise<void> {
    return this.exchange(address, peer, async stream => {
      let total = 0;
      for (const part of splitEnvelope(blob, DEPOSIT_LIMITS)) {
        if (this.closed || (stillPending && !stillPending())) throw new Error('Queued letter is no longer pending');
        total = part.total;
        await this.writeClient(stream, { SendPartV1: { to: unhex(peer), ...part } });
      }
      for (let index = 0; index < total; index++) {
        const reply = await this.readRelay(stream);
        const valid = typeof reply === 'object' && (index === total - 1
          ? 'Deposited' in reply && Number.isSafeInteger(reply.Deposited.id) && reply.Deposited.id > 0
          : 'SendPartAckV1' in reply && reply.SendPartAckV1.index === index);
        if (!valid) throw new Error('Relay did not acknowledge the fragmented deposit');
      }
    }, 120_000);
  }
}
