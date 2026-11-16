// The Android build runs inside whatever WebView the phone has. China-market
// Honor/Huawei phones without Google services ship Huawei WebView 11 (Chromium
// 83) and never update past it. A bundle with newer syntax fails to parse there
// and the app opens as a blank page — rc3 did (2026-11-07). This holds the
// built Tauri UI (ui/dist) to that floor: every script parses without syntax
// Chromium 83 lacks, and no unshimmed newer API is referenced.
//
// Run after `npm run build`. The browser build (browser-dist) is not covered:
// it targets current browsers and uses top-level await by design.
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { scanWebviewCompatibility } from './webview-compat-scan.mjs';

const dist = new URL('../dist/', import.meta.url).pathname;
if (!existsSync(join(dist, 'assets'))) {
  // The browser-only CI job builds browser-dist and never dist/; the gate's
  // subject is the Tauri bundle, and the `ui` job builds it before testing.
  console.log('webview-compat: no dist/ (Tauri build) here — nothing to hold to Chromium 83; `npm run build` makes it');
  process.exit(0);
}
const files = [join(dist, 'theme-init.js'), ...readdirSync(join(dist, 'assets')).filter(f => f.endsWith('.js')).map(f => join(dist, 'assets', f))];
if (files.length < 2) throw new Error('webview-compat: dist/ holds no scripts; run `npm run build`');

let failures = 0;
for (const file of files) {
  const src = readFileSync(file, 'utf8');
  for (const finding of scanWebviewCompatibility(src)) {
    failures++;
    console.error(`webview-compat: ${file.replace(dist, 'dist/')}: ${finding.what} at line ${finding.line}: …${finding.excerpt}…`);
  }
}

if (failures) { console.error(`webview-compat: ${failures} finding(s); see WEBVIEW_FLOOR in vite.config.ts`); process.exit(1); }
console.log(`webview-compat: ${files.length} script(s) hold to Chromium 83`);
