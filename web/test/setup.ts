// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

// The tape parser is the wasm core, which has to be instantiated before any
// test parses anything. Registered as `setupFiles` in vite.config.ts.
import { initCore } from '../src/tzx/core';

await initCore();
