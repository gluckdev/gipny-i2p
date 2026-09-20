import { Api } from './api';
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
        h('div', { class: 'auth-title' }, 'New profile'),
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
          }, 'Назад'),
          (() => {
            const b = h('button', {
              class: 'btn',
              style: { flex: '1' },
              onClick: () => busy(b, () => this.create()),
            }, 'Создать профиль') as HTMLButtonElement;
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
    this.passI = h('input', { class: 'input', type: 'password', placeholder: 'пароль', autofocus: true });
    this.err = h('div', { class: 'err' });
    this.net = h('div', { class: 'auth-net' });
    this.el = h('div', { class: 'auth' },
      h('div', { class: 'auth-card' },
        logo(),
        h('div', { class: 'auth-title' }, profile),
        h('div', { class: 'auth-sub' }, 'Введите пароль профиля'),
        h('div', { class: 'field' }, h('label', null, 'Пароль'), this.passI),
        this.err,
        this.net,
        h('div', { class: 'row', style: { marginTop: '18px', gap: '8px' } },
          h('button', {
            class: 'btn btn-ghost',
            onClick: () => store.cancelToProfileSelect(),
          }, 'Назад'),
          (() => {
            const b = h('button', {
              class: 'btn', style: { flex: '1' },
              onClick: () => busy(b, () => this.unlock()),
            }, 'Открыть') as HTMLButtonElement;
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
        ? 'Сеть i2p: строятся туннели, не дожидайтесь — вводите пароль'
        : s === 'ready' ? 'Сеть i2p: туннели построены' : '';
      this.net.className = 'auth-net' + (s === 'ready' ? ' ok' : '');
    });
  }

  private async unlock(): Promise<void> {
    this.err.textContent = '';
    const profile = this.store.currentProfile.get();
    if (!profile) { this.err.textContent = 'Профиль не выбран'; return; }
    const pass = this.passI.value;
    if (!pass) { this.err.textContent = 'Введите пароль'; return; }
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
      if (msg.includes('wiped')) this.store.showToast('Профиль стёрт', true);
      else if (msg.includes('invalid passphrase')) this.store.showToast('Неверный пароль', true);
      else this.store.showToast(msg, true);
    }
  }
}

export class AuthBooting extends View {
  el: HTMLElement;
  private rows = new Map<BootStepId, HTMLElement>();
  private elapsedEl: HTMLElement;
  private logEl: HTMLElement;
  private enterBtn: HTMLButtonElement;
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

    this.elapsedEl = h('div', { class: 'boot-elapsed' }, 'прошло 0.0 с');
    this.logEl = h('pre', { class: 'boot-log' }, '');
    this.enterBtn = h('button', {
      class: 'btn btn-ghost boot-enter hidden',
      onClick: () => store.enterMain(),
    }, 'Открыть чаты сейчас') as HTMLButtonElement;

    this.el = h('div', { class: 'auth' },
      h('div', { class: 'auth-card boot-card' },
        logo(),
        h('div', { class: 'auth-title' }, 'Открываю профиль'),
        h('div', { class: 'auth-sub' },
          'Первый запуск занимает минуты: роутер ищет узлы i2p и строит туннели. Дальше быстрее.'),
        list,
        h('div', { class: 'row-between boot-foot' }, this.elapsedEl, this.enterBtn),
        h('details', { class: 'boot-details' },
          h('summary', null, 'Технические подробности'),
          this.logEl,
        ),
      ),
    );

    this.sub(store.bootSteps, (steps) => this.paint(steps));
    this.sub(store.bootLog, (lines) => {
      this.logEl.textContent = lines.join('\n');
      this.logEl.scrollTop = this.logEl.scrollHeight;
    });
    this.sub(store.bootCanEnter, (can) => this.enterBtn.classList.toggle('hidden', !can));
    this.timer = window.setInterval(() => this.tick(), 500);
  }

  private makeRow(label: string, hint: string): HTMLElement {
    return h('div', { class: 'boot-step' },
      h('span', { class: 'boot-mark' }, '○'),
      h('div', { class: 'boot-step-main' },
        h('div', { class: 'boot-step-label' }, label),
        h('div', { class: 'boot-step-hint' }, hint),
      ),
      h('div', { class: 'boot-step-ms' }, ''),
    );
  }

  private paint(steps: BootStep[]): void {
    for (const step of steps) {
      const row = this.rows.get(step.id);
      if (!row) continue;
      const mark = row.querySelector('.boot-mark') as HTMLElement;
      const ms = row.querySelector('.boot-step-ms') as HTMLElement;
      const hint = row.querySelector('.boot-step-hint') as HTMLElement;
      row.className = `boot-step boot-${step.state}`;
      mark.textContent = step.state === 'done' ? '✓' : step.state === 'failed' ? '✕' : step.state === 'active' ? '◐' : '○';
      ms.textContent = step.state === 'active' && step.startedAt
        ? fmtSecs(Date.now() - step.startedAt)
        : step.ms > 0 ? fmtSecs(step.ms) : '';
      if (step.state === 'failed' && step.detail) hint.textContent = step.detail;
    }
  }

  private tick(): void {
    this.elapsedEl.textContent = `прошло ${fmtSecs(Date.now() - this.startedAt)}`;
    // The active step's own clock keeps moving between backend messages, so a
    // long tunnel build never looks stuck.
    const active = this.store.bootSteps.get().find((s) => s.state === 'active');
    if (!active?.startedAt) return;
    const ms = this.rows.get(active.id)?.querySelector('.boot-step-ms') as HTMLElement | null;
    if (ms) ms.textContent = fmtSecs(Date.now() - active.startedAt);
  }

  destroy(): void {
    if (this.timer != null) clearInterval(this.timer);
    super.destroy();
  }
}

/** What each backend stage is called on screen, and what it is doing. */
const BOOT_LABELS: [BootStepId, string, string][] = [
  ['vault', 'Расшифровываю профиль', 'argon2id, это нагружает процессор'],
  ['router', 'Запускаю роутер i2p', 'он живёт рядом с приложением'],
  ['tunnels', 'Строю туннели', 'самая долгая часть первого запуска'],
  ['session', 'Получаю адрес в сети', 'новый на каждый запуск'],
  ['core', 'Готовлю переписку', 'ключи, база, очереди'],
  ['relay', 'Поднимаю свой релей', 'через него вам пишут'],
  ['dht', 'Вхожу в сеть релеев', 'нужна для доставки в офлайне'],
];

function fmtSecs(ms: number): string {
  const s = Math.max(0, ms) / 1000;
  return s < 10 ? `${s.toFixed(1)} с` : `${Math.round(s)} с`;
}
