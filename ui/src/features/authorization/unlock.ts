import { errorText } from '../../app/error-text';
import { t as tr } from '../localization/i18n';
import { Api, usesBrowserCore } from '../../app/api';
import type { Store } from '../../app/state';
import { View, h } from '../../app/view';
import { authHeader } from '../profiles/profile-salon';

export class AuthUnlock extends View {
  el: HTMLElement;
  private passI: HTMLInputElement;
  private totpI: HTMLInputElement;
  private totpField: HTMLElement;
  private totpRequired = false;
  private biometricAvailable = false;
  private biometricBtn: HTMLButtonElement;
  private openBtn: HTMLButtonElement;
  private passkeyBtn: HTMLButtonElement | null;
  private optionsReady = false;
  private optionsRequest: Promise<void> | null = null;
  private unlocking = false;
  private disposed = false;
  private err: HTMLElement;
  /** What i2p is doing while the password is being typed. The tunnels are
   * built ahead of it, so this is the minutes that used to come after. */
  private net: HTMLElement;

  constructor(private store: Store) {
    super();
    const profile = store.currentProfile.get() ?? tr('auth.unknown');
    this.passI = h('input', { class: 'input', type: 'password', id: 'unlock-password', autocomplete: 'current-password', placeholder: tr('auth.passphrase'), autofocus: true });
    this.totpI = h('input', { class: 'input', type: 'text', inputmode: 'numeric', autocomplete: 'one-time-code',
      maxlength: '6', pattern: '[0-9]{6}', placeholder: '000000', id: 'unlock-totp' });
    this.totpField = h('div', { class: 'field hidden' },
      h('label', { for: 'unlock-totp' }, tr('auth.totp_code')), this.totpI,
      h('div', { class: 'hint' }, tr('auth.totp_hint')));
    this.err = h('div', { class: 'err' });
    this.sub(store.authError, (text) => { this.err.textContent = text ?? ''; });
    this.net = h('div', { class: 'auth-net' });
    this.biometricBtn = h('button', { class: 'btn btn-ghost btn-block hidden', type: 'button', disabled: true,
      title: tr('auth.biometric_prompt'), onClick: () => void this.unlock('biometric') }, tr('auth.biometric_unlock'));
    this.passkeyBtn = usesBrowserCore() ? h('button', { class: 'btn btn-ghost btn-block', type: 'button', disabled: true,
      onClick: () => void this.unlock('passkey') }, tr('browser.unlock_passkey')) : null;
    this.openBtn = h('button', { class: 'btn', style: { flex: '1' }, type: 'button', disabled: true,
      onClick: () => void this.unlock('password') }, tr('auth.open_btn'));
    const othersBtn = h('button', {
      class: 'btn btn-ghost btn-block auth-other-profiles', type: 'button',
      onClick: () => { if (!this.unlocking) void store.cancelToProfileSelect(); },
    });
    this.el = h('div', { class: 'auth login-cyber' },
      h('div', { class: 'auth-card' },
        authHeader(),
        h('div', { class: 'auth-title' }, profile),
        h('div', { class: 'auth-sub' }, tr('auth.unlock_sub')),
        h('div', { class: 'field' }, h('label', { for: 'unlock-password' }, tr('auth.passphrase')), this.passI),
        this.totpField,
        this.err,
        this.net,
        this.biometricBtn,
        this.passkeyBtn,
        h('div', { class: 'row auth-actions' }, this.openBtn),
        othersBtn,
      ),
    );
    this.sub(store.profiles, (profiles) => {
      const count = profiles.filter((p) => p !== store.currentProfile.get()).length;
      othersBtn.textContent = tr('auth.other_profiles', { p0: count });
    });
    this.sub(store.prewarm, (s) => {
      this.net.textContent = s === 'building'
        ? tr('auth.net_building')
        : s === 'ready' ? tr('auth.net_ready') : '';
      this.net.className = 'auth-net' + (s === 'ready' ? ' ok' : '');
    });
    for (const input of [this.passI, this.totpI]) input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); void this.unlock('password'); }
    });
    void this.loadOptions().catch(() => {});
  }

  private loadOptions(): Promise<void> {
    if (this.optionsRequest) return this.optionsRequest;
    const profile = this.store.currentProfile.get();
    this.optionsRequest = Api.vaultStatus(profile ?? '').then((status) => {
      if (this.disposed) return;
      this.totpRequired = status?.totp_enabled === true;
      this.biometricAvailable = status?.biometric_available === true;
      this.totpField.classList.toggle('hidden', !this.totpRequired);
      this.totpI.required = this.totpRequired;
      this.biometricBtn.classList.toggle('hidden', !this.biometricAvailable);
      this.optionsReady = true;
    }).catch((error) => {
      if (!this.disposed) this.err.textContent = tr('auth.security_status_failed');
      throw error;
    }).finally(() => {
      this.optionsRequest = null;
      if (!this.disposed && !this.unlocking) this.setBusy(false);
    });
    return this.optionsRequest;
  }

  private setBusy(value: boolean): void {
    this.openBtn.disabled = value;
    this.biometricBtn.disabled = value;
    if (this.passkeyBtn) this.passkeyBtn.disabled = value;
  }

  private async unlock(method: 'password' | 'biometric' | 'passkey'): Promise<void> {
    if (this.unlocking || this.disposed) return;
    this.unlocking = true; this.setBusy(true); this.err.textContent = '';
    const profile = this.store.currentProfile.get();
    try {
      if (!profile) { this.err.textContent = tr('auth.profile_not_selected'); return; }
      if (!this.optionsReady) { try { await this.loadOptions(); } catch { return; } }
      if (this.disposed || this.store.currentProfile.get() !== profile) return;
      const pass = this.passI.value;
      const code = this.totpRequired ? this.totpI.value.trim() : undefined;
      if (method === 'password' && !pass) { this.err.textContent = tr('auth.pass_required'); return; }
      if (this.totpRequired && !/^[0-9]{6}$/.test(code ?? '')) {
        this.err.textContent = tr('auth.totp_required'); this.totpI.focus(); return;
      }
      if (method === 'biometric' && !this.biometricAvailable) return;
      await this.store.beginBoot(profile);
      if (!this.store.isBooting(profile)) return;
      const warning = method === 'password' ? await Api.vaultUnlock(profile, pass, code)
        : method === 'biometric' ? await Api.vaultUnlockBiometric(profile, code)
        : await new Promise<null>((resolve, reject) => window.dispatchEvent(new CustomEvent('gipny-unlock-passkey', {
          detail: { profile, totpCode: code, done: (error?: unknown) => error ? reject(error) : resolve(null) },
        })));
      if (!this.store.isBooting(profile)) return;
      await this.store.onUnlocked(profile);
      if (warning) this.store.showToast(warning, true);
    } catch (e) {
      if (!profile || !this.store.isBooting(profile)) return;
      const msg = String(e);
      this.store.endBoot();
      const text = /totp|2fa|authenticator code/i.test(msg) ? tr('auth.totp_invalid')
        : msg.includes('wiped') ? tr('auth.profile_wiped')
        : msg.includes('invalid passphrase') ? tr('auth.wrong_pass')
        : method === 'biometric' ? tr('auth.biometric_failed')
        : errorText(msg);
      // A destructive unlock can remove the profile. Do not leave a dead
      // password form or keep that identifier as the remembered preference.
      if (msg.includes('wiped')) {
        try {
          await this.store.refreshProfiles();
          this.store.selectDefaultProfileForUnlock();
        } catch {
          this.store.authError.set(text);
          this.store.view.set('auth-unlock');
        }
      } else {
        // The same message stays in the rebuilt form once the toast has gone.
        this.store.authError.set(text);
        this.store.view.set('auth-unlock');
      }
      this.store.showToast(text, true);
    } finally {
      this.unlocking = false;
      if (!this.disposed) this.setBusy(false);
    }
  }

  override destroy(): void {
    this.disposed = true; this.passI.value = ''; this.totpI.value = '';
    super.destroy();
  }
}
