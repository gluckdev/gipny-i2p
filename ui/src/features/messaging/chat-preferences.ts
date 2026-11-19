import { Api } from '../../app/api';
import { DEFAULT_TTL_SECS, MAX_TTL_SECS } from '../../app/message-lifetime';
import { h } from '../../app/view';
import { t as tr } from '../localization/i18n';

const ttlUnits = [[60, 'chat.ttl_unit_minutes'], [3600, 'chat.ttl_unit_hours'], [86_400, 'chat.ttl_unit_days']] as const;

export function lifetimeControls(secs: number, choose: (secs: number) => void): [HTMLInputElement, HTMLSelectElement] {
  const scale = secs > 0 && secs % 86_400 === 0 ? 86_400 : secs > 0 && secs % 3600 === 0 ? 3600 : 60;
  const duration = h('input', { type: 'number', class: 'ttl-duration', min: 0, max: MAX_TTL_SECS / scale,
    step: 1, required: true, inputmode: 'numeric', value: secs / scale, 'aria-label': tr('chat.ttl_label') });
  const units = h('select', { class: 'ttl-units', 'aria-label': tr('chat.ttl_units') },
    ...ttlUnits.map(([value, key]) => h('option', { value }, tr(key))));
  units.value = String(scale);
  const change = (): void => {
    const factor = Number(units.value);
    duration.max = String(MAX_TTL_SECS / factor);
    if (!duration.reportValidity()) return;
    choose(duration.valueAsNumber * factor);
  };
  duration.addEventListener('change', change);
  units.addEventListener('change', change);
  return [duration, units];
}

export function chatPreferences(key: string, failed: () => void): HTMLElement {
  const lifetime = h('div', { class: 'ttl-picker' });
  const save = h('input', { type: 'checkbox', disabled: true });
  const section = h('div', { class: 'stack chat-preferences' },
    h('label', {}, tr('chat.ttl_label'), lifetime),
    h('label', {}, save, ' ', tr('chat.history_save')));
  const load = async (): Promise<void> => {
    const prefs = await Api.getChatPrefs();
    save.checked = prefs.saveHistory?.[key] ?? !prefs.clearOnLock;
    save.disabled = false;
    lifetime.replaceChildren(...lifetimeControls(prefs.chatTtlSecs?.[key] ?? prefs.ttlSecs ?? DEFAULT_TTL_SECS,
      secs => void Api.updateChatPrefs({ chatTtlSecs: { [key]: secs } }).catch(() => { failed(); void load(); })));
  };
  save.addEventListener('change', () => {
    void Api.updateChatPrefs({ saveHistory: { [key]: save.checked } }).catch(() => { failed(); void load(); });
  });
  void load().catch(failed);
  return section;
}
