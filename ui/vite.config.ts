import { defineConfig } from 'vite';
import { buildDefinitions } from './build-policy';

// The oldest WebView the Android build has to run in. China-market Honor and
// Huawei phones without Google services ship Huawei WebView 11 (EMUI 11), which
// is Chromium 83 and never updates past it; a bundle emitted for a newer
// baseline fails to parse there and the app opens as a blank page.
// ui/tests/webview-compat.test.mjs holds the build to this floor.
export const WEBVIEW_FLOOR = ['chrome83'];

export default defineConfig(({ command }) => ({
  define: buildDefinitions(command === 'serve'),
  base: process.env.VITE_BASE || '/',
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: {
    target: WEBVIEW_FLOOR,
    cssTarget: WEBVIEW_FLOOR,
    sourcemap: false,
    outDir: 'dist',
    emptyOutDir: true,
  },
}));
