import { focusVoiceDialog } from './dialog';
import { Api } from '../../app/api';
import type { App } from '../../app/app';
import type { Store } from '../../app/state';
import { h, avatar } from '../../app/view';
import { t } from '../localization/i18n';
import { supportsCapture, VoiceAudio, voiceTime } from './audio';
import { CALL_RATE, FRAME_SAMPLES, decodeAudio, encodeAudio, parsePacket, type CallPacket, type Kind } from './call-wire';

const probeData = encodeAudio(new Int16Array(FRAME_SAMPLES));
const frameLifetime = 6000;
const controlLifetime = 30000;
type Phase = 'ringing' | 'preflight' | 'live';

class Session {
  phase: Phase = 'ringing';
  close: () => void = () => {};
  audio?: VoiceAudio;
  accepted = false;
  peerReady = false;
  lastActivity = performance.now();
  lastPlayed = -1;
  seq = 0;
  window = 1;
  received = 0;
  connectedAt = 0;
  probeRetries = 0;
  readonly clock = h('time', { class: 'voice-time' });
  readonly mute = h('button', { class: 'btn btn-ghost', hidden: true, 'aria-pressed': 'false' }, t('voice.mute'));
  firstProbeMs = 0;
  smoothedRtt = 0;
  readonly blocks: Int16Array[] = [];
  probeStart = 0;
  sends = 0;
  readonly pending = new Map<number, number>();
  readonly status = h('p', { role: 'status' });
  constructor(readonly peer: number, readonly id: string, readonly inbound: boolean) {}
}

export class Calls {
  private current?: Session;
  private recent = new Map<string, number>();

  constructor(private app: App, private store: Store) {}

  start(peer: number): void {
    if (this.current || !this.eligible(peer)) return;
    const id = Array.from(crypto.getRandomValues(new Uint8Array(12)), n => n.toString(16).padStart(2, '0')).join('');
    const call = this.open(peer, id, false);
    void this.answer(call);
  }

  private eligible(peer: number): boolean {
    return supportsCapture() && this.store.view.get() === 'main'
      && this.store.contacts.get().some(c => c.id === peer && c.request === 'none' && c.trust !== 2);
  }

  receive(peer: number, raw: string, at: number): void {
    if (Math.abs(Date.now() - at) > controlLifetime || !this.eligible(peer)) return;
    const p = parsePacket(raw);
    if (!p) return;
    const call = this.current;
    if (p.kind === 'end' && (!call || call.id !== p.id || call.peer !== peer)) {
      this.prune(); this.recent.set(peer + '/' + p.id, Date.now());
      return;
    }
    if (p.kind === 'invite' && !call) {
      this.prune();
      if (!this.recent.has(peer + '/' + p.id)) this.open(peer, p.id, true);
      return;
    }
    if (!call || peer !== call.peer || call.id !== p.id) return;
    if (p.kind === 'end') { call.close(); return; }
    if (!call.accepted || p.kind === 'invite') return;
    if (p.kind === 'accept' && !call.inbound && !call.peerReady) {
      call.peerReady = true;
      this.preflight(call);
    } else if (p.kind === 'probe' && call.peerReady) {
      this.send(call, 'echo', p.seq);
    } else if (p.kind === 'echo' && call.phase === 'preflight') {
      const sent = call.pending.get(p.seq);
      if (sent === undefined) return;
      const elapsed = Math.max(1, performance.now() - sent);
      call.pending.delete(p.seq);
      if (!call.firstProbeMs) {
        // The first answer to any single-lane attempt measures that attempt.
        // Discard the remaining singles before measuring the parallel pair.
        call.pending.clear();
        call.firstProbeMs = call.smoothedRtt = elapsed;
        this.probes(call, 2);
      } else if (!call.pending.size) {
        call.window = elapsed < 1.8 * call.firstProbeMs ? 2 : 1;
        this.live(call);
      }
    } else if (p.kind === 'audio' && call.peerReady && Math.abs(Date.now() - at) < frameLifetime) {
      this.send(call, 'ack', p.seq);
      if (p.seq > call.lastPlayed) {
        call.lastPlayed = p.seq;
        call.audio?.play(decodeAudio(p.data!), CALL_RATE);
      }
    } else if (p.kind === 'ack' && call.phase === 'live') {
      const start = call.pending.get(p.seq);
      if (start === undefined) return;
      call.pending.delete(p.seq);
      const rtt = performance.now() - start;
      call.smoothedRtt += (rtt - call.smoothedRtt) / 8;
      if (rtt > Math.max(1500, call.firstProbeMs * 2)) {
        call.window = Math.max(1, call.window - 1);
        call.received = 0;
      } else if (++call.received >= 16) {
        call.window = Math.min(4, call.window + 1);
        call.received = 0;
      }
    }
    call.lastActivity = performance.now();
  }

  private prune(): void {
    for (const [key, at] of this.recent) if (Date.now() - at > 60000) this.recent.delete(key);
    if (this.recent.size >= 128) this.recent.delete(this.recent.keys().next().value!);
  }

