/** Telex over the Latin syllable before the caret, with no hidden input state. */
type ToneForms = readonly [string, string, string, string, string];

// Sắc, huyền, hỏi, ngã, nặng. Literal NFC forms avoid combining-mark caret edits.
const TONES: Readonly<Record<string, ToneForms>> = {
  a: ['á', 'à', 'ả', 'ã', 'ạ'], ă: ['ắ', 'ằ', 'ẳ', 'ẵ', 'ặ'], â: ['ấ', 'ầ', 'ẩ', 'ẫ', 'ậ'],
  e: ['é', 'è', 'ẻ', 'ẽ', 'ẹ'], ê: ['ế', 'ề', 'ể', 'ễ', 'ệ'], i: ['í', 'ì', 'ỉ', 'ĩ', 'ị'],
  o: ['ó', 'ò', 'ỏ', 'õ', 'ọ'], ô: ['ố', 'ồ', 'ổ', 'ỗ', 'ộ'], ơ: ['ớ', 'ờ', 'ở', 'ỡ', 'ợ'],
  u: ['ú', 'ù', 'ủ', 'ũ', 'ụ'], ư: ['ứ', 'ừ', 'ử', 'ữ', 'ự'], y: ['ý', 'ỳ', 'ỷ', 'ỹ', 'ỵ'],
  A: ['Á', 'À', 'Ả', 'Ã', 'Ạ'], Ă: ['Ắ', 'Ằ', 'Ẳ', 'Ẵ', 'Ặ'], Â: ['Ấ', 'Ầ', 'Ẩ', 'Ẫ', 'Ậ'],
  E: ['É', 'È', 'Ẻ', 'Ẽ', 'Ẹ'], Ê: ['Ế', 'Ề', 'Ể', 'Ễ', 'Ệ'], I: ['Í', 'Ì', 'Ỉ', 'Ĩ', 'Ị'],
  O: ['Ó', 'Ò', 'Ỏ', 'Õ', 'Ọ'], Ô: ['Ố', 'Ồ', 'Ổ', 'Ỗ', 'Ộ'], Ơ: ['Ớ', 'Ờ', 'Ở', 'Ỡ', 'Ợ'],
  U: ['Ú', 'Ù', 'Ủ', 'Ũ', 'Ụ'], Ư: ['Ứ', 'Ừ', 'Ử', 'Ữ', 'Ự'], Y: ['Ý', 'Ỳ', 'Ỷ', 'Ỹ', 'Ỵ'],
};
const TONE_KEYS = 'sfrxj';
const VOWELS = new Map<string, { base: string; tone: number }>();
for (const [base, forms] of Object.entries(TONES)) {
  VOWELS.set(base, { base, tone: -1 });
  forms.forEach((form, tone) => VOWELS.set(form, { base, tone }));
}

// Modifier, plain letter, shaped letter: the reverse direction also defines undo.
const SHAPES = [
  ['a', 'a', 'â'], ['e', 'e', 'ê'], ['o', 'o', 'ô'],
  ['w', 'a', 'ă'], ['w', 'o', 'ơ'], ['w', 'u', 'ư'], ['d', 'd', 'đ'],
] as const;

// Prefer a shaped vowel; equal priorities use the last vowel, whether or not a
// consonant follows. Plain oa/oe/uy therefore use the second vowel (modern style).
const TONE_PRIORITY: Readonly<Record<string, number>> = {
  a: 0, ă: 1, â: 1, e: 0, ê: 1, i: 0, o: 0, ô: 1, ơ: 1, u: 0, ư: 1, y: 0,
};

function sameCase(base: string, original: string): string {
  return original === original.toUpperCase() ? base.toUpperCase() : base;
}

/** A non-null result consumes the key and replaces a UTF-16 suffix of `before`. */
export function telexCompose(before: string, key: string): { replace: number; text: string } | null {
  if (!/^[aeowdsfrxjz]$/i.test(key)) return null;
  const syllable = before.match(/\p{Script=Latin}+$/u)?.[0];
  // `$` can also match before a final newline; composition must touch the caret.
  if (!syllable || !before.endsWith(syllable)) return null;
  const letters = [...syllable];
  const modifier = key.toLowerCase();
  const result = (literal = ''): { replace: number; text: string } => ({
    replace: syllable.length, text: letters.join('') + literal,
  });
  const tone = TONE_KEYS.indexOf(modifier);

  if (tone < 0 && modifier !== 'z') {
    // Searching the syllable also allows modifiers after a final consonant.
    for (let i = letters.length - 1; i >= 0; i--) {
      const original = letters[i]!;
      const vowel = VOWELS.get(original);
      const base = (vowel?.base ?? original).toLowerCase();
      const shape = SHAPES.find(([k, plain, formed]) => k === modifier && (base === plain || base === formed));
      if (!shape) continue;
      const undo = base === shape[2];
      const next = sameCase(undo ? shape[1] : shape[2], original);
      letters[i] = vowel && vowel.tone >= 0 ? TONES[next]![vowel.tone]! : next;
      return result(undo ? key : '');
    }
    return null;
  }

  let target = -1;
  let priority = -1;
  let previousTone = -1;
  for (let i = 0; i < letters.length; i++) {
    const vowel = VOWELS.get(letters[i]!);
    if (!vowel) continue;
    // Replacing a tone must not leave a second mark on another vowel.
    letters[i] = vowel.base;
    if (vowel.tone >= 0) previousTone = vowel.tone;
    const rank = TONE_PRIORITY[vowel.base.toLowerCase()]!;
    if (rank >= priority) { target = i; priority = rank; }
  }
  if (target < 0) return null;
  if (modifier === 'z') return previousTone < 0 ? null : result();
  if (previousTone === tone) return result(key);
  letters[target] = TONES[letters[target]!]![tone]!;
  return result();
}
