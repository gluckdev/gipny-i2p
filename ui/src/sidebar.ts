import type { Store, ChatTarget } from './state';
import { targetKey, sameTarget } from './state';
import { View, h, short } from './view';
import type { App } from './app';
import { AddContactModal } from './contact';
import { CreateGroupModal } from './group';
import { IdentityModal } from './identity';
import { SettingsModal } from './settings';
import { SecurityModal } from './security';
import { SearchModal } from './search';

export class Sidebar extends View {
  el: HTMLElement;
  private listEl: HTMLElement;

  constructor(private store: Store, private app: App) {
    super();
    this.listEl = h('div', { class: 'contact-list' });
    const footer = h('div', { class: 'sidebar-footer' });
    const profile = store.currentProfile.get();
    this.el = h('div', { class: 'sidebar' },
      h('div', { class: 'sidebar-header' },
        h('div', { class: 'sidebar-title' }, `── ${profile ?? 'chats'} ──`),
        h('div', { class: 'row' },
          h('button', { class: 'icon-btn', title: 'add contact', onClick: () => this.addContact() }, '+'),
          h('button', { class: 'icon-btn', title: 'new group', onClick: () => this.newGroup() }, '◫'),
          h('button', { class: 'icon-btn', title: 'search messages', onClick: () => this.search() }, '⌕'),
          h('button', { class: 'icon-btn', title: 'security', onClick: () => this.security() }, '⛨'),
          h('button', { class: 'icon-btn', title: 'settings', onClick: () => this.settings() }, '⚙'),
          h('button', { class: 'icon-btn', title: 'lock', onClick: () => this.store.lock() }, '🔒'),
        ),
      ),
      this.listEl,
      footer,
    );
    this.sub(store.identity, (id) => {
      footer.replaceChildren();
      if (!id) return;
      footer.appendChild(h('div', { class: 'onion', title: id.onion }, id.onion));
      footer.appendChild(h('button', {
        class: 'icon-btn', title: 'my identity', onClick: () => this.myIdentity(),
      }, '◉'));
    });
    this.sub(store.contacts, () => this.renderList());
    this.sub(store.groups, () => this.renderList(), false);
    this.sub(store.selectedChat, () => this.renderList(), false);
    this.sub(store.peerOnline, () => this.renderList(), false);
    this.sub(store.unread, () => this.renderList(), false);
  }

  private renderList(): void {
    this.listEl.replaceChildren();
    const contacts = this.store.contacts.get();
    const groups = this.store.groups.get();
    const selected = this.store.selectedChat.get();
    const online = this.store.peerOnline.get();
    const unread = this.store.unread.get();

    if (groups.length > 0) {
      this.listEl.appendChild(h('div', { class: 'section-label' }, '── groups ──'));
      for (const g of groups) {
        const target: ChatTarget = { kind: 'group', id: g.id };
        const u = unread.get(targetKey(target)) ?? 0;
        const row = h('div', {
          class: 'contact' + (sameTarget(selected, target) ? ' active' : ''),
          onClick: () => this.store.selectChat(target),
        },
          h('div', { class: 'contact-status group' }, '◫'),
          h('div', { class: 'contact-info' },
            h('div', { class: 'contact-name' }, g.name),
            h('div', { class: 'contact-sub' }, 'group · ' + g.id.slice(0, 10)),
          ),
          u > 0 && h('div', { class: 'contact-badge' }, String(u)),
        );
        this.listEl.appendChild(row);
      }
    }

    this.listEl.appendChild(h('div', { class: 'section-label' }, '── contacts ──'));
    if (contacts.length === 0) {
      this.listEl.appendChild(h('div', { class: 'empty', style: { padding: '20px 16px' } }, '── empty ──'));
    } else {
      for (const c of contacts) {
        const target: ChatTarget = { kind: 'contact', id: c.id };
        const u = unread.get(targetKey(target)) ?? 0;
        const row = h('div', {
          class: 'contact' + (sameTarget(selected, target) ? ' active' : ''),
          onClick: () => this.store.selectChat(target),
        },
          h('div', { class: 'contact-status' + (online.has(c.id) ? ' online' : '') }),
          h('div', { class: 'contact-info' },
            h('div', { class: 'contact-name' }, c.name),
            h('div', { class: 'contact-sub' }, short(c.onion)),
          ),
          u > 0 && h('div', { class: 'contact-badge' }, String(u)),
        );
        this.listEl.appendChild(row);
      }
    }
  }

  private addContact(): void {
    this.app.openModal((close) => new AddContactModal(this.store, close).el);
  }

  private newGroup(): void {
    if (this.store.contacts.get().length === 0) {
      this.store.showToast('add at least one contact first', true);
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

  private security(): void {
    this.app.openModal((close) => new SecurityModal(close).el);
  }

  private search(): void {
    this.app.openModal((close) => new SearchModal(this.store, close).el);
  }
}
