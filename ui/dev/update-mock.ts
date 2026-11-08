import type { CheckResult, CoreEvent } from '../src/api';

/** Stateful preview backend: no downloads, installers or restarts leave the frame. */
export function previewUpdates(mode = 'current', emit: (event: CoreEvent) => Promise<void>, android = false,
  wait: (ms: number) => Promise<void> = ms => new Promise(resolve => setTimeout(resolve, ms))) {
  const release = { version: '0.5.1', notes: 'Console layout and update improvements.', size: 12_933_976 };
  let current = mode === 'current' ? release.version : '0.5.0-rc3';
  let automatic = true;
  let dismissed: string | null = null;
  let staged = mode === 'staged';
  let installing = false;
  const check = async (): Promise<CheckResult> => {
    await wait(200);
    if (mode === 'error') throw new Error('Preview update service could not be reached');
    if (mode === 'no_release') return { status: 'no_release' };
    if (mode === 'unavailable') return { status: 'unavailable' };
    if (mode === 'unsupported') return { status: 'unsupported', latest: release.version };
    if (mode === 'no_asset') return { status: 'no_asset', latest: release.version, wanted: 'gipny-preview.AppImage' };
    if (dismissed || mode === 'dismissed') return { status: 'dismissed', version: dismissed ?? release.version };
    if (mode === 'current' || current === release.version || staged) return { status: 'current', latest: staged ? release.version : current };
    return { status: 'update', ...release };
  };
  const install = async (): Promise<void> => {
    if (installing || staged) return;
    installing = true;
    try {
      for (const pct of [0, 35, 75, 100]) {
        await emit({ UpdateProgress: { downloaded: Math.round(release.size * pct / 100), total: release.size, pct } });
        await wait(250);
      }
      if (mode === 'error') throw new Error('Preview download failed');
      if (android) await emit({ UpdateInstallerOpened: { version: release.version, path: '/preview/gipny.apk' } });
      else { staged = true; await emit({ UpdateStaged: { version: release.version } }); }
    } catch (error) {
      await emit({ UpdateFailed: { reason: String(error) } });
      throw error;
    } finally { installing = false; }
  };
  return {
    async invoke(cmd: string, args: Record<string, unknown> = {}): Promise<unknown> {
      switch (cmd) {
        case 'current_version': return current;
        case 'update_configured': return mode !== 'unavailable';
        case 'update_installs_itself': return mode !== 'unsupported';
        case 'update_asks_for_root': return false;
        case 'check_update': return check();
        case 'install_update': return install();
        case 'dismiss_update': dismissed = String(args.version); return null;
        case 'get_auto_update': return automatic;
        case 'set_auto_update': automatic = args.enabled === true; return null;
        case 'restart_app':
        case 'apply_staged_update':
          if (staged) { current = release.version; staged = false; }
          return null;
        default: throw new Error(`Unknown preview update command: ${cmd}`);
      }
    },
    async announce(): Promise<void> {
      if (mode === 'available') await emit({ UpdateAvailable: release });
      if (staged) await emit({ UpdateStaged: { version: release.version } });
      if (mode === 'error') await emit({ UpdateFailed: { reason: 'Preview automatic update failed; check again in Settings.' } });
    },
  };
}
