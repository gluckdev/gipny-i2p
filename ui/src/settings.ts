import { errorText } from './error-text';
import { t as tr } from './i18n';
import { save } from '@tauri-apps/plugin-dialog';
import { Api, isTauri, usesBrowserCore, encodeCard } from './api';
import type { RouterSettings, TransitProfile } from './api';
import { getTheme, setTheme } from './theme';
import type { Theme } from './theme';
import { getLang, setLang, type Lang } from './i18n';
import { getOskMode, setOskMode } from './osk-mode';
import type { OskMode } from './osk-mode';
import type { Store } from './state';
import { h, busy, humanSize } from './view';
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
    const updateSection = h('div', { class: 'stack' });

    const closeWrapped = close;

    this.el = h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, `${tr('settings.title')} · ${store.currentProfile.get() ?? ''}`),
        h('button', { class: 'icon-btn', title: tr('common.close'), onClick: closeWrapped }, icon('close')),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'card-label' }, tr('settings.version')),
        verSlot,
        !usesBrowserCore() && (() => {
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
              Api.updateConfigured().then((configured) => {
                if (!configured) { line.textContent = tr('settings.automatic_updates_are_unavailable_in_this_session_download'); return; }
                return Api.updateAsksForRoot().then((asks) => {
                  if (asks) line.textContent = tr('settings.this_app_was_installed_as_a_deb_package');
                });
              }).catch(() => {});
              return line;
            })(),
          );
          return updateSection;
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

        !usesBrowserCore() && h('div', { class: 'divider-text' }, tr('settings.agent_mode')),
        !usesBrowserCore() && (() => {
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

        !usesBrowserCore() && h('div', { class: 'card-block', style: { marginTop: '14px' } },
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

        h('div', { class: 'divider-text' }, tr('settings.osk')),
        h('div', { class: 'hint', style: { marginBottom: '8px' } }, tr('settings.osk_hint')),
        (() => {
          // The choice is read once per page load (oskActive() caches it), so
          // unlike theme or language it cannot apply while the app runs.
          const choices: { id: OskMode; title: string }[] = [
            { id: 'auto', title: tr('settings.osk_auto') },
            { id: 'on', title: tr('settings.osk_on') },
            { id: 'off', title: tr('settings.osk_off') },
          ];
          const note = h('div', { class: 'hint hidden', style: { marginTop: '6px' } });
          const current = getOskMode();
          const rows = choices.map(({ id, title }) => {
            const r = h('input', {
              type: 'radio', name: 'osk', id: `opt-osk-${id}`,
            }) as HTMLInputElement;
            r.checked = id === current;
            r.addEventListener('change', () => {
              if (r.checked) {
                setOskMode(id);
                note.classList.remove('hidden');
                note.textContent = tr('settings.osk_restart');
              }
            });
            return h('label', { class: 'opt', for: `opt-osk-${id}` },
              r,
              h('span', { class: 'opt-text' },
                h('span', { class: 'opt-title' }, title)),
            );
          });
          return h('div', { class: 'opt-group' }, ...rows, note);
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

        !usesBrowserCore() && h('div', { class: 'divider-text' }, tr('settings.router_i2p')),
        !usesBrowserCore() && (() => {
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
          const radios = new Map<TransitProfile, HTMLInputElement>();
          // On by default. Off, the router never talks to the home gateway: no
          // search on the LAN and no rule that shows a P2P program runs here.
          const upnpBox = h('input', { type: 'checkbox', checked: true }) as HTMLInputElement;

          const current = (): RouterSettings => ({
            transit: [...radios].find(([, r]) => r.checked)?.[0] ?? 'balanced',
            upnp: upnpBox.checked ? 'on' : 'off',
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

          upnpBox.addEventListener('change', () => { void persist(); });

          Api.getRouterSettings()
            .then((s2) => {
              const t = radios.get(s2.transit);
              if (t) t.checked = true;
              upnpBox.checked = s2.upnp !== 'off';
            })
            .catch(() => {
              radios.get('balanced')!.checked = true;
            });

          return h('div', null,
            h('div', { class: 'hint', style: { marginBottom: '8px' } },
              tr('settings.how_many_other_people_s_tunnels_your_router')),
            h('div', { class: 'opt-group' }, ...options),

            h('div', { class: 'divider', style: { margin: '14px 0' } }),
            h('label', { class: 'opt', style: { alignItems: 'center' } },
              upnpBox,
              h('span', { class: 'opt-text' },
                h('span', { class: 'opt-title' }, tr('settings.open_the_router_port_automatically_upnp')),
                h('span', { class: 'opt-blurb' },
                  tr('settings.speeds_up_connecting_but_the_local_network_can_tell')))),
            note,
            err,
          );
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

        usesBrowserCore() && (() => {
          const error = h('div', { class: 'err' });
          const button = h('button', { class: 'btn btn-block', onClick: () => busy(button as HTMLButtonElement, async () => {
            error.textContent = '';
            try {
              const profile = store.currentProfile.get();
              if (!profile) return;
              await new Promise<void>((resolve, reject) => window.dispatchEvent(new CustomEvent('gipny-enroll-passkey', { detail: { profile, done: (err?: unknown) => err ? reject(err) : resolve() } })));
              store.showToast(tr('browser.passkey_added'));
            } catch (err) { error.textContent = errorText(err); }
          }) }, tr('browser.add_passkey'));
          return h('div', { class: 'stack' }, h('div', { class: 'divider-text' }, tr('browser.passkey')), h('div', { class: 'hint' }, tr('browser.passkey_hint')), button, error);
        })(),

        !usesBrowserCore() && h('div', { class: 'divider-text' }, tr('settings.duress_passphrase')),
        !usesBrowserCore() && h('div', { class: 'field' }, currP),
        !usesBrowserCore() && h('div', { class: 'field' }, duP, h('div', { class: 'hint' }, tr('settings.leave_empty_to_remove'))),
        !usesBrowserCore() && h('label', { class: 'chk', style: { marginBottom: '10px' } },
          duWipe, h('span', { class: 'box' }), h('span', null, tr('settings.wipe_on_duress_unchecked_decoy'))),
        !usesBrowserCore() && duErr,
        !usesBrowserCore() && h('button', {
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

        !usesBrowserCore() && h('div', { class: 'divider-text' }, tr('settings.attempt_limit')),
        !usesBrowserCore() && h('div', { class: 'field' }, attP),
        !usesBrowserCore() && h('div', { class: 'field' }, attN, h('div', { class: 'hint' }, tr('settings.0_unlimited'))),
        !usesBrowserCore() && attErr,
        !usesBrowserCore() && h('button', {
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


        !usesBrowserCore() && h('div', { class: 'divider-text' }, tr('settings.backup')),
        !usesBrowserCore() && h('div', { class: 'hint', style: { marginBottom: '8px' } },
          tr('settings.export_your_whole_profile_identity_contacts_groups_all'),
          tr('settings.on_another_device_profile_selection_import_backup'),
          tr('settings.one_identity_means_one_active_device_after_importing')),
        !usesBrowserCore() && (() => {
          const passI = h('input', { class: 'input', type: 'password', placeholder: tr('settings.backup_passphrase_at_least_8_characters') }) as HTMLInputElement;
          const errEl = h('div', { class: 'err' });
          const exportBtn = h('button', {
            class: 'btn btn-block',
            onClick: async () => {
              errEl.textContent = '';
              if (!isTauri) { errEl.textContent = tr('web.desktop_only'); return; }
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
    let cardError = false;
    if (id && currentRelay) {
      try {
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
      } catch { cardError = true; }
    }
    output.value = command || tr(cardError ? 'card.encode_error' : 'settings.waiting_for_the_master_s_card_and_relay');
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
