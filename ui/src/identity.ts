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

export class IdentityModal {
  el: HTMLElement;
  constructor(store: Store, close: () => void) {
    const id = store.identity.get();
    const b32Slot = h('div', { class: 'card-block' }, '…');
    Api.myB32().then((b: string) => { b32Slot.textContent = b || '(unavailable)'; }).catch((e) => {
      console.error('[identity] b32 address computation failed:', e);
      b32Slot.textContent = '(unavailable)';
    });
    const bundleSlot = h('div', { class: 'card-block' }, 'loading bundle...');
    const nameI = h('input', {
      class: 'input', value: store.displayName.get(),
      placeholder: 'e.g., gipny', maxlength: '64',
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
        store.showToast('card copied');
      },
    }, 'Copy card') as HTMLButtonElement;
    const cardNote = h('div', { class: 'hint', style: { marginTop: '6px' } });
    const updateCard = (): void => {
      const name = nameI.value.trim();
      copyBtn.disabled = waiting;
      if (waiting) {
        cardBlock.textContent = 'встроенный релей запускается — карточка появится здесь через минуту-две…';
        return;
      }
      cardBlock.textContent = id
        ? encodeCard(id.onion, id.card.sign_pk, id.card.dh_pk, name || undefined, myRelay)
        : '';
    };
    const paintRelay = (info: RelayInfo | null): void => {
      const builtin = info?.mode === 'builtin';
      if (builtin) myRelay = info.hosted.state === 'ready' ? info.hosted.address.trim() : '';
      waiting = builtin && !myRelay;
      cardNote.textContent = builtin && myRelay
        ? 'адрес релея в карточке действует до перезапуска приложения. Тот, с кем вы уже переписываетесь, '
          + 'узнает новый адрес сам; новому контакту давайте свежую карточку.'
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
            }, 'Save'),
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
        cardNote,
        h('div', { class: 'row', style: { marginTop: '10px' } },
          copyBtn,
        ),
        h('div', { class: 'divider-text' }, 'bundle'),
        bundleSlot,
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: close }, 'Close'),
      ),
    );
  }
}
