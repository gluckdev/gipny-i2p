import { Api, type SearchHit } from './api';
import type { Store, ChatTarget } from './state';
import { h, fmtTime, fmtDate } from './view';

export class SearchModal {
  el: HTMLElement;
  private input: HTMLInputElement;
  private results: HTMLElement;
  private status: HTMLElement;
  private timer: number | null = null;
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
      placeholder: 'type to search…',
      onInput: () => this.scheduleSearch(),
    }) as HTMLInputElement;

    const scopeLabel = this.scope.contactId != null
      ? `in this contact`
      : this.scope.groupId != null
        ? `in this group`
        : `global (all chats)`;

    this.status = h('div', { class: 'hint' }, scopeLabel);
    this.results = h('div', { class: 'search-results' });

    this.el = h('div', { class: 'modal modal-wide' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, '── search ──'),
        h('button', { class: 'icon-btn', onClick: close }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'field' }, this.input),
        this.status,
        this.results,
      ),
    );

    setTimeout(() => this.input.focus(), 50);
  }

  private scheduleSearch(): void {
    if (this.timer != null) window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.runSearch(), 200);
  }

  private async runSearch(): Promise<void> {
    const q = this.input.value.trim();
    if (q.length < 2) {
      this.results.replaceChildren();
      return;
    }
    try {
      const hits = await Api.searchMessages(q, this.scope.contactId, this.scope.groupId, 100);
      this.renderResults(hits, q);
    } catch (e) {
      this.results.replaceChildren(h('div', { class: 'hint' }, 'search failed: ' + String(e)));
    }
  }

  private renderResults(hits: SearchHit[], needle: string): void {
    this.results.replaceChildren();
    if (hits.length === 0) {
      this.results.appendChild(h('div', { class: 'hint' }, 'no matches'));
      return;
    }
    for (const hit of hits) {
      const where = hit.group_name ? `group: ${hit.group_name}` : (hit.contact_name ? `dm: ${hit.contact_name}` : 'unknown');
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
    this.store.selectedChat.set(target);
    setTimeout(() => {
      this.store.scrollToMessage.set({ target: target!, messageId: hit.message.id, nonce: Date.now() });
    }, 100);
    this.close();
  }
}
