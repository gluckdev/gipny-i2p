/**
 * Key tables for the on-screen keyboard. A layout is three rows of letters;
 * the component adds shift, backspace and the bottom row itself. Shift is
 * `toUpperCase()` unless `shift` says otherwise (ß stays ß: 'SS' is a word,
 * not a key). Vietnamese (Telex) and Khmer (NiDA) are the next slice — see
 * docs/plans/2026-11-08-own-keyboard.md.
 */
export type LayoutId = 'ru' | 'en' | 'de';

export interface Layout {
  id: LayoutId;
  /** What the layout key shows: the language code people know. */
  label: string;
  rows: readonly (readonly string[])[];
  shift?: Readonly<Record<string, string>>;
}

const split = (row: string): string[] => [...row];

export const LAYOUTS: Readonly<Record<LayoutId, Layout>> = {
  ru: {
    id: 'ru', label: 'RU',
    rows: [split('йцукенгшщзхъ'), split('фывапролджэ'), split('ячсмитьбюё')],
  },
  en: {
    id: 'en', label: 'EN',
    rows: [split('qwertyuiop'), split('asdfghjkl'), split('zxcvbnm')],
  },
  de: {
    id: 'de', label: 'DE',
    rows: [split('qwertzuiopü'), split('asdfghjklöä'), split('yxcvbnmß')],
    shift: { 'ß': 'ß' },
  },
};

export const LAYOUT_IDS: readonly LayoutId[] = ['ru', 'en', 'de'];

export function isLayoutId(id: string): id is LayoutId {
  return (LAYOUT_IDS as readonly string[]).indexOf(id) !== -1;
}

/** The shifted form of a letter key. */
export function shifted(layout: Layout, key: string): string {
  const override = layout.shift?.[key];
  if (override !== undefined) return override;
  const upper = key.toUpperCase();
  // Letters whose upper case is more than one character keep their case.
  return [...upper].length === [...key].length ? upper : key;
}

/** Digits and the punctuation a message needs; the second page holds the
 *  rest. Rows are strings of single characters, like the letter rows. */
export const SYMBOL_PAGES: readonly (readonly (readonly string[])[])[] = [
  [split('1234567890'), split('@#$_&-+()/'), split('*"\':;!?,.')],
  [split('~`|•√π÷×¶∆'), split('£€¥₽^°={}\\'), split('%©®™✓[]<>')],
];

/** The digit page for number fields: nothing a number field would refuse. */
export const DIGIT_ROWS: readonly (readonly string[])[] = [split('123'), split('456'), split('789'), ['.', '0', '-']];

/** A basic set until the owner decides on categories and search. Pairs with
 *  U+FE0F are kept as one key, so backspace removes the whole emoji. */
export const EMOJI: readonly string[] = [
  '😀', '😂', '🙂', '😉', '😍', '😘', '😎', '🤔', '😢', '😭',
  '😡', '😱', '🥳', '😴', '🤝', '👍', '👎', '👌', '✌️', '🙏',
  '👏', '💪', '❤️', '💔', '🔥', '✨', '🎉', '🎂', '☕', '🍕',
  '🍺', '⚽', '🚗', '✈️', '🏠', '📱', '💻', '📷', '🎵', '⏰',
  '✅', '❌', '❓', '❗', '💯', '🙈', '🐱', '🐶', '🌞', '🌙',
  '⭐', '🌧️', '❄️', '🍀', '🌹', '🎁', '📎', '🔒', '🔑', '🛡️',
];
