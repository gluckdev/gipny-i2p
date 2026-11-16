import type { PublicIdentity } from './model';

/** Fail before persisting a snapshot if UI and WASM identity contracts differ. */
export function publicIdentity(json: string): PublicIdentity {
  let value: unknown;
  try { value = JSON.parse(json); } catch { value = null; }
  const fields = ['card', 'sign_pk', 'dh_pk', 'destination', 'i2p_address', 'fingerprint'] as const;
  if (!value || typeof value !== 'object' || fields.some(key => {
    const field = (value as Record<string, unknown>)[key];
    return typeof field !== 'string' || !field;
  })) throw new Error('Unsupported browser engine identity. The saved profile was not changed.');
  return value as PublicIdentity;
}
