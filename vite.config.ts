/// <reference types="vitest/config" />
import { defineConfig } from 'vitest/config';
import preact from '@preact/preset-vite';

// `base: './'` keeps the build relocatable, so the same dist/ works on any web path.
export default defineConfig({
  plugins: [preact()],
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
