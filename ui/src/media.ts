import { save } from '@tauri-apps/plugin-dialog';
import { Api, type MediaItem } from './api';
import type { Store, ChatTarget } from './state';
import { h, fmtDate, fmtTime, humanSize, mimeFromName } from './view';

export class MediaModal {
  el: HTMLElement;
  private store: Store;
  private close: () => void;
  private target: ChatTarget;
  private grid: HTMLElement;
  private status: HTMLElement;

  constructor(store: Store, target: ChatTarget, close: () => void) {
    this.store = store;
    this.close = close;
    this.target = target;

    this.status = h('div', { class: 'hint' }, 'loading…');
    this.grid = h('div', { class: 'media-grid' });

    this.el = h('div', { class: 'modal modal-wide' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, '── media ──'),
        h('button', { class: 'icon-btn', onClick: close }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        this.status,
        this.grid,
      ),
    );

    void this.load();
  }

  private async load(): Promise<void> {
    try {
      const items = this.target.kind === 'contact'
        ? await Api.listMediaContact(this.target.id as number, 500)
        : await Api.listMediaGroup(this.target.id as string, 500);
      this.render(items);
    } catch (e) {
      this.status.textContent = 'load failed: ' + String(e);
    }
  }

  private render(items: MediaItem[]): void {
    this.status.textContent = items.length === 0
      ? 'no files in this chat'
      : `${items.length} file${items.length === 1 ? '' : 's'}`;
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
            if (!dest) return;
            await Api.saveAttachment(it.id, dest as string);
            this.store.showToast('saved');
          } catch (e) {
            this.store.showToast('save failed: ' + String(e), true);
          }
        },
      }, '[ SAVE ]'));
      tile.addEventListener('click', () => this.openInChat(it));
      if (isImage) void this.fillPreview(preview, it.id);
      this.grid.appendChild(tile);
    }
  }

  private async fillPreview(into: HTMLElement, attId: number): Promise<void> {
    try {
      const b64 = await Api.loadAttachment(attId);
      const img = h('img', { src: `data:application/octet-stream;base64,${b64}` });
      into.replaceChildren(img);
    } catch {
      into.textContent = '⨯';
    }
  }

  private openInChat(it: MediaItem): void {
    setTimeout(() => {
      this.store.scrollToMessage.set({ target: this.target, messageId: it.message_id, nonce: Date.now() });
    }, 50);
    this.close();
  }
}
