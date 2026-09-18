import { open, save } from '@tauri-apps/plugin-dialog';
import { Api, CONSOLE_COMMAND, CONSOLE_OUTPUT, CONSOLE_GRANT, CONSOLE_REVOKE, CONSOLE_OFF } from './api';
import type { Message } from './api';
import type { Store, ChatTarget } from './state';
import { targetKey, sameTarget, pasteFileToTempPath } from './state';
import { icon } from './icons';
import { onAvatarsChanged } from './avatars';
import { View, h, avatar, fmtTime, fmtDate, fmtAgo, trustLabel, short, humanSize, isImageName, mimeFromName } from './view';
import type { App } from './app';
import { PinnedBanner, EditInline, messageMenuItems, scrollLogToMessage, attachContextMenu } from './actions';
import { ContactModal } from './contact';
import { GroupModal } from './group';
import { SearchModal } from './search';
import { MediaModal } from './media';
import { ForwardModal } from './forward';

interface PendingFile { name: string; path: string; size: number }

export class ChatView extends View {
  el: HTMLElement;
  private log: HTMLElement;
  private logWrap: HTMLElement;
  private jumpBtn: HTMLElement;
  private input: HTMLTextAreaElement;
  private pending: PendingFile[] = [];
  private fileChips: HTMLElement;
  private ttlSecs: number | null = null;
  private ttlPicker: HTMLElement;
  private attachmentCache = new Map<number, Array<{ id: number; name: string; size: number }>>();
  private imageDataCache = new Map<number, string>();
  private pinnedBanner: PinnedBanner;
  private editingId: number | null = null;
  private replyTo: Message | null = null;
  private replyChip: HTMLElement;
  private pasteHandler: (e: ClipboardEvent) => void;
  private typingActive = false;
  private typingStopTimer: number | null = null;
  private typingHeader: HTMLElement;
  private loadingMore = false;
  private noMoreOlder = false;
  private prependCompensation: { prevHeight: number; prevTop: number } | null = null;
  private stickyBottom = true;
  private lastScrollTop = 0;
  private static readonly STICKY_THRESHOLD = 4;
  private static readonly LOAD_MORE_THRESHOLD = 200;
  private modeBtn: HTMLButtonElement;
  private agentOffBtn: HTMLButtonElement;
  private promptEl: HTMLElement;
  private unreachableNote: HTMLElement;

