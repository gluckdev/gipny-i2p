import { open, save } from '@tauri-apps/plugin-dialog';
import { Api } from './api';
import type { Message } from './api';
import type { Store, ChatTarget } from './state';
import { targetKey, sameTarget, pasteFileToTempPath } from './state';
import { View, h, fmtTime, fmtDate, trustLabel, short, humanSize, isImageName, mimeFromName } from './view';
import type { App } from './app';
import { PinnedBanner, EditInline, messageMenuItems, scrollLogToMessage, attachContextMenu } from './actions';
import { ContactModal } from './contact';
import { GroupModal } from './group';
import { SearchModal } from './search';
import { MediaModal } from './media';

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

  constructor(private store: Store, private app: App, private target: ChatTarget) {
    super();
    const isGroup = target.kind === 'group';
    const title = isGroup
      ? (store.groups.get().find((g) => g.id === target.id)?.name ?? 'group')
      : (store.contacts.get().find((c) => c.id === target.id)?.name ?? 'unknown');

    const subEl = h('div', { class: 'chat-sub' });
    this.typingHeader = h('div', { class: 'chat-typing', style: { display: 'none' } });
    const computeSub = (): string => isGroup
      ? `group · ${(store.groupMembers.get().get(target.id)?.length ?? 0)} members`
      : short(store.contacts.get().find((c) => c.id === target.id)?.onion ?? '');
    subEl.textContent = computeSub();

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
    this.input = h('textarea', {
      placeholder: 'type message... (enter to send, shift+enter newline)',
      rows: '1',
    }) as HTMLTextAreaElement;
    const isTouch = window.matchMedia('(hover: none) and (pointer: coarse)').matches;
    this.input.addEventListener('keydown', (e) => {
      const ke = e as KeyboardEvent;
      if (isTouch) return;
      if (ke.key === 'Enter' && !ke.shiftKey) { e.preventDefault(); this.send(); }
    });
    this.input.addEventListener('input', () => {
      this.input.style.height = 'auto';
      this.input.style.height = Math.min(this.input.scrollHeight, 180) + 'px';
      this.bumpTyping();
    });
    this.pasteHandler = (e: ClipboardEvent) => this.handlePaste(e);
    document.addEventListener('paste', this.pasteHandler);

    this.fileChips = h('div', { class: 'msg-attachments', style: { marginTop: '0' } });
    this.replyChip = h('div', { class: 'reply-chip', style: { display: 'none' } });
    this.ttlPicker = h('div', { class: 'ttl-picker' });
    if (!isGroup) this.renderTtlPicker();

    const detailsBtn = isGroup
      ? h('button', { class: 'btn btn-ghost', onClick: () => this.openGroupDetails() }, '[ members ]')
      : h('button', { class: 'btn btn-ghost', onClick: () => this.openContactDetails() }, '[ details ]');
    const searchBtn = h('button', { class: 'btn btn-ghost', title: 'search in this chat', onClick: () => this.openSearch() }, '[ ⌕ ]');
    const mediaBtn = h('button', { class: 'btn btn-ghost', title: 'media in this chat', onClick: () => this.openMedia() }, '[ ◉ ]');

    const headerRight = isGroup
      ? h('div', { class: 'row' }, searchBtn, mediaBtn, detailsBtn)
      : (() => {
          const c = store.contacts.get().find((x) => x.id === (target.id as number));
          return h('div', { class: 'row' },
            h('span', {
              class: 'trust-badge trust-' + (c?.trust ?? 0),
            }, trustLabel(c?.trust ?? 0)),
            searchBtn,
            mediaBtn,
            detailsBtn,
          );
        })();

    this.pinnedBanner = new PinnedBanner(store, target);

    this.el = h('div', { class: 'chat' },
      h('div', { class: 'chat-header' },
        h('button', {
          class: 'chat-back icon-btn',
          title: 'back',
          onClick: () => store.selectedChat.set(null),
        }, '[ ← ]'),
        h('div', null,
          h('div', { class: 'chat-title' }, title),
          subEl,
          this.typingHeader,
        ),
        headerRight,
      ),
      this.pinnedBanner.el,
      this.logWrap,
      h('div', { class: 'chat-input' },
        this.replyChip,
        this.fileChips,
        h('div', { class: 'chat-input-row' },
          h('div', { class: 'prompt' }, '>'),
          this.input,
          h('button', { class: 'btn btn-ghost', title: 'attach', onClick: () => this.pickFiles() }, '[+]'),
          h('button', { class: 'btn', onClick: () => this.send() }, '[ SEND ]'),
        ),
        h('div', { class: 'chat-input-meta' },
          h('span', null, 'ratchet: active · aead: xchacha20-poly1305'),
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
    this.sub(store.contacts, () => this.renderLog(), false);
    this.sub(store.scrollToMessage, (s) => {
      if (!s || !sameTarget(s.target, target)) return;
      void this.scrollToMessage(s.messageId);
    }, false);
    this.sub(store.typing, (m) => this.renderTypingHeader(m), true);

    if (isGroup && !store.groupMembers.get().has(target.id)) {
      Api.listGroupMembers(target.id as string).then((members) => {
        store.groupMembers.update((m) => { const n = new Map(m); n.set(target.id as string, members); return n; });
      }).catch(() => {});
    }
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
    return `${m.body}|${m.sent ? 1 : 0}${m.delivered ? 1 : 0}|${pinned}|${btns}|${this.editingId === m.id ? 'e' : ''}`;
  }

  private renderLog(): void {
    const key = targetKey(this.target);
    const list = this.store.messages.get().get(key) ?? [];
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
    if (this.editingId === m.id) return this.renderEditing(m);
    const meta = m.outgoing
      ? (m.delivered ? 'delivered ✓✓' : (m.sent ? 'sent ✓' : 'pending…'))
      : '';
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
        h('div', { class: 'msg-ts' }, fmtTime(m.sent_at)),
        h('div', { class: 'msg-body' }, m.body),
        pinned && h('div', { class: 'msg-pin-indicator', title: 'pinned' }, '◆'),
        meta && h('div', { class: 'msg-meta' }, meta),
      ),
    );
    attachContextMenu(wrap, () => messageMenuItems(this.store, this.target, m, () => this.startEdit(m), () => this.startReply(m)));
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
      }, '[ RESET ]');
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
}
