import { errorText } from './error-text';
import { languagePicker } from './language-picker';
import { profileNameFor } from './profile-name';
import { t as tr } from './i18n';
import { Api, usesBrowserCore } from './api';
import type { Store, BootStep, BootStepId } from './state';
import { View, h, busy, logo } from './view';
import { salonBackdrop, salonEmblem } from './profile-salon';

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

  constructor(private store: Store) {
    super();
    // One name (owner, 2026-11-07): the one contacts see. The profile's name
    // on this device is derived from it (profile-name.ts) and shown, not asked.
    this.displayI = h('input', {
      class: 'input', placeholder: tr('auth.visible_to_contacts'), autofocus: true, maxlength: '64',
    });
    this.nameHint = h('div', { class: 'hint' });
    const paintNameHint = (): void => {
      const display = this.displayI.value.trim();
      this.nameHint.textContent = display
        ? tr('auth.name_hint', { p0: profileNameFor(display, store.profiles.get()) })
        : tr('auth.display_name_hint');
    };
    this.displayI.addEventListener('input', paintNameHint);
    paintNameHint();
    this.passI = h('input', { class: 'input', type: 'password', placeholder: tr('auth.passphrase') });
    this.confirmI = h('input', { class: 'input', type: 'password', placeholder: tr('auth.confirm_passphrase') });
    this.duressI = h('input', { class: 'input', type: 'password', placeholder: tr('auth.duress_passphrase_optional') });
    this.wipeC = h('input', { type: 'checkbox', checked: true });
    this.attemptsI = h('input', { class: 'input', type: 'number', value: usesBrowserCore() ? '0' : '10', min: '0', max: '100' });
    this.err = h('div', { class: 'err' });

    const hasProfiles = store.profiles.get().length > 0;

    this.el = h('div', { class: 'auth profile-salon salon-create' },
      salonBackdrop(),
      h('div', { class: 'auth-card salon-panel' },
        // One row on a phone (dial left, language right); stacked on desktop.
        h('div', { class: 'salon-head' }, languagePicker(), salonEmblem()),
        h('div', { class: 'auth-title' }, tr('auth.new_profile_title')),
        h('div', { class: 'auth-sub' }, tr('auth.new_profile_sub')),
        h('div', { class: 'field' },
          h('label', null, tr('auth.name')), this.displayI, this.nameHint,
        ),
        h('div', { class: 'field' }, h('label', null, tr('auth.passphrase')), this.passI),
        h('div', { class: 'field' }, h('label', null, tr('auth.confirm_pass')), this.confirmI),
        !usesBrowserCore() && h('div', { class: 'divider-text' }, tr('auth.duress_title')),
        !usesBrowserCore() && h('div', { class: 'field' }, h('label', null, tr('auth.duress_pass')), this.duressI,
          h('div', { class: 'hint' }, tr('auth.duress_pass_hint'))),
        !usesBrowserCore() && h('label', { class: 'chk auth-chk' },
          this.wipeC, h('span', { class: 'box' }),
          h('span', null, tr('auth.duress_wipe'))),
        !usesBrowserCore() && h('div', { class: 'field' }, h('label', null, tr('auth.max_attempts')), this.attemptsI),
        this.err,
        h('div', { class: 'row auth-actions' },
          hasProfiles && h('button', {
            class: 'btn btn-ghost',
            onClick: () => store.cancelToProfileSelect(),
          }, tr('common.back')),
          (() => {
            const b = h('button', {
              class: 'btn',
              style: { flex: '1' },
              onClick: () => busy(b, () => this.create()),
            }, tr('auth.create_btn')) as HTMLButtonElement;
            this.confirmI.addEventListener('keydown', (e) => {
              if ((e as KeyboardEvent).key === 'Enter') busy(b, () => this.create());
            });
            return b;
          })(),
        ),
      ),
    );
    this.passI.addEventListener('keydown', (e) => { if ((e as KeyboardEvent).key === 'Enter') this.confirmI.focus(); });
  }

  private async create(): Promise<void> {
    this.err.textContent = '';
    const display = this.displayI.value.trim();
    const pass = this.passI.value;
    const conf = this.confirmI.value;
    const duress = this.duressI.value.trim();
    const wipe = this.wipeC.checked;
    const max = parseInt(this.attemptsI.value) || 0;

    if (!display) { this.err.textContent = tr('auth.display_name_required_your_contacts_will_see_this'); return; }
    if (display.length > 64) { this.err.textContent = tr('auth.display_name_too_long_max_64'); return; }
    // Unique on this device only; the list is what the device has right now.
    const profile = profileNameFor(display, this.store.profiles.get());
    if (pass.length < 8) { this.err.textContent = tr('auth.passphrase_too_short_min_8'); return; }
    if (pass !== conf) { this.err.textContent = tr('auth.passphrases_do_not_match'); return; }
    if (duress && duress === pass) { this.err.textContent = tr('auth.duress_must_differ_from_primary'); return; }

    this.err.textContent = '';
    try {
      await this.store.beginBoot(profile, true);
      if (!this.store.isBooting(profile)) return;
      await Api.vaultCreate(profile, pass, display, duress || null, wipe, max);
      if (!this.store.isBooting(profile)) return;
      await this.store.onUnlocked(profile);
    } catch (e) {
      if (!this.store.isBooting(profile)) return;
      this.store.endBoot();
      this.store.view.set('auth-create');
      this.store.showToast(tr('auth.could_not_create_profile', { p0: (errorText(e)) }), true);
    }
  }
}

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
    this.passI = h('input', { class: 'input', type: 'password', placeholder: tr('auth.passphrase'), autofocus: true });
    this.totpI = h('input', { class: 'input', type: 'text', inputmode: 'numeric', autocomplete: 'one-time-code',
      maxlength: '6', pattern: '[0-9]{6}', placeholder: '000000', id: 'unlock-totp' });
    this.totpField = h('div', { class: 'field hidden' },
      h('label', { for: 'unlock-totp' }, tr('auth.totp_code')), this.totpI,
      h('div', { class: 'hint' }, tr('auth.totp_hint')));
    this.err = h('div', { class: 'err' });
    this.net = h('div', { class: 'auth-net' });
    this.biometricBtn = h('button', { class: 'btn btn-ghost btn-block hidden', type: 'button', disabled: true,
      title: tr('auth.biometric_prompt'), onClick: () => void this.unlock('biometric') }, tr('auth.biometric_unlock'));
    this.passkeyBtn = usesBrowserCore() ? h('button', { class: 'btn btn-ghost btn-block', type: 'button', disabled: true,
      onClick: () => void this.unlock('passkey') }, tr('browser.unlock_passkey')) : null;
    this.openBtn = h('button', { class: 'btn', style: { flex: '1' }, type: 'button', disabled: true,
      onClick: () => void this.unlock('password') }, tr('auth.open_btn'));
    this.el = h('div', { class: 'auth login-cyber' },
      h('div', { class: 'auth-card' },
        logo(),
        languagePicker(),
        h('div', { class: 'auth-title' }, profile),
        h('div', { class: 'auth-sub' }, tr('auth.unlock_sub')),
        h('div', { class: 'field' }, h('label', null, tr('auth.passphrase')), this.passI),
        this.totpField,
        this.err,
        this.net,
        this.biometricBtn,
        this.passkeyBtn,
        h('div', { class: 'row auth-actions' },
          h('button', {
            class: 'btn btn-ghost',
            onClick: () => store.cancelToProfileSelect(),
          }, tr('common.back')),
          this.openBtn,
        ),
      ),
    );
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
      this.store.view.set('auth-unlock');
      if (/totp|2fa|authenticator code/i.test(msg)) this.store.showToast(tr('auth.totp_invalid'), true);
      else if (msg.includes('wiped')) this.store.showToast(tr('auth.profile_wiped'), true);
      else if (msg.includes('invalid passphrase')) this.store.showToast(tr('auth.wrong_pass'), true);
      else if (method === 'biometric') this.store.showToast(tr('auth.biometric_failed'), true);
      else this.store.showToast(errorText(msg), true);
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