  constructor(private store: Store, private app: App, private target: ChatTarget) {
    super();
    const isGroup = target.kind === 'group';
    const title = isGroup
      ? (store.groups.get().find((g) => g.id === target.id)?.name ?? 'group')
      : (store.contacts.get().find((c) => c.id === target.id)?.name ?? 'unknown');

    const subEl = h('div', { class: 'chat-sub' });
    this.typingHeader = h('div', { class: 'chat-typing', style: { display: 'none' } });
    const statusEl = h('div', { class: 'chat-status' });
    const computeSub = (): string => isGroup
      ? `${(store.groupMembers.get().get(target.id)?.length ?? 0)} участн.`
      : '';
    subEl.textContent = computeSub();
    const renderStatus = (): void => {
      if (isGroup) { statusEl.textContent = ''; return; }
      const cid = target.id as number;
      const online = store.peerOnline.get().has(cid);
      if (online) {
        statusEl.textContent = 'в сети';
        statusEl.className = 'chat-status online';
        return;
      }
      const ls = store.contacts.get().find((c) => c.id === cid)?.last_seen ?? null;
      statusEl.className = 'chat-status';
      statusEl.textContent = ls != null ? `был(а) в сети ${fmtAgo(ls)}` : 'не в сети';
    };
    renderStatus();

    this.log = h('div', { class: 'chat-log' });
    this.jumpBtn = h('button', {
      class: 'jump-btn',
      title: 'jump to latest',
      style: { display: 'none' },
      onClick: () => this.scrollBottom(),
    }, '↓');
    this.logWrap = h('div', { class: 'chat-log-wrap' }, this.log, this.jumpBtn);
    this.log.addEventListener('scroll', () => {
      const now = this.log.scrollTop;
      if (now < this.lastScrollTop - 1) {
        this.stickyBottom = false;
      } else if (this.isAtBottom()) {
        this.stickyBottom = true;
      }
      this.lastScrollTop = now;
      this.updateJumpBtn();
      this.maybeLoadMore();
    });
    const isTouch = window.matchMedia('(hover: none) and (pointer: coarse)').matches;
    // On a touch keyboard Enter is a newline and there is no Shift+Enter to
    // explain; the long hint also wraps to a second line at phone width.
    const placeholderFor = (consoleMode: boolean): string => {
      if (isTouch) return consoleMode ? 'Команда…' : 'Сообщение…';
      return consoleMode
        ? 'Команда… (Enter — выполнить, Shift+Enter — новая строка)'
        : 'Сообщение… (Enter — отправить, Shift+Enter — новая строка)';
    };
    this.input = h('textarea', {
      placeholder: placeholderFor(false),
      rows: '1',
    }) as HTMLTextAreaElement;
    this.input.addEventListener('keydown', (e) => {
      const ke = e as KeyboardEvent;
      if (isTouch) return;
      if (ke.key === 'Enter' && !ke.shiftKey) { e.preventDefault(); this.send(); }
    });
    const supportsFieldSizing = ((): boolean => {
      try { return typeof CSS !== 'undefined' && CSS.supports('field-sizing', 'content'); }
      catch { return false; }
    })();
    let resizePending = false;
    const resizeInput = (): void => {
      if (supportsFieldSizing || resizePending) return;
      resizePending = true;
      requestAnimationFrame(() => {
        resizePending = false;
        this.input.style.height = 'auto';
        this.input.style.height = Math.min(this.input.scrollHeight, 180) + 'px';
      });
    };
    this.input.addEventListener('input', () => {
      resizeInput();
      this.bumpTyping();
    });
    this.pasteHandler = (e: ClipboardEvent) => this.handlePaste(e);
    document.addEventListener('paste', this.pasteHandler);

    this.fileChips = h('div', { class: 'msg-attachments', style: { marginTop: '0' } });
    this.replyChip = h('div', { class: 'reply-chip', style: { display: 'none' } });
    this.ttlPicker = h('div', { class: 'ttl-picker' });
    if (!isGroup) this.renderTtlPicker();

    // Icon and label both; the stylesheet shows the label where the chat pane
    // has room for it and the icon alone where it does not.
    const detailsBtn = h('button', {
      class: 'btn btn-ghost chat-details',
      title: isGroup ? 'Участники' : 'Профиль контакта',
      onClick: () => (isGroup ? this.openGroupDetails() : this.openContactDetails()),
    },
      h('span', { class: 'chat-details-icon', 'aria-hidden': 'true' }, icon('info', 18)),
      h('span', { class: 'chat-details-label' }, isGroup ? 'Участники' : 'Профиль'),
    );
    const searchBtn = h('button', { class: 'icon-btn', title: 'Поиск в чате', onClick: () => this.openSearch() }, icon('search'));
    const mediaBtn = h('button', { class: 'icon-btn', title: 'Медиа и файлы', onClick: () => this.openMedia() }, icon('image'));

    const agentControls = h('div', { class: 'agent-controls' });
    this.modeBtn = h('button', {
      class: 'btn btn-ghost btn-sm',
      onClick: () => {
        store.toggleConsoleMode(target);
        renderAgentControls();
        renderInputMode();
        this.renderLog();
      },
    }) as HTMLButtonElement;
    this.agentOffBtn = h('button', {
      class: 'btn btn-danger btn-sm',
      onClick: () => void this.disableRemoteAgent(),
    }, 'disable') as HTMLButtonElement;
    const renderAgentControls = (): void => {
      if (isGroup) { agentControls.replaceChildren(); return; }
      const c = store.contacts.get().find((x) => x.id === (target.id as number));
      if (!c?.agent_granted) {
        agentControls.replaceChildren();
        if (store.isConsoleMode(target)) store.toggleConsoleMode(target);
        return;
      }
      const consoleMode = store.isConsoleMode(target);
      this.modeBtn.textContent = consoleMode ? 'chat' : 'console';
      this.modeBtn.title = consoleMode ? 'show ordinary chat' : 'open remote console';
      agentControls.replaceChildren(this.modeBtn, this.agentOffBtn);
    };
    const renderInputMode = (): void => {
      const consoleMode = !isGroup && store.isConsoleMode(target);
      this.el?.classList.toggle('console', consoleMode);
      this.promptEl.textContent = consoleMode ? '$' : '>';
      this.input.placeholder = placeholderFor(consoleMode);
      this.ttlPicker.classList.toggle('hidden', consoleMode);
      this.replyChip.classList.toggle('hidden', consoleMode);
    };
    renderAgentControls();

    const headerRight = isGroup
      ? h('div', { class: 'row chat-actions' }, searchBtn, mediaBtn, detailsBtn)
      : (() => {
          const c = store.contacts.get().find((x) => x.id === (target.id as number));
          return h('div', { class: 'row chat-actions' },
            (c?.trust ?? 0) !== 0 && h('span', {
              class: 'trust-badge trust-' + (c?.trust ?? 0),
              title: trustLabel(c?.trust ?? 0),
            }, trustLabel(c?.trust ?? 0)),
            agentControls,
            searchBtn,
            mediaBtn,
            detailsBtn,
          );
        })();

    this.pinnedBanner = new PinnedBanner(store, target);

    const avatarSeed = isGroup ? String(target.id) : (store.contacts.get().find((x) => x.id === target.id)?.sign_pk ?? title);
    const headerAvatar = h('div', { class: 'chat-avatar-slot' }, avatar(title, avatarSeed, (isGroup ? 'avatar-group ' : '') + 'chat-avatar', !isGroup));
    this.subs.push(onAvatarsChanged(() => {
      headerAvatar.replaceChildren(avatar(title, avatarSeed, (isGroup ? 'avatar-group ' : '') + 'chat-avatar', !isGroup));
    }));

    // With built-in relays a contact's address lasts until they restart. If it
    // has been silent for a while with mail queued, say so and say what helps —
    // the alternative is messages that wait forever with no explanation.
    this.unreachableNote = h('div', { class: 'chat-notice hidden' },
      'Контакт давно не на связи. Сообщения сохранены и будут отправляться повторно, пока он не появится '
      + 'в сети (до 7 дней); когда он запустит приложение, его новый адрес придёт сам.');
    this.sub(store.unreachable, (set) => {
      const hit = target.kind === 'contact' && set.has(target.id as number);
      this.unreachableNote.classList.toggle('hidden', !hit);
    }, true);

    this.el = h('div', { class: 'chat' },
      h('div', { class: 'chat-header' },
        h('button', {
          class: 'chat-back icon-btn',
          title: 'back',
          onClick: () => store.selectedChat.set(null),
        }, icon('back')),
        headerAvatar,
        h('div', { class: 'chat-heading' },
          h('div', { class: 'chat-title', title }, title),
          subEl,
          statusEl,
          this.typingHeader,
        ),
        headerRight,
      ),
      this.unreachableNote,
      this.pinnedBanner.el,
      this.logWrap,
      h('div', { class: 'chat-input' },
        this.replyChip,
        this.fileChips,
        h('div', { class: 'chat-input-row' },
          this.promptEl = h('div', { class: 'prompt' }, '>'),
          this.input,
          h('button', { class: 'icon-btn', title: 'Прикрепить файл', onClick: () => this.pickFiles() }, icon('attach')),
          h('button', { class: 'btn chat-send', title: 'Отправить', onClick: () => this.send() }, icon('send', 18), h('span', { class: 'chat-send-label' }, 'Отправить')),
        ),
        h('div', { class: 'chat-input-meta' },
          h('span', { class: 'chat-e2e' }, icon('lock', 13), 'Сквозное шифрование'),
          this.ttlPicker,
        ),
      ),
    );

    this.sub(store.messages, () => this.renderLog());
    this.sub(store.groupMembers, () => {
      subEl.textContent = computeSub();
      if (isGroup) this.renderLog();
    }, false);
    this.sub(store.pinned, () => this.renderLog(), false);
    this.sub(store.contacts, () => { renderAgentControls(); this.renderLog(); }, false);
    this.sub(store.consoleMode, () => { renderAgentControls(); renderInputMode(); this.renderLog(); }, false);
    this.sub(store.scrollToMessage, (s) => {
      if (!s || !sameTarget(s.target, target)) return;
      void this.scrollToMessage(s.messageId);
    }, false);
    this.sub(store.typing, (m) => this.renderTypingHeader(m), true);
    this.sub(store.peerOnline, () => renderStatus(), false);
    this.sub(store.onlineTick, () => renderStatus(), false);
    this.sub(store.contacts, () => renderStatus(), false);

    if (isGroup && !store.groupMembers.get().has(target.id)) {
      Api.listGroupMembers(target.id as string).then((members) => {
        store.groupMembers.update((m) => { const n = new Map(m); n.set(target.id as string, members); return n; });
      }).catch(() => {});
    }
    renderInputMode();
  }

