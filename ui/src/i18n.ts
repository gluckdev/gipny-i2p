/** Device-local language preference. No account, vault or network access. */
import { ru } from './locales/ru.ts';
import { en } from './locales/en.ts';

export type Lang = 'ru' | 'en';
export type TranslationKey = keyof typeof ru;
export const translations = { ru, en } as const;
const KEY = 'gipny.lang';

/** An explicit choice wins. Otherwise Russian only for a Russian system — a
 *  Russian locale among the preferred ones, or a Russian keyboard layout —
 *  and English for everyone else. The website follows the same rule. */
export function detectLang(saved: string | null, languages: readonly string[], russianLayout = false): Lang {
  if (saved === 'ru' || saved === 'en') return saved;
  return russianLayout || languages.some((tag) => /^ru(?:-|$)/i.test(tag ?? '')) ? 'ru' : 'en';
}

/** Letters only a Russian layout has together (Ukrainian lacks ы/ъ/э,
 *  Belarusian lacks ъ), from KeyboardLayoutMap values. */
export function isRussianLayout(keys: Iterable<string>): boolean {
  const letters = new Set(keys);
  return ['ы', 'ъ', 'э'].every((letter) => letters.has(letter));
}

let saved: string | null = null;
try { saved = localStorage.getItem(KEY); } catch { /* storage may be disabled */ }
let currentLang: Lang = (() => {
  if (typeof navigator === 'undefined') return detectLang(saved, []);
  const languages = [...(navigator.languages ?? []), navigator.language];
  // The system locale, which can differ from the browser's or webview's UI language.
  try { languages.push(Intl.DateTimeFormat().resolvedOptions().locale); } catch { /* restricted */ }
  return detectLang(saved, languages.filter(Boolean));
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
  saved = lang;
  try { localStorage.setItem(KEY, lang); } catch { /* keep working in memory */ }
  switchTo(lang);
}

function switchTo(lang: Lang): void {
  if (currentLang === lang) return;
  currentLang = lang;
  applyDocumentLang();
  for (const fn of [...listeners]) fn(lang);
}

// The keyboard layout is only known asynchronously, and only in Chromium
// (and its webviews). A Russian layout switches to Russian unless the person
// has chosen a language; it is a detection, so it is not saved.
type LayoutNavigator = Navigator & { keyboard?: { getLayoutMap?: () => Promise<Map<string, string>> } };
if (typeof navigator !== 'undefined' && saved !== 'ru' && saved !== 'en' && currentLang !== 'ru') {
  (navigator as LayoutNavigator).keyboard?.getLayoutMap?.()
    .then((map) => { if (saved !== 'ru' && saved !== 'en' && isRussianLayout(map.values())) switchTo('ru'); })
    .catch(() => { /* No layout information: the locale decides. */ });
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