export class AuthBooting extends View {
  el: HTMLElement;
  private titleEl: HTMLElement;
  private summaryEl: HTMLElement;
  private detailsEl: HTMLElement;
  private networkEl: HTMLElement;
  private rows = new Map<BootStepId, HTMLElement>();
  private elapsedEl: HTMLElement;
  private progressEl: HTMLElement;
  private progressFill: HTMLElement;
  private progressText: HTMLElement;
  private enterBtn: HTMLButtonElement;
  private backBtn: HTMLButtonElement;
  private startedAt = Date.now();
  private timer: number | null = null;

  constructor(private store: Store) {
    super();

    const list = h('div', { class: 'boot-steps' });
    for (const [id, label, hint] of bootLabels()) {
      const row = this.makeRow(label, hint);
      this.rows.set(id, row);
      list.appendChild(row);
    }

    this.elapsedEl = h('span', { class: 'boot-elapsed' }, tr('auth.elapsed_0_0_s'));
    this.progressFill = h('div', { class: 'boot-progress-fill' });
    this.progressEl = h('div', {
      class: 'boot-progress-bar', role: 'progressbar',
      'aria-label': tr('boot.title'), 'aria-valuemin': '0', 'aria-valuemax': '100',
      'aria-valuenow': '0',
    }, this.progressFill);
    this.progressText = h('span', { class: 'boot-progress-text' }, '0%');
    this.enterBtn = h('button', {
      class: 'boot-button boot-enter hidden', type: 'button',
      onClick: () => store.enterMain(),
    }, tr('auth.enter'));
    this.backBtn = h('button', {
      class: 'boot-button boot-back hidden', type: 'button',
      onClick: () => {
        store.endBoot();
        void store.cancelToProfileSelect();
      },
    }, tr('auth.back'));

    this.titleEl = h('h1', { class: 'boot-title' }, tr('auth.gipny_system_initialization'));
    this.summaryEl = h('p', { class: 'boot-status hidden', role: 'status', 'aria-live': 'polite' });
    // Shown when the router reports that nothing on the network has answered
    // for long enough that the device looks offline; withdrawn when it does.
    // Ten silent minutes on a Wi-Fi without internet is what this replaces.
    this.networkEl = h('p', { class: 'boot-network hidden', role: 'status', 'aria-live': 'polite' }, tr('boot.no_network'));
    this.detailsEl = h('div', { class: 'boot-details' },
      h('div', { class: 'boot-progress' }, this.progressEl, this.progressText),
      h('p', { class: 'boot-sub' }, tr('boot.sub')), list, this.networkEl);
    this.el = h('div', { class: 'auth boot-cyber' },
      h('div', { class: 'auth-card boot-card' },
        languagePicker(),
        h('header', { class: 'boot-header' },
          this.titleEl,
          h('div', { class: 'boot-meta' },
            h('span', { class: 'boot-node' }, tr('auth.profile', { p0: (store.currentProfile.get() ?? 'unknown') })),
            this.elapsedEl,
          ),
        ),
        this.summaryEl,
        this.detailsEl,
        h('div', { class: 'boot-foot' }, this.backBtn, this.enterBtn),
      ),
    );

    this.sub(store.bootSteps, (steps) => this.paint(steps));
    this.sub(store.bootNetwork, (verdict) => this.networkEl.classList.toggle('hidden', verdict !== 'none'));
    this.sub(store.bootCanEnter, (can) => this.enterBtn.classList.toggle('hidden', !can));
    this.sub(store.bootResuming, (resuming) => {
      this.detailsEl.classList.toggle('hidden', resuming);
      this.summaryEl.classList.toggle('hidden', !resuming);
      this.elapsedEl.classList.toggle('hidden', resuming);
      this.titleEl.textContent = resuming ? tr('boot.resuming') : tr('auth.gipny_system_initialization');
      this.paint(store.bootSteps.get());
    });
    this.sub(store.bootStage, () => this.paint(store.bootSteps.get()));
    this.timer = window.setInterval(() => this.tick(), 500);
  }

