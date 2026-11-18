// A packet carries one to three independent 250 ms G.711 μ-law blocks at 8 kHz.
export const CALL_RATE = 8000;
export const FRAME_SAMPLES = CALL_RATE / 4;
export type Kind = 'invite' | 'accept' | 'end' | 'probe' | 'echo' | 'audio' | 'ack';
export interface CallPacket { id: string; kind: Kind; seq: number; data?: string }

export function parsePacket(raw: string): CallPacket | null {
  if (raw.length > 8192) return null;
  try {
    const p = JSON.parse(raw);
    if (!p || typeof p.id !== 'string' || !/^[a-f0-9]{24}$/.test(p.id)
        || !Number.isSafeInteger(p.seq) || p.seq < 0 || p.seq > 0x7fffffff) return null;
    if (p.kind === 'audio' || p.kind === 'probe') {
      if (typeof p.data !== 'string') return null;
      const valid = p.data.length === 2668 && /^[A-Za-z0-9+/]{2667}=$/.test(p.data)
        || p.kind === 'audio' && (p.data.length === 5336 && /^[A-Za-z0-9+/]{5334}==$/.test(p.data)
          || p.data.length === 8000 && /^[A-Za-z0-9+/]{8000}$/.test(p.data));
      if (!valid) return null;
    } else if (!['invite', 'accept', 'end', 'echo', 'ack'].includes(p.kind) || p.data !== undefined) return null;
    return p;
  } catch { return null; }
}

export function encodeAudio(samples: Int16Array): string {
  const bytes = new Uint8Array(samples.length);
  for (let i = 0; i < samples.length; i++) {
    const x = samples[i]!, magnitude = Math.min(Math.abs(x), 32635) + 132;
    const exponent = 24 - Math.clz32(magnitude);
    bytes[i] = ~((x < 0 ? 128 : 0) | (exponent << 4) | ((magnitude >> (exponent + 3)) & 15));
  }
  return btoa(String.fromCharCode(...bytes));
}

export function decodeAudio(encoded: string): Float32Array<ArrayBuffer> {
  const bytes = atob(encoded);
  return Float32Array.from(bytes, char => {
    const code = 255 ^ char.charCodeAt(0);
    const magnitude = (((code & 15) * 8 + 132) << ((code >> 4) & 7)) - 132;
    return (code & 128 ? -magnitude : magnitude) / 32768;
  });
}
