import { errorText } from '../../app/error-text';
import { profileNameFor } from './profile-name';
import { t as tr } from '../localization/i18n';
import { Api, usesBrowserCore } from '../../app/api';
import type { Store } from '../../app/state';
import { View, h } from '../../app/view';
import { salonBackdrop, authHeader } from './profile-salon';

export interface AuthCreateDraft {
  step: 1 | 2 | 3;
  displayName: string;
  password: string;
  confirmation: string;
  duressPassword: string;
  wipeOnDuress: boolean;
  maxAttempts: string;
  advancedOpen: boolean;
}

export class AuthCreate extends View {
  el: HTMLElement;
  private displayI: HTMLInputElement;
  private nameHint: HTMLElement;
  private passI: HTMLInputElement;
  private confirmI: HTMLInputElement;
  private duressI: HTMLInputElement;
  private wipeC: HTMLInputElement;
  private attemptsI: HTMLInputElement;
  private err: HTMLElement;
  private step: 1 | 2 | 3 = 1;
  private stepLabel: HTMLElement;
  private panels: HTMLElement[];
  private advanced: HTMLDetailsElement;
  private backBtn: HTMLButtonElement;
  private nextBtn: HTMLButtonElement;
  private submitting = false;
  private disposed = false;
  private readonly browser = usesBrowserCore();

