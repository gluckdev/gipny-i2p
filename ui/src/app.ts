import { errorText } from './error-text';
import { t as tr, onLangChange } from './i18n';
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
    this.subs.push(onLangChange(() => this.refreshLanguage()));
  }

  private refreshLanguage(): void {
    const draft = this.current instanceof MainView ? this.current.captureDraft() : null;
    // Form values stay in memory only; never copy credentials into the Store.
    const values = Array.from(this.current?.el.querySelectorAll('input, textarea') ?? [])
      .map((el) => ({ value: (el as HTMLInputElement).value, checked: (el as HTMLInputElement).checked }));
    this.render(this.store.view.get());
    if (this.current instanceof MainView) this.current.restoreDraft(draft);
    this.current?.el.querySelectorAll('input, textarea').forEach((el, i) => {
      const saved = values[i];
      if (!saved) return;
      (el as HTMLInputElement).value = saved.value;
      (el as HTMLInputElement).checked = saved.checked;
      if (el.classList.contains('sidebar-search')) el.dispatchEvent(new Event('input'));
    });
  }

  override destroy(): void {
    this.current?.destroy();
    this.current = null;
    this.closeUpdateModal();
    super.destroy();
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
  prompt(title: string, label: string, initial = '', action = tr('common.save'), password = false): Promise<string | null> {
    return new Promise((resolve) => {
      this.openModal((close) => {
        const input = h('input', {
          class: 'input', value: initial, maxlength: '80',
          ...(password ? { type: 'password' } : {}),
        });
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
            h('button', { class: 'icon-btn', title: tr('common.close'), onClick: () => done(null) }, icon('close')),
          ),
          h('div', { class: 'modal-body' },
            h('div', { class: 'field' }, h('label', null, label), input),
          ),
          h('div', { class: 'modal-footer' },
            h('button', { class: 'btn btn-ghost', onClick: () => done(null) }, tr('common.cancel')),
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
          h('button', { class: 'icon-btn', title: tr('common.close'), onClick: () => { close(); resolve(false); } }, icon('close')),
        ),
        h('div', { class: 'modal-body' }, h('div', { class: 'fp' }, body)),
        h('div', { class: 'modal-footer' },
          h('button', { class: 'btn btn-ghost', onClick: () => { close(); resolve(false); } }, tr('common.cancel')),
          h('button', {
            class: danger ? 'btn btn-danger' : 'btn',
            onClick: () => { close(); resolve(true); },
          }, tr('app.confirm')),
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
      ? tr('app.downloading', { p0: (humanSize(p.downloaded)), p1: (humanSize(p.total)), p2: (p.pct) })
      : tr('app.downloading_2', { p0: (humanSize(p.downloaded)) });
    this.updateActionsEl.replaceChildren(
      h('div', { class: 'hint' }, tr('app.do_not_close_the_window_install_will_start')),
    );
  }

  private onUpdateReady(path: string | null): void {
    if (!path || !this.updateStatusEl || !this.updateActionsEl) return;
    this.updateStatusEl.textContent = tr('app.downloaded', { p0: (path) });
    this.updateActionsEl.replaceChildren(
      h('div', { class: 'hint' }, tr('app.install_the_file_then_restart_gipny')),
      h('button', {
        class: 'btn btn-ghost',
        onClick: () => { this.closeUpdateModal(); this.store.updateReadyPath.set(null); },
      }, tr('common.close')),
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
        h('div', { class: 'modal-title' }, tr('app.update_ready')),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'about-p' },
          tr('app.version_is_installed_and_will_take_effect_after', { p0: (version) })),
      ),
      h('div', { class: 'modal-footer' },
        h('button', {
          class: 'btn btn-ghost',
          onClick: () => { this.store.updateStaged.set(null); close(); },
        }, tr('app.later')),
        h('button', {
          class: 'btn',
          onClick: () => { this.store.updateStaged.set(null); Api.restartApp().catch(() => close()); },
        }, tr('app.restart')),
      ),
    ));
  }

  private onUpdateError(errMsg: string | null): void {
    if (!errMsg || !this.updateStatusEl || !this.updateActionsEl) return;
    this.updateStatusEl.textContent = tr('app.error', { p0: errorText(errMsg) });
    this.updateActionsEl.replaceChildren(
      h('button', {
        class: 'btn btn-ghost',
        onClick: () => { this.closeUpdateModal(); this.store.updateError.set(null); },
      }, tr('common.close')),
    );
  }

  private openUpdateModal(info: UpdateInfo): void {
    const backdrop = h('div', { class: 'modal-backdrop' });
    this.updateStatusEl = h('div', {
      class: 'fp',
      style: { marginTop: '8px', wordBreak: 'break-all' },
    }, tr('app.version', { p0: (info.version), p1: (humanSize(info.size)) }));

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
      }, tr('app.later')),
      (() => {
        const b = h('button', {
          class: 'btn',
          onClick: () => busy(b, async () => {
            (this.updateProgressEl as HTMLElement).style.display = 'block';
            await this.store.installUpdate();
          }),
        }, tr('app.update_now')) as HTMLButtonElement;
        // Android and unmanaged packages cannot install themselves; offering
        // the button would download into a directory nobody can reach.
        Api.updateInstallsItself().then((can: boolean) => {
          if (!can) {
            b.remove();
            if (this.updateStatusEl) {
              this.updateStatusEl.textContent +=
                tr('app.manual_installation_settings_android_app_or_the_releases');
            }
            return;
          }
          // A .deb goes through the package manager, which asks for the
          // administrator password — say so before the dialog appears.
          void Api.updateAsksForRoot().then((asks: boolean) => {
            if (asks && this.updateStatusEl) {
              this.updateStatusEl.textContent += tr('app.administrator_password_required');
            }
          }).catch(() => {});
        }).catch(() => {});
        return b;
      })(),
    );

    const modal = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, tr('app.a_new_version_is_available')),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'card-label' }, tr('app.release_notes')),
        h('div', {
          class: 'card-block',
          style: { whiteSpace: 'pre-wrap', maxHeight: '200px', overflowY: 'auto' },
        }, info.notes || tr('app.no_notes')),
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
  private chat: ChatView | null = null;
  captureDraft(): ReturnType<ChatView['captureDraft']> | null { return this.chat?.captureDraft() ?? null; }
  restoreDraft(draft: ReturnType<ChatView['captureDraft']> | null): void {
    if (draft) this.chat?.restoreDraft(draft);
  }

  constructor(store: Store, app: App) {
    super();
    const sidebar = new Sidebar(store, app);
    const chatSlot = h('div', { class: 'stack grow', style: { minHeight: '0' } });
    const empty = h('div', { class: 'empty empty-chat' },
      emptyChatArt(),
      h('div', { class: 'empty-title' }, tr('app.select_a_chat')),
      h('div', { class: 'empty-sub' }, tr('app.messages_are_encrypted_on_your_device_and_travel')),
    );
    chatSlot.appendChild(empty);
    const main = h('div', { class: 'main' }, sidebar.el, chatSlot);
    // In «кофеин» there is one chat and no way around it: the list, and with it
    // settings and lock, are not on screen at all.
    this.sub(store.caffeine, (on) => main.classList.toggle('caffeine', !!on), true);

    // Relay status. Nothing in the UI used to show it at all, so a client that
    // could not send looked identical to one that could — and with no relay
    // configured it cannot send to anyone. The banner says which of the two it
    // is and where to fix it.
    const banner = h('div', { class: 'relay-banner' });
    const paintBanner = (): void => {
      const unconfigured = store.relayUnconfigured.get();
      const connected = store.relayConnected.get();
      const info = store.relayInfo.get();
      // Our own relay is read over a pipe, so "connected" says nothing about
      // whether anyone else can reach it. Two failed checks in a row from the
      // network (one can be a LeaseSet still spreading) and we say so.
      const unreachable = (info?.dial?.unreachable_checks ?? 0) >= 2;
      banner.classList.toggle('hidden', connected && !unreachable);
      banner.classList.toggle('warn', unconfigured || (connected && unreachable));
      if (connected) {
        // Our relay is up, but the relay network is what makes us findable and
        // able to look people up. If we reached nobody, the banner says so:
        // "connected" here means our own relay answered, not that we are in
        // the network, and the difference is what someone is actually stuck on.
        const dht = store.dhtReached.get();
        if (dht === false) {
          const peers = store.dhtPeers.get();
          banner.replaceChildren(
            peers > 0
              ? tr('app.your_relay_is_running_but_no_i2p_network')
              : tr('app.your_relay_is_running_but_we_have_not')
          );
        } else if (unreachable) {
          const why = info?.dial?.unreachable_error ? ` (${info.dial.unreachable_error.slice(0, 120)})` : '';
          banner.replaceChildren(
            tr('app.your_relay_cannot_currently_be_reached_from_i2p', { p0: (info?.dial?.unreachable_checks ?? 0), p1: (why) })
          );
        }
        return;
      }
      const builtin = info?.mode === 'builtin';
      banner.classList.toggle('warn', unconfigured || (builtin && info?.hosted.state === 'failed'));
      // When the relay itself is up and we still cannot reach it, say what is
      // actually happening. A line that never changes ("нет связи с релеем")
      // is indistinguishable from a frozen app, and on a phone there is no
      // log to open — so the attempt count and the transport's own words go
      // on screen.
      const dial = info?.dial;
      const attempts = dial?.attempts ?? 0;
      const why = dial?.last_error ? ` (${dial.last_error.slice(0, 120)})` : '';
      const stuck = attempts > 0
        ? tr('app.cannot_connect_to_your_relay_attempt_messages_are', { p0: (attempts), p1: (why) })
        : tr('app.no_relay_connection_messages_will_be_sent_when');
      banner.replaceChildren(
        unconfigured
          ? tr('app.external_relay_selected_without_an_address_sending_is')
          : builtin && info?.hosted.state === 'failed'
            ? tr('app.built_in_relay_failed_to_start_retrying_messages', { p0: (info.hosted.reason ? ` (${info.hosted.reason.slice(0, 120)})` : '') })
            : builtin && info?.hosted.state !== 'ready'
              ? tr('app.built_in_relay_is_starting_usually_1_2')
              : stuck
      );
    };
    this.subs.push(store.relayConnected.subscribe(paintBanner, true));
    this.subs.push(store.relayUnconfigured.subscribe(paintBanner, true));
    this.subs.push(store.relayInfo.subscribe(paintBanner, false));
    this.subs.push(store.dhtReached.subscribe(paintBanner, false));
    this.subs.push(store.dhtPeers.subscribe(paintBanner, false));
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
            store.showToast(tr('app.agent_mode') + errorText(e), true);
          }
        }),
      }, tr('app.disable')) as HTMLButtonElement;
      agentBanner.replaceChildren(
        h('span', null, tr('app.agent_mode_master', { p0: (master.name || 'unnamed') })),
        off,
      );
    };
    main.insertBefore(agentBanner, main.firstChild);
    this.subs.push(store.agentMode.subscribe(paintAgentBanner, true));
    this.subs.push(store.sidebarCollapsed.subscribe((c: boolean) => {
      main.classList.toggle('sidebar-collapsed', c);
    }));
    this.subs.push(store.selectedChat.subscribe((target: ChatTarget | null) => {
      this.chat?.destroy();
      this.chat = null;
      chatSlot.replaceChildren();
      if (!target) {
        main.classList.remove('has-chat');
        chatSlot.appendChild(empty);
      } else {
        main.classList.add('has-chat');
        this.chat = new ChatView(store, app, target);
        chatSlot.appendChild(this.chat.el);
      }
    }));
    this.subs.push(() => { sidebar.destroy(); this.chat?.destroy(); });
    this.el = main;
  }
}
