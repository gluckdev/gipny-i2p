import { errorText } from './error-text';
import { t as tr, onLangChange } from '../features/localization/i18n';
import { LanguageSetup, SetupLayoutSelection, needsLanguageSetup } from '../features/onboarding/language-picker';
import { Api, type UpdateInfo } from './api';
import { emptyChatArt, icon } from './icons';
import type { Store, UpdateProgress, ChatTarget } from './state';
import { View, h, busy, humanSize } from './view';
import { ProfileSelect } from '../features/profiles/profile';
import { AuthCreate } from '../features/profiles/create';
import { ScreenUnlock } from '../features/authorization/screen-unlock';
import { AuthUnlock } from '../features/authorization/unlock';
import { AuthBooting } from '../features/authorization/boot';
import { Sidebar } from '../features/messaging/sidebar';
import { Calls } from '../features/voice/calls';
import { ChatView } from '../features/messaging/chat';
import { ToastLayer } from '../features/notifications/toast';
import { installEffects } from './effects';

export class App extends View {
  el: HTMLElement;
  private setupPending = needsLanguageSetup();
  private setupLayouts = new SetupLayoutSelection();
  private current: View | null = null;
  private coveredDraft: ReturnType<MainView['captureDraft']> = null;
  private updateBanner = h('div', { class: 'update-banner hidden', role: 'status', 'aria-live': 'polite' });
  private updateModalEl: HTMLElement | null = null;
  private updateProgressEl: HTMLElement | null = null;
  private updateStatusEl: HTMLElement | null = null;
  private updateActionsEl: HTMLElement | null = null;
  private modalClosers = new Set<() => void>();
  private toasts = new ToastLayer();

  readonly calls: Calls;

  constructor(private store: Store) {
    super();
    this.calls = new Calls(this, store);
    store.onCallFrame = (peer, frame, at) => this.calls.receive(peer, frame, at);
    this.subs.push(() => { store.onCallFrame = undefined; });
    this.subs.push(installEffects());
    this.el = h('div', { class: 'stack app-root', style: { height: '100%' } }, this.updateBanner);
    this.sub(store.view, (v) => this.render(v));
    this.sub(store.screenLocked, () => this.render(store.view.get()), false);
    this.sub(store.selectedChat, () => this.closeModals(), false);
    this.sub(store.toast, (t) => this.renderToast(t));
    this.sub(store.updateAvailable, (info) => this.onUpdateAvailable(info));
    this.sub(store.updateProgress, (p) => this.onUpdateProgress(p));
    this.sub(store.updateReadyPath, (path) => this.onUpdateReady(path));
    this.sub(store.updateError, (err) => this.onUpdateError(err));
    this.sub(store.updateStaged, (version) => this.onUpdateStaged(version));
    this.sub(store.updateInstallerOpened, (info) => this.onUpdateInstallerOpened(info));
    this.subs.push(onLangChange(() => this.refreshLanguage()));
  }

  private refreshLanguage(): void {
    const chatDraft = this.current instanceof MainView ? this.current.captureDraft() : null;
    const authDraft = this.current instanceof AuthCreate ? this.current.captureDraft() : null;
    // Form values stay in memory only; never copy credentials into the Store.
    // Setup owns its layout choices and AuthCreate owns its wizard state. Positional
    // input restoration would overwrite their defaults and lose the wizard step.
    const values = this.current instanceof LanguageSetup || this.current instanceof AuthCreate ? []
      : Array.from(this.current?.el.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>('input, textarea') ?? [])
        .map((el, index) => ({ id: el.id, index, value: el.value, checked: el instanceof HTMLInputElement && el.checked }));
    try {
      this.render(this.store.view.get());
      if (this.current instanceof MainView) this.current.restoreDraft(chatDraft);
      if (this.current instanceof AuthCreate) this.current.restoreDraft(authDraft);
      const inputs = this.current?.el.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>('input, textarea');
      for (const saved of values) {
        const input = saved.id ? Array.from(inputs ?? []).find(el => el.id === saved.id) : inputs?.[saved.index];
        if (!input) continue;
        input.value = saved.value;
        if (input instanceof HTMLInputElement) input.checked = saved.checked;
        if (input.classList.contains('sidebar-search')) input.dispatchEvent(new Event('input'));
      }
    } finally {
      // Release the temporary credential copies immediately after restoration.
      if (authDraft) authDraft.password = authDraft.confirmation = authDraft.duressPassword = '';
      for (const saved of values) saved.value = '';
    }
    if (this.store.updateStaged.get()) this.onUpdateStaged(this.store.updateStaged.get());
    else this.onUpdateProgress(this.store.updateProgress.get());
  }

