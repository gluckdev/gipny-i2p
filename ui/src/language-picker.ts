import { LANGS, getLang, setLang, t, type Lang } from './i18n';
import { h } from './view';

/** Available before unlocking, including on a device with no profiles yet. */
export function languagePicker(): HTMLElement {
  const select = h('select', { class: 'input', 'aria-label': t('settings.language') },
    ...LANGS.map(({ id, label }) => h('option', { value: id, lang: id }, label)),
  );
  select.value = getLang();
  select.addEventListener('change', () => setLang(select.value as Lang));
  // The text is its own node: a phone hides it and keeps the select (aria-label).
  return h('label', { class: 'auth-language' }, h('span', { class: 'auth-language-text' }, t('settings.language')), select);
}
