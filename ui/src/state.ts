import type { Contact, Message, IdentityCard, CoreEvent, Group, GroupMember, UpdateInfo } from './api';
import { Api } from './api';
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
  createChannel,
  Importance,
  Visibility,
} from '@tauri-apps/plugin-notification';

export class Signal<T> {
  private subs = new Set<(v: T) => void>();
  constructor(private value: T) {}
  get(): T { return this.value; }
  set(v: T): void {
    if (v === this.value) return;
    this.value = v;
    for (const s of this.subs) s(v);
  }
  update(fn: (v: T) => T): void { this.set(fn(this.value)); }
  subscribe(fn: (v: T) => void, fireImmediately = true): () => void {
    this.subs.add(fn);
    if (fireImmediately) fn(this.value);
    return () => { this.subs.delete(fn); };
  }
}

export type ViewKind = 'profile-select' | 'auth-create' | 'auth-unlock' | 'auth-booting' | 'main';

export type BootStage = 'unlocking' | 'tor' | 'relay' | 'done';

export type ChatTarget =
  | { kind: 'contact'; id: number }
  | { kind: 'group'; id: string };

export function sameTarget(a: ChatTarget | null, b: ChatTarget | null): boolean {
  if (!a || !b) return a === b;
  return a.kind === b.kind && a.id === b.id;
}

export function targetKey(t: ChatTarget): string {
  return `${t.kind}:${t.id}`;
}

export interface Identity {
  card: IdentityCard;
  onion: string;
  fingerprint: string;
}

export interface UpdateProgress {
  downloaded: number;
  total: number;
  pct: number;
}

export class Store {
  view = new Signal<ViewKind>('profile-select');
  bootStage = new Signal<BootStage>('unlocking');
  profiles = new Signal<string[]>([]);
  currentProfile = new Signal<string | null>(null);
  contacts = new Signal<Contact[]>([]);
  groups = new Signal<Group[]>([]);
  selectedChat = new Signal<ChatTarget | null>(null);
  messages = new Signal<Map<string, Message[]>>(new Map());
  pinned = new Signal<Map<string, Message[]>>(new Map());
  groupMembers = new Signal<Map<string, GroupMember[]>>(new Map());
  identity = new Signal<Identity | null>(null);
  displayName = new Signal<string>('');
  peerOnline = new Signal<Set<number>>(new Set());
  unread = new Signal<Map<string, number>>(new Map());
  toast = new Signal<{ text: string; err: boolean } | null>(null);
  relayConnected = new Signal<boolean>(false);
  muted = new Signal<Set<string>>(new Set());
  typing = new Signal<Map<string, { sender_sign_pk: string | null; until: number }>>(new Map());
  private typingClearTimers: Map<string, number> = new Map();
  private settled = false;
  private settledTimer: number | null = null;
  private windowVisible = true;
  private static readonly SETTLE_MS = 5_000;
  private static readonly NOTIFY_MAX_AGE_MS = 60_000;
  updateAvailable = new Signal<UpdateInfo | null>(null);
  updateProgress = new Signal<UpdateProgress | null>(null);
  updateReadyPath = new Signal<string | null>(null);
  updateError = new Signal<string | null>(null);
  scrollToMessage = new Signal<{ target: ChatTarget; messageId: number; nonce: number } | null>(null);
  sidebarCollapsed = new Signal<boolean>(typeof localStorage !== 'undefined' && localStorage.getItem('gipny:sidebar-collapsed') === '1');
  onlineTick = new Signal<number>(0);

  toggleSidebar(): void {
    const next = !this.sidebarCollapsed.get();
    this.sidebarCollapsed.set(next);
    try { localStorage.setItem('gipny:sidebar-collapsed', next ? '1' : '0'); } catch {}
  }

  private lastTrayBadge = -1;
  private pushTrayBadge(): void {
    let count = 0;
    for (const v of this.unread.get().values()) if (v > 0) count += 1;
    if (count === this.lastTrayBadge) return;
    this.lastTrayBadge = count;
    Api.updateTrayBadge(count).catch(() => {});
  }

