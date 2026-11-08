/** Device-local language preference. No account, vault or network access. */
import { ru } from './locales/ru.ts';
import { en } from './locales/en.ts';
import { de } from './locales/de.ts';
import { uk } from './locales/uk.ts';
import { km } from './locales/km.ts';
import { vi } from './locales/vi.ts';
import { hi } from './locales/hi.ts';
import { zh } from './locales/zh.ts';

export type Lang = 'ru' | 'en' | 'uk' | 'de' | 'km' | 'vi' | 'hi' | 'zh';
export type TranslationKey = keyof typeof ru;
export const translations = { ru, en, uk, de, km, vi, hi, zh } as const;
export const LANGS: readonly { id: Lang; label: string }[] = [
  { id: 'ru', label: 'Русский' },
  { id: 'en', label: 'English' },
  { id: 'uk', label: 'Українська' },
  { id: 'de', label: 'Deutsch' },
  { id: 'km', label: 'ខ្មែរ' },
  { id: 'vi', label: 'Tiếng Việt' },
  { id: 'hi', label: 'हिन्दी' },
  { id: 'zh', label: '中文' },
];
const locales: Record<Lang, string> = { ru: 'ru-RU', en: 'en-GB', uk: 'uk-UA', de: 'de-DE', km: 'km-KH', vi: 'vi-VN', hi: 'hi-IN', zh: 'zh-CN' };
const KEY = 'gipny.lang';

function isLang(value: string | null): value is Lang {
  return LANGS.some(({ id }) => id === value);
}

/** An explicit preference wins; Russian system languages/layouts take priority. */
export function detectLang(saved: string | null, languages: readonly string[], layoutOrRussian: boolean | Lang | Iterable<string> | null = false): Lang {
  if (isLang(saved)) return saved;
  const kbdLang = layoutOrRussian === true ? 'ru'
    : typeof layoutOrRussian === 'string' && isLang(layoutOrRussian) ? layoutOrRussian
    : layoutOrRussian && typeof layoutOrRussian === 'object' ? detectKeyboardLang(layoutOrRussian) : null;
  if (kbdLang === 'ru' || languages.some(tag => /^ru(?:-|$)/i.test(tag ?? ''))) return 'ru';
  for (const tag of languages) {
    const lang = LANGS.find(({ id }) => new RegExp(`^${id}(?:-|$)`, 'i').test(tag ?? ''))?.id;
    if (lang) return lang === 'en' && kbdLang ? kbdLang : lang;
  }
  return kbdLang ?? 'en';
}

export function isRussianLayout(keys: Iterable<string>): boolean {
  const letters = new Set(Array.from(keys, key => key.toLowerCase()));
  return ['ы', 'ъ', 'э'].every(letter => letters.has(letter));
}

/** Infer a layout from KeyboardLayoutMap values, not physical key codes. */
export function detectKeyboardLang(keys: Iterable<string>): Lang | null {
  const letters = new Set(Array.from(keys, key => key.toLowerCase()));
  if (isRussianLayout(letters)) return 'ru';
  if (['і', 'ї', 'є'].every(letter => letters.has(letter))) return 'uk';
  if (['ä', 'ö', 'ü'].every(letter => letters.has(letter)) || letters.has('ß')) return 'de';
  const text = [...letters].join('');
  if (/[\u1780-\u17FF]/.test(text)) return 'km';
  if (/[ăâđêôơư]/i.test(text)) return 'vi';
  if (/[\u0900-\u097F]/.test(text)) return 'hi';
  if (/[\u3100-\u312F\u4E00-\u9FFF]/.test(text)) return 'zh';
  return null;
}

function systemLanguages(): string[] {
  const languages = typeof navigator === 'undefined' ? [] : [...(navigator.languages ?? []), navigator.language];
  try { languages.push(Intl.DateTimeFormat().resolvedOptions().locale); } catch { /* restricted */ }
  return languages.filter(Boolean);
}

/** Unique supported browser/system languages in preference order, then the keyboard. */
export function detectSystemLangs(languages: readonly string[] = systemLanguages(), layoutKeys?: Iterable<string> | null): Lang[] {
  const result: Lang[] = [];
  for (const tag of languages) {
    const lang = LANGS.find(({ id }) => new RegExp(`^${id}(?:-|$)`, 'i').test(tag ?? ''))?.id;
    if (lang && !result.includes(lang)) result.push(lang);
  }
  const keyboard = layoutKeys ? detectKeyboardLang(layoutKeys) : null;
  if (keyboard && !result.includes(keyboard)) result.push(keyboard);
  return result;
}

let saved: string | null = null;
try { saved = localStorage.getItem(KEY); } catch { /* storage may be disabled */ }
let currentLang: Lang = detectLang(saved, systemLanguages());
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

// Automatic detection never persists a choice, and a choice made while the
// browser resolves its layout map still wins.
type LayoutNavigator = Navigator & { keyboard?: { getLayoutMap?: () => Promise<Map<string, string>> } };
if (typeof navigator !== 'undefined' && !isLang(saved)) {
  try {
    (navigator as LayoutNavigator).keyboard?.getLayoutMap?.()
      .then(map => {
        const kbd = detectKeyboardLang(map.values());
        if (!isLang(saved) && kbd && (kbd === 'ru' || currentLang === 'en')) switchTo(kbd);
      }).catch(() => { /* No layout information: the locale decides. */ });
  } catch { /* Some webviews expose the API but refuse it synchronously. */ }
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
