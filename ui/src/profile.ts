import { errorText } from './error-text';
import { languagePicker } from './language-picker';
import { t as tr } from './i18n';
import { open } from '@tauri-apps/plugin-dialog';
import { Api } from './api';
import type { Store } from './state';
import { View, h, logo } from './view';
import type { App } from './app';

export class ProfileSelect extends View {
  el: HTMLElement;
  private list: HTMLElement;
  private subTitle: HTMLElement;

  constructor(private store: Store, private app: App) {
    super();
    this.list = h('div', { class: 'stack', style: { gap: '8px', marginBottom: '16px' } });
    this.subTitle = h('div', { class: 'auth-sub' }, '');
    this.el = h('div', { class: 'auth' },
      h('div', { class: 'auth-card' },
        logo(),
        languagePicker(),
        h('div', { class: 'auth-title' }, tr('profile.select_title')),
        this.subTitle,
        this.list,
        h('div', { class: 'divider-text' }, tr('common.or')),
        h('button', {
          class: 'btn btn-block btn-amber',
          onClick: () => store.goToCreate(),
        }, tr('profile.new_profile')),
        h('button', {
          class: 'btn btn-block btn-ghost',
          style: { marginTop: '6px' },
          onClick: () => this.startImport(),
        }, tr('profile.import_backup')),
      ),
    );
    this.sub(store.profiles, (p) => this.renderList(p));
  }

  private async startImport(): Promise<void> {
    let backupPath: string;
    try {
      const sel = await open({ multiple: false, filters: [{ name: tr('profile.gipny_backup'), extensions: ['bin'] }] });
      if (!sel || Array.isArray(sel)) return;
      backupPath = sel as string;
    } catch (e) {
      this.store.showToast(tr('profile.open_failed') + errorText(e), true);
      return;
    }
    const profileI = h('input', { class: 'input', placeholder: tr('profile.new_profile_name') }) as HTMLInputElement;
    const vaultI = h('input', { class: 'input', type: 'password', placeholder: tr('profile.new_vault_passphrase_min_8') }) as HTMLInputElement;
    const backupI = h('input', { class: 'input', type: 'password', placeholder: tr('profile.backup_passphrase') }) as HTMLInputElement;
    const errEl = h('div', { class: 'err' });
    const submit = async (close: () => void): Promise<void> => {
      errEl.textContent = '';
      const profile = profileI.value.trim();
      if (!profile) { errEl.textContent = tr('auth.profile_name_required'); return; }
      if (vaultI.value.length < 8) { errEl.textContent = tr('profile.vault_passphrase_too_short'); return; }
      if (!backupI.value) { errEl.textContent = tr('profile.backup_passphrase_required'); return; }
      try {
        await Api.importIdentityToProfile(profile, vaultI.value, backupPath, backupI.value);
        this.store.showToast(tr('profile.profile_imported', { p0: (profile) }));
        await this.store.refreshProfiles();
        close();
      } catch (e) {
        errEl.textContent = errorText(e);
      }
    };
    this.app.openModal((close) => h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, tr('profile.import_backup_2')),
        h('button', { class: 'icon-btn', onClick: close }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'hint', style: { marginBottom: '10px' } }, tr('profile.from', { p0: (backupPath) })),
        h('div', { class: 'field' }, h('label', null, tr('profile.profile_name')), profileI),
        h('div', { class: 'field' }, h('label', null, tr('profile.new_vault_passphrase')), vaultI),
        h('div', { class: 'field' }, h('label', null, tr('profile.backup_passphrase_2')), backupI),
        errEl,
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: close }, tr('common.cancel')),
        h('button', { class: 'btn', onClick: () => submit(close) }, tr('profile.import')),
      ),
    ));
  }

  private renderList(profiles: string[]): void {
    this.subTitle.textContent = tr('profile.profile_s_on_this_machine', { p0: (profiles.length) });
    this.list.replaceChildren();
    for (const p of profiles) {
      this.list.appendChild(h('div', { class: 'row', style: { gap: '6px' } },
        h('button', {
          class: 'btn btn-block',
          style: { justifyContent: 'flex-start', flex: '1' },
          onClick: () => this.store.selectProfileForUnlock(p),
        }, `◉  ${p}`),
        h('button', {
          class: 'btn btn-danger',
          style: { padding: '10px 12px' },
          title: tr('profile.wipe_title'),
          onClick: async () => {
            const ok = await this.app.confirm(tr('profile.wipe_title'), tr('profile.delete_confirm', { name: p }), true);
            if (!ok) return;
            try {
              await this.store.deleteProfile(p);
              this.store.showToast(tr('profile.wiped_toast', { name: p }));
              this.store.view.set(this.store.profiles.get().length > 0 ? 'profile-select' : 'auth-create');
            } catch (e) { this.store.showToast(errorText(e), true); }
          },
        }, 'x'),
      ));
    }
  }
}
