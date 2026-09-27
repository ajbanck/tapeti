// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

/// <reference types="vitest/config" />
import { defineConfig } from 'vitest/config';
import preact from '@preact/preset-vite';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import type { Plugin } from 'vite';

/** This directory: the web app's root, whatever directory vite is run from. */
const here = fileURLToPath(new URL('.', import.meta.url));

/** Serves core/tests/samples/ at /samples/ from the dev server, for `npm run smoke`
 *  and `?open=samples/…`. The tapes are test fixtures, so a build ships none of them. */
function testSamples(): Plugin {
  const dir = path.resolve(here, '../core/tests/samples');
  return {
    name: 'test-samples',
    apply: 'serve',
    configureServer(server) {
      server.middlewares.use('/samples', (req, res, next) => {
        const name = decodeURIComponent((req.url ?? '').split('?')[0]);
        const file = path.resolve(dir, '.' + name);
        if (!file.startsWith(dir + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) return next();
        res.setHeader('Content-Type', 'application/octet-stream');
        fs.createReadStream(file).pipe(res);
      });
    },
  };
}

// `base: './'` keeps the build relocatable, so the same dist/ works on any web path.
export default defineConfig({
  root: here,
  plugins: [preact(), testSamples()],
  base: './',
  clearScreen: false,
  // Oldest engines we support: Safari 14.1 (macOS Big Sur 11.3 / WebKitGTK 2.32), Chrome 90, Firefox 90.
  build: {
    target: ['safari14', 'chrome90', 'firefox90'],
    // The core's wasm, inlined as base64 (scripts/build-wasm.mjs), is most of the bundle: a chunk
    // of its own keeps the app's chunk under the size warning and lets either change without
    // the other's cache going. Still a static import, so the page preloads both in parallel.
    rolldownOptions: {
      output: {
        codeSplitting: { groups: [{ name: 'core', test: /core\.wasm\.ts$/ }] },
      },
    },
  },
  server: {
    port: 5173,
    strictPort: true,
  },
  test: {
    environment: 'node',
    // Instantiates the wasm tape core before the first test parses a tape.
    setupFiles: ['./test/setup.ts'],
  },
});
