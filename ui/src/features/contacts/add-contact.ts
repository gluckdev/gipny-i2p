import { errorText } from '../../app/error-text';
import { t as tr } from '../localization/i18n';
import type { Store } from '../../app/state';
import { h } from '../../app/view';
import { icon } from '../../app/icons';
import { QrScanner, scanQrInFile } from '../identity/qr-scan';
import { ContactImport, parseContactImport, validateContact } from './contact-import';
import type { DecodedCard } from './contact-card';

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
    let closed = false;
    const addBtn = h('button', { class: 'btn', onClick: () => void submit(fields()) }, tr('contact.add'));
    const importer = new ContactImport(store, (busy) => {
      addBtn.disabled = scanBtn.disabled = fileBtn.disabled = busy;
      pasteI.readOnly = busy;
      for (const input of [onionI, relayI, signI, dhI]) input.readOnly = busy;
      this.el?.setAttribute('aria-busy', String(busy));
    });
    const fields = (): DecodedCard => ({ onion: onionI.value.trim(), signPk: signI.value.trim().toLowerCase(),
      dhPk: dhI.value.trim().toLowerCase(), name: cardName, relay: relayI.value.trim() || undefined });
    const submit = async (card: DecodedCard): Promise<void> => {
      if (closed) return;
      const invalid = validateContact(card);
      if (invalid) { err.textContent = invalid; return; }
      err.textContent = '';
      try {
        const result = await importer.add(card);
        if (closed || result === 'ignored') return;
        store.showToast(tr(result === 'added' ? 'contact.contact_added' : 'contact.already_added'));
        finish();
      } catch (error) { if (!closed) err.textContent = tr('contact.could_not_add') + errorText(error); }
    };
    const dispose = (): void => { closed = true; importer.close(); stopScan(); };
    const finish = (): void => { dispose(); close(); };

    const applyCard = (text: string, autoAdd: boolean): boolean => {
      const parsed = parseContactImport(text);
      if (!parsed) return false;
      onionI.value = parsed.onion;
      signI.value = parsed.signPk;
      dhI.value = parsed.dhPk;
      relayI.value = parsed.relay ?? '';
      cardName = parsed.name?.trim() ?? '';
      err.textContent = '';
      if (autoAdd) void submit(parsed);
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
    // the same complete-card validation and single-flight import handle both.
    const acceptScan = (text: string): void => {
      if (closed) return;
      const ok = applyCard(text, true);
      pasteI.value = text;
      stopScan();
      if (!ok) err.textContent = tr('contact.this_code_is_not_a_gipny_card');
    };
    const failScan = (message: string): void => { if (!closed) { err.textContent = message; stopScan(); } };
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
        scanQrInFile(file, acceptScan, failScan);
      },
    }) as HTMLInputElement;
    const fileBtn = h('button', {
      class: 'btn btn-ghost',
      onClick: () => fileI.click(),
    }, tr('qr.load_file'));

    // A legacy URI can already parse before its optional name has been typed.
    // Only a complete paste or scan commits automatically; typing fills fields.
    pasteI.addEventListener('input', (event) => {
      if (!closed) applyCard(pasteI.value, (event as InputEvent).inputType === 'insertFromPaste');
    });

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, tr('contact.add_contact')),
        h('button', { class: 'icon-btn', title: tr('common.close'), onClick: finish }, icon('close')),
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
        h('button', { class: 'btn btn-ghost', onClick: finish }, tr('common.cancel')),
        addBtn,
      ),
    );
    this.el.addEventListener('gipny-modal-close', dispose, { once: true });
  }
}
