import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface VaultStatus { exists: boolean; unlocked: boolean; }

export interface IdentityCard { sign_pk: string; dh_pk: string; }

/** A folder the person made for their contacts. Kept in the vault as JSON;
 * the backend does not look inside. */
export interface ContactFolder {
  id: string;
  name: string;
  collapsed: boolean;
  contacts: number[];
}

/** The relay network as this device's node sees it. */
export interface DhtStatus {
  peers: number;
  items: number;
  bytes: number;
  joined: boolean;
  stores: boolean;
  seeds: number;
}

export interface Contact {
  id: number;
  sign_pk: string;
  dh_pk: string;
  onion: string;
  name: string;
  trust: number;
  created_at: number;
  last_seen: number | null;
  is_bot: boolean;
  pinned_at: number | null;
  last_message_at: number | null;
  /** Relay this contact receives through; null falls back to our own setting. */
  relay: string | null;
  /** This contact is running in agent mode with us as master: its console is
   * open. The chat shows the чат/консоль switch when true. */
  agent_granted: boolean;
  /** 'incoming': they introduced themselves and wait for an answer; their
   * messages are held back until then. 'outgoing': we added their card and
   * have not heard from them yet. */
  request: 'none' | 'incoming' | 'outgoing';
}

export type TransitProfile = 'frugal' | 'balanced' | 'generous';
export type YggdrasilMode = 'off' | 'auto' | 'on';

export interface RouterSettings {
  /** How much of the line carries other people's tunnels. */
  transit: TransitProfile;
  /** Speak i2p over a Yggdrasil mesh as well, when one is running locally. */
  yggdrasil: YggdrasilMode;
}

export interface Button { text: string; callback_data: string; }

/** Console framing on a message. `kind` is one of the CONSOLE_* constants. */
export interface ConsoleFrame {
  kind: number;
  exit_code: number | null;
  duration_ms: number | null;
  truncated: boolean;
}

export type RelayMode = 'builtin' | 'external';

/** The relay this process hosts for itself in built-in mode. */
export type HostedRelayState =
  | { state: 'off' }
  | { state: 'starting' }
  | { state: 'ready'; address: string }
  | { state: 'failed'; reason: string };

export interface RelayInfo {
  mode: RelayMode;
  /** The address saved for external mode, whether or not it is in use. */
  external: string;
  hosted: HostedRelayState;
  /** How the attempt to reach our own relay is going. */
  dial: { attempts: number; last_error: string | null; unreachable_checks: number; unreachable_error: string | null };
}

/** A contact that put this client into agent mode, from `get_agent_mode`. */
export interface AgentMaster {
  contact_id: number;
  name: string;
  sign_pk: string;
}

/**
 * Console message kinds. Mirrors the `CONSOLE_*` u8 constants in
 * `libcore/src/session.rs`; a value the UI does not know is drawn as a plain
 * system line rather than breaking the log.
 */
export const CONSOLE_COMMAND = 0;
export const CONSOLE_OUTPUT = 1;
export const CONSOLE_GRANT = 2;
export const CONSOLE_REVOKE = 3;
export const CONSOLE_OFF = 4;

export interface Message {
  id: number;
  contact_id: number | null;
  group_id: string | null;
  sender_sign_pk: string | null;
  outgoing: boolean;
  body: string;
  sent_at: number;
  sent: boolean;
  delivered: boolean;
  read: boolean;
  expires_at: number | null;
  buttons?: Button[][] | null;
  reply_to: number | null;
  /** Set when the row is console-framed (command, output or mode marker). */
  console?: ConsoleFrame | null;
}

export interface Group {
  id: string;
  name: string;
  created_at: number;
  pinned_at: number | null;
  last_message_at: number | null;
}

export interface GroupMember {
  sign_pk: string;
  dh_pk: string;
  onion: string;
  name: string;
  is_self: boolean;
}

export interface Attachment {
  id: number;
  message_id: number;
  name: string;
  size: number;
}

