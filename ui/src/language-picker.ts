import { getLang, setLang, t, type Lang } from './i18n';
import { h } from './view';

/** Available before unlocking, including on a device with no profiles yet. */
export function languagePicker(): HTMLElement {
  const select = h('select', { class: 'input', 'aria-label': t('settings.language') },
    h('option', { value: 'en' }, t('settings.lang_en')),
    h('option', { value: 'ru' }, t('settings.lang_ru')),
  );
  select.value = getLang();
  select.addEventListener('change', () => setLang(select.value as Lang));
  return h('label', { class: 'auth-language' }, t('settings.language'), select);
}
