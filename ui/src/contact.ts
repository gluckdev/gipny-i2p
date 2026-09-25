import { Api, decodeCard, isValidI2pAddress } from './api';
import type { Store } from './state';
import { targetKey } from './state';
import { h, fmtFp } from './view';
import type { App } from './app';

import { avatarPicker } from './avatar-picker';
import { QrScanner } from './qr-scan';
import { icon } from './icons';
export class ContactModal {
  el: HTMLElement;
  constructor(store: Store, app: App, contactId: number, close: () => void) {
    const c = store.contacts.get().find((x) => x.id === contactId);
    if (!c) { this.el = h('div'); close(); return; }

    const nameI = h('input', { class: 'input', value: c.name });
    const trustSel = h('select', { class: 'input' },
      h('option', { value: '0', selected: c.trust === 0 }, 'не проверен'),
      h('option', { value: '1', selected: c.trust === 1 }, 'проверен'),
      h('option', { value: '2', selected: c.trust === 2 }, 'заблокирован'),
    );
    const botCb = h('input', { type: 'checkbox', class: 'cb', checked: !!c.is_bot }) as HTMLInputElement;
    const isMutedNow = store.muted.get().has(targetKey({ kind: 'contact', id: c.id }));
    const muteCb = h('input', { type: 'checkbox', class: 'cb', checked: isMutedNow }) as HTMLInputElement;

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, 'Контакт'),
        h('button', { class: 'icon-btn', title: 'Закрыть', onClick: close }, icon('close')),
      ),
      h('div', { class: 'modal-body' },
        avatarPicker(c.sign_pk, 'Аватарку видите только вы', (e) => store.showToast(String(e), true)),
        h('div', { class: 'field' }, h('label', null, 'Имя'), nameI),
        h('div', { class: 'field' }, h('label', null, 'Доверие'), trustSel),
        h('label', { class: 'cb-row' }, botCb, h('span', null, 'Это бот')),
        h('label', { class: 'cb-row' }, muteCb, h('span', null, 'Без уведомлений')),
        h('div', { class: 'card-label', style: { marginTop: '10px' } }, 'Адрес i2p'),
        h('div', { class: 'card-block' }, c.onion),
        h('div', { class: 'card-label', style: { marginTop: '10px' } }, 'Отпечаток ключа'),
        h('div', { class: 'card-block fp' }, fmtFp(c.dh_pk)),
        h('div', { class: 'hint', style: { marginTop: '8px' } }, 'Сверьте отпечаток с собеседником при встрече или по другому каналу, прежде чем отмечать контакт проверенным.'),
        h('div', { class: 'divider-text' }, 'Если сообщения не доходят'),
        h('button', {
          class: 'btn btn-amber',
          style: { width: '100%' },
          onClick: async () => {
            const ok = await app.confirm(
              'Сбросить сессию',
              'Начать шифрованную сессию с контактом заново? Следующее сообщение любого из вас создаст её снова.',
            );
            if (!ok) return;
            try {
              await Api.resetContactSession(c.id);
              store.showToast('Сессия сброшена — пересоздастся при следующем сообщении');
            } catch (e) {
              store.showToast('Не удалось сбросить: ' + String(e), true);
            }
          },
        }, 'Сбросить сессию'),
      ),
      h('div', { class: 'modal-footer' },
        h('button', {
          class: 'btn btn-danger',
          onClick: async () => {
            const ok = await app.confirm('Удалить контакт', `Удалить «${c.name}» и переписку с ним у вас? У собеседника всё останется.`, true);
            if (!ok) return;
            await Api.deleteContact(c.id);
            await store.refreshContacts();
            const sel = store.selectedChat.get();
            if (sel?.kind === 'contact' && sel.id === c.id) store.selectedChat.set(null);
            close();
          },
        }, 'Удалить у себя'),
        h('button', {
          class: 'btn btn-danger',
          onClick: async () => {
            const ok = await app.confirm(
              'Удалить у обоих',
              `Удалить «${c.name}» и переписку у вас, и попросить приложение собеседника удалить переписку и вас у себя? ` +
                'У вас всё удалится сразу. У собеседника — когда запрос до него дойдёт (если он не в сети — когда появится). ' +
                'Это просьба к его приложению: то, что он уже прочитал, скопировал или снял с экрана, не вернуть.',
              true,
            );
            if (!ok) return;
            await Api.deleteContact(c.id, true);
            await store.refreshContacts();
            const sel = store.selectedChat.get();
            if (sel?.kind === 'contact' && sel.id === c.id) store.selectedChat.set(null);
            close();
          },
        }, 'Удалить у обоих'),
        h('div', { class: 'grow' }),
        h('button', { class: 'btn btn-ghost', onClick: close }, 'Отмена'),
        h('button', {
          class: 'btn',
          onClick: async () => {
            await Api.updateContact(c.id, nameI.value.trim(), parseInt((trustSel as HTMLSelectElement).value) || 0);
            if (botCb.checked !== !!c.is_bot) await Api.setContactBot(c.id, botCb.checked);
            if (muteCb.checked !== isMutedNow) await store.toggleMute({ kind: 'contact', id: c.id }, muteCb.checked);
            await store.refreshContacts();
            store.showToast('Сохранено');
            close();
          },
        }, 'Сохранить'),
      ),
    );
  }
}

