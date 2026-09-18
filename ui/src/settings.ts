import { save } from '@tauri-apps/plugin-dialog';
import { Api } from './api';
import type { RelayInfo, RelayMode, RouterSettings, TransitProfile, YggdrasilMode } from './api';
import { getTheme, setTheme } from './theme';
import type { Theme } from './theme';
import type { Store } from './state';
import { h, busy, humanSize, short } from './view';
import type { App } from './app';

import { icon } from './icons';
export class SettingsModal {
  el: HTMLElement;
  constructor(store: Store, app: App, close: () => void) {
    const oldP = h('input', { class: 'input', type: 'password', placeholder: 'текущий пароль' });
    const newP = h('input', { class: 'input', type: 'password', placeholder: 'новый пароль' });
    const newP2 = h('input', { class: 'input', type: 'password', placeholder: 'ещё раз новый' });
    const passErr = h('div', { class: 'err' });

    const currP = h('input', { class: 'input', type: 'password', placeholder: 'текущий пароль' });
    const duP = h('input', { class: 'input', type: 'password', placeholder: 'пароль под принуждением (пусто — убрать)' });
    const duWipe = h('input', { type: 'checkbox', checked: true });
    const duErr = h('div', { class: 'err' });

    const attP = h('input', { class: 'input', type: 'password', placeholder: 'текущий пароль' });
    const attN = h('input', { class: 'input', type: 'number', value: '10', min: '0' });
    const attErr = h('div', { class: 'err' });

    const verSlot = h('div', { class: 'card-block' }, 'loading...');
    Api.currentVersion().then((v) => { verSlot.textContent = `gipny v${v}`; }).catch(() => { verSlot.textContent = '?'; });
    const updErr = h('div', { class: 'err' });

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
              apkProgress.textContent = `Скачиваю ${humanSize(it.size)}…`;
              await Api.downloadApk(it.arch, dest);
              apkProgress.textContent = `Сохранено → ${dest}`;
              store.showToast(`APK сохранён (${humanSize(it.size)})`);
            } catch (e) {
              apkErr.textContent = String(e);
              apkProgress.textContent = '';
            }
          }),
        }, `Скачать ${it.arch.toUpperCase()} · ${humanSize(it.size)}`) as HTMLButtonElement;
        apkButtons.appendChild(btn);
      }
    };

    // Only ask when this session has a local HTTP proxy to check GitHub
    // through (Android, or attached to a router we don't own, has none).
    const updateSection = h('div', { class: 'stack' });
    const apkSection = h('div', { class: 'stack' });
    Api.updateConfigured()
      .then((configured) => {
        if (!configured) {
          // No local HTTP proxy this run (Android, or a router we don't own):
          // say so instead of quietly hiding the whole block.
          updateSection.replaceChildren(h('div', { class: 'hint' },
            'Автообновление в этом запуске недоступно: проверка идёт через выходной узел i2p, '
            + 'а его нет на Android и при подключении к внешнему роутеру. Свежую версию берите '
            + 'со страницы релизов (на Android — APK ниже).'));
          apkSection.classList.remove('hidden');
          return Api.listApkArtifacts()
            .then((info) => renderApk(info.version, info.artifacts))
            .catch(() => { apkInfo.textContent = ''; });
        }
        return Api.listApkArtifacts()
          .then((info) => renderApk(info.version, info.artifacts))
          .catch((e) => { apkInfo.textContent = ''; apkErr.textContent = `apk info failed: ${e}`; });
      })
      .catch(() => { updateSection.classList.add('hidden'); apkSection.classList.add('hidden'); });

    const unsubProg = store.updateProgress.subscribe((p) => {
      if (!p) return;
      apkProgress.textContent = `Скачиваю ${p.pct}% · ${humanSize(p.downloaded)} / ${humanSize(p.total)}`;
    }, false);
    const closeWrapped = (): void => { unsubProg(); close(); };

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, `Настройки · ${store.currentProfile.get() ?? ''}`),
        h('button', { class: 'icon-btn', title: 'Закрыть', onClick: closeWrapped }, icon('close')),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'card-label' }, 'Версия'),
        verSlot,
        (() => {
          // A found-but-not-installed version: either auto-update is off, or
          // its install failed and the log was the only place that said so.
          const pending = h('div', { class: 'hint' });
          const installBtn = h('button', { class: 'btn' }, 'Установить сейчас') as HTMLButtonElement;
          const installRow = h('div', { class: 'row hidden', style: { marginTop: '8px' } }, installBtn);
          const showPending = (v: { version: string; size: number } | null): void => {
            installRow.classList.toggle('hidden', !v);
            pending.textContent = v ? `Готово к установке: ${v.version} · ${humanSize(v.size)}` : '';
          };
          // A version this session already found (auto-update off, or its
          // install failed) — the button must be there without checking again.
          const known = store.updateAvailable.get();
          if (known) showPending({ version: known.version, size: known.size });
          const lastErr = store.updateError.get();
          if (lastErr) {
            updErr.textContent = `Последняя попытка установки не удалась: ${lastErr}`;
            installRow.classList.remove('hidden');
          }
          installBtn.addEventListener('click', () => void busy(installBtn, async () => {
            updErr.textContent = '';
            try {
              await Api.installUpdate();
              showPending(null);
            } catch (e) { updErr.textContent = String(e); }
          }));
          updateSection.append(
            h('div', { class: 'row', style: { marginTop: '8px' } },
              (() => {
                const b = h('button', {
                  class: 'btn btn-ghost',
                  onClick: () => busy(b, async () => {
                    updErr.textContent = '';
                    try {
                      const info = await Api.checkUpdate();
                      if (!info) {
                        store.showToast('Установлена последняя версия');
                        showPending(null);
                      } else {
                        store.showToast(`Найдена версия ${info.version}`);
                        showPending({ version: info.version, size: info.size });
                      }
                    } catch (e) { updErr.textContent = String(e); }
                  }),
                }, 'Проверить обновления') as HTMLButtonElement;
                return b;
              })(),
            ),
            pending,
            installRow,
            updErr,
            h('div', { class: 'hint', style: { marginTop: '8px' } },
              'Обновления ставятся сами: проверка при запуске и раз в несколько часов, '
              + 'загрузка через сеть i2p, установка в фоне. Новая версия начинает работать '
              + 'после перезапуска — приложение предложит его сразу, как только обновление готово.'),
          );
          return updateSection;
        })(),

        h('div', { class: 'divider-text' }, 'Релей'),
        (() => {
          const choices: { id: RelayMode; title: string; blurb: string }[] = [
            {
              id: 'builtin', title: 'встроенный',
              blurb: 'релей внутри приложения, настраивать нечего. Адрес новый при каждом запуске и нигде не '
                + 'хранится — по нему не отследить, когда вы в сети; контакты узнают его из ваших сообщений. '
                + 'Почту принимает, пока приложение запущено: иначе она ждёт у отправителя.',
            },
            {
              id: 'external', title: 'внешний',
              blurb: 'релей на сервере, своём или чужом: почтовый ящик, который работает и когда приложение '
                + 'закрыто. Адрес постоянный — так надёжнее для агентов и для тех, кто редко в сети.',
            },
          ];
          const relayI = h('input', { class: 'input', placeholder: 'destination релея в base64', 'aria-label': 'relay i2p destination' }) as HTMLInputElement;
          const status = h('div', { class: 'hint', style: { margin: '8px 0 6px' } });
          const relayErr = h('div', { class: 'err' });
          const saveBtn = h('button', {
            class: 'btn btn-block',
            onClick: () => busy(saveBtn, async () => {
              relayErr.textContent = '';
              try {
                await Api.setRelayAddress(relayI.value.trim());
                store.relayInfo.set(await Api.getRelayInfo());
                store.showToast('адрес релея сохранён — переподключусь в течение 20 секунд');
              } catch (e) { relayErr.textContent = String(e); }
            }),
          }, 'Сохранить адрес релея') as HTMLButtonElement;
          const externalBox = h('div', null, h('div', { class: 'field' }, relayI), saveBtn);

          const radios = new Map<RelayMode, HTMLInputElement>();
          const rows = choices.map(({ id, title, blurb }) => {
            const r = h('input', { type: 'radio', name: 'relay-mode', id: `opt-relay-${id}` }) as HTMLInputElement;
            radios.set(id, r);
            r.addEventListener('change', () => {
              if (!r.checked) return;
              relayErr.textContent = '';
              Api.setRelayMode(id)
                .then(() => Api.getRelayInfo())
                .then((info) => store.relayInfo.set(info))
                .catch((e) => { relayErr.textContent = String(e); paint(store.relayInfo.get()); });
            });
            return h('label', { class: 'opt', for: `opt-relay-${id}` },
              r,
              h('span', { class: 'opt-text' },
                h('span', { class: 'opt-title' }, title),
                h('span', { class: 'opt-blurb' }, blurb)),
            );
          });

          const paint = (info: RelayInfo | null): void => {
            if (!info) { status.textContent = '…'; return; }
            for (const [id, r] of radios) r.checked = id === info.mode;
            externalBox.classList.toggle('hidden', info.mode !== 'external');
            if (document.activeElement !== relayI) relayI.value = info.external;
            status.textContent = info.mode === 'external'
              ? (info.external.trim() ? 'почта собирается с внешнего релея' : 'адрес внешнего релея не задан — отправка недоступна')
              : info.hosted.state === 'ready' ? `встроенный релей работает · ${short(info.hosted.address)}`
              : info.hosted.state === 'failed' ? `не поднялся, пробую снова: ${info.hosted.reason}`
              : 'встроенный релей запускается — обычно 1–2 минуты';
          };

          const box = h('div', null, h('div', { class: 'opts' }, ...rows), status, externalBox, relayErr);
          // The modal has no teardown hook; the subscription lets go of itself
          // once the modal has been on screen and is gone again.
          let shown = false;
          const unsub = store.relayInfo.subscribe((info) => {
            if (box.isConnected) shown = true;
            else if (shown) { unsub(); return; }
            paint(info);
          }, true);
          Api.getRelayInfo().then((info) => store.relayInfo.set(info)).catch(() => {});
          return box;
        })(),

        h('div', { class: 'divider-text' }, 'Сеть релеев'),
        (() => {
          const stat = (label: string) => {
            const v = h('div', { class: 'stat-value' }, '…');
            return { v, el: h('div', { class: 'stat' }, v, h('div', { class: 'stat-label' }, label)) };
          };
          const peers = stat('узлов знаем');
          const items = stat('хранится для других');
          const role = stat('роль устройства');
          const note = h('div', { class: 'hint' });
          Api.getDhtStatus().then((d) => {
            peers.v.textContent = String(d.peers);
            items.v.textContent = d.stores ? `${d.items} · ${humanSize(d.bytes)}` : '—';
            role.v.textContent = d.stores ? 'хранит' : 'только клиент';
            note.textContent = !d.joined
              ? 'Узел подключится к сети, когда поднимется встроенный релей.'
              : d.peers === 0 && d.seeds === 0
                ? 'Пока ищем сеть через релеи ваших контактов.'
                : 'Устройство участвует в сети: хранит зашифрованные записи и отвечает другим узлам.';
          }).catch(() => { note.textContent = 'Состояние сети недоступно.'; });
          return h('div', null, h('div', { class: 'stats' }, peers.el, items.el, role.el), note);
        })(),

        h('div', { class: 'divider-text' }, 'Приватность'),
        (() => {
          const cb = h('input', { type: 'checkbox' }) as HTMLInputElement;
          const err = h('div', { class: 'err' });
          Api.getAttachmentPrivacy().then((v) => { cb.checked = v; }).catch(() => { cb.checked = true; });
          cb.addEventListener('change', () => {
            err.textContent = '';
            Api.setAttachmentPrivacy(cb.checked).catch((e) => {
              err.textContent = String(e);
              cb.checked = !cb.checked;
            });
          });
          return h('div', null,
            h('label', { class: 'opt', style: { alignItems: 'center' } },
              cb,
              h('span', { class: 'opt-text' },
                h('span', { class: 'opt-title' }, 'очищать метаданные вложений'),
                h('span', { class: 'opt-blurb' },
                  'перед отправкой из фото и PDF удаляются EXIF/XMP/IPTC и авторские '
                  + 'поля, имена фото с камеры заменяются на нейтральные. Формат, который '
                  + 'нельзя очистить, не отправится — выключите приватность или пришлите '
                  + 'другой файл. Команды агента отправляются как есть.'))),
            err,
          );
        })(),

        h('div', { class: 'divider-text' }, 'Режим агента'),
        (() => {
          const picker = h('select', { class: 'input' }) as HTMLSelectElement;
          const status = h('div', { class: 'hint', style: { margin: '8px 0 6px' } });
          const err = h('div', { class: 'err' });
          const toggle = h('button', { class: 'btn btn-block' }) as HTMLButtonElement;

          const candidates = () => store.contacts.get().filter((c) => c.trust !== 2 && c.request !== 'incoming');

          const render = (): void => {
            const master = store.agentMode.get();
            picker.replaceChildren();
            for (const c of candidates()) picker.appendChild(h('option', { value: String(c.id) }, c.name));
            if (master) {
              status.textContent = `включён · мастер: ${master.name || 'unnamed'} · ${master.sign_pk.slice(0, 16)}…`;
              toggle.textContent = 'Выключить режим агента';
              toggle.className = 'btn btn-block btn-danger';
              picker.disabled = true;
            } else {
              status.textContent = candidates().length === 0
                ? 'нет контактов, которым можно доверить консоль'
                : 'выключен';
              toggle.textContent = 'Включить режим агента';
              toggle.className = 'btn btn-block';
              picker.disabled = false;
            }
            toggle.disabled = !master && candidates().length === 0;
          };
          render();

          toggle.addEventListener('click', () => busy(toggle, async () => {
            err.textContent = '';
            try {
              const master = store.agentMode.get();
              if (master) {
                await store.setAgentMode(null);
                store.showToast('agent mode off');
              } else {
                const id = parseInt(picker.value, 10);
                if (!Number.isFinite(id)) { err.textContent = 'выберите мастера'; return; }
                await store.setAgentMode(id);
                store.showToast('agent mode on');
              }
              render();
            } catch (e) { err.textContent = String(e); }
          }));

          return h('div', null,
            h('div', { class: 'hint', style: { marginBottom: '6px' } },
              'мастер сможет выполнять на этом устройстве команды от вашего имени, а вы — '
              + 'видеть вывод. выключить может и мастер, и вы. откройте режим чата/консоли '
              + 'в переписке с мастером. на Android команды идут в песочнице приложения.'),
            h('div', { class: 'field' }, picker),
            status,
            err,
            toggle,
          );
        })(),

        h('div', { class: 'divider-text' }, 'Оформление'),
        (() => {
          // Applied the moment it is picked — unlike the router settings there is
          // nothing to restart, so there is no "takes effect later" to explain.
          const choices: { id: Theme; title: string; blurb: string }[] = [
            { id: 'light', title: 'светлая', blurb: 'светлая и воздушная' },
            { id: 'dark', title: 'тёмная', blurb: 'тёмная, для работы вечером' },
            { id: 'system', title: 'как в системе', blurb: 'следовать настройке операционной системы' },
          ];
          const current = getTheme();
          const rows = choices.map(({ id, title, blurb }) => {
            const r = h('input', {
              type: 'radio', name: 'theme', id: `opt-theme-${id}`,
            }) as HTMLInputElement;
            r.checked = id === current;
            r.addEventListener('change', () => { if (r.checked) setTheme(id); });
            return h('label', { class: 'opt', for: `opt-theme-${id}` },
              r,
              h('span', { class: 'opt-text' },
                h('span', { class: 'opt-title' }, title),
                h('span', { class: 'opt-blurb' }, blurb)),
            );
          });
          return h('div', { class: 'opt-group' }, ...rows);
        })(),

        h('div', { class: 'divider-text' }, 'Роутер i2p'),
        (() => {
          const err = h('div', { class: 'err' });
          const note = h('div', { class: 'hint', style: { marginTop: '6px' } });

          // Transit is cover traffic other people generate and pay for: a router
          // carrying only its own messages is far easier to single out. The
          // labels say that, because "bandwidth setting" reads like generosity
          // and this is actually about the user's own anonymity.
          const levels: { id: TransitProfile; title: string; blurb: string }[] = [
            { id: 'frugal', title: 'экономно',
              blurb: 'минимум чужого трафика. для мобильной сети и батареи. маскировка слабее' },
            { id: 'balanced', title: 'обычно',
              blurb: 'разумный баланс. подходит большинству' },
            { id: 'generous', title: 'щедро',
              blurb: 'много чужого трафика — лучшая маскировка для тебя и польза сети' },
          ];
          const yggLevels: { id: YggdrasilMode; title: string; blurb: string }[] = [
            { id: 'off', title: 'выключен', blurb: 'только обычный интернет' },
            { id: 'auto', title: 'автоматически',
              blurb: 'попробовать обычный путь, а если роутер вообще не поднялся — пойти через mesh' },
            { id: 'on', title: 'всегда', blurb: 'объявлять себя в mesh-сети постоянно' },
          ];
          const radios = new Map<TransitProfile, HTMLInputElement>();
          const yggRadios = new Map<YggdrasilMode, HTMLInputElement>();

          const current = (): RouterSettings => ({
            transit: [...radios].find(([, r]) => r.checked)?.[0] ?? 'balanced',
            yggdrasil: [...yggRadios].find(([, r]) => r.checked)?.[0] ?? 'auto',
          });
          const persist = async (): Promise<void> => {
            err.textContent = '';
            try {
              await Api.setRouterSettings(current());
              note.textContent = 'сохранено — применится при следующем запуске приложения';
            } catch (e) { err.textContent = String(e); }
          };

          const options = levels.map(({ id, title, blurb }) => {
            const r = h('input', { type: 'radio', name: 'transit', id: `opt-${id}` }) as HTMLInputElement;
            radios.set(id, r);
            r.addEventListener('change', () => { void persist(); });
            return h('label', { class: 'opt', for: `opt-${id}` },
              r,
              h('span', { class: 'opt-text' },
                h('span', { class: 'opt-title' }, title),
                h('span', { class: 'opt-blurb' }, blurb)),
            );
          });

          const yggOptions = yggLevels.map(({ id, title, blurb }) => {
            const r = h('input', { type: 'radio', name: 'ygg', id: `opt-ygg-${id}` }) as HTMLInputElement;
            yggRadios.set(id, r);
            r.addEventListener('change', () => { void persist(); });
            return h('label', { class: 'opt', for: `opt-ygg-${id}` },
              r,
              h('span', { class: 'opt-text' },
                h('span', { class: 'opt-title' }, title),
                h('span', { class: 'opt-blurb' }, blurb)),
            );
          });

          Api.getRouterSettings()
            .then((s2) => {
              const t = radios.get(s2.transit);
              if (t) t.checked = true;
              const y = yggRadios.get(s2.yggdrasil);
              if (y) y.checked = true;
            })
            .catch(() => {
              radios.get('balanced')!.checked = true;
              yggRadios.get('auto')!.checked = true;
            });

          return h('div', null,
            h('div', { class: 'hint', style: { marginBottom: '8px' } },
              'сколько чужих туннелей пропускает твой роутер. это настройка анонимности, '
              + 'а не щедрости: узел, через который идёт только собственный трафик, '
              + 'различить намного проще.'),
            h('div', { class: 'opt-group' }, ...options),

            h('div', { class: 'divider-text', style: { marginTop: '14px' } }, 'yggdrasil'),
            h('div', { class: 'hint', style: { marginBottom: '8px' } },
              'обходной путь, если провайдер режет обычные входы в i2p: туннели идут '
              + 'поверх mesh-сети. нужен запущенный узел yggdrasil на этой машине — '
              + 'без него ничего не изменится, включать безопасно.'),
            h('div', { class: 'opt-group' }, ...yggOptions),
            note,
            err,
          );
        })(),

        (() => {
          apkSection.append(
            h('div', { class: 'divider-text' }, 'Android-приложение'),
            apkInfo,
            apkButtons,
            apkProgress,
            apkErr,
          );
          return apkSection;
        })(),

        h('div', { class: 'divider-text' }, 'Смена пароля'),
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
        }, 'Изменить'),

        h('div', { class: 'divider-text' }, 'Пароль под принуждением'),
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
        }, 'Сохранить'),

        h('div', { class: 'divider-text' }, 'Лимит попыток'),
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
        }, 'Сохранить'),


        h('div', { class: 'divider-text' }, 'Резервная копия'),
        h('div', { class: 'hint', style: { marginBottom: '8px' } },
          'полный экспорт профиля: identity, контакты, группы, ВСЯ переписка с вложениями, прекеи, pinned, settings — всё в один зашифрованный файл. ',
          'импорт на другом устройстве: profile-select → [ IMPORT BACKUP ]. ',
          'ВАЖНО: одна identity = одно активное устройство. После импорта закрой gipny на старом — иначе session ratchet поплывёт и сообщения начнут падать в resync.'),
        (() => {
          const passI = h('input', { class: 'input', type: 'password', placeholder: 'пароль копии (от 8 символов)' }) as HTMLInputElement;
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
          }, 'Экспортировать копию');
          return h('div', null,
            h('div', { class: 'field' }, passI),
            errEl,
            exportBtn,
          );
        })(),

        h('div', { class: 'divider-text' }, 'Журнал работы'),
        (() => {
          const out = h('pre', { class: 'log-view hidden' });
          const pathLine = h('div', { class: 'hint' }, '');
          const cb = h('input', { type: 'checkbox' }) as HTMLInputElement;
          let showing: 'current' | 'previous' = 'current';

          const load = (which: 'current' | 'previous') => busy(showBtn, async () => {
            showing = which;
            try {
              const txt = which === 'current' ? await Api.readDebugLog() : await Api.readPreviousLog();
              out.textContent = txt || '(журнал пуст)';
              out.classList.remove('hidden');
              out.scrollTop = out.scrollHeight;
            } catch (e) {
              out.textContent = String(e);
              out.classList.remove('hidden');
            }
          });

          const showBtn = h('button', { class: 'btn btn-ghost', onClick: () => void load('current') }, 'Показать журнал') as HTMLButtonElement;
          const prevBtn = h('button', { class: 'btn btn-ghost', onClick: () => void load('previous') }, 'Прошлый запуск') as HTMLButtonElement;
          const copyBtn = h('button', {
            class: 'btn btn-ghost',
            onClick: () => {
              navigator.clipboard.writeText(out.textContent ?? '')
                .then(() => store.showToast('Журнал скопирован'))
                .catch(() => store.showToast('Скопировать не удалось', true));
            },
          }, 'Скопировать');
          const clearBtn = h('button', {
            class: 'btn btn-ghost',
            onClick: () => busy(clearBtn, async () => {
              await Api.clearDebugLog().catch((e) => store.showToast(String(e), true));
              out.textContent = '(журнал пуст)';
              store.showToast('Журнал очищен');
            }),
          }, 'Очистить') as HTMLButtonElement;

          Api.logSettings().then((s) => {
            cb.checked = s.enabled;
            pathLine.textContent = `Файл: ${s.path}`;
          }).catch(() => { cb.checked = true; });
          cb.addEventListener('change', () => {
            Api.setLogEnabled(cb.checked)
              .then(() => store.showToast(cb.checked
                ? 'Журнал включён — начнёт писаться со следующего запуска'
                : 'Журнал выключен и стёрт'))
              .catch((e) => { store.showToast(String(e), true); cb.checked = !cb.checked; });
          });

          return h('div', null,
            h('label', { class: 'opt', style: { alignItems: 'center' } },
              cb,
              h('span', { class: 'opt-text' },
                h('span', { class: 'opt-title' }, 'вести подробный журнал'),
                h('span', { class: 'opt-blurb' },
                  'по умолчанию включён: когда что-то ломается, должно остаться что почитать. '
                  + 'В журнал попадают адреса релеев, ошибки и тайминги — но не тексты сообщений. '
                  + 'Лежит на диске открытым; при стирании профиля под принуждением стирается тоже.')),
            ),
            pathLine,
            h('div', { class: 'row', style: { marginTop: '8px', flexWrap: 'wrap' } }, showBtn, prevBtn, copyBtn, clearBtn),
            out,
          );
        })(),

        h('div', { class: 'divider-text' }, 'Опасная зона'),
        h('button', {
          class: 'btn btn-block btn-danger',
          onClick: async () => {
            const ok = await app.confirm('Заблокировать', 'Убрать ключи из памяти и вернуться к выбору профиля?');
            if (!ok) return;
            closeWrapped();
            await store.lock();
          },
        }, 'Заблокировать сейчас'),
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: closeWrapped }, 'Закрыть'),
      ),
    );
  }
}
