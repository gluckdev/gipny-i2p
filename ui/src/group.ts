import { Api } from './api';
import type { Store } from './state';
import { targetKey } from './state';
import { h, busy, short } from './view';
import type { App } from './app';

export class GroupModal {
  el: HTMLElement;
  private unsub: (() => void) | null = null;
  private unsubContacts: (() => void) | null = null;

  constructor(store: Store, app: App, groupId: string, close: () => void) {
    const g = store.groups.get().find((x) => x.id === groupId);
    if (!g) { this.el = h('div'); close(); return; }

    const list = h('div', { class: 'stack', style: { gap: '6px' } });
    const countLabel = h('div', { class: 'card-label', style: { marginTop: '12px' } }, 'members (0)');
    const picker = h('select', { class: 'input' }) as HTMLSelectElement;
    const addBtn = h('button', { class: 'btn btn-amber' }, '[ + ADD ]') as HTMLButtonElement;
    const addErr = h('div', { class: 'err' });

    const renderPicker = (): void => {
      const memberSigns = new Set((store.groupMembers.get().get(groupId) ?? []).map((m) => m.sign_pk));
      const candidates = store.contacts.get().filter((c) => !memberSigns.has(c.sign_pk));
      picker.replaceChildren();
      const opt0 = h('option', { value: '' }, '-- pick contact / bot --');
      picker.appendChild(opt0);
      for (const c of candidates) {
        picker.appendChild(h('option', { value: String(c.id) }, c.name));
      }
      addBtn.disabled = candidates.length === 0;
    };

    const renderMembers = (): void => {
      const members = store.groupMembers.get().get(groupId) ?? [];
      list.replaceChildren();
      countLabel.textContent = `members (${members.length})`;
      for (const m of members) {
        list.appendChild(h('div', {
          class: 'contact',
          style: { cursor: 'default' },
        },
          h('div', { class: 'contact-status' + (m.is_self ? ' online' : '') }),
          h('div', { class: 'contact-info' },
            h('div', { class: 'contact-name' }, m.name, m.is_self ? ' (me)' : ''),
            h('div', { class: 'contact-sub' }, short(m.onion)),
          ),
        ));
      }
      renderPicker();
    };

    this.unsub = store.groupMembers.subscribe(renderMembers, true);
    this.unsubContacts = store.contacts.subscribe(() => renderPicker(), false);

    if (!store.groupMembers.get().has(groupId)) {
      Api.listGroupMembers(groupId).then((members) => {
        store.groupMembers.update((m) => { const n = new Map(m); n.set(groupId, members); return n; });
      }).catch(() => {});
    }

    addBtn.addEventListener('click', async () => {
      addErr.textContent = '';
      const cid = parseInt(picker.value, 10);
      if (!Number.isFinite(cid)) { addErr.textContent = 'pick a contact'; return; }
      addBtn.disabled = true;
      const orig = addBtn.textContent;
      addBtn.textContent = '[ ... ]';
      try {
        await store.addGroupMember(groupId, cid);
        store.showToast('member added');
      } catch (e) {
        addErr.textContent = String(e);
      } finally {
        addBtn.disabled = false;
        addBtn.textContent = orig;
      }
    });

    const closeWrapped = () => {
      this.unsub?.(); this.unsub = null;
      this.unsubContacts?.(); this.unsubContacts = null;
      close();
    };

    const isMutedNow = store.muted.get().has(targetKey({ kind: 'group', id: groupId }));
    const muteCb = h('input', { type: 'checkbox', class: 'cb', checked: isMutedNow }) as HTMLInputElement;
    muteCb.addEventListener('change', () => {
      store.toggleMute({ kind: 'group', id: groupId }, muteCb.checked).catch((e) => {
        store.showToast('mute failed: ' + String(e), true);
      });
    });

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, `── group :: ${g.name} ──`),
        h('button', { class: 'icon-btn', onClick: closeWrapped }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'card-label' }, 'id'),
        h('div', { class: 'card-block' }, g.id),
        h('label', { class: 'cb-row', style: { marginTop: '10px' } }, muteCb, h('span', null, 'mute notifications')),
        countLabel,
        list,
        h('div', { class: 'divider-text' }, 'add member'),
        h('div', { class: 'field' }, picker),
        h('div', { class: 'row', style: { gap: '8px', marginTop: '6px' } }, addBtn),
        addErr,
      ),
      h('div', { class: 'modal-footer' },
        h('button', {
          class: 'btn btn-danger',
          onClick: async () => {
            const ok = await app.confirm('leave group', `delete local copy of "${g.name}"? others in the group will still have it.`, true);
            if (!ok) return;
            await store.deleteGroup(groupId);
            closeWrapped();
          },
        }, '[ LEAVE (local) ]'),
        h('div', { class: 'grow' }),
        h('button', { class: 'btn btn-ghost', onClick: closeWrapped }, '[ close ]'),
      ),
    );
  }
}

export class CreateGroupModal {
  el: HTMLElement;
  private selected = new Set<number>();
  constructor(store: Store, close: () => void) {
    const nameI = h('input', { class: 'input', placeholder: 'group name', autofocus: true, maxlength: '64' }) as HTMLInputElement;
    const err = h('div', { class: 'err', style: { minHeight: '14px', marginTop: '8px' } });
    const list = h('div', { class: 'stack', style: { gap: '4px', maxHeight: '300px', overflowY: 'auto' } });

    for (const c of store.contacts.get()) {
      if (c.trust === 2) continue;
      const cb = h('input', { type: 'checkbox' }) as HTMLInputElement;
      cb.addEventListener('change', () => {
        if (cb.checked) this.selected.add(c.id);
        else this.selected.delete(c.id);
      });
      list.appendChild(h('label', { class: 'chk' },
        cb, h('span', { class: 'box' }),
        h('span', null, `${c.name}  `, h('span', { class: 'hint', style: { display: 'inline' } }, short(c.onion, 18))),
      ));
    }

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, '── new group ──'),
        h('button', { class: 'icon-btn', onClick: close }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'field' }, h('label', null, 'name'), nameI),
        h('div', { class: 'divider-text' }, 'members'),
        list,
        err,
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: close }, '[ cancel ]'),
        (() => {
          const b = h('button', {
            class: 'btn',
            onClick: () => busy(b, async () => {
              const name = nameI.value.trim();
              if (!name) { err.textContent = 'name required'; return; }
              if (this.selected.size === 0) { err.textContent = 'select ≥1 member'; return; }
              try {
                await store.createGroup(name, [...this.selected]);
                store.showToast(`group "${name}" created`);
                close();
              } catch (e) { err.textContent = 'err: ' + String(e); }
            }),
          }, '[ CREATE ]') as HTMLButtonElement;
          return b;
        })(),
      ),
    );
  }
}
