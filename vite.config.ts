/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

const host = process.env.TAURI_DEV_HOST;

// https://v2.tauri.app/start/frontend/vite/
// Port 1430 (not V1's 1420) so V1 and V2 dev servers can run side by side.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1430,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1431 } : undefined,
    watch: { ignored: ['**/src-tauri/**', '**/crates/**', '**/target/**', '**/browser-extension/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    target: 'es2022',
    sourcemap: false,
  },
  test: {
    projects: [
      // The UI's tests run in jsdom, with Tauri's IPC mocked (src/test/setup.ts, #29).
      {
        extends: true,
        test: {
          name: 'ui',
          environment: 'jsdom',
          include: ['src/**/*.test.{ts,tsx}'],
          setupFiles: ['src/test/setup.ts'],
        },
      },
      // The field test's tools (#262) are checked with the scripts; the field test itself runs by hand.
      { extends: true, test: { name: 'scripts', environment: 'node', include: ['scripts/**/*.test.mjs', 'tests/field/**/*.test.mjs'] } },
      // The browser extension's logic runs against fakes of the WebExtension APIs (#64).
      { extends: true, test: { name: 'extension', environment: 'node', include: ['browser-extension/tests/**/*.test.ts'] } },
    ],
  },
});
