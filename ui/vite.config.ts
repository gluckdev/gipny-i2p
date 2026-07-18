import { defineConfig, type Plugin } from 'vite';

// Build-time theme selection. The classic look is the default and is never
// touched. Setting VITE_THEME=xbox injects an extra stylesheet (loaded AFTER
// styles.css, so it overrides) — used only by the separate Xbox build variant.
// No runtime switch, no behaviour change.
function themePlugin(): Plugin {
  const theme = process.env.VITE_THEME;
  return {
    name: 'gipny-theme',
    transformIndexHtml(html) {
      if (theme === 'xbox') {
        return html.replace(
          '</head>',
          '  <link rel="stylesheet" href="/src/themes/xbox.css" />\n  </head>',
        );
      }
      return html;
    },
  };
}

export default defineConfig({
  clearScreen: false,
  plugins: [themePlugin()],
  server: { port: 5173, strictPort: true },
  build: {
    target: 'es2022',
    sourcemap: false,
    outDir: 'dist',
    emptyOutDir: true,
  },
});
