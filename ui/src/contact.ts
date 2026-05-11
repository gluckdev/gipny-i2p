import { Api, decodeCard } from './api';
import type { Store } from './state';
import { targetKey } from './state';
import { h, fmtFp } from './view';
import type { App } from './app';

export class ContactModal {
  el: HTMLElement;
  constructor(store: Store, app: App, contactId: number, close: () => void) {
    const c = store.contacts.get().find((x) => x.id === contactId);
    if (!c) { this.el = h('div'); close(); return; }

    const nameI = h('input', { class: 'input', value: c.name });
    const trustSel = h('select', { class: 'input' },
      h('option', { value: '0', selected: c.trust === 0 }, 'unverified'),
      h('option', { value: '1', selected: c.trust === 1 }, 'verified'),
      h('option', { value: '2', selected: c.trust === 2 }, 'blocked'),
    );
    const botCb = h('input', { type: 'checkbox', class: 'cb', checked: !!c.is_bot }) as HTMLInputElement;
    const isMutedNow = store.muted.get().has(targetKey({ kind: 'contact', id: c.id }));
    const muteCb = h('input', { type: 'checkbox', class: 'cb', checked: isMutedNow }) as HTMLInputElement;

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, '── contact ──'),
        h('button', { class: 'icon-btn', onClick: close }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'field' }, h('label', null, 'name'), nameI),
        h('div', { class: 'field' }, h('label', null, 'trust'), trustSel),
        h('label', { class: 'cb-row' }, botCb, h('span', null, 'mark as bot (amber bubbles in chat)')),
        h('label', { class: 'cb-row' }, muteCb, h('span', null, 'mute notifications')),
        h('div', { class: 'card-label', style: { marginTop: '10px' } }, 'onion'),
        h('div', { class: 'card-block' }, c.onion),
        h('div', { class: 'card-label', style: { marginTop: '10px' } }, 'fingerprint'),
        h('div', { class: 'card-block fp' }, fmtFp(c.dh_pk)),
        h('div', { class: 'hint', style: { marginTop: '8px' } }, 'verify fingerprint out-of-band before marking verified'),
        h('div', { class: 'divider-text' }, 'troubleshoot'),
        h('button', {
          class: 'btn btn-amber',
          style: { width: '100%' },
          onClick: async () => {
            const ok = await app.confirm(
              'reset session',
              'force a fresh X3DH handshake with this contact? next message you both send will re-establish the ratchet. use this if messages are not arriving.',
            );
            if (!ok) return;
            try {
              await Api.resetContactSession(c.id);
              store.showToast('session reset; ratchet will rebuild on next exchange');
            } catch (e) {
              store.showToast('reset failed: ' + String(e), true);
            }
          },
        }, '[ RESET SESSION ]'),
      ),
      h('div', { class: 'modal-footer' },
        h('button', {
          class: 'btn btn-danger',
          onClick: async () => {
            const ok = await app.confirm('delete contact', `delete "${c.name}"?`, true);
            if (!ok) return;
            await Api.deleteContact(c.id);
            await store.refreshContacts();
            const sel = store.selectedChat.get();
            if (sel?.kind === 'contact' && sel.id === c.id) store.selectedChat.set(null);
            close();
          },
        }, '[ DELETE ]'),
        h('div', { class: 'grow' }),
        h('button', { class: 'btn btn-ghost', onClick: close }, '[ cancel ]'),
        h('button', {
          class: 'btn',
          onClick: async () => {
            await Api.updateContact(c.id, nameI.value.trim(), parseInt((trustSel as HTMLSelectElement).value) || 0);
            if (botCb.checked !== !!c.is_bot) await Api.setContactBot(c.id, botCb.checked);
            if (muteCb.checked !== isMutedNow) await store.toggleMute({ kind: 'contact', id: c.id }, muteCb.checked);
            await store.refreshContacts();
            store.showToast('saved');
            close();
          },
        }, '[ SAVE ]'),
      ),
    );
  }
}

export class AddContactModal {
  el: HTMLElement;
  constructor(store: Store, close: () => void) {
    const pasteI = h('textarea', {
      class: 'textarea',
      placeholder: 'paste contact card\n(gipny:v1:<onion>:<sign_pk_hex>:<dh_pk_hex>[:name])',
      rows: '4',
    }) as HTMLTextAreaElement;
    const err = h('div', { class: 'err', style: { minHeight: '14px', marginTop: '8px' } });
    const onionI = h('input', { class: 'input', placeholder: 'abc...xyz.onion' });
    const signI = h('input', { class: 'input', placeholder: 'sign_pk (64 hex)' });
    const dhI = h('input', { class: 'input', placeholder: 'dh_pk (64 hex)' });
    let cardName = '';

    pasteI.addEventListener('input', () => {
      const parsed = decodeCard(pasteI.value);
      if (parsed) {
        onionI.value = parsed.onion;
        signI.value = parsed.signPk;
        dhI.value = parsed.dhPk;
        cardName = parsed.name?.trim() ?? '';
        err.textContent = '';
      }
    });

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, '── add contact ──'),
        h('button', { class: 'icon-btn', onClick: close }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'field' }, h('label', null, 'paste card'), pasteI),
        h('div', { class: 'hint', style: { marginBottom: '8px' } },
          'имя контакта приходит из его карточки и потом обновляется автоматически из его сообщений. локально не задаётся — каждый сам себя называет.'),
        h('div', { class: 'divider-text' }, 'manual entry'),
        h('div', { class: 'field' }, h('label', null, 'onion address'), onionI),
        h('div', { class: 'field' }, h('label', null, 'sign_pk'), signI),
        h('div', { class: 'field' }, h('label', null, 'dh_pk'), dhI),
        err,
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: close }, '[ cancel ]'),
        h('button', {
          class: 'btn',
          onClick: async () => {
            const onion = onionI.value.trim();
            const sign = signI.value.trim().toLowerCase();
            const dh = dhI.value.trim().toLowerCase();
            const name = cardName || `${sign.slice(0, 16)}`;
            if (!onion.endsWith('.onion')) { err.textContent = 'onion must end with .onion'; return; }
            if (!/^[0-9a-f]{64}$/.test(sign)) { err.textContent = 'sign_pk must be 64 hex'; return; }
            if (!/^[0-9a-f]{64}$/.test(dh)) { err.textContent = 'dh_pk must be 64 hex'; return; }
            try {
              await Api.addContact(onion, sign, dh, name);
              await store.refreshContacts();
              store.showToast('contact added');
              close();
            } catch (e) { err.textContent = 'err: ' + String(e); }
          },
        }, '[ ADD ]'),
      ),
    );
  }
}
