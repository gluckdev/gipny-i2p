import { errorText } from './error-text';
import { t as tr } from './i18n';
import { open, save } from '@tauri-apps/plugin-dialog';
import { forgetBrowserFiles } from './browser-files';
import { saveBrowserAttachment } from './browser-download';
import { Api, isTauri, usesBrowserCore, CONSOLE_COMMAND, CONSOLE_OUTPUT, CONSOLE_GRANT, CONSOLE_REVOKE, CONSOLE_OFF, PEER_EVENT } from './api';
import type { Message, MessageFiles, ChatPrefs, PeerEvent } from './api';
import type { Store, ChatTarget } from './state';
import { targetKey, sameTarget, pasteFileToTempPath } from './state';
import { icon } from './icons';
import { onAvatarsChanged } from './avatars';
import { View, h, avatar, fmtTime, fmtDate, fmtAgo, trustLabel, short, humanSize, isImageName, mimeFromName } from './view';
import type { App } from './app';
import { PinnedBanner, EditInline, ContextMenu, messageMenuItems, scrollLogToMessage, attachContextMenu } from './actions';
import { ContactModal } from './contact';
import { GroupModal } from './group';
import { SearchModal } from './search';
import { MediaModal } from './media';
import { ForwardModal } from './forward';
import { renderAnsi } from './ansi';
import { deletionRuns } from './deletion-notices';

interface PendingFile { name: string; path: string; size: number }

/** What the disappearing-message timer starts at, in seconds.
 *
 * A day, not an hour: this is the floor under a messenger that has no server
 * copy of anything, so a wrong guess costs the conversation rather than a
 * glance. Anyone who wants it shorter has one tap away, and the choice sticks. */
const DEFAULT_TTL_SECS = 86_400;
/** How a row with no files known yet, or none at all, is drawn. */
const NO_FILES = JSON.stringify({ attachments: [], progress: [] });

/** The picker offers the same steps in both directions; the label next to it is
 * what a person reads, so the seconds get said in words. */
const TTL_OPTIONS: Array<[string, number | null]> = [
  ['off', null], ['5m', 300], ['1h', 3600], ['1d', 86_400], ['7d', 604_800],
];

/** The chips say "5m" and "1d"; the note has to say what that is, in the
 * language the rest of the window is in. Units are keys rather than literals
 * because a half-translated sentence ("disappear after 5 мин") reads as a bug,
 * and a wholly untranslated one is at least honest about being untranslated. */
function ttlSpoken(secs: number | null): string {
  if (secs === null) return tr('chat.ttl_off');
  if (secs < 3600) return tr('chat.ttl_on', { time: tr('chat.ttl_minutes', { n: Math.round(secs / 60) }) });
  if (secs < 86_400) return tr('chat.ttl_on', { time: tr('chat.ttl_hours', { n: Math.round(secs / 3600) }) });
  return tr('chat.ttl_on', { time: tr('chat.ttl_days', { n: Math.round(secs / 86_400) }) });
}

/** One reminder per launch, not one per chat opened. */
let ttlNoticeShown = false;

export class ChatView extends View {
  el: HTMLElement;
  private log: HTMLElement;
  private logWrap: HTMLElement;
  private jumpBtn: HTMLElement;
  private input: HTMLTextAreaElement;
  private pending: PendingFile[] = [];
  private fileChips: HTMLElement;
  private ttlSecs: number | null = DEFAULT_TTL_SECS;
  private ttlPicker: HTMLElement;
  private ttlNote: HTMLElement;
  private ttlRow: HTMLElement;
  private peerNote: HTMLElement;
  private peerHistoryNote: HTMLElement;
  private historyPrompt: HTMLElement;
  private historySaving = false;
  private prefsRevision = 0;
  private attachmentCache = new Map<number, Array<{ id: number; name: string; size: number }>>();
  private fileCache = new Map<number, MessageFiles>();
  private fileVersions = new Map<number, number>();
  private fileMessageSignatures = new Map<number, string>();
  private fileQueue = new Set<number>();
  private fileBatchPending = false;
  private fileRetryTimer: number | null = null;
  private imageDataCache = new Map<number, string>();
  private imageMessages = new Map<number, number>();
  private imageClosers = new Map<number, () => void>();
  private ownedBlobUrls = new Set<string>();
  private disposed = false;
  private draftEpoch = 0;
  private filePickers = new Set<HTMLInputElement>();
  private deletedSeen = new Set<number>();
  private pinnedBanner: PinnedBanner;
  private editingId: number | null = null;
  /** The files each row was last drawn with (JSON), to skip redraws that
   *  would change nothing. */
  private drawnFiles = new Map<number, string>();
  private composer!: HTMLElement;
  private composerObserver: ResizeObserver | null = null;
  /** What the user typed into the inline editor, kept across re-renders. */
  private editDraft: string | undefined;
  private replyTo: Message | null = null;
  private replyChip: HTMLElement;
  private pasteHandler: (e: ClipboardEvent) => void;
  private copyHandler: (e: ClipboardEvent) => void;
  private keyHandler: (e: KeyboardEvent) => void;
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
  private caffeineBar: HTMLElement;
  private turboTitle: HTMLElement;
  private turboHint: HTMLElement;
  private fastBtn: HTMLButtonElement;
  private consoleHistory: string[] = [];
  private historyIndex = -1;
  private consoleDraft = '';
  private consoleClearedThrough = -1;
  private consoleSending = 0;
  private consoleSendingSince = 0;
  private consoleRunningTimer: number | null = null;

