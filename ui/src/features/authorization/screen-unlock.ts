import { Api, isTauri, usesBrowserCore } from '../../app/api';
import { errorText } from '../../app/error-text';
import { Store } from '../../app/state';
import { h, View } from '../../app/view';
import { t } from '../localization/i18n';
import { authHeader } from '../profiles/profile-salon';

export class ScreenUnlock extends View {
  el: HTMLElement;
  constructor(store: Store) {
    super();
    const pass = h('input', { class: 'input', type: 'password', autocomplete: 'current-password', id: 'screen-pass' });
    const error = h('div', { class: 'err', role: 'alert' });
    const open = h('button', { type: 'submit', class: 'btn' }, t('auth.open_btn'));
    const provider = h('button', { type: 'button', class: 'btn btn-ghost hidden' }, isTauri ? t('auth.device_unlock') : t('browser.unlock_passkey'));
    const verify = async (check: (profile: string) => Promise<void>, systemPrompt = false): Promise<void> => {
      if (open.disabled) return;
      open.disabled = provider.disabled = true;
      error.textContent = '';
      try { await store.revealScreen(check, systemPrompt); }
      catch (cause) { error.textContent = errorText(cause); }
      finally { pass.value = ''; open.disabled = provider.disabled = false; }
    };
    const form = h('form', { class: 'auth-card', onSubmit: (event: SubmitEvent) => {
      event.preventDefault();
      if (pass.value) void verify(profile => Api.verifyScreenPassword(profile, pass.value));
    } }, authHeader(), h('h1', { class: 'auth-title' }, t('auth.screen_locked')),
      h('p', { class: 'auth-sub' }, t('auth.screen_unlock_hint')),
      h('div', { class: 'field' }, h('label', { for: 'screen-pass' }, t('auth.passphrase')), pass),
      error, open, provider,
      h('button', { type: 'button', class: 'btn btn-ghost', onClick: () => {
        void store.lockAndClose().catch(cause => { error.textContent = errorText(cause); });
      } }, t('auth.lock_and_close')));
    if (usesBrowserCore()) {
      const profile = store.currentProfile.get();
      if (profile) void Api.hasScreenPasskey(profile).then(available => provider.classList.toggle('hidden', !available)).catch(() => {});
      provider.onclick = () => { void verify(profile => Api.verifyScreenPasskey(profile), true); };
    } else if (isTauri) {
      void Api.deviceScreenAuthAvailable().then(available => provider.classList.toggle('hidden', !available)).catch(() => {});
      provider.onclick = () => { void verify(profile => Api.verifyDeviceScreenAuth(profile, t('auth.screen_locked')), true); };
    }
    this.el = h('div', { class: 'auth login-cyber' }, form);
  }
}
