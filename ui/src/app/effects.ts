/** Optional local resource hints; these values are never stored or sent. */
interface EffectsNavigator {
  hardwareConcurrency?: number;
  deviceMemory?: number;
  connection?: { saveData?: boolean; addEventListener?: EventTarget['addEventListener']; removeEventListener?: EventTarget['removeEventListener'] };
}

export function reducedEffects(nav: EffectsNavigator): boolean {
  return nav.connection?.saveData === true ||
    (typeof nav.hardwareConcurrency === 'number' && nav.hardwareConcurrency > 0 && nav.hardwareConcurrency <= 2) ||
    (typeof nav.deviceMemory === 'number' && nav.deviceMemory > 0 && nav.deviceMemory <= 2);
}

/** Low-resource hints supplement CSS accessibility/slow-display preferences.
 * They are not a claim to detect the operating system's battery-saver mode. */
export function installEffects(root: HTMLElement = document.documentElement, nav: EffectsNavigator = navigator): () => void {
  const update = (): void => { root.classList.toggle('reduced-effects', reducedEffects(nav)); };
  update();
  nav.connection?.addEventListener?.('change', update);
  return () => { nav.connection?.removeEventListener?.('change', update); root.classList.remove('reduced-effects'); };
}
