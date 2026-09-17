import { h } from './view';
import { AVATARS, avatarIndex, isPicked, onAvatarsChanged, paintAvatar, setAvatar } from './avatars';

/** A grid of all avatars for one key; the current one is marked. `note` says
 * who sees the choice. Modals have no teardown hook, so it lets go of its
 * listener once it has been on screen and is gone again. */
export function avatarPicker(seed: string, note: string, onError: (e: unknown) => void): HTMLElement {
  const grid = h('div', { class: 'avatar-grid', role: 'listbox' });
  const reset = h('button', { class: 'btn btn-sm btn-ghost' }, 'Случайная');
  let shown = false;
  const paint = () => {
    if (grid.isConnected) shown = true;
    else if (shown) { dispose(); return; }
    const current = avatarIndex(seed);
    grid.replaceChildren(...AVATARS.map((title, i) => {
      const tile = h('button', {
        class: 'avatar avatar-choice' + (i === current ? ' selected' : ''),
        title, 'aria-label': title, role: 'option', 'aria-selected': String(i === current),
        onClick: () => setAvatar(seed, i).catch(onError),
      });
      paintAvatar(tile, i);
      return tile;
    }));
    reset.toggleAttribute('disabled', !isPicked(seed));
  };
  reset.addEventListener('click', () => setAvatar(seed, null).catch(onError));
  const dispose = onAvatarsChanged(paint);
  paint();
  return h('div', { class: 'avatar-picker' },
    grid,
    h('div', { class: 'row-between avatar-picker-foot' }, h('div', { class: 'hint' }, note), reset),
  );
}
