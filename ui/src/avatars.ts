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
export const AVATARS = [
  'Тануки с бородой', 'Чёрный пёс', 'Лис', 'Заяц', 'Крыса', 'Барсук',
  'Кошка-красавица', 'Кот с лапой', 'Кот в платке', 'Сонный кот', 'Трёхцветная', 'Осьминог',
  'Морской осьминог', 'Сова', 'Карп', 'Белый кот', 'Кот у корзины', 'Кот-ворчун',
  'Кошка с платком', 'Кот в ошейнике', 'Кот смотрит вверх', 'Пятнистый кот', 'Кот с добычей', 'Кот умывается',
] as const;

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
  return hash % AVATARS.length;
}

/** The avatar index for a key: the person's pick, else the random default. */
export function avatarIndex(seed: string): number {
  const picked = Number(choices[seed]);
  return Number.isInteger(picked) && picked >= 0 && picked < AVATARS.length ? picked : hashIndex(seed);
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
