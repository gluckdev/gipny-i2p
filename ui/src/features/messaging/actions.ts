import { t as tr } from '../localization/i18n';
import { Api, PEER_EVENT } from '../../app/api';
import type { Message } from '../../app/api';
import type { Store, ChatTarget } from '../../app/state';
import { targetKey } from '../../app/state';
import { h } from '../../app/view';

export interface MenuItem {
  label: string;
  danger?: boolean;
  onClick: () => void;
}

export class ContextMenu {
  private static current: HTMLElement | null = null;
  private static listenerTimer: number | null = null;
  private static removeListeners: (() => void) | null = null;
  private static trigger: HTMLElement | null = null;

  static open(x: number, y: number, items: MenuItem[]): void {
    this.close();
    this.trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const menu = h('div', { class: 'ctx-menu', role: 'menu', 'aria-label': tr('chat.more_actions') });
    for (const item of items) {
      menu.appendChild(h('button', {
        type: 'button', role: 'menuitem', tabindex: '-1',
        class: 'ctx-menu-item' + (item.danger ? ' danger' : ''),
        onClick: () => { this.close(true); item.onClick(); },
      }, item.label));
    }
    document.body.appendChild(menu);
    const rect = menu.getBoundingClientRect();
    const px = Math.max(8, Math.min(x, window.innerWidth - rect.width - 8));
    const py = Math.max(8, Math.min(y, window.innerHeight - rect.height - 8));
    menu.style.left = px + 'px';
    menu.style.top = py + 'px';
    this.current = menu;
    const buttons = Array.from(menu.querySelectorAll('button'));
    buttons[0]?.focus({ preventScroll: true });
    menu.addEventListener('keydown', event => {
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      switch (event.key) {
        case 'ArrowDown': buttons[(index + 1) % buttons.length]?.focus(); break;
        case 'ArrowUp': buttons[(index + buttons.length - 1) % buttons.length]?.focus(); break;
        case 'Home': buttons[0]?.focus(); break;
        case 'End': buttons[buttons.length - 1]?.focus(); break;
        case 'Escape': this.close(true); break;
        case 'Tab': this.close(true); return;
        default: return;
      }
      event.preventDefault();
      event.stopPropagation();
    });
    this.listenerTimer = window.setTimeout(() => {
      this.listenerTimer = null;
      if (this.current !== menu) return;
      const onDown = (ev: Event) => {
        if (!menu.contains(ev.target as Node)) this.close();
      };
      const onEsc = (ev: KeyboardEvent) => { if (ev.key === 'Escape') this.close(true); };
      document.addEventListener('pointerdown', onDown);
      document.addEventListener('keydown', onEsc);
      this.removeListeners = () => {
        document.removeEventListener('pointerdown', onDown);
        document.removeEventListener('keydown', onEsc);
      };
    }, 0);
  }

  static close(restoreFocus = false): void {
    if (this.listenerTimer != null) { window.clearTimeout(this.listenerTimer); this.listenerTimer = null; }
    this.removeListeners?.(); this.removeListeners = null;
    this.current?.replaceChildren();
    this.current?.remove();
    this.current = null;
    if (restoreFocus && this.trigger?.isConnected) this.trigger.focus({ preventScroll: true });
    this.trigger = null;
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
      if (!el.isConnected) return;
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
  onForward: () => void,
  onDelete?: () => void,
): MenuItem[] {
  if (m.deleted_at != null) return [];
  const pinnedList = store.pinned.get().get(targetKey(target)) ?? [];
  const isPinned = pinnedList.some((p) => p.id === m.id);
  const items: MenuItem[] = [];

  items.push({ label: tr('chat.reply'), onClick: onReply });
  items.push({ label: tr('chat.forward'), onClick: onForward });

  items.push({
    label: isPinned ? tr('chat.unpin') : tr('chat.pin'),
    onClick: () => {
      if (isPinned) store.unpinMessage(target, m.id).catch((e) => store.showToast(tr('actions.unpin_failed') + e, true));
      else store.pinMessage(target, m.id).catch((e) => store.showToast(tr('actions.pin_failed') + e, true));
    },
  });

  if (m.outgoing) {
    items.push({ label: tr('actions.change'), onClick: onEdit });
  }

  if (m.body) {
    items.push({
      label: tr('common.copy'),
      onClick: () => {
        navigator.clipboard.writeText(m.body).then(() => {
          // The contact learns their text was copied; our own needs no report.
          if (target.kind === 'contact' && !m.outgoing) void Api.reportPeerEvent(m.id, PEER_EVENT.copied).catch(() => {});
        }).catch(() => store.showToast(tr('actions.copy_failed'), true));
      },
    });
  }

  if (target.kind === 'contact' && onDelete) {
    items.push({ label: tr('chat.delete_for_both'), danger: true, onClick: onDelete });
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
      title: tr('actions.show_all_pinned'),
      'aria-label': tr('actions.pinned_messages'),
    }, '▾') as HTMLButtonElement;

    this.el = h('div', { class: 'pinned-banner', style: { display: 'none' } },
      h('div', { class: 'pinned-header' },
        h('span', { class: 'pinned-label' }, tr('actions.pinned_messages')),
        this.toggleBtn,
      ),
      this.listEl,
    );

    this.unsub = store.pinned.subscribe(() => this.render(), true);
  }

  destroy(): void {
    this.unsub?.();
    this.unsub = null;
    this.items = [];
    this.el.replaceChildren();
    this.el.remove();
  }

  private render(): void {
    const key = targetKey(this.target);
    this.items = this.store.pinned.get().get(key) ?? [];
    if (this.items.length === 0) {
      this.listEl.replaceChildren();
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
    this.toggleBtn.setAttribute('aria-expanded', String(this.expanded));
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
    const preview = m.body.length > 100 ? m.body.slice(0, 100) + '…' : (m.body || tr('chat.attachment'));
    return h('div', { class: 'pin-item' },
      h('button', { type: 'button', class: 'pin-item-body', title: preview,
        onClick: () => this.store.requestScrollTo(this.target, m.id) }, preview),
      h('button', {
        class: 'pin-item-unpin',
        title: tr('chat.unpin'), 'aria-label': tr('chat.unpin'),
        onClick: () => {
          this.store.unpinMessage(this.target, m.id).catch((err) => this.store.showToast(tr('actions.unpin_failed') + err, true));
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
        h('button', { class: 'btn btn-ghost', onClick: () => this.onCancel() }, tr('common.cancel')),
        h('button', { class: 'btn', onClick: () => this.save() }, tr('common.save')),
      ),
      h('div', { class: 'edit-inline-hint' }, tr('actions.enter_to_save_esc_to_cancel')),
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
  const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  el.scrollIntoView({ behavior: reduce ? 'auto' : 'smooth', block: 'center' });
  el.classList.add('msg-flash');
  setTimeout(() => el.classList.remove('msg-flash'), 1500);
}
