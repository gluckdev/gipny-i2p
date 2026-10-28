import { errorText } from './error-text';
import { t as tr } from './i18n';
import type { Store, ChatTarget } from './state';
import { targetKey, sameTarget } from './state';
import { View, h, avatar, fmtTime } from './view';
import { Api, type Contact, type ContactFolder, type Group } from './api';
import { attachContextMenu, ContextMenu, type MenuItem } from './actions';
import type { App } from './app';
import { AddContactModal } from './contact';
import { CreateGroupModal } from './group';
import { IdentityModal } from './identity';
import { SettingsModal } from './settings';
import { AboutModal } from './about';
import { SearchModal } from './search';
import { icon } from './icons';
import { loadAvatarChoices, onAvatarsChanged } from './avatars';
import { getLocale } from './i18n';

/** Built-in sections that can be folded, remembered per device. */
type Section = 'groups' | 'requests' | 'contacts';
const FOLDED_KEY = 'gipny:folded-sections';

function loadFolded(): Set<Section> {
  try {
    const raw = localStorage.getItem(FOLDED_KEY);
    return new Set(raw ? JSON.parse(raw) as Section[] : []);
  } catch {
    return new Set();
  }
}

/** "14:02" today, "пн" this week, "12.09" before that. */
function fmtWhen(ts: number | null): string {
  if (!ts) return '';
  const d = new Date(ts);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return fmtTime(ts);
  if (now.getTime() - ts < 6 * 24 * 3600 * 1000) return d.toLocaleDateString(getLocale(), { weekday: 'short' });
  return d.toLocaleDateString(getLocale(), { day: '2-digit', month: '2-digit' });
}

export class Sidebar extends View {
  el: HTMLElement;
  private listEl: HTMLElement;
  private filter = '';
  private folders: ContactFolder[] = [];
  private folded = loadFolded();

  constructor(private store: Store, private app: App) {
    super();
    this.listEl = h('div', { class: 'contact-list' });

    const me = h('button', { class: 'sidebar-me', title: tr('identity.my_card'), onClick: () => this.myIdentity() });
    const renderMe = () => {
      const name = store.displayName.get() || store.currentProfile.get() || 'gipny';
      me.replaceChildren(
        avatar(name, store.identity.get()?.card.sign_pk ?? name, 'avatar-sm'),
        h('span', { class: 'sidebar-me-name' }, name),
      );
    };
    renderMe();
    this.sub(store.displayName, renderMe, false);
    this.sub(store.identity, renderMe, false);

    const collapseBtn = h('button', {
      class: 'icon-btn sidebar-collapse',
      title: tr('sidebar.collapse'),
      onClick: () => this.store.toggleSidebar(),
    }, icon(store.sidebarCollapsed.get() ? 'chevronRight' : 'chevronLeft'));
    this.sub(store.sidebarCollapsed, (c) => { collapseBtn.replaceChildren(icon(c ? 'chevronRight' : 'chevronLeft')); }, false);

    const newBtn = h('button', { class: 'icon-btn icon-btn-accent', title: tr('sidebar.new_chat') }, icon('plus'));
    newBtn.addEventListener('click', () => {
      const r = newBtn.getBoundingClientRect();
      ContextMenu.open(r.left, r.bottom + 4, [
        { label: tr('sidebar.new_contact'), onClick: () => this.addContact() },
        { label: tr('sidebar.new_group'), onClick: () => this.newGroup() },
        { label: tr('sidebar.new_folder'), onClick: () => void this.newFolder() },
      ]);
    });

    const search = h('input', { class: 'sidebar-search', type: 'search', placeholder: tr('sidebar.search_placeholder') });
    search.addEventListener('input', () => { this.filter = search.value.trim().toLowerCase(); this.renderList(); });

    this.el = h('div', { class: 'sidebar' },
      h('div', { class: 'sidebar-header' },
        me,
        h('div', { class: 'row sidebar-actions' },
          newBtn,
          h('button', { class: 'icon-btn', title: tr('sidebar.search_messages'), onClick: () => this.search() }, icon('search')),
          h('button', { class: 'icon-btn', title: tr('sidebar.settings'), onClick: () => this.settings() }, icon('settings')),
          collapseBtn,
        ),
      ),
      h('div', { class: 'sidebar-search-wrap' }, search),
      this.listEl,
      h('div', { class: 'sidebar-footer' },
        h('button', { class: 'sidebar-link', onClick: () => this.about() }, icon('shield', 16), tr('sidebar.about')),
        h('button', { class: 'icon-btn', title: tr('sidebar.lock'), onClick: () => this.store.lock() }, icon('lock', 18)),
      ),
    );
    this.sub(store.contacts, () => this.renderList());
    this.sub(store.groups, () => this.renderList(), false);
    this.sub(store.selectedChat, () => this.renderList(), false);
    this.sub(store.peerOnline, () => this.renderList(), false);
    this.sub(store.unread, () => this.renderList(), false);
    void Api.getContactFolders().then((f) => { this.folders = f; this.renderList(); });
    this.subs.push(onAvatarsChanged(() => { renderMe(); this.renderList(); }));
    void loadAvatarChoices();
  }

