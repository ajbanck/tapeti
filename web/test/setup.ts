// The tape parser is the wasm core, which has to be instantiated before any
// test parses anything. Registered as `setupFiles` in vite.config.ts.
import { initCore } from '../src/tzx/core';

await initCore();
