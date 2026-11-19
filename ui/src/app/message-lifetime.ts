export const DEFAULT_TTL_SECS = 30 * 86_400;
export const MAX_TTL_SECS = 90 * 86_400;

export function messageLifetimeSeconds(value: unknown): number {
  if (value == null) return DEFAULT_TTL_SECS;
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0 || value > MAX_TTL_SECS) {
    throw new Error('Invalid message lifetime');
  }
  return value;
}
