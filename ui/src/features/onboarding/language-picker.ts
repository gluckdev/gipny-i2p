import { LANGS, getLang, getDetectedLang, setLang, t, hasLanguagePreference, type Lang } from '../localization/i18n';
import { h, View } from '../../app/view';
import { enabledLayouts, savedLayouts, setLayouts } from '../keyboard/osk';

export function needsLanguageSetup(): boolean {
  try {
    const status = localStorage.getItem('gipny.setup');
    if (status === 'done') return false;
    if (status === 'pending') return true;
  } catch { /* optional storage */ }
  // Existing installations keep their explicit device preferences.
  return !hasLanguagePreference() && savedLayouts() === null && savedLayouts('gipny.osk.layouts') === null;
}

/** App retains suggestions and explicit overrides separately across rebuilds. */
export class SetupLayoutSelection {
  private inferred: Set<Lang> | null = null;
  private choices = new Map<Lang, boolean>();

  selected(): Set<Lang> {
    if (!this.inferred) this.inferred = new Set(enabledLayouts());
    // getLayoutMap may resolve after setup first mounts. Do not freeze its
    // initial English-only suggestion or re-add an explicitly removed layout.
    this.inferred.add(getDetectedLang());
    const selected = new Set(this.inferred);
    for (const [id, enabled] of this.choices) {
      if (enabled) selected.add(id);
      else selected.delete(id);
    }
    selected.add('en');
    return selected;
  }

  choose(id: Lang, enabled: boolean): void {
    this.choices.set(id, id === 'en' || enabled);
  }
}

/** App owns this screen just like the profile/auth views; no startup overlay. */
export class LanguageSetup extends View {
  el: HTMLElement;
  constructor(layouts: SetupLayoutSelection, done: () => void) {
    super();
    try { localStorage.setItem('gipny.setup', 'pending'); } catch { /* optional */ }
    const selected = layouts.selected();
    const detected = LANGS.find(({ id }) => id === getDetectedLang())!;
    this.el = h('div', { class: 'auth' }, h('section', { class: 'auth-card language-setup' },
      h('h1', null, t('setup.title')),
      h('p', { class: 'hint' }, t('setup.detected', { language: detected.label })),
      languagePicker(),
      h('fieldset', { class: 'setup-layouts' },
        h('legend', null, t('setup.layouts')),
        ...LANGS.map(({ id, label, flag }) => {
          const input = h('input', { type: 'checkbox', value: id, checked: selected.has(id), disabled: id === 'en' });
          input.addEventListener('change', () => {
            if (id === 'en') input.checked = true;
            layouts.choose(id, input.checked);
          });
          return h('label', { class: 'opt' }, input, h('span', { lang: id }, `${flag} ${label}`));
        }),
      ),
      h('p', { class: 'hint' }, t('setup.english')),
      h('button', { class: 'btn', type: 'button', onClick: () => {
        // Merge once more before persisting, even if no UI rebuild was needed.
        setLayouts([...layouts.selected()]);
        setLang(getLang());
        try { localStorage.setItem('gipny.setup', 'done'); } catch { /* optional */ }
        done();
      } }, t('setup.continue')),
    ));
  }
}

/** Available before unlocking, including on a device with no profiles yet. */
export function languagePicker(): HTMLElement {
  const select = h('select', { class: 'input', 'aria-label': t('settings.language') },
    ...LANGS.map(({ id, label, flag }) => h('option', { value: id, lang: id }, `${flag} ${label}`)),
  );
  select.value = getLang();
  select.addEventListener('change', () => setLang(select.value as Lang));
  // The text is its own node: a phone hides it and keeps the select (aria-label).
  return h('label', { class: 'auth-language' }, h('span', { class: 'auth-language-text' }, t('settings.language')), select);
}