  constructor(private store: Store) {
    super();
    // One name: the profile identifier on this device is derived, not asked.
    this.displayI = h('input', {
      class: 'input', placeholder: tr('auth.visible_to_contacts'), autofocus: true, maxlength: '64',
      id: 'create-name', autocomplete: 'nickname',
    });
    this.nameHint = h('div', { class: 'hint' });
    this.displayI.addEventListener('input', () => this.paintNameHint());
    this.paintNameHint();
    this.passI = h('input', { class: 'input', type: 'password', id: 'create-password', autocomplete: 'new-password', placeholder: tr('auth.passphrase') });
    this.confirmI = h('input', { class: 'input', type: 'password', id: 'create-confirm', autocomplete: 'new-password', placeholder: tr('auth.confirm_passphrase') });
    this.duressI = h('input', { class: 'input', type: 'password', id: 'create-duress', autocomplete: 'new-password', placeholder: tr('auth.duress_passphrase_optional') });
    this.wipeC = h('input', { type: 'checkbox', checked: true });
    this.attemptsI = h('input', { class: 'input', type: 'number', id: 'create-attempts', value: this.browser ? '0' : '10', min: '0', max: '100' });
    this.err = h('div', { class: 'err', role: 'alert' });
    this.sub(store.authError, (text) => { this.err.textContent = text ?? ''; });
    this.stepLabel = h('div', { class: 'auth-step', role: 'status', 'aria-live': 'polite' });
    this.advanced = h('details', { class: 'auth-security' },
      h('summary', null, tr('auth.advanced_options')),
      h('div', { class: 'auth-security-body' },
        h('label', { class: 'chk auth-chk' }, this.wipeC, h('span', { class: 'box' }),
          h('span', null, tr('auth.duress_wipe'))),
        h('div', { class: 'field' }, h('label', { for: 'create-attempts' }, tr('auth.max_attempts')), this.attemptsI),
      ),
    );
    this.panels = [
      h('section', { class: 'auth-create-step', 'data-step': '1' },
        h('div', { class: 'field' }, h('label', { for: 'create-name' }, tr('auth.name')), this.displayI, this.nameHint)),
      h('section', { class: 'auth-create-step hidden', 'data-step': '2' },
        h('p', { class: 'auth-local-data' }, tr('auth.local_data_hint')),
        h('div', { class: 'field' }, h('label', { for: 'create-password' }, tr('auth.passphrase')), this.passI),
        h('div', { class: 'field' }, h('label', { for: 'create-confirm' }, tr('auth.confirm_pass')), this.confirmI)),
      h('section', { class: 'auth-create-step hidden', 'data-step': '3' },
        h('div', { class: 'auth-sub' }, tr('auth.duress_title')),
        h('p', { class: 'auth-duress-explanation' }, tr(this.browser ? 'auth.duress_unavailable_browser' : 'auth.duress_setup_hint')),
        !this.browser && h('div', { class: 'field' },
          h('label', { for: 'create-duress' }, tr('auth.duress_passphrase_optional')), this.duressI),
        !this.browser && this.advanced),
    ];
    this.backBtn = h('button', {
      class: 'btn btn-ghost', type: 'button', onClick: () => {
        if (this.submitting) return;
        if (this.step > 1) this.showStep((this.step - 1) as 1 | 2);
        else if (this.browser && !store.profiles.get().length) store.view.set('profile-select');
        else void store.cancelToProfileSelect();
      },
    }, tr('common.back'));
    this.nextBtn = h('button', {
      class: 'btn', type: 'button', style: { flex: '1' },
      onClick: () => { void this.advance(); },
    }, tr('auth.next'));
    this.el = h('div', { class: 'auth profile-salon salon-create' },
      salonBackdrop(),
      h('div', { class: 'auth-card salon-panel' },
        authHeader(),
        h('div', { class: 'auth-title' }, tr('auth.new_profile_title')),
        this.stepLabel,
        ...this.panels,
        this.err,
        h('div', { class: 'row auth-actions' }, this.backBtn, this.nextBtn),
      ),
    );
    this.showStep(1, false);
    for (const input of [this.displayI, this.confirmI, this.duressI]) input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.isComposing) {
        e.preventDefault();
        void this.advance();
      }
    });
    this.passI.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); this.confirmI.focus(); }
    });
  }

  private paintNameHint(): void {
    const display = this.displayI.value.trim();
    this.nameHint.textContent = display
      ? tr('auth.name_hint', { p0: profileNameFor(display, this.store.profiles.get()) })
      : tr('auth.display_name_hint');
  }

  private showStep(step: 1 | 2 | 3, focus = true): void {
    this.step = step;
    this.panels.forEach((panel, i) => {
      panel.hidden = i + 1 !== step;
      panel.classList.toggle('hidden', panel.hidden);
    });
    this.stepLabel.textContent = tr('auth.create_step', { p0: step });
    const firstProfile = step === 1 && !this.store.profiles.get().length;
    this.backBtn.classList.toggle('hidden', firstProfile && !this.browser);
    this.backBtn.textContent = tr(firstProfile && this.browser ? 'profile.select_title' : 'common.back');
    this.nextBtn.textContent = tr(step === 3 ? 'auth.create_btn' : 'auth.next');
    if (focus) {
      const target = step === 1 ? this.displayI : step === 2 ? this.passI : this.browser ? this.nextBtn : this.duressI;
      target.focus();
    }
  }

  /** Only for rebuilding this view (e.g. a language switch). Never persist. */
  captureDraft(): AuthCreateDraft {
    return {
      step: this.step, displayName: this.displayI.value, password: this.passI.value,
      confirmation: this.confirmI.value, duressPassword: this.browser ? '' : this.duressI.value,
      wipeOnDuress: this.wipeC.checked, maxAttempts: this.attemptsI.value, advancedOpen: this.advanced.open,
    };
  }

  restoreDraft(draft: AuthCreateDraft | null): void {
    if (!draft || this.disposed || this.submitting) return;
    this.displayI.value = draft.displayName;
    this.passI.value = draft.password;
    this.confirmI.value = draft.confirmation;
    this.duressI.value = this.browser ? '' : draft.duressPassword;
    this.wipeC.checked = draft.wipeOnDuress;
    this.attemptsI.value = this.browser ? '0' : draft.maxAttempts;
    this.advanced.open = draft.advancedOpen;
    this.paintNameHint();
    // Restoring a step is presentation only: advancing/creating validates again.
    this.showStep(draft.step === 2 || draft.step === 3 ? draft.step : 1, false);
  }

  private validateName(): boolean {
    const display = this.displayI.value.trim();
    if (!display || display.length > 64) {
      this.showStep(1);
      this.err.textContent = tr(!display ? 'auth.display_name_required_your_contacts_will_see_this' : 'auth.display_name_too_long_max_64');
      return false;
    }
    return true;
  }

  private validatePassword(): boolean {
    if (this.passI.value.length < 8 || this.passI.value !== this.confirmI.value) {
      this.showStep(2);
      this.err.textContent = tr(this.passI.value.length < 8 ? 'auth.passphrase_too_short_min_8' : 'auth.passphrases_do_not_match');
      return false;
    }
    return true;
  }

  private async advance(): Promise<void> {
    if (this.disposed || this.submitting) return;
    this.err.textContent = '';
    if (this.step === 1) { if (this.validateName()) this.showStep(2); }
    else if (this.step === 2) { if (this.validateName() && this.validatePassword()) this.showStep(3); }
    else await this.create();
  }

  private async create(): Promise<void> {
    if (this.disposed || this.submitting) return;
    this.err.textContent = '';
    if (!this.validateName() || !this.validatePassword()) return;
    const display = this.displayI.value.trim();
    const pass = this.passI.value;
    // Do not trim passwords: a space can intentionally distinguish duress.
    const duress = this.browser ? '' : this.duressI.value;
    const wipe = this.browser ? false : this.wipeC.checked;
    const max = this.browser ? 0 : Number(this.attemptsI.value);
    const profile = profileNameFor(display, this.store.profiles.get());
    if (duress && duress === pass) { this.showStep(3); this.err.textContent = tr('auth.duress_must_differ_from_primary'); return; }
    if (!Number.isInteger(max) || max < 0 || max > 100) {
      this.showStep(3); this.advanced.open = true;
      this.err.textContent = tr('auth.invalid_max_attempts'); this.attemptsI.focus(); return;
    }

    this.submitting = true;
    this.backBtn.disabled = true;
    this.nextBtn.disabled = true;
    try {
      await this.store.beginBoot(profile, true);
      if (!this.store.isBooting(profile)) return;
      await Api.vaultCreate(profile, pass, display, duress || null, wipe, max);
      if (!this.store.isBooting(profile)) return;
      await this.store.onUnlocked(profile);
    } catch (e) {
      if (!this.store.isBooting(profile)) return;
      this.store.endBoot();
      const text = tr('auth.could_not_create_profile', { p0: errorText(e) });
      this.store.authError.set(text);
      this.store.view.set('auth-create');
      this.store.showToast(text, true);
    } finally {
      this.submitting = false;
      if (!this.disposed) { this.backBtn.disabled = false; this.nextBtn.disabled = false; }
    }
  }

  override destroy(): void {
    this.disposed = true;
    for (const input of [this.displayI, this.passI, this.confirmI, this.duressI]) input.value = '';
    super.destroy();
  }
}
