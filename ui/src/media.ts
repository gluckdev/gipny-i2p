import { errorText } from './error-text';
import { t as tr } from './i18n';
import { save } from '@tauri-apps/plugin-dialog';
import { Api, type MediaItem } from './api';
import type { Store, ChatTarget } from './state';
import { h, fmtDate, fmtTime, humanSize, mimeFromName } from './view';

import { icon } from './icons';
export class MediaModal {
  el: HTMLElement;
  private store: Store;
  private close: () => void;
  private target: ChatTarget;
  private grid: HTMLElement;
  private status: HTMLElement;
  private closed = false;
  private ownedUrls = new Set<string>();

  constructor(store: Store, target: ChatTarget, close: () => void) {
    this.store = store;
    this.close = close;
    this.target = target;

    this.status = h('div', { class: 'hint' }, tr('media.loading'));
    this.grid = h('div', { class: 'media-grid' });

    this.el = h('div', { class: 'modal modal-wide' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, tr('chat.media_and_files')),
        h('button', { class: 'icon-btn', title: tr('common.close'), onClick: close }, icon('close')),
      ),
      h('div', { class: 'modal-body' },
        this.status,
        this.grid,
      ),
    );

    this.el.addEventListener('gipny-modal-close', () => {
      this.closed = true;
      for (const url of this.ownedUrls) URL.revokeObjectURL(url);
      this.ownedUrls.clear();
      this.grid.replaceChildren(); this.status.textContent = '';
    }, { once: true });
    void this.load();
  }

  private async load(): Promise<void> {
    try {
      const items = this.target.kind === 'contact'
        ? await Api.listMediaContact(this.target.id as number, 500)
        : await Api.listMediaGroup(this.target.id as string, 500);
      if (this.closed) return;
      this.render(items);
    } catch (e) {
      if (this.closed) return;
      this.status.textContent = tr('chat.load_failed_2') + errorText(e);
    }
  }

  private render(items: MediaItem[]): void {
    this.status.textContent = items.length === 0
      ? tr('media.no_files_in_this_chat')
      : tr('media.file_count', { n: items.length });
    this.grid.replaceChildren();
    for (const it of items) {
      const mime = mimeFromName(it.name);
      const isImage = mime.startsWith('image/');
      const tile = h('div', {
        class: 'media-tile' + (isImage ? ' is-image' : ''),
        title: `${it.name}\n${humanSize(it.size)}\n${fmtDate(it.sent_at)} ${fmtTime(it.sent_at)}`,
      });
      const preview = h('div', { class: 'media-preview' });
      tile.appendChild(preview);
      tile.appendChild(h('div', { class: 'media-name' }, it.name));
      tile.appendChild(h('div', { class: 'media-sub' },
        h('span', null, humanSize(it.size)),
        h('span', null, fmtDate(it.sent_at)),
      ));
      tile.appendChild(h('button', {
        class: 'btn btn-ghost btn-sm',
        onClick: async (ev: Event) => {
          ev.stopPropagation();
          try {
            const dest = await save({ defaultPath: it.name });
            if (!dest || this.closed) return;
            await Api.saveAttachment(it.id, dest as string);
            this.store.showToast(tr('contact.saved'));
          } catch (e) {
            this.store.showToast(tr('media.save_failed') + errorText(e), true);
          }
        },
      }, tr('common.save')));
      tile.addEventListener('click', () => this.openInChat(it));
      if (isImage) void this.fillPreview(preview, it.id, mime);
      this.grid.appendChild(tile);
    }
  }

  private async fillPreview(into: HTMLElement, attId: number, mime: string): Promise<void> {
    try {
      const b64 = await Api.loadAttachment(attId);
      if (this.closed || !into.isConnected) return;
      const bytes = Uint8Array.from(atob(b64), (character) => character.charCodeAt(0));
      const blob = new Blob([bytes], { type: mime }); bytes.fill(0);
      const url = URL.createObjectURL(blob); this.ownedUrls.add(url);
      const img = h('img', { src: url });
      into.replaceChildren(img);
    } catch {
      if (this.closed) return;
      into.textContent = '⨯';
    }
  }

  private openInChat(it: MediaItem): void {
    this.close();
    this.store.requestScrollTo(this.target, it.message_id);
  }
}
