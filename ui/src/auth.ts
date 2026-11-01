import { errorText } from './error-text';
import { languagePicker } from './language-picker';
import { t as tr } from './i18n';
import { Api, usesBrowserCore } from './api';
import type { Store, BootStep, BootStepId } from './state';
import { View, h, busy, logo } from './view';

export class AuthCreate extends View {
  el: HTMLElement;
  private nameI: HTMLInputElement;
  private displayI: HTMLInputElement;
  private passI: HTMLInputElement;
  private confirmI: HTMLInputElement;
  private duressI: HTMLInputElement;
  private wipeC: HTMLInputElement;
  private attemptsI: HTMLInputElement;
  private err: HTMLElement;

  constructor(private store: Store) {
    super();
    this.nameI = h('input', {
      class: 'input', placeholder: tr('auth.e_g_alice'), autofocus: true, maxlength: '32',
    });
    this.displayI = h('input', {
      class: 'input', placeholder: tr('auth.visible_to_contacts'), maxlength: '64',
    });
    this.passI = h('input', { class: 'input', type: 'password', placeholder: tr('auth.passphrase') });
    this.confirmI = h('input', { class: 'input', type: 'password', placeholder: tr('auth.confirm_passphrase') });
    this.duressI = h('input', { class: 'input', type: 'password', placeholder: tr('auth.duress_passphrase_optional') });
    this.wipeC = h('input', { type: 'checkbox', checked: true });
    this.attemptsI = h('input', { class: 'input', type: 'number', value: usesBrowserCore() ? '0' : '10', min: '0', max: '100' });
    this.err = h('div', { class: 'err' });

    const hasProfiles = store.profiles.get().length > 0;

    this.el = h('div', { class: 'auth' },
      h('div', { class: 'auth-card' },
        logo(),
        languagePicker(),
        h('div', { class: 'auth-title' }, tr('auth.new_profile_title')),
        h('div', { class: 'auth-sub' }, tr('auth.new_profile_sub')),
        h('div', { class: 'field' },
          h('label', null, tr('auth.profile_name')), this.nameI,
          h('div', { class: 'hint' }, tr('auth.profile_name_hint')),
        ),
        h('div', { class: 'field' },
          h('label', null, tr('auth.display_name')), this.displayI,
          h('div', { class: 'hint' }, tr('auth.display_name_hint')),
        ),
        h('div', { class: 'field' }, h('label', null, tr('auth.passphrase')), this.passI),
        h('div', { class: 'field' }, h('label', null, tr('auth.confirm_pass')), this.confirmI),
        !usesBrowserCore() && h('div', { class: 'divider-text' }, tr('auth.duress_title')),
        !usesBrowserCore() && h('div', { class: 'field' }, h('label', null, tr('auth.duress_pass')), this.duressI,
          h('div', { class: 'hint' }, tr('auth.duress_pass_hint'))),
        !usesBrowserCore() && h('label', { class: 'chk', style: { marginBottom: '14px' } },
          this.wipeC, h('span', { class: 'box' }),
          h('span', null, tr('auth.duress_wipe'))),
        !usesBrowserCore() && h('div', { class: 'field' }, h('label', null, tr('auth.max_attempts')), this.attemptsI),
        this.err,
        h('div', { class: 'row', style: { marginTop: '18px', gap: '8px' } },
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
    const profile = this.nameI.value.trim();
    const display = this.displayI.value.trim();
    const pass = this.passI.value;
    const conf = this.confirmI.value;
    const duress = this.duressI.value.trim();
    const wipe = this.wipeC.checked;
    const max = parseInt(this.attemptsI.value) || 0;

    if (!profile) { this.err.textContent = tr('auth.profile_name_required'); return; }
    if (!/^[A-Za-z0-9_-]{1,32}$/.test(profile)) {
      this.err.textContent = tr('auth.profile_name_letters_digits_and_up_to_32');
      return;
    }
    if (!display) { this.err.textContent = tr('auth.display_name_required_your_contacts_will_see_this'); return; }
    if (display.length > 64) { this.err.textContent = tr('auth.display_name_too_long_max_64'); return; }
    if (pass.length < 8) { this.err.textContent = tr('auth.passphrase_too_short_min_8'); return; }
    if (pass !== conf) { this.err.textContent = tr('auth.passphrases_do_not_match'); return; }
    if (duress && duress === pass) { this.err.textContent = tr('auth.duress_must_differ_from_primary'); return; }

    this.err.textContent = '';
    await this.store.beginBoot(profile);
    try {
      await Api.vaultCreate(profile, pass, display, duress || null, wipe, max);
      await this.store.onUnlocked(profile);
    } catch (e) {
      this.store.endBoot();
      this.store.view.set('auth-create');
      this.err.textContent = tr('auth.could_not_create_profile', { p0: (errorText(e)) });
    }
  }
}

export class AuthUnlock extends View {
  el: HTMLElement;
  private passI: HTMLInputElement;
  private err: HTMLElement;
  /** What i2p is doing while the password is being typed. The tunnels are
   * built ahead of it, so this is the minutes that used to come after. */
  private net: HTMLElement;

  constructor(private store: Store) {
    super();
    const profile = store.currentProfile.get() ?? tr('auth.unknown');
    this.passI = h('input', { class: 'input', type: 'password', placeholder: tr('auth.passphrase'), autofocus: true });
    this.err = h('div', { class: 'err' });
    this.net = h('div', { class: 'auth-net' });
    this.el = h('div', { class: 'auth' },
      h('div', { class: 'auth-card' },
        logo(),
        languagePicker(),
        h('div', { class: 'auth-title' }, profile),
        h('div', { class: 'auth-sub' }, tr('auth.unlock_sub')),
        h('div', { class: 'field' }, h('label', null, tr('auth.passphrase')), this.passI),
        this.err,
        this.net,
        usesBrowserCore() && (() => {
          const button = h('button', { class: 'btn btn-ghost btn-block', onClick: () => busy(button as HTMLButtonElement, () => this.unlockPasskey()) }, tr('browser.unlock_passkey'));
          return button;
        })(),
        h('div', { class: 'row', style: { marginTop: '18px', gap: '8px' } },
          h('button', {
            class: 'btn btn-ghost',
            onClick: () => store.cancelToProfileSelect(),
          }, tr('common.back')),
          (() => {
            const b = h('button', {
              class: 'btn', style: { flex: '1' },
              onClick: () => busy(b, () => this.unlock()),
            }, tr('auth.open_btn')) as HTMLButtonElement;
            this.passI.addEventListener('keydown', (e) => {
              if ((e as KeyboardEvent).key === 'Enter') busy(b, () => this.unlock());
            });
            return b;
          })(),
        ),
      ),
    );
    this.sub(store.prewarm, (s) => {
      this.net.textContent = s === 'building'
        ? tr('auth.net_building')
        : s === 'ready' ? tr('auth.net_ready') : '';
      this.net.className = 'auth-net' + (s === 'ready' ? ' ok' : '');
    });
  }

  private async unlock(): Promise<void> {
    this.err.textContent = '';
    const profile = this.store.currentProfile.get();
    if (!profile) { this.err.textContent = tr('auth.profile_not_selected'); return; }
    const pass = this.passI.value;
    if (!pass) { this.err.textContent = tr('auth.pass_required'); return; }
    // The loading screen goes up *before* the call: everything slow (argon2id,
    // the router, tunnels) happens inside it, and this used to be a disabled
    // button and nothing else for up to three minutes.
    await this.store.beginBoot(profile);
    try {
      const warning = await Api.vaultUnlock(profile, pass);
      await this.store.onUnlocked(profile);
      if (warning) this.store.showToast(warning, true);
    } catch (e) {
      const msg = String(e);
      this.store.endBoot();
      this.store.view.set('auth-unlock');
      if (msg.includes('wiped')) this.store.showToast(tr('auth.profile_wiped'), true);
      else if (msg.includes('invalid passphrase')) this.store.showToast(tr('auth.wrong_pass'), true);
      else this.store.showToast(errorText(msg), true);
    }
  }

  private async unlockPasskey(): Promise<void> {
    const profile = this.store.currentProfile.get();
    if (!profile) return;
    await this.store.beginBoot(profile);
    try {
      await new Promise<void>((resolve, reject) => window.dispatchEvent(new CustomEvent('gipny-unlock-passkey', { detail: { profile, done: (error?: unknown) => error ? reject(error) : resolve() } })));
      await this.store.onUnlocked(profile);
    } catch (error) {
      this.store.endBoot(); this.store.view.set('auth-unlock');
      this.store.showToast(errorText(error), true);
    }
  }
}

export class AuthBooting extends View {
  el: HTMLElement;
  private rows = new Map<BootStepId, HTMLElement>();
  private elapsedEl: HTMLElement;
  private progressEl: HTMLElement;
  private progressFill: HTMLElement;
  private progressText: HTMLElement;
  private logEl: HTMLElement;
  private logPanel: HTMLElement;
  private logToggle: HTMLButtonElement;
  private logExpanded = false;
  private logFrame: number | null = null;
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
    this.logEl = h('div', {
      id: 'boot-live-output', class: 'boot-log', role: 'log',
      'aria-label': tr('auth.live_system_log'), 'aria-live': 'off', tabindex: '0',
    });
    this.logToggle = h('button', {
      class: 'boot-log-toggle', type: 'button',
      'aria-expanded': 'false', 'aria-controls': 'boot-live-output',
      'aria-label': tr('boot.details'), title: tr('boot.details'),
      onClick: () => {
        this.logExpanded = !this.logExpanded;
        this.logPanel.classList.toggle('is-expanded', this.logExpanded);
        this.logToggle.setAttribute('aria-expanded', String(this.logExpanded));
        this.logToggle.textContent = this.logExpanded ? '[ − ]' : '[ + ]';
        this.renderLog(this.store.bootLog.get());
      },
    }, '[ + ]');
    this.logPanel = h('section', { class: 'boot-live-log' },
      h('div', { class: 'boot-log-head' },
        h('span', null, h('span', { class: 'boot-live-dot', 'aria-hidden': 'true' }), tr('auth.live_system_log')),
        this.logToggle,
      ),
      this.logEl,
    );
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

    this.el = h('div', { class: 'auth boot-cyber' },
      h('div', { class: 'auth-card boot-card' },
        languagePicker(),
        h('header', { class: 'boot-header' },
          h('h1', { class: 'boot-title' }, tr('auth.gipny_system_initialization')),
          h('div', { class: 'boot-meta' },
            h('span', { class: 'boot-node' }, tr('auth.profile', { p0: (store.currentProfile.get() ?? 'unknown') })),
            this.elapsedEl,
          ),
        ),
        h('div', { class: 'boot-progress' }, this.progressEl, this.progressText),
        h('p', { class: 'boot-sub' }, tr('boot.sub')),
        list,
        this.logPanel,
        h('div', { class: 'boot-foot' }, this.backBtn, this.enterBtn),
      ),
    );

    this.sub(store.bootSteps, (steps) => this.paint(steps));
    this.sub(store.bootLog, (lines) => this.renderLog(lines));
    this.sub(store.bootCanEnter, (can) => this.enterBtn.classList.toggle('hidden', !can));
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
      hint.textContent = step.state === 'failed'
        ? `— ${step.detail || tr('boot.back')}`
        : bootLabels().find(([id]) => id === step.id)?.[2] ?? '';
    }
    const done = steps.filter((step) => step.state === 'done').length;
    const percent = steps.length ? Math.round(done / steps.length * 100) : 0;
    this.progressFill.style.width = `${percent}%`;
    this.progressText.textContent = `${percent}%`;
    this.progressEl.setAttribute('aria-valuenow', String(percent));
    this.backBtn.classList.toggle('hidden', !steps.some((step) => step.state === 'failed'));
  }

  private renderLog(lines: string[]): void {
    const visible = this.logExpanded ? lines : lines.slice(-6);
    this.logEl.replaceChildren(...(visible.length ? visible : ['…']).map((line) =>
      h('div', { class: 'boot-log-line' },
        h('span', { class: 'boot-log-prompt', 'aria-hidden': 'true' }, '>'),
        h('span', null, line),
      ),
    ));
    // Wait for layout so the first render and expansion scroll correctly too.
    if (this.logFrame != null) cancelAnimationFrame(this.logFrame);
    this.logFrame = requestAnimationFrame(() => {
      this.logEl.scrollTop = this.logEl.scrollHeight;
      this.logFrame = null;
    });
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
    if (this.logFrame != null) cancelAnimationFrame(this.logFrame);
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
  // Address before tunnels: the address is what the tunnels get built for, so
  // this is the order the work happens in. The rows are built from this list,
  // and the back-fill in the store counts on the same order.
  ['session', tr('boot.step.session'), tr('boot.step.session_hint')],
  ['tunnels', tr('boot.step.tunnels'), tr('boot.step.tunnels_hint')],
  ['core', tr('boot.step.core'), tr('boot.step.core_hint')],
  ['relay', tr('boot.step.relay'), tr('boot.step.relay_hint')],
  ['dht', tr('boot.step.dht'), tr('boot.step.dht_hint')],
];

function fmtSecs(ms: number): string {
  const s = Math.max(0, ms) / 1000;
  return tr('view.s', { p0: s < 10 ? s.toFixed(1) : Math.round(s) });
}
