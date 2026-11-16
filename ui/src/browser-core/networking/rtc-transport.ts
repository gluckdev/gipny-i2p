/** WebRTC carries NTCP2 ciphertext only. Identity, ratchet and profile keys
 * stay in the Worker. No contact-to-contact ICE or automatic WSS fallback. */
export type RtcSettings = { iceServers: RTCIceServer[]; iceTransportPolicy: RTCIceTransportPolicy; fixturePrivate: boolean };
const FRAME = 16 * 1024;
/** Worker → Window bytes not yet handed to the DataChannel. Identical to the
 * WorkerRtcSocket.send bound and to socket.rs MAX_BUFFERED (2 × 64 KiB): Rust
 * waits above 64 KiB and then writes at most one 64 KiB frame. */
const SEND_BUDGET = 128 * 1024;
/** Window → Worker bytes not yet consumed by Rust. DataChannel receive cannot be
 * paused, so this is a fail-closed memory bound, not flow control. It is sized
 * far above the forwarder's 128 KiB SCTP send window so a Worker stall of
 * several round trips does not kill a healthy session. */
const RECEIVE_BUDGET = 1024 * 1024;
/** RTCDataChannel buffering: stop above HIGH, resume on `bufferedamountlow` at
 * LOW. Event driven, so background-tab timer throttling does not apply. */
const CHANNEL_HIGH = 256 * 1024;
const CHANNEL_LOW = 64 * 1024;
const CLOSE_DRAIN = 3000;
const DEADLINE = 30_000;
const LABEL = 'gipny-ntcp2-v1';
type Packet = { kind: string; data?: ArrayBuffer; size?: number };

export function rtcSettings(value: Record<string, unknown>): RtcSettings {
  const policy = value.rtc_ice_policy ?? 'relay';
  if (policy !== 'relay' && policy !== 'all') throw new Error('Invalid WebRTC ICE policy');
  const raw = value.rtc_ice_servers ?? [];
  if (!Array.isArray(raw) || raw.length > 4) throw new Error('Invalid WebRTC ICE servers');
  const servers: RTCIceServer[] = raw.map((entry: unknown) => {
    if (!entry || typeof entry !== 'object') throw new Error('Invalid ICE server');
    const item = entry as Record<string, unknown>;
    const urls = typeof item.urls === 'string' ? [item.urls] : item.urls;
    if (!Array.isArray(urls) || !urls.length || urls.length > 4 || urls.some(url => typeof url !== 'string' || url.length > 2048 || !/^(turns?|stun):[^\s/@]+(?:\?transport=(?:udp|tcp))?$/.test(url))) throw new Error('Invalid ICE URL');
    const credential: RTCIceServer = { urls };
    for (const key of ['username', 'credential'] as const) {
      if (item[key] !== undefined) {
        if (typeof item[key] !== 'string' || item[key].length > 1024) throw new Error('Invalid TURN credential');
        credential[key] = item[key];
      }
    }
    return credential;
  });
  if (policy === 'relay' && !servers.some(server => (server.urls as string[]).some(url => /^turns?:/.test(url)) && server.username && server.credential)) throw new Error('Relay-only WebRTC requires TURN credentials');
  const fixturePrivate = value.rtc_fixture_private === true;
  if (fixturePrivate && !['localhost', '127.0.0.1', '[::1]'].includes(globalThis.location.hostname)) throw new Error('Private ICE candidates are restricted to loopback fixtures');
  return { iceServers: servers, iceTransportPolicy: policy, fixturePrivate };
}

/** Keeps literal public IPv4 UDP candidates only. A browser behind NAT has no
 * such candidate without STUN (host candidates are LAN or mDNS), so direct
 * `rtc_ice_policy: "all"` needs a STUN server or a public interface; `relay`
 * needs TURN. Zero candidates fails closed instead of relying on peer-reflexive
 * discovery by the bridge. */
export function filteredSdp(sdp: string, fixturePrivate = false): string {
  if (sdp.length > 65536 || sdp.includes('\0')) throw new Error('Invalid SDP');
  const media = sdp.split(/\r?\n/).filter(line => line.startsWith('m='));
  if (media.length !== 1 || !/^m=application \d+ UDP\/DTLS\/SCTP webrtc-datachannel\s*$/.test(media[0]!)) throw new Error('Data channel SDP required');
  let candidates = 0;
  const lines = sdp.split(/\r?\n/).filter(line => {
    if (!line.startsWith('a=candidate:')) return true;
    const fields = line.split(/\s+/), address = fields[4] ?? '';
    if (fields[1] !== '1' || fields[2]?.toLowerCase() !== 'udp' || !/^\d+\.\d+\.\d+\.\d+$/.test(address)) return false;
    const [a = -1, b = -1, c = -1, d = -1] = address.split('.').map(Number);
    if ([a, b, c, d].some(n => n < 0 || n > 255) || [a, b, c, d].join('.') !== address) return false;
    const local = a === 10 || a === 127 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168);
    const reserved = a === 0 || a >= 224 || local || (a === 169 && b === 254) || (a === 100 && b >= 64 && b <= 127)
      || (a === 192 && b === 0 && (c === 0 || c === 2)) || (a === 192 && b === 88 && c === 99)
      || (a === 198 && (b === 18 || b === 19 || (b === 51 && c === 100))) || (a === 203 && b === 0 && c === 113);
    if (reserved && !(fixturePrivate && local)) return false;
    candidates++; return true;
  });
  if (!candidates || candidates > 16) throw new Error('No supported public IPv4 ICE path');
  return lines.join('\r\n');
}

