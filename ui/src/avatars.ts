import { t as tr } from './i18n';
import { Api } from './api';

/** Default avatars: characters cut from Japanese woodblock prints in the public
 * domain, packed into one sprite (`public/avatars.webp`, 6×4 tiles of 96 px,
 * ≈47 KB). Each key gets one at random but stably (by hash); the person can
 * pick another, which is kept in the vault and seen only by them.
 *
 * Sources (Wikimedia Commons, public domain):
 *   Utagawa Kuniyoshi — «Cats in various poses»; «Cats suggested as the
 *   fifty-three stations of the Tokaido» (Rijksmuseum); «Animals dyeing
 *   fabrics»; «Octopus, red fish»; «A cat dressed as a woman tapping the head
 *   of an octopus». Utagawa Hiroshige — «Owl on a pine branch» (Brooklyn
 *   Museum). Order below is the sprite's, row by row. */
const AVATAR_KEYS = [
  'avatars.bearded_tanuki', 'avatars.black_dog', 'avatars.fox', 'avatars.hare', 'avatars.rat', 'avatars.badger',
  'avatars.elegant_cat', 'avatars.cat_with_a_raised_paw', 'avatars.cat_in_a_headscarf', 'avatars.sleepy_cat', 'avatars.calico_cat', 'avatars.octopus',
  'avatars.sea_octopus', 'avatars.owl', 'avatars.carp', 'avatars.white_cat', 'avatars.cat_by_a_basket', 'avatars.grumpy_cat',
  'avatars.cat_with_a_scarf', 'avatars.cat_with_a_collar', 'avatars.cat_looking_up', 'avatars.spotted_cat', 'avatars.cat_with_prey', 'avatars.cat_washing',
] as const;

export const avatarNames = (): string[] => AVATAR_KEYS.map((key) => tr(key));

const COLS = 6;
const ROWS = 4;

let choices: Record<string, string> = {};
const listeners = new Set<() => void>();

/** Load the saved picks once the vault is open; repaints whoever listens. */
export async function loadAvatarChoices(): Promise<void> {
  choices = await Api.getAvatarChoices();
  listeners.forEach((l) => l());
}

export function onAvatarsChanged(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function hashIndex(seed: string): number {
  let hash = 2166136261;
  for (let i = 0; i < seed.length; i++) hash = Math.imul(hash ^ seed.charCodeAt(i), 16777619) >>> 0;
  return hash % AVATAR_KEYS.length;
}

/** The avatar index for a key: the person's pick, else the random default. */
export function avatarIndex(seed: string): number {
  const picked = Number(choices[seed]);
  return Number.isInteger(picked) && picked >= 0 && picked < AVATAR_KEYS.length ? picked : hashIndex(seed);
}

export function isPicked(seed: string): boolean {
  return choices[seed] !== undefined;
}

/** `index` null goes back to the random default. */
export async function setAvatar(seed: string, index: number | null): Promise<void> {
  const next = { ...choices };
  if (index == null) delete next[seed]; else next[seed] = String(index);
  choices = next;
  listeners.forEach((l) => l());
  await Api.setAvatarChoices(next);
}

/** Paint tile `index` of the sprite as `el`'s background. */
export function paintAvatar(el: HTMLElement, index: number): void {
  const col = index % COLS;
  const row = Math.floor(index / COLS);
  el.classList.add('avatar-pic');
  el.style.backgroundPosition = `${(col / (COLS - 1)) * 100}% ${(row / (ROWS - 1)) * 100}%`;
}
