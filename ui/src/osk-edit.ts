/**
 * Text editing for the on-screen keyboard, as pure functions over the
 * field's value and selection. The component applies the result to the
 * element; tests check the arithmetic here without a DOM.
 */
export interface TextState { value: string; start: number; end: number }

function clamp(state: TextState): TextState {
  const len = state.value.length;
  const start = Math.max(0, Math.min(state.start, len));
  const end = Math.max(start, Math.min(state.end, len));
  return { value: state.value, start, end };
}

/** Replaces the selection (or inserts at the caret) and leaves the caret
 *  after the new text. */
export function insertText(input: TextState, text: string): TextState {
  const s = clamp(input);
  const value = s.value.slice(0, s.start) + text + s.value.slice(s.end);
  const caret = s.start + text.length;
  return { value, start: caret, end: caret };
}

/** Composition replaces a suffix before the caret along with any selection. */
export function replaceBeforeCaret(input: TextState, count: number, text: string): TextState {
  const s = clamp(input);
  return insertText({ ...s, start: Math.max(0, s.start - Math.max(0, count)) }, text);
}

const isLowSurrogate = (c: number): boolean => c >= 0xdc00 && c <= 0xdfff;
const isHighSurrogate = (c: number): boolean => c >= 0xd800 && c <= 0xdbff;
/** Marks that belong to the character before them: combining diacritics,
 *  variation selectors, emoji skin tones. */
function isTrailingMark(cp: number): boolean {
  return (cp >= 0x0300 && cp <= 0x036f) || (cp >= 0xfe00 && cp <= 0xfe0f) || (cp >= 0x1f3fb && cp <= 0x1f3ff) || cp === 0x20e3;
}
const ZWJ = 0x200d;

/** Code point ending at `pos` (exclusive) and where it starts. */
function codePointBefore(value: string, pos: number): { start: number; cp: number } {
  const low = value.charCodeAt(pos - 1);
  if (isLowSurrogate(low) && pos >= 2 && isHighSurrogate(value.charCodeAt(pos - 2))) {
    return { start: pos - 2, cp: value.codePointAt(pos - 2) ?? low };
  }
  return { start: pos - 1, cp: low };
}

/** Where the grapheme that ends at `pos` begins. Without Intl.Segmenter
 *  (Chromium 83 has none) this covers what a keyboard needs: a base code
 *  point with its combining marks, variation selectors and skin tones, and
 *  ZWJ sequences such as family or profession emoji. */
export function graphemeStart(value: string, pos: number): number {
  if (pos <= 0) return 0;
  let { start, cp } = codePointBefore(value, pos);
  // Marks hang off what precedes them; keep walking back to the base.
  while (start > 0 && isTrailingMark(cp)) ({ start, cp } = codePointBefore(value, start));
  // A ZWJ joins this base to the one before it, which has its own marks.
  while (start > 0 && value.charCodeAt(start - 1) === ZWJ && start - 1 > 0) {
    let { start: s, cp: c } = codePointBefore(value, start - 1);
    while (s > 0 && isTrailingMark(c)) ({ start: s, cp: c } = codePointBefore(value, s));
    start = s;
  }
  return start;
}

/** Backspace: a selection goes as a whole, otherwise one grapheme. */
export function deleteBackward(input: TextState): TextState {
  const s = clamp(input);
  if (s.start !== s.end) {
    const value = s.value.slice(0, s.start) + s.value.slice(s.end);
    return { value, start: s.start, end: s.start };
  }
  if (s.start === 0) return s;
  const from = graphemeStart(s.value, s.start);
  return { value: s.value.slice(0, from) + s.value.slice(s.start), start: from, end: from };
}

/** The caret moves one grapheme; a selection collapses to its edge first. */
export function moveCaret(input: TextState, delta: -1 | 1): TextState {
  const s = clamp(input);
  if (s.start !== s.end) {
    const pos = delta < 0 ? s.start : s.end;
    return { value: s.value, start: pos, end: pos };
  }
  let pos = s.start;
  if (delta < 0) pos = graphemeStart(s.value, pos);
  else if (pos < s.value.length) {
    // Forward: step over the code point, then any marks or ZWJ continuation.
    const cp = s.value.codePointAt(pos) ?? 0;
    pos += cp > 0xffff ? 2 : 1;
    for (;;) {
      if (pos >= s.value.length) break;
      const next = s.value.codePointAt(pos) ?? 0;
      if (isTrailingMark(next)) { pos += next > 0xffff ? 2 : 1; continue; }
      if (next === ZWJ && pos + 1 < s.value.length) {
        pos += 1;
        const joined = s.value.codePointAt(pos) ?? 0;
        pos += joined > 0xffff ? 2 : 1;
        continue;
      }
      break;
    }
  }
  return { value: s.value, start: pos, end: pos };
}
