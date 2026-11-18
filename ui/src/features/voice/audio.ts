import workletUrl from './capture-worklet.js?url&no-inline';

export const CLIP_RATE = 16000;
export const CLIP_SECONDS = 180;

export function voiceTime(seconds: number): string {
  return Math.floor(seconds / 60) + ':' + String(Math.floor(seconds % 60)).padStart(2, '0');
}

export function supportsCapture(): boolean {
  return typeof AudioContext !== 'undefined' && typeof AudioWorkletNode !== 'undefined'
    && !!navigator.mediaDevices?.getUserMedia;
}

export class VoiceAudio {
  readonly context = new AudioContext();
  private stream?: MediaStream;
  private capture?: AudioWorkletNode;
  private closed = false;
  private playbackAt = 0;

  async start(rate: number, block: (samples: Int16Array) => void): Promise<void> {
    await this.context.resume();
    if (this.closed) return;
    const stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
    if (this.closed) { stream.getTracks().forEach(track => track.stop()); return; }
    this.stream = stream;
    await this.context.audioWorklet.addModule(workletUrl);
    if (this.closed) return;
    const source = this.context.createMediaStreamSource(stream);
    const node = new AudioWorkletNode(this.context, 'voice-capture', {
      channelCount: 1, channelCountMode: 'explicit', processorOptions: { rate },
    });
    this.capture = node;
    node.port.onmessage = event => { if (!this.closed) block(event.data as Int16Array); };
    source.connect(node).connect(this.context.destination);
  }

  mute(muted: boolean): void { this.stream?.getAudioTracks().forEach(track => { track.enabled = !muted; }); }

  play(samples: Float32Array<ArrayBuffer>, rate: number): void {
    if (this.closed || this.playbackAt > this.context.currentTime + 1) return;
    const buffer = this.context.createBuffer(1, samples.length, rate);
    buffer.copyToChannel(samples, 0);
    const source = this.context.createBufferSource();
    source.buffer = buffer;
    source.connect(this.context.destination);
    const start = Math.max(this.context.currentTime + 0.03, this.playbackAt);
    source.start(start);
    this.playbackAt = start + buffer.duration;
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.stream?.getTracks().forEach(track => track.stop());
    this.capture?.port.close();
    this.capture?.disconnect();
    void this.context.close().catch(() => {});
  }
}

export function voiceFile(blocks: Int16Array[]): File {
  const length = blocks.reduce((sum, block) => sum + block.length, 0);
  if (!length || length > CLIP_RATE * CLIP_SECONDS) throw new Error('Invalid recording length');
  const bytes = new ArrayBuffer(44 + length * 2);
  const view = new DataView(bytes);
  const tag = (offset: number, value: string) => new Uint8Array(bytes, offset, 4).set(new TextEncoder().encode(value));
  tag(0, 'RIFF'); view.setUint32(4, bytes.byteLength - 8, true);
  tag(8, 'WAVE'); tag(12, 'fmt '); view.setUint32(16, 16, true);
  view.setUint16(20, 1, true); view.setUint16(22, 1, true);
  view.setUint32(24, CLIP_RATE, true); view.setUint32(28, CLIP_RATE * 2, true);
  view.setUint16(32, 2, true); view.setUint16(34, 16, true);
  tag(36, 'data'); view.setUint32(40, length * 2, true);
  let offset = 44;
  for (const block of blocks) for (const sample of block) { view.setInt16(offset, sample, true); offset += 2; }
  const id = Array.from(crypto.getRandomValues(new Uint8Array(12)), n => n.toString(16).padStart(2, '0')).join('');
  return new File([bytes], 'voice-' + id + '.wav', { type: 'audio/wav' });
}

export function isVoiceFile(name: string, size: number): boolean {
  return /^(?:\d{13}-)?voice-[a-f0-9]{24}\.wav$/.test(name) && size >= 44 && size <= 44 + CLIP_RATE * CLIP_SECONDS * 2;
}
