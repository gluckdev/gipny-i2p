import type { Message } from './api';
import type { Store, ChatTarget } from './state';
import { targetKey } from './state';
import { h } from './view';

export interface MenuItem {
  label: string;
  danger?: boolean;
  onClick: () => void;
}

export class ContextMenu {
  private static current: HTMLElement | null = null;

  static open(x: number, y: number, items: MenuItem[]): void {
    this.close();
    const menu = h('div', { class: 'ctx-menu' });
    for (const item of items) {
      menu.appendChild(h('div', {
        class: 'ctx-menu-item' + (item.danger ? ' danger' : ''),
        onClick: () => { item.onClick(); this.close(); },
      }, item.label));
    }
    document.body.appendChild(menu);
    const rect = menu.getBoundingClientRect();
    const px = Math.min(x, window.innerWidth - rect.width - 8);
    const py = Math.min(y, window.innerHeight - rect.height - 8);
    menu.style.left = px + 'px';
    menu.style.top = py + 'px';
    this.current = menu;
    setTimeout(() => {
      const onDown = (ev: Event) => {
        if (!menu.contains(ev.target as Node)) this.close();
      };
      const onEsc = (ev: KeyboardEvent) => { if (ev.key === 'Escape') this.close(); };
      document.addEventListener('pointerdown', onDown, { once: true });
      document.addEventListener('keydown', onEsc, { once: true });
    }, 0);
  }

  static close(): void {
    this.current?.remove();
    this.current = null;
  }
}

export function attachContextMenu(el: HTMLElement, build: (x: number, y: number) => MenuItem[] | null): void {
  el.addEventListener('contextmenu', (e) => {
    const items = build((e as MouseEvent).clientX, (e as MouseEvent).clientY);
    if (!items || items.length === 0) return;
    e.preventDefault();
    ContextMenu.open((e as MouseEvent).clientX, (e as MouseEvent).clientY, items);
  });
  let pressTimer: number | null = null;
  let startX = 0, startY = 0;
  const cancel = (): void => {
    if (pressTimer != null) { clearTimeout(pressTimer); pressTimer = null; }
  };
  el.addEventListener('pointerdown', (e) => {
    if (e.pointerType !== 'touch') return;
    startX = e.clientX; startY = e.clientY;
    cancel();
    pressTimer = window.setTimeout(() => {
      pressTimer = null;
      const items = build(startX, startY);
      if (!items || items.length === 0) return;
      try { (navigator as Navigator & { vibrate?: (p: number) => boolean }).vibrate?.(15); } catch {}
      ContextMenu.open(startX, startY, items);
    }, 480);
  });
  el.addEventListener('pointermove', (e) => {
    if (Math.abs(e.clientX - startX) > 8 || Math.abs(e.clientY - startY) > 8) cancel();
  });
  el.addEventListener('pointerup', cancel);
  el.addEventListener('pointercancel', cancel);
  el.addEventListener('pointerleave', cancel);
}

export function messageMenuItems(
  store: Store,
  target: ChatTarget,
  m: Message,
  onEdit: () => void,
  onReply: () => void,
): MenuItem[] {
  const pinnedList = store.pinned.get().get(targetKey(target)) ?? [];
  const isPinned = pinnedList.some((p) => p.id === m.id);
  const items: MenuItem[] = [];

  items.push({ label: '[ REPLY ]', onClick: onReply });

  items.push({
    label: isPinned ? '[ UNPIN ]' : '[ PIN ]',
    onClick: () => {
      if (isPinned) store.unpinMessage(target, m.id).catch((e) => store.showToast('unpin failed: ' + e, true));
      else store.pinMessage(target, m.id).catch((e) => store.showToast('pin failed: ' + e, true));
    },
  });

  if (m.outgoing) {
    items.push({ label: '[ EDIT ]', onClick: onEdit });
  }

  if (m.body) {
    items.push({
      label: '[ COPY ]',
      onClick: () => {
        navigator.clipboard.writeText(m.body).catch(() => store.showToast('copy failed', true));
      },
    });
  }

  return items;
}

export class PinnedBanner {
  el: HTMLElement;
  private listEl: HTMLElement;
  private toggleBtn: HTMLButtonElement;
  private expanded: boolean = false;
  private unsub: (() => void) | null = null;
  private items: Message[] = [];

