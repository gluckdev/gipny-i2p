import { open } from '@tauri-apps/plugin-dialog';
import { Api } from './api';
import type { Store } from './state';
import { View, h, LOGO } from './view';
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
        h('pre', { class: 'auth-logo' }, LOGO),
        h('div', { class: 'auth-title' }, ':: SELECT PROFILE ::'),
        this.subTitle,
        this.list,
        h('div', { class: 'divider-text' }, 'or'),
        h('button', {
          class: 'btn btn-block btn-amber',
          onClick: () => store.goToCreate(),
        }, '[ + NEW PROFILE ]'),
        h('button', {
          class: 'btn btn-block btn-ghost',
          style: { marginTop: '6px' },
          onClick: () => this.startImport(),
        }, '[ IMPORT BACKUP ]'),
      ),
    );
    this.sub(store.profiles, (p) => this.renderList(p));
  }

  private async startImport(): Promise<void> {
    let backupPath: string;
    try {
      const sel = await open({ multiple: false, filters: [{ name: 'gipny backup', extensions: ['bin'] }] });
      if (!sel || Array.isArray(sel)) return;
      backupPath = sel as string;
    } catch (e) {
      this.store.showToast('open failed: ' + String(e), true);
      return;
    }
    const profileI = h('input', { class: 'input', placeholder: 'new profile name' }) as HTMLInputElement;
    const vaultI = h('input', { class: 'input', type: 'password', placeholder: 'new vault passphrase (min 8)' }) as HTMLInputElement;
    const backupI = h('input', { class: 'input', type: 'password', placeholder: 'backup passphrase' }) as HTMLInputElement;
    const errEl = h('div', { class: 'err' });
    const submit = async (close: () => void): Promise<void> => {
      errEl.textContent = '';
      const profile = profileI.value.trim();
      if (!profile) { errEl.textContent = 'profile name required'; return; }
      if (vaultI.value.length < 8) { errEl.textContent = 'vault passphrase too short'; return; }
      if (!backupI.value) { errEl.textContent = 'backup passphrase required'; return; }
      try {
        await Api.importIdentityToProfile(profile, vaultI.value, backupPath, backupI.value);
        this.store.showToast(`profile "${profile}" imported`);
        await this.store.refreshProfiles();
        close();
      } catch (e) {
        errEl.textContent = String(e);
      }
    };
    this.app.openModal((close) => h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, '── import backup ──'),
        h('button', { class: 'icon-btn', onClick: close }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'hint', style: { marginBottom: '10px' } }, `from: ${backupPath}`),
        h('div', { class: 'field' }, h('label', null, 'profile name'), profileI),
        h('div', { class: 'field' }, h('label', null, 'new vault passphrase'), vaultI),
        h('div', { class: 'field' }, h('label', null, 'backup passphrase'), backupI),
        errEl,
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: close }, '[ cancel ]'),
        h('button', { class: 'btn', onClick: () => submit(close) }, '[ IMPORT ]'),
      ),
    ));
  }

  private renderList(profiles: string[]): void {
    this.subTitle.textContent = `${profiles.length} profile(s) on this machine`;
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
          title: 'delete profile',
          onClick: async () => {
            const ok = await this.app.confirm('delete profile', `wipe "${p}" and all its data?`, true);
            if (!ok) return;
            try {
              await this.store.deleteProfile(p);
              this.store.showToast(`profile "${p}" wiped`);
              this.store.view.set(this.store.profiles.get().length > 0 ? 'profile-select' : 'auth-create');
            } catch (e) { this.store.showToast(String(e), true); }
          },
        }, 'x'),
      ));
    }
  }
}
