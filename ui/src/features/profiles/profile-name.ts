// The on-device profile name, derived from the name a person gives themselves.
//
// Registration asks for one name (owner, 2026-11-07): the one contacts see.
// The profile's directory on this device still needs an identifier the core
// accepts — ASCII letters, digits, dash, underscore, at most 32 — and it must
// not collide with another profile here. Uniqueness is per device only; the
// network never sees this name, so nothing global is consulted.

/** Russian and Ukrainian Cyrillic to Latin, without apostrophes. */
const CYRILLIC: Record<string, string> = {
  а: 'a', б: 'b', в: 'v', г: 'g', д: 'd', е: 'e', ё: 'e', ж: 'zh', з: 'z', и: 'i', й: 'y',
  к: 'k', л: 'l', м: 'm', н: 'n', о: 'o', п: 'p', р: 'r', с: 's', т: 't', у: 'u', ф: 'f',
  х: 'kh', ц: 'ts', ч: 'ch', ш: 'sh', щ: 'shch', ъ: '', ы: 'y', ь: '', э: 'e', ю: 'yu', я: 'ya',
  є: 'ye', і: 'i', ї: 'yi', ґ: 'g',
};

export const PROFILE_NAME_MAX = 32;
const FALLBACK = 'profile';

/** `display` reduced to the characters a profile name may hold, or '' when
 * none survive (a name in a script with no transliteration here). */
export function profileSlug(display: string): string {
  let out = '';
  // Transliterate before decomposing accents: NFKD would turn ї into і
  // and й into и and lose the distinctions in their Latin spelling.
  for (const ch of display.trim().toLowerCase().normalize('NFC')) {
    if (ch in CYRILLIC) out += CYRILLIC[ch];
    else if (/\s/.test(ch)) out += '-';
    else for (const latin of ch.normalize('NFKD')) {
      if (/[a-z0-9_-]/.test(latin)) out += latin;
    }
  }
  return out.replace(/-+/g, '-').replace(/^-|-$/g, '').slice(0, PROFILE_NAME_MAX).replace(/-$/, '');
}

/** The profile name to create for `display`: its slug, or `profile` when the
 * name leaves nothing usable, made unique among `taken` (case-insensitive)
 * with a numeric suffix that keeps the whole within the length limit. */
export function profileNameFor(display: string, taken: Iterable<string>): string {
  const base = profileSlug(display) || FALLBACK;
  const used = new Set(Array.from(taken, (name) => name.toLowerCase()));
  if (!used.has(base)) return base;
  for (let n = 2; ; n++) {
    const suffix = `-${n}`;
    const candidate = base.slice(0, PROFILE_NAME_MAX - suffix.length).replace(/-$/, '') + suffix;
    if (!used.has(candidate)) return candidate;
  }
}
