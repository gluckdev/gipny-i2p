import { configureWebTransport, connectWebTransport, invoke } from './api';
import { clearWebPairing, readWebEndpoint, resolveWebEndpoint, saveWebPairing } from './web-transport';
import { h } from './view';
import { t, onLangChange } from './i18n';
import { languagePicker } from './language-picker';

/** Web-only gate: no Store/bootstrap or vault calls before a local handshake. */
export async function pairWeb(root: HTMLElement): Promise<void> {
  let endpoint = readWebEndpoint(window);
  let failed = false;
  const remember = (url: string): void => {
    const connection = new URL(url);
    saveWebPairing(connection.port || '80', connection.searchParams.get('token')!);
  };
  if (endpoint) {
    configureWebTransport(endpoint);
    try {
      await connectWebTransport(); await invoke('web_info'); remember(endpoint); return;
    } catch { configureWebTransport(null); clearWebPairing(); failed = true; }
  }
  return new Promise((resolve) => {
    const port = h('input', { class: 'input', type: 'number', min: '1', max: '65535', required: true, autocomplete: 'off', name: 'port' });
    const token = h('input', { class: 'input', type: 'password', required: true, autocomplete: 'off', name: 'token' });
    const hint = h('div', { class: 'err', role: 'status' });
    const button = h('button', { class: 'btn btn-block', type: 'submit' });
    const title = h('h1', { class: 'auth-title' });
    const description = h('p', { class: 'auth-sub' });
    const portLabel = h('span'), tokenLabel = h('span');
    const translate = (): void => {
      title.textContent = t('web.title'); description.textContent = t('web.description');
      portLabel.textContent = t('web.port'); tokenLabel.textContent = t('web.token');
      button.textContent = t('web.connect'); hint.textContent = failed ? t('web.failed') : '';
    };
    const unsubscribe = onLangChange(translate);
    const form = h('form', { class: 'auth-card' }, languagePicker(), title, description,
      h('label', { class: 'field' }, portLabel, port), h('label', { class: 'field' }, tokenLabel, token), hint, button);
    translate();
    root.replaceChildren(h('div', { class: 'auth' }, form));
    form.addEventListener('submit', async (event) => {
      event.preventDefault();
      endpoint = resolveWebEndpoint(window.location, { port: port.value.trim(), token: token.value.trim() });
      if (!endpoint) { failed = true; translate(); return; }
      button.disabled = true;
      configureWebTransport(endpoint);
      try {
        await connectWebTransport(); await invoke('web_info'); remember(endpoint);
        token.value = ''; unsubscribe(); root.replaceChildren(); resolve();
      } catch { configureWebTransport(null); clearWebPairing(); failed = true; translate(); }
      finally { button.disabled = false; }
    });
  });
}
