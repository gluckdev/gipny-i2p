import type { Store, ChatTarget } from './state';
import { Api } from './api';
import { h, busy } from './view';

export class ForwardModal {
  el: HTMLElement;

  constructor(private store: Store, private sourceMessageId: number, private close: () => void) {
    const search = h('input', {
      class: 'input',
      placeholder: 'filter chats...',
      autofocus: true,
    }) as HTMLInputElement;
    const list = h('div', { class: 'forward-list' });

    const render = (): void => {
      const q = search.value.trim().toLowerCase();
      const contacts = this.store.contacts.get();
      const groups = this.store.groups.get();
      list.replaceChildren();
      const matches = (name: string): boolean => q === '' || name.toLowerCase().includes(q);
      const filteredGroups = groups.filter((g) => matches(g.name));
      const filteredContacts = contacts.filter((c) => matches(c.name));
      if (filteredGroups.length > 0) {
        list.appendChild(h('div', { class: 'section-label' }, '── groups ──'));
        for (const g of filteredGroups) {
          list.appendChild(this.row(`◫  ${g.name}`, { kind: 'group', id: g.id }));
        }
      }
      if (filteredContacts.length > 0) {
        list.appendChild(h('div', { class: 'section-label' }, '── contacts ──'));
        for (const c of filteredContacts) {
          list.appendChild(this.row(`●  ${c.name}`, { kind: 'contact', id: c.id }));
        }
      }
      if (filteredGroups.length === 0 && filteredContacts.length === 0) {
        list.appendChild(h('div', { class: 'empty', style: { padding: '20px' } }, '── no chats ──'));
      }
    };
    search.addEventListener('input', render);
    render();

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, '── FORWARD TO ──'),
        h('button', { class: 'icon-btn', onClick: () => this.close() }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'field' }, search),
        list,
      ),
    );
  }

  private row(label: string, target: ChatTarget): HTMLElement {
    const btn = h('button', {
      class: 'forward-row',
      onClick: () => busy(btn, async () => {
        try {
          const cid = target.kind === 'contact' ? target.id : null;
          const gid = target.kind === 'group' ? target.id : null;
          await Api.forwardMessage(this.sourceMessageId, cid, gid);
          this.store.showToast('forwarded');
          this.close();
          await this.store.selectChat(target);
        } catch (e) {
          this.store.showToast('forward failed: ' + e, true);
        }
      }),
    }, label) as HTMLButtonElement;
    return btn;
  }
}
