import { Api, type UpdateInfo } from './api';
import { emptyChatArt, icon } from './icons';
import type { Store, UpdateProgress, ChatTarget } from './state';
import { View, h, busy, humanSize } from './view';
import { ProfileSelect } from './profile';
import { AuthCreate, AuthUnlock, AuthBooting } from './auth';
import { Sidebar } from './sidebar';
import { ChatView } from './chat';

export class App extends View {
  el: HTMLElement;
  private current: View | null = null;
  private updateModalEl: HTMLElement | null = null;
  private updateProgressEl: HTMLElement | null = null;
  private updateStatusEl: HTMLElement | null = null;
  private updateActionsEl: HTMLElement | null = null;

  constructor(private store: Store) {
    super();
    this.el = h('div', { class: 'stack', style: { height: '100%' } });
    this.sub(store.view, (v) => this.render(v));
    this.sub(store.toast, (t) => this.renderToast(t));
    this.sub(store.updateAvailable, (info) => this.onUpdateAvailable(info));
    this.sub(store.updateProgress, (p) => this.onUpdateProgress(p));
    this.sub(store.updateReadyPath, (path) => this.onUpdateReady(path));
    this.sub(store.updateError, (err) => this.onUpdateError(err));
    this.sub(store.updateStaged, (version) => this.onUpdateStaged(version));
  }

  private render(view: string): void {
    this.current?.destroy();
    let next: View;
    switch (view) {
      case 'profile-select': next = new ProfileSelect(this.store, this); break;
      case 'auth-create': next = new AuthCreate(this.store); break;
      case 'auth-unlock': next = new AuthUnlock(this.store); break;
      case 'auth-booting': next = new AuthBooting(this.store); break;
      case 'main': next = new MainView(this.store, this); break;
      default: next = new ProfileSelect(this.store, this);
    }
    this.current = next;
    this.el.appendChild(next.el);
  }

  private renderToast(t: { text: string; err: boolean } | null): void {
    if (!t) return;
    const el = h('div', { class: t.err ? 'toast err' : 'toast' }, t.text);
    const existing = document.querySelectorAll('.toast');
    const offset = existing.length * 56;
    (el as HTMLElement).style.bottom = `${20 + offset}px`;
    document.body.appendChild(el);
    setTimeout(() => el.remove(), 3500);
  }

  openModal(build: (close: () => void) => HTMLElement): void {
    const backdrop = h('div', { class: 'modal-backdrop' });
    const close = () => backdrop.remove();
    backdrop.addEventListener('click', (e) => { if (e.target === backdrop) close(); });
    backdrop.appendChild(build(close));
    document.body.appendChild(backdrop);
  }

  /** Ask for one line of text; resolves null when cancelled. */
  prompt(title: string, label: string, initial = '', action = 'Сохранить'): Promise<string | null> {
    return new Promise((resolve) => {
      this.openModal((close) => {
        const input = h('input', { class: 'input', value: initial, maxlength: '80' });
        const done = (v: string | null) => { close(); resolve(v); };
        const submit = () => { const v = input.value.trim(); if (v) done(v); };
        input.addEventListener('keydown', (e) => {
          if (e.key === 'Enter') submit();
          if (e.key === 'Escape') done(null);
        });
        setTimeout(() => { input.focus(); input.select(); }, 0);
        return h('div', { class: 'modal modal-sm' },
          h('div', { class: 'modal-header' },
            h('div', { class: 'modal-title' }, title),
            h('button', { class: 'icon-btn', title: 'Закрыть', onClick: () => done(null) }, icon('close')),
          ),
          h('div', { class: 'modal-body' },
            h('div', { class: 'field' }, h('label', null, label), input),
          ),
          h('div', { class: 'modal-footer' },
            h('button', { class: 'btn btn-ghost', onClick: () => done(null) }, 'Отмена'),
            h('button', { class: 'btn', onClick: submit }, action),
          ),
        );
      });
    });
  }

  isModalActive(): boolean {
    return document.querySelector('.modal-backdrop') !== null;
  }

  confirm(title: string, body: string, danger = false): Promise<boolean> {
    return new Promise((resolve) => {
      this.openModal((close) => h('div', { class: 'modal' },
        h('div', { class: 'modal-header' },
          h('div', { class: 'modal-title' }, title),
          h('button', { class: 'icon-btn', title: 'Закрыть', onClick: () => { close(); resolve(false); } }, icon('close')),
        ),
        h('div', { class: 'modal-body' }, h('div', { class: 'fp' }, body)),
        h('div', { class: 'modal-footer' },
          h('button', { class: 'btn btn-ghost', onClick: () => { close(); resolve(false); } }, 'Отмена'),
          h('button', {
            class: danger ? 'btn btn-danger' : 'btn',
            onClick: () => { close(); resolve(true); },
          }, 'Подтвердить'),
        ),
      ));
    });
  }

