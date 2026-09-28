// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

import { render } from 'preact';
import { App } from './ui/App';
import { applyTheme, theme, themePinned } from './state/store';
import { loadBytes } from './state/files';
import { initCore } from './tzx/core';

applyTheme(theme.value);
// A pinned theme (?theme=auto included) is settled at load: the embedding page may flip
// data-theme itself afterwards, and following the system here would undo that.
if (!themePinned) {
  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => applyTheme(theme.value));
}

// start() can parse a tape immediately, so the core is instantiated first; the module
// is inlined in the bundle, so this costs no request. Written as a callback rather than
// top-level await, which Safari 14.1 does not have.
initCore()
  .catch((e) => console.error('The tape core failed to load', e))
  .then(start);

function start() {
  render(<App />, document.getElementById('app')!);

  // Optional: open tapes given as URL parameters, e.g. ?open=tapes/foo.tzx&right=tapes/bar.tzx
  // (the dev server serves core/tests/samples/ at samples/, which is what the smoke test opens).
  const params = new URLSearchParams(location.search);
  for (const [key, side] of [['open', 0], ['right', 1]] as const) {
    const url = params.get(key);
    if (!url) continue;
    fetch(url)
      .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`${r.status} ${r.statusText}`))))
      .then((buf) => loadBytes(side, decodeURIComponent(url.split('/').pop() ?? url), new Uint8Array(buf)))
      .catch((e) => console.error('Could not open', url, e));
  }
}