  /**
   * Which messages this pane shows, and how a console frame becomes a row.
   *
   * The master's console pane is only framed messages. In the ordinary log a
   * client that holds a peer's console keeps commands and output out of the
   * conversation (they live in the console pane); a client that only *is* an
   * agent has no pane, so it keeps them inline, marked as commands. Mode
   * markers (grant/revoke/off) are conversation-level events and always show.
   */
  private showsInLog(m: Message): boolean {
    const kind = m.console?.kind;
    if (kind == null) return true;
    if (this.store.isConsoleMode(this.target)) return true;
    if (kind === CONSOLE_COMMAND || kind === CONSOLE_OUTPUT) {
      return !this.holdsPeerConsole();
    }
    return true;
  }

  private holdsPeerConsole(): boolean {
    if (this.target.kind !== 'contact') return false;
    return !!this.store.contacts.get().find((c) => c.id === (this.target.id as number))?.agent_granted;
  }

  destroy(): void {
    document.removeEventListener('paste', this.pasteHandler);
    if (this.typingStopTimer != null) window.clearTimeout(this.typingStopTimer);
    if (this.typingActive) {
      const cid = this.target.kind === 'contact' ? (this.target.id as number) : null;
      const gid = this.target.kind === 'group' ? (this.target.id as string) : null;
      Api.sendTyping(cid, gid, false).catch(() => {});
      this.typingActive = false;
    }
    this.pinnedBanner.destroy();
    super.destroy();
  }

  private bumpTyping(): void {
    const cid = this.target.kind === 'contact' ? (this.target.id as number) : null;
    const gid = this.target.kind === 'group' ? (this.target.id as string) : null;
    if (!this.typingActive) {
      this.typingActive = true;
      Api.sendTyping(cid, gid, true).catch(() => {});
    }
    if (this.typingStopTimer != null) window.clearTimeout(this.typingStopTimer);
    this.typingStopTimer = window.setTimeout(() => {
      this.typingActive = false;
      Api.sendTyping(cid, gid, false).catch(() => {});
      this.typingStopTimer = null;
    }, 3000);
  }

