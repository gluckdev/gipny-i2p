import { configureBrowserBackend } from './app/api';
import { BrowserCoreClient } from './browser-core/client';
import { clearBrowserFiles } from './features/files/browser-files';

const client = new BrowserCoreClient();
configureBrowserBackend(client);
// UI, configuration and the fresh worker's JS/WASM load concurrently. Profile
// commands that need networking wait on the client's configuration barrier.
void client.prepare().catch(() => { /* The ordinary auth path reports failures. */ });
window.addEventListener('online', () => { void client.reconnect().catch(() => {}); });
window.addEventListener('pagehide', () => { client.terminate(); clearBrowserFiles(); document.getElementById('app')?.replaceChildren(); });
window.addEventListener('pageshow', event => { if (event.persisted) window.dispatchEvent(new Event('gipny-browser-locked')); });
window.addEventListener('gipny-browser-locked', () => { window.location.reload(); });
// The shared authentication/settings views call these after a user gesture,
// which WebAuthn requires. No key or PRF output is retained on window.
window.addEventListener('gipny-enroll-passkey', event => {
  const detail = (event as CustomEvent<{ profile: string; done(error?: unknown): void }>).detail;
  void client.enrollPasskey(detail.profile).then(() => detail.done(), error => detail.done(error));
});
window.addEventListener('gipny-unlock-passkey', event => {
  const detail = (event as CustomEvent<{ profile: string; totpCode?: string; screenOnly?: boolean; done(error?: unknown): void }>).detail;
  void client.unlockPasskey(detail.profile, detail.totpCode, detail.screenOnly).then(() => detail.done(), error => detail.done(error));
});
const { installNativeHandoff } = await import('./features/native-handoff');
installNativeHandoff();
await import('./main');
if ('serviceWorker' in navigator) void navigator.serviceWorker.register(new URL('browser-sw.js', document.baseURI), { scope: './' });
