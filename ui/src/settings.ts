import { save } from '@tauri-apps/plugin-dialog';
import { Api } from './api';
import type { Store } from './state';
import { h, busy, humanSize } from './view';
import type { App } from './app';

export class SettingsModal {
  el: HTMLElement;
  constructor(store: Store, app: App, close: () => void) {
    const oldP = h('input', { class: 'input', type: 'password', placeholder: 'current passphrase' });
    const newP = h('input', { class: 'input', type: 'password', placeholder: 'new passphrase' });
    const newP2 = h('input', { class: 'input', type: 'password', placeholder: 'confirm new' });
    const passErr = h('div', { class: 'err' });

    const currP = h('input', { class: 'input', type: 'password', placeholder: 'current passphrase' });
    const duP = h('input', { class: 'input', type: 'password', placeholder: 'duress passphrase (empty = remove)' });
    const duWipe = h('input', { type: 'checkbox', checked: true });
    const duErr = h('div', { class: 'err' });

    const attP = h('input', { class: 'input', type: 'password', placeholder: 'current passphrase' });
    const attN = h('input', { class: 'input', type: 'number', value: '10', min: '0' });
    const attErr = h('div', { class: 'err' });

    const verSlot = h('div', { class: 'card-block' }, 'loading...');
    Api.currentVersion().then((v) => { verSlot.textContent = `gipny v${v}`; }).catch(() => { verSlot.textContent = '?'; });
    const updErr = h('div', { class: 'err' });

    const proxyKindSel = h('select', { class: 'input' },
      h('option', { value: 'none' }, 'none (direct)'),
      h('option', { value: 'socks5' }, 'socks5'),
    ) as HTMLSelectElement;
    const proxyHost = h('input', { class: 'input', placeholder: 'host (e.g. 127.0.0.1)' }) as HTMLInputElement;
    const proxyPort = h('input', { class: 'input', type: 'number', placeholder: 'port', min: '1', max: '65535' }) as HTMLInputElement;
    const proxyUser = h('input', { class: 'input', placeholder: 'username (optional)' }) as HTMLInputElement;
    const proxyPass = h('input', { class: 'input', type: 'password', placeholder: 'password (optional)' }) as HTMLInputElement;
    const proxyErr = h('div', { class: 'err' });
    Api.getProxyConfig().then((cfg) => {
      proxyKindSel.value = cfg.kind === 'socks5' ? 'socks5' : 'none';
      proxyHost.value = cfg.host ?? '';
      proxyPort.value = cfg.port ? String(cfg.port) : '';
      proxyUser.value = cfg.user ?? '';
      proxyPass.value = cfg.pass ?? '';
    }).catch(() => {});

    const apkInfo = h('div', { class: 'hint', style: { marginBottom: '6px' } }, 'fetching APK info...');
    const apkButtons = h('div', { class: 'row' });
    const apkErr = h('div', { class: 'err' });
    const apkProgress = h('div', { class: 'hint', style: { marginTop: '4px' } });

    const renderApk = (version: string, items: { arch: string; size: number }[]): void => {
      apkInfo.textContent = items.length === 0
        ? 'no APK in current release'
        : `gipny v${version} for android · sideload .apk for your phone`;
      apkButtons.replaceChildren();
      for (const it of items) {
        const btn = h('button', {
          class: 'btn btn-ghost',
          onClick: () => busy(btn as HTMLButtonElement, async () => {
            apkErr.textContent = '';
            apkProgress.textContent = '';
            try {
              const fname = `gipny-${version}-android-${it.arch}.apk`;
              const dest = await save({ defaultPath: fname, filters: [{ name: 'APK', extensions: ['apk'] }] });
              if (!dest) return;
              apkProgress.textContent = `downloading ${humanSize(it.size)}...`;
              await Api.downloadApk(it.arch, dest);
              apkProgress.textContent = `saved → ${dest}`;
              store.showToast(`apk saved (${humanSize(it.size)})`);
            } catch (e) {
              apkErr.textContent = String(e);
              apkProgress.textContent = '';
            }
          }),
        }, `[ DOWNLOAD ${it.arch.toUpperCase()} · ${humanSize(it.size)} ]`) as HTMLButtonElement;
        apkButtons.appendChild(btn);
      }
    };

    Api.listApkArtifacts()
      .then((info) => renderApk(info.version, info.artifacts))
      .catch((e) => { apkInfo.textContent = ''; apkErr.textContent = `apk info failed: ${e}`; });

    const unsubProg = store.updateProgress.subscribe((p) => {
      if (!p) return;
      apkProgress.textContent = `downloading ${p.pct}% · ${humanSize(p.downloaded)} / ${humanSize(p.total)}`;
    }, false);
    const closeWrapped = (): void => { unsubProg(); close(); };

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, `── settings :: ${store.currentProfile.get() ?? ''} ──`),
        h('button', { class: 'icon-btn', onClick: closeWrapped }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'card-label' }, 'version'),
        verSlot,
        h('div', { class: 'row', style: { marginTop: '8px' } },
          (() => {
            const b = h('button', {
              class: 'btn btn-ghost',
              onClick: () => busy(b, async () => {
                updErr.textContent = '';
                try {
                  const info = await Api.checkUpdate();
                  if (!info) store.showToast('you are on the latest version');
                } catch (e) { updErr.textContent = String(e); }
              }),
            }, '[ CHECK FOR UPDATES ]') as HTMLButtonElement;
            return b;
          })(),
        ),
        updErr,

        h('div', { class: 'divider-text' }, 'mobile apk'),
        apkInfo,
        apkButtons,
        apkProgress,
        apkErr,

        h('div', { class: 'divider-text' }, 'change passphrase'),
        h('div', { class: 'field' }, oldP),
        h('div', { class: 'field' }, newP),
        h('div', { class: 'field' }, newP2),
        passErr,
        h('button', {
          class: 'btn btn-block',
          onClick: async () => {
            passErr.textContent = '';
            if (newP.value.length < 8) { passErr.textContent = 'too short'; return; }
            if (newP.value !== newP2.value) { passErr.textContent = 'mismatch'; return; }
            try {
              await Api.changePassphrase(oldP.value, newP.value);
              store.showToast('passphrase changed');
              oldP.value = newP.value = newP2.value = '';
            } catch (e) { passErr.textContent = String(e); }
          },
        }, '[ CHANGE ]'),

        h('div', { class: 'divider-text' }, 'duress'),
        h('div', { class: 'field' }, currP),
        h('div', { class: 'field' }, duP, h('div', { class: 'hint' }, 'leave empty to remove')),
        h('label', { class: 'chk', style: { marginBottom: '10px' } },
          duWipe, h('span', { class: 'box' }), h('span', null, 'wipe on duress (unchecked = decoy)')),
        duErr,
        h('button', {
          class: 'btn btn-block btn-amber',
          onClick: async () => {
            duErr.textContent = '';
            try {
              await Api.setDuress(currP.value, duP.value.trim() || null, duWipe.checked);
              store.showToast('duress updated');
              currP.value = duP.value = '';
            } catch (e) { duErr.textContent = String(e); }
          },
        }, '[ UPDATE DURESS ]'),

        h('div', { class: 'divider-text' }, 'max attempts'),
        h('div', { class: 'field' }, attP),
        h('div', { class: 'field' }, attN, h('div', { class: 'hint' }, '0 = unlimited')),
        attErr,
        h('button', {
          class: 'btn btn-block btn-amber',
          onClick: async () => {
            attErr.textContent = '';
            try {
              await Api.setMaxAttempts(attP.value, parseInt(attN.value) || 0);
              store.showToast('updated');
              attP.value = '';
            } catch (e) { attErr.textContent = String(e); }
          },
        }, '[ UPDATE ]'),

        h('div', { class: 'divider-text' }, 'outer proxy'),
        h('div', { class: 'hint', style: { marginBottom: '8px' } },
          'route tor entry through a SOCKS5 proxy. ISP no longer sees that you use tor. requires app restart to apply.'),
        h('div', { class: 'field' }, h('label', null, 'type'), proxyKindSel),
        h('div', { class: 'field' }, h('label', null, 'host'), proxyHost),
        h('div', { class: 'field' }, h('label', null, 'port'), proxyPort),
        h('div', { class: 'field' }, h('label', null, 'user'), proxyUser),
        h('div', { class: 'field' }, h('label', null, 'pass'), proxyPass),
        proxyErr,
        h('button', {
          class: 'btn btn-block',
          onClick: async () => {
            proxyErr.textContent = '';
            const kind = proxyKindSel.value === 'socks5' ? 'socks5' : 'none';
            const host = proxyHost.value.trim();
            const port = parseInt(proxyPort.value || '0', 10) || 0;
            if (kind !== 'none') {
              if (!host) { proxyErr.textContent = 'host required'; return; }
              if (port < 1 || port > 65535) { proxyErr.textContent = 'invalid port'; return; }
            }
            try {
              await Api.setProxyConfig({
                kind, host, port,
                user: proxyUser.value.trim() || null,
                pass: proxyPass.value || null,
              });
              store.showToast('proxy saved · restart app to apply');
            } catch (e) {
              proxyErr.textContent = String(e);
            }
          },
        }, '[ SAVE PROXY ]'),

        h('div', { class: 'divider-text' }, 'backup'),
        h('div', { class: 'hint', style: { marginBottom: '8px' } },
          'полный экспорт профиля: identity, контакты, группы, ВСЯ переписка с вложениями, прекеи, pinned, settings — всё в один зашифрованный файл. ',
          'импорт на другом устройстве: profile-select → [ IMPORT BACKUP ]. ',
          'ВАЖНО: одна identity = одно активное устройство. После импорта закрой gipny на старом — иначе session ratchet поплывёт и сообщения начнут падать в resync.'),
        (() => {
          const passI = h('input', { class: 'input', type: 'password', placeholder: 'backup passphrase (min 8)' }) as HTMLInputElement;
          const errEl = h('div', { class: 'err' });
          const exportBtn = h('button', {
            class: 'btn btn-block',
            onClick: async () => {
              errEl.textContent = '';
              if (passI.value.length < 8) { errEl.textContent = 'passphrase too short'; return; }
              try {
                const stamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
                const path = await save({ defaultPath: `gipny-backup-${stamp}.bin` });
                if (!path) return;
                await Api.exportIdentity(passI.value, path as string);
                store.showToast('backup exported');
                passI.value = '';
              } catch (e) {
                errEl.textContent = String(e);
              }
            },
          }, '[ EXPORT BACKUP ]');
          return h('div', null,
            h('div', { class: 'field' }, passI),
            errEl,
            exportBtn,
          );
        })(),

        h('div', { class: 'divider-text' }, 'debug'),
        (() => {
          const out = h('pre', {
            class: 'card-block',
            style: { maxHeight: '300px', overflow: 'auto', whiteSpace: 'pre-wrap', wordBreak: 'break-all', fontSize: '10px', display: 'none' },
          });
          const refreshBtn = h('button', {
            class: 'btn btn-ghost',
            onClick: () => busy(refreshBtn as HTMLButtonElement, async () => {
              try {
                const txt = await Api.readDebugLog();
                out.textContent = txt || '(empty)';
                (out as HTMLElement).style.display = '';
              } catch (e) { out.textContent = String(e); (out as HTMLElement).style.display = ''; }
            }),
          }, '[ SHOW DEBUG LOG ]') as HTMLButtonElement;
          const copyBtn = h('button', {
            class: 'btn btn-ghost',
            onClick: () => {
              navigator.clipboard.writeText(out.textContent ?? '').catch(() => store.showToast('copy failed', true));
              store.showToast('copied');
            },
          }, '[ COPY ]');
          return h('div', null,
            h('div', { class: 'row' }, refreshBtn, copyBtn),
            out,
          );
        })(),

        h('div', { class: 'divider-text' }, 'danger zone'),
        h('button', {
          class: 'btn btn-block btn-danger',
          onClick: async () => {
            const ok = await app.confirm('lock vault', 'drop keys from memory and return to profile selection?');
            if (!ok) return;
            closeWrapped();
            await store.lock();
          },
        }, '[ LOCK NOW ]'),
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: closeWrapped }, '[ close ]'),
      ),
    );
  }
}
