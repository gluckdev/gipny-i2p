import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface VaultStatus { exists: boolean; unlocked: boolean; }

export interface IdentityCard { sign_pk: string; dh_pk: string; }

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
}

export interface Button { text: string; callback_data: string; }

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

export interface UpdateInfo {
  version: string;
  notes: string;
  target_key: string;
  size: number;
}

export interface ApkArtifact { arch: string; size: number; }
export interface ApkArtifacts { version: string; artifacts: ApkArtifact[]; }

export type CoreEvent =
  | { IncomingMessage: { contact_id: number | null; group_id: string | null; sender_sign_pk: string | null; message_id: number; body: string; sent_at: number; notify_sound: string | null } }
  | { MessageEdited: { message_id: number; body: string; buttons: Button[][] | null } }
  | { MessagePinned: { contact_id: number | null; group_id: string | null; message_id: number } }
  | { MessageUnpinned: { contact_id: number | null; group_id: string | null; message_id: number } }
  | { MessageSent: { message_id: number } }
  | { MessageDelivered: { message_id: number } }
  | { Typing: { contact_id: number | null; group_id: string | null; sender_sign_pk: string | null; typing: boolean } }
  | { ContactAdded: { contact_id: number } }
  | { ContactUpdated: { contact_id: number } }
  | { GroupUpdated: { group_id: string } }
  | { PeerOnline: { contact_id: number } }
  | { PeerOffline: { contact_id: number } }
  | { UpdateAvailable: { version: string; notes: string; target_key: string; size: number } }
  | { UpdateProgress: { downloaded: number; total: number; pct: number } }
  | { UpdateReady: { path: string } }
  | { UpdateFailed: { reason: string } };

export interface PendingAttachment { name: string; data: string; }

export class Api {
  static listProfiles(): Promise<string[]> {
    return invoke('list_profiles');
  }
  static deleteProfile(profile: string): Promise<void> {
    return invoke('delete_profile', { profile });
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
  static setRelayAddress(addr: string): Promise<void> {
    return invoke('set_relay_address', { addr });
  }
  static addContact(onion: string, signPk: string, dhPk: string, name: string): Promise<number> {
    return invoke('add_contact', { onion, signPk, dhPk, name });
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
  static deleteContact(id: number): Promise<void> {
    return invoke('delete_contact', { id });
  }
  static setContactBot(id: number, isBot: boolean): Promise<void> {
    return invoke('set_contact_bot', { id, isBot });
  }
  static resetContactSession(id: number): Promise<void> {
    return invoke('reset_contact_session', { id });
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
  static checkUpdate(): Promise<UpdateInfo | null> {
    return invoke('check_update');
  }
  static installUpdate(): Promise<void> {
    return invoke('install_update');
  }
  static dismissUpdate(version: string): Promise<void> {
    return invoke('dismiss_update', { version });
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
  static readDebugLog(): Promise<string> {
    return invoke('read_debug_log');
  }
  static onEvent(handler: (e: CoreEvent) => void): Promise<UnlistenFn> {
    return listen<CoreEvent>('core_event', (ev) => handler(ev.payload));
  }
  static onBootStatus(handler: (s: string) => void): Promise<UnlistenFn> {
    return listen<string>('boot_status', (ev) => handler(ev.payload));
  }
}

export function encodeCard(onion: string, signPk: string, dhPk: string, name?: string): string {
  const base = `gipny:v1:${onion}:${signPk}:${dhPk}`;
  return name ? `${base}:${encodeURIComponent(name)}` : base;
}

export function decodeCard(input: string): { onion: string; signPk: string; dhPk: string; name?: string } | null {
  const m = input.trim().match(/^gipny:v1:([^:]+):([0-9a-fA-F]{64}):([0-9a-fA-F]{64})(?::(.+))?$/);
  if (!m) return null;
  return {
    onion: m[1]!,
    signPk: m[2]!.toLowerCase(),
    dhPk: m[3]!.toLowerCase(),
    name: m[4] ? decodeURIComponent(m[4]) : undefined,
  };
}