  private renderTypingHeader(map: Map<string, { sender_sign_pk: string | null; until: number }>): void {
    const key = targetKey(this.target);
    const info = map.get(key);
    if (!info) {
      this.typingHeader.style.display = 'none';
      this.typingHeader.textContent = '';
      return;
    }
    let who = '…';
    if (this.target.kind === 'contact') {
      who = this.store.contacts.get().find((c) => c.id === this.target.id)?.name ?? '…';
    } else if (info.sender_sign_pk) {
      const members = this.store.groupMembers.get().get(this.target.id as string) ?? [];
      who = members.find((m) => m.sign_pk === info.sender_sign_pk)?.name
        ?? this.store.contacts.get().find((c) => c.sign_pk === info.sender_sign_pk)?.name
        ?? 'someone';
    }
    this.typingHeader.textContent = `${who} is typing…`;
    this.typingHeader.style.display = '';
  }

  private async pickFiles(): Promise<void> {
    try {
      const sel = await open({ multiple: true });
      if (!sel) return;
      const arr = Array.isArray(sel) ? sel : [sel];
      for (const p of arr) {
        const path = typeof p === 'string' ? p : (p as { path: string }).path;
        if (!path) continue;
        const name = path.replace(/\\/g, '/').split('/').pop() ?? 'file';
        this.pending.push({ name, path, size: 0 });
      }
      this.renderFileChips();
    } catch (e) {
      this.store.showToast('pick failed: ' + String(e), true);
    }
  }

  private async handlePaste(e: ClipboardEvent): Promise<void> {
    if (this.app.isModalActive()) return;
    const items = e.clipboardData?.items;
    const itemsArr = items ? Array.from(items) : [];
    const hasImage = itemsArr.some((it) => it.type.startsWith('image/'));
    const hasText = itemsArr.some((it) => it.kind === 'string');
    const targetEl = e.target as Node | null;
    const inInput = !!(targetEl && this.input.contains(targetEl));
    if (inInput && hasText && !hasImage) return;
    const collected: File[] = [];
    for (const item of itemsArr) {
      if (item.type.startsWith('image/')) {
        const f = item.getAsFile();
        if (f) {
          const ext = item.type.split('/')[1] ?? 'png';
          collected.push(new File([f], f.name || `pasted-${Date.now()}.${ext}`, { type: item.type }));
        }
      }
    }
    if (collected.length === 0) {
      let path: string | null = null;
      try { path = await Api.pasteClipboardImage(); } catch { path = null; }
      if (!path) return;
      e.preventDefault();
      const name = path.replace(/\\/g, '/').split('/').pop() ?? 'clipboard.png';
      this.pending.push({ name, path, size: 0 });
      this.renderFileChips();
      return;
    }
    e.preventDefault();
    for (const f of collected) {
      try {
        const path = await pasteFileToTempPath(f);
        this.pending.push({ name: f.name, path, size: f.size });
      } catch (err) {
        this.store.showToast('paste failed: ' + String(err), true);
      }
    }
    this.renderFileChips();
  }

  private renderedKey: string = '';
  private renderedIds: number[] = [];
  private renderedSignatures: Map<number, string> = new Map();

  private msgSignature(m: Message): string {
    const pinned = this.isPinned(m) ? '1' : '0';
    const btns = m.buttons ? JSON.stringify(m.buttons) : '';
    const consoleFrame = m.console ? JSON.stringify(m.console) : '';
    return `${m.body}|${m.sent ? 1 : 0}${m.delivered ? 1 : 0}|${pinned}|${btns}|${consoleFrame}|${this.editingId === m.id ? 'e' : ''}`;
  }