  override destroy(): void {
    this.closeModals();
    this.clearToasts();
    this.toasts.destroy();
    this.current?.destroy();
    this.current = null;
    this.closeUpdateModal();
    super.destroy();
  }

  private render(view: string): void {
    if (view !== 'main') this.coveredDraft = null;
    else if (this.store.screenLocked.get() && this.current instanceof MainView) {
      this.coveredDraft = this.current.captureDraft();
      // A quoted message may be deleted or expire while the display is locked.
      if (this.coveredDraft) this.coveredDraft.replyTo = null;
    }
    this.closeModals();
    this.current?.destroy();
    if (this.store.screenLocked.get() && view === 'main') {
      this.clearToasts();
      this.closeUpdateModal();
      this.current = new ScreenUnlock(this.store);
      this.el.appendChild(this.current.el);
      return;
    }
    if (this.setupPending) {
      this.current = new LanguageSetup(this.setupLayouts, () => {
        this.setupPending = false;
        this.render(this.store.view.get());
      });
      this.el.appendChild(this.current.el);
      return;
    }
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
    if (next instanceof MainView && this.coveredDraft) {
      next.restoreDraft(this.coveredDraft);
      this.coveredDraft = null;
    }
  }

  private renderToast(t: { text: string; err: boolean } | null): void {
    this.toasts.show(t);
  }

  private clearToasts(): void {
    this.toasts.show(null);
  }

  closeModals(): void {
    for (const close of [...this.modalClosers]) close();
  }

  openModal(build: (close: () => void) => HTMLElement, onClose?: () => void): () => void {
    if (this.store.screenLocked.get()) { onClose?.(); return () => {}; }
    const backdrop = h('div', { class: 'modal-backdrop' });
    const close = () => {
      if (!this.modalClosers.delete(close)) return;
      backdrop.firstElementChild?.dispatchEvent(new Event('gipny-modal-close'));
      onClose?.();
      backdrop.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>('input, textarea').forEach((input) => { input.value = ''; });
      backdrop.querySelectorAll<HTMLMediaElement>('audio, video').forEach((media) => { media.pause(); media.removeAttribute('src'); media.load(); });
      backdrop.querySelectorAll('img').forEach((image) => image.removeAttribute('src'));
      backdrop.replaceChildren();
      backdrop.remove();
    };
    this.modalClosers.add(close);
    backdrop.addEventListener('click', (e) => { if (e.target === backdrop) close(); });
    backdrop.appendChild(build(close));
    document.body.appendChild(backdrop);
    return close;
  }