  private onUpdateAvailable(info: UpdateInfo | null): void {
    if (!info) { this.closeUpdateModal(); return; }
    if (this.updateModalEl) return;
    this.openUpdateModal(info);
  }

  private onUpdateProgress(p: UpdateProgress | null): void {
    if (!p || !this.updateProgressEl || !this.updateStatusEl || !this.updateActionsEl) return;
    const bar = this.updateProgressEl.querySelector('.update-bar') as HTMLElement | null;
    if (bar) bar.style.width = `${p.pct}%`;
    this.updateStatusEl.textContent = p.total > 0
      ? `downloading ${humanSize(p.downloaded)} / ${humanSize(p.total)} (${p.pct}%)`
      : `downloading ${humanSize(p.downloaded)}`;
    this.updateActionsEl.replaceChildren(
      h('div', { class: 'hint' }, 'do not close the window — install will start automatically'),
    );
  }

  private onUpdateReady(path: string | null): void {
    if (!path || !this.updateStatusEl || !this.updateActionsEl) return;
    this.updateStatusEl.textContent = `Скачано: ${path}`;
    this.updateActionsEl.replaceChildren(
      h('div', { class: 'hint' }, 'Установите файл сами, затем перезапустите gipny'),
      h('button', {
        class: 'btn btn-ghost',
        onClick: () => { this.closeUpdateModal(); this.store.updateReadyPath.set(null); },
      }, 'Закрыть'),
    );
  }

  /// An update is installed and takes effect on the next start. The person is
  /// asked once, and can keep working — the running version still functions.
  private onUpdateStaged(version: string | null): void {
    if (!version) return;
    this.closeUpdateModal();
    this.store.updateProgress.set(null);
    this.store.updateAvailable.set(null);
    this.openModal((close) => h('div', { class: 'modal modal-sm' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, 'Обновление готово'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'about-p' },
          `Версия ${version} установлена и заработает после перезапуска. `
          + 'Можно перезапустить сейчас или продолжить — тогда она включится при следующем запуске.'),
      ),
      h('div', { class: 'modal-footer' },
        h('button', {
          class: 'btn btn-ghost',
          onClick: () => { this.store.updateStaged.set(null); close(); },
        }, 'Позже'),
        h('button', {
          class: 'btn',
          onClick: () => { this.store.updateStaged.set(null); Api.restartApp().catch(() => close()); },
        }, 'Перезапустить'),
      ),
    ));
  }

  private onUpdateError(errMsg: string | null): void {
    if (!errMsg || !this.updateStatusEl || !this.updateActionsEl) return;
    this.updateStatusEl.textContent = `error: ${errMsg}`;
    this.updateActionsEl.replaceChildren(
      h('button', {
        class: 'btn btn-ghost',
        onClick: () => { this.closeUpdateModal(); this.store.updateError.set(null); },
      }, 'Закрыть'),
    );
  }

  private openUpdateModal(info: UpdateInfo): void {
    const backdrop = h('div', { class: 'modal-backdrop' });
    this.updateStatusEl = h('div', {
      class: 'fp',
      style: { marginTop: '8px', wordBreak: 'break-all' },
    }, `version ${info.version} · ${humanSize(info.size)}`);

    this.updateProgressEl = h('div', {
      class: 'update-progress',
      style: {
        height: '6px', background: 'rgba(51,255,102,0.15)',
        marginTop: '14px', overflow: 'hidden', display: 'none',
        border: '1px solid #33ff66',
      },
    },
      h('div', {
        class: 'update-bar',
        style: {
          height: '100%', width: '0%', background: '#33ff66',
          transition: 'width .2s ease', boxShadow: '0 0 8px #33ff66',
        },
      }),
    );

    this.updateActionsEl = h('div', { class: 'row', style: { gap: '8px', justifyContent: 'flex-end' } },
      h('button', {
        class: 'btn btn-ghost',
        onClick: async () => {
          await this.store.dismissUpdate();
          this.closeUpdateModal();
        },
      }, 'Позже'),
      (() => {
        const b = h('button', {
          class: 'btn',
          onClick: () => busy(b, async () => {
            (this.updateProgressEl as HTMLElement).style.display = 'block';
            await this.store.installUpdate();
          }),
        }, 'Обновить сейчас') as HTMLButtonElement;
        // Android and unmanaged packages cannot install themselves; offering
        // the button would download into a directory nobody can reach.
        Api.updateInstallsItself().then((can: boolean) => {
          if (can) return;
          b.remove();
          if (this.updateStatusEl) {
            this.updateStatusEl.textContent +=
              ' · установка вручную: Настройки → Android-приложение (или страница релизов)';
          }
        }).catch(() => {});
        return b;
      })(),
    );

    const modal = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, 'Доступна новая версия'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'card-label' }, 'release notes'),
        h('div', {
          class: 'card-block',
          style: { whiteSpace: 'pre-wrap', maxHeight: '200px', overflowY: 'auto' },
        }, info.notes || '(no notes)'),
        this.updateStatusEl,
        this.updateProgressEl,
      ),
      h('div', { class: 'modal-footer' }, this.updateActionsEl),
    );

    backdrop.appendChild(modal);
    document.body.appendChild(backdrop);
    this.updateModalEl = backdrop;
  }

  private closeUpdateModal(): void {
    this.updateModalEl?.remove();
    this.updateModalEl = null;
    this.updateProgressEl = null;
    this.updateStatusEl = null;
    this.updateActionsEl = null;
  }
}