  private makeRow(label: string, hint: string): HTMLElement {
    return h('div', { class: 'boot-step boot-idle', title: hint },
      h('span', { class: 'boot-mark' }, '[  --  ]'),
      h('div', { class: 'boot-step-main' },
        h('div', { class: 'boot-step-label' }, label,
          h('span', { class: 'boot-cursor', 'aria-hidden': 'true' }, '_')),
        h('div', { class: 'boot-step-hint' }, hint),
      ),
      h('span', { class: 'boot-step-ms' }),
    );
  }

  private paint(steps: BootStep[]): void {
    // The relay and the relay network keep retrying after the chats open: a
    // failure there is not a reason to go back and sign in again.
    const background = (step: BootStep): boolean => step.id === 'relay' || step.id === 'dht';
    const failed = steps.find((step) => step.state === 'failed' && !background(step));
    this.backBtn.classList.toggle('hidden', !failed);
    if (this.store.bootResuming.get()) {
      this.summaryEl.textContent = failed?.detail || (this.store.bootStage.get() === 'done'
        ? tr('boot.ready') : tr('boot.loading'));
      return;
    }
    const marks: Record<BootStep['state'], string> = {
      done: tr('auth.ok'), active: tr('auth.run'), idle: '[  --  ]',
      failed: tr('auth.fail'), skipped: tr('auth.skip'),
    };
    for (const step of steps) {
      const row = this.rows.get(step.id);
      if (!row) continue;
      const mark = row.querySelector('.boot-mark') as HTMLElement;
      const ms = row.querySelector('.boot-step-ms') as HTMLElement;
      const hint = row.querySelector('.boot-step-hint') as HTMLElement;
      row.className = `boot-step boot-${step.state}`;
      mark.textContent = marks[step.state];
      ms.textContent = step.state === 'active' && step.startedAt
        ? fmtBootMs(Date.now() - step.startedAt)
        : step.state === 'done' ? `· ${fmtBootMs(step.ms)}` : '';
      hint.textContent = step.state !== 'failed'
        ? bootLabels().find(([id]) => id === step.id)?.[2] ?? ''
        : background(step) ? tr('boot.retrying_in_background')
        // With no log in stable builds, the reason shown here is all there is.
        : step.detail ? `${tr('boot.stage_failed')} ${errorText(step.detail)}` : tr('boot.stage_failed');
    }
    const done = steps.filter((step) => step.state === 'done' || step.state === 'skipped').length;
    const percent = steps.length ? Math.round(done / steps.length * 100) : 0;
    this.progressFill.style.width = `${percent}%`;
    this.progressText.textContent = `${percent}%`;
    this.progressEl.setAttribute('aria-valuenow', String(percent));
  }