  private renderLog(): void {
    const key = targetKey(this.target);
    const list = (this.store.messages.get().get(key) ?? []).filter((m) => this.showsInLog(m));
    const wasAtBottom = this.isAtBottom();
    const newIds = list.map((m) => m.id);

    const targetChanged = this.renderedKey !== key;
    if (targetChanged) {
      this.noMoreOlder = false;
      this.loadingMore = false;
      this.prependCompensation = null;
      this.stickyBottom = true;
    }
    let prefixLen = 0;
    if (!targetChanged) {
      while (prefixLen < this.renderedIds.length && prefixLen < newIds.length
        && this.renderedIds[prefixLen] === newIds[prefixLen]) prefixLen++;
    }
    const isTailOnly = !targetChanged
      && prefixLen === this.renderedIds.length
      && newIds.length >= this.renderedIds.length;

    if (isTailOnly && this.renderedIds.length > 0) {
      const savedTop = this.log.scrollTop;
      for (let i = 0; i < prefixLen; i++) {
        const m = list[i];
        if (!m) continue;
        const sig = this.msgSignature(m);
        if (this.renderedSignatures.get(m.id) !== sig) {
          const old = this.log.querySelector(`[data-mid="${m.id}"]`);
          if (old) old.replaceWith(this.renderMessage(m));
          this.renderedSignatures.set(m.id, sig);
        }
      }
      const lastPrefixMsg = prefixLen > 0 ? list[prefixLen - 1] : undefined;
      let lastDate = lastPrefixMsg ? fmtDate(lastPrefixMsg.sent_at) : '';
      for (let i = prefixLen; i < list.length; i++) {
        const m = list[i];
        if (!m) continue;
        const d = fmtDate(m.sent_at);
        if (d !== lastDate) {
          this.log.appendChild(h('div', { class: 'divider-text' }, d));
          lastDate = d;
        }
        this.log.appendChild(this.renderMessage(m));
        this.renderedSignatures.set(m.id, this.msgSignature(m));
      }
      if (this.stickyBottom) this.scrollBottom();
      else this.log.scrollTop = savedTop;
    } else {
      const prevTop = this.log.scrollTop;
      this.log.replaceChildren();
      this.renderedSignatures.clear();
      let lastDate = '';
      for (const m of list) {
        const d = fmtDate(m.sent_at);
        if (d !== lastDate) {
          this.log.appendChild(h('div', { class: 'divider-text' }, d));
          lastDate = d;
        }
        this.log.appendChild(this.renderMessage(m));
        this.renderedSignatures.set(m.id, this.msgSignature(m));
      }
      if (this.prependCompensation) {
        const { prevHeight, prevTop } = this.prependCompensation;
        this.prependCompensation = null;
        const newHeight = this.log.scrollHeight;
        this.log.scrollTop = prevTop + (newHeight - prevHeight);
      } else if (targetChanged || this.stickyBottom) {
        this.scrollBottom();
      } else {
        this.log.scrollTop = prevTop;
      }
    }

    this.renderedKey = key;
    this.renderedIds = newIds;
    this.updateJumpBtn();
  }

  private maybeLoadMore(): void {
    if (this.loadingMore || this.noMoreOlder) return;
    if (this.log.scrollTop > ChatView.LOAD_MORE_THRESHOLD) return;
    if (this.renderedIds.length === 0) return;
    this.loadingMore = true;
    this.prependCompensation = { prevHeight: this.log.scrollHeight, prevTop: this.log.scrollTop };
    this.store.loadMoreMessages(this.target).then((more) => {
      if (!more) {
        this.noMoreOlder = true;
        this.prependCompensation = null;
      }
    }).catch(() => {
      this.prependCompensation = null;
    }).finally(() => {
      this.loadingMore = false;
    });
  }

  private async scrollToMessage(messageId: number): Promise<void> {
    const present = this.renderedIds.includes(messageId);
    if (!present) {
      try {
        const ok = await this.store.loadUntilMessage(this.target, messageId);
        if (!ok) {
          this.store.showToast('message not found in this chat', true);
          return;
        }
      } catch {
        this.store.showToast('load failed', true);
        return;
      }
    }
    this.stickyBottom = false;
    let frames = 0;
    const tryScroll = (): void => {
      const el = this.log.querySelector(`[data-mid="${messageId}"]`);
      if (el) {
        scrollLogToMessage(this.log, messageId);
        return;
      }
      if (++frames > 30) return;
      requestAnimationFrame(tryScroll);
    };
    requestAnimationFrame(tryScroll);
  }

  private isAtBottom(): boolean {
    return this.log.scrollHeight - this.log.scrollTop - this.log.clientHeight < ChatView.STICKY_THRESHOLD;
  }

  private updateJumpBtn(): void {
    this.jumpBtn.style.display = this.isAtBottom() ? 'none' : 'flex';
  }

  private senderNameFor(m: Message): string {
    if (m.outgoing) return '';
    if (this.target.kind !== 'group') return '';
    if (!m.sender_sign_pk) return 'unknown';
    const members = this.store.groupMembers.get().get(this.target.id as string) ?? [];
    const member = members.find((mm) => mm.sign_pk === m.sender_sign_pk);
    if (member) return member.name;
    const contact = this.store.contacts.get().find((c) => c.sign_pk === m.sender_sign_pk);
    return contact?.name ?? 'unknown';
  }

  private isPinned(m: Message): boolean {
    const pins = this.store.pinned.get().get(targetKey(this.target)) ?? [];
    return pins.some((p) => p.id === m.id);
  }

  private isFromBot(m: Message): boolean {
    const contacts = this.store.contacts.get();
    if (this.target.kind === 'contact') {
      const c = contacts.find((x) => x.id === this.target.id);
      return !!c?.is_bot;
    }
    if (!m.sender_sign_pk) return false;
    const c = contacts.find((x) => x.sign_pk === m.sender_sign_pk);
    return !!c?.is_bot;
  }