class MainView extends View {
  el: HTMLElement;
  constructor(store: Store, app: App) {
    super();
    const sidebar = new Sidebar(store, app);
    const chatSlot = h('div', { class: 'stack grow', style: { minHeight: '0' } });
    const empty = h('div', { class: 'empty empty-chat' },
      emptyChatArt(),
      h('div', { class: 'empty-title' }, 'Выберите чат'),
      h('div', { class: 'empty-sub' }, 'Сообщения шифруются на вашем устройстве и идут через сеть i2p.'),
    );
    chatSlot.appendChild(empty);
    const main = h('div', { class: 'main' }, sidebar.el, chatSlot);

    // Relay status. Nothing in the UI used to show it at all, so a client that
    // could not send looked identical to one that could — and with no relay
    // configured it cannot send to anyone. The banner says which of the two it
    // is and where to fix it.
    const banner = h('div', { class: 'relay-banner' });
    const paintBanner = (): void => {
      const unconfigured = store.relayUnconfigured.get();
      const connected = store.relayConnected.get();
      banner.classList.toggle('hidden', connected);
      banner.classList.toggle('warn', unconfigured);
      if (connected) return;
      const info = store.relayInfo.get();
      const builtin = info?.mode === 'builtin';
      banner.classList.toggle('warn', unconfigured || (builtin && info?.hosted.state === 'failed'));
      banner.replaceChildren(
        unconfigured
          ? 'внешний релей выбран, но адрес не задан — отправка недоступна. Настройки → relay'
          : builtin && info?.hosted.state === 'failed'
            ? 'встроенный релей не поднялся, пробую снова — сообщения пока ждут в очереди'
            : builtin && info?.hosted.state !== 'ready'
              ? 'встроенный релей запускается — обычно 1–2 минуты. Сообщения уйдут, как только он будет готов'
              : 'нет связи с релеем — сообщения уйдут, когда связь восстановится'
      );
    };
    this.subs.push(store.relayConnected.subscribe(paintBanner, true));
    this.subs.push(store.relayUnconfigured.subscribe(paintBanner, true));
    this.subs.push(store.relayInfo.subscribe(paintBanner, false));
    main.insertBefore(banner, main.firstChild);

    // Agent mode is intentionally visible in the same always-on strip as relay
    // health. A remote shell should never be easy to forget or accidentally
    // leave enabled after the user returns to an ordinary chat.
    const agentBanner = h('div', { class: 'agent-banner hidden' });
    const paintAgentBanner = (): void => {
      const master = store.agentMode.get();
      agentBanner.classList.toggle('hidden', master == null);
      if (!master) return;
      const off = h('button', {
        class: 'btn btn-danger btn-sm',
        onClick: () => busy(off as HTMLButtonElement, async () => {
          try {
            await store.setAgentMode(null);
          } catch (e) {
            store.showToast('agent mode: ' + String(e), true);
          }
        }),
      }, 'disable') as HTMLButtonElement;
      agentBanner.replaceChildren(
        h('span', null, `AGENT MODE · master: ${master.name || 'unnamed'}`),
        off,
      );
    };
    main.insertBefore(agentBanner, main.firstChild);
    this.subs.push(store.agentMode.subscribe(paintAgentBanner, true));
    this.subs.push(store.sidebarCollapsed.subscribe((c: boolean) => {
      main.classList.toggle('sidebar-collapsed', c);
    }));
    this.subs.push(store.selectedChat.subscribe((target: ChatTarget | null) => {
      chatSlot.replaceChildren();
      if (!target) {
        main.classList.remove('has-chat');
        chatSlot.appendChild(empty);
      } else {
        main.classList.add('has-chat');
        chatSlot.appendChild(new ChatView(store, app, target).el);
      }
    }));
    this.subs.push(() => sidebar.destroy());
    this.el = main;
  }
}