/** WebSocket-shaped local adapter, explicitly bound by Rust as RtcSocket.
 * MessagePort credits bound both directions even when the Window is stalled. */
export class WorkerRtcSocket {
  binaryType = 'arraybuffer';
  bufferedAmount = 0;
  readyState = 0;
  readonly protocol: string;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  private port: MessagePort;
  private timer: ReturnType<typeof setTimeout>;
  constructor(endpoint: string, protocol: string, connect: (endpoint: string, protocol: string, port: MessagePort) => void) {
    this.protocol = protocol;
    const channel = new MessageChannel();
    this.port = channel.port1;
    this.timer = setTimeout(() => this.fail(), DEADLINE);
    this.port.onmessage = (event: MessageEvent<Packet>): void => {
      const packet = event.data;
      if (!packet || this.readyState === 3) return;
      if (packet.kind === 'open' && this.readyState === 0) {
        clearTimeout(this.timer); this.readyState = 1; this.onopen?.(new Event('open'));
      } else if (packet.kind === 'data' && this.readyState === 1 && packet.data instanceof ArrayBuffer && packet.data.byteLength > 0 && packet.data.byteLength <= FRAME) {
        const size = packet.data.byteLength;
        this.onmessage?.(new MessageEvent('message', { data: packet.data }));
        this.port.postMessage({ kind: 'received', size });
      } else if (packet.kind === 'written' && Number.isSafeInteger(packet.size) && packet.size! > 0 && packet.size! <= this.bufferedAmount) {
        this.bufferedAmount -= packet.size!;
      } else this.fail();
    };
    this.port.onmessageerror = () => this.fail();
    try { connect(endpoint, protocol, channel.port2); }
    catch { channel.port2.close(); this.fail(); }
  }
  send(data: Uint8Array): void {
    if (this.readyState !== 1 || !data.byteLength || data.byteLength > 65536 || this.bufferedAmount + data.byteLength > SEND_BUDGET) throw new Error('WebRTC send limit');
    const bytes = new Uint8Array(data);
    this.bufferedAmount += bytes.byteLength;
    this.port.postMessage({ kind: 'write', data: bytes.buffer }, [bytes.buffer]);
  }
  private fail(): void { if (this.readyState !== 3) { this.onerror?.(new Event('error')); this.close(); } }
  close(): void {
    if (this.readyState === 3) return;
    clearTimeout(this.timer); this.readyState = 3;
    this.port.postMessage({ kind: 'close' });
    this.port.onmessage = null; this.port.onmessageerror = null; this.port.close();
    // A signaling/ICE close is not evidence of an NTCP2 clean EOF.
    this.onclose?.(new CloseEvent('close', { wasClean: false }));
  }
}