export interface MediaItem {
  id: number;
  message_id: number;
  name: string;
  size: number;
  sent_at: number;
}

export interface SearchHit {
  message: Message;
  contact_id: number | null;
  group_id: string | null;
  contact_name: string | null;
  group_name: string | null;
}

export interface Bundle {
  sign_pk: string;
  dh_pk: string;
  signed_prekey: string;
  signed_prekey_sig: string;
  one_time_prekey: string | null;
  one_time_id: number | null;
}

/** One step of opening a profile, from the backend's `boot_status` event.
 * `stage` is a stable id the interface turns into its own wording; `detail` is
 * the technical line, shown under «технические подробности». */
export interface BootStatus {
  stage: 'vault' | 'router' | 'tunnels' | 'session' | 'core' | 'relay' | 'dht';
  state: 'active' | 'done' | 'failed';
  detail: string;
}

/** Which speed/anonymity trade is in force.
 *
 * Named for what each one does. The buttons say «КОКАИН» and «НИТРО»; those
 * are labels, and labels change. */
export type Lane = 'normal' | 'fast';

/** The channel to one contact, as the strip above the chat shows it.
 *
 * Two hop counts, not one: a letter goes out through our tunnel and arrives
 * through the inbound tunnel of the relay the other person collects from.
 * Each side owns its own leg, and shortening a leg exposes the side that
 * shortened it — which is why they are counted separately. */
export interface LinkStats {
  lane: Lane;
  our_hops: number;
  their_hops: number;
  padded: boolean;
  route: 'relay' | 'archive' | 'none';
  rtt_ms: number | null;
}

export interface UpdateInfo {
  version: string;
  notes: string;
  size: number;
}

/** What a check found. Every "nothing to install" case has its own name:
 * one sentence for all of them is how a .deb install spent three releases
 * being told it was current. */
export type CheckResult =
  | { status: 'update'; version: string; notes: string; size: number }
  | { status: 'current'; latest: string }
  | { status: 'dismissed'; version: string }
  | { status: 'unavailable' }
  | { status: 'unsupported'; latest: string }
  | { status: 'no_asset'; latest: string; wanted: string };

export interface ApkArtifact { arch: string; size: number; }
export interface ApkArtifacts { version: string; artifacts: ApkArtifact[]; }

export type CoreEvent =
  | { IncomingMessage: { contact_id: number | null; group_id: string | null; sender_sign_pk: string | null; message_id: number; body: string; sent_at: number; notify_sound: string | null; console_kind: number | null } }
  | { MessageEdited: { message_id: number; body: string; buttons: Button[][] | null } }
  | { MessagePinned: { contact_id: number | null; group_id: string | null; message_id: number } }
  | { MessageUnpinned: { contact_id: number | null; group_id: string | null; message_id: number } }
  | { MessageSent: { message_id: number } }
  | { MessageDelivered: { message_id: number } }
  | { Typing: { contact_id: number | null; group_id: string | null; sender_sign_pk: string | null; typing: boolean } }
  | { ContactAdded: { contact_id: number } }
  | { ContactUpdated: { contact_id: number } }
  | { ContactWiped: { contact_id: number; name: string } }
  | { ContactRequest: { contact_id: number } }
  | { GroupUpdated: { group_id: string } }
  // The only presence signal: something arrived from them, written at
  // `at_ms` by their clock. Mail held on a relay for hours arrives with an
  // old stamp, which is exactly how "online" tells itself from "delivered".
  | { PeerSeen: { contact_id: number; at_ms: number } }
  | { UpdateAvailable: { version: string; notes: string; size: number } }
  | { UpdateProgress: { downloaded: number; total: number; pct: number } }
  | { UpdateStaged: { version: string } }
  | { UpdateReady: { path: string } }
  | { UpdateFailed: { reason: string } }
  | { AgentModeChanged: { master: AgentMaster | null } }
  | { ConsoleActivity: { contact_id: number } }
  | { RelayInfoChanged: { info: RelayInfo } }
  | { ContactReachability: { contact_id: number; unreachable: boolean } }
  | { LinkRtt: { contact_id: number; ms: number } }
  | { LaneChanged: { lane: Lane } };