  private tick(): void {
    this.elapsedEl.textContent = tr('auth.elapsed', { p0: (fmtSecs(Date.now() - this.startedAt)) });
    for (const active of this.store.bootSteps.get().filter((step) => step.state === 'active')) {
      if (!active.startedAt) continue;
      const ms = this.rows.get(active.id)?.querySelector('.boot-step-ms') as HTMLElement | null;
      if (ms) ms.textContent = fmtBootMs(Date.now() - active.startedAt);
    }
  }

  destroy(): void {
    if (this.timer != null) clearInterval(this.timer);
    super.destroy();
  }
}

function fmtBootMs(ms: number): string {
  return tr('view.ms', { p0: Math.max(0, Math.round(ms)) });
}

/** What each backend stage is called on screen, and what it is doing. */
const bootLabels = (): [BootStepId, string, string][] => [
  ['vault', tr('boot.step.vault'), tr('boot.step.vault_hint')],
  ['router', tr('boot.step.router'), tr('boot.step.router_hint')],
  // Inbound/outbound pools progress independently after address creation.
  ['session', tr('boot.step.session'), tr('boot.step.session_hint')],
  ['inbound', tr('boot.step.inbound'), tr('boot.step.inbound_hint')],
  ['outbound', tr('boot.step.outbound'), tr('boot.step.outbound_hint')],
  ['tunnels', tr('boot.step.tunnels'), tr('boot.step.tunnels_hint')],
  ['core', tr('boot.step.core'), tr('boot.step.core_hint')],
  ['relay', tr('boot.step.relay'), tr('boot.step.relay_hint')],
  ['dht', tr('boot.step.dht'), tr('boot.step.dht_hint')],
];

function fmtSecs(ms: number): string {
  const s = Math.max(0, ms) / 1000;
  return tr('view.s', { p0: s < 10 ? s.toFixed(1) : Math.round(s) });
}
