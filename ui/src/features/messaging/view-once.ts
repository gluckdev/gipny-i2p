import type { App } from '../../app/app';
import { Api, type ViewOnceNotice } from '../../app/api';
import type { Store } from '../../app/state';
import { fmtTime, h } from '../../app/view';
import { t as tr } from '../localization/i18n';

/** Only the opaque notice belongs in chat state. The editor and reader are
 * independent of the normal composer, its draft cache and history. */
export class ViewOnceChat {
  readonly inbox = h('div', { class: 'view-once-inbox' });
  readonly compose = h('button', { type: 'button', class: 'icon-btn view-once-compose',
    title: tr('chat.view_once_write'), 'aria-label': tr('chat.view_once_write'), onClick: () => this.write() }, '①');
  private close?: () => void;
  private revision = 0;
  private disposed = false;
  private timer?: number;
  private hide = (): void => { if (document.visibilityState === 'hidden') this.close?.(); };

  constructor(private app: App, private store: Store, private contact: number, private ttl: () => number) {
    document.addEventListener('visibilitychange', this.hide);
  }

  async refresh(): Promise<void> {
    const revision = ++this.revision;
    const list = await Api.listViewOnce(this.contact).catch(() => []);
    if (revision !== this.revision || this.disposed) return;
    window.clearTimeout(this.timer);
    const now = Date.now();
    const pending = (list ?? []).filter(item => item.expires_at > now);
    this.inbox.replaceChildren(...pending.map(item => h('button', { type: 'button', class: 'btn btn-sm',
      onClick: () => this.read(item) }, tr('chat.view_once_read'), ' · ', fmtTime(item.sent_at))));
    if (pending.length) this.timer = window.setTimeout(() => void this.refresh(),
      Math.min(2_147_000_000, Math.min(...pending.map(item => item.expires_at)) - now));
  }

  private read(item: ViewOnceNotice): void {
    if (this.disposed || this.store.screenLocked.get() || document.visibilityState === 'hidden') return;
    this.close?.();
    const epoch = this.store.contentVersion;
    const canDisplay = (): boolean => !this.disposed && !this.store.screenLocked.get()
      && this.store.contentVersion === epoch && document.visibilityState !== 'hidden';
    const body = h('div', { class: 'modal-body view-once-text', 'aria-live': 'polite' });
    let active = true;
    const close = this.app.openModal(dismiss => h('div', { class: 'modal' },
      h('div', { class: 'modal-title' }, tr('chat.view_once')), body,
      h('div', { class: 'modal-footer' }, h('button', { class: 'btn', onClick: dismiss }, tr('common.close')))),
    () => { active = false; body.replaceChildren(); this.close = undefined; });
    this.close = close;
    const expire = (): void => {
      if (!active) return;
      const left = item.expires_at - Date.now();
      if (left <= 0) close();
      else timer = window.setTimeout(expire, Math.min(left, 2_147_000_000));
    };
    let timer: number | undefined;
    expire();
    body.parentElement?.addEventListener('gipny-modal-close', () => window.clearTimeout(timer), { once: true });
    if (!active || !canDisplay()) { close(); return; }
    void Api.openViewOnce(item.id).then(value => {
      if (active && canDisplay() && Date.now() < item.expires_at) body.textContent = value;
    }).catch(() => { if (active) body.textContent = tr('chat.view_once_failed'); })
      .finally(() => { if (!this.disposed) void this.refresh(); });
  }

  private write(): void {
    this.close?.();
    const field = h('textarea', { class: 'input', rows: 5, maxlength: 16384, spellcheck: 'false',
      autocomplete: 'off', autocapitalize: 'off', 'aria-label': tr('chat.view_once_write') });
    const error = h('div', { class: 'err', role: 'alert' });
    let active = true;
    this.close = this.app.openModal(close => {
      const submit = h('button', { type: 'button', class: 'btn', onClick: async () => {
        if (submit.disabled || !field.value.trim()) return;
        if (new TextEncoder().encode(field.value).length > 16 * 1024) {
          error.textContent = tr('chat.view_once_too_large'); return;
        }
        submit.disabled = true; field.readOnly = true;
        try {
          await Api.sendViewOnce(this.contact, field.value, this.ttl());
          if (active) { close(); this.store.showToast(tr('chat.view_once_sent')); }
        } catch {
          if (active) error.textContent = tr('chat.view_once_failed');
        } finally { submit.disabled = false; field.readOnly = false; }
      } }, tr('chat.send'));
      return h('div', { class: 'modal' }, h('div', { class: 'modal-title' }, tr('chat.view_once_write')),
        h('div', { class: 'modal-body' }, h('p', {}, tr('chat.view_once_hint')), field, error),
        h('div', { class: 'modal-footer' }, h('button', { class: 'btn btn-ghost', onClick: close }, tr('common.close')), submit));
    }, () => { active = false; field.value = ''; this.close = undefined; });
  }

  destroy(): void {
    this.disposed = true;
    window.clearTimeout(this.timer);
    document.removeEventListener('visibilitychange', this.hide);
    this.close?.();
    this.inbox.replaceChildren();
  }
}
