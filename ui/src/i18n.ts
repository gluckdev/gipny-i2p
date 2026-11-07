/** Device-local language preference. No account, vault or network access. */
import { ru } from './locales/ru.ts';
import { en } from './locales/en.ts';
import { de } from './locales/de.ts';

export type Lang = 'ru' | 'en' | 'de';
export type TranslationKey = keyof typeof ru;
export const translations = { ru, en, de } as const;
export const LANGS: readonly { id: Lang; label: string }[] = [
  { id: 'ru', label: 'Русский' },
  { id: 'en', label: 'English' },
  { id: 'de', label: 'Deutsch' },
];
const locales: Record<Lang, string> = { ru: 'ru-RU', en: 'en-GB', de: 'de-DE' };
const KEY = 'gipny.lang';

function isLang(value: string | null): value is Lang {
  return LANGS.some(({ id }) => id === value);
}

/** An explicit choice wins; Russian locale/layout detection takes priority
 *  over other preferred languages. English is the fallback. */
export function detectLang(saved: string | null, languages: readonly string[], russianLayout = false): Lang {
  if (isLang(saved)) return saved;
  if (russianLayout || languages.some((tag) => /^ru(?:-|$)/i.test(tag ?? ''))) return 'ru';
  return LANGS.find(({ id }) => id !== 'ru' && id !== 'en' &&
    languages.some((tag) => new RegExp(`^${id}(?:-|$)`, 'i').test(tag ?? '')))?.id ?? 'en';
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
export function getLocale(): string { return locales[currentLang]; }

export function setLang(lang: Lang): void {
  if (!isLang(lang)) return;
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
if (typeof navigator !== 'undefined' && !isLang(saved) && currentLang !== 'ru') {
  (navigator as LayoutNavigator).keyboard?.getLayoutMap?.()
    .then((map) => { if (!isLang(saved) && isRussianLayout(map.values())) switchTo('ru'); })
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