export interface PendingAttachment { name: string; data: string; }

export class Api {
  static listProfiles(): Promise<string[]> {
    return invoke('list_profiles');
  }
  static deleteProfile(profile: string): Promise<void> {
    return invoke('delete_profile', { profile });
  }
  /** Start building i2p tunnels for this profile now, before the password.
   * Nothing in the transport needs the vault, so the wait can happen while
   * the unlock screen is on rather than after it. */
  static prewarmNetwork(profile: string): Promise<void> {
    return invoke('prewarm_network', { profile });
  }
  static prewarmStatus(): Promise<'building' | 'ready' | 'off'> {
    return invoke('prewarm_status');
  }
  static vaultStatus(profile: string): Promise<VaultStatus> {
    return invoke('vault_status', { profile });
  }
  static vaultCreate(profile: string, pass: string, displayName: string, duressPass: string | null, duressWipe: boolean, maxAttempts: number): Promise<void> {
    return invoke('vault_create', { profile, pass, displayName, duressPass, duressWipe, maxAttempts });
  }
  static vaultUnlock(profile: string, pass: string): Promise<string | null> {
    return invoke('vault_unlock', { profile, pass });
  }
  /** Checks the profile passphrase without restarting anything. Resolves
   * 'ok', 'wiped' (a duress passphrase wiped the profile), or rejects. */
  static verifyPassphrase(pass: string): Promise<string> {
    return invoke('verify_passphrase', { pass });
  }
  static vaultLock(): Promise<void> {
    return invoke('vault_lock');
  }
  static changePassphrase(oldPass: string, newPass: string): Promise<void> {
    return invoke('change_passphrase', { old: oldPass, new: newPass });
  }
  static setDuress(pass: string, duressPass: string | null, wipe: boolean): Promise<void> {
    return invoke('set_duress', { pass, duressPass, wipe });
  }
  static setMaxAttempts(pass: string, max: number): Promise<void> {
    return invoke('set_max_attempts', { pass, max });
  }
  static myCard(): Promise<IdentityCard> {
    return invoke('my_card');
  }
  static myOnion(): Promise<string> {
    return invoke('my_onion');
  }
  static myB32(): Promise<string> {
    return invoke('my_b32');
  }
  static myFingerprint(): Promise<string> {
    return invoke('my_fingerprint');
  }
  static myBundle(): Promise<Bundle> {
    return invoke('my_bundle');
  }
  static getDisplayName(): Promise<string> {
    return invoke('get_display_name');
  }
  static setDisplayName(name: string): Promise<void> {
    return invoke('set_display_name', { name });
  }
  static getRelayAddress(): Promise<string> {
    return invoke('get_relay_address');
  }
  /** Whether an update server destination is baked in; false hides the update UI. */
  static updateConfigured(): Promise<boolean> {
    return invoke('update_configured');
  }
  static getRouterSettings(): Promise<RouterSettings> {
    return invoke('get_router_settings');
  }
  static setRouterSettings(settings: RouterSettings): Promise<void> {
    return invoke('set_router_settings', { settings });
  }
  static setRelayAddress(addr: string): Promise<void> {
    return invoke('set_relay_address', { addr });
  }
  static getRelayInfo(): Promise<RelayInfo> {
    return invoke('get_relay_info');
  }
  static setRelayMode(mode: RelayMode): Promise<void> {
    return invoke('set_relay_mode', { mode });
  }
  /** Contacts whose relay has been silent for a while with mail waiting. */
  static listUnreachableContacts(): Promise<number[]> {
    return invoke('list_unreachable_contacts');
  }
  /** Strip private metadata from local attachments before they are sent. */
  static getAttachmentPrivacy(): Promise<boolean> {
    return invoke('get_attachment_privacy');
  }
  static setAttachmentPrivacy(enabled: boolean): Promise<void> {
    return invoke('set_attachment_privacy', { enabled });
  }
  static addContact(
    onion: string, signPk: string, dhPk: string, name: string, relay?: string,
  ): Promise<number> {
    return invoke('add_contact', { onion, signPk, dhPk, name, relay: relay ?? null });
  }
  static listContacts(): Promise<Contact[]> {
    return invoke('list_contacts');
  }
  static getContact(id: number): Promise<Contact | null> {
    return invoke('get_contact', { id });
  }
  static updateContact(id: number, name: string, trust: number): Promise<void> {
    return invoke('update_contact', { id, name, trust });
  }
  /** `forBoth`: also ask the contact's client to delete the chat and us. */
  static deleteContact(id: number, forBoth = false): Promise<void> {
    return invoke('delete_contact', { id, forBoth });
  }
  static acceptContactRequest(id: number): Promise<void> {
    return invoke('accept_contact_request', { id });
  }
  static declineContactRequest(id: number): Promise<void> {
    return invoke('decline_contact_request', { id });
  }
  static setContactBot(id: number, isBot: boolean): Promise<void> {
    return invoke('set_contact_bot', { id, isBot });
  }
  static resetContactSession(id: number): Promise<void> {
    return invoke('reset_contact_session', { id });
  }
  /** The contact this client runs console commands for, if agent mode is on. */
  static getAgentMode(): Promise<AgentMaster | null> {
    return invoke('get_agent_mode');
  }
  /** `some contact` opens this client's console to them; `null` closes it. */
  static setAgentMode(contactId: number | null): Promise<void> {
    return invoke('set_agent_mode', { contactId });
  }
  /** Master side: one console line, with files to upload before it runs. */
  static sendConsoleCommand(contactId: number, body: string, paths: string[] = []): Promise<number> {
    return invoke('send_console_command', { contactId, body, paths });
  }
  /** Master side: close the agent's console remotely. */
  static sendAgentOff(contactId: number): Promise<number> {
    return invoke('send_agent_off', { contactId });
  }
  static messagePosition(contactId: number | null, groupId: string | null, messageId: number): Promise<number | null> {
    return invoke('message_position', { contactId, groupId, messageId });
  }
  static listMessages(contactId: number, limit = 100, beforeId: number | null = null): Promise<Message[]> {
    return invoke('list_messages', { contactId, limit, beforeId });
  }
  static unreadCount(contactId: number): Promise<number> {
    return invoke('unread_count', { contactId });
  }
  static markRead(contactId: number): Promise<void> {
    return invoke('mark_read', { contactId });
  }
  static deleteMessage(id: number): Promise<void> {
    return invoke('delete_message', { id });
  }
  static sendMessage(contactId: number, body: string, attachments: PendingAttachment[] = [], ttlSecs: number | null = null, replyTo: number | null = null): Promise<number> {
    return invoke('send_message', { contactId, body, attachments, ttlSecs, replyTo });
  }
  static sendMessagePaths(contactId: number, body: string, paths: string[] = [], ttlSecs: number | null = null, replyTo: number | null = null): Promise<number> {
    return invoke('send_message_paths', { contactId, body, paths, ttlSecs, replyTo });
  }
  static sendEdit(contactId: number, messageId: number, newBody: string): Promise<void> {
    return invoke('send_edit', { contactId, messageId, newBody });
  }
  static sendEditGroup(groupId: string, messageId: number, newBody: string): Promise<void> {
    return invoke('send_edit_group', { groupId, messageId, newBody });
  }
  static pressButton(contactId: number, messageId: number, callbackData: string): Promise<void> {
    return invoke('press_button', { contactId, messageId, callbackData });
  }
  static pressGroupButton(groupId: string, messageId: number, callbackData: string): Promise<void> {
    return invoke('press_group_button', { groupId, messageId, callbackData });
  }
  static listAttachments(messageId: number): Promise<Attachment[]> {
    return invoke('list_attachments', { messageId });
  }
  static loadAttachment(attachmentId: number): Promise<string> {
    return invoke('load_attachment', { attachmentId });
  }
  static listMediaContact(contactId: number, limit = 200): Promise<MediaItem[]> {
    return invoke('list_media_contact', { contactId, limit });
  }
  static listMediaGroup(groupId: string, limit = 200): Promise<MediaItem[]> {
    return invoke('list_media_group', { groupId, limit });
  }
  static searchMessages(query: string, contactId: number | null, groupId: string | null, limit = 100): Promise<SearchHit[]> {
    return invoke('search_messages', { query, contactId, groupId, limit });
  }
  static listMuted(): Promise<string[]> {
    return invoke('list_muted');
  }
  static setMuted(targetKey: string, muted: boolean): Promise<void> {
    return invoke('set_muted', { targetKey, muted });
  }
  static exportIdentity(passphrase: string, destPath: string): Promise<void> {
    return invoke('export_identity', { passphrase, destPath });
  }
  static importIdentityToProfile(profile: string, vaultPass: string, backupPath: string, backupPass: string): Promise<void> {
    return invoke('import_identity_to_profile', { profile, vaultPass, backupPath, backupPass });
  }
  static sendTyping(contactId: number | null, groupId: string | null, typing: boolean): Promise<void> {
    return invoke('send_typing', { contactId, groupId, typing });
  }
  static playNotifySound(name?: string | null): Promise<void> {
    return invoke('play_notify_sound', { name: name ?? null });
  }
  static notifyOs(title: string, body: string): Promise<void> {
    return invoke('notify_os', { title, body });
  }
  static notifyProbe(): Promise<string> {
    return invoke('notify_probe');
  }
  static pinChat(contactId: number | null, groupId: string | null): Promise<void> {
    return invoke('pin_chat', { contactId, groupId });
  }
  static unpinChat(contactId: number | null, groupId: string | null): Promise<void> {
    return invoke('unpin_chat', { contactId, groupId });
  }
  static updateTrayBadge(count: number): Promise<void> {
    return invoke('update_tray_badge', { count });
  }
  static forwardMessage(sourceMessageId: number, contactId: number | null, groupId: string | null): Promise<number> {
    return invoke('forward_message', { sourceMessageId, contactId, groupId });
  }
  static listGroups(): Promise<Group[]> {
    return invoke('list_groups');
  }
  static createGroup(name: string, memberContactIds: number[]): Promise<string> {
    return invoke('create_group', { name, memberContactIds });
  }
  static listGroupMembers(groupId: string): Promise<GroupMember[]> {
    return invoke('list_group_members', { groupId });
  }
  static addGroupMember(groupId: string, contactId: number): Promise<void> {
    return invoke('add_group_member', { groupId, contactId });
  }
  static listGroupMessages(groupId: string, limit = 100, beforeId: number | null = null): Promise<Message[]> {
    return invoke('list_group_messages', { groupId, limit, beforeId });
  }
  static sendGroupMessage(groupId: string, body: string, attachments: PendingAttachment[] = [], ttlSecs: number | null = null, replyTo: number | null = null): Promise<number> {
    return invoke('send_group_message', { groupId, body, attachments, ttlSecs, replyTo });
  }
  static sendGroupMessagePaths(groupId: string, body: string, paths: string[] = [], ttlSecs: number | null = null, replyTo: number | null = null): Promise<number> {
    return invoke('send_group_message_paths', { groupId, body, paths, ttlSecs, replyTo });
  }
  static saveAttachment(attachmentId: number, destPath: string): Promise<void> {
    return invoke('save_attachment', { attachmentId, destPath });
  }
  static savePasteTemp(name: string, data: number[]): Promise<string> {
    return invoke('save_paste_temp', { name, data });
  }
  static pasteClipboardImage(): Promise<string | null> {
    return invoke('paste_clipboard_image');
  }
  static deleteGroup(groupId: string): Promise<void> {
    return invoke('delete_group', { groupId });
  }
  static markGroupRead(groupId: string): Promise<void> {
    return invoke('mark_group_read', { groupId });
  }
  static groupUnreadCount(groupId: string): Promise<number> {
    return invoke('group_unread_count', { groupId });
  }
  static pinContactMessage(contactId: number, messageId: number): Promise<void> {
    return invoke('pin_contact_message', { contactId, messageId });
  }
  static unpinContactMessage(contactId: number, messageId: number): Promise<void> {
    return invoke('unpin_contact_message', { contactId, messageId });
  }
  static listPinnedContact(contactId: number): Promise<Message[]> {
    return invoke('list_pinned_contact', { contactId });
  }
  static pinGroupMessage(groupId: string, messageId: number): Promise<void> {
    return invoke('pin_group_message', { groupId, messageId });
  }
  static unpinGroupMessage(groupId: string, messageId: number): Promise<void> {
    return invoke('unpin_group_message', { groupId, messageId });
  }
  static listPinnedGroup(groupId: string): Promise<Message[]> {
    return invoke('list_pinned_group', { groupId });
  }
  static checkUpdate(): Promise<CheckResult> {
    return invoke('check_update');
  }
  static updateInstallsItself(): Promise<boolean> {
    return invoke('update_installs_itself');
  }
  static restartApp(): Promise<void> {
    return invoke('restart_app');
  }
  static updateAsksForRoot(): Promise<boolean> {
    return invoke('update_asks_for_root');
  }
  static installUpdate(): Promise<void> {
    return invoke('install_update');
  }
  static dismissUpdate(version: string): Promise<void> {
    return invoke('dismiss_update', { version });
  }
  static async getContactFolders(): Promise<ContactFolder[]> {
    try {
      const parsed: unknown = JSON.parse((await invoke<string | null>('get_ui_data', { key: 'contact_folders' })) ?? '[]');
      return Array.isArray(parsed) ? parsed as ContactFolder[] : [];
    } catch {
      return [];
    }
  }
  static setContactFolders(folders: ContactFolder[]): Promise<void> {
    return invoke('set_ui_data', { key: 'contact_folders', json: JSON.stringify(folders) });
  }
  /** Avatar picked per key (sign_pk hex → avatar id); unpicked keys get a default. */
  static async getAvatarChoices(): Promise<Record<string, string>> {
    try {
      const parsed: unknown = JSON.parse((await invoke<string | null>('get_ui_data', { key: 'avatars' })) ?? '{}');
      return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed as Record<string, string> : {};
    } catch {
      return {};
    }
  }
  static setAvatarChoices(choices: Record<string, string>): Promise<void> {
    return invoke('set_ui_data', { key: 'avatars', json: JSON.stringify(choices) });
  }
  static getDhtStatus(): Promise<DhtStatus> {
    return invoke('get_dht_status');
  }
  static linkStats(contactId: number): Promise<LinkStats> {
    return invoke('link_stats', { contactId });
  }
  /** Rebuilds tunnels; takes tens of seconds and rejects if it could not. */
  static setLane(lane: Lane): Promise<void> {
    return invoke('set_lane', { lane });
  }
  static getAutoUpdate(): Promise<boolean> {
    return invoke('get_auto_update');
  }
  static setAutoUpdate(enabled: boolean): Promise<void> {
    return invoke('set_auto_update', { enabled });
  }
  /** The card (or any short text) as an SVG QR picture. */
  static qrSvg(text: string): Promise<string> {
    return invoke('qr_svg', { text });
  }
  static currentVersion(): Promise<string> {
    return invoke('current_version');
  }
  static listApkArtifacts(): Promise<ApkArtifacts> {
    return invoke('list_apk_artifacts');
  }
  static downloadApk(arch: string, destPath: string): Promise<void> {
    return invoke('download_apk', { arch, destPath });
  }
  static readPreviousLog(): Promise<string> {
    return invoke('read_previous_log');
  }
  static logSettings(): Promise<{ enabled: boolean; path: string }> {
    return invoke('log_settings');
  }
  static setLogEnabled(enabled: boolean): Promise<void> {
    return invoke('set_log_enabled', { enabled });
  }
  static clearDebugLog(): Promise<void> {
    return invoke('clear_debug_log');
  }
  static readDebugLog(): Promise<string> {
    return invoke('read_debug_log');
  }
  static onEvent(handler: (e: CoreEvent) => void): Promise<UnlistenFn> {
    return listen<CoreEvent>('core_event', (ev) => handler(ev.payload));
  }
  static onBootStatus(handler: (s: BootStatus) => void): Promise<UnlistenFn> {
    return listen<BootStatus>('boot_status', (ev) => handler(ev.payload));
  }
}

