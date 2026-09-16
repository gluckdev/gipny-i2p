/**
 * Light / dark / follow-the-system.
 *
 * Kept in localStorage, not the encrypted settings: the theme has to be right on
 * the profile screen, before any vault is unlocked, and a colour preference is
 * not something worth keeping secret. Applied to <html> as data-theme, which
 * styles.css keys the palettes off.
 */
export type Theme = 'light' | 'dark' | 'system';

const KEY = 'gipny.theme';
/** Light is the default because it is what was asked for. */
const DEFAULT: Theme = 'light';

export function getTheme(): Theme {
  try {
    const v = localStorage.getItem(KEY);
    if (v === 'light' || v === 'dark' || v === 'system') return v;
  } catch {
    // Storage can be unavailable; fall through to the default.
  }
  return DEFAULT;
}

export function applyTheme(theme: Theme = getTheme()): void {
  document.documentElement.setAttribute('data-theme', theme);
}

export function setTheme(theme: Theme): void {
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    // Not persisted, but still applied for this session.
  }
  applyTheme(theme);
}