  constructor(private store: Store, private app: App, private target: ChatTarget) {
    super();
    const isGroup = target.kind === 'group';
    const title = isGroup
      ? (store.groups.get().find((g) => g.id === target.id)?.name ?? 'group')
      : (store.contacts.get().find((c) => c.id === target.id)?.name ?? tr('auth.unknown'));

    const subEl = h('div', { class: 'chat-sub' });
    this.typingHeader = h('div', { class: 'chat-typing', style: { display: 'none' } });
    const statusEl = h('div', { class: 'chat-status' });
    const computeSub = (): string => isGroup
      ? tr('chat.members', { p0: ((store.groupMembers.get().get(target.id)?.length ?? 0)) })
      : '';
    subEl.textContent = computeSub();
    const renderStatus = (): void => {
      if (isGroup) { statusEl.textContent = ''; return; }
      const cid = target.id as number;
      const online = store.peerOnline.get().has(cid);
      if (online) {
        statusEl.textContent = tr('chat.online');
        statusEl.className = 'chat-status online';
        return;
      }
      const ls = store.lastSeen(cid);
      statusEl.className = 'chat-status';
      const contact = store.contacts.get().find((c) => c.id === cid);
      const lost = contact ? store.lostForDays(contact) : null;
      statusEl.textContent = lost != null
        ? tr('chat.no_contact_for_days_messages_are_queued_if', { p0: (lost) })
        : ls != null ? tr('chat.last_seen', { p0: (fmtAgo(ls)) }) : tr('chat.offline');
    };
    renderStatus();

    this.log = h('div', { class: 'chat-log' });
    this.jumpBtn = h('button', {
      class: 'jump-btn',
      title: tr('chat.jump_to_latest'),
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
      if (isTouch) return consoleMode ? tr('chat.command') : tr('chat.message');
      return consoleMode
        ? tr('chat.command_enter_to_run_shift_enter_for_a')
        : tr('chat.input_placeholder');
    };
    this.input = h('textarea', {
      placeholder: placeholderFor(false),
      rows: '1',
    }) as HTMLTextAreaElement;
    this.input.addEventListener('keydown', (e) => {
      const ke = e as KeyboardEvent;
      if (ke.isComposing) return;
      if (store.isConsoleMode(target)) {
        if (ke.key === 'Tab') { e.preventDefault(); return; }
        if (ke.ctrlKey && !ke.altKey && (ke.key.toLowerCase() === 'c' || ke.key.toLowerCase() === 'l')) {
          e.preventDefault();
          if (ke.key.toLowerCase() === 'c') {
            this.input.value = '';
            this.historyIndex = -1;
            this.consoleDraft = '';
            this.input.dispatchEvent(new Event('input'));
          } else {
            const messages = store.messages.get().get(targetKey(target)) ?? [];
            this.consoleClearedThrough = messages.reduce((id, m) => Math.max(id, m.id), this.consoleClearedThrough);
            this.renderLog();
          }
          return;
        }
        if ((ke.key === 'ArrowUp' || ke.key === 'ArrowDown') && !ke.shiftKey && !ke.ctrlKey && !ke.altKey && !ke.metaKey
          && !this.input.value.includes('\n')) {
          e.preventDefault();
          if (ke.key === 'ArrowUp' && this.consoleHistory.length > 0) {
            if (this.historyIndex === -1) {
              this.consoleDraft = this.input.value;
              this.historyIndex = this.consoleHistory.length - 1;
            } else this.historyIndex = Math.max(0, this.historyIndex - 1);
          } else if (ke.key === 'ArrowDown' && this.historyIndex !== -1) {
            this.historyIndex++;
            if (this.historyIndex >= this.consoleHistory.length) this.historyIndex = -1;
          } else return;
          this.input.value = this.historyIndex === -1 ? this.consoleDraft : this.consoleHistory[this.historyIndex]!;
          this.input.setSelectionRange(this.input.value.length, this.input.value.length);
          resizeInput();
          return;
        }
      }
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
      this.historyIndex = -1;
      this.consoleDraft = this.input.value;
      if (!store.isConsoleMode(target)) this.bumpTyping();
    });
    this.pasteHandler = (e: ClipboardEvent) => this.handlePaste(e);
    document.addEventListener('paste', this.pasteHandler);
    this.copyHandler = (e: ClipboardEvent) => this.handleCopy(e);
    document.addEventListener('copy', this.copyHandler);
    this.keyHandler = (e: KeyboardEvent) => this.handleKey(e);
    window.addEventListener('keydown', this.keyHandler);

    this.fileChips = h('div', { class: 'msg-attachments', style: { marginTop: '0' } });
    this.replyChip = h('div', { class: 'reply-chip', style: { display: 'none' } });
    this.ttlPicker = h('div', { class: 'ttl-picker' });
    // Both exist before the first paint of the picker: it writes the note's text.
    this.ttlNote = h('span', { class: 'ttl-note' });
    this.ttlRow = h('div', { class: 'ttl-row' }, this.ttlNote, this.ttlPicker);
    // A group's messages never carry a timer (see `send`), so there is nothing
    // there to show or set — the row is not even in the layout for one.
    if (!isGroup) {
      this.renderTtlPicker();
    }

    // Icon and label both; the stylesheet shows the label where the chat pane
    // has room for it and the icon alone where it does not.
    const detailsBtn = h('button', {
      class: 'btn btn-ghost chat-details',
      title: isGroup ? tr('chat.members_2') : tr('chat.contact_profile'),
      onClick: () => (isGroup ? this.openGroupDetails() : this.openContactDetails()),
    },
      h('span', { class: 'chat-details-icon', 'aria-hidden': 'true' }, icon('info', 18)),
      h('span', { class: 'chat-details-label' }, isGroup ? tr('chat.members_2') : tr('chat.profile')),
    );
    const searchBtn = h('button', { class: 'icon-btn', title: tr('chat.search_this_chat'), onClick: () => this.openSearch() }, icon('search'));
    // «Кофеин»: экран не гаснет, чат не сменить, выход — по паролю профиля.
    // Ускорение живёт внутри него, отдельной кнопкой.
    const caffeineBtn = h('button', {
      class: 'icon-btn',
      title: tr('chat.caffeine_keep_the_screen_awake_and_stay_in'),
      onClick: () => void store.startCaffeine(target),
    }, icon('coffee'));
    const mediaBtn = h('button', { class: 'icon-btn', title: tr('chat.media_and_files'), onClick: () => this.openMedia() }, icon('image'));

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
    }, tr('app.disable')) as HTMLButtonElement;
    const renderAgentControls = (): void => {
      if (isGroup) { agentControls.replaceChildren(); return; }
      const c = store.contacts.get().find((x) => x.id === (target.id as number));
      if (!c?.agent_granted) {
        agentControls.replaceChildren();
        if (store.isConsoleMode(target)) store.toggleConsoleMode(target);
        return;
      }
      const consoleMode = store.isConsoleMode(target);
      this.modeBtn.textContent = consoleMode ? tr('chat.mode_chat') : tr('chat.mode_console');
      this.modeBtn.title = consoleMode ? tr('chat.show_ordinary_chat') : tr('chat.open_remote_console');
      agentControls.replaceChildren(this.modeBtn, this.agentOffBtn);
    };
    const renderInputMode = (): void => {
      const consoleMode = !isGroup && store.isConsoleMode(target);
      this.el?.classList.toggle('console', consoleMode);
      this.promptEl.textContent = consoleMode ? '$' : '>';
      this.input.placeholder = placeholderFor(consoleMode);
      this.ttlRow.classList.toggle('hidden', consoleMode);
      this.replyChip.classList.toggle('hidden', consoleMode);
    };
    renderAgentControls();

