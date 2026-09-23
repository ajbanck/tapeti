/// <reference types="vitest/config" />
import { defineConfig } from 'vitest/config';
import preact from '@preact/preset-vite';
import fs from 'node:fs';
import path from 'node:path';
import type { Plugin } from 'vite';

/** Serves test/samples/ at /samples/ from the dev server, for `npm run smoke` and
 *  `?open=samples/…`. The tapes are test fixtures, so a build ships none of them. */
function testSamples(): Plugin {
  const dir = path.resolve('test/samples');
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
  plugins: [preact(), testSamples()],
  base: './',
  clearScreen: false,
  // Oldest engines we support: Safari 14.1 (macOS Big Sur 11.3 / WebKitGTK 2.32), Chrome 90, Firefox 90.
  build: {
    target: ['safari14', 'chrome90', 'firefox90'],
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