  // ── list ────────────────────────────────────────────────────────────────

  private matches(name: string): boolean {
    return !this.filter || name.toLowerCase().includes(this.filter);
  }

  private renderList(): void {
    this.listEl.replaceChildren();
    const contacts = this.store.contacts.get();
    const groups = this.store.groups.get().filter((g) => this.matches(g.name));

    if (groups.length > 0) {
      this.section('groups', tr('forward.groups'), groups.length, groups.map((g) => this.groupRow(g)));
    }

    const requests = contacts.filter((c) => c.request === 'incoming' && c.trust !== 2 && this.matches(c.name));
    if (requests.length > 0) {
      this.section('requests', tr('sidebar.requests'), requests.length, requests.map((c) => this.requestRow(c)));
    }

    const known = contacts.filter((c) => c.request !== 'incoming');
    const byId = new Map(known.map((c) => [c.id, c]));
    const inFolder = new Set<number>();
    for (const f of this.folders) {
      const members = f.contacts.map((id) => byId.get(id)).filter((c): c is Contact => !!c);
      members.forEach((c) => inFolder.add(c.id));
      const shown = members.filter((c) => this.matches(c.name));
      if (this.filter && shown.length === 0) continue;
      this.folderBlock(f, shown);
    }

    const rest = known.filter((c) => !inFolder.has(c.id) && this.matches(c.name));
    if (known.length === 0) {
      this.listEl.appendChild(h('div', { class: 'list-empty' },
        h('div', { class: 'list-empty-title' }, tr('sidebar.no_contacts_yet')),
        h('div', { class: 'list-empty-sub' }, tr('sidebar.ask_for_someone_s_contact_card_and_add')),
        h('button', { class: 'btn btn-sm', onClick: () => this.addContact() }, tr('contact.add_contact')),
      ));
    } else if (rest.length > 0 || !this.filter) {
      const title = this.folders.length > 0 ? tr('sidebar.unfiled') : tr('forward.contacts');
      this.section('contacts', title, rest.length, rest.map((c) => this.contactRow(c)));
    }
    if (this.filter && this.listEl.childElementCount === 0) {
      this.listEl.appendChild(h('div', { class: 'list-empty' }, h('div', { class: 'list-empty-sub' }, tr('sidebar.nothing_found'))));
    }
  }

  private header(title: string, count: number, folded: boolean, onToggle: () => void, menu?: () => MenuItem[]): HTMLElement {
    const el = h('div', { class: 'section-label' + (folded ? ' folded' : ''), role: 'button', onClick: onToggle },
      h('span', { class: 'section-chevron' }, '▾'),
      h('span', { class: 'section-title' }, title),
      h('span', { class: 'section-count' }, String(count)),
    );
    if (menu) {
      const more = h('button', { class: 'icon-btn section-more', title: tr('sidebar.folder_actions') }, icon('more', 16));
      more.addEventListener('click', (e) => {
        e.stopPropagation();
        const r = more.getBoundingClientRect();
        ContextMenu.open(r.left, r.bottom + 4, menu());
      });
      el.appendChild(more);
      attachContextMenu(el, () => menu());
    }
    return el;
  }

  private section(key: Section, title: string, count: number, rows: HTMLElement[]): void {
    // A search shows everything it found, folded or not.
    const folded = this.folded.has(key) && !this.filter;
    this.listEl.appendChild(this.header(title, count, folded, () => {
      if (this.folded.has(key)) this.folded.delete(key); else this.folded.add(key);
      try { localStorage.setItem(FOLDED_KEY, JSON.stringify([...this.folded])); } catch { /* per-device nicety */ }
      this.renderList();
    }));
    if (!folded) rows.forEach((r) => this.listEl.appendChild(r));
  }

  private folderBlock(f: ContactFolder, members: Contact[]): void {
    const folded = f.collapsed && !this.filter;
    this.listEl.appendChild(this.header(f.name, members.length, folded, () => {
      f.collapsed = !f.collapsed;
      this.saveFolders();
    }, () => [
      { label: tr('sidebar.rename'), onClick: () => void this.renameFolder(f) },
      { label: tr('sidebar.delete_folder'), danger: true, onClick: () => void this.deleteFolder(f) },
    ]));
    if (folded) return;
    if (members.length === 0) {
      this.listEl.appendChild(h('div', { class: 'folder-empty' }, tr('sidebar.empty_move_a_contact_here_using_their_menu')));
    }
    members.forEach((c) => this.listEl.appendChild(this.contactRow(c)));
  }

  private rowShell(target: ChatTarget, pinned: boolean, pic: HTMLElement, name: string, sub: string, when: number | null): HTMLElement {
    const u = this.store.unread.get().get(targetKey(target)) ?? 0;
    return h('div', {
      class: 'contact' + (sameTarget(this.store.selectedChat.get(), target) ? ' active' : '') + (pinned ? ' pinned' : '') + (u > 0 ? ' has-unread' : ''),
      onClick: () => this.store.selectChat(target),
    },
      pic,
      h('div', { class: 'contact-info' },
        h('div', { class: 'contact-top' },
          h('div', { class: 'contact-name' }, name),
          h('div', { class: 'contact-when' }, fmtWhen(when)),
        ),
        h('div', { class: 'contact-bottom' },
          h('div', { class: 'contact-sub' }, sub),
          pinned && h('div', { class: 'contact-pin', title: tr('sidebar.pinned') }, '📌'),
          u > 0 && h('div', { class: 'contact-badge' }, u > 99 ? '99+' : String(u)),
        ),
      ),
    );
  }

  private groupRow(g: Group): HTMLElement {
    const target: ChatTarget = { kind: 'group', id: g.id };
    const pinned = g.pinned_at != null;
    const row = this.rowShell(target, pinned, avatar(g.name, g.id, 'avatar-group', false), g.name, tr('sidebar.group'), g.last_message_at ?? null);
    attachContextMenu(row, () => this.chatMenu(target, pinned));
    return row;
  }

  private contactRow(c: Contact): HTMLElement {
    const target: ChatTarget = { kind: 'contact', id: c.id };
    const pinned = c.pinned_at != null;
    const online = this.store.peerOnline.get().has(c.id);
    const lost = this.store.lostForDays(c);
    const sub = c.request === 'outgoing' ? tr('sidebar.awaiting_approval')
      : online ? tr('sidebar.online')
      : lost != null ? tr('sidebar.unreachable_for_days', { p0: (lost) })
      : c.is_bot ? tr('sidebar.bot')
      : c.trust === 1 ? tr('sidebar.verified_contact')
      : tr('sidebar.offline');
    const pic = h('div', { class: 'avatar-wrap' }, avatar(c.name, c.sign_pk), online && h('span', { class: 'presence' }));
    const row = this.rowShell(target, pinned, pic, c.name, sub, c.last_message_at);
    if (online) row.querySelector('.contact-sub')?.classList.add('online');
    attachContextMenu(row, () => [...this.chatMenu(target, pinned), ...this.folderMenu(c)]);
    return row;
  }

