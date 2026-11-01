import {
  BrowserEngine, type BrowserNetwork, type BrowserStream,
  encode_client_frame, decode_client_frame, encode_relay_frame, decode_relay_frame,
} from '../../../browser-router/pkg/gipny_browser_router.js';
import { hex, unhex, type PublicIdentity } from './model';

type ClientFrame = 'Ping' | { AuthV2: { sign_pk: number[]; signature: number[] } }
  | { GetBundle: { pk: number[] } } | { Send: { to: number[]; blob: number[] } }
  | { Publish: { bundle: number[] } } | { Ack: { id: number } };
type RelayFrame = 'AuthOk' | 'AuthFail' | 'Pong' | { Challenge: number[] }
  | { Bundle: { pk: number[]; bundle: number[] | null } } | { Deposited: { id: number } }
  | { Error: string };

/** One process hosts only its owner. There is no open mailbox service or
 * plaintext connection to another local application. The native relay codec,
 * frame bounds and authentication context are supplied by Rust. */
export class BrowserRelay {
  private closed = false;
  private streams = new Set<BrowserStream>();
  private serving = 0;
  private sequence = 1;
  constructor(private engine: BrowserEngine, private network: BrowserNetwork, private identity: PublicIdentity,
    private incoming: (from: string, blob: Uint8Array) => Promise<void>, private failure: (reason: string) => void) {}

  start(): void { void this.acceptLoop(); }
  close(): void {
    this.closed = true;
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
    if (typeof auth !== 'object' || !('AuthV2' in auth) || !BrowserEngine.verify_auth(hex(auth.AuthV2.sign_pk), this.identity.onion, challenge, new Uint8Array(auth.AuthV2.signature))) {
      await this.writeRelay(stream, 'AuthFail');
      return;
    }
    const from = hex(auth.AuthV2.sign_pk);
    await this.writeRelay(stream, 'AuthOk');
    while (!this.closed) {
      const frame = await this.readClient(stream);
      if (frame === 'Ping') { await this.writeRelay(stream, 'Pong'); continue; }
      if (typeof frame !== 'object') throw new Error('Unexpected relay frame');
      if ('GetBundle' in frame) {
        const served = hex(frame.GetBundle.pk) === this.identity.sign_pk;
        await this.writeRelay(stream, { Bundle: { pk: frame.GetBundle.pk, bundle: served ? [...this.engine.my_bundle()] : null } });
      } else if ('Send' in frame) {
        if (hex(frame.Send.to) !== this.identity.sign_pk) { await this.writeRelay(stream, { Error: 'this relay does not serve that recipient' }); continue; }
        if (frame.Send.blob.length > 4 * 1024 * 1024) throw new Error('Envelope size limit');
        // Decrypt, apply, seal, write and flush before Deposited. The sender
        // still needs the authenticated end-to-end receipt for Delivered.
        await this.incoming(from, new Uint8Array(frame.Send.blob));
        await this.writeRelay(stream, { Deposited: { id: this.sequence++ } });
      } else if ('Publish' in frame) {
        if (from !== this.identity.sign_pk) await this.writeRelay(stream, { Error: 'this relay does not serve that recipient' });
        // Our bundle lives in the Rust profile, not an unauthenticated cache.
      } else if (!('Ack' in frame)) throw new Error('Unexpected relay frame');
    }
  }

  private async authenticated(address: string): Promise<BrowserStream> {
    if (this.closed) throw new Error('Router is locked');
    const stream = await this.network.connect(address, 443);
    if (this.closed) { stream.close(); stream.free(); throw new Error('Router is locked'); }
    this.streams.add(stream);
    try {
      const challenge = await this.readRelay(stream);
      if (typeof challenge !== 'object' || !('Challenge' in challenge) || challenge.Challenge.length !== 32) throw new Error('Relay did not challenge');
      const signature = this.engine.sign_auth(address, new Uint8Array(challenge.Challenge));
      await this.writeClient(stream, { AuthV2: { sign_pk: unhex(this.identity.sign_pk), signature: [...signature] } });
      if (await this.readRelay(stream) !== 'AuthOk') throw new Error('Relay authentication failed');
      return stream;
    } catch (error) { this.release(stream); throw error; }
  }
  async bundle(address: string, peer: string): Promise<Uint8Array> {
    const stream = await this.authenticated(address);
    try {
      await this.writeClient(stream, { GetBundle: { pk: unhex(peer) } });
      const frame = await this.readRelay(stream);
      if (typeof frame !== 'object' || !('Bundle' in frame) || hex(frame.Bundle.pk) !== peer || !frame.Bundle.bundle) throw new Error('Peer bundle is unavailable');
      return new Uint8Array(frame.Bundle.bundle);
    } finally { this.release(stream); }
  }
  async deposit(address: string, peer: string, blob: number[]): Promise<void> {
    const stream = await this.authenticated(address);
    try {
      await this.writeClient(stream, { Send: { to: unhex(peer), blob } });
      const frame = await this.readRelay(stream);
      if (typeof frame !== 'object' || !('Deposited' in frame)) throw new Error('Relay did not acknowledge deposit');
    } finally { this.release(stream); }
  }
}
