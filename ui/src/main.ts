import { Store } from './state';
import { App } from './app';

const root = document.getElementById('app');
if (!root) throw new Error('no #app');

const store = new Store();
const app = new App(store);
root.appendChild(app.el);
store.bootstrap();

window.addEventListener('keydown', (e) => {
  if (e.key === 'F5' || (e.ctrlKey && (e.key === 'r' || e.key === 'R'))) {
    e.preventDefault();
    e.stopPropagation();
    if (store.view.get() === 'main' || store.view.get() === 'auth-booting') {
      store.lock().catch(() => {});
    }
  }
}, true);

const vv = window.visualViewport;
if (vv) {
  const update = (): void => {
    const offset = Math.max(0, window.innerHeight - vv.height - vv.offsetTop);
    document.body.style.setProperty('--kbd-offset', offset + 'px');
    if (offset > 0) {
      const log = document.querySelector<HTMLElement>('.chat-log');
      if (log) requestAnimationFrame(() => { log.scrollTop = log.scrollHeight; });
    }
  };
  vv.addEventListener('resize', update);
  vv.addEventListener('scroll', update);
}