export class AddContactModal {
  el: HTMLElement;
  constructor(store: Store, close: () => void) {
    const pasteI = h('textarea', {
      class: 'textarea',
      placeholder: 'вставьте карточку контакта\n(gipny:v2:…)',
      rows: '4',
    }) as HTMLTextAreaElement;
    const err = h('div', { class: 'err', style: { minHeight: '14px', marginTop: '8px' } });
    const onionI = h('input', { class: 'input', placeholder: 'destination (b64) или abc…xyz.b32.i2p' });
    const relayI = h('input', {
      class: 'input',
      placeholder: 'пусто — будет использован ваш',
      'aria-label': 'contact relay destination',
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
      scanBtn.textContent = 'Сканировать QR';
    };
    const startScan = (): void => {
      err.textContent = '';
      scanner = new QrScanner(
        (text) => {
          const ok = applyCard(text);
          pasteI.value = text;
          stopScan();
          if (ok) store.showToast('Карточка распознана — проверьте и нажмите «Добавить»');
          else err.textContent = 'В коде не карточка gipny';
        },
        (message) => { err.textContent = message; stopScan(); },
      );
      scanSlot.replaceChildren(scanner.el);
      scanSlot.classList.remove('hidden');
      scanBtn.textContent = 'Остановить';
      void scanner.start();
    };
    const scanBtn = h('button', {
      class: 'btn btn-ghost',
      onClick: () => (scanner ? stopScan() : startScan()),
    }, 'Сканировать QR');

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
        h('div', { class: 'modal-title' }, 'Добавить контакт'),
        h('button', { class: 'icon-btn', title: 'Закрыть', onClick: close }, icon('close')),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'field' }, h('label', null, 'Карточка контакта'), pasteI),
        h('div', { class: 'row', style: { marginBottom: '8px' } }, scanBtn),
        scanSlot,
        h('div', { class: 'hint', style: { marginBottom: '8px' } },
          'имя контакта приходит из его карточки и потом обновляется автоматически из его сообщений. локально не задаётся — каждый сам себя называет.'),
        h('div', { class: 'divider-text' }, 'Вручную'),
        h('div', { class: 'field' }, h('label', null, 'Адрес i2p'), onionI),
        h('div', { class: 'field' }, h('label', null, 'Релей контакта'), relayI),
        h('div', { class: 'hint', style: { marginBottom: '8px' } },
          'релей — это где контакт забирает почту. приходит из его карточки; ' +
          'пусто — используется твой.'),
        h('div', { class: 'field' }, h('label', null, 'sign_pk'), signI),
        h('div', { class: 'field' }, h('label', null, 'dh_pk'), dhI),
        err,
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: close }, 'Отмена'),
        h('button', {
          class: 'btn',
          onClick: async () => {
            const onion = onionI.value.trim();
            const sign = signI.value.trim().toLowerCase();
            const dh = dhI.value.trim().toLowerCase();
            const name = cardName || `${sign.slice(0, 16)}`;
            if (!isValidI2pAddress(onion)) {
              err.textContent = 'Нужен адрес .b32.i2p (52 символа) или полный destination в base64';
              return;
            }
            if (!/^[0-9a-f]{64}$/.test(sign)) { err.textContent = 'sign_pk — 64 шестнадцатеричных символа'; return; }
            if (!/^[0-9a-f]{64}$/.test(dh)) { err.textContent = 'dh_pk — 64 шестнадцатеричных символа'; return; }
            const relay = relayI.value.trim();
            if (relay && !isValidI2pAddress(relay)) {
              err.textContent = 'Релей — адрес .b32.i2p или полный destination в base64';
              return;
            }
            try {
              await Api.addContact(onion, sign, dh, name, relay || undefined);
              await store.refreshContacts();
              store.showToast('Контакт добавлен');
              close();
            } catch (e) { err.textContent = 'Не удалось добавить: ' + String(e); }
          },
        }, 'Добавить'),
      ),
    );
  }
}