  constructor(private store: Store, private target: ChatTarget) {
    this.listEl = h('div', { class: 'pinned-list' });
    this.toggleBtn = h('button', {
      class: 'pinned-toggle',
      onClick: () => this.toggle(),
      title: 'show all pinned',
    }, '▾') as HTMLButtonElement;

    this.el = h('div', { class: 'pinned-banner', style: { display: 'none' } },
      h('div', { class: 'pinned-header' },
        h('span', { class: 'pinned-label' }, '── PINNED ──'),
        this.toggleBtn,
      ),
      this.listEl,
    );

    this.unsub = store.pinned.subscribe(() => this.render(), true);
  }

  destroy(): void {
    this.unsub?.();
    this.unsub = null;
    this.el.remove();
  }

  private render(): void {
    const key = targetKey(this.target);
    this.items = this.store.pinned.get().get(key) ?? [];
    if (this.items.length === 0) {
      this.el.style.display = 'none';
      return;
    }
    this.el.style.display = 'flex';
    this.listEl.replaceChildren();
    const visible = this.expanded ? this.items : this.items.slice(0, 1);
    for (const m of visible) {
      this.listEl.appendChild(this.renderItem(m));
    }
    const hiddenCount = this.items.length - visible.length;
    if (!this.expanded && this.items.length > 1) {
      this.toggleBtn.textContent = `▾ +${hiddenCount}`;
      this.toggleBtn.style.display = '';
    } else if (this.expanded) {
      this.toggleBtn.textContent = '▴';
      this.toggleBtn.style.display = '';
    } else {
      this.toggleBtn.style.display = 'none';
    }
  }

  private renderItem(m: Message): HTMLElement {
    const preview = m.body.length > 100 ? m.body.slice(0, 100) + '…' : (m.body || '(attachment)');
    return h('div', {
      class: 'pin-item',
      onClick: () => this.store.requestScrollTo(this.target, m.id),
    },
      h('div', { class: 'pin-item-body' }, preview),
      h('button', {
        class: 'pin-item-unpin',
        title: 'unpin',
        onClick: (e: Event) => {
          e.stopPropagation();
          this.store.unpinMessage(this.target, m.id).catch((err) => this.store.showToast('unpin failed: ' + err, true));
        },
      }, '×'),
    );
  }

  private toggle(): void {
    this.expanded = !this.expanded;
    this.render();
  }
}

export class EditInline {
  el: HTMLElement;
  private textarea: HTMLTextAreaElement;
  private onCancel: () => void;
  private onSave: (newBody: string) => void;

  constructor(initial: string, onSave: (newBody: string) => void, onCancel: () => void) {
    this.onSave = onSave;
    this.onCancel = onCancel;
    this.textarea = h('textarea', {
      class: 'edit-inline-textarea',
      rows: '2',
    }) as HTMLTextAreaElement;
    this.textarea.value = initial;
    this.textarea.addEventListener('keydown', (e) => {
      const ke = e as KeyboardEvent;
      if (ke.key === 'Enter' && !ke.shiftKey) { e.preventDefault(); this.save(); }
      if (ke.key === 'Escape') { e.preventDefault(); this.onCancel(); }
    });
    this.textarea.addEventListener('input', () => {
      this.textarea.style.height = 'auto';
      this.textarea.style.height = Math.min(this.textarea.scrollHeight, 180) + 'px';
    });

    this.el = h('div', { class: 'edit-inline' },
      this.textarea,
      h('div', { class: 'edit-inline-actions' },
        h('button', { class: 'btn btn-ghost', onClick: () => this.onCancel() }, '[ cancel ]'),
        h('button', { class: 'btn', onClick: () => this.save() }, '[ save ]'),
      ),
      h('div', { class: 'edit-inline-hint' }, 'enter = save · esc = cancel'),
    );
    setTimeout(() => {
      this.textarea.focus();
      this.textarea.setSelectionRange(initial.length, initial.length);
      this.textarea.style.height = 'auto';
      this.textarea.style.height = Math.min(this.textarea.scrollHeight, 180) + 'px';
    }, 0);
  }

  private save(): void {
    const v = this.textarea.value.trim();
    if (!v) return;
    this.onSave(v);
  }
}

export function scrollLogToMessage(log: HTMLElement, messageId: number): void {
  const el = log.querySelector(`[data-mid="${messageId}"]`) as HTMLElement | null;
  if (!el) return;
  el.scrollIntoView({ behavior: 'smooth', block: 'center' });
  el.classList.add('msg-flash');
  setTimeout(() => el.classList.remove('msg-flash'), 1500);
}
