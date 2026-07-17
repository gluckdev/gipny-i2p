import { Api } from './api';
import type { Store, BootStage } from './state';
import { View, h, busy, LOGO } from './view';

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
        h('pre', { class: 'auth-logo' }, LOGO),
        h('div', { class: 'auth-title' }, ':: NEW PROFILE ::'),
        h('div', { class: 'auth-sub' }, 'у каждого профиля свой i2p-адрес, ключи, контакты'),
        h('div', { class: 'field' },
          h('label', null, 'profile name'), this.nameI,
          h('div', { class: 'hint' }, 'локально на этом устройстве (alphanumeric + dash/underscore)'),
        ),
        h('div', { class: 'field' },
          h('label', null, 'display name'), this.displayI,
          h('div', { class: 'hint' }, 'имя которое увидят твои контакты — приходит в каждом сообщении'),
        ),
        h('div', { class: 'field' }, h('label', null, 'passphrase'), this.passI),
        h('div', { class: 'field' }, h('label', null, 'confirm'), this.confirmI),
        h('div', { class: 'divider-text' }, 'duress protection'),
        h('div', { class: 'field' }, h('label', null, 'duress passphrase'), this.duressI,
          h('div', { class: 'hint' }, 'alternate pass that triggers fail-safe')),
        h('label', { class: 'chk', style: { marginBottom: '14px' } },
          this.wipeC, h('span', { class: 'box' }),
          h('span', null, 'on duress: WIPE everything')),
        h('div', { class: 'field' }, h('label', null, 'max attempts (0 = unlimited)'), this.attemptsI),
        this.err,
        h('div', { class: 'row', style: { marginTop: '18px', gap: '8px' } },
          hasProfiles && h('button', {
            class: 'btn btn-ghost',
            onClick: () => store.cancelToProfileSelect(),
          }, '[ back ]'),
          (() => {
            const b = h('button', {
              class: 'btn',
              style: { flex: '1' },
              onClick: () => busy(b, () => this.create()),
            }, '[ INITIALIZE ]') as HTMLButtonElement;
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

    this.err.textContent = 'генерация ключей, запуск i2p-роутера... (1-3 мин при первом запуске)';
    try {
      await Api.vaultCreate(profile, pass, display, duress || null, wipe, max);
      await this.store.onUnlocked(profile);
    } catch (e) {
      this.err.textContent = `err: ${String(e)}`;
    }
  }
}

export class AuthUnlock extends View {
  el: HTMLElement;
  private passI: HTMLInputElement;
  private err: HTMLElement;

  constructor(private store: Store) {
    super();
    const profile = store.currentProfile.get() ?? 'unknown';
    this.passI = h('input', { class: 'input', type: 'password', placeholder: 'enter passphrase', autofocus: true });
    this.err = h('div', { class: 'err' });
    this.el = h('div', { class: 'auth' },
      h('div', { class: 'auth-card' },
        h('pre', { class: 'auth-logo' }, LOGO),
        h('div', { class: 'auth-title' }, `:: UNLOCK :: ${profile} ::`),
        h('div', { class: 'auth-sub' }, h('span', { class: 'blink' }, '>'), ' awaiting key'),
        h('div', { class: 'field' }, h('label', null, 'passphrase'), this.passI),
        this.err,
        h('div', { class: 'row', style: { marginTop: '18px', gap: '8px' } },
          h('button', {
            class: 'btn btn-ghost',
            onClick: () => store.cancelToProfileSelect(),
          }, '[ back ]'),
          (() => {
            const b = h('button', {
              class: 'btn', style: { flex: '1' },
              onClick: () => busy(b, () => this.unlock()),
            }, '[ UNLOCK ]') as HTMLButtonElement;
            this.passI.addEventListener('keydown', (e) => {
              if ((e as KeyboardEvent).key === 'Enter') busy(b, () => this.unlock());
            });
            return b;
          })(),
        ),
      ),
    );
  }

  private async unlock(): Promise<void> {
    this.err.textContent = '';
    const profile = this.store.currentProfile.get();
    if (!profile) { this.err.textContent = 'no profile selected'; return; }
    const pass = this.passI.value;
    if (!pass) { this.err.textContent = 'passphrase required'; return; }
    this.err.textContent = '';
    try {
      const warning = await Api.vaultUnlock(profile, pass);
      await this.store.onUnlocked(profile);
      if (warning) this.store.showToast(warning, true);
    } catch (e) {
      const msg = String(e);
      if (msg.includes('wiped')) this.err.textContent = 'vault wiped';
      else if (msg.includes('invalid passphrase')) this.err.textContent = 'invalid passphrase';
      else this.err.textContent = msg;
      this.passI.value = '';
    }
  }
}

export class AuthBooting extends View {
  el: HTMLElement;
  private stageTor: HTMLElement;
  private stageRelay: HTMLElement;
  private statusLine: HTMLElement;
  private log: HTMLElement;
  private startedAt = Date.now();
  private timerHandle: number | null = null;
  private logTimer: number | null = null;
  private logLines = [
    '> запуск i2p-роутера...',
    '> обновление netdb (reseed)...',
    '> строим входящие/исходящие туннели...',
    '> генерация destination...',
    '> публикация leaseSet...',
    '> i2p-адрес зарезервирован.',
    '> подключение к релею через i2p...',
    '> аутентификация ed25519...',
  ];
  private logIdx = 0;

  constructor(store: Store) {
    super();

    this.stageTor = this.makeStage('[ ] i2p router', 'building tunnels');
    this.stageRelay = this.makeStage('[ ] relay connect', 'authenticating');

    this.statusLine = h('div', {
      class: 'hint',
      style: { marginTop: '12px', textAlign: 'center' },
    }, '0s elapsed · anonymity > speed · hang tight');

    this.log = h('pre', {
      style: {
        marginTop: '18px',
        padding: '12px',
        background: 'rgba(51,255,102,0.04)',
        border: '1px solid rgba(51,255,102,0.25)',
        color: '#33ff66',
        fontSize: '11px',
        lineHeight: '1.6',
        maxHeight: '140px',
        overflow: 'hidden',
        whiteSpace: 'pre-wrap',
        wordBreak: 'break-all',
      },
    }, '');

    this.el = h('div', { class: 'auth' },
      h('div', { class: 'auth-card', style: { maxWidth: '560px' } },
        h('pre', { class: 'auth-logo' }, LOGO),
        h('div', { class: 'auth-title' }, ':: ESTABLISHING TOR CIRCUIT ::'),
        h('div', { class: 'auth-sub', style: { textAlign: 'center' } },
          'this may take ',
          h('span', { style: { color: '#ffb000' } }, '30 seconds to 10 minutes'),
          ' on first unlock',
        ),
        h('div', { class: 'stack', style: { gap: '10px', marginTop: '20px' } },
          this.stageTor,
          this.stageRelay,
        ),
        this.statusLine,
        this.log,
        h('div', { class: 'hint', style: { marginTop: '14px', textAlign: 'center', opacity: '0.7' } },
          'once connected, subsequent sessions are fast',
        ),
      ),
    );

    this.sub(store.bootStage, (s) => this.updateStages(s));

    this.timerHandle = window.setInterval(() => this.tickTimer(), 1000);
    this.logTimer = window.setInterval(() => this.appendLog(), 1400);
    this.appendLog();
  }

  private makeStage(label: string, sub: string): HTMLElement {
    return h('div', {
      class: 'card-block',
      style: {
        display: 'flex',
        alignItems: 'center',
        gap: '12px',
        padding: '10px 14px',
        border: '1px solid rgba(51,255,102,0.3)',
      },
    },
      h('span', { class: 'stage-spinner', style: { width: '14px', display: 'inline-block' } }, '·'),
      h('div', { style: { flex: '1' } },
        h('div', { class: 'stage-label', style: { fontSize: '13px' } }, label),
        h('div', { class: 'hint', style: { fontSize: '11px' } }, sub),
      ),
    );
  }

  private updateStages(stage: BootStage): void {
    const spinFrames = ['|', '/', '─', '\\'];
    const setStage = (el: HTMLElement, state: 'idle' | 'active' | 'done') => {
      const label = el.querySelector('.stage-label') as HTMLElement;
      const spinner = el.querySelector('.stage-spinner') as HTMLElement;
      const text = label.textContent ?? '';
      const rest = text.replace(/^\[.\] /, '');
      if (state === 'done') {
        label.textContent = `[✓] ${rest}`;
        label.style.color = '#33ff66';
        spinner.textContent = '✓';
        spinner.style.color = '#33ff66';
        el.style.borderColor = 'rgba(51,255,102,0.6)';
      } else if (state === 'active') {
        label.textContent = `[.] ${rest}`;
        label.style.color = '#ffb000';
        spinner.style.color = '#ffb000';
        el.style.borderColor = 'rgba(255,176,0,0.6)';
        el.dataset.spinning = '1';
        this.startSpinner(spinner, spinFrames);
      } else {
        label.textContent = `[ ] ${rest}`;
        label.style.color = '';
        spinner.textContent = '·';
        spinner.style.color = 'rgba(51,255,102,0.4)';
        delete el.dataset.spinning;
      }
    };

    switch (stage) {
      case 'unlocking':
        setStage(this.stageTor, 'idle');
        setStage(this.stageRelay, 'idle');
        break;
      case 'i2p':
        setStage(this.stageTor, 'active');
        setStage(this.stageRelay, 'idle');
        break;
      case 'relay':
        setStage(this.stageTor, 'done');
        setStage(this.stageRelay, 'active');
        break;
      case 'done':
        setStage(this.stageTor, 'done');
        setStage(this.stageRelay, 'done');
        break;
    }
  }

  private startSpinner(el: HTMLElement, frames: string[]): void {
    if (el.dataset.spinActive === '1') return;
    el.dataset.spinActive = '1';
    let i = 0;
    const tick = () => {
      if (!el.isConnected || el.dataset.spinActive !== '1') return;
      el.textContent = frames[i % frames.length] ?? '·';
      i++;
      setTimeout(tick, 120);
    };
    tick();
  }

  private tickTimer(): void {
    const secs = Math.floor((Date.now() - this.startedAt) / 1000);
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    const label = m > 0 ? `${m}m ${s}s` : `${s}s`;
    this.statusLine.textContent = `${label} elapsed · anonymity > speed · hang tight`;
  }

  private appendLog(): void {
    if (this.logIdx >= this.logLines.length) {
      this.log.textContent += `\n> still routing... hold on (${Math.floor((Date.now() - this.startedAt) / 1000)}s)`;
      this.log.scrollTop = this.log.scrollHeight;
      this.logIdx++;
      return;
    }
    const line = this.logLines[this.logIdx];
    this.log.textContent += (this.logIdx === 0 ? '' : '\n') + line;
    this.log.scrollTop = this.log.scrollHeight;
    this.logIdx++;
  }

  destroy(): void {
    if (this.timerHandle != null) clearInterval(this.timerHandle);
    if (this.logTimer != null) clearInterval(this.logTimer);
    super.destroy();
  }
}
