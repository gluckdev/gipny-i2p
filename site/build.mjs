import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, resolve } from 'node:path';
import { messages } from './main.js';

const root = dirname(fileURLToPath(import.meta.url));
export const publicFiles = ['index.html', 'styles.css', 'main.js', 'assets/logo.svg'];
export const catalogKeys = ['catalogRelease', 'catalogSystem', 'catalogChoose', 'catalogDownload', 'catalogAll', 'catalogHelp', 'catalogUnavailable'];

const repository = 'https://github.com/gluckdev/gipny-i2p';
const escapeHtml = value => String(value).replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[char]));
const translated = key => '<span data-i18n="' + key + '">' + escapeHtml(messages.ru[key]) + '</span>';

export function classifyAsset(name, version) {
  const app = 'gipny-i2p_' + version + '_';
  const native = {
    'x64-setup.exe': ['Windows', 'x86_64', 'EXE', 'windows'],
    'amd64.AppImage': ['Linux', 'x86_64', 'AppImage', 'linux'],
    'aarch64.AppImage': ['Linux', 'ARM64', 'AppImage', 'linux'],
    'amd64.deb': ['Linux', 'x86_64', 'DEB', 'linux'],
    'arm64.deb': ['Linux', 'ARM64', 'DEB', 'linux'],
    'x64.dmg': ['macOS', 'Intel x86_64', 'DMG', 'macos'],
    'aarch64.dmg': ['macOS', 'Apple Silicon ARM64', 'DMG', 'macos'],
    'android-arm64.apk': ['Android', 'ARM64', 'APK', 'android'],
    'android-armv7.apk': ['Android', 'ARMv7', 'APK', 'android'],
  };
  if (name.startsWith(app) && Object.hasOwn(native, name.slice(app.length))) {
    const [platform, arch, format, icon] = native[name.slice(app.length)];
    return { group: 'native', platform, arch, format, icon };
  }
  for (const component of ['agent', 'relay']) {
    const prefix = 'gipny-' + component + '_' + version + '_';
    if (!name.startsWith(prefix) || !name.endsWith('.tar.gz')) continue;
    const target = name.slice(prefix.length, -7);
    const targets = {
      'linux-amd64': ['Linux', 'x86_64 · GNU', 'linux'],
      'linux-arm64': ['Linux', 'ARM64 · GNU', 'linux'],
      ...(component === 'agent' ? {
        'darwin-amd64': ['macOS', 'Intel x86_64', 'macos'],
        'darwin-arm64': ['macOS', 'Apple Silicon ARM64', 'macos'],
        'windows-amd64': ['Windows', 'x86_64', 'windows'],
        'aarch64-unknown-linux-musl': ['OpenWrt / musl', 'ARM64 / AArch64', 'router'],
        'armv7-unknown-linux-musleabi': ['OpenWrt / musl', 'ARMv7-A · soft-float', 'router'],
        'i686-unknown-linux-musl': ['OpenWrt / musl', 'x86 · Pentium 4+ / SSE2', 'router'],
        'x86_64-unknown-linux-musl': ['OpenWrt / musl', 'x86_64', 'router'],
      } : {}),
    };
    if (!Object.hasOwn(targets, target)) break;
    const [platform, arch, icon] = targets[target];
    return { group: icon === 'router' ? 'openwrt' : component + 's', platform, arch, format: 'TAR.GZ', icon };
  }
  if (['SHA256SUMS.txt', 'BUILD-MANIFEST.json', 'SOURCE-MANIFEST.json'].includes(name)) return { group: 'metadata' };
  const source = 'gipny-source_' + version + '_';
  if (name.startsWith(source) && /^[0-9a-f]{40}\.tar\.gz$/.test(name.slice(source.length))) return { group: 'source' };
  throw new Error('Unrecognized release asset: ' + name);
}