/**
 * A shareable contact card.
 *
 * v2 adds the relay the holder receives through. That is what makes more than
 * one relay possible: before it, every client had a single relay setting for
 * everyone, so somebody had to run infrastructure for the whole network. With
 * the relay in the card, a message goes to where its *recipient* collects it.
 *
 * The address slot is the holder's i2p destination. It is regenerated every
 * launch and nothing dials it, so it is informational only — the relay is what
 * delivery actually uses.
 */
export function encodeCard(
  onion: string, signPk: string, dhPk: string, name?: string, relay?: string,
): string {
  const r = relay?.trim() ?? '';
  // v1 stays the format when there is no relay to carry, so cards handed out by
  // clients that have none keep working with older builds.
  if (!r) {
    const base = `gipny:v1:${onion}:${signPk}:${dhPk}`;
    return name ? `${base}:${encodeURIComponent(name)}` : base;
  }
  const base = `gipny:v2:${onion}:${signPk}:${dhPk}:${r}`;
  return name ? `${base}:${encodeURIComponent(name)}` : base;
}

/**
 * Whether a string is a usable i2p address.
 *
 * Two shapes are valid: a `.b32.i2p` hostname, which is exactly 52 base32
 * characters plus the suffix, or a full base64 destination, which is at least
 * 516 characters in i2p's base64 alphabet (`-` and `~` replace `+` and `/`).
 *
 * The previous check was `!onion.endsWith('.i2p') && onion.length < 300`, which
 * accepted "x.i2p" and any 300-character string — a typo became a contact that
 * could never receive anything, with no error at the point of entry.
 */