    const headerRight = isGroup
      ? h('div', { class: 'row chat-actions' }, caffeineBtn, searchBtn, mediaBtn, detailsBtn)
      : (() => {
          const c = store.contacts.get().find((x) => x.id === (target.id as number));
          return h('div', { class: 'row chat-actions' },
            caffeineBtn,
            (c?.trust ?? 0) !== 0 && h('span', {
              class: 'trust-badge trust-' + (c?.trust ?? 0),
              title: trustLabel(c?.trust ?? 0),
            }, trustLabel(c?.trust ?? 0)),
            agentControls,
            searchBtn,
            mediaBtn,
            detailsBtn,
            h('button', { class: 'icon-btn chat-more', title: tr('chat.more_actions'), onClick: (event: Event) => {
              const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
              ContextMenu.open(rect.left, rect.bottom, [{ label: tr('chat.clear_for_both'), danger: true,
                onClick: () => { void this.clearDialog(); } }]);
            } }, icon('more')),
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
      tr('chat.this_contact_has_been_unreachable_for_a_while'));
    this.sub(store.unreachable, (set) => {
      const hit = target.kind === 'contact' && set.has(target.id as number);
      this.unreachableNote.classList.toggle('hidden', !hit);
    }, true);

    this.historyPrompt = h('div', { class: 'chat-notice history-prompt hidden', role: 'group',
      'aria-label': tr('chat.save_history_prompt') },
      h('span', null, tr('chat.save_history_prompt')),
      h('button', { class: 'btn btn-ghost btn-sm', type: 'button', onClick: () => void this.saveHistory(false) }, tr('chat.history_dont_save')),
      h('button', { class: 'btn btn-sm', type: 'button', onClick: () => void this.saveHistory(true) }, tr('chat.history_save')));
    this.peerHistoryNote = h('div', { class: 'chat-notice peer-history-note hidden', role: 'status' });
    // Message expiry and the peer's explicit history choice are separate.
    this.peerNote = h('div', { class: 'chat-notice hidden' },
      icon('lock', 13), h('span', {}, tr('chat.peer_keeps')));
    this.sub(store.messages, () => this.updatePeerNote(), true);
    this.sub(store.contacts, () => this.updatePeerNote(), false);
    this.subs.push(Api.onChatPrefsChanged((prefs) => {
      this.prefsRevision++;
      this.updateHistoryPrompt(prefs);
    }));
    void this.loadPrefs();

    this.turboTitle = h('div', { class: 'caffeine-title' }, tr('chat.turbo'));
    this.turboHint = h('div', { class: 'caffeine-hint' });
    this.fastBtn = h('button', {
      class: 'btn btn-sm btn-fast',
      title: tr('chat.cocaine_one_fewer_hop_and_no_message_size'),
      onClick: () => void this.enterFast(),
    }, tr('chat.cocaine')) as HTMLButtonElement;
    this.caffeineBar = h('div', { class: 'caffeine-bar hidden' },
      icon('coffee', 18),
      h('div', { class: 'caffeine-text' }, this.turboTitle, this.turboHint),
      this.fastBtn,
      h('button', { class: 'btn btn-sm', onClick: () => void this.leaveCaffeine() }, tr('chat.exit')),
    );
    this.sub(store.caffeine, (on) => {
      const mine = !!on && sameTarget(on, target);
      this.caffeineBar.classList.toggle('hidden', !mine);
      caffeineBtn.classList.toggle('hidden', mine);
    }, true);
    this.sub(store.lane, () => this.paintTurboBar());
    this.sub(store.laneSwitching, () => this.paintTurboBar());

    this.el = h('div', { class: 'chat' },
      this.caffeineBar,
      h('div', { class: 'chat-header' },
        h('button', {
          class: 'chat-back icon-btn',
          title: tr('common.back'),
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
      this.historyPrompt,
      this.peerHistoryNote,
      this.peerNote,
      this.pinnedBanner.el,
      this.logWrap,
      this.composer = h('div', { class: 'chat-input' },
        this.replyChip,
        this.fileChips,
        h('div', { class: 'chat-input-row' },
          this.promptEl = h('div', { class: 'prompt' }, '>'),
          this.input,
          h('button', { class: 'icon-btn chat-attach', title: tr('chat.attach_file'), onClick: () => this.pickFiles() }, icon('attach')),
          h('button', { class: 'btn chat-send', title: tr('chat.send'), onClick: () => this.send() }, icon('send', 18), h('span', { class: 'chat-send-label' }, tr('chat.send')), h('span', { class: 'console-send-label' }, '↵')),
        ),
        h('div', { class: 'chat-input-meta' },
          h('span', { class: 'chat-e2e' }, icon('lock', 13), tr('about.end_to_end_encryption')),
          this.ttlRow,
        ),
      ),
    );

    // The connection status floats just above the composer instead of over
    // its controls; it reads the height from here.
    if (typeof ResizeObserver !== 'undefined') {
      this.composerObserver = new ResizeObserver(() => {
        document.documentElement.style.setProperty('--composer-height', `${this.composer.offsetHeight}px`);
      });
      this.composerObserver.observe(this.composer);
    }

    this.sub(store.messages, () => {
      const messages = store.messages.get().get(targetKey(this.target)) ?? [];
      for (const message of messages) {
        const signature = `${message.sent}:${message.delivered}`;
        const previous = this.fileMessageSignatures.get(message.id);
        this.fileMessageSignatures.set(message.id, signature);
        if (previous !== undefined && previous !== signature && message.deleted_at == null) this.invalidateFiles(message.id);
      }
      for (const message of messages) if (message.deleted_at != null) {
        // Notices now stay until confirmed, so history loaded later (opening
        // the chat, scrolling up, a locale switch) holds old deletions too:
        // only one that has just happened clears what is being typed.
        const fresh = Date.now() - message.deleted_at < 60_000;
        if (!this.deletedSeen.has(message.id) && !fresh) this.deletedSeen.add(message.id);
        if (!this.deletedSeen.has(message.id)) {
          this.deletedSeen.add(message.id);
          this.draftEpoch++;
          this.app.closeModals();
          this.input.value = '';
          if (!isTauri) forgetBrowserFiles(this.pending.map((file) => file.path));
          this.pending = [];
          this.consoleHistory.fill('');
          this.consoleHistory = [];
          this.consoleDraft = '';
          for (const picker of this.filePickers) { picker.value = ''; picker.remove(); }
          this.filePickers.clear();
          this.renderFileChips();
        }
        for (const [attachmentId, messageId] of this.imageMessages) if (messageId === message.id) this.releaseImage(attachmentId);
        this.attachmentCache.delete(message.id);
        this.fileCache.delete(message.id);
        this.fileVersions.set(message.id, (this.fileVersions.get(message.id) ?? 0) + 1);
        if (this.editingId === message.id) { this.editingId = null; this.editDraft = undefined; }
        if (this.replyTo?.id === message.id) this.cancelReply();
      }
      this.renderLog();
    });
    let lastProgress = store.fileProgress.get();
    this.sub(store.fileProgress, (progress) => {
      for (const [id, value] of progress) if (lastProgress.get(id) !== value) this.invalidateFiles(id);
      for (const id of lastProgress.keys()) if (!progress.has(id)) this.invalidateFiles(id);
      lastProgress = progress;
    }, false);
    this.sub(store.fileArrived, value => { if (value.mid) this.invalidateFiles(value.mid); }, false);
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
        if (this.disposed) return;
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
    if (this.store.isConsoleMode(this.target)) return kind != null && m.id > this.consoleClearedThrough;
    if (kind == null) return true;
    if (kind === CONSOLE_COMMAND || kind === CONSOLE_OUTPUT) {
      return !this.holdsPeerConsole();
    }
    return true;
  }

  private holdsPeerConsole(): boolean {
    if (this.target.kind !== 'contact') return false;
    return !!this.store.contacts.get().find((c) => c.id === (this.target.id as number))?.agent_granted;
  }

  /** Locale changes rebuild labels without losing an unsent draft or reply. */
  captureDraft() {
    return {
      body: this.input.value, pending: [...this.pending], replyTo: this.replyTo,
      ttlSecs: this.ttlSecs, history: [...this.consoleHistory], historyIndex: this.historyIndex,
      consoleDraft: this.consoleDraft, consoleClearedThrough: this.consoleClearedThrough,
      editingId: this.editingId,
      editBody: this.el.querySelector<HTMLTextAreaElement>('.edit-inline-textarea')?.value ?? this.editDraft,
      scrollTop: this.log.scrollTop, stickyBottom: this.stickyBottom,
    };
  }

  restoreDraft(draft: ReturnType<ChatView['captureDraft']>): void {
    this.input.value = draft.body;
    this.input.dispatchEvent(new Event('input'));
    this.pending = draft.pending;
    this.replyTo = draft.replyTo;
    this.ttlSecs = draft.ttlSecs;
    this.consoleHistory = draft.history;
    this.historyIndex = draft.historyIndex;
    this.consoleDraft = draft.consoleDraft;
    this.consoleClearedThrough = draft.consoleClearedThrough;
    this.editingId = draft.editingId;
    this.editDraft = draft.editBody;
    this.renderedKey = '';
    this.renderLog();
    this.renderFileChips();
    this.renderReplyChip();
    if (this.target.kind !== 'group') this.renderTtlPicker();
    this.stickyBottom = draft.stickyBottom;
    requestAnimationFrame(() => { this.log.scrollTop = draft.stickyBottom ? this.log.scrollHeight : draft.scrollTop; });
  }

  destroy(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.draftEpoch++;
    this.composerObserver?.disconnect();
    document.documentElement.style.removeProperty('--composer-height');
    ContextMenu.close();
    if (this.consoleRunningTimer != null) window.clearInterval(this.consoleRunningTimer);
    document.removeEventListener('paste', this.pasteHandler);
    document.removeEventListener('copy', this.copyHandler);
    window.removeEventListener('keydown', this.keyHandler);
    if (this.typingStopTimer != null) window.clearTimeout(this.typingStopTimer);
    if (this.typingActive) {
      const cid = this.target.kind === 'contact' ? (this.target.id as number) : null;
      const gid = this.target.kind === 'group' ? (this.target.id as string) : null;
      Api.sendTyping(cid, gid, false).catch(() => {});
      this.typingActive = false;
    }
    this.pinnedBanner.destroy();
    for (const id of [...this.imageMessages.keys()]) this.releaseImage(id);
    for (const url of this.ownedBlobUrls) URL.revokeObjectURL(url);
    this.ownedBlobUrls.clear();
    this.imageDataCache.clear();
    this.imageMessages.clear();
    this.imageClosers.clear();
    this.deletedSeen.clear();
    this.attachmentCache.clear();
    this.fileCache.clear(); this.fileVersions.clear(); this.fileMessageSignatures.clear(); this.fileQueue.clear();
    if (this.fileRetryTimer !== null) window.clearTimeout(this.fileRetryTimer);
    this.renderedSignatures.clear();
    this.renderedIds = [];
    this.renderedKey = '';
    this.replyTo = null;
    this.editingId = null;
    this.input.value = '';
    this.consoleHistory.fill('');
    this.consoleHistory = [];
    this.consoleDraft = '';
    this.consoleSending = 0;
    this.prependCompensation = null;
    if (!isTauri) forgetBrowserFiles(this.pending.map((file) => file.path));
    this.pending = [];
    for (const picker of this.filePickers) { picker.value = ''; picker.remove(); }
    this.filePickers.clear();
    this.el.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>('input, textarea').forEach((input) => { input.value = ''; });
    this.el.querySelectorAll<HTMLMediaElement>('audio, video').forEach((media) => { media.pause(); media.removeAttribute('src'); media.load(); });
    this.el.querySelectorAll('img').forEach((image) => image.removeAttribute('src'));
    this.el.replaceChildren();
    super.destroy();
  }

  /** Lookup by id over this chat's current list. The list is replaced, never
   *  mutated, on every change, so the map is rebuilt only when it changes:
   *  rendering asks once per row, and a linear search each time was O(n²). */
  private messageIndex: { list: readonly Message[]; byId: Map<number, Message> } | null = null;
  private messageById(messageId: number): Message | undefined {
    const list = this.store.messages.get().get(targetKey(this.target)) ?? [];
    if (this.messageIndex?.list !== list) this.messageIndex = { list, byId: new Map(list.map(message => [message.id, message])) };
    return this.messageIndex.byId.get(messageId);
  }

  private isMessageLive(messageId: number): boolean {
    if (this.disposed || !sameTarget(this.store.selectedChat.get(), this.target)) return false;
    const message = this.messageById(messageId);
    return message !== undefined && message.deleted_at == null;
  }

  private releaseImage(attachmentId: number): void {
    this.imageClosers.get(attachmentId)?.();
    this.imageClosers.delete(attachmentId);
    const url = this.imageDataCache.get(attachmentId);
    if (url && this.ownedBlobUrls.delete(url)) URL.revokeObjectURL(url);
    this.imageDataCache.delete(attachmentId);
    this.imageMessages.delete(attachmentId);
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
        ?? tr('chat.someone');
    }
    this.typingHeader.textContent = tr('chat.is_typing', { p0: (who) });
    this.typingHeader.style.display = '';
  }

  /**
   * On Android the file dialog hands back content:// URIs, which the core
   * cannot open as paths, so nothing could be attached from a phone. There the
   * webview's own chooser is used instead: the files are read here and copied
   * into the app's cache, as a drop or a paste already is.
   */
  private pickFilesAndroid(): void {
    if (this.disposed) return;
    const epoch = this.draftEpoch;
    const input = h('input', { type: 'file', multiple: true, style: 'display:none' }) as HTMLInputElement;
    this.filePickers.add(input);
    input.addEventListener('change', async () => {
      try {
        for (const f of Array.from(input.files ?? [])) {
          const path = await pasteFileToTempPath(f);
          if (this.disposed || epoch !== this.draftEpoch) return;
          this.pending.push({ name: f.name, path, size: f.size });
        }
        this.renderFileChips();
      } catch (e) {
        this.store.showToast(tr('chat.attach_failed', { error: errorText(e) }), true);
      } finally {
        input.value = '';
        this.filePickers.delete(input);
        input.remove();
      }
    });
    document.body.appendChild(input);
    input.click();
  }

  private async pickFiles(): Promise<void> {
    if (this.disposed) return;
    const epoch = this.draftEpoch;
    if (!isTauri || /Android/.test(navigator.userAgent)) {
      this.pickFilesAndroid();
      return;
    }
    try {
      const sel = await open({ multiple: true });
      if (this.disposed || epoch !== this.draftEpoch) return;
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
      this.store.showToast(tr('chat.pick_failed_2') + errorText(e), true);
    }
  }

  private async handlePaste(e: ClipboardEvent): Promise<void> {
    if (this.disposed) return;
    const epoch = this.draftEpoch;
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
      try { if (isTauri) path = await Api.pasteClipboardImage(); } catch { path = null; }
      if (this.disposed || epoch !== this.draftEpoch) return;
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
        if (this.disposed || epoch !== this.draftEpoch) return;
        this.pending.push({ name: f.name, path, size: f.size });
      } catch (err) {
        this.store.showToast(tr('chat.paste_failed') + String(err), true);
      }
    }
    this.renderFileChips();
  }

  /** A live message of this direct chat, by its row in the log. */
  private liveDirectMessage(el: Element | null | undefined): Message | undefined {
    if (this.target.kind !== 'contact' || !el) return undefined;
    const id = Number(el.getAttribute('data-mid'));
    if (!Number.isFinite(id) || id <= 0) return undefined;
    const message = this.messageById(id);
    return message?.deleted_at == null ? message : undefined;
  }

  /** Copying the contact's text tells them so (their client shows it under
   *  their message). Our own messages need no report. */
  private handleCopy(_e: ClipboardEvent): void {
    if (this.disposed || this.target.kind !== 'contact') return;
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || !sel.toString().trim()) return;
    for (const el of this.log.querySelectorAll('.msg[data-mid]')) {
      if (!sel.containsNode(el, true)) continue;
      const message = this.liveDirectMessage(el);
      if (message && !message.outgoing) void Api.reportPeerEvent(message.id, PEER_EVENT.copied).catch(() => {});
    }
  }

