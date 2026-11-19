export type PartLimits = { envelope: number; part: number };
export const DEPOSIT_LIMITS: PartLimits = { envelope: 4 * 1024 * 1024 + 4096, part: 256 * 1024 };
export type EnvelopePart = { index: number; total: number; bytes: number[] };
export function* splitEnvelope(envelope: Uint8Array, limits: PartLimits): Generator<EnvelopePart> {
  if (envelope.length < 2 || envelope.length > limits.envelope) throw new Error('Invalid fragmented envelope');
  const total = Math.max(2, Math.ceil(envelope.length / limits.part));
  const width = Math.ceil(envelope.length / total);
  for (let index = 0; index < total; index++) {
    yield { index, total, bytes: [...envelope.subarray(index * width, (index + 1) * width)] };
  }
}

/** A single offer owns a bounded, strictly ordered assembly in memory. */
export class EnvelopeParts {
  constructor(private limits: PartLimits) {}
  private chunks: Uint8Array[] = [];
  private total = 0;
  private length = 0;

  add(part: EnvelopePart): Uint8Array | null {
    const count = part.bytes.length, first = this.chunks[0]?.length ?? count;
    const final = part.index === part.total - 1;
    if (!Number.isInteger(part.total) || part.total < 2 || part.total > Math.ceil(this.limits.envelope / this.limits.part) ||
        part.index !== this.chunks.length || part.index >= part.total || (this.total && this.total !== part.total) ||
        !count || count > this.limits.part || (final ? count > first : count !== first) || this.length + count > this.limits.envelope) throw new Error('Invalid fragmented envelope');
    this.total = part.total;
    this.chunks.push(new Uint8Array(part.bytes)); this.length += count;
    if (!final) return null;
    const result = new Uint8Array(this.length);
    let offset = 0;
    for (const chunk of this.chunks) { result.set(chunk, offset); offset += chunk.length; }
    return result;
  }

  clear(): void {
    for (const chunk of this.chunks) chunk.fill(0);
    this.chunks = []; this.length = 0; this.total = 0;
  }
}
