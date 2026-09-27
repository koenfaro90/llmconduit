/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { fileURLToPath, URL } from 'node:url';
import { chmod, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { configDefaults } from 'vitest/config';

async function makeReadable(path: string): Promise<void> {
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = join(path, entry.name);
    if (entry.isDirectory()) {
      await makeReadable(child);
      await chmod(child, 0o755);
    } else if (entry.isFile()) {
      await chmod(child, 0o644);
    }
  }
}

// The Rust host (D8) embeds `dist/` via include_dir! and serves the SPA at `/dashboard`
// with static assets under `/dashboard/assets/*`. `base: '/dashboard/'` makes the built
// `index.html` reference absolute `/dashboard/assets/...` URLs that resolve under that
// mount regardless of the route hash (finding 1). A relative base would resolve against
// the current path (e.g. `#/topology`) and 404.
export default defineConfig({
  base: '/dashboard/',
  plugins: [react(), {
    name: 'readable-dashboard-build',
    async writeBundle(options) {
      // The watch container writes the bind-mounted bundle for the gateway's
      // nonroot uid. Vite can create assets with mode 0600 under a strict umask.
      if (options.dir) {
        await makeReadable(options.dir);
        await chmod(options.dir, 0o755);
      }
    },
  }],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // CSP forbids inline scripts (script-src 'self'); never inline assets as data: URIs.
    assetsInlineLimit: 0,
    sourcemap: false,
  },
  server: {
    port: 5273,
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./vitest.setup.ts'],
    css: false,
    // Playwright owns e2e/*.spec.ts; keep Vitest's default glob from grabbing them.
    exclude: [...configDefaults.exclude, 'e2e/**'],
  },
});
