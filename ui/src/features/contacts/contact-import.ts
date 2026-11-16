import { Api } from '../../app/api';
import type { Store } from '../../app/state';
import { t } from '../localization/i18n';
import { decodeCard, encodeCard, isValidI2pAddress, type DecodedCard } from './contact-card';

/** Parsing proves a complete card, not the identity of the person sharing it. */
export function validateContact(card: DecodedCard): string | null {
  if (!isValidI2pAddress(card.i2p_address)) return t('contact.enter_a_b32_i2p_address_52_characters_or');
  if (!/^[0-9a-f]{64}$/.test(card.signPk)) return t('contact.sign_pk_must_contain_64_hexadecimal_characters');
  if (!/^[0-9a-f]{64}$/.test(card.dhPk)) return t('contact.dh_pk_must_contain_64_hexadecimal_characters');
  if (card.relay && !isValidI2pAddress(card.relay)) return t('contact.the_relay_must_be_a_b32_i2p_address');
  // The canonical encoder also checks destination length/certificate, base32
  // trailing bits and UTF-8 name bounds, including for legacy v1/v2 imports.
  try { encodeCard(card.i2p_address, card.signPk, card.dhPk, card.name, card.relay || undefined); }
  catch { return t('contact.this_code_is_not_a_gipny_card'); }
  return null;
}

export function parseContactImport(text: string): DecodedCard | null {
  const card = decodeCard(text);
  return card && !validateContact(card) ? card : null;
}

const pending = new WeakMap<Store, Set<string>>();
type ImportResult = 'added' | 'existing' | 'ignored';

/** One owner per modal, plus a cross-modal lock for the same profile/key. */
export class ContactImport {
  private closed = false;
  private busy = false;
  private unsubs: Array<() => void>;
  private readonly profile: string | null;

  constructor(private store: Store, private onBusy: (busy: boolean) => void) {
    this.profile = store.currentProfile.get();
    this.unsubs = [
      store.currentProfile.subscribe(() => this.close(), false),
      store.view.subscribe((view) => { if (view !== 'main') this.close(); }, false),
    ];
  }

  close(): void {
    this.closed = true;
    for (const unsubscribe of this.unsubs) unsubscribe();
    this.unsubs = [];
  }

  async add(card: DecodedCard): Promise<ImportResult> {
    if (this.closed || this.busy) return 'ignored';
    const invalid = validateContact(card);
    if (invalid) throw new Error(invalid);
    if (card.signPk === this.store.identity.get()?.card.sign_pk.toLowerCase()) throw new Error(t('contact.own_card'));
    const key = `${this.profile ?? ''}:${card.signPk}`;
    const locks = pending.get(this.store) ?? new Set<string>();
    if (locks.has(key)) return 'ignored';
    pending.set(this.store, locks);
    locks.add(key);
    this.busy = true;
    this.onBusy(true);
    try {
      // Read the vault as well as the UI cache: another window may have added
      // this identity since the modal opened. Never call upsert for duplicates.
      const contacts = await Api.listContacts();
      if (this.closed) return 'ignored';
      const existing = contacts.find((contact) => contact.sign_pk.toLowerCase() === card.signPk);
      if (existing) {
        if (existing.dh_pk.toLowerCase() !== card.dhPk) throw new Error(t('contact.identity_conflict'));
        return 'existing';
      }
      await Api.addContact(card.i2p_address, card.signPk, card.dhPk, card.name?.trim() || card.signPk.slice(0, 16), card.relay || undefined);
      if (this.closed) return 'ignored';
      await this.store.refreshContacts();
      return this.closed ? 'ignored' : 'added';
    } finally {
      locks.delete(key);
      this.busy = false;
      if (!this.closed) this.onBusy(false);
    }
  }
}
