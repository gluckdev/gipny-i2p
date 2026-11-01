import { configureBrowserBackend } from './api';
import { BrowserCoreClient } from './browser-core/client';
import { clearBrowserFiles } from './browser-files';

const client = new BrowserCoreClient();
configureBrowserBackend(client);
await client.configure();
window.addEventListener('pagehide', () => { client.terminate(); clearBrowserFiles(); document.getElementById('app')?.replaceChildren(); });
document.addEventListener('freeze', () => { client.terminate(); window.dispatchEvent(new Event('gipny-browser-locked')); });
window.addEventListener('pageshow', event => { if (event.persisted) window.dispatchEvent(new Event('gipny-browser-locked')); });
window.addEventListener('gipny-browser-locked', () => { window.location.reload(); });
// The shared authentication/settings views call these after a user gesture,
// which WebAuthn requires. No key or PRF output is retained on window.
window.addEventListener('gipny-enroll-passkey', event => {
  const detail = (event as CustomEvent<{ profile: string; done(error?: unknown): void }>).detail;
  void client.enrollPasskey(detail.profile).then(() => detail.done(), error => detail.done(error));
});
window.addEventListener('gipny-unlock-passkey', event => {
  const detail = (event as CustomEvent<{ profile: string; done(error?: unknown): void }>).detail;
  void client.unlockPasskey(detail.profile).then(() => detail.done(), error => detail.done(error));
});
await import('./main');
if ('serviceWorker' in navigator) void navigator.serviceWorker.register(new URL('browser-sw.js', document.baseURI), { scope: './' });