export class RtcBridge {
  private sessions = new Set<() => void>();
  constructor(private settings: RtcSettings) {}
  close(): void { for (const close of [...this.sessions]) close(); }
  connect(endpoint: string, protocol: string, port: MessagePort): void {
    if (this.sessions.size >= 16 || typeof RTCPeerConnection !== 'function') { port.postMessage({ kind: 'error' }); port.close(); return; }
    const peer = new RTCPeerConnection({ ...this.settings, iceCandidatePoolSize: 0 });
    let data: RTCDataChannel, signal: WebSocket;
    try {
      data = peer.createDataChannel(LABEL, { ordered: true, protocol: LABEL });
      data.binaryType = 'arraybuffer';
      signal = new WebSocket(endpoint, protocol);
    } catch {
      peer.close(); port.postMessage({ kind: 'error' }); port.close(); return;
    }
    let closed = false, answered = false, queuedBytes = 0, receivedBytes = 0;
    let pending = Promise.resolve();
    let resume: (() => void) | null = null;
    const close = (): void => {
      if (closed) return;
      closed = true; clearTimeout(deadline); clearInterval(heartbeat);
      this.sessions.delete(close);
      signal.onopen = null; signal.onmessage = null; signal.onerror = null; signal.onclose = null;
      data.onopen = null; data.onmessage = null; data.onerror = null; data.onclose = null; data.onbufferedamountlow = null;
      const wake = resume; resume = null; wake?.();
      peer.onicegatheringstatechange = null; peer.onconnectionstatechange = null;
      signal.close(); data.close(); peer.close();
      port.postMessage({ kind: 'error' }); port.onmessage = null; port.onmessageerror = null; port.close();
    };
    const deadline = setTimeout(close, DEADLINE);
    // A dead Worker must not retain an entry connection indefinitely.
    let lastActivity = performance.now();
    const heartbeat = setInterval(() => { if (performance.now() - lastActivity > 120_000) close(); }, 5000);
    this.sessions.add(close);
    data.bufferedAmountLowThreshold = CHANNEL_LOW;
    data.onbufferedamountlow = (): void => { const wake = resume; resume = null; wake?.(); };
    // Waits until the channel buffer is at most `limit`. The synchronous check
    // precedes registration, so a low event cannot be missed in between.
    const below = async (limit: number, ms: number): Promise<void> => {
      const until = performance.now() + ms;
      while (!closed && data.readyState === 'open' && data.bufferedAmount > limit) {
        data.bufferedAmountLowThreshold = limit;
        await new Promise<void>((resolve, reject) => {
          const timer = setTimeout(() => { resume = null; reject(new Error('WebRTC backpressure timeout')); }, Math.max(0, until - performance.now()));
          resume = (): void => { clearTimeout(timer); resolve(); };
        });
      }
      if (closed || data.readyState !== 'open') throw new Error('WebRTC channel closed');
    };
    port.onmessageerror = close;
    port.onmessage = (event: MessageEvent<Packet>): void => {
      const packet = event.data;
      lastActivity = performance.now();
      if (packet?.kind === 'received' && Number.isSafeInteger(packet.size) && packet.size! > 0 && packet.size! <= receivedBytes) { receivedBytes -= packet.size!; return; }
      if (packet?.kind === 'close') {
        // Bytes the Worker already counts as written may still sit in the
        // channel buffer: drain them (bounded) before tearing the peer down.
        port.onmessage = null;
        const stop = setTimeout(close, CLOSE_DRAIN);
        void pending.then(() => below(0, CLOSE_DRAIN)).catch(() => {}).finally(() => { clearTimeout(stop); close(); });
        return;
      }
      if (packet?.kind !== 'write' || !(packet.data instanceof ArrayBuffer) || !packet.data.byteLength || packet.data.byteLength > 65536 || queuedBytes + packet.data.byteLength > SEND_BUDGET || data.readyState !== 'open') { close(); return; }
      const bytes = new Uint8Array(packet.data);
      queuedBytes += bytes.byteLength;
      pending = pending.then(async () => {
        if (closed) return;
        for (let offset = 0; offset < bytes.byteLength; offset += FRAME) {
          if (data.bufferedAmount > CHANNEL_HIGH - FRAME) await below(CHANNEL_LOW, DEADLINE);
          if (closed) return;
          data.send(bytes.subarray(offset, offset + FRAME));
        }
        if (!closed) { queuedBytes -= bytes.byteLength; port.postMessage({ kind: 'written', size: bytes.byteLength }); }
      }).catch(close);
    };
    data.onopen = (): void => { if (closed) return; clearTimeout(deadline); port.postMessage({ kind: 'open' }); };
    data.onmessage = (event: MessageEvent): void => {
      lastActivity = performance.now();
      if (!(event.data instanceof ArrayBuffer) || !event.data.byteLength || event.data.byteLength > FRAME || receivedBytes + event.data.byteLength > RECEIVE_BUDGET) { close(); return; }
      receivedBytes += event.data.byteLength;
      port.postMessage({ kind: 'data', data: event.data }, [event.data]);
    };
    data.onerror = close; data.onclose = close; signal.onerror = close; signal.onclose = close;
    peer.onconnectionstatechange = (): void => { if (['failed', 'closed'].includes(peer.connectionState)) close(); };
    signal.onopen = (): void => {
      if (signal.protocol !== protocol) { close(); return; }
      void (async () => {
        await peer.setLocalDescription(await peer.createOffer());
        if (peer.iceGatheringState !== 'complete') await new Promise<void>((resolve, reject) => {
          const timer = setTimeout(() => reject(new Error('ICE gathering timeout')), 15_000);
          peer.onicegatheringstatechange = (): void => {
            if (peer.iceGatheringState === 'complete') { clearTimeout(timer); peer.onicegatheringstatechange = null; resolve(); }
          };
        });
        if (closed || !peer.localDescription) return;
        const offer = JSON.stringify({ type: 'offer', sdp: filteredSdp(peer.localDescription.sdp, this.settings.fixturePrivate) });
        if (offer.length > 65536) throw new Error('SDP too large');
        signal.send(offer);
      })().catch(close);
    };
    signal.onmessage = (event: MessageEvent): void => {
      if (answered || typeof event.data !== 'string' || event.data.length > 65536) { close(); return; }
      answered = true;
      try {
        const answer = JSON.parse(event.data) as RTCSessionDescriptionInit;
        if (answer.type !== 'answer' || typeof answer.sdp !== 'string') { close(); return; }
        answer.sdp = filteredSdp(answer.sdp, this.settings.fixturePrivate);
        void peer.setRemoteDescription(answer).catch(close);
      } catch { close(); }
    };
  }
}
