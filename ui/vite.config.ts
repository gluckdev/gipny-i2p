import { defineConfig } from 'vite';
import { buildDefinitions } from './build-policy';

export default defineConfig(({ command }) => ({
  define: buildDefinitions(command === 'serve'),
  base: process.env.VITE_BASE || '/',
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: {
    target: 'es2022',
    sourcemap: false,
    outDir: 'dist',
    emptyOutDir: true,
  },
}));
