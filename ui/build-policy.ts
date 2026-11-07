import { readFileSync } from 'node:fs';

const version: string = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).version;
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) throw new Error('Invalid application version');

/** Stable builds cannot opt into diagnostics using a runtime/environment flag. */
export function buildDefinitions(development: boolean): Record<string, string> {
  return {
    __GIPNY_VERSION__: JSON.stringify(version),
    __GIPNY_DIAGNOSTICS__: JSON.stringify(development || version.includes('-')),
  };
}