  private unsubEvents: (() => void) | null = null;
  private unsubTrayBadge: (() => void) | null = null;
  private notifyGranted: boolean = false;
  private readonly linuxDesktop: boolean = /Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent);
  private scrollNonce: number = 0;
  private watchdogTimer: number | null = null;
  private lastSeenMs: Map<number, number> = new Map();
  private onlineTickTimer: number | null = null;
  private static readonly ONLINE_WINDOW_MS = 60_000;

  private soundCache: Map<string, HTMLAudioElement> = new Map();
  private audioCtx: AudioContext | null = null;
  private audioBufferCache: Map<string, AudioBuffer> = new Map();
  private audioPathOk: boolean | null = null;

  private async playViaAudioContext(url: string): Promise<boolean> {
    try {
      if (!this.audioCtx) {
        const Ctor = (window.AudioContext || (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext);
        if (!Ctor) return false;
        this.audioCtx = new Ctor();
      }
      const ctx = this.audioCtx;
      let buf = this.audioBufferCache.get(url) ?? null;
      if (!buf) {
        const r = await fetch(url);
        if (!r.ok) return false;
        const data = await r.arrayBuffer();
        buf = await ctx.decodeAudioData(data);
        this.audioBufferCache.set(url, buf);
      }
      const src = ctx.createBufferSource();
      src.buffer = buf;
      const gain = ctx.createGain();
      gain.gain.value = 0.4;
      src.connect(gain).connect(ctx.destination);
      if (ctx.state === 'suspended') await ctx.resume();
      src.start();
      return true;
    } catch {
      return false;
    }
  }

  private playViaHtmlAudio(url: string): void {
    let audio = this.soundCache.get(url);
    if (!audio) {
      audio = new Audio(url);
      audio.volume = 0.4;
      audio.preload = 'auto';
      this.soundCache.set(url, audio);
    }
    audio.currentTime = 0;
    audio.play().catch((e) => console.error('[notify-sound]', url, e));
  }

  private playNotify(name?: string | null): void {
    const url = name ? `/sounds/${name}.wav` : '/notify.wav';
    void this.playViaAudioContext(url).then((ok) => {
      if (ok) { this.audioPathOk = true; return; }
      if (this.linuxDesktop) {
        Api.playNotifySound(name ?? null).then(() => { this.audioPathOk = true; })
          .catch((e) => {
            console.error('[notify-sound] rust path failed', e);
            this.playViaHtmlAudio(url);
          });
        return;
      }
      this.playViaHtmlAudio(url);
    });
  }

  private async ensureNotifyPermission(): Promise<void> {
    if (this.linuxDesktop) {
      try {
        const report = await Api.notifyProbe();
        console.log('[notify-probe]', report);
      } catch (e) { console.error('[notify-probe]', e); }
      this.notifyGranted = false;
      return;
    }
    try {
      this.notifyGranted = await isPermissionGranted();
      if (!this.notifyGranted) {
        const p = await requestPermission();
        this.notifyGranted = p === 'granted';
      }
      if (this.notifyGranted) {
        try {
          await createChannel({
            id: 'gipny_messages',
            name: 'messages',
            description: 'incoming chat messages',
            importance: Importance.High,
            visibility: Visibility.Private,
            lights: true,
            lightColor: '#5cff5c',
            vibration: true,
          });
        } catch (e) { console.error('[notify-channel]', e); }
      }
    } catch { this.notifyGranted = false; }
  }

  private nativeNotify(title: string, body: string): void {
    if (this.linuxDesktop) {
      Api.notifyOs(title, body).catch((e) => {
        console.error('[notify-os]', e);
        if (!this.pluginNotify(title, body)) this.inAppToastNotify(title, body);
      });
      return;
    }
    if (!this.pluginNotify(title, body)) this.inAppToastNotify(title, body);
  }

  private pluginNotify(title: string, body: string): boolean {
    if (!this.notifyGranted) return false;
    try {
      sendNotification({ title, body, channelId: 'gipny_messages' });
      return true;
    } catch (e) { console.error('[notify] err', e); return false; }
  }

  private inAppToastNotify(title: string, body: string): void {
    const preview = body.length > 80 ? body.slice(0, 80) + '…' : body;
    this.toast.set({ text: `${title}: ${preview}`, err: false });
  }

  async bootstrap(): Promise<void> {
    await this.refreshProfiles();
    const list = this.profiles.get();
    this.view.set(list.length === 0 ? 'auth-create' : 'profile-select');
  }

  async refreshProfiles(): Promise<void> {
    this.profiles.set(await Api.listProfiles());
  }

  selectProfileForUnlock(profile: string): void {
    this.currentProfile.set(profile);
    this.view.set('auth-unlock');
  }

  goToCreate(): void {
    this.currentProfile.set(null);
    this.view.set('auth-create');
  }

  async cancelToProfileSelect(): Promise<void> {
    await this.refreshProfiles();
    this.currentProfile.set(null);
    this.view.set(this.profiles.get().length > 0 ? 'profile-select' : 'auth-create');
  }

  async onUnlocked(profile: string): Promise<void> {
    this.currentProfile.set(profile);
    this.bootStage.set('tor');
    this.relayConnected.set(false);
    this.view.set('auth-booting');
    const [card, onion, fingerprint, displayName] = await Promise.all([
      Api.myCard(), Api.myOnion(), Api.myFingerprint(), Api.getDisplayName(),
    ]);
    this.identity.set({ card, onion, fingerprint });
    this.displayName.set(displayName);
    await this.refreshAll();
    this.unsubEvents = await Api.onEvent((e) => this.handleEvent(e));
    this.unsubTrayBadge?.();
    this.unsubTrayBadge = this.unread.subscribe(() => this.pushTrayBadge(), true);
    await this.ensureNotifyPermission();
    this.installVisibilityHook();
    this.startWatchdog();
    this.bootStage.set('relay');
  }

  private installVisibilityHook(): void {
    this.windowVisible = !document.hidden && document.hasFocus();
    const update = (): void => { this.windowVisible = !document.hidden && document.hasFocus(); };
    document.addEventListener('visibilitychange', update);
    window.addEventListener('focus', update);
    window.addEventListener('blur', update);
  }

  private startWatchdog(): void {
    if (this.watchdogTimer != null) window.clearInterval(this.watchdogTimer);
    this.watchdogTimer = window.setInterval(() => {
      const t = this.selectedChat.get();
      if (!t) return;
      this.loadMessages(t).catch(() => {});
      this.loadPinned(t).catch(() => {});
    }, 30_000);
    if (this.onlineTickTimer != null) window.clearInterval(this.onlineTickTimer);
    this.onlineTickTimer = window.setInterval(() => this.recomputeOnline(), 15_000);
  }

  private stopWatchdog(): void {
    if (this.watchdogTimer != null) {
      window.clearInterval(this.watchdogTimer);
      this.watchdogTimer = null;
    }
    if (this.onlineTickTimer != null) {
      window.clearInterval(this.onlineTickTimer);
      this.onlineTickTimer = null;
    }
  }

  private markPeerActive(contactId: number, ts: number): void {
    const cur = this.lastSeenMs.get(contactId) ?? 0;
    if (ts > cur) this.lastSeenMs.set(contactId, ts);
    if (!this.peerOnline.get().has(contactId)) {
      this.peerOnline.update((s) => { const n = new Set(s); n.add(contactId); return n; });
    }
  }

  private recomputeOnline(): void {
    const cutoff = Date.now() - Store.ONLINE_WINDOW_MS;
    const next = new Set<number>();
    for (const [id, ts] of this.lastSeenMs) if (ts > cutoff) next.add(id);
    const cur = this.peerOnline.get();
    if (next.size !== cur.size || [...next].some((x) => !cur.has(x))) {
      this.peerOnline.set(next);
    }
    this.onlineTick.set(Date.now());
  }

  async updateDisplayName(name: string): Promise<void> {
    await Api.setDisplayName(name);
    this.displayName.set(name);
  }

  async lock(): Promise<void> {
    this.stopWatchdog();
    this.unsubEvents?.();
    this.unsubEvents = null;
    this.unsubTrayBadge?.();
    this.unsubTrayBadge = null;
    Api.updateTrayBadge(0).catch(() => {});
    this.lastTrayBadge = -1;
    await Api.vaultLock();
    this.contacts.set([]);
    this.groups.set([]);
    this.messages.set(new Map());
    this.pinned.set(new Map());
    this.groupMembers.set(new Map());
    this.selectedChat.set(null);
    this.peerOnline.set(new Set());
    this.lastSeenMs.clear();
    this.unread.set(new Map());
    this.identity.set(null);
    this.currentProfile.set(null);
    this.relayConnected.set(false);
    this.bootStage.set('unlocking');
    this.updateAvailable.set(null);
    this.updateProgress.set(null);
    this.updateReadyPath.set(null);
    this.updateError.set(null);
    await this.cancelToProfileSelect();
  }

  async deleteProfile(profile: string): Promise<void> {
    await Api.deleteProfile(profile);
    await this.refreshProfiles();
    if (this.profiles.get().length === 0) this.view.set('auth-create');
  }

  async refreshAll(): Promise<void> {
    const [contacts, groups, mutedList] = await Promise.all([
      Api.listContacts(), Api.listGroups(), Api.listMuted().catch(() => []),
    ]);
    this.contacts.set(contacts);
    this.groups.set(groups);
    this.muted.set(new Set(mutedList));
    for (const c of contacts) {
      if (c.last_seen != null) this.lastSeenMs.set(c.id, c.last_seen);
    }
    this.recomputeOnline();
    const unread = new Map<string, number>();
    await Promise.all([
      ...contacts.map(async (c) => unread.set(targetKey({ kind: 'contact', id: c.id }), await Api.unreadCount(c.id))),
      ...groups.map(async (g) => unread.set(targetKey({ kind: 'group', id: g.id }), await Api.groupUnreadCount(g.id))),
    ]);
    this.unread.set(unread);
  }

  async toggleMute(target: ChatTarget, muted: boolean): Promise<void> {
    const key = targetKey(target);
    await Api.setMuted(key, muted);
    this.muted.update((s) => {
      const n = new Set(s);
      if (muted) n.add(key); else n.delete(key);
      return n;
    });
  }

  async pinChat(target: ChatTarget): Promise<void> {
    const ts = Date.now();
    if (target.kind === 'contact') {
      await Api.pinChat(target.id, null);
      this.contacts.update((list) => Store.sortContacts(list.map((c) => c.id === target.id ? { ...c, pinned_at: ts } : c)));
    } else {
      await Api.pinChat(null, target.id);
      this.groups.update((list) => Store.sortGroups(list.map((g) => g.id === target.id ? { ...g, pinned_at: ts } : g)));
    }
  }

  async unpinChat(target: ChatTarget): Promise<void> {
    if (target.kind === 'contact') {
      await Api.unpinChat(target.id, null);
      this.contacts.update((list) => Store.sortContacts(list.map((c) => c.id === target.id ? { ...c, pinned_at: null } : c)));
    } else {
      await Api.unpinChat(null, target.id);
      this.groups.update((list) => Store.sortGroups(list.map((g) => g.id === target.id ? { ...g, pinned_at: null } : g)));
    }
  }

  private bumpChatOrder(target: ChatTarget, ts: number): void {
    if (target.kind === 'contact') {
      this.contacts.update((list) => {
        const idx = list.findIndex((c) => c.id === target.id);
        if (idx < 0) return list;
        const cur = list[idx]!.last_message_at ?? 0;
        if (ts <= cur) return list;
        const next = list.slice();
        next[idx] = { ...next[idx]!, last_message_at: ts };
        return Store.sortContacts(next);
      });
    } else {
      this.groups.update((list) => {
        const idx = list.findIndex((g) => g.id === target.id);
        if (idx < 0) return list;
        const cur = list[idx]!.last_message_at ?? 0;
        if (ts <= cur) return list;
        const next = list.slice();
        next[idx] = { ...next[idx]!, last_message_at: ts };
        return Store.sortGroups(next);
      });
    }
  }

  private static cmpChat<T extends { pinned_at: number | null; last_message_at: number | null; name: string }>(a: T, b: T): number {
    const ap = a.pinned_at != null;
    const bp = b.pinned_at != null;
    if (ap !== bp) return ap ? -1 : 1;
    if (ap && bp && a.pinned_at !== b.pinned_at) return (b.pinned_at as number) - (a.pinned_at as number);
    const al = a.last_message_at ?? 0;
    const bl = b.last_message_at ?? 0;
    if (al !== bl) return bl - al;
    return a.name.localeCompare(b.name);
  }

  static sortContacts(list: Contact[]): Contact[] {
    return list.slice().sort(Store.cmpChat);
  }

  static sortGroups(list: Group[]): Group[] {
    return list.slice().sort(Store.cmpChat);
  }

  async refreshContacts(): Promise<void> { await this.refreshAll(); }
  async refreshGroups(): Promise<void> { await this.refreshAll(); }

  async selectChat(target: ChatTarget | null): Promise<void> {
    this.selectedChat.set(target);
    if (!target) return;
    await this.loadMessages(target);
    await this.loadPinned(target);
    if (target.kind === 'contact') {
      await Api.markRead(target.id);
    } else {
      await Api.markGroupRead(target.id);
      if (!this.groupMembers.get().has(target.id)) {
        const members = await Api.listGroupMembers(target.id);
        this.groupMembers.update((m) => { const n = new Map(m); n.set(target.id, members); return n; });
      }
    }
    this.unread.update((m) => { const n = new Map(m); n.set(targetKey(target), 0); return n; });
  }

  async loadMessages(target: ChatTarget): Promise<void> {
    const list = target.kind === 'contact'
      ? await Api.listMessages(target.id, 200)
      : await Api.listGroupMessages(target.id, 200);
    list.reverse();
    const key = targetKey(target);
    this.messages.update((m) => {
      const n = new Map(m);
      const existing = n.get(key) ?? [];
      if (list.length === 0) { n.set(key, existing); return n; }
      const serverMinId = list[0]?.id ?? Number.POSITIVE_INFINITY;
      const olderLocal = existing.filter((msg) => msg.id < serverMinId);
      const merged = [...olderLocal, ...list];
      n.set(key, merged);
      return n;
    });
  }

  async loadMoreMessages(target: ChatTarget): Promise<boolean> {
    const key = targetKey(target);
    const existing = this.messages.get().get(key) ?? [];
    const oldest = existing.length > 0 ? existing[0]?.id : null;
    if (oldest == null) {
      await this.loadMessages(target);
      return (this.messages.get().get(key) ?? []).length > 0;
    }
    const more = target.kind === 'contact'
      ? await Api.listMessages(target.id, 200, oldest)
      : await Api.listGroupMessages(target.id, 200, oldest);
    if (more.length === 0) return false;
    more.reverse();
    this.messages.update((m) => {
      const n = new Map(m);
      const cur = n.get(key) ?? [];
      n.set(key, [...more, ...cur]);
      return n;
    });
    return true;
  }

  async loadUntilMessage(target: ChatTarget, messageId: number): Promise<boolean> {
    const key = targetKey(target);
    const list = this.messages.get().get(key) ?? [];
    if (list.some((m) => m.id === messageId)) return true;

    const contactId = target.kind === 'contact' ? target.id : null;
    const groupId = target.kind === 'group' ? target.id : null;
    const pos = await Api.messagePosition(contactId, groupId, messageId);
    if (pos == null) return false;

    const limit = Math.min(pos + 50, 20_000);
    const slice = target.kind === 'contact'
      ? await Api.listMessages(target.id, limit)
      : await Api.listGroupMessages(target.id, limit);
    slice.reverse();
    if (slice.length === 0) return false;
    this.messages.update((m) => {
      const n = new Map(m);
      const existing = n.get(key) ?? [];
      const sliceMin = slice[0]?.id ?? Number.POSITIVE_INFINITY;
      const sliceMax = slice[slice.length - 1]?.id ?? Number.NEGATIVE_INFINITY;
      const olderLocal = existing.filter((msg) => msg.id < sliceMin);
      const newerLocal = existing.filter((msg) => msg.id > sliceMax);
      n.set(key, [...olderLocal, ...slice, ...newerLocal]);
      return n;
    });
    return (this.messages.get().get(key) ?? []).some((m) => m.id === messageId);
  }

  async loadPinned(target: ChatTarget): Promise<void> {
    const list = target.kind === 'contact'
      ? await Api.listPinnedContact(target.id)
      : await Api.listPinnedGroup(target.id);
    const key = targetKey(target);
    this.pinned.update((m) => { const n = new Map(m); n.set(key, list); return n; });
  }

  async sendMessage(
    target: ChatTarget,
    body: string,
    paths: string[] = [],
    ttlSecs: number | null = null,
    replyTo: number | null = null,
  ): Promise<void> {
    if (target.kind === 'contact') {
      await Api.sendMessagePaths(target.id, body, paths, ttlSecs, replyTo);
    } else {
      await Api.sendGroupMessagePaths(target.id, body, paths, ttlSecs, replyTo);
    }
    await this.loadMessages(target);
  }

  async editMessage(target: ChatTarget, messageId: number, newBody: string): Promise<void> {
    if (target.kind === 'contact') await Api.sendEdit(target.id, messageId, newBody);
    else await Api.sendEditGroup(target.id, messageId, newBody);
    await this.loadMessages(target);
  }

  async pinMessage(target: ChatTarget, messageId: number): Promise<void> {
    if (target.kind === 'contact') await Api.pinContactMessage(target.id, messageId);
    else await Api.pinGroupMessage(target.id, messageId);
    await this.loadPinned(target);
  }

  async unpinMessage(target: ChatTarget, messageId: number): Promise<void> {
    if (target.kind === 'contact') await Api.unpinContactMessage(target.id, messageId);
    else await Api.unpinGroupMessage(target.id, messageId);
    await this.loadPinned(target);
  }

  requestScrollTo(target: ChatTarget, messageId: number): void {
    this.scrollNonce++;
    this.scrollToMessage.set({ target, messageId, nonce: this.scrollNonce });
  }

  async pressButton(contactId: number, messageId: number, callbackData: string): Promise<void> {
    await Api.pressButton(contactId, messageId, callbackData);
  }

  async pressGroupButton(groupId: string, messageId: number, callbackData: string): Promise<void> {
    await Api.pressGroupButton(groupId, messageId, callbackData);
  }

  async createGroup(name: string, memberContactIds: number[]): Promise<string> {
    const gid = await Api.createGroup(name, memberContactIds);
    await this.refreshGroups();
    const members = await Api.listGroupMembers(gid);
    this.groupMembers.update((m) => { const n = new Map(m); n.set(gid, members); return n; });
    return gid;
  }

  async addGroupMember(groupId: string, contactId: number): Promise<void> {
    await Api.addGroupMember(groupId, contactId);
    const members = await Api.listGroupMembers(groupId);
    this.groupMembers.update((m) => { const n = new Map(m); n.set(groupId, members); return n; });
  }

  async deleteGroup(groupId: string): Promise<void> {
    await Api.deleteGroup(groupId);
    await this.refreshGroups();
    if (this.selectedChat.get()?.kind === 'group' && (this.selectedChat.get() as { id: string }).id === groupId) {
      this.selectedChat.set(null);
    }
  }

  async installUpdate(): Promise<void> {
    this.updateProgress.set({ downloaded: 0, total: 0, pct: 0 });
    this.updateError.set(null);
    try {
      await Api.installUpdate();
    } catch (e) {
      this.updateError.set(String(e));
      this.updateProgress.set(null);
    }
  }

  async dismissUpdate(): Promise<void> {
    const info = this.updateAvailable.get();
    if (!info) return;
    await Api.dismissUpdate(info.version);
    this.updateAvailable.set(null);
  }

  private handleEvent(e: CoreEvent): void {
    if (typeof e === 'string') {
      if (e === 'RelayConnected') {
        const wasOffline = !this.relayConnected.get();
        this.relayConnected.set(true);
        this.bootStage.set('done');
        if (this.view.get() === 'auth-booting') this.view.set('main');
        if (wasOffline) {
          const t = this.selectedChat.get();
          if (t) {
            this.loadMessages(t).catch(() => {});
            this.loadPinned(t).catch(() => {});
          }
          this.settled = false;
          if (this.settledTimer != null) window.clearTimeout(this.settledTimer);
          this.settledTimer = window.setTimeout(() => { this.settled = true; }, Store.SETTLE_MS);
        }
      } else if (e === 'RelayDisconnected') {
        this.relayConnected.set(false);
        this.settled = false;
        if (this.settledTimer != null) {
          window.clearTimeout(this.settledTimer);
          this.settledTimer = null;
        }
      }
      return;
    }
    if (!e || typeof e !== 'object') return;
    if ('IncomingMessage' in e) {
      const m = e.IncomingMessage;
      const target: ChatTarget | null = m.group_id
        ? { kind: 'group', id: m.group_id }
        : m.contact_id != null ? { kind: 'contact', id: m.contact_id } : null;
      if (!target) return;
      const key = targetKey(target);
      this.bumpChatOrder(target, m.sent_at);
      if (target.kind === 'contact') {
        this.markPeerActive(target.id, Date.now());
      } else if (m.sender_sign_pk) {
        const sc = this.contacts.get().find((c) => c.sign_pk === m.sender_sign_pk);
        if (sc) this.markPeerActive(sc.id, Date.now());
      }
      this.loadMessages(target);
      const selected = this.selectedChat.get();
      const isActiveChat = sameTarget(selected, target);
      const visible = this.windowVisible;
      if (isActiveChat && visible) {
        if (target.kind === 'contact') Api.markRead(target.id);
        else Api.markGroupRead(target.id);
      } else {
        this.unread.update((u) => {
          const n = new Map(u);
          n.set(key, (n.get(key) ?? 0) + 1);
          return n;
        });
      }
      const fresh = this.settled && (Date.now() - m.sent_at) < Store.NOTIFY_MAX_AGE_MS;
      const muted = this.muted.get().has(key);
      const shouldNotify = fresh && !muted && !(isActiveChat && visible);
      if (shouldNotify) {
        const label = target.kind === 'contact'
          ? (this.contacts.get().find((c) => c.id === target.id)?.name ?? 'unknown')
          : (this.groups.get().find((g) => g.id === target.id)?.name ?? 'group');
        const senderName = target.kind === 'group' && m.sender_sign_pk
          ? (this.groupMembers.get().get(target.id)?.find((mm) => mm.sign_pk === m.sender_sign_pk)?.name
            ?? this.contacts.get().find((c) => c.sign_pk === m.sender_sign_pk)?.name
            ?? 'someone')
          : label;
        const preview = m.body.length > 60 ? m.body.slice(0, 60) + '…' : m.body;
        const prefix = target.kind === 'group' ? `[${label}] ${senderName}` : label;
        this.nativeNotify(prefix, preview);
        this.playNotify(m.notify_sound ?? null);
      }
    } else if ('MessageSent' in e) {
      const id = e.MessageSent.message_id;
      const bumps: Array<{ target: ChatTarget; ts: number }> = [];
      this.messages.update((m) => {
        const n = new Map(m);
        for (const [k, list] of n) {
          const idx = list.findIndex((x) => x.id === id);
          if (idx >= 0) {
            const copy = list.slice();
            copy[idx] = { ...copy[idx]!, sent: true };
            n.set(k, copy);
            const msg = copy[idx]!;
            const t: ChatTarget | null = msg.group_id
              ? { kind: 'group', id: msg.group_id }
              : msg.contact_id != null ? { kind: 'contact', id: msg.contact_id } : null;
            if (t) bumps.push({ target: t, ts: msg.sent_at });
          }
        }
        return n;
      });
      for (const b of bumps) this.bumpChatOrder(b.target, b.ts);
    } else if ('MessageDelivered' in e) {
      const id = e.MessageDelivered.message_id;
      const activeContacts: number[] = [];
      this.messages.update((m) => {
        const n = new Map(m);
        for (const [k, list] of n) {
          const idx = list.findIndex((x) => x.id === id);
          if (idx >= 0) {
            const copy = list.slice();
            copy[idx] = { ...copy[idx]!, sent: true, delivered: true };
            n.set(k, copy);
            const cid = copy[idx]!.contact_id;
            if (cid != null) activeContacts.push(cid);
          }
        }
        return n;
      });
      for (const cid of activeContacts) this.markPeerActive(cid, Date.now());
    } else if ('Typing' in e) {
      const { contact_id, group_id, sender_sign_pk, typing } = e.Typing;
      const target: ChatTarget | null = group_id != null
        ? { kind: 'group', id: group_id }
        : contact_id != null ? { kind: 'contact', id: contact_id } : null;
      if (target) {
        if (typing) {
          if (target.kind === 'contact') {
            this.markPeerActive(target.id, Date.now());
          } else if (sender_sign_pk) {
            const sc = this.contacts.get().find((c) => c.sign_pk === sender_sign_pk);
            if (sc) this.markPeerActive(sc.id, Date.now());
          }
        }
        const key = targetKey(target);
        if (typing) {
          this.typing.update((m) => {
            const n = new Map(m);
            n.set(key, { sender_sign_pk, until: Date.now() + 5000 });
            return n;
          });
          if (this.typingClearTimers.has(key)) window.clearTimeout(this.typingClearTimers.get(key)!);
          const t = window.setTimeout(() => {
            this.typing.update((m) => { const n = new Map(m); n.delete(key); return n; });
            this.typingClearTimers.delete(key);
          }, 5000);
          this.typingClearTimers.set(key, t);
        } else {
          this.typing.update((m) => { const n = new Map(m); n.delete(key); return n; });
          if (this.typingClearTimers.has(key)) {
            window.clearTimeout(this.typingClearTimers.get(key)!);
            this.typingClearTimers.delete(key);
          }
        }
      }
    } else if ('MessageEdited' in e) {
      const { message_id, body, buttons } = e.MessageEdited;
      this.messages.update((m) => {
        const n = new Map(m);
        for (const [k, list] of n) {
          const idx = list.findIndex((x) => x.id === message_id);
          if (idx >= 0) {
            const copy = list.slice();
            copy[idx] = { ...copy[idx]!, body, buttons };
            n.set(k, copy);
          }
        }
        return n;
      });
      this.pinned.update((m) => {
        const n = new Map(m);
        for (const [k, list] of n) {
          const idx = list.findIndex((x) => x.id === message_id);
          if (idx >= 0) {
            const copy = list.slice();
            copy[idx] = { ...copy[idx]!, body, buttons };
            n.set(k, copy);
          }
        }
        return n;
      });
    } else if ('MessagePinned' in e || 'MessageUnpinned' in e) {
      const p = 'MessagePinned' in e ? e.MessagePinned : e.MessageUnpinned;
      const target: ChatTarget | null = p.group_id
        ? { kind: 'group', id: p.group_id }
        : p.contact_id != null ? { kind: 'contact', id: p.contact_id } : null;
      if (target) this.loadPinned(target);
    } else if ('ContactAdded' in e) {
      this.refreshContacts();
    } else if ('ContactUpdated' in e) {
      this.refreshContacts();
    } else if ('GroupUpdated' in e) {
      this.refreshGroups();
      const gid = e.GroupUpdated.group_id;
      Api.listGroupMembers(gid).then((members) => {
        this.groupMembers.update((m) => { const n = new Map(m); n.set(gid, members); return n; });
      }).catch(() => {});
    } else if ('PeerOnline' in e) {
      this.peerOnline.update((s) => { const n = new Set(s); n.add(e.PeerOnline.contact_id); return n; });
    } else if ('PeerOffline' in e) {
      this.peerOnline.update((s) => { const n = new Set(s); n.delete(e.PeerOffline.contact_id); return n; });
    } else if ('UpdateAvailable' in e) {
      this.updateAvailable.set(e.UpdateAvailable);
    } else if ('UpdateProgress' in e) {
      this.updateProgress.set(e.UpdateProgress);
    } else if ('UpdateReady' in e) {
      this.updateReadyPath.set(e.UpdateReady.path);
      this.updateProgress.set(null);
    } else if ('UpdateFailed' in e) {
      this.updateError.set(e.UpdateFailed.reason);
      this.updateProgress.set(null);
    }
  }

  showToast(text: string, err = false): void {
    this.toast.set({ text, err });
    setTimeout(() => this.toast.set(null), 2800);
  }
}

export async function pasteFileToTempPath(file: File): Promise<string> {
  const buf = await file.arrayBuffer();
  const bytes = Array.from(new Uint8Array(buf));
  return Api.savePasteTemp(file.name || `paste-${Date.now()}.bin`, bytes);
}