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
  if (now.getTime() - ts < 6 * 24 * 3600 * 1000) return d.toLocaleDateString('ru-RU', { weekday: 'short' });
  return d.toLocaleDateString('ru-RU', { day: '2-digit', month: '2-digit' });
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

    const me = h('button', { class: 'sidebar-me', title: 'Моя карточка', onClick: () => this.myIdentity() });
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
      title: 'Свернуть панель',
      onClick: () => this.store.toggleSidebar(),
    }, icon(store.sidebarCollapsed.get() ? 'chevronRight' : 'chevronLeft'));
    this.sub(store.sidebarCollapsed, (c) => { collapseBtn.replaceChildren(icon(c ? 'chevronRight' : 'chevronLeft')); }, false);

    const newBtn = h('button', { class: 'icon-btn icon-btn-accent', title: 'Создать' }, icon('plus'));
    newBtn.addEventListener('click', () => {
      const r = newBtn.getBoundingClientRect();
      ContextMenu.open(r.left, r.bottom + 4, [
        { label: 'Добавить контакт', onClick: () => this.addContact() },
        { label: 'Новая группа', onClick: () => this.newGroup() },
        { label: 'Новая папка', onClick: () => void this.newFolder() },
      ]);
    });

    const search = h('input', { class: 'sidebar-search', type: 'search', placeholder: 'Поиск контактов' });
    search.addEventListener('input', () => { this.filter = search.value.trim().toLowerCase(); this.renderList(); });

    this.el = h('div', { class: 'sidebar' },
      h('div', { class: 'sidebar-header' },
        me,
        h('div', { class: 'row sidebar-actions' },
          newBtn,
          h('button', { class: 'icon-btn', title: 'Поиск по сообщениям', onClick: () => this.search() }, icon('search')),
          h('button', { class: 'icon-btn', title: 'Настройки', onClick: () => this.settings() }, icon('settings')),
          collapseBtn,
        ),
      ),
      h('div', { class: 'sidebar-search-wrap' }, search),
      this.listEl,
      h('div', { class: 'sidebar-footer' },
        h('button', { class: 'sidebar-link', onClick: () => this.about() }, icon('shield', 16), 'О gipny и безопасности'),
        h('button', { class: 'icon-btn', title: 'Заблокировать', onClick: () => this.store.lock() }, icon('lock', 18)),
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
      this.section('groups', 'Группы', groups.length, groups.map((g) => this.groupRow(g)));
    }

    const requests = contacts.filter((c) => c.request === 'incoming' && c.trust !== 2 && this.matches(c.name));
    if (requests.length > 0) {
      this.section('requests', 'Запросы', requests.length, requests.map((c) => this.requestRow(c)));
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
        h('div', { class: 'list-empty-title' }, 'Пока нет контактов'),
        h('div', { class: 'list-empty-sub' }, 'Попросите карточку у собеседника и добавьте её — у него появится ваш запрос.'),
        h('button', { class: 'btn btn-sm', onClick: () => this.addContact() }, 'Добавить контакт'),
      ));
    } else if (rest.length > 0 || !this.filter) {
      const title = this.folders.length > 0 ? 'Без папки' : 'Контакты';
      this.section('contacts', title, rest.length, rest.map((c) => this.contactRow(c)));
    }
    if (this.filter && this.listEl.childElementCount === 0) {
      this.listEl.appendChild(h('div', { class: 'list-empty' }, h('div', { class: 'list-empty-sub' }, 'Ничего не найдено')));
    }
  }

  private header(title: string, count: number, folded: boolean, onToggle: () => void, menu?: () => MenuItem[]): HTMLElement {
    const el = h('div', { class: 'section-label' + (folded ? ' folded' : ''), role: 'button', onClick: onToggle },
      h('span', { class: 'section-chevron' }, '▾'),
      h('span', { class: 'section-title' }, title),
      h('span', { class: 'section-count' }, String(count)),
    );
    if (menu) {
      const more = h('button', { class: 'icon-btn section-more', title: 'Действия с папкой' }, icon('more', 16));
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
      { label: 'Переименовать', onClick: () => void this.renameFolder(f) },
      { label: 'Удалить папку', danger: true, onClick: () => void this.deleteFolder(f) },
    ]));
    if (folded) return;
    if (members.length === 0) {
      this.listEl.appendChild(h('div', { class: 'folder-empty' }, 'Пусто — перенесите сюда контакт через его меню'));
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
          pinned && h('div', { class: 'contact-pin', title: 'Закреплён' }, '📌'),
          u > 0 && h('div', { class: 'contact-badge' }, u > 99 ? '99+' : String(u)),
        ),
      ),
    );
  }

  private groupRow(g: Group): HTMLElement {
    const target: ChatTarget = { kind: 'group', id: g.id };
    const pinned = g.pinned_at != null;
    const row = this.rowShell(target, pinned, avatar(g.name, g.id, 'avatar-group', false), g.name, 'Группа', g.last_message_at ?? null);
    attachContextMenu(row, () => this.chatMenu(target, pinned));
    return row;
  }

  private contactRow(c: Contact): HTMLElement {
    const target: ChatTarget = { kind: 'contact', id: c.id };
    const pinned = c.pinned_at != null;
    const online = this.store.peerOnline.get().has(c.id);
    const lost = this.store.lostForDays(c);
    const sub = c.request === 'outgoing' ? 'Ждёт подтверждения'
      : online ? 'В сети'
      : lost != null ? `Нет связи ${lost} дн.`
      : c.is_bot ? 'Бот'
      : c.trust === 1 ? 'Проверенный контакт'
      : 'Не в сети';
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
      }).catch((e: unknown) => this.store.showToast(String(e), true));
    };
    return h('div', { class: 'contact request' },
      avatar(c.name, c.sign_pk),
      h('div', { class: 'contact-info' },
        h('div', { class: 'contact-name' }, c.name),
        h('div', { class: 'contact-sub' }, 'Хочет добавить вас в контакты'),
        h('div', { class: 'request-actions' },
          h('button', { class: 'btn btn-sm', onClick: act(() => Api.acceptContactRequest(c.id), `${c.name} добавлен`) }, 'Принять'),
          h('button', { class: 'btn btn-sm btn-ghost', onClick: act(() => Api.declineContactRequest(c.id), 'Запрос отклонён') }, 'Отклонить'),
          h('button', {
            class: 'icon-btn', title: 'Заблокировать',
            onClick: act(() => Api.updateContact(c.id, c.name, 2), `${c.name} заблокирован`),
          }, icon('block', 16)),
        ),
      ),
    );
  }

  private chatMenu(target: ChatTarget, pinned: boolean): MenuItem[] {
    return [{
      label: pinned ? 'Открепить' : 'Закрепить наверху',
      onClick: () => {
        const op = pinned ? this.store.unpinChat(target) : this.store.pinChat(target);
        op.catch((e: unknown) => this.store.showToast(String(e), true));
      },
    }];
  }

  // ── folders ─────────────────────────────────────────────────────────────

  private folderMenu(c: Contact): MenuItem[] {
    const current = this.folders.find((f) => f.contacts.includes(c.id));
    const items: MenuItem[] = this.folders
      .filter((f) => f !== current)
      .map((f) => ({ label: `В папку «${f.name}»`, onClick: () => this.moveTo(c.id, f) }));
    items.push({ label: 'В новую папку…', onClick: () => void this.newFolder(c.id) });
    if (current) items.push({ label: `Убрать из «${current.name}»`, onClick: () => this.moveTo(c.id, null) });
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
    const name = await this.app.prompt('Новая папка', 'Название', '', 'Создать');
    if (!name) return;
    const folder: ContactFolder = { id: `f-${Date.now().toString(36)}`, name, collapsed: false, contacts: [] };
    this.folders.push(folder);
    if (withContact != null) this.moveTo(withContact, folder); else this.saveFolders();
  }

  private async renameFolder(f: ContactFolder): Promise<void> {
    const name = await this.app.prompt('Переименовать папку', 'Название', f.name);
    if (!name) return;
    f.name = name;
    this.saveFolders();
  }

  private async deleteFolder(f: ContactFolder): Promise<void> {
    if (!await this.app.confirm('Удалить папку', `Папка «${f.name}» исчезнет, контакты останутся в списке.`, true)) return;
    this.folders = this.folders.filter((x) => x !== f);
    this.saveFolders();
  }

  private saveFolders(): void {
    this.renderList();
    Api.setContactFolders(this.folders).catch((e: unknown) => this.store.showToast(String(e), true));
  }

  // ── modals ──────────────────────────────────────────────────────────────

  private addContact(): void {
    this.app.openModal((close) => new AddContactModal(this.store, close).el);
  }

  private newGroup(): void {
    if (this.store.contacts.get().length === 0) {
      this.store.showToast('Сначала добавьте хотя бы один контакт', true);
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