  private renderMessage(m: Message): HTMLElement {
    if (m.console) return this.renderConsoleMessage(m);
    if (this.editingId === m.id) return this.renderEditing(m);
    const meta = m.outgoing
      ? (m.delivered ? '✓✓' : (m.sent ? '✓' : '🕓'))
      : '';
    const metaTitle = m.delivered ? 'Доставлено' : m.sent ? 'Отправлено' : 'Ждёт отправки';
    const sender = this.senderNameFor(m);
    const pinned = this.isPinned(m);
    const fromBot = !m.outgoing && this.isFromBot(m);
    const replyQuote = m.reply_to != null ? this.renderReplyQuote(m.reply_to) : null;
    const wrap = h('div', {
      class: 'msg ' + (m.outgoing ? 'out' : 'in') + (pinned ? ' pinned' : '') + (fromBot ? ' bot' : ''),
      'data-mid': String(m.id),
    },
      sender && h('div', { class: 'msg-sender' }, sender),
      replyQuote,
      h('div', { class: 'msg-row' },
        h('div', { class: 'msg-body' }, m.body,
          h('span', { class: 'msg-foot' },
            pinned && h('span', { class: 'msg-pin-indicator', title: 'Закреплено' }, '📌'),
            h('span', { class: 'msg-ts' }, fmtTime(m.sent_at)),
            meta && h('span', { class: 'msg-meta' + (m.delivered ? ' delivered' : ''), title: metaTitle }, meta),
          ),
        ),
      ),
    );
    attachContextMenu(wrap, () => messageMenuItems(
      this.store, this.target, m,
      () => this.startEdit(m),
      () => this.startReply(m),
      () => this.openForward(m),
    ));
    this.loadAttachmentsFor(m.id, wrap);
    if (m.buttons && m.buttons.length > 0 && !m.outgoing) {
      const btns = h('div', { class: 'msg-buttons' });
      for (const row of m.buttons) {
        const rowEl = h('div', { class: 'msg-button-row' });
        for (const b of row) {
          rowEl.appendChild(h('button', {
            class: 'msg-button',
            onClick: async () => {
              try {
                if (this.target.kind === 'group') {
                  await this.store.pressGroupButton(this.target.id as string, m.id, b.callback_data);
                } else if (m.contact_id != null) {
                  await this.store.pressButton(m.contact_id, m.id, b.callback_data);
                }
              } catch (e) {
                console.error('[press_button]', e);
                this.store.showToast('button failed: ' + String(e), true);
              }
            },
          }, b.text));
        }
        btns.appendChild(rowEl);
      }
      wrap.appendChild(btns);
    }
    return wrap;
  }

  /**
   * A console-framed row. Commands read as `$ line`, output as its text with a
   * dim `exit · time · truncated` trailer, and the three mode markers as the
   * same system divider the chat uses for days — they are events about the
   * conversation, not console traffic.
   */
  private renderConsoleMessage(m: Message): HTMLElement {
    const frame = m.console!;
    if (frame.kind === CONSOLE_GRANT || frame.kind === CONSOLE_REVOKE || frame.kind === CONSOLE_OFF) {
      const text = frame.kind === CONSOLE_GRANT ? 'Консоль открыта'
        : frame.kind === CONSOLE_OFF ? 'Мастер выключил режим агента'
        : 'Консоль закрыта';
      return h('div', { class: 'divider-text console-divider', 'data-mid': String(m.id) }, text);
    }
    const wrap = h('div', { class: 'console-msg', 'data-mid': String(m.id) });
    if (frame.kind !== CONSOLE_COMMAND && frame.kind !== CONSOLE_OUTPUT) {
      wrap.appendChild(h('div', { class: 'divider-text console-divider' }, `── console #${frame.kind} ──`));
      return wrap;
    }
    if (frame.kind === CONSOLE_COMMAND) {
      wrap.classList.add('cmd');
      wrap.appendChild(h('span', { class: 'console-prompt' }, '$'));
      wrap.appendChild(h('div', { class: 'console-body' }, m.body));
    } else {
      wrap.classList.add('out');
      wrap.appendChild(h('div', { class: 'console-body' }, m.body || '(no output)'));
      const bits: string[] = [];
      if (m.outgoing) bits.push(frame.exit_code == null ? 'no exit code' : `exit ${frame.exit_code}`);
      if (frame.duration_ms != null) bits.push(`${(frame.duration_ms / 1000).toFixed(2)}s`);
      if (frame.truncated) bits.push('truncated');
      if (bits.length > 0) wrap.appendChild(h('div', { class: 'console-meta' }, bits.join(' · ')));
    }
    this.loadAttachmentsFor(m.id, wrap);
    return wrap;
  }

  private async disableRemoteAgent(): Promise<void> {
    try {
      await this.store.sendAgentOff(this.target);
      this.store.showToast('agent mode off requested');
    } catch (e) {
      this.store.showToast('agent off failed: ' + String(e), true);
    }
  }

  private renderEditing(m: Message): HTMLElement {
    const wrap = h('div', {
      class: 'msg out editing',
      'data-mid': String(m.id),
    });
    const editor = new EditInline(
      m.body,
      (newBody) => {
        this.store.editMessage(this.target, m.id, newBody)
          .then(() => { this.editingId = null; this.renderLog(); })
          .catch((e) => this.store.showToast('edit failed: ' + e, true));
      },
      () => { this.editingId = null; this.renderLog(); },
    );
    wrap.appendChild(editor.el);
    return wrap;
  }