  private requestRow(c: Contact): HTMLElement {
    const act = (op: () => Promise<void>, done: string) => (ev: Event) => {
      ev.stopPropagation();
      op().then(() => {
        this.store.showToast(done);
        return this.store.refreshContacts();
      }).catch((e: unknown) => this.store.showToast(errorText(e), true));
    };
    return h('div', { class: 'contact request' },
      avatar(c.name, c.sign_pk),
      h('div', { class: 'contact-info' },
        h('div', { class: 'contact-name' }, c.name),
        h('div', { class: 'contact-sub' }, tr('sidebar.wants_to_add_you_as_a_contact')),
        h('div', { class: 'request-actions' },
          h('button', { class: 'btn btn-sm', onClick: act(() => Api.acceptContactRequest(c.id), tr('sidebar.added', { p0: (c.name) })) }, tr('sidebar.accept')),
          h('button', { class: 'btn btn-sm btn-ghost', onClick: act(() => Api.declineContactRequest(c.id), tr('sidebar.request_declined')) }, tr('sidebar.decline')),
          h('button', {
            class: 'icon-btn', title: tr('sidebar.lock'),
            onClick: act(() => Api.updateContact(c.id, c.name, 2), tr('sidebar.blocked', { p0: (c.name) })),
          }, icon('block', 16)),
        ),
      ),
    );
  }

  private chatMenu(target: ChatTarget, pinned: boolean): MenuItem[] {
    return [{
      label: pinned ? tr('chat.unpin') : tr('sidebar.pin_to_top'),
      onClick: () => {
        const op = pinned ? this.store.unpinChat(target) : this.store.pinChat(target);
        op.catch((e: unknown) => this.store.showToast(errorText(e), true));
      },
    }];
  }

  // ── folders ─────────────────────────────────────────────────────────────

  private folderMenu(c: Contact): MenuItem[] {
    const current = this.folders.find((f) => f.contacts.includes(c.id));
    const items: MenuItem[] = this.folders
      .filter((f) => f !== current)
      .map((f) => ({ label: tr('sidebar.move_to', { p0: (f.name) }), onClick: () => this.moveTo(c.id, f) }));
    items.push({ label: tr('sidebar.new_folder_2'), onClick: () => void this.newFolder(c.id) });
    if (current) items.push({ label: tr('sidebar.remove_from', { p0: (current.name) }), onClick: () => this.moveTo(c.id, null) });
    return items;
  }

  private moveTo(contactId: number, folder: ContactFolder | null): void {
    for (const f of this.folders) f.contacts = f.contacts.filter((id) => id !== contactId);
    if (folder) {
      folder.contacts.push(contactId);
      folder.collapsed = false;
    }
    this.saveFolders();
  }

  private async newFolder(withContact?: number): Promise<void> {
    const name = await this.app.prompt(tr('sidebar.new_folder'), tr('sidebar.name'), '', tr('group.create'));
    if (!name) return;
    const folder: ContactFolder = { id: `f-${Date.now().toString(36)}`, name, collapsed: false, contacts: [] };
    this.folders.push(folder);
    if (withContact != null) this.moveTo(withContact, folder); else this.saveFolders();
  }

  private async renameFolder(f: ContactFolder): Promise<void> {
    const name = await this.app.prompt(tr('sidebar.rename_folder'), tr('sidebar.name'), f.name);
    if (!name) return;
    f.name = name;
    this.saveFolders();
  }

  private async deleteFolder(f: ContactFolder): Promise<void> {
    if (!await this.app.confirm(tr('sidebar.delete_folder'), tr('sidebar.folder_will_be_removed_contacts_stay_in_your', { p0: (f.name) }), true)) return;
    this.folders = this.folders.filter((x) => x !== f);
    this.saveFolders();
  }

  private saveFolders(): void {
    this.renderList();
    Api.setContactFolders(this.folders).catch((e: unknown) => this.store.showToast(errorText(e), true));
  }

  // ── modals ──────────────────────────────────────────────────────────────

  private addContact(): void {
    this.app.openModal((close) => new AddContactModal(this.store, close).el);
  }

  private newGroup(): void {
    if (this.store.contacts.get().length === 0) {
      this.store.showToast(tr('sidebar.need_contact_for_folder'), true);
      return;
    }
    this.app.openModal((close) => new CreateGroupModal(this.store, close).el);
  }

  private myIdentity(): void {
    this.app.openModal((close) => new IdentityModal(this.store, close).el);
  }

  private settings(): void {
    this.app.openModal((close) => new SettingsModal(this.store, this.app, close).el);
  }

  private about(): void {
    this.app.openModal((close) => new AboutModal(close).el);
  }

  private search(): void {
    this.app.openModal((close) => new SearchModal(this.store, close).el);
  }
}
