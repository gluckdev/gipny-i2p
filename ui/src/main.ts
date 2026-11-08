import './compat';
import { isTauri, usesBrowserCore } from './api';
import { clearBrowserFiles } from './browser-files';
import { Store } from './state';
import { App } from './app';
import { SearchModal } from './search';
import { applyTheme } from './theme';
import { installOsk, setViewportOffset } from './osk';
import { errorText } from './error-text';

// theme-init.js already set this before paint; re-apply in case it was blocked.
applyTheme();
// On a phone the app's own keyboard replaces the system one: mounted before
// the first screen renders, so every field it draws is already covered.
installOsk();

const root = document.getElementById('app');
if (!root) throw new Error('no #app');

let store: Store;
let app: App;
async function start(): Promise<void> {
  if (!isTauri && !usesBrowserCore()) throw new Error('Open browser.html to use the browser messenger');
  store = new Store();
  app = new App(store);
  root!.replaceChildren(app.el);
  await store.bootstrap();
}
void start().catch(error => {
  clearBrowserFiles();
  const message = document.createElement('div');
  message.className = 'auth err'; message.textContent = errorText(error);
  root!.replaceChildren(message);
});

window.addEventListener('keydown', (e) => {
  if (!store || !app) return;
  if (e.key === 'F5' || (e.ctrlKey && (e.key === 'r' || e.key === 'R'))) {
    e.preventDefault();
    e.stopPropagation();
    if (store.view.get() === 'main' || store.view.get() === 'auth-booting') {
      store.lock().catch(() => {});
    }
    return;
  }
  if (store.view.get() !== 'main') return;
  if (app.isModalActive()) return;
  if (e.key === 'Escape') {
    if (store.selectedChat.get() != null) {
      e.preventDefault();
      store.selectedChat.set(null);
    }
    return;
  }
  if (e.ctrlKey && e.code === 'KeyF') {
    e.preventDefault();
    const target = store.selectedChat.get();
    const scope = target
      ? (target.kind === 'contact'
          ? { contactId: target.id as number, groupId: null }
          : { contactId: null, groupId: target.id as string })
      : undefined;
    app.openModal((close) => new SearchModal(store, close, scope).el);
  }
}, true);

// A system keyboard, where one still appears (a desktop with a touch screen,
// a browser without the app's keyboard): --kbd-offset follows the viewport.
// osk.ts merges this with its own panel's height.
const vv = window.visualViewport;
if (vv) {
  const update = (): void => setViewportOffset(window.innerHeight - vv.height - vv.offsetTop);
  vv.addEventListener('resize', update);
  vv.addEventListener('scroll', update);
}
