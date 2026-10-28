import { errorText } from './error-text';
import { t as tr } from './i18n';
import { Api, type SearchHit } from './api';
import type { Store, ChatTarget } from './state';
import { h, fmtTime, fmtDate } from './view';

import { icon } from './icons';
export class SearchModal {
  el: HTMLElement;
  private input: HTMLInputElement;
  private results: HTMLElement;
  private status: HTMLElement;
  private timer: number | null = null;
  private closed = false;
  private generation = 0;
  private store: Store;
  private close: () => void;
  private scope: { contactId: number | null; groupId: string | null } = { contactId: null, groupId: null };

  constructor(store: Store, close: () => void, scope?: { contactId?: number | null; groupId?: string | null }) {
    this.store = store;
    this.close = close;
    if (scope) {
      this.scope.contactId = scope.contactId ?? null;
      this.scope.groupId = scope.groupId ?? null;
    }

    this.input = h('input', {
      class: 'input',
      placeholder: tr('search.type_to_search'),
      onInput: () => this.scheduleSearch(),
    }) as HTMLInputElement;

    const scopeLabel = this.scope.contactId != null
      ? tr('search.in_this_contact')
      : this.scope.groupId != null
        ? tr('search.in_this_group')
        : tr('search.global_all_chats');

    this.status = h('div', { class: 'hint' }, scopeLabel);
    this.results = h('div', { class: 'search-results' });

    this.el = h('div', { class: 'modal modal-wide' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, tr('sidebar.search_messages')),
        h('button', { class: 'icon-btn', title: tr('common.close'), onClick: close }, icon('close')),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'field' }, this.input),
        this.status,
        this.results,
      ),
    );

    this.el.addEventListener('gipny-modal-close', () => {
      this.closed = true; this.generation++;
      if (this.timer != null) window.clearTimeout(this.timer);
      this.input.value = ''; this.results.replaceChildren(); this.status.textContent = '';
    }, { once: true });
    setTimeout(() => { if (!this.closed) this.input.focus(); }, 50);
  }

  private scheduleSearch(): void {
    if (this.timer != null) window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.runSearch(), 200);
  }

  private async runSearch(): Promise<void> {
    if (this.closed) return;
    const generation = ++this.generation;
    const q = this.input.value.trim();
    if (q.length < 2) {
      this.results.replaceChildren();
      return;
    }
    try {
      const hits = await Api.searchMessages(q, this.scope.contactId, this.scope.groupId, 100);
      if (this.closed || generation !== this.generation) return;
      this.renderResults(hits, q);
    } catch (e) {
      if (this.closed || generation !== this.generation) return;
      this.results.replaceChildren(h('div', { class: 'hint' }, tr('search.search_failed') + errorText(e)));
    }
  }

  private renderResults(hits: SearchHit[], needle: string): void {
    this.results.replaceChildren();
    if (hits.length === 0) {
      this.results.appendChild(h('div', { class: 'hint' }, tr('sidebar.nothing_found')));
      return;
    }
    for (const hit of hits) {
      const where = hit.group_name ? `group: ${hit.group_name}` : (hit.contact_name ? `dm: ${hit.contact_name}` : tr('auth.unknown'));
      const row = h('div', {
        class: 'search-row',
        onClick: () => this.openHit(hit),
      },
        h('div', { class: 'search-row-meta' },
          h('span', { class: 'search-where' }, where),
          h('span', { class: 'search-when' }, `${fmtDate(hit.message.sent_at)} ${fmtTime(hit.message.sent_at)}`),
        ),
        h('div', { class: 'search-snippet' }, this.snippet(hit.message.body, needle)),
      );
      this.results.appendChild(row);
    }
  }

  private snippet(body: string, needle: string): HTMLElement {
    const lc = body.toLowerCase();
    const idx = lc.indexOf(needle.toLowerCase());
    if (idx < 0) return h('span', null, body.slice(0, 200));
    const start = Math.max(0, idx - 30);
    const end = Math.min(body.length, idx + needle.length + 100);
    const before = (start > 0 ? '…' : '') + body.slice(start, idx);
    const match = body.slice(idx, idx + needle.length);
    const after = body.slice(idx + needle.length, end) + (end < body.length ? '…' : '');
    return h('span', null, before, h('mark', null, match), after);
  }

  private openHit(hit: SearchHit): void {
    let target: ChatTarget | null = null;
    if (hit.contact_id != null) target = { kind: 'contact', id: hit.contact_id };
    else if (hit.group_id != null) target = { kind: 'group', id: hit.group_id };
    if (!target) return;
    const messageId = hit.message.id;
    const selecting = this.store.selectChat(target);
    const version = this.store.contentVersion;
    void selecting.then(() => {
      if (version !== this.store.contentVersion || this.store.view.get() !== 'main') return;
      this.store.requestScrollTo(target!, messageId);
    }).catch(() => {});
    this.close();
  }
}
