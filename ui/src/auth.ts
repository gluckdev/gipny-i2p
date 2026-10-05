import { Api } from './api';
import type { Store, BootStep, BootStepId } from './state';
import { View, h, busy, logo } from './view';
import { t } from './i18n';

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
      class: 'input', placeholder: 'e.g., alice', autofocus: true, maxlength: '32',
    });
    this.displayI = h('input', {
      class: 'input', placeholder: 'visible to contacts', maxlength: '64',
    });
    this.passI = h('input', { class: 'input', type: 'password', placeholder: 'passphrase' });
    this.confirmI = h('input', { class: 'input', type: 'password', placeholder: 'confirm passphrase' });
    this.duressI = h('input', { class: 'input', type: 'password', placeholder: 'duress passphrase (optional)' });
    this.wipeC = h('input', { type: 'checkbox', checked: true });
    this.attemptsI = h('input', { class: 'input', type: 'number', value: '10', min: '0', max: '100' });
    this.err = h('div', { class: 'err' });

    const hasProfiles = store.profiles.get().length > 0;

    this.el = h('div', { class: 'auth' },
      h('div', { class: 'auth-card' },
        logo(),
        h('div', { class: 'auth-title' }, t('auth.new_profile_title')),
        h('div', { class: 'auth-sub' }, t('auth.new_profile_sub')),
        h('div', { class: 'field' },
          h('label', null, t('auth.profile_name')), this.nameI,
          h('div', { class: 'hint' }, t('auth.profile_name_hint')),
        ),
        h('div', { class: 'field' },
          h('label', null, t('auth.display_name')), this.displayI,
          h('div', { class: 'hint' }, t('auth.display_name_hint')),
        ),
        h('div', { class: 'field' }, h('label', null, t('auth.passphrase')), this.passI),
        h('div', { class: 'field' }, h('label', null, t('auth.confirm_pass')), this.confirmI),
        h('div', { class: 'divider-text' }, t('auth.duress_title')),
        h('div', { class: 'field' }, h('label', null, t('auth.duress_pass')), this.duressI,
          h('div', { class: 'hint' }, t('auth.duress_pass_hint'))),
        h('label', { class: 'chk', style: { marginBottom: '14px' } },
          this.wipeC, h('span', { class: 'box' }),
          h('span', null, t('auth.duress_wipe'))),
        h('div', { class: 'field' }, h('label', null, t('auth.max_attempts')), this.attemptsI),
        this.err,
        h('div', { class: 'row', style: { marginTop: '18px', gap: '8px' } },
          hasProfiles && h('button', {
            class: 'btn btn-ghost',
            onClick: () => store.cancelToProfileSelect(),
          }, t('common.back')),
          (() => {
            const b = h('button', {
              class: 'btn',
              style: { flex: '1' },
              onClick: () => busy(b, () => this.create()),
            }, t('auth.create_btn')) as HTMLButtonElement;
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

    if (!profile) { this.err.textContent = 'profile name required'; return; }
    if (!/^[A-Za-z0-9_-]{1,32}$/.test(profile)) {
      this.err.textContent = 'profile: alphanumeric + - _ (max 32)';
      return;
    }
    if (!display) { this.err.textContent = 'display name required (your contacts will see this)'; return; }
    if (display.length > 64) { this.err.textContent = 'display name too long (max 64)'; return; }
    if (pass.length < 8) { this.err.textContent = 'passphrase too short (min 8)'; return; }
    if (pass !== conf) { this.err.textContent = 'passphrases do not match'; return; }
    if (duress && duress === pass) { this.err.textContent = 'duress must differ from primary'; return; }

    this.err.textContent = '';
    await this.store.beginBoot(profile);
    try {
      await Api.vaultCreate(profile, pass, display, duress || null, wipe, max);
      await this.store.onUnlocked(profile);
    } catch (e) {
      this.store.endBoot();
      this.store.view.set('auth-create');
      this.err.textContent = `Не удалось создать профиль: ${String(e)}`;
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
    const profile = store.currentProfile.get() ?? 'unknown';
    this.passI = h('input', { class: 'input', type: 'password', placeholder: t('auth.passphrase'), autofocus: true });
    this.err = h('div', { class: 'err' });
    this.net = h('div', { class: 'auth-net' });
    this.el = h('div', { class: 'auth' },
      h('div', { class: 'auth-card' },
        logo(),
        h('div', { class: 'auth-title' }, profile),
        h('div', { class: 'auth-sub' }, t('auth.unlock_sub')),
        h('div', { class: 'field' }, h('label', null, t('auth.passphrase')), this.passI),
        this.err,
        this.net,
        h('div', { class: 'row', style: { marginTop: '18px', gap: '8px' } },
          h('button', {
            class: 'btn btn-ghost',
            onClick: () => store.cancelToProfileSelect(),
          }, t('common.back')),
          (() => {
            const b = h('button', {
              class: 'btn', style: { flex: '1' },
              onClick: () => busy(b, () => this.unlock()),
            }, t('auth.open_btn')) as HTMLButtonElement;
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
        ? t('auth.net_building')
        : s === 'ready' ? t('auth.net_ready') : '';
      this.net.className = 'auth-net' + (s === 'ready' ? ' ok' : '');
    });
  }

  private async unlock(): Promise<void> {
    this.err.textContent = '';
    const profile = this.store.currentProfile.get();
    if (!profile) { this.err.textContent = t('auth.profile_not_selected'); return; }
    const pass = this.passI.value;
    if (!pass) { this.err.textContent = t('auth.pass_required'); return; }
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
      if (msg.includes('wiped')) this.store.showToast(t('auth.profile_wiped'), true);
      else if (msg.includes('invalid passphrase')) this.store.showToast(t('auth.wrong_pass'), true);
      else this.store.showToast(msg, true);
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
    for (const [id, label, hint] of BOOT_LABELS) {
      const row = this.makeRow(label, hint);
      this.rows.set(id, row);
      list.appendChild(row);
    }

    this.elapsedEl = h('span', { class: 'boot-elapsed' }, 'UPTIME: 0.0s');
    this.progressFill = h('div', { class: 'boot-progress-fill' });
    this.progressEl = h('div', {
      class: 'boot-progress-bar', role: 'progressbar',
      'aria-label': t('boot.title'), 'aria-valuemin': '0', 'aria-valuemax': '100',
      'aria-valuenow': '0',
    }, this.progressFill);
    this.progressText = h('span', { class: 'boot-progress-text' }, '0%');
    this.logEl = h('div', {
      id: 'boot-live-output', class: 'boot-log', role: 'log',
      'aria-label': 'LIVE SYSTEM LOG', 'aria-live': 'off', tabindex: '0',
    });
    this.logToggle = h('button', {
      class: 'boot-log-toggle', type: 'button',
      'aria-expanded': 'false', 'aria-controls': 'boot-live-output',
      'aria-label': t('boot.details'), title: t('boot.details'),
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
        h('span', null, h('span', { class: 'boot-live-dot', 'aria-hidden': 'true' }), 'LIVE SYSTEM LOG'),
        this.logToggle,
      ),
      this.logEl,
    );
    this.enterBtn = h('button', {
      class: 'boot-button boot-enter hidden', type: 'button',
      onClick: () => store.enterMain(),
    }, '[ ВХОД В СИСТЕМУ ↵ ]');
    this.backBtn = h('button', {
      class: 'boot-button boot-back hidden', type: 'button',
      onClick: () => {
        store.endBoot();
        void store.cancelToProfileSelect();
      },
    }, '[ ✕ НАЗАД ]');

    this.el = h('div', { class: 'auth boot-cyber' },
      h('div', { class: 'auth-card boot-card' },
        h('header', { class: 'boot-header' },
          h('h1', { class: 'boot-title' }, 'GIPNY // SYSTEM INITIALIZATION'),
          h('div', { class: 'boot-meta' },
            h('span', { class: 'boot-node' }, `NODE: ${store.currentProfile.get() ?? 'unknown'}`),
            this.elapsedEl,
          ),
        ),
        h('div', { class: 'boot-progress' }, this.progressEl, this.progressText),
        h('p', { class: 'boot-sub' }, t('boot.sub')),
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
      done: '[  OK  ]', active: '[ RUN  ]', idle: '[  --  ]',
      failed: '[ FAIL ]', skipped: '[ SKIP ]',
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
        ? `— ${step.detail || t('boot.back')}`
        : BOOT_LABELS.find(([id]) => id === step.id)?.[2] ?? '';
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
    this.elapsedEl.textContent = `UPTIME: ${fmtSecs(Date.now() - this.startedAt)}`;
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
  return `${Math.max(0, Math.round(ms))} ms`;
}

/** What each backend stage is called on screen, and what it is doing. */
const BOOT_LABELS: [BootStepId, string, string][] = [
  ['vault', t('boot.step.vault'), t('boot.step.vault_hint')],
  ['router', t('boot.step.router'), t('boot.step.router_hint')],
  // Address before tunnels: the address is what the tunnels get built for, so
  // this is the order the work happens in. The rows are built from this list,
  // and the back-fill in the store counts on the same order.
  ['session', t('boot.step.session'), t('boot.step.session_hint')],
  ['tunnels', t('boot.step.tunnels'), t('boot.step.tunnels_hint')],
  ['core', t('boot.step.core'), t('boot.step.core_hint')],
  ['relay', t('boot.step.relay'), t('boot.step.relay_hint')],
  ['dht', t('boot.step.dht'), t('boot.step.dht_hint')],
];

function fmtSecs(ms: number): string {
  const s = Math.max(0, ms) / 1000;
  return s < 10 ? `${s.toFixed(1)}s` : `${Math.round(s)}s`;
}
