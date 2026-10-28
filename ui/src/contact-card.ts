import { sha256 } from '@noble/hashes/sha2.js';

const PREFIX = 'gipny:v3:';
const DOMAIN = new TextEncoder().encode('gipny/contact-card/v3\0');
const MAX_NAME_BYTES = 96;
const MAX_CARD_CHARS = 316;
const BASE32 = 'abcdefghijklmnopqrstuvwxyz234567';

export interface DecodedCard {
  onion: string;
  signPk: string;
  dhPk: string;
  name?: string;
  relay?: string;
}

function invalid(): never { throw new Error('Invalid contact card address, key or name'); }
function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
}
function key(text: string): Uint8Array {
  if (!/^[0-9a-fA-F]{64}$/.test(text)) return invalid();
  return Uint8Array.from(text.match(/../g)!, (pair) => parseInt(pair, 16));
}
function base64(bytes: Uint8Array): string { return btoa(String.fromCharCode(...bytes)); }
function url64(bytes: Uint8Array): string { return base64(bytes).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, ''); }
function unbase64(text: string): Uint8Array { return Uint8Array.from(atob(text), (char) => char.charCodeAt(0)); }
function checksum(body: Uint8Array): Uint8Array {
  const input = new Uint8Array(DOMAIN.length + body.length);
  input.set(DOMAIN);
  input.set(body, DOMAIN.length);
  return sha256(input).slice(0, 4);
}

function b32Address(bytes: Uint8Array): string {
  let text = '', acc = 0, bits = 0;
  for (const byte of bytes) {
    acc = (acc << 8) | byte; bits += 8;
    while (bits >= 5) { bits -= 5; text += BASE32[(acc >>> bits) & 31]!; }
  }
  if (bits) text += BASE32[(acc << (5 - bits)) & 31]!;
  return text + '.b32.i2p';
}

/** An I2P lookup hash, never a shortened/truncated public key. */
function addressHash(input: string): Uint8Array {
  const address = input.trim();
  const match = /^([a-z2-7]{52})\.b32\.i2p$/i.exec(address);
  if (match) {
    let acc = 0, bits = 0;
    const bytes: number[] = [];
    for (const char of match[1]!.toLowerCase()) {
      acc = (acc << 5) | BASE32.indexOf(char); bits += 5;
      if (bits >= 8) { bits -= 8; bytes.push((acc >>> bits) & 255); }
    }
    if (bits !== 4 || (acc & 15) !== 0 || bytes.length !== 32) return invalid();
    return Uint8Array.from(bytes);
  }
  if (address.length > 4096 || !/^[A-Za-z0-9~-]{516,}={0,2}$/.test(address)) return invalid();
  const body = address.replace(/=+$/, '').replace(/-/g, '+').replace(/~/g, '/');
  const padding = (4 - body.length % 4) % 4;
  const provided = address.length - address.replace(/=+$/, '').length;
  if (provided !== 0 && provided !== padding) return invalid();
  const raw = unbase64(body + '='.repeat(padding));
  if (base64(raw).replace(/=+$/, '') !== body || raw.length < 387 ||
      raw.length !== 387 + raw[385]! * 256 + raw[386]!) return invalid();
  return sha256(raw);
}

/** Export canonical v3. Invalid input fails; a long name is never truncated. */
export function encodeCard(onion: string, signPk: string, dhPk: string, name?: string, relay?: string): string {
  const outbound = addressHash(onion);
  const relayHash = relay === undefined ? undefined : addressHash(relay);
  if (name && name.length > MAX_NAME_BYTES) return invalid();
  const label = name ? new TextEncoder().encode(name) : undefined;
  if (label && (label.length > MAX_NAME_BYTES || new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(label) !== name)) return invalid();
  const body = new Uint8Array(97 + (relayHash ? 32 : 0) + (label ? 1 + label.length : 0));
  body[0] = (relayHash ? 1 : 0) | (label ? 2 : 0);
  body.set(key(signPk), 1); body.set(key(dhPk), 33); body.set(outbound, 65);
  let offset = 97;
  if (relayHash) { body.set(relayHash, offset); offset += 32; }
  if (label) { body[offset++] = label.length; body.set(label, offset); }
  const payload = new Uint8Array(body.length + 4);
  payload.set(body); payload.set(checksum(body), body.length);
  return PREFIX + url64(payload);
}

function decodeV3(payload: string): DecodedCard | null {
  if (payload.length + PREFIX.length > MAX_CARD_CHARS || !/^[A-Za-z0-9_-]+$/.test(payload)) return null;
  const bytes = unbase64(payload.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - payload.length % 4) % 4));
  if (bytes.length < 101 || bytes.length > 230 || url64(bytes) !== payload) return null;
  const body = bytes.subarray(0, -4);
  if ((body[0]! & ~3) !== 0 || !checksum(body).every((byte, i) => byte === bytes[body.length + i])) return null;
  let offset = 97;
  const card: DecodedCard = { signPk: hex(body.subarray(1, 33)), dhPk: hex(body.subarray(33, 65)), onion: b32Address(body.subarray(65, 97)) };
  if (body[0]! & 1) {
    if (body.length < offset + 32) return null;
    card.relay = b32Address(body.subarray(offset, offset + 32)); offset += 32;
  }
  if (body[0]! & 2) {
    const length = body[offset++];
    if (!length || length > MAX_NAME_BYTES || body.length < offset + length) return null;
    card.name = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(body.subarray(offset, offset + length)); offset += length;
  }
  return offset === body.length ? card : null;
}

/** v1/v2 imports remain supported; all new exports use v3. */
export function decodeCard(input: string): DecodedCard | null {
  const raw = input.trim();
  try {
    if (raw.startsWith(PREFIX)) return decodeV3(raw.slice(PREFIX.length));
    if (raw.length > 16_384) return null;
    const v2 = /^gipny:v2:([^:]+):([0-9a-fA-F]{64}):([0-9a-fA-F]{64}):([^:]+)(?::(.*))?$/.exec(raw);
    if (v2) {
      if (!isValidI2pAddress(v2[4]!)) return null;
      return { onion: v2[1]!, signPk: v2[2]!.toLowerCase(), dhPk: v2[3]!.toLowerCase(), relay: v2[4]!, name: v2[5] ? decodeURIComponent(v2[5]) : undefined };
    }
    const v1 = /^gipny:v1:([^:]+):([0-9a-fA-F]{64}):([0-9a-fA-F]{64})(?::(.*))?$/.exec(raw);
    return v1 ? { onion: v1[1]!, signPk: v1[2]!.toLowerCase(), dhPk: v1[3]!.toLowerCase(), name: v1[4] ? decodeURIComponent(v1[4]) : undefined } : null;
  } catch { return null; }
}

export function isValidI2pAddress(address: string): boolean {
  const a = address.trim();
  return /^[a-z2-7]{52}\.b32\.i2p$/i.test(a) || /^[A-Za-z0-9~-]{516,}={0,2}$/.test(a);
}