  private startEdit(m: Message): void {
    if (!m.outgoing) return;
    this.editingId = m.id;
    this.renderLog();
  }

  private startReply(m: Message): void {
    this.replyTo = m;
    this.renderReplyChip();
    this.input.focus();
  }

  private cancelReply(): void {
    this.replyTo = null;
    this.renderReplyChip();
  }

  private renderReplyChip(): void {
    this.replyChip.replaceChildren();
    if (!this.replyTo) {
      this.replyChip.style.display = 'none';
      return;
    }
    const m = this.replyTo;
    const senderLabel = m.outgoing
      ? 'you'
      : (this.senderNameFor(m) || 'unknown');
    const preview = m.body
      ? (m.body.length > 80 ? m.body.slice(0, 80) + '…' : m.body)
      : '(attachment)';
    this.replyChip.style.display = '';
    this.replyChip.appendChild(h('div', { class: 'reply-chip-bar' }));
    this.replyChip.appendChild(h('div', { class: 'reply-chip-content' },
      h('div', { class: 'reply-chip-label' }, `↩ replying to ${senderLabel}`),
      h('div', { class: 'reply-chip-body' }, preview),
    ));
    this.replyChip.appendChild(h('button', {
      class: 'reply-chip-close',
      title: 'cancel reply',
      onClick: () => this.cancelReply(),
    }, '×'));
  }

  private renderReplyQuote(replyToId: number): HTMLElement {
    const list = this.store.messages.get().get(targetKey(this.target)) ?? [];
    const orig = list.find((x) => x.id === replyToId);
    if (!orig) {
      return h('div', { class: 'msg-reply unresolved' },
        h('div', { class: 'msg-reply-bar' }),
        h('div', { class: 'msg-reply-content' },
          h('div', { class: 'msg-reply-label' }, '↩'),
          h('div', { class: 'msg-reply-body' }, '(message unavailable)'),
        ),
      );
    }
    const senderLabel = orig.outgoing
      ? 'you'
      : (this.senderNameFor(orig) || 'unknown');
    const preview = orig.body
      ? (orig.body.length > 100 ? orig.body.slice(0, 100) + '…' : orig.body)
      : '(attachment)';
    return h('div', {
      class: 'msg-reply',
      onClick: (e: Event) => {
        e.stopPropagation();
        this.store.requestScrollTo(this.target, replyToId);
      },
    },
      h('div', { class: 'msg-reply-bar' }),
      h('div', { class: 'msg-reply-content' },
        h('div', { class: 'msg-reply-label' }, `↩ ${senderLabel}`),
        h('div', { class: 'msg-reply-body' }, preview),
      ),
    );
  }

  private async loadAttachmentsFor(msgId: number, container: HTMLElement): Promise<void> {
    let list = this.attachmentCache.get(msgId);
    if (!list) {
      list = await Api.listAttachments(msgId);
      this.attachmentCache.set(msgId, list);
    }
    if (list.length === 0) return;
    const bar = h('div', { class: 'msg-attachments' });
    for (const a of list) {
      if (isImageName(a.name)) {
        bar.appendChild(h('div', {
          class: 'attachment image-stub',
          onClick: () => this.openImageLazy(a.id, a.name),
        }, `▣ ${a.name} (${humanSize(a.size)}) — [ open ]`));
      } else {
        bar.appendChild(h('div', {
          class: 'attachment',
          onClick: () => this.downloadAttachment(a.id, a.name),
        }, `◆ ${a.name} (${humanSize(a.size)})`));
      }
    }
    container.appendChild(bar);
  }

  private async openImageLazy(attId: number, name: string): Promise<void> {
    let url = this.imageDataCache.get(attId);
    if (!url) {
      try {
        const b64 = await Api.loadAttachment(attId);
        url = `data:${mimeFromName(name)};base64,${b64}`;
        this.imageDataCache.set(attId, url);
      } catch (e) {
        this.store.showToast('load failed: ' + String(e), true);
        return;
      }
    }
    this.openImage(url, name);
  }