  /** A screenshot shortcut that reaches the page. Many systems take the
   *  picture before the page sees any key, so this proves nothing either way;
   *  the contact is told a shortcut was pressed, not that a picture exists. */
  private handleKey(e: KeyboardEvent): void {
    if (this.disposed || this.target.kind !== 'contact') return;
    const shortcut = e.key === 'PrintScreen'
      || (e.metaKey && e.shiftKey && ['3', '4', '5', 's', 'S'].includes(e.key));
    if (!shortcut) return;
    const view = this.log.getBoundingClientRect();
    const visible = [...this.log.querySelectorAll('.msg[data-mid]')].filter(el => {
      const box = el.getBoundingClientRect();
      return box.bottom > view.top && box.top < view.bottom;
    });
    const message = this.liveDirectMessage(visible[visible.length - 1]);
    if (message) void Api.reportPeerEvent(message.id, PEER_EVENT.screenshot).catch(() => {});
  }

  private renderedKey: string = '';
  private renderedIds: number[] = [];
  private renderedSignatures: Map<number, string> = new Map();
  private deletionGroups = new Map<number, Message[]>();

  private msgSignature(m: Message): string {
    const pinned = this.isPinned(m) ? '1' : '0';
    const btns = m.buttons ? JSON.stringify(m.buttons) : '';
    const consoleFrame = m.console ? JSON.stringify(m.console) : '';
    const peerEvents = m.peer_events?.length ? JSON.stringify(m.peer_events) : '';
    const deletion = m.deleted_at != null ? JSON.stringify((this.deletionGroups.get(m.id) ?? [m]).map(row => [row.id, row.deletion_notice])) : '';
    return `${m.body}|${m.sent ? 1 : 0}${m.delivered ? 1 : 0}|${pinned}|${btns}|${consoleFrame}|${m.dm_seq}|${m.deleted_at}|${deletion}|${this.editingId === m.id ? 'e' : ''}|${peerEvents}`;
  }

