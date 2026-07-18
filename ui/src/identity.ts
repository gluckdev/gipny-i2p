import { Api, encodeCard } from './api';
import type { Bundle } from './api';
import type { Store } from './state';
import { h, fmtFp } from './view';

function kv(label: string, value: string): HTMLElement {
  return h('div', { style: { marginBottom: '8px' } },
    h('div', { class: 'card-label' }, label),
    h('div', { style: { wordBreak: 'break-all' } }, value),
  );
}

export class IdentityModal {
  el: HTMLElement;
  constructor(store: Store, close: () => void) {
    const id = store.identity.get();
    const b32Slot = h('div', { class: 'card-block' }, '…');
    Api.myB32().then((b: string) => { b32Slot.textContent = b || '(unavailable)'; }).catch(() => { b32Slot.textContent = '?'; });
    const bundleSlot = h('div', { class: 'card-block' }, 'loading bundle...');
    const nameI = h('input', {
      class: 'input', value: store.displayName.get(),
      placeholder: 'e.g., gipny', maxlength: '64',
    }) as HTMLInputElement;
    const cardBlock = h('div', { class: 'card-block' });
    const updateCard = (): void => {
      const name = nameI.value.trim();
      cardBlock.textContent = id
        ? encodeCard(id.onion, id.card.sign_pk, id.card.dh_pk, name || undefined)
        : '';
    };
    updateCard();
    nameI.addEventListener('input', updateCard);

    Api.myBundle().then((b: Bundle) => {
      bundleSlot.replaceChildren();
      bundleSlot.appendChild(h('div', null,
        kv('sign_pk', b.sign_pk),
        kv('dh_pk', b.dh_pk),
        kv('signed_prekey', b.signed_prekey),
        kv('signed_prekey_sig', b.signed_prekey_sig.slice(0, 60) + '…'),
        kv('one_time_prekey', b.one_time_prekey ?? '(none)'),
      ));
    }).catch(() => { bundleSlot.textContent = 'err loading bundle'; });

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, '── my identity ──'),
        h('button', { class: 'icon-btn', onClick: close }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'field' },
          h('label', null, 'display name'),
          h('div', { class: 'row' },
            nameI,
            h('button', {
              class: 'btn',
              onClick: async () => {
                try {
                  await store.updateDisplayName(nameI.value.trim());
                  updateCard();
                  store.showToast('name saved');
                } catch (e) { store.showToast(String(e), true); }
              },
            }, '[ save ]'),
          ),
          h('div', { class: 'hint' }, 'embedded in shared card'),
        ),
        h('div', { class: 'card-label', style: { marginTop: '14px' } }, 'i2p address (full)'),
        h('div', { class: 'card-block' }, id?.onion ?? ''),
        h('div', { class: 'card-label', style: { marginTop: '14px' } }, 'i2p address (b32)'),
        b32Slot,
        h('div', { class: 'card-label', style: { marginTop: '14px' } }, 'fingerprint'),
        h('div', { class: 'card-block fp' }, id ? fmtFp(id.fingerprint) : ''),
        h('div', { class: 'card-label', style: { marginTop: '14px' } }, 'card (share this)'),
        cardBlock,
        h('div', { class: 'row', style: { marginTop: '10px' } },
          h('button', {
            class: 'btn btn-amber',
            onClick: async () => {
              await navigator.clipboard.writeText(cardBlock.textContent ?? '');
              store.showToast('card copied');
            },
          }, '[ COPY CARD ]'),
        ),
        h('div', { class: 'divider-text' }, 'bundle'),
        bundleSlot,
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: close }, '[ close ]'),
      ),
    );
  }
}
