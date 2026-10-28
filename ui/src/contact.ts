import { errorText } from './error-text';
import { t as tr } from './i18n';
import { Api, decodeCard, isValidI2pAddress } from './api';
import type { Store } from './state';
import { targetKey } from './state';
import { h, fmtFp } from './view';
import type { App } from './app';

import { avatarPicker } from './avatar-picker';
import { QrScanner, scanQrInFile } from './qr-scan';
import { icon } from './icons';
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

export class AddContactModal {
  el: HTMLElement;
  constructor(store: Store, close: () => void) {
    const pasteI = h('textarea', {
      class: 'textarea',
      placeholder: tr('contact.paste_a_contact_card_gipny_v2'),
      rows: '4',
    }) as HTMLTextAreaElement;
    const err = h('div', { class: 'err', style: { minHeight: '14px', marginTop: '8px' } });
    const onionI = h('input', { class: 'input', placeholder: tr('contact.destination_base64_or_abc_xyz_b32_i2p') });
    const relayI = h('input', {
      class: 'input',
      placeholder: tr('contact.leave_empty_to_use_yours'),
      'aria-label': tr('contact.contact_relay_destination'),
    }) as HTMLInputElement;
    const signI = h('input', { class: 'input', placeholder: 'sign_pk (64 hex)' });
    const dhI = h('input', { class: 'input', placeholder: 'dh_pk (64 hex)' });
    let cardName = '';

    const applyCard = (text: string): boolean => {
      const parsed = decodeCard(text);
      if (!parsed) return false;
      onionI.value = parsed.onion;
      signI.value = parsed.signPk;
      dhI.value = parsed.dhPk;
      relayI.value = parsed.relay ?? '';
      cardName = parsed.name?.trim() ?? '';
      err.textContent = '';
      return true;
    };

    // Reading a card off someone's screen instead of copying 600 characters.
    const scanSlot = h('div', { class: 'qr-slot hidden' });
    let scanner: QrScanner | null = null;
    const stopScan = (): void => {
      scanner?.stop();
      scanner = null;
      scanSlot.replaceChildren();
      scanSlot.classList.add('hidden');
      scanBtn.textContent = tr('contact.scan_qr');
    };
    // A scanned card lands here whatever it was read from — camera or file — so
    // there is one place that fills the fields and one place that complains.
    const acceptScan = (text: string): void => {
      const ok = applyCard(text);
      pasteI.value = text;
      stopScan();
      if (ok) store.showToast(tr('contact.card_recognized_review_it_and_select_add'));
      else err.textContent = tr('contact.this_code_is_not_a_gipny_card');
    };
    const failScan = (message: string): void => { err.textContent = message; stopScan(); };
    const startScan = (): void => {
      err.textContent = '';
      scanner = new QrScanner(acceptScan, failScan);
      scanSlot.replaceChildren(scanner.el);
      scanSlot.classList.remove('hidden');
      scanBtn.textContent = tr('contact.stop');
      void scanner.start();
    };
    const scanBtn = h('button', {
      class: 'btn btn-ghost',
      onClick: () => (scanner ? stopScan() : startScan()),
    }, tr('contact.scan_qr'));

    // A photo or a screenshot of the code, for when the camera is refused, is
    // missing, or is simply not being pointed well enough. Usable whether or
    // not the scanner is running, which is why it is a button of its own rather
    // than part of the camera view.
    const fileI = h('input', {
      type: 'file',
      accept: 'image/*',
      class: 'qr-file',
      onChange: () => {
        const file = fileI.files?.[0];
        // Clear it either way: picking the same screenshot twice in a row has to
        // fire `change` again, and a filled input does not.
        fileI.value = '';
        if (!file) return;
        err.textContent = '';
        stopScan();
        scanQrInFile(file, acceptScan, (message) => { err.textContent = message; });
      },
    }) as HTMLInputElement;
    const fileBtn = h('button', {
      class: 'btn btn-ghost',
      onClick: () => fileI.click(),
    }, tr('qr.load_file'));

    pasteI.addEventListener('input', () => {
      const parsed = decodeCard(pasteI.value);
      if (parsed) {
        onionI.value = parsed.onion;
        signI.value = parsed.signPk;
        dhI.value = parsed.dhPk;
        // v1 cards carry no relay; leaving the field alone then means "use ours",
        // which is exactly the old behaviour.
        relayI.value = parsed.relay ?? '';
        cardName = parsed.name?.trim() ?? '';
        err.textContent = '';
      }
    });

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, tr('contact.add_contact')),
        h('button', { class: 'icon-btn', title: tr('common.close'), onClick: close }, icon('close')),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'field' }, h('label', null, tr('contact.contact_card')), pasteI),
        h('div', { class: 'row', style: { marginBottom: '8px' } }, scanBtn, fileBtn, fileI),
        scanSlot,
        h('div', { class: 'hint', style: { marginBottom: '8px' } },
          tr('contact.a_contact_s_name_comes_from_their_card')),
        h('div', { class: 'divider-text' }, tr('contact.enter_manually')),
        h('div', { class: 'field' }, h('label', null, tr('contact.i2p_address')), onionI),
        h('div', { class: 'field' }, h('label', null, tr('contact.contact_s_relay')), relayI),
        h('div', { class: 'hint', style: { marginBottom: '8px' } },
          tr('contact.the_relay_is_where_this_contact_collects_mail')),
        h('div', { class: 'field' }, h('label', null, 'sign_pk'), signI),
        h('div', { class: 'field' }, h('label', null, 'dh_pk'), dhI),
        err,
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: close }, tr('common.cancel')),
        h('button', {
          class: 'btn',
          onClick: async () => {
            const onion = onionI.value.trim();
            const sign = signI.value.trim().toLowerCase();
            const dh = dhI.value.trim().toLowerCase();
            const name = cardName || `${sign.slice(0, 16)}`;
            if (!isValidI2pAddress(onion)) {
              err.textContent = tr('contact.enter_a_b32_i2p_address_52_characters_or');
              return;
            }
            if (!/^[0-9a-f]{64}$/.test(sign)) { err.textContent = tr('contact.sign_pk_must_contain_64_hexadecimal_characters'); return; }
            if (!/^[0-9a-f]{64}$/.test(dh)) { err.textContent = tr('contact.dh_pk_must_contain_64_hexadecimal_characters'); return; }
            const relay = relayI.value.trim();
            if (relay && !isValidI2pAddress(relay)) {
              err.textContent = tr('contact.the_relay_must_be_a_b32_i2p_address');
              return;
            }
            try {
              await Api.addContact(onion, sign, dh, name, relay || undefined);
              await store.refreshContacts();
              store.showToast(tr('contact.contact_added'));
              close();
            } catch (e) { err.textContent = tr('contact.could_not_add') + errorText(e); }
          },
        }, tr('contact.add')),
      ),
    );
  }
}
