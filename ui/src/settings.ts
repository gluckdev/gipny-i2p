import { errorText, logText } from './error-text';
import { t as tr } from './i18n';
import { save } from '@tauri-apps/plugin-dialog';
import { Api, encodeCard } from './api';
import type { RelayInfo, RelayMode, RouterSettings, TransitProfile, YggdrasilMode } from './api';
import { getTheme, setTheme } from './theme';
import type { Theme } from './theme';
import { getLang, setLang, type Lang } from './i18n';
import type { Store } from './state';
import { h, busy, humanSize, short } from './view';
import type { App } from './app';

import { icon } from './icons';
export class SettingsModal {
  el: HTMLElement;
  constructor(store: Store, app: App, close: () => void) {
    const oldP = h('input', { class: 'input', type: 'password', placeholder: tr('settings.current_passphrase') });
    const newP = h('input', { class: 'input', type: 'password', placeholder: tr('settings.new_passphrase') });
    const newP2 = h('input', { class: 'input', type: 'password', placeholder: tr('settings.repeat_new_passphrase') });
    const passErr = h('div', { class: 'err' });

    const currP = h('input', { class: 'input', type: 'password', placeholder: tr('settings.current_passphrase') });
    const duP = h('input', { class: 'input', type: 'password', placeholder: tr('settings.duress_passphrase_leave_empty_to_remove') });
    const duWipe = h('input', { type: 'checkbox', checked: true });
    const duErr = h('div', { class: 'err' });

    const attP = h('input', { class: 'input', type: 'password', placeholder: tr('settings.current_passphrase') });
    const attN = h('input', { class: 'input', type: 'number', value: '10', min: '0' });
    const attErr = h('div', { class: 'err' });

    const verSlot = h('div', { class: 'card-block' }, tr('media.loading'));
    Api.currentVersion().then((v) => { verSlot.textContent = `gipny v${v}`; }).catch(() => { verSlot.textContent = '?'; });
    const updErr = h('div', { class: 'err' });

    const apkInfo = h('div', { class: 'hint', style: { marginBottom: '6px' } }, tr('settings.fetching_apk_info'));
    const apkButtons = h('div', { class: 'row' });
    const apkErr = h('div', { class: 'err' });
    const apkProgress = h('div', { class: 'hint', style: { marginTop: '4px' } });

    const renderApk = (version: string, items: { arch: string; size: number }[]): void => {
      apkInfo.textContent = items.length === 0
        ? tr('settings.no_apk_in_current_release')
        : tr('settings.gipny_v_for_android_sideload_apk_for_your', { p0: (version) });
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
              apkProgress.textContent = tr('settings.downloading', { p0: (humanSize(it.size)) });
              await Api.downloadApk(it.arch, dest);
              apkProgress.textContent = tr('settings.saved', { p0: (dest) });
              store.showToast(tr('settings.apk_saved', { p0: (humanSize(it.size)) }));
            } catch (e) {
              apkErr.textContent = errorText(e);
              apkProgress.textContent = '';
            }
          }),
        }, tr('settings.download', { p0: (it.arch.toUpperCase()), p1: (humanSize(it.size)) })) as HTMLButtonElement;
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
            tr('settings.automatic_updates_are_unavailable_in_this_session_download')));
          apkSection.classList.remove('hidden');
          return Api.listApkArtifacts()
            .then((info) => renderApk(info.version, info.artifacts))
            .catch(() => { apkInfo.textContent = ''; });
        }
        return Api.listApkArtifacts()
          .then((info) => renderApk(info.version, info.artifacts))
          .catch((e) => { apkInfo.textContent = ''; apkErr.textContent = tr('settings.apk_info_failed', { p0: (e) }); });
      })
      .catch(() => { updateSection.classList.add('hidden'); apkSection.classList.add('hidden'); });

    const unsubProg = store.updateProgress.subscribe((p) => {
      if (!p) return;
      apkProgress.textContent = tr('settings.downloading_2', { p0: (p.pct), p1: (humanSize(p.downloaded)), p2: (humanSize(p.total)) });
    }, false);
    const closeWrapped = (): void => { unsubProg(); close(); };

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, `${tr('settings.title')} · ${store.currentProfile.get() ?? ''}`),
        h('button', { class: 'icon-btn', title: tr('common.close'), onClick: closeWrapped }, icon('close')),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'card-label' }, tr('settings.version')),
        verSlot,
        (() => {
          // A found-but-not-installed version: either auto-update is off, or
          // its install failed and the log was the only place that said so.
          const pending = h('div', { class: 'hint' });
          const installBtn = h('button', { class: 'btn' }, tr('settings.install_now')) as HTMLButtonElement;
          const installRow = h('div', { class: 'row hidden', style: { marginTop: '8px' } }, installBtn);
          const showPending = (v: { version: string; size: number } | null): void => {
            installRow.classList.toggle('hidden', !v);
            pending.textContent = v ? tr('settings.ready_to_install', { p0: (v.version), p1: (humanSize(v.size)) }) : '';
          };
          // A version this session already found (auto-update off, or its
          // install failed) — the button must be there without checking again.
          const known = store.updateAvailable.get();
          if (known) showPending({ version: known.version, size: known.size });
          const lastErr = store.updateError.get();
          if (lastErr) {
            updErr.textContent = tr('settings.the_last_installation_attempt_failed', { p0: errorText(lastErr) });
            installRow.classList.remove('hidden');
          }
          installBtn.addEventListener('click', () => void busy(installBtn, async () => {
            updErr.textContent = '';
            try {
              await Api.installUpdate();
              showPending(null);
            } catch (e) { updErr.textContent = errorText(e); }
          }));
          updateSection.append(
            pending,
            installRow,
            updErr,
            (() => {
              const line = h('div', { class: 'hint', style: { marginTop: '8px' } },
                tr('settings.updates_are_checked_at_startup_and_every_few'));
              Api.updateAsksForRoot().then((asks) => {
                if (!asks) return;
                line.textContent = tr('settings.this_app_was_installed_as_a_deb_package');
              }).catch(() => {});
              return line;
            })(),
          );
          return updateSection;
        })(),

        h('div', { class: 'divider-text' }, tr('settings.relay')),
        (() => {
          const choices: { id: RelayMode; title: string; blurb: string }[] = [
            {
              id: 'builtin', title: tr('settings.relay_builtin'),
              blurb: tr('settings.a_relay_inside_the_app_with_no_setup'),
            },
            {
              id: 'external', title: tr('settings.relay_external'),
              blurb: tr('settings.a_relay_on_your_own_or_someone_else'),
            },
          ];
          const relayI = h('input', { class: 'input', placeholder: tr('settings.relay_destination_in_base64'), 'aria-label': tr('settings.relay_i2p_destination') }) as HTMLInputElement;
          const status = h('div', { class: 'hint', style: { margin: '8px 0 6px' } });
          const relayErr = h('div', { class: 'err' });
          const saveBtn = h('button', {
            class: 'btn btn-block',
            onClick: () => busy(saveBtn, async () => {
              relayErr.textContent = '';
              try {
                await Api.setRelayAddress(relayI.value.trim());
                store.relayInfo.set(await Api.getRelayInfo());
                store.showToast(tr('settings.relay_address_saved_reconnecting_within_20_seconds'));
              } catch (e) { relayErr.textContent = errorText(e); }
            }),
          }, tr('settings.save_relay')) as HTMLButtonElement;
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
                .catch((e) => { relayErr.textContent = errorText(e); paint(store.relayInfo.get()); });
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
              ? (info.external.trim() ? tr('settings.collecting_mail_from_the_external_relay') : tr('settings.no_external_relay_address_sending_is_unavailable'))
              : info.hosted.state === 'ready' ? tr('settings.built_in_relay_is_running', { p0: (short(info.hosted.address)) })
              : info.hosted.state === 'failed' ? tr('settings.failed_to_start_retrying', { p0: (info.hosted.reason) })
              : tr('settings.built_in_relay_is_starting_usually_1_2');
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

        h('div', { class: 'divider-text' }, tr('about.relay_network')),
        (() => {
          const stat = (label: string) => {
            const v = h('div', { class: 'stat-value' }, '…');
            return { v, el: h('div', { class: 'stat' }, v, h('div', { class: 'stat-label' }, label)) };
          };
          const peers = stat(tr('settings.known_nodes'));
          const items = stat(tr('settings.records_stored_for_others'));
          const role = stat(tr('settings.device_role'));
          const note = h('div', { class: 'hint' });
          Api.getDhtStatus().then((d) => {
            peers.v.textContent = String(d.peers);
            items.v.textContent = d.stores ? `${d.items} · ${humanSize(d.bytes)}` : '—';
            role.v.textContent = d.stores ? tr('settings.storage_node') : tr('settings.client_only');
            note.textContent = !d.joined
              ? tr('settings.this_node_will_join_the_network_when_the')
              : d.peers === 0 && d.seeds === 0
                ? tr('settings.looking_for_the_network_through_your_contacts_relays')
                : d.peers === 0
                  ? tr('settings.no_nodes_found_gathering_candidates_from_saved_nodes')
                  : tr('settings.this_device_participates_in_the_network_it_stores');
          }).catch(() => { note.textContent = tr('settings.network_status_unavailable'); });
          return h('div', null, h('div', { class: 'stats' }, peers.el, items.el, role.el), note);
        })(),

        h('div', { class: 'divider-text' }, tr('settings.privacy')),
        (() => {
          const cb = h('input', { type: 'checkbox' }) as HTMLInputElement;
          const err = h('div', { class: 'err' });
          Api.getAttachmentPrivacy().then((v) => { cb.checked = v; }).catch(() => { cb.checked = true; });
          cb.addEventListener('change', () => {
            err.textContent = '';
            Api.setAttachmentPrivacy(cb.checked).catch((e) => {
              err.textContent = errorText(e);
              cb.checked = !cb.checked;
            });
          });
          return h('div', null,
            h('label', { class: 'opt', style: { alignItems: 'center' } },
              cb,
              h('span', { class: 'opt-text' },
                h('span', { class: 'opt-title' }, tr('settings.remove_attachment_metadata')),
                h('span', { class: 'opt-blurb' },
                  tr('settings.before_sending_exif_xmp_iptc_and_author_metadata')))),
            h('div', { class: 'divider', style: { margin: '14px 0' } }),
            (() => {
              const wipe = h('input', { type: 'checkbox' }) as HTMLInputElement;
              const wipeErr = h('div', { class: 'err' });
              Api.getChatPrefs().then((p) => { wipe.checked = p.clearOnLock; }).catch(() => { wipe.checked = false; });
              wipe.addEventListener('change', () => {
                wipeErr.textContent = '';
                Api.updateChatPrefs({ clearOnLock: wipe.checked }).catch((e) => {
                  wipeErr.textContent = errorText(e);
                  wipe.checked = !wipe.checked;
                });
              });
              return h('div', null,
                h('label', { class: 'opt', style: { alignItems: 'center' } },
                  wipe,
                  h('span', { class: 'opt-text' },
                    h('span', { class: 'opt-title' }, tr('settings.clear_history_when_locking')),
                    h('span', { class: 'opt-blurb' },
                      tr('settings.messages_are_deleted_on_lock_f5_and_profile')))),
                wipeErr,
              );
            })(),
            err,
          );
        })(),

        h('div', { class: 'divider-text' }, tr('settings.agent_mode')),
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
              status.textContent = tr('settings.enabled_master', { p0: (master.name || 'unnamed'), p1: (master.sign_pk.slice(0, 16)) });
              toggle.textContent = tr('settings.disable_agent_mode');
              toggle.className = 'btn btn-block btn-danger';
              picker.disabled = true;
            } else {
              status.textContent = candidates().length === 0
                ? tr('settings.no_contacts_available_to_trust_with_the_console')
                : tr('chat.off');
              toggle.textContent = tr('settings.enable_agent_mode');
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
                store.showToast(tr('settings.agent_mode_off'));
              } else {
                const id = parseInt(picker.value, 10);
                if (!Number.isFinite(id)) { err.textContent = tr('settings.choose_a_master'); return; }
                await store.setAgentMode(id);
                store.showToast(tr('settings.agent_mode_on'));
              }
              render();
            } catch (e) { err.textContent = errorText(e); }
          }));

          return h('div', null,
            h('div', { class: 'hint', style: { marginBottom: '6px' } },
              tr('settings.the_master_can_run_commands_on_this_device')),
            h('div', { class: 'field' }, picker),
            status,
            err,
            toggle,
          );
        })(),

        h('div', { class: 'card-block', style: { marginTop: '14px' } },
          h('div', { class: 'card-label' }, tr('settings.remote_server_agent_linux_macos_windows')),
          h('button', {
            class: 'btn btn-block', style: { marginTop: '10px' },
            onClick: () => app.openModal((dismiss) => remoteAgentModal(store, dismiss)),
          }, tr('settings.connect_a_remote_server_agent')),
        ),

        h('div', { class: 'divider-text' }, tr('settings.language')),
        (() => {
          const choices: { id: Lang; title: string }[] = [
            { id: 'ru', title: tr('settings.lang_ru') },
            { id: 'en', title: tr('settings.lang_en') },
          ];
          const current = getLang();
          const rows = choices.map(({ id, title }) => {
            const r = h('input', {
              type: 'radio', name: 'language', id: `opt-lang-${id}`,
            }) as HTMLInputElement;
            r.checked = id === current;
            r.addEventListener('change', () => {
              if (r.checked) {
                setLang(id);
                // Re-open settings and release its update-progress subscription.
                closeWrapped();
                app.openModal((c) => new SettingsModal(store, app, c).el);
              }
            });
            return h('label', { class: 'opt', for: `opt-lang-${id}` },
              r,
              h('span', { class: 'opt-text' },
                h('span', { class: 'opt-title' }, title)),
            );
          });
          return h('div', { class: 'opt-group' }, ...rows);
        })(),

        h('div', { class: 'divider-text' }, tr('settings.theme')),
        (() => {
          // Applied the moment it is picked — unlike the router settings there is
          // nothing to restart, so there is no "takes effect later" to explain.
          const choices: { id: Theme; title: string; blurb: string }[] = [
            { id: 'light', title: tr('settings.theme_light'), blurb: tr('settings.theme_light_blurb') },
            { id: 'dark', title: tr('settings.theme_dark'), blurb: tr('settings.theme_dark_blurb') },
            { id: 'system', title: tr('settings.theme_system'), blurb: tr('settings.theme_system_blurb') },
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

        h('div', { class: 'divider-text' }, tr('settings.router_i2p')),
        (() => {
          const err = h('div', { class: 'err' });
          const note = h('div', { class: 'hint', style: { marginTop: '6px' } });

          // Transit is cover traffic other people generate and pay for: a router
          // carrying only its own messages is far easier to single out. The
          // labels say that, because "bandwidth setting" reads like generosity
          // and this is actually about the user's own anonymity.
          const levels: { id: TransitProfile; title: string; blurb: string }[] = [
            { id: 'frugal', title: tr('settings.frugal'),
              blurb: tr('settings.minimal_transit_traffic_suitable_for_mobile_data_and') },
            { id: 'balanced', title: tr('settings.balanced'),
              blurb: tr('settings.a_reasonable_balance_for_most_devices') },
            { id: 'generous', title: tr('settings.generous'),
              blurb: tr('settings.more_transit_traffic_provides_cover_traffic_and_helps') },
          ];
          const yggLevels: { id: YggdrasilMode; title: string; blurb: string }[] = [
            { id: 'off', title: tr('chat.off'), blurb: tr('settings.regular_internet_only') },
            { id: 'auto', title: tr('settings.automatic'),
              blurb: tr('settings.try_the_regular_route_use_mesh_if_the') },
            { id: 'on', title: tr('settings.always'), blurb: tr('settings.always_announce_this_node_in_the_mesh_network') },
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
              note.textContent = tr('settings.saved_takes_effect_the_next_time_the_app');
            } catch (e) { err.textContent = errorText(e); }
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
              tr('settings.how_many_other_people_s_tunnels_your_router')),
            h('div', { class: 'opt-group' }, ...options),

            h('div', { class: 'divider-text', style: { marginTop: '14px' } }, 'yggdrasil'),
            h('div', { class: 'hint', style: { marginBottom: '8px' } },
              tr('settings.an_alternative_when_your_isp_blocks_regular_i2p')),
            h('div', { class: 'opt-group' }, ...yggOptions),
            note,
            err,
          );
        })(),

        (() => {
          apkSection.append(
            h('div', { class: 'divider-text' }, tr('settings.android_app')),
            apkInfo,
            apkButtons,
            apkProgress,
            apkErr,
          );
          return apkSection;
        })(),

        h('div', { class: 'divider-text' }, tr('settings.change_passphrase')),
        h('div', { class: 'field' }, oldP),
        h('div', { class: 'field' }, newP),
        h('div', { class: 'field' }, newP2),
        passErr,
        h('button', {
          class: 'btn btn-block',
          onClick: async () => {
            passErr.textContent = '';
            if (newP.value.length < 8) { passErr.textContent = tr('settings.too_short'); return; }
            if (newP.value !== newP2.value) { passErr.textContent = tr('auth.passphrases_do_not_match'); return; }
            try {
              await Api.changePassphrase(oldP.value, newP.value);
              store.showToast(tr('settings.passphrase_changed'));
              oldP.value = newP.value = newP2.value = '';
            } catch (e) { passErr.textContent = errorText(e); }
          },
        }, tr('actions.change')),

        h('div', { class: 'divider-text' }, tr('settings.duress_passphrase')),
        h('div', { class: 'field' }, currP),
        h('div', { class: 'field' }, duP, h('div', { class: 'hint' }, tr('settings.leave_empty_to_remove'))),
        h('label', { class: 'chk', style: { marginBottom: '10px' } },
          duWipe, h('span', { class: 'box' }), h('span', null, tr('settings.wipe_on_duress_unchecked_decoy'))),
        duErr,
        h('button', {
          class: 'btn btn-block btn-amber',
          onClick: async () => {
            duErr.textContent = '';
            try {
              await Api.setDuress(currP.value, duP.value.trim() || null, duWipe.checked);
              store.showToast(tr('settings.duress_updated'));
              currP.value = duP.value = '';
            } catch (e) { duErr.textContent = errorText(e); }
          },
        }, tr('common.save')),

        h('div', { class: 'divider-text' }, tr('settings.attempt_limit')),
        h('div', { class: 'field' }, attP),
        h('div', { class: 'field' }, attN, h('div', { class: 'hint' }, tr('settings.0_unlimited'))),
        attErr,
        h('button', {
          class: 'btn btn-block btn-amber',
          onClick: async () => {
            attErr.textContent = '';
            try {
              await Api.setMaxAttempts(attP.value, parseInt(attN.value) || 0);
              store.showToast(tr('settings.attempt_limit_updated'));
              attP.value = '';
            } catch (e) { attErr.textContent = errorText(e); }
          },
        }, tr('common.save')),


        h('div', { class: 'divider-text' }, tr('settings.backup')),
        h('div', { class: 'hint', style: { marginBottom: '8px' } },
          tr('settings.export_your_whole_profile_identity_contacts_groups_all'),
          tr('settings.on_another_device_profile_selection_import_backup'),
          tr('settings.one_identity_means_one_active_device_after_importing')),
        (() => {
          const passI = h('input', { class: 'input', type: 'password', placeholder: tr('settings.backup_passphrase_at_least_8_characters') }) as HTMLInputElement;
          const errEl = h('div', { class: 'err' });
          const exportBtn = h('button', {
            class: 'btn btn-block',
            onClick: async () => {
              errEl.textContent = '';
              if (passI.value.length < 8) { errEl.textContent = tr('settings.too_short'); return; }
              try {
                const stamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
                const path = await save({ defaultPath: `gipny-backup-${stamp}.bin` });
                if (!path) return;
                await Api.exportIdentity(passI.value, path as string);
                store.showToast(tr('settings.backup_exported'));
                passI.value = '';
              } catch (e) {
                errEl.textContent = errorText(e);
              }
            },
          }, tr('settings.export_backup'));
          return h('div', null,
            h('div', { class: 'field' }, passI),
            errEl,
            exportBtn,
          );
        })(),

        h('div', { class: 'divider-text' }, tr('settings.application_log')),
        (() => {
          const out = h('pre', { class: 'log-view hidden' });
          const pathLine = h('div', { class: 'hint' }, '');
          const cb = h('input', { type: 'checkbox' }) as HTMLInputElement;
          let showing: 'current' | 'previous' = 'current';

          const load = (which: 'current' | 'previous') => busy(showBtn, async () => {
            showing = which;
            try {
              const txt = which === 'current' ? await Api.readDebugLog() : await Api.readPreviousLog();
              out.textContent = logText(txt);
              out.classList.remove('hidden');
              out.scrollTop = out.scrollHeight;
            } catch (e) {
              out.textContent = errorText(e);
              out.classList.remove('hidden');
            }
          });

          const showBtn = h('button', { class: 'btn btn-ghost', onClick: () => void load('current') }, tr('settings.show_log')) as HTMLButtonElement;
          const prevBtn = h('button', { class: 'btn btn-ghost', onClick: () => void load('previous') }, tr('settings.previous_session')) as HTMLButtonElement;
          const copyBtn = h('button', {
            class: 'btn btn-ghost',
            onClick: () => {
              navigator.clipboard.writeText(out.textContent ?? '')
                .then(() => store.showToast(tr('settings.log_copied')))
                .catch(() => store.showToast(tr('settings.could_not_copy'), true));
            },
          }, tr('settings.copy'));
          const clearBtn = h('button', {
            class: 'btn btn-ghost',
            onClick: () => busy(clearBtn, async () => {
              await Api.clearDebugLog().catch((e) => store.showToast(errorText(e), true));
              out.textContent = tr('settings.log_is_empty');
              store.showToast(tr('settings.log_cleared'));
            }),
          }, tr('settings.clear')) as HTMLButtonElement;

          Api.logSettings().then((s) => {
            cb.checked = s.enabled;
            pathLine.textContent = tr('settings.file', { p0: (s.path) });
          }).catch(() => { cb.checked = true; });
          cb.addEventListener('change', () => {
            Api.setLogEnabled(cb.checked)
              .then(() => store.showToast(cb.checked
                ? tr('settings.logging_enabled_starts_on_the_next_launch')
                : tr('settings.logging_disabled_and_log_erased')))
              .catch((e) => { store.showToast(errorText(e), true); cb.checked = !cb.checked; });
          });

          return h('div', null,
            h('label', { class: 'opt', style: { alignItems: 'center' } },
              cb,
              h('span', { class: 'opt-text' },
                h('span', { class: 'opt-title' }, tr('settings.keep_a_detailed_log')),
                h('span', { class: 'opt-blurb' },
                  tr('settings.enabled_by_default_to_help_diagnose_problems_the'))),
            ),
            pathLine,
            h('div', { class: 'row', style: { marginTop: '8px', flexWrap: 'wrap' } }, showBtn, prevBtn, copyBtn, clearBtn),
            out,
          );
        })(),

        h('div', { class: 'divider-text' }, tr('settings.danger_zone')),
        h('button', {
          class: 'btn btn-block btn-danger',
          onClick: async () => {
            const ok = await app.confirm(tr('sidebar.lock'), tr('settings.remove_keys_from_memory_and_return_to_profile'));
            if (!ok) return;
            closeWrapped();
            await store.lock();
          },
        }, tr('settings.lock_now')),
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: closeWrapped }, tr('common.close')),
      ),
    );
  }
}

