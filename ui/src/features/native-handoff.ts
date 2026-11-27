import { h } from '../app/view';
import { onLangChange, t } from './localization/i18n';

const releases = 'https://github.com/gluckdev/gipny-i2p/releases';
// These versioned assets exist even when GitHub's /latest has no stable release.
const downloadBase = releases + '/download/v0.5.0-rc3/gipny-i2p_0.5.0-rc3_';
const hideKey = 'gipny:hide-native-prompt';
const handoffKey = 'gipny:native-handoff';
const handoffTimeout = 1500;

type ArchitectureHint = { architecture?: string; bitness?: string };
type Device = Pick<Navigator, 'userAgent' | 'platform' | 'maxTouchPoints'> & {
  userAgentData?: {
    platform?: string;
    getHighEntropyValues?(keys: string[]): Promise<ArchitectureHint>;
  };
};

export function nativeDownload(device: Device, hint?: ArchitectureHint) {
  const system = [device.userAgentData?.platform, device.platform, device.userAgent].join(' ');
  if (/iPhone|iPad|iPod|CrOS|Chrome ?OS/i.test(system) || (/Mac/i.test(system) && device.maxTouchPoints > 1)) return null;
  const arm64 = hint?.architecture ? hint.architecture === 'arm' && hint.bitness === '64' : /arm64|aarch64/i.test(system);
  const arm32 = hint?.architecture ? hint.architecture === 'arm' && hint.bitness === '32' : /armv[67]/i.test(system);
  if (/Android/i.test(system)) {
    if ((device.userAgentData?.platform === 'Android' && hint?.architecture === 'x86') ||
      /\b(?:x86(?:_64)?|i[3-6]86)\b/i.test(device.userAgent)) return null;
    return { os: 'Android', format: '.apk', url: downloadBase + (arm32 ? 'android-armv7.apk' : 'android-arm64.apk') };
  }
  if (arm32 || hint?.bitness === '32' || /\bi[3-6]86\b/i.test(system)) return null;
  if (/Windows|Win32|Win64/i.test(system)) return { os: 'Windows', format: '.exe', url: downloadBase + 'x64-setup.exe' };
  if (/Mac/i.test(system)) return { os: 'macOS', format: '.dmg', url: downloadBase + (arm64 ? 'aarch64.dmg' : 'x64.dmg') };
  if (/Linux/i.test(system)) return { os: 'Linux', format: '.AppImage', url: downloadBase + (arm64 ? 'aarch64.AppImage' : 'amd64.AppImage') };
  return null;
}

export function installNativeHandoff(): void {
  const device: Device = navigator;
  let build = nativeDownload(device);
  const launch = h('button', { type: 'button', class: 'btn native-handoff-launch' });
  const close = h('button', { type: 'button', class: 'native-handoff-close' }, '×');
  const dismiss = h('a', { href: '#native-prompt' });
  const prompt = h('p', { id: 'native-prompt-text' });
  const download = h('a', { class: 'btn native-download', rel: 'noreferrer', target: '_blank' });
  const allBuilds = h('a', { class: 'native-all-builds', href: releases, rel: 'noreferrer', target: '_blank' });
  const card = h('aside', { id: 'native-prompt', 'aria-labelledby': prompt.id, hidden: true },
    h('div', { class: 'native-prompt-actions' }, dismiss, close), prompt, download, allBuilds);
  const header = h('header', { class: 'native-handoff-bar' }, launch, card);
  document.body.prepend(header);

  function localize(): void {
    launch.textContent = t('native.open');
    close.setAttribute('aria-label', t('common.close'));
    dismiss.textContent = t('native.never');
    prompt.textContent = t(build ? 'native.prompt' : 'native.no_build');
    download.hidden = !build;
    if (build) {
      download.href = build.url;
      download.textContent = t('native.download', { os: build.os, format: build.format });
    } else download.removeAttribute('href');
    allBuilds.textContent = t('native.all_builds');
  }
  localize();
  const unsubscribe = onLangChange(localize);
  device.userAgentData?.getHighEntropyValues?.(['architecture', 'bitness']).then(hint => {
    build = nativeDownload(device, hint);
    localize();
  }).catch(() => { /* Continue with the locally reported platform. */ });

  // A manual popover leaves the messenger interactive and preserves the
  // focused field on startup. Older supported browsers use the same inline card.
  const supportsPopover = typeof card.showPopover === 'function';
  if (supportsPopover) card.setAttribute('popover', 'manual');
  function showCard(): void {
    card.hidden = false;
    if (supportsPopover) card.showPopover();
  }
  function hideCard(): void {
    const restoreFocus = card.contains(document.activeElement);
    if (supportsPopover && !card.hidden) card.hidePopover();
    card.hidden = true;
    if (restoreFocus) launch.focus({ preventScroll: true });
  }
  close.onclick = hideCard;
  dismiss.onclick = event => {
    event.preventDefault();
    try { window.localStorage.setItem(hideKey, '1'); } catch { /* Storage may be disabled. */ }
    hideCard();
  };
  function closeOnEscape(event: KeyboardEvent): void {
    if (event.key !== 'Escape' || !(event.target instanceof Node) || !card.contains(event.target)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    hideCard();
  }
  // Handle Escape in the card before the shared chat shortcut clears a conversation.
  window.addEventListener('keydown', closeOnEscape, true);

  let timer: number | undefined;
  let waitingForFocus = false;
  function endAttempt(): void {
    window.clearTimeout(timer);
    timer = undefined;
    waitingForFocus = false;
  }
  function leftPage(): void {
    if (timer === undefined && !waitingForFocus) return;
    endAttempt();
    try { window.sessionStorage.setItem(handoffKey, '1'); } catch { /* The card is already hidden. */ }
  }
  function onVisibility(): void {
    if (document.visibilityState === 'hidden') leftPage();
  }
  function fallback(): void {
    endAttempt();
    try { window.sessionStorage.removeItem(handoffKey); } catch { /* Storage may be disabled. */ }
    showCard();
    if (document.activeElement === launch) (build ? download : allBuilds).focus();
  }
  function onFocus(): void {
    if (waitingForFocus && document.visibilityState === 'visible') fallback();
  }
  document.addEventListener('visibilitychange', onVisibility);
  window.addEventListener('focus', onFocus);
  launch.onclick = () => {
    endAttempt();
    hideCard();
    timer = window.setTimeout(() => {
      if (document.visibilityState === 'hidden') leftPage();
      else if (document.hasFocus()) fallback();
      // A browser permission sheet can temporarily take focus. Wait for its
      // dismissal or a visibility change before deciding the outcome.
      else { timer = undefined; waitingForFocus = true; }
    }, handoffTimeout);
    try { window.location.assign('gipny://open'); } catch { fallback(); }
  };

  let suppressed = false;
  try { suppressed = window.localStorage.getItem(hideKey) !== null; } catch { /* Storage may be disabled. */ }
  try { suppressed ||= window.sessionStorage.getItem(handoffKey) === '1'; } catch { /* Same for tab storage. */ }
  if (!suppressed) showCard();
  window.addEventListener('pagehide', () => {
    leftPage();
    unsubscribe();
    window.removeEventListener('keydown', closeOnEscape, true);
    document.removeEventListener('visibilitychange', onVisibility);
    window.removeEventListener('focus', onFocus);
    header.remove();
  }, { once: true });
}
