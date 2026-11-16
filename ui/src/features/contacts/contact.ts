import { errorText } from '../../app/error-text';
import { t as tr } from '../localization/i18n';
import { Api } from '../../app/api';
import type { Store } from '../../app/state';
import { targetKey } from '../../app/state';
import { h, fmtFp } from '../../app/view';
import type { App } from '../../app/app';

import { avatarPicker } from '../identity/avatar-picker';
import { icon } from '../../app/icons';
export class ContactModal {
  el: HTMLElement;
  constructor(store: Store, app: App, contactId: number, close: () => void) {
    const c = store.contacts.get().find((x) => x.id === contactId);
    if (!c) { this.el = h('div'); close(); return; }

    const nameI = h('input', { class: 'input', value: c.name });
    const trustSel = h('select', { class: 'input' },
      h('option', { value: '0', selected: c.trust === 0 }, tr('contact.unverified')),
      h('option', { value: '1', selected: c.trust === 1 }, tr('contact.verified')),
      h('option', { value: '2', selected: c.trust === 2 }, tr('contact.blocked')),
    );
    const botCb = h('input', { type: 'checkbox', class: 'cb', checked: !!c.is_bot }) as HTMLInputElement;
    const isMutedNow = store.muted.get().has(targetKey({ kind: 'contact', id: c.id }));
    const muteCb = h('input', { type: 'checkbox', class: 'cb', checked: isMutedNow }) as HTMLInputElement;

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, tr('contact.contact')),
        h('button', { class: 'icon-btn', title: tr('common.close'), onClick: close }, icon('close')),
      ),
      h('div', { class: 'modal-body' },
        avatarPicker(c.sign_pk, tr('contact.only_you_see_this_avatar'), (e) => store.showToast(errorText(e), true)),
        h('div', { class: 'field' }, h('label', null, tr('contact.name')), nameI),
        h('div', { class: 'field' }, h('label', null, tr('contact.trust')), trustSel),
        h('label', { class: 'cb-row' }, botCb, h('span', null, tr('contact.this_is_a_bot'))),
        h('label', { class: 'cb-row' }, muteCb, h('span', null, tr('contact.mute_notifications'))),
        h('div', { class: 'card-label', style: { marginTop: '10px' } }, tr('contact.i2p_address')),
        h('div', { class: 'card-block' }, c.onion),
        h('div', { class: 'card-label', style: { marginTop: '10px' } }, tr('contact.key_fingerprint')),
        h('div', { class: 'card-block fp' }, fmtFp(c.dh_pk)),
        h('div', { class: 'hint', style: { marginTop: '8px' } }, tr('contact.compare_this_fingerprint_with_your_contact_in_person')),
        h('div', { class: 'divider-text' }, tr('contact.if_messages_are_not_arriving')),
        h('button', {
          class: 'btn btn-amber',
          style: { width: '100%' },
          onClick: async () => {
            const ok = await app.confirm(
              tr('contact.reset_session'),
              tr('contact.restart_the_encrypted_session_with_this_contact_the'),
            );
            if (!ok) return;
            try {
              await Api.resetContactSession(c.id);
              store.showToast(tr('contact.session_reset_the_next_message_will_establish_it'));
            } catch (e) {
              store.showToast(tr('contact.could_not_reset') + errorText(e), true);
            }
          },
        }, tr('contact.reset_session')),
      ),
      h('div', { class: 'modal-footer' },
        h('button', {
          class: 'btn btn-danger',
          onClick: async () => {
            const ok = await app.confirm(tr('contact.delete_contact'), tr('contact.delete_and_your_copy_of_the_conversation_your', { p0: (c.name) }), true);
            if (!ok) return;
            await Api.deleteContact(c.id);
            await store.refreshContacts();
            const sel = store.selectedChat.get();
            if (sel?.kind === 'contact' && sel.id === c.id) store.selectedChat.set(null);
            close();
          },
        }, tr('contact.delete_for_me')),
        h('button', {
          class: 'btn btn-danger',
          onClick: async () => {
            const ok = await app.confirm(
              tr('contact.delete_for_both'),
              tr('contact.delete_and_your_conversation_and_ask_their_app', { p0: (c.name) }),
              true,
            );
            if (!ok) return;
            await Api.deleteContact(c.id, true);
            await store.refreshContacts();
            const sel = store.selectedChat.get();
            if (sel?.kind === 'contact' && sel.id === c.id) store.selectedChat.set(null);
            close();
          },
        }, tr('contact.delete_for_both')),
        h('div', { class: 'grow' }),
        h('button', { class: 'btn btn-ghost', onClick: close }, tr('common.cancel')),
        h('button', {
          class: 'btn',
          onClick: async () => {
            await Api.updateContact(c.id, nameI.value.trim(), parseInt((trustSel as HTMLSelectElement).value) || 0);
            if (botCb.checked !== !!c.is_bot) await Api.setContactBot(c.id, botCb.checked);
            if (muteCb.checked !== isMutedNow) await store.toggleMute({ kind: 'contact', id: c.id }, muteCb.checked);
            await store.refreshContacts();
            store.showToast(tr('contact.saved'));
            close();
          },
        }, tr('common.save')),
      ),
    );
  }
}

export { AddContactModal } from './add-contact';