/** Build the command from the current identity and its receiving relay. */
function remoteAgentModal(store: Store, close: () => void): HTMLElement {
  const base = 'https://raw.githubusercontent.com/gluckdev/gipny-i2p/main/scripts/';
  const platforms = [
    { title: 'Linux / macOS · root', script: 'root' },
    { title: tr('settings.linux_without_root'), script: 'user' },
    { title: 'Windows · PowerShell', script: 'windows' },
  ];
  let selected = 'root';
  let relay = '';
  let command = '';
  let relayLoaded = false;
  let disposed = false;
  const output = h('textarea', {
    class: 'input', readonly: true, rows: '7', 'aria-label': tr('settings.agent_installation_command'),
    style: { fontFamily: 'monospace', resize: 'vertical', marginTop: '12px' },
  }) as HTMLTextAreaElement;
  const note = h('div', { class: 'hint', style: { marginTop: '10px' } });
  const error = h('div', { class: 'err' });
  const copy = h('button', {
    class: 'btn btn-amber btn-block',
    onClick: async () => {
      if (!command) return;
      try {
        await navigator.clipboard.writeText(command);
        store.showToast(tr('settings.command_copied'));
      } catch (e) { store.showToast(tr('settings.could_not_copy_2', { p0: (errorText(e)) }), true); }
    },
  }, tr('settings.copy_command')) as HTMLButtonElement;
  const tabs = platforms.map((platform) => h('button', {
    class: 'btn btn-ghost', role: 'tab',
    onClick: () => { selected = platform.script; render(); },
  }, platform.title) as HTMLButtonElement);
  const render = (): void => {
    if (disposed) return;
    const id = store.identity.get();
    const info = store.relayInfo.get();
    const currentRelay = info?.mode === 'builtin'
      ? (info.hosted.state === 'ready' ? info.hosted.address.trim() : '')
      : (relayLoaded ? relay : '');
    command = '';
    if (id && currentRelay) {
      const card = encodeCard(id.onion, id.card.sign_pk, id.card.dh_pk,
        store.displayName.get() || undefined, currentRelay);
      if (selected === 'windows') {
        // Escape PowerShell interpolation in a double-quoted string.
        const quoted = card.replace(/[`$"\r\n]/g, (char) => '`' + char);
        command = `irm ${base}install-agent.ps1 | iex; Install-GipnyAgent -Master "${quoted}"`;
      } else {
        const quoted = card.replace(/[\\$`"\r\n]/g, (char) => '\\' + char);
        command = `curl -fsSL ${base}install-agent.sh | ${selected === 'root' ? 'sudo ' : ''}bash -s -- --master "${quoted}"`;
      }
    }
    output.value = command || tr('settings.waiting_for_the_master_s_card_and_relay');
    copy.disabled = !command;
    tabs.forEach((tab, index) => {
      const active = platforms[index]!.script === selected;
      tab.className = active ? 'btn btn-amber' : 'btn btn-ghost';
      tab.setAttribute('aria-selected', String(active));
    });
    note.textContent = selected === 'windows'
      ? tr('settings.open_powershell_as_administrator_the_agent_runs_through')
      : selected === 'user'
        ? tr('settings.for_systemd_user_without_an_active_login_the')
        : tr('settings.on_linux_the_agent_runs_as_a_system');
  };
  const unsub = store.relayInfo.subscribe(() => render(), false);
  const cleanup = (): void => { disposed = true; unsub(); observer.disconnect(); };
  const dismiss = (): void => { cleanup(); close(); };
  const el = h('div', { class: 'modal' },
    h('div', { class: 'modal-header' },
      h('div', { class: 'modal-title' }, tr('settings.connect_a_remote_agent')),
      h('button', { class: 'icon-btn', title: tr('common.close'), onClick: dismiss }, icon('close')),
    ),
    h('div', { class: 'modal-body' },
      h('div', { class: 'row', role: 'tablist', 'aria-label': tr('settings.operating_system'), style: { flexWrap: 'wrap' } }, ...tabs),
      output, note, error,
      h('div', { class: 'hint', style: { marginTop: '14px' } },
        tr('settings.paste_this_command_into_the_target_server_s')),
    ),
    h('div', { class: 'modal-footer' }, copy),
  );
  // Backdrop closure also needs to release the relay subscription.
  const observer = new MutationObserver(() => { if (!el.isConnected) cleanup(); });
  observer.observe(document.body, { childList: true });
  Api.getRelayAddress().then((address) => {
    relay = address.trim(); relayLoaded = true; render();
  }).catch((e) => { if (!disposed) error.textContent = tr('settings.could_not_get_relay', { p0: (errorText(e)) }); });
  Api.getRelayInfo().then((info) => { if (!disposed) store.relayInfo.set(info); }).catch(() => {});
  render();
  return el;
}
