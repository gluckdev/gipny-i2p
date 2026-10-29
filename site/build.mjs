import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, resolve } from 'node:path';
import { messages } from './main.js';

const root = dirname(fileURLToPath(import.meta.url));
export const publicFiles = ['index.html', 'styles.css', 'main.js', 'assets/logo.svg'];
export const catalogKeys = ['catalogRelease', 'catalogAdvanced', 'catalogAdvancedNote', 'catalogAgents', 'catalogRelays', 'catalogOpenWrt', 'catalogSource', 'catalogVerification'];

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

function icon(kind) {
  const shapes = {
    windows: '<path d="M3 4l8-1v8H3zm10-1 8-1v9h-8zM3 13h8v8l-8-1zm10 0h8v9l-8-1z"/>',
    linux: '<path d="M8 9c0-4 1-7 4-7s4 3 4 7c0 2 3 5 3 8 0 3-3 4-7 4s-7-1-7-4c0-3 3-6 3-8z" fill="none" stroke="currentColor" stroke-width="1.5"/><circle cx="10" cy="8" r="1"/><circle cx="14" cy="8" r="1"/><path d="m10 11 2 2 2-2z"/>',
    macos: '<rect x="3" y="3" width="18" height="14" rx="2" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 21h8M12 17v4" fill="none" stroke="currentColor" stroke-width="1.5"/>',
    android: '<path d="M5 10h14v9H5zM5 8c1-7 13-7 14 0z"/><path d="m7 2 2 3m8-3-2 3M3 11v7m18-7v7M8 19v3m8-3v3" fill="none" stroke="currentColor" stroke-width="1.5"/><circle cx="9" cy="6" r=".7" fill="var(--background)"/><circle cx="15" cy="6" r=".7" fill="var(--background)"/>',
    router: '<rect x="3" y="12" width="18" height="8" rx="2" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M6 12V4m12 8V4M9 16h1m3 0h1" fill="none" stroke="currentColor" stroke-width="1.5"/>',
  };
  return '<svg class="platform-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false" fill="currentColor">' + shapes[kind] + '</svg>';
}

function assetLink(asset, advanced = false) {
  const size = (asset.size / 1_000_000).toFixed(1) + ' MB';
  const label = (advanced ? asset.platform + ' · ' : '') + asset.arch;
  return '<a class="download-file" data-download-asset="' + escapeHtml(asset.name) + '" href="' + escapeHtml(asset.url) + '" title="' + escapeHtml(asset.name) + '" aria-label="' + escapeHtml(asset.platform + ' · ' + asset.arch + ' · ' + asset.format + ' · ' + size) + '">' +
    '<span class="download-architecture">' + escapeHtml(label) + '</span><span class="download-file-info">' + escapeHtml(asset.format + ' · ' + size) + '</span><span class="download-arrow" aria-hidden="true">↓</span></a>';
}

export function renderCatalog(catalog) {
  const cards = ['Windows', 'Linux', 'macOS', 'Android'].map(platform => {
    const assets = catalog.assets.filter(asset => asset.group === 'native' && asset.platform === platform);
    if (!assets.length) return '';
    return '<article class="download-platform"><h3>' + icon(assets[0].icon) + escapeHtml(platform) + '</h3><div class="download-files">' + assets.map(asset => assetLink(asset)).join('') + '</div></article>';
  }).join('');
  const advanced = [['agents', 'catalogAgents'], ['relays', 'catalogRelays'], ['openwrt', 'catalogOpenWrt']].map(([group, label]) => {
    const assets = catalog.assets.filter(asset => asset.group === group);
    if (!assets.length) return '';
    return '<section class="download-component"><h4>' + translated(label) + '</h4><div class="download-grid">' +
      assets.map(asset => '<div class="download-platform download-component-file">' + icon(asset.icon) + assetLink(asset, true) + '</div>').join('') + '</div></section>';
  }).join('');
  const extra = catalog.assets.filter(asset => ['metadata', 'source'].includes(asset.group)).map(asset =>
    '<a href="' + escapeHtml(asset.url) + '">' + (asset.group === 'source' ? translated('catalogSource') : escapeHtml(asset.name)) + '</a>').join('');
  return [
    '<p class="catalog-release">' + translated('catalogRelease') + ' <a href="' + escapeHtml(catalog.url) + '">' + escapeHtml(catalog.tag) + '</a></p>',
    '<div class="download-grid">' + cards + '</div>',
    advanced ? '<details class="download-advanced"><summary>' + translated('catalogAdvanced') + '</summary><p class="download-advanced-note">' + translated('catalogAdvancedNote') + '</p>' + advanced + '</details>' : '',
    extra ? '<p class="catalog-verification">' + translated('catalogVerification') + '</p><div class="catalog-resources">' + extra + '</div>' : '',
  ].join('\n');
}

export async function build(output, release) {
  const destination = resolve(output);
  if (destination === root) throw new Error('Build output must differ from the source directory');
  const catalog = release === undefined ? undefined : renderCatalog(releaseCatalog(release));
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