export function releaseCatalog(release) {
  if (!release || release.draft !== false || release.prerelease !== false ||
      typeof release.published_at !== 'string' || !Number.isFinite(Date.parse(release.published_at)) ||
      typeof release.tag_name !== 'string' || !/^v?\d+\.\d+\.\d+$/.test(release.tag_name) ||
      !Number.isSafeInteger(release.id) || release.id <= 0 ||
      release.url !== 'https://api.github.com/repos/gluckdev/gipny-i2p/releases/' + release.id ||
      release.html_url !== repository + '/releases/tag/' + release.tag_name ||
      !Array.isArray(release.assets) || release.assets.length === 0 || release.assets.length > 64) {
    throw new Error('Expected a published stable release from gluckdev/gipny-i2p');
  }
  const version = release.tag_name.replace(/^v/, '');
  const names = new Set();
  const assets = release.assets.map(asset => {
    if (!asset || typeof asset.name !== 'string' || !/^[A-Za-z0-9_.-]+$/.test(asset.name) ||
        names.has(asset.name) || asset.state !== 'uploaded' ||
        !Number.isSafeInteger(asset.size) || asset.size <= 0 ||
        asset.browser_download_url !== repository + '/releases/download/' + release.tag_name + '/' + asset.name) {
      throw new Error('Invalid or duplicate uploaded release asset');
    }
    names.add(asset.name);
    return { ...classifyAsset(asset.name, version), name: asset.name, size: asset.size, url: asset.browser_download_url };
  });
  if (!assets.some(asset => asset.group === 'native')) throw new Error('Release has no supported native application');
  return { tag: release.tag_name, url: release.html_url, assets };
}

/** One preferred native package per platform/architecture; full catalog stays in Releases. */
export function nativeChoices(catalog) {
  const choices = new Map();
  for (const asset of catalog.assets.filter(asset => asset.group === 'native')) {
    const key = asset.platform + '/' + asset.arch;
    if (!choices.has(key) || asset.format === 'AppImage') choices.set(key, asset);
  }
  return [...choices.values()];
}

export function renderCatalog(catalog) {
  const options = nativeChoices(catalog).map(asset => {
    const architecture = /ARM64/.test(asset.arch) ? 'arm64' : asset.arch === 'ARMv7' ? 'arm' : 'x64';
    const label = asset.platform + ' · ' + asset.arch + ' · ' + asset.format;
    const size = (asset.size / 1_000_000).toFixed(1) + ' MB';
    return '<option value="' + escapeHtml(asset.url) + '" data-platform="' + escapeHtml(asset.platform) +
      '" data-architecture="' + architecture + '" data-download-asset="' + escapeHtml(asset.name) +
      '" data-size="' + size + '">' + escapeHtml(label) + '</option>';
  }).join('');
  return [
    '<p class="catalog-release">' + translated('catalogRelease') + ' <span>' + escapeHtml(catalog.tag) + '</span></p>',
    '<div class="download-choice" id="download-choice" hidden>',
    '<label for="download-system">' + translated('catalogSystem') + '</label>',
    '<select id="download-system" aria-describedby="download-help"><option value="" data-i18n="catalogChoose">' + escapeHtml(messages.ru.catalogChoose) + '</option>' + options + '</select>',
    '<p id="download-help" class="download-help">' + translated('catalogHelp') + '</p>',
    '<p id="download-unavailable" hidden>' + translated('catalogUnavailable') + '</p>',
    '<a id="download-primary" class="button button-primary" hidden></a>',
    '<p id="download-detail" class="download-help" aria-live="polite"></p>',
    '</div>',
    '<p class="catalog-fallback"><a href="' + repository + '/releases" data-i18n="catalogAll">' + escapeHtml(messages.ru.catalogAll) + '</a></p>',
  ].join('\n');
}

export async function build(output, release) {
  const destination = resolve(output);
  if (destination === root) throw new Error('Build output must differ from the source directory');
  // null is an authenticated API observation that no stable release exists.
  // Invalid release objects still fail validation; never invent asset links.
  const catalog = release == null ? undefined : renderCatalog(releaseCatalog(release));
  for (const file of publicFiles) {
    const target = resolve(destination, file);
    await mkdir(dirname(target), { recursive: true });
    if (file === 'index.html' && catalog !== undefined) {
      const source = await readFile(resolve(root, file), 'utf8');
      const marker = /<!-- download-catalog:start -->[\s\S]*?<!-- download-catalog:end -->/;
      if (!marker.test(source)) throw new Error('Download catalog marker is missing');
      await writeFile(target, source.replace(marker, '<!-- download-catalog:start -->\n' + catalog + '\n<!-- download-catalog:end -->'));
    } else {
      await copyFile(resolve(root, file), target);
    }
  }
  return destination;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const release = process.argv[3] ? JSON.parse(await readFile(resolve(process.argv[3]), 'utf8')) : undefined;
  console.log('Static site: ' + await build(process.argv[2] ?? '_site', release));
}
