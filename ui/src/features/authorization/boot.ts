import { errorText } from '../../app/error-text';
import { t as tr } from '../localization/i18n';
import { isBackgroundStep, type Store, type BootStep, type BootStepId } from '../../app/state';
import { View, h } from '../../app/view';
import { authHeader } from '../profiles/profile-salon';

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
      class: 'btn boot-button boot-enter hidden', type: 'button',
      onClick: () => store.enterMain(),
    }, tr('auth.enter'));
    this.backBtn = h('button', {
      class: 'btn btn-ghost boot-button boot-back hidden', type: 'button',
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
        authHeader(),
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
    // Whether a failed relay row says "retrying" or "no network service here"
    // is known only once the core has been asked about the relay.
    this.sub(store.relayInfo, () => this.paint(store.bootSteps.get()), false);
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
    const background = (step: BootStep): boolean => isBackgroundStep(step.id);
    const failed = steps.find((step) => step.state === 'failed' && !background(step));
    // "We will keep trying" is only true while there is a network service to
    // try. The browser client on a hosting without one gets the truth instead.
    const unconfigured = this.store.relayInfo.get()?.hosted.state === 'unconfigured';
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
        : background(step) ? tr(unconfigured ? 'boot.network_unconfigured' : 'boot.retrying_in_background')
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