  private open(peer: number, id: string, inbound: boolean): Session {
    const call = new Session(peer, id, inbound);
    this.current = call;
    call.status.textContent = t('voice.ringing');
    const timer = window.setInterval(() => {
      if (call.phase === 'live') call.clock.textContent = voiceTime((performance.now() - call.connectedAt) / 1000);
      if (call.phase === 'preflight' && !call.firstProbeMs && call.probeRetries < 2 && performance.now() - call.probeStart > frameLifetime) {
        call.probeRetries++; this.probes(call, 1);
      }
      if (call.phase === 'preflight' && call.firstProbeMs && performance.now() - call.probeStart > frameLifetime) {
        call.window = 1;
        this.live(call);
      }
      // An outgoing invite, the recipient's time to answer, and their accept
      // are three separate waits. Each control frame still expires in transit.
      const waitingForAnswer = call.phase === 'ringing' && call.accepted && !call.inbound;
      const timeout = waitingForAnswer ? 3 * controlLifetime : controlLifetime;
      if (performance.now() - call.lastActivity > timeout) {
        this.store.showToast(t('voice.timeout'), true); call.close();
      }
    }, 500);
    const end = () => call.close();
    window.addEventListener('pagehide', end);
    call.close = this.app.openModal(close => {
      const answer = inbound && h('button', { class: 'btn voice-answer', onClick: () => { if (answer) answer.remove(); void this.answer(call); } }, t('voice.answer'));
      const person = this.store.contacts.get().find(c => c.id === peer)!;
      call.mute.onclick = () => {
        const mute = call.mute.getAttribute('aria-pressed') !== 'true';
        call.audio?.mute(mute);
        call.mute.setAttribute('aria-pressed', String(mute));
        call.mute.textContent = t(mute ? 'voice.unmute' : 'voice.mute');
      };
      return focusVoiceDialog(h('div', { class: 'modal voice-panel voice-call-panel', role: 'dialog', 'aria-modal': true, 'aria-label': t('voice.call') },
        avatar(person.name, person.sign_pk), h('h2', {}, person.name),
        h('div', { class: 'voice-call-state' }, t('voice.call'), ' · I2P'), call.status, call.clock,
        h('div', { class: 'voice-actions' }, answer, call.mute,
          h('button', { class: 'btn btn-danger voice-end', onClick: close }, t('voice.hangup')))), close);
    }, () => {
      clearInterval(timer);
      window.removeEventListener('pagehide', end);
      call.audio?.close();
      call.blocks.length = 0;
      call.pending.clear();
      this.current = undefined;
      this.prune(); this.recent.set(peer + '/' + id, Date.now());
      void Api.sendCallFrame(peer, JSON.stringify({ id, kind: 'end', seq: 0 }), 0).catch(() => {});
    });
    return call;
  }

  private async answer(call: Session): Promise<void> {
    if (this.current !== call || call.audio) return;
    try {
      call.status.textContent = t('voice.mic');
      call.audio = new VoiceAudio();
      await call.audio.start(CALL_RATE, samples => this.media(call, samples));
      if (this.current !== call) return;
      call.accepted = true;
      call.lastActivity = performance.now();
      call.mute.hidden = false;
      call.status.textContent = t('voice.ringing');
      if (call.inbound) {
        call.peerReady = true;
        await this.transmit(call, 'accept', 0);
        if (this.current === call) this.preflight(call);
      } else await this.transmit(call, 'invite', 0);
    } catch (error) { this.fail(call, error); }
  }

  private preflight(call: Session): void {
    call.phase = 'preflight';
    call.lastActivity = performance.now();
    call.status.textContent = t('voice.probe');
    this.probes(call, 1);
  }

  private probes(call: Session, count: number): void {
    call.probeStart = performance.now();
    for (let lane = 0; lane < count; lane++) {
      const seq = ++call.seq;
      call.pending.set(seq, call.probeStart);
      this.send(call, 'probe', seq, probeData, lane);
    }
  }

  private live(call: Session): void {
    call.phase = 'live';
    call.connectedAt = performance.now();
    call.pending.clear();
    call.status.textContent = t('voice.connected');
  }

  private media(call: Session, samples: Int16Array): void {
    if (this.current !== call || call.phase !== 'live') return;
    const now = performance.now();
    for (const [seq, sent] of call.pending) {
      if (now - sent > frameLifetime) {
        call.pending.delete(seq); call.window = Math.max(1, call.window - 1); call.received = 0;
      }
    }
    const count = Math.max(1, Math.min(3, Math.ceil(call.smoothedRtt / (250 * call.window))));
    call.blocks.push(samples);
    while (call.blocks.length > count) call.blocks.shift();
    if (call.blocks.length < count || call.pending.size >= call.window || call.sends >= call.window) return;
    const packet = new Int16Array(count * FRAME_SAMPLES);
    call.blocks.forEach((block, index) => packet.set(block, index * FRAME_SAMPLES));
    call.blocks.length = 0;
    const seq = ++call.seq;
    call.pending.set(seq, now);
    this.send(call, 'audio', seq, encodeAudio(packet), seq % call.window);
  }

  private transmit(call: Session, kind: Kind, seq: number, data?: string, lane = 0): Promise<void> {
    const p: CallPacket = { id: call.id, kind, seq, data };
    return Api.sendCallFrame(call.peer, JSON.stringify(p), lane);
  }

  private send(call: Session, kind: Kind, seq: number, data?: string, lane = 0): void {
    if (call.sends >= 8) return;
    call.sends++;
    // Probes and media can lose one route while another lane is answering.
    // Their receive timers and bounded windows already handle a missing reply.
    void this.transmit(call, kind, seq, data, lane).catch(() => {
      if (kind === 'audio' && this.current === call) {
        call.pending.delete(seq);
        call.window = 1;
      }
    }).finally(() => call.sends--);
  }

  private fail(call: Session, error: unknown): void {
    if (this.current !== call) return;
    this.store.showToast(String(error), true); call.close();
  }
}
