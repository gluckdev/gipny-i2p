/** Device-local language preference. No account, vault or network access. */
import { ru } from './locales/ru.ts';
import { en } from './locales/en.ts';

export type Lang = 'ru' | 'en';
export type TranslationKey = keyof typeof ru;
export const translations = { ru, en } as const;
const KEY = 'gipny.lang';

/** Existing preferences win; Russian browsers use Russian, all others English. */
export function detectLang(saved: string | null, languages: readonly string[]): Lang {
  if (saved === 'ru' || saved === 'en') return saved;
  return /^(ru|be|uk)(-|$)/i.test(languages[0] ?? '') ? 'ru' : 'en';
}

let currentLang: Lang = (() => {
  let saved: string | null = null;
  try { saved = localStorage.getItem(KEY); } catch { /* storage may be disabled */ }
  const languages = typeof navigator === 'undefined' ? []
    : navigator.languages?.length ? navigator.languages : [navigator.language];
  return detectLang(saved, languages);
})();
const listeners = new Set<(lang: Lang) => void>();

function applyDocumentLang(): void {
  if (typeof document !== 'undefined') document.documentElement.lang = currentLang;
}
applyDocumentLang();

export function getLang(): Lang { return currentLang; }
export function getLocale(): string { return currentLang === 'ru' ? 'ru-RU' : 'en-GB'; }

export function setLang(lang: Lang): void {
  if (lang !== 'ru' && lang !== 'en') return;
  // Persist an explicit choice even if it matches the detected language.
  try { localStorage.setItem(KEY, lang); } catch { /* keep working in memory */ }
  if (currentLang === lang) return;
  currentLang = lang;
  applyDocumentLang();
  for (const fn of [...listeners]) fn(lang);
}

export function onLangChange(fn: (lang: Lang) => void): () => void {
  listeners.add(fn);
  return () => { listeners.delete(fn); };
}

/** Callback replacement preserves literal $, $&, $` and braces in user data. */
export function t(key: TranslationKey, params?: Record<string, string | number>): string {
  const text: string = translations[currentLang][key];
  return params ? text.replace(/\{(\w+)\}/g, (match, name: string) =>
    Object.hasOwn(params, name) ? String(params[name]) : match) : text;
}
