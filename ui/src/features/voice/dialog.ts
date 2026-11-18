/** Keep a ringing call or recording reachable when the composer had focus. */
export function focusVoiceDialog(root: HTMLElement, close: () => void): HTMLElement {
  const previous = document.activeElement;
  root.tabIndex = -1;
  requestAnimationFrame(() => { if (root.isConnected) root.focus(); });
  root.addEventListener('keydown', event => {
    if (event.key === 'Escape') { event.preventDefault(); close(); return; }
    if (event.key !== 'Tab') return;
    const controls = [...root.querySelectorAll<HTMLElement>('button:enabled:not([hidden]), audio[controls]')];
    const first = controls[0], last = controls[controls.length - 1];
    if (event.shiftKey && (document.activeElement === first || document.activeElement === root)) {
      event.preventDefault(); last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault(); first?.focus();
    }
  });
  root.addEventListener('gipny-modal-close', () => {
    if (previous instanceof HTMLElement && previous.isConnected) previous.focus({ preventScroll: true });
  }, { once: true });
  return root;
}
