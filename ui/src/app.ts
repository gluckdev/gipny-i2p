import type { UpdateInfo } from './api';
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

  isModalActive(): boolean {
    return document.querySelector('.modal-backdrop') !== null;
  }

  confirm(title: string, body: string, danger = false): Promise<boolean> {
    return new Promise((resolve) => {
      this.openModal((close) => h('div', { class: 'modal' },
        h('div', { class: 'modal-header' },
          h('div', { class: 'modal-title' }, title),
          h('button', { class: 'icon-btn', onClick: () => { close(); resolve(false); } }, 'x'),
        ),
        h('div', { class: 'modal-body' }, h('div', { class: 'fp' }, body)),
        h('div', { class: 'modal-footer' },
          h('button', { class: 'btn btn-ghost', onClick: () => { close(); resolve(false); } }, '[ cancel ]'),
          h('button', {
            class: danger ? 'btn btn-danger' : 'btn',
            onClick: () => { close(); resolve(true); },
          }, '[ confirm ]'),
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
    this.updateStatusEl.textContent = `downloaded to ${path}`;
    this.updateActionsEl.replaceChildren(
      h('div', { class: 'hint' }, 'install manually, then restart gipny'),
      h('button', {
        class: 'btn btn-ghost',
        onClick: () => { this.closeUpdateModal(); this.store.updateReadyPath.set(null); },
      }, '[ close ]'),
    );
  }

  private onUpdateError(errMsg: string | null): void {
    if (!errMsg || !this.updateStatusEl || !this.updateActionsEl) return;
    this.updateStatusEl.textContent = `error: ${errMsg}`;
    this.updateActionsEl.replaceChildren(
      h('button', {
        class: 'btn btn-ghost',
        onClick: () => { this.closeUpdateModal(); this.store.updateError.set(null); },
      }, '[ close ]'),
    );
  }

  private openUpdateModal(info: UpdateInfo): void {
    const backdrop = h('div', { class: 'modal-backdrop' });
    this.updateStatusEl = h('div', {
      class: 'fp',
      style: { marginTop: '8px', wordBreak: 'break-all' },
    }, `version ${info.version} · ${humanSize(info.size)} · target: ${info.target_key}`);

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
      }, '[ LATER ]'),
      (() => {
        const b = h('button', {
          class: 'btn btn-amber',
          onClick: () => busy(b, async () => {
            (this.updateProgressEl as HTMLElement).style.display = 'block';
            await this.store.installUpdate();
          }),
        }, '[ UPDATE NOW ]') as HTMLButtonElement;
        return b;
      })(),
    );

    const modal = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, '── NEW VERSION AVAILABLE ──'),
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
    const empty = h('div', { class: 'empty' }, '── select chat ──');
    chatSlot.appendChild(empty);
    const main = h('div', { class: 'main' }, sidebar.el, chatSlot);
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
