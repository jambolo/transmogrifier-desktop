import { readFileSync } from 'node:fs';
import { defineConfig } from 'vitest/config';

// Only the version string is injected, not package.json.
const { version } = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')) as { version: string };

// Vite config tuned for Tauri: fixed dev port, no screen clearing so Rust logs
// stay visible, and a build target modern webviews support. Imported from
// "vitest/config" rather than "vite" so the `test` block below is typed; this
// is otherwise a drop-in superset of a normal Vite config.
export default defineConfig({
  clearScreen: false,
  define: {
    __APP_VERSION__: JSON.stringify(version),
  },
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: 'dist',
    target: 'esnext',
    emptyOutDir: true,
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
  },
});
