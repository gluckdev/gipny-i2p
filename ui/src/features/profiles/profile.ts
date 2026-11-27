import { errorText } from '../../app/error-text';
import { t as tr } from '../localization/i18n';
import { open } from '@tauri-apps/plugin-dialog';
import { Api, isTauri, usesBrowserCore } from '../../app/api';
import { forgetBrowserFiles, registerBrowserFile } from '../files/browser-files';
import type { Store } from '../../app/state';
import { View, h } from '../../app/view';
import { salonBackdrop, authHeader } from './profile-salon';
import type { App } from '../../app/app';

type ImportOperation = { active: boolean };

export class ProfileSelect extends View {
  el: HTMLElement;
  private list: HTMLElement;
  private subTitle: HTMLElement;
  private disposed = false;
  private importOperation: ImportOperation | null = null;
  private closeImport: (() => void) | null = null;

  constructor(private store: Store, private app: App) {
    super();
    this.list = h('div', { class: 'stack profile-list' });
    this.subTitle = h('div', { class: 'auth-sub' }, '');
    this.el = h('div', { class: 'auth profile-salon' },
      salonBackdrop(),
      h('div', { class: 'auth-card salon-panel' },
        authHeader(),
        h('div', { class: 'auth-title' }, tr('profile.select_title')),
        this.subTitle,
        this.list,
        h('div', { class: 'divider-text' }, tr('common.or')),
        h('button', {
          class: 'btn btn-block',
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
    this.sub(store.view, (view) => {
      if (view !== 'profile-select') this.invalidateImport();
    }, false);
  }

  private invalidateImport(): void {
    if (this.importOperation) this.importOperation.active = false;
    this.importOperation = null;
    const close = this.closeImport;
    this.closeImport = null;
    close?.();
  }

  private beginImport(): ImportOperation {
    this.invalidateImport();
    const operation = { active: true };
    this.importOperation = operation;
    return operation;
  }

  private isImportActive(operation: ImportOperation): boolean {
    return !this.disposed && operation.active && this.importOperation === operation
      && this.store.view.get() === 'profile-select';
  }

  private async startImport(): Promise<void> {
    if (this.disposed || this.store.view.get() !== 'profile-select') return;
    const operation = this.beginImport();
    try {
      if (!isTauri) {
        if (!usesBrowserCore()) { this.store.showToast(tr('web.desktop_only'), true); return; }
        const picker = h('input', { type: 'file', accept: '.bin' });
        picker.addEventListener('change', () => {
          const file = picker.files?.[0];
          if (file && this.isImportActive(operation)) this.showImportModal(file, operation);
        }, { once: true });
        picker.click();
        return;
      }
      const sel = await open({ multiple: false, filters: [{ name: tr('profile.gipny_backup'), extensions: ['bin'] }] });
      if (!this.isImportActive(operation) || !sel || Array.isArray(sel)) return;
      this.showImportModal(sel, operation);
    } catch (e) {
      if (this.isImportActive(operation)) this.store.showToast(tr('profile.open_failed') + errorText(e), true);
    }
  }

  private showImportModal(backup: string | File, operation = this.beginImport()): void {
    if (!this.isImportActive(operation)) return;
    const backupPath = typeof backup === 'string' ? backup : registerBrowserFile(backup);
    const filename = typeof backup === 'string' ? backup : backup.name;
    const profileI = h('input', { class: 'input', placeholder: tr('profile.new_profile_name') }) as HTMLInputElement;
    const vaultI = h('input', { class: 'input', type: 'password', placeholder: tr('profile.new_vault_passphrase_min_8') }) as HTMLInputElement;
    const backupI = h('input', { class: 'input', type: 'password', placeholder: tr('profile.backup_passphrase') }) as HTMLInputElement;
    const errEl = h('div', { class: 'err' });
    let submitting = false;
    const submit = async (close: () => void): Promise<void> => {
      if (!this.isImportActive(operation) || submitting) return;
      errEl.textContent = '';
      const profile = profileI.value.trim();
      if (!profile) { errEl.textContent = tr('auth.profile_name_required'); return; }
      if (vaultI.value.length < 8) { errEl.textContent = tr('profile.vault_passphrase_too_short'); return; }
      if (!backupI.value) { errEl.textContent = tr('profile.backup_passphrase_required'); return; }
      submitting = true;
      try {
        await Api.importIdentityToProfile(profile, vaultI.value, backupPath, backupI.value);
        if (!this.isImportActive(operation)) return;
        // Fetch separately so a close/navigation during the request cannot
        // publish a stale list into another selector or an active unlock form.
        const profiles = await Api.listProfiles();
        if (!this.isImportActive(operation)) return;
        this.store.profiles.set(profiles);
        if (!this.isImportActive(operation)) return;
        this.store.showToast(tr('profile.profile_imported', { p0: (profile) }));
        // Closing invalidates the operation; check it BEFORE our own close.
        if (!this.isImportActive(operation)) return;
        close();
        // Modal-close listeners can navigate or start another interaction too.
        if (this.disposed || this.importOperation !== operation || this.store.view.get() !== 'profile-select') return;
        // Selection is not authentication: the password still needs verifying.
        this.store.selectProfileForUnlock(profile);
      } catch (e) {
        if (this.isImportActive(operation)) errEl.textContent = errorText(e);
      } finally {
        submitting = false;
      }
    };
    this.closeImport = this.app.openModal((close) => h('div', { class: 'modal' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, tr('profile.import_backup_2')),
        h('button', { class: 'icon-btn', onClick: close }, 'x'),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'hint', style: { marginBottom: '10px' } }, tr('profile.from', { p0: filename })),
        h('div', { class: 'field' }, h('label', null, tr('profile.profile_name')), profileI),
        h('div', { class: 'field' }, h('label', null, tr('profile.new_vault_passphrase')), vaultI),
        h('div', { class: 'field' }, h('label', null, tr('profile.backup_passphrase_2')), backupI),
        errEl,
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: close }, tr('common.cancel')),
        h('button', { class: 'btn', onClick: () => submit(close) }, tr('profile.import')),
      ),
    ), () => {
      if (typeof backup !== 'string') forgetBrowserFiles([backupPath]);
      operation.active = false;
      if (this.importOperation === operation) this.closeImport = null;
      profileI.value = ''; vaultI.value = ''; backupI.value = '';
    });
  }

  override destroy(): void {
    this.disposed = true;
    this.invalidateImport();
    super.destroy();
  }

  private renderList(profiles: string[]): void {
    this.subTitle.textContent = tr('profile.profile_s_on_this_machine', { p0: (profiles.length) });
    this.list.replaceChildren();
    for (const p of profiles) {
      this.list.appendChild(h('div', { class: 'row', style: { gap: '6px' } },
        h('button', {
          class: 'btn btn-block salon-profile',
          style: { justifyContent: 'flex-start', flex: '1' },
          onClick: () => this.store.selectProfileForUnlock(p),
        }, h('span', { class: 'salon-profile-light', 'aria-hidden': 'true' }), h('span', { class: 'salon-profile-name' }, p)),
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
            } catch (e) { this.store.showToast(errorText(e), true); }
          },
        }, 'x'),
      ));
    }
  }
}