  private openImage(src: string, name: string): void {
    this.app.openModal((close) => {
      const stage = h('div', { class: 'zoom-stage' });
      const img = h('img', { src, class: 'zoom-img' }) as HTMLImageElement;
      stage.appendChild(img);
      let scale = 1, tx = 0, ty = 0;
      const apply = (): void => { img.style.transform = `translate(${tx}px, ${ty}px) scale(${scale})`; };
      stage.addEventListener('wheel', (e) => {
        e.preventDefault();
        const delta = e.deltaY > 0 ? 0.85 : 1.18;
        const newScale = Math.max(0.3, Math.min(10, scale * delta));
        const rect = stage.getBoundingClientRect();
        const cx = e.clientX - rect.left - rect.width / 2;
        const cy = e.clientY - rect.top - rect.height / 2;
        tx -= (cx - tx) * (newScale / scale - 1);
        ty -= (cy - ty) * (newScale / scale - 1);
        scale = newScale;
        apply();
      }, { passive: false });
      let dragging = false;
      let lastX = 0, lastY = 0;
      const onMove = (e: MouseEvent): void => {
        if (!dragging) return;
        tx += e.clientX - lastX;
        ty += e.clientY - lastY;
        lastX = e.clientX; lastY = e.clientY;
        apply();
      };
      const onUp = (): void => {
        dragging = false;
        stage.classList.remove('grabbing');
      };
      stage.addEventListener('mousedown', (e) => {
        dragging = true;
        lastX = e.clientX; lastY = e.clientY;
        stage.classList.add('grabbing');
        e.preventDefault();
      });
      document.addEventListener('mousemove', onMove);
      document.addEventListener('mouseup', onUp);
      const wrappedClose = (): void => {
        document.removeEventListener('mousemove', onMove);
        document.removeEventListener('mouseup', onUp);
        close();
      };
      const resetBtn = h('button', {
        class: 'btn btn-ghost',
        style: { marginLeft: '12px' },
        onClick: () => { scale = 1; tx = 0; ty = 0; apply(); },
      }, 'Сбросить');
      return h('div', {
        class: 'modal',
        style: { width: 'auto', maxWidth: '95vw', maxHeight: '95vh', padding: '0' },
      },
        h('div', { class: 'modal-header' },
          h('div', { class: 'modal-title' }, name),
          h('div', { class: 'hint', style: { marginLeft: '12px', fontSize: '11px' } },
            'wheel = zoom · drag = pan'),
          resetBtn,
          h('div', { class: 'grow' }),
          h('button', { class: 'icon-btn', onClick: wrappedClose }, 'x'),
        ),
        stage,
      );
    });
  }

  private async downloadAttachment(id: number, name: string): Promise<void> {
    try {
      const dest = await save({ defaultPath: name });
      if (!dest) return;
      await Api.saveAttachment(id, dest);
      this.store.showToast('saved');
    } catch (e) {
      this.store.showToast('save failed: ' + String(e), true);
    }
  }

  private renderFileChips(): void {
    this.fileChips.replaceChildren();
    this.pending.forEach((f, i) => {
      this.fileChips.appendChild(h('div', {
        class: 'attachment',
        onClick: () => { this.pending.splice(i, 1); this.renderFileChips(); },
      }, `◆ ${f.name} [x]`));
    });
  }

  private renderTtlPicker(): void {
    const options: Array<[string, number | null]> = [
      ['off', null], ['5m', 300], ['1h', 3600], ['1d', 86400], ['7d', 604800],
    ];
    this.ttlPicker.replaceChildren();
    for (const [label, secs] of options) {
      this.ttlPicker.appendChild(h('div', {
        class: 'ttl-chip' + (this.ttlSecs === secs ? ' active' : ''),
        onClick: () => { this.ttlSecs = secs; this.renderTtlPicker(); },
      }, label));
    }
  }

  private async send(): Promise<void> {
    const body = this.input.value.trim();
    if (!body && this.pending.length === 0) return;
    const paths = this.pending.map((f) => f.path);
    const consoleMode = this.target.kind === 'contact' && this.store.isConsoleMode(this.target);
    if (consoleMode) {
      // A console line is a command, not a message: attachments go up first
      // (the agent saves them), then the body runs.
      this.input.value = '';
      this.input.style.height = 'auto';
      this.pending = [];
      this.renderFileChips();
      try {
        await this.store.sendConsoleCommand(this.target, body, paths);
      } catch (e) {
        this.store.showToast('command failed: ' + String(e), true);
      }
      return;
    }
    const replyToId = this.replyTo?.id ?? null;
    this.input.value = '';
    this.input.style.height = 'auto';
    this.pending = [];
    this.replyTo = null;
    this.renderFileChips();
    this.renderReplyChip();
    const ttl = this.target.kind === 'group' ? null : this.ttlSecs;
    try {
      await this.store.sendMessage(this.target, body, paths, ttl, replyToId);
    } catch (e) {
      this.store.showToast('send failed: ' + String(e), true);
    }
  }

  private scrollBottom(): void {
    this.stickyBottom = true;
    requestAnimationFrame(() => { this.log.scrollTop = this.log.scrollHeight; });
  }

  private openContactDetails(): void {
    this.app.openModal((close) => new ContactModal(this.store, this.app, this.target.id as number, close).el);
  }

  private openGroupDetails(): void {
    this.app.openModal((close) => new GroupModal(this.store, this.app, this.target.id as string, close).el);
  }

  private openSearch(): void {
    const scope = this.target.kind === 'contact'
      ? { contactId: this.target.id as number, groupId: null }
      : { contactId: null, groupId: this.target.id as string };
    this.app.openModal((close) => new SearchModal(this.store, close, scope).el);
  }

  private openMedia(): void {
    this.app.openModal((close) => new MediaModal(this.store, this.target, close).el);
  }

  private openForward(m: Message): void {
    this.app.openModal((close) => new ForwardModal(this.store, m.id, close).el);
  }
}