export function isValidI2pAddress(addr: string): boolean {
  const a = addr.trim();
  if (/^[a-z2-7]{52}\.b32\.i2p$/i.test(a)) return true;
  return /^[A-Za-z0-9~-]{516,}={0,2}$/.test(a);
}

export interface DecodedCard {
  onion: string;
  signPk: string;
  dhPk: string;
  name?: string;
  /** Relay the holder receives through; absent on v1 cards. */
  relay?: string;
}

export function decodeCard(input: string): DecodedCard | null {
  const raw = input.trim();

  const v2 = raw.match(
    /^gipny:v2:([^:]+):([0-9a-fA-F]{64}):([0-9a-fA-F]{64}):([^:]+)(?::(.+))?$/,
  );
  if (v2) {
    return {
      onion: v2[1]!,
      signPk: v2[2]!.toLowerCase(),
      dhPk: v2[3]!.toLowerCase(),
      relay: v2[4]!,
      name: v2[5] ? decodeURIComponent(v2[5]) : undefined,
    };
  }

  const v1 = raw.match(/^gipny:v1:([^:]+):([0-9a-fA-F]{64}):([0-9a-fA-F]{64})(?::(.+))?$/);
  if (!v1) return null;
  return {
    onion: v1[1]!,
    signPk: v1[2]!.toLowerCase(),
    dhPk: v1[3]!.toLowerCase(),
    name: v1[4] ? decodeURIComponent(v1[4]) : undefined,
  };
}