  private renderLog(): void {
    if (this.disposed) return;
    const consoleMode = this.store.isConsoleMode(this.target);
    const key = targetKey(this.target) + (consoleMode ? ':console' : ':chat');
    const all = this.store.messages.get().get(targetKey(this.target)) ?? [];
    const windowIds = new Set(all.map(row => row.id));
    for (const id of this.fileCache.keys()) if (!windowIds.has(id)) { this.fileCache.delete(id); this.attachmentCache.delete(id); this.fileMessageSignatures.delete(id); this.drawnFiles.delete(id); }
    const rows = deletionRuns(all.filter((m) => this.showsInLog(m)));
    this.deletionGroups = new Map(rows.filter(row => row.message.deleted_at != null).map(row => [row.message.id, row.members]));
    const list = rows.map(row => row.message);
    this.log.querySelector('.console-running')?.remove();
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
        if (!consoleMode && d !== lastDate) {
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
        if (!consoleMode && d !== lastDate) {
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
    this.renderConsoleRunning(all, consoleMode);
    this.updateJumpBtn();
  }

  private renderConsoleRunning(messages: Message[], consoleMode: boolean): void {
    const latest = [...messages].reverse().find((m) =>
      (m.outgoing && m.console?.kind === CONSOLE_COMMAND)
      || (!m.outgoing && m.console?.kind === CONSOLE_OUTPUT)
      || m.console?.kind === CONSOLE_OFF || m.console?.kind === CONSOLE_REVOKE);
    const pendingCommand = latest?.outgoing && latest.console?.kind === CONSOLE_COMMAND;
    if (consoleMode && (this.consoleSending > 0 || pendingCommand)) {
      const since = pendingCommand ? latest.sent_at : this.consoleSendingSince;
      const label = h('span');
      const update = (): void => { label.textContent = tr('chat.running_s', { p0: (Math.max(0, Math.floor((Date.now() - since) / 1000))) }); };
      update();
      this.log.appendChild(h('div', { class: 'console-running', role: 'status' },
        h('span', { class: 'console-cursor', 'aria-hidden': 'true' }, '_'), label));
      if (this.consoleRunningTimer != null) window.clearInterval(this.consoleRunningTimer);
      this.consoleRunningTimer = window.setInterval(update, 1000);
      if (this.stickyBottom) this.scrollBottom();
    } else if (this.consoleRunningTimer != null) {
      window.clearInterval(this.consoleRunningTimer);
      this.consoleRunningTimer = null;
    }
  }

  private maybeLoadMore(): void {
    if (this.loadingMore || this.noMoreOlder) return;
    if (this.log.scrollTop > ChatView.LOAD_MORE_THRESHOLD) return;
    if (!(this.store.messages.get().get(targetKey(this.target))?.length)) return;
    this.loadingMore = true;
    this.prependCompensation = { prevHeight: this.log.scrollHeight, prevTop: this.log.scrollTop };
    this.store.loadMoreMessages(this.target).then((more) => {
      if (this.disposed) return;
      if (!more) {
        this.noMoreOlder = true;
        this.prependCompensation = null;
      }
    }).catch(() => {
      this.prependCompensation = null;
    }).finally(() => {
      this.loadingMore = false;
      if (!this.disposed && !this.noMoreOlder && this.renderedIds.length === 0) window.setTimeout(() => this.maybeLoadMore(), 0);
    });
  }

  private async scrollToMessage(messageId: number): Promise<void> {
    if (this.disposed) return;
    const present = this.renderedIds.includes(messageId);
    if (!present) {
      try {
        const ok = await this.store.loadUntilMessage(this.target, messageId);
        if (this.disposed) return;
        if (!ok) {
          this.store.showToast(tr('chat.message_not_found_in_this_chat'), true);
          return;
        }
      } catch {
        this.store.showToast(tr('chat.load_failed'), true);
        return;
      }
    }
    this.stickyBottom = false;
    let frames = 0;
    const tryScroll = (): void => {
      if (this.disposed) return;
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
    if (!m.sender_sign_pk) return tr('auth.unknown');
    const members = this.store.groupMembers.get().get(this.target.id as string) ?? [];
    const member = members.find((mm) => mm.sign_pk === m.sender_sign_pk);
    if (member) return member.name;
    const contact = this.store.contacts.get().find((c) => c.sign_pk === m.sender_sign_pk);
    return contact?.name ?? tr('auth.unknown');
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
    if (m.deleted_at != null) {
      return this.renderDeletionNotice(this.deletionGroups.get(m.id) ?? [m]);
    }
    if (m.console) return this.renderConsoleMessage(m);
    if (this.editingId === m.id) return this.renderEditing(m);
    const meta = m.outgoing
      ? (m.delivered ? '✓✓' : (m.sent ? '✓' : '🕓'))
      : '';
    const metaTitle = m.delivered ? tr('chat.delivered') : m.sent ? tr('chat.sent') : tr('chat.queued');
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
            m.dm_seq != null && h('span', { class: 'msg-number', title: tr('chat.local_message_number') }, `#${m.dm_seq} `),
            pinned && h('span', { class: 'msg-pin-indicator', title: tr('chat.pinned') }, '📌'),
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
      () => { void this.deleteForBoth(m); },
    ));
    if (m.peer_events?.length && this.target.kind === 'contact') wrap.appendChild(this.renderPeerEvents(m.peer_events));
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
                this.store.showToast(tr('chat.button_failed') + errorText(e), true);
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

  /** What the contact reported doing with this message on their device. */
  private renderPeerEvents(events: PeerEvent[]): HTMLElement {
    const key = { 5: 'chat.peer_copied', 6: 'chat.peer_saved', 7: 'chat.peer_screenshot_keys' } as const;
    const today = fmtDate(Date.now());
    return h('div', { class: 'msg-peer-events', role: 'note' }, ...events.map(event => {
      const at = fmtDate(event.at) === today ? fmtTime(event.at) : `${fmtDate(event.at)} ${fmtTime(event.at)}`;
      const times = event.count > 1 ? ` ×${event.count}` : '';
      return h('div', { class: 'msg-peer-event' }, `${tr(key[event.kind])}${times} · ${at}`);
    }));
  }

  private renderDeletionNotice(messages: Message[]): HTMLElement {
    const first = messages[0]!;
    // The date only when it is not today: a grouped notice has up to four times.
    const today = fmtDate(Date.now());
    const format = (at: number): string => fmtDate(at) === today ? fmtTime(at) : `${fmtDate(at)} ${fmtTime(at)}`;
    const from = Math.min(...messages.map(row => row.sent_at));
    const to = Math.max(...messages.map(row => row.sent_at));
    const removed = messages.map(row => row.deletion_notice?.deleted_at ?? row.deleted_at!);
    const removedFrom = Math.min(...removed), removedTo = Math.max(...removed);
    const at = removedFrom === removedTo ? format(removedFrom) : `${format(removedFrom)} — ${format(removedTo)}`;
    const theirs = first.deletion_notice?.can_ack === true;
    const by = tr(theirs ? 'chat.deleted_by_contact' : 'chat.deleted_by_you');
    const text = messages.length === 1
      ? tr('chat.deletion_notice_one', { time: format(from), by, at })
      : tr('chat.deletion_notice_many', { count: messages.length, from: format(from), to: format(to), by, at });
    const acknowledge = theirs && this.target.kind === 'contact';
    const button = h('button', { class: 'deletion-ok', type: 'button', title: tr('chat.deletion_ack_hint'), 'aria-label': tr('chat.deletion_ack_hint') }, tr('chat.deletion_ok')) as HTMLButtonElement;
    if (acknowledge) button.addEventListener('click', () => {
      if (button.disabled) return;
      button.disabled = true;
      const target = this.target;
      if (target.kind !== 'contact') return;
      // The core takes at most 500 ids per call; a cleared chat can hold more.
      const ids = messages.map(row => row.id);
      const batches: number[][] = [];
      for (let i = 0; i < ids.length; i += 500) batches.push(ids.slice(i, i + 500));
      void batches.reduce((done, batch) => done.then(() => Api.acknowledgeDeletionNotices(target.id, batch)), Promise.resolve())
        .then(() => this.store.loadMessages(target))
        .catch(error => { if (!this.disposed) this.store.showToast(errorText(error), true); })
        .finally(() => { if (!this.disposed) button.disabled = false; });
    });
    return h('div', { class: 'msg-deleted', 'data-mid': String(first.id), role: 'note' },
      acknowledge && button, h('span', null, theirs ? text : `${text} ${tr('chat.deletion_waiting_ok')}`));
  }

  private async deleteForBoth(message: Message): Promise<void> {
    if (this.target.kind !== 'contact' || message.deleted_at != null) return;
    if (!await this.app.confirm(tr('chat.delete_for_both'), tr('chat.delete_for_both_confirm'), true)) return;
    try {
      await Api.deleteMessage(message.id);
      await this.store.loadMessages(this.target);
    } catch (error) {
      this.store.showToast(errorText(error), true);
    }
  }

  private async clearDialog(): Promise<void> {
    if (this.target.kind !== 'contact') return;
    if (!await this.app.confirm(tr('chat.clear_for_both'), tr('chat.clear_for_both_confirm'), true)) return;
    try {
      await Api.clearDmChat(this.target.id as number);
      await this.store.loadMessages(this.target);
    } catch (error) { this.store.showToast(errorText(error), true); }
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
      const text = frame.kind === CONSOLE_GRANT ? tr('chat.console_opened')
        : frame.kind === CONSOLE_OFF ? tr('chat.the_master_disabled_agent_mode')
        : tr('chat.console_closed');
      return h('div', { class: 'divider-text console-divider', 'data-mid': String(m.id) }, text);
    }
    const wrap = h('div', { class: 'console-msg', 'data-mid': String(m.id) });
    if (frame.kind !== CONSOLE_COMMAND && frame.kind !== CONSOLE_OUTPUT) {
      wrap.appendChild(h('div', { class: 'divider-text console-divider' }, tr('chat.console', { p0: (frame.kind) })));
      return wrap;
    }
    if (frame.kind === CONSOLE_COMMAND) {
      wrap.classList.add('cmd');
      wrap.appendChild(h('span', { class: 'console-prompt' }, '$'));
      wrap.appendChild(h('div', { class: 'console-body' }, m.body));
    } else {
      wrap.classList.add('out');
      wrap.appendChild(h('div', { class: 'console-body' }, renderAnsi(m.body || tr('chat.no_output'))));
      const bits: string[] = [];
      if (m.outgoing) bits.push(frame.exit_code == null ? tr('chat.no_exit_code') : tr('chat.exit_2', { p0: (frame.exit_code) }));
      if (frame.duration_ms != null) bits.push(`${(frame.duration_ms / 1000).toFixed(2)}s`);
      if (frame.truncated) bits.push(tr('chat.truncated'));
      if (bits.length > 0) wrap.appendChild(h('div', { class: 'console-meta' }, bits.join(' · ')));
    }
    this.loadAttachmentsFor(m.id, wrap);
    return wrap;
  }

  private async disableRemoteAgent(): Promise<void> {
    try {
      await this.store.sendAgentOff(this.target);
      this.store.showToast(tr('chat.agent_mode_off_requested'));
    } catch (e) {
      this.store.showToast(tr('chat.agent_off_failed') + errorText(e), true);
    }
  }

  private renderEditing(m: Message): HTMLElement {
    const wrap = h('div', {
      class: 'msg out editing',
      'data-mid': String(m.id),
    });
    const editor = new EditInline(
      this.editDraft ?? m.body,
      (newBody) => {
        this.store.editMessage(this.target, m.id, newBody)
          .then(() => { this.editingId = null; this.editDraft = undefined; this.renderLog(); })
          .catch((e) => this.store.showToast(tr('chat.edit_failed') + e, true));
      },
      () => { this.editingId = null; this.editDraft = undefined; this.renderLog(); },
    );
    // A re-render (a receipt, a locale switch, history arriving late) must
    // not reset what the user has typed so far.
    editor.el.addEventListener('input', (e) => {
      if ((e.target as HTMLElement).classList.contains('edit-inline-textarea')) this.editDraft = (e.target as HTMLTextAreaElement).value;
    });
    wrap.appendChild(editor.el);
    return wrap;
  }

  private startEdit(m: Message): void {
    if (!m.outgoing || !this.isMessageLive(m.id)) return;
    this.editingId = m.id;
    this.editDraft = undefined;
    this.renderLog();
  }

  private startReply(m: Message): void {
    if (!this.isMessageLive(m.id)) return;
    this.replyTo = m;
    this.renderReplyChip();
    this.input.focus();
  }

  private cancelReply(): void {
    this.replyTo = null;
    this.renderReplyChip();
  }

  private renderReplyChip(): void {
    if (this.disposed) return;
    this.replyChip.replaceChildren();
    if (!this.replyTo) {
      this.replyChip.style.display = 'none';
      return;
    }
    const m = this.replyTo;
    const senderLabel = m.outgoing
      ? tr('chat.you')
      : (this.senderNameFor(m) || tr('auth.unknown'));
    const preview = m.body
      ? (m.body.length > 80 ? m.body.slice(0, 80) + '…' : m.body)
      : tr('chat.attachment');
    this.replyChip.style.display = '';
    this.replyChip.appendChild(h('div', { class: 'reply-chip-bar' }));
    this.replyChip.appendChild(h('div', { class: 'reply-chip-content' },
      h('div', { class: 'reply-chip-label' }, tr('chat.replying_to', { p0: (senderLabel) })),
      h('div', { class: 'reply-chip-body' }, preview),
    ));
    this.replyChip.appendChild(h('button', {
      class: 'reply-chip-close',
      title: tr('chat.cancel_reply'),
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
          h('div', { class: 'msg-reply-body' }, tr('chat.message_unavailable')),
        ),
      );
    }
    const senderLabel = orig.outgoing
      ? tr('chat.you')
      : (this.senderNameFor(orig) || tr('auth.unknown'));
    const preview = orig.deleted_at != null ? tr('chat.deleted_number', { number: orig.dm_seq ?? '?' }) : orig.body
      ? (orig.body.length > 100 ? orig.body.slice(0, 100) + '…' : orig.body)
      : tr('chat.attachment');
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

  private invalidateFiles(id: number): void {
    if (!this.isMessageLive(id)) return;
    this.fileVersions.set(id, (this.fileVersions.get(id) ?? 0) + 1);
    this.fileCache.delete(id); this.attachmentCache.delete(id);
    this.queueFiles(id);
  }

  private queueFiles(id: number): void {
    this.fileQueue.add(id);
    if (this.fileBatchPending || this.disposed) return;
    this.fileBatchPending = true;
    // Rendering the whole history coalesces into one bounded IPC request.
    queueMicrotask(() => { void this.fetchFiles(); });
  }

  private async fetchFiles(): Promise<void> {
    // A message deleted or scrolled out of this chat while queued leaves the
    // queue too; left behind, it re-armed this batch forever (a busy loop).
    for (const id of [...this.fileQueue]) if (!this.isMessageLive(id)) this.fileQueue.delete(id);
    const ids = [...this.fileQueue].slice(0, 500);
    ids.forEach(id => this.fileQueue.delete(id));
    const versions = new Map(ids.map(id => [id, this.fileVersions.get(id) ?? 0]));
    try {
      if (!ids.length || this.disposed) return;
      const result = await Api.messageFiles(ids);
      if (this.disposed) return;
      let changed = false;
      for (const id of ids) {
        if (!this.isMessageLive(id)) continue;
        if ((this.fileVersions.get(id) ?? 0) !== versions.get(id)) { this.fileQueue.add(id); continue; }
        // No entry means no files: cache that, or rendering asks again forever.
        const files = result[id] ?? { attachments: [], progress: [] };
        this.fileCache.set(id, files); this.attachmentCache.set(id, files.attachments);
        // Re-render a row only when what it shows changes.
        if (JSON.stringify(files) !== (this.drawnFiles.get(id) ?? NO_FILES)) { this.renderedSignatures.delete(id); changed = true; }
      }
      if (changed) this.renderLog();
    } catch {
      // Keep unknown transfers pending, never fabricate an empty completed file.
      if (!this.disposed && this.fileRetryTimer === null) this.fileRetryTimer = window.setTimeout(() => {
        this.fileRetryTimer = null;
        ids.forEach(id => { if (this.isMessageLive(id)) this.queueFiles(id); });
      }, 3000);
    } finally {
      this.fileBatchPending = false;
      if (!this.disposed && this.fileQueue.size && this.fileRetryTimer === null) this.queueFiles(this.fileQueue.values().next().value!);
    }
  }

  private loadAttachmentsFor(msgId: number, container: HTMLElement): void {
    if (!this.isMessageLive(msgId)) return;
    const message = this.messageById(msgId);
    const files = this.fileCache.get(msgId);
    this.drawnFiles.set(msgId, files ? JSON.stringify(files) : NO_FILES);
    if (!files) { this.queueFiles(msgId); return; }
    const { attachments } = files;
    // A finished file already has its card below: a «complete» row would
    // repeat its name and size.
    const progress = files.progress.filter(file => file.status !== 'complete' || !attachments.some(card => card.name === file.name));
    if (!attachments.length && !progress.length) return;
    container.classList.toggle('attachment-only', !message?.body.trim());
    const sending = progress.some(file => file.status === 'sending') || (!!message?.outgoing && !message.delivered && attachments.length > 0 && progress.length === 0);
    container.classList.toggle('file-sending', sending);
    // Metadata ACK never substitutes for file-part ACK in the displayed status.
    if (progress.some(file => !file.incoming && file.status !== 'complete')) container.querySelector('.msg-meta')?.remove();
    const labelFor = (status: import('./api').FileProgress['status'], incoming: boolean): string => tr(status === 'waiting' ? 'files.waiting_contact' : status === 'sending' ? 'files.sending' : status === 'receiving' ? 'files.receiving' : status === 'failed' ? 'files.failed' : status === 'cancelled' ? 'files.cancelled' : incoming ? 'files.complete' : 'files.delivered');
    if (progress.length) {
      const box = h('div', { class: 'msg-files-moving', 'data-files-mid': String(msgId) });
      for (const file of progress) {
        const pct = file.total ? Math.min(100, Math.floor(100 * file.done / file.total)) : 0;
        const moving = file.status === 'sending' || file.status === 'receiving';
        const row = h('div', { class: 'file-progress ' + file.status },
          h('div', { class: 'file-progress-name' }, file.name, h('span', { class: 'file-size' }, humanSize(file.size))),
          moving && h('div', { class: 'file-progress-track', role: 'progressbar', 'aria-label': file.name, 'aria-valuemin': '0', 'aria-valuemax': '100', 'aria-valuenow': String(pct) }, h('div', { class: 'file-progress-bar', style: `width:${pct}%` })),
          h('div', { class: 'file-progress-label' }, labelFor(file.status, file.incoming), moving ? ` · ${pct}%` : ''),
        );
        if (!file.incoming && moving && file.cancelable !== false) row.appendChild(h('button', { class: 'file-progress-cancel', type: 'button', onClick: async () => {
          try { await Api.cancelFiles(msgId); this.invalidateFiles(msgId); }
          catch (error) { this.store.showToast(tr('chat.cancel_failed') + errorText(error), true); }
        } }, tr('chat.cancel')));
        box.appendChild(row);
      }
      container.appendChild(box);
    } else if (sending) container.appendChild(h('div', { class: 'file-progress-label' }, tr('files.sending')));
    if (!attachments.length) return;
    const bar = h('div', { class: 'msg-attachments' });
    for (const attachment of attachments) {
      const image = isImageName(attachment.name) && (isTauri || attachment.size <= 32 * 1024 * 1024);
      bar.appendChild(h('button', {
        type: 'button', class: 'attachment file-card' + (image ? ' image-stub' : ''),
        onClick: () => image ? this.openImageLazy(attachment.id, attachment.name, msgId) : this.downloadAttachment(attachment.id, attachment.name),
      }, icon('attach', 20), h('span', { class: 'file-card-copy' }, h('span', { class: 'file-card-name' }, attachment.name), h('span', { class: 'file-size' }, humanSize(attachment.size)))));
    }
    container.appendChild(bar);
  }

  private async openImageLazy(attId: number, name: string, messageId: number): Promise<void> {
    if (!this.isMessageLive(messageId)) return;
    let url = this.imageDataCache.get(attId);
    if (!url) {
      try {
        const b64 = await Api.loadAttachment(attId);
        if (!this.isMessageLive(messageId)) return;
        const bytes = Uint8Array.from(atob(b64), (character) => character.charCodeAt(0));
        const blob = new Blob([bytes], { type: mimeFromName(name) });
        bytes.fill(0);
        url = URL.createObjectURL(blob);
        this.ownedBlobUrls.add(url);
        this.imageDataCache.set(attId, url);
        this.imageMessages.set(attId, messageId);
      } catch (e) {
        if (!this.disposed) this.store.showToast(tr('chat.load_failed_2') + errorText(e), true);
        return;
      }
    }
    this.openImage(url, name, attId);
  }

  private openImage(src: string, name: string, attachmentId: number): void {
    this.imageClosers.get(attachmentId)?.();
    let cleanup = (): void => {};
    const dismiss = this.app.openModal((close) => {
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
      cleanup = (): void => {
        document.removeEventListener('mousemove', onMove);
        document.removeEventListener('mouseup', onUp);
        img.removeAttribute('src');
        this.imageClosers.delete(attachmentId);
      };
      const resetBtn = h('button', {
        class: 'btn btn-ghost',
        style: { marginLeft: '12px' },
        onClick: () => { scale = 1; tx = 0; ty = 0; apply(); },
      }, tr('chat.reset'));
      return h('div', {
        class: 'modal',
        style: { width: 'auto', maxWidth: '95vw', maxHeight: '95vh', padding: '0' },
      },
        h('div', { class: 'modal-header' },
          h('div', { class: 'modal-title' }, name),
          h('div', { class: 'hint', style: { marginLeft: '12px', fontSize: '11px' } },
            tr('chat.wheel_zoom_drag_pan')),
          resetBtn,
          h('div', { class: 'grow' }),
          h('button', { class: 'icon-btn', onClick: close }, 'x'),
        ),
        stage,
      );
    }, () => cleanup());
    this.imageClosers.set(attachmentId, dismiss);
  }

  private async downloadAttachment(id: number, name: string): Promise<void> {
    // Saving the contact's file out of the app tells them so.
    const reportSaved = (): void => {
      const msgId = this.imageMessages.get(id) || [...this.attachmentCache.entries()].find(([_, atts]) => atts.some(a => a.id === id))?.[0];
      const message = msgId ? this.liveDirectMessage(this.log.querySelector(`.msg[data-mid="${msgId}"]`)) : undefined;
      if (message && !message.outgoing) void Api.reportPeerEvent(message.id, PEER_EVENT.saved).catch(() => {});
    };
    try {
      if (!isTauri) {
        if (this.disposed) return;
        await saveBrowserAttachment(id);
        reportSaved();
        return;
      }
      const dest = await save({ defaultPath: name });
      if (!dest || this.disposed) return;
      await Api.saveAttachment(id, dest);
      this.store.showToast(tr('chat.saved'));
      reportSaved();
    } catch (e) {
      this.store.showToast(tr('chat.save_failed', { error: errorText(e) }), true);
    }
  }

  private renderFileChips(): void {
    if (this.disposed) return;
    this.fileChips.replaceChildren();
    this.pending.forEach((f, i) => {
      this.fileChips.appendChild(h('div', {
        class: 'attachment',
        onClick: () => { if (!isTauri) forgetBrowserFiles([f.path]); this.pending.splice(i, 1); this.renderFileChips(); },
      }, `◆ ${f.name} [x]`));
    });
  }

  /** Show only an explicitly reported peer preference as a history choice. */
  private updatePeerNote(): void {
    if (this.target.kind !== 'contact') {
      this.peerNote.classList.add('hidden'); this.peerHistoryNote.classList.add('hidden'); return;
    }
    // No client sends this preference yet, so «unknown» would sit in every
    // chat for good: say nothing until the contact has actually said something.
    const peerChoice = this.store.contacts.get().find((contact) => contact.id === this.target.id)?.save_history;
    this.peerHistoryNote.textContent = peerChoice === true ? tr('chat.peer_saves_history')
      : peerChoice === false ? tr('chat.peer_doesnt_save_history') : '';
    this.peerHistoryNote.classList.toggle('hidden', peerChoice == null);
    const list = this.store.messages.get().get(targetKey(this.target)) ?? [];
    const newest = [...list].reverse().find((m) => !m.outgoing && m.console == null && m.group_id == null && m.deleted_at == null);
    this.peerNote.classList.toggle('hidden', newest == null || newest.expires_at != null);
  }

  private async saveHistory(save: boolean): Promise<void> {
    if (this.historySaving || this.disposed) return;
    this.historySaving = true;
    const buttons = this.historyPrompt.querySelectorAll('button');
    buttons.forEach((button) => { button.disabled = true; });
    try {
      await Api.updateChatPrefs({ saveHistory: { [targetKey(this.target)]: save } });
      if (!this.disposed) this.historyPrompt.classList.add('hidden');
    } catch {
      if (!this.disposed) this.store.showToast(tr('chat.history_save_failed'), true);
    } finally {
      this.historySaving = false;
      buttons.forEach((button) => { button.disabled = false; });
    }
  }

  private updateHistoryPrompt(prefs: ChatPrefs): void {
    if (this.disposed) return;
    this.historyPrompt.classList.toggle('hidden', !prefs.clearOnLock
      || typeof prefs.saveHistory?.[targetKey(this.target)] === 'boolean');
  }

  private renderTtlPicker(): void {
    this.ttlPicker.replaceChildren();
    for (const [label, secs] of TTL_OPTIONS) {
      this.ttlPicker.appendChild(h('div', {
        class: 'ttl-chip' + (this.ttlSecs === secs ? ' active' : ''),
        title: tr('chat.ttl_title'),
        onClick: () => this.setTtl(secs),
      }, label));
    }
    this.ttlNote.textContent = ttlSpoken(this.ttlSecs);
  }

  /** Change the timer and remember it.
   *
   * The pick is per profile rather than per chat: a person who wants their
   * messages to disappear means it of the people they talk to, and a timer that
   * reset on every new conversation would be a timer nobody could rely on. It
   * still only ever applies to what is sent from now on. */
  private setTtl(secs: number | null): void {
    this.prefsRevision++;
    this.ttlSecs = secs;
    this.renderTtlPicker();
    void Api.updateChatPrefs({ ttlSecs: secs, ttlChosen: true }).catch(() => {
      // A preference that will not persist is worth saying out loud: the person
      // would otherwise find the timer back where it started after a restart.
      this.store.showToast(tr('chat.ttl_save_failed'), true);
    });
  }

  /** Read the saved timer, and on a profile that never chose one, say what the
   * new default does rather than quietly starting to delete messages. */
  private async loadPrefs(): Promise<void> {
    const revision = this.prefsRevision;
    const prefs = await Api.getChatPrefs();
    if (this.disposed || revision !== this.prefsRevision) return;
    this.updateHistoryPrompt(prefs);
    if (this.target.kind === 'group') return;
    // null is a choice, not an absence: picking "off" stores null, and testing
    // ttlSecs on its own cannot tell that from a profile that never chose.
    // It would leave the default sitting in this.ttlSecs and put the timer back
    // on a day later, with the picker showing "off" — the one state the person
    // had explicitly ruled out. ttlChosen is what separates the two.
    if (prefs.ttlChosen || prefs.ttlSecs !== null) this.ttlSecs = prefs.ttlSecs;
    this.renderTtlPicker();
    if (prefs.ttlChosen || ttlNoticeShown) return;
    ttlNoticeShown = true;
    this.store.showToast(tr('chat.ttl_default_notice'));
  }

  private async send(): Promise<void> {
    const body = this.input.value.trim();
    if (!body && this.pending.length === 0) return;
    const paths = this.pending.map((f) => f.path);
    const consoleMode = this.target.kind === 'contact' && this.store.isConsoleMode(this.target);
    if (consoleMode) {
      if (body) this.consoleHistory.push(body);
      this.historyIndex = -1;
      this.consoleDraft = '';
      if (this.consoleSending === 0) this.consoleSendingSince = Date.now();
      this.consoleSending++;
      this.renderLog();
      // A console line is a command, not a message: attachments go up first
      // (the agent saves them), then the body runs.
      this.input.value = '';
      this.input.style.height = 'auto';
      this.pending = [];
      this.renderFileChips();
      try {
        await this.store.sendConsoleCommand(this.target, body, paths);
      } catch (e) {
        this.store.showToast(tr('chat.command_failed') + errorText(e), true);
      } finally {
        this.consoleSending--;
        this.renderLog();
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
      this.store.showToast(tr('chat.send_failed', { error: errorText(e) }), true);
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

  /** Enter «кокаин», having said what it costs.
   *
   * The confirmation is words and not a tooltip on purpose: this shortens the
   * path at our own expense and stops hiding how much was said. Somebody who
   * taps it should already know both before the tunnels come down. */
  private async enterFast(): Promise<void> {
    const ok = await this.app.confirm(tr('chat.enable_cocaine'),
      tr('chat.the_route_will_be_one_hop_shorter_and'));
    if (!ok) return;
    await this.store.startFast();
  }

  /** The «кофеин» bar: what is on, and what can be turned on next. */
  private paintTurboBar(): void {
    const lane = this.store.lane.get();
    if (this.store.laneSwitching.get()) {
      this.turboTitle.textContent = tr('chat.rebuilding_tunnels');
      this.turboHint.textContent = tr('chat.nothing_is_sent_while_tunnels_are_being_built');
      this.fastBtn.classList.add('hidden');
      return;
    }
    if (lane === 'fast') {
      this.turboTitle.textContent = tr('chat.cocaine');
      this.turboHint.textContent = tr('chat.two_hops_no_size_padding_screen_stays_awake');
      this.fastBtn.classList.add('hidden');
      return;
    }
    this.turboTitle.textContent = tr('chat.caffeine_enabled');
    this.turboHint.textContent = tr('chat.screen_stays_awake_chat_stays_selected');
    this.fastBtn.classList.toggle('hidden', usesBrowserCore());
  }

  private async leaveCaffeine(): Promise<void> {
    const pass = await this.app.prompt(
      tr('chat.exit_caffeine'),
      tr('chat.profile_passphrase'),
      '',
      tr('chat.exit'),
      true,
    );
    if (!pass) return;
    try {
      const result = await Api.verifyPassphrase(pass);
      await this.store.stopCaffeine();
      if (result === 'wiped') {
        this.store.showToast(tr('auth.profile_wiped'), true);
        await this.store.lock();
      }
    } catch {
      this.store.showToast(tr('auth.wrong_pass'), true);
    }
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