  /** Ask for one line of text; resolves null when cancelled. */
  prompt(title: string, label: string, initial = '', action = tr('common.save'), password = false): Promise<string | null> {
    return new Promise((resolve) => {
      this.openModal((close) => {
        const input = h('input', {
          class: 'input', value: initial, maxlength: '80',
          ...(password ? { type: 'password' } : {}),
        });
        const done = (v: string | null) => { resolve(v); close(); };
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
      }, () => resolve(null));
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
          h('button', { class: 'icon-btn', title: tr('common.close'), onClick: () => { resolve(false); close(); } }, icon('close')),
        ),
        h('div', { class: 'modal-body' }, h('div', { class: 'fp' }, body)),
        h('div', { class: 'modal-footer' },
          h('button', { class: 'btn btn-ghost', onClick: () => { resolve(false); close(); } }, tr('common.cancel')),
          h('button', {
            class: danger ? 'btn btn-danger' : 'btn',
            onClick: () => { resolve(true); close(); },
          }, tr('app.confirm')),
        ),
      ), () => resolve(false));
    });
  }

  private onUpdateAvailable(info: UpdateInfo | null): void {
    if (!info) { this.closeUpdateModal(); return; }
    // Settings owns its live status and install action while it is open.
    if (document.querySelector('[data-update-section]')) return;
    if (this.updateModalEl) this.closeUpdateModal();
    this.openUpdateModal(info);
  }

  private onUpdateProgress(p: UpdateProgress | null): void {
    if (!p) {
      if (!this.store.updateStaged.get()) this.updateBanner.classList.add('hidden');
      return;
    }
    if (!this.updateProgressEl || !this.updateStatusEl || !this.updateActionsEl) {
      this.updateBanner.classList.remove('hidden');
      this.updateBanner.replaceChildren(h('span', null, tr('app.downloading', {
        p0: humanSize(p.downloaded), p1: humanSize(p.total), p2: p.pct,
      })));
      return;
    }
    this.updateProgressEl.classList.remove('hidden');
    this.updateProgressEl.setAttribute('aria-valuenow', String(p.pct));
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

  private onUpdateInstallerOpened(info: { version: string; path: string } | null): void {
    if (!info) return;
    this.closeUpdateModal();
    this.openModal((close) => h('div', { class: 'modal modal-sm update-installer-handoff' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, tr('app.installer_opened')),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'about-p' }, tr('app.complete_update_in_system', { version: info.version })),
      ),
      h('div', { class: 'modal-footer' },
        h('button', {
          class: 'btn',
          onClick: () => { this.store.updateInstallerOpened.set(null); close(); },
        }, tr('common.close')),
      ),
    ));
  }

  /// An update is prepared and takes effect on the next start. The person is
  /// asked once, and can keep working — the running version still functions.
  private onUpdateStaged(version: string | null): void {
    this.updateBanner.classList.toggle('hidden', !version);
    if (!version) { this.updateBanner.replaceChildren(); return; }
    this.closeUpdateModal();
    const restart = h('button', {
      class: 'btn btn-sm', 'data-restart-update': '',
      onClick: () => busy(restart, async () => {
        try { await Api.restartApp(); }
        catch (error) { this.store.showToast(errorText(error), true); }
      }),
    }, tr('app.restart'));
    this.updateBanner.replaceChildren(
      h('span', null, tr('app.version_is_installed_and_will_take_effect_after', { p0: version })), restart,
    );
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
    if (this.store.screenLocked.get()) return;
    const backdrop = h('div', { class: 'modal-backdrop update-modal' });
    this.updateStatusEl = h('div', {
      class: 'fp',
      style: { marginTop: '8px', wordBreak: 'break-all' },
    }, tr('app.version', { p0: (info.version), p1: (humanSize(info.size)) }));

    this.updateProgressEl = h('div', {
      class: 'update-progress hidden', role: 'progressbar', 'aria-label': tr('settings.install_now'),
      'aria-valuemin': '0', 'aria-valuemax': '100', 'aria-valuenow': '0',
    }, h('div', { class: 'update-bar' }));

    this.updateActionsEl = h('div', { class: 'row', style: { gap: '8px', justifyContent: 'flex-end' } },
      h('button', {
        class: 'btn btn-ghost',
        onClick: async () => {
          try { await this.store.dismissUpdate(); this.closeUpdateModal(); }
          catch (error) { this.store.showToast(errorText(error), true); }
        },
      }, tr('app.later')),
      (() => {
        const b = h('button', {
          class: 'btn',
          onClick: () => busy(b, async () => {
            this.updateProgressEl?.classList.remove('hidden');
            await this.store.installUpdate();
          }),
        }, tr('app.update_now')) as HTMLButtonElement;
        // Offer the action only when this build has a supported install or
        // system-installer handoff route.
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
    // Ambient connection feedback never takes pointer events or focus.
    const networkText = h('span', { class: 'network-ambient-label', role: 'status', 'aria-live': 'polite' });
    const network = h('div', { class: 'network-ambient hidden' },
      h('div', { class: 'network-ambient-lines', 'aria-hidden': 'true' }, h('i'), h('i'), h('i')),
      networkText);
    const paintNetwork = (): void => {
      const info = store.relayInfo.get();
      const discovery = store.bootSteps.get().find(step => step.id === 'dht')?.state;
      const failed = info?.hosted.state === 'failed' || (info?.dial?.unreachable_checks ?? 0) >= 2 || discovery === 'failed';
      const ready = store.relayConnected.get() && (store.dhtReached.get() === true || discovery === 'skipped' || store.contacts.get().length === 0) && !failed;
      const unconfigured = info?.hosted.state === 'unconfigured';
      network.classList.toggle('hidden', ready);
      network.classList.toggle('is-retrying', failed);
      network.classList.toggle('is-offline', unconfigured);
      networkText.textContent = tr(unconfigured ? 'app.network_unconfigured' : failed ? 'app.network_retry' : 'app.network_connecting');
    };
    this.sub(store.relayConnected, paintNetwork);
    this.sub(store.relayInfo, paintNetwork);
    this.sub(store.dhtReached, paintNetwork);
    this.sub(store.bootSteps, paintNetwork);
    this.sub(store.contacts, paintNetwork);
    main.appendChild(network);

    // Agent mode remains an explicit, actionable safety control. A remote shell should never be easy to forget or accidentally
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
