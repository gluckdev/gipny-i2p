import { errorText } from './error-text';
import { t as tr } from './i18n';
import { Api, encodeCard } from './api';
import type { Bundle, RelayInfo } from './api';
import type { Store } from './state';
import { h, fmtFp } from './view';

function kv(label: string, value: string): HTMLElement {
  return h('div', { style: { marginBottom: '8px' } },
    h('div', { class: 'card-label' }, label),
    h('div', { style: { wordBreak: 'break-all' } }, value),
  );
}

import { avatarPicker } from './avatar-picker';
import { icon } from './icons';
export class IdentityModal {
  el: HTMLElement;
  constructor(store: Store, close: () => void) {
    const id = store.identity.get();
    const b32Slot = h('div', { class: 'card-block' }, '…');
    Api.myB32().then((b: string) => { b32Slot.textContent = b || tr('identity.unavailable'); }).catch((e) => {
      console.error('[identity] b32 address computation failed:', e);
      b32Slot.textContent = tr('identity.unavailable');
    });
    const bundleSlot = h('div', { class: 'card-block' }, tr('identity.loading_bundle'));
    const nameI = h('input', {
      class: 'input', value: store.displayName.get(),
      placeholder: tr('identity.e_g_gipny'), maxlength: '64',
    }) as HTMLInputElement;
    const cardBlock = h('div', { class: 'card-block' });
    // The card carries the relay we receive through, so whoever adds us deposits
    // where we actually collect. Without it they would have to already use the
    // same relay as us, which is what made one shared relay mandatory.
    let myRelay = '';
    // Built-in relay not up yet: a card without a relay is one nobody can
    // write to, so it is not shown and cannot be copied until there is one.
    let waiting = false;
    const copyBtn = h('button', {
      class: 'btn btn-amber',
      onClick: async () => {
        if (waiting) return;
        await navigator.clipboard.writeText(cardBlock.textContent ?? '');
        store.showToast(tr('identity.card_copied'));
      },
    }, tr('identity.copy_card')) as HTMLButtonElement;
    const cardNote = h('div', { class: 'hint', style: { marginTop: '6px' } });
    // The card is ~600 characters; nobody dictates that. A QR on screen is how
    // two people standing next to each other exchange cards.
    const qrSlot = h('div', { class: 'card-qr' });
    let qrFor = '';
    const paintQr = (card: string): void => {
      if (card === qrFor) return;
      qrFor = card;
      if (!card) { qrSlot.replaceChildren(); return; }
      Api.qrSvg(card)
        .then((svg) => {
          if (qrFor !== card) return;
          qrSlot.innerHTML = svg;
          qrSlot.appendChild(h('div', { class: 'hint' }, tr('identity.let_your_contact_scan_this_code_add_contact')));
        })
        .catch(() => { qrSlot.replaceChildren(h('div', { class: 'hint' }, tr('identity.could_not_generate_qr_code'))); });
    };
    const updateCard = (): void => {
      const name = nameI.value.trim();
      copyBtn.disabled = waiting;
      if (waiting) {
        cardBlock.textContent = tr('identity.built_in_relay_is_starting_your_card_will');
        paintQr('');
        return;
      }
      const card = id
        ? encodeCard(id.onion, id.card.sign_pk, id.card.dh_pk, name || undefined, myRelay)
        : '';
      cardBlock.textContent = card;
      paintQr(card);
    };
    const paintRelay = (info: RelayInfo | null): void => {
      const builtin = info?.mode === 'builtin';
      if (builtin) myRelay = info.hosted.state === 'ready' ? info.hosted.address.trim() : '';
      waiting = builtin && !myRelay;
      cardNote.textContent = builtin && myRelay
        ? tr('identity.the_relay_address_in_this_card_is_valid')
        : '';
      updateCard();
    };
    updateCard();
    Api.getRelayAddress()
      .then((r) => { myRelay = r.trim(); paintRelay(store.relayInfo.get()); })
      .catch(() => {});
    // The relay comes up while this window is open: the card fills in by itself.
    let shown = false;
    const unsub = store.relayInfo.subscribe((info) => {
      if (cardBlock.isConnected) shown = true;
      else if (shown) { unsub(); return; }
      paintRelay(info);
    });
    Api.getRelayInfo().then((info) => store.relayInfo.set(info)).catch(() => {});
    nameI.addEventListener('input', updateCard);

    Api.myBundle().then((b: Bundle) => {
      bundleSlot.replaceChildren();
      bundleSlot.appendChild(h('div', null,
        kv('sign_pk', b.sign_pk),
        kv('dh_pk', b.dh_pk),
        kv('signed_prekey', b.signed_prekey),
        kv('signed_prekey_sig', b.signed_prekey_sig.slice(0, 60) + '…'),
        kv('one_time_prekey', b.one_time_prekey ?? tr('identity.none')),
      ));
    }).catch(() => { bundleSlot.textContent = tr('identity.err_loading_bundle'); });

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, tr('identity.my_card')),
        h('button', { class: 'icon-btn', title: tr('common.close'), onClick: close }, icon('close')),
      ),
      h('div', { class: 'modal-body' },
        id && avatarPicker(id.card.sign_pk, tr('identity.your_avatar_on_this_device_contacts_choose_what'), (e) => store.showToast(errorText(e), true)),
        h('div', { class: 'field' },
          h('label', null, tr('identity.name_shown_to_contacts')),
          h('div', { class: 'row' },
            nameI,
            h('button', {
              class: 'btn',
              onClick: async () => {
                try {
                  await store.updateDisplayName(nameI.value.trim());
                  updateCard();
                  store.showToast(tr('identity.name_saved'));
                } catch (e) { store.showToast(errorText(e), true); }
              },
            }, tr('common.save')),
          ),
          h('div', { class: 'hint' }, tr('identity.included_in_your_card_and_every_message')),
        ),
        h('div', { class: 'card-label', style: { marginTop: '14px' } }, tr('identity.i2p_address_full')),
        h('div', { class: 'card-block' }, id?.onion ?? ''),
        h('div', { class: 'card-label', style: { marginTop: '14px' } }, tr('identity.i2p_address_b32')),
        b32Slot,
        h('div', { class: 'card-label', style: { marginTop: '14px' } }, tr('contact.key_fingerprint')),
        h('div', { class: 'card-block fp' }, id ? fmtFp(id.fingerprint) : ''),
        h('div', { class: 'card-label', style: { marginTop: '14px' } }, tr('identity.share_this_contact_card')),
        qrSlot,
        cardBlock,
        cardNote,
        h('div', { class: 'row', style: { marginTop: '10px' } },
          copyBtn,
        ),
        h('div', { class: 'divider-text' }, tr('identity.session_keys')),
        bundleSlot,
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: close }, tr('common.close')),
      ),
    );
  }
}
