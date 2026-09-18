// Getting tapes in and out of the store: parsing loaded bytes, saving through the platform
// adapter, and routing files that arrive from dialogs or drops.
import { Side, tapes, active, dialog, emptyTape, markSaved, insertBlocks, setStatus, showMessage, blockNo } from './store';
import { parseTape, isTzx } from '../tzx/parser';
import { serializeTzx, serializeTap, saveVersion } from '../tzx/writer';
import { platform, OpenedFile, TAPE_FILTERS, filtersForName, FileFilter, fileToOpened } from '../platform';

export function newTape(side: Side) {
  tapes[side].value = emptyTape();
  active.value = side;
}

export function loadBytes(side: Side, name: string, bytes: Uint8Array, insertAtCursor = false) {
  let parsed;
  try {
    parsed = parseTape(bytes);
  } catch (e) {
    showMessage('Cannot load file', (e as Error).message);
    return;
  }
  if (parsed.warnings.length) showMessage(`Warnings while loading ${name}`, parsed.warnings);
  if (insertAtCursor) {
    const t = tapes[side].value;
    insertBlocks(side, t.cursor < 0 ? t.blocks.length : t.cursor, parsed.blocks);
  } else {
    tapes[side].value = {
      ...emptyTape(name), blocks: parsed.blocks, saved: parsed.blocks, cursor: parsed.blocks.length ? 0 : -1,
      // TAP files carry no version; only remember it for real TZX headers
      loadedVersion: isTzx(bytes) ? { major: parsed.major, minor: parsed.minor } : null,
    };
  }
  active.value = side;
  setStatus(`Loaded ${name}: ${parsed.blocks.length} blocks, TZX v${parsed.major}.${String(parsed.minor).padStart(2, '0')}`);
}

/** Save bytes through the platform: a download, named after the tape. */
export async function downloadBytes(bytes: Uint8Array, name: string, mime = 'application/octet-stream', filters?: FileFilter[]) {
  const p = await platform();
  const r = await p.saveFile({ suggestedName: name, filters: filters ?? filtersForName(name), bytes, mime });
  if (r) setStatus(`Saved ${r.name}`);
  return r;
}

function stem(name: string) {
  return name.replace(/\.(tap|tzx)$/i, '') || 'tape';
}

/** Save as TZX. The browser downloads it; the desktop app (desktop/) writes in place. */
export async function saveTzx(side: Side) {
  const t = tapes[side].value;
  const v = saveVersion(t.blocks, t.loadedVersion);
  const bytes = serializeTzx(t.blocks, v);
  const p = await platform();
  const r = await p.saveFile({ suggestedName: stem(t.name) + '.tzx', filters: [{ name: 'TZX tape image', extensions: ['tzx'] }], bytes });
  if (!r) return;
  markSaved(side, { loadedVersion: v, name: r.name });
  setStatus(`Saved ${r.name} as TZX v${v.major}.${String(v.minor).padStart(2, '0')}`);
}

export async function saveTap(side: Side) {
  const t = tapes[side].value;
  const { bytes, skipped } = serializeTap(t.blocks);
  const p = await platform();
  const r = await p.saveFile({ suggestedName: stem(t.name) + '.tap', filters: [{ name: 'TAP tape image', extensions: ['tap'] }], bytes });
  if (!r) return;
  if (skipped.length) {
    showMessage('TAP export', [
      'TAP files can only hold data blocks. These blocks were skipped:',
      ...skipped.map((i) => `#${blockNo(i)}`),
      'Turbo/pure data blocks were written as standard blocks (their timings are lost).',
    ]);
  } else {
    markSaved(side, { name: r.name });
    setStatus(`Saved ${r.name}`);
  }
}

/** Load already-read files (from a dialog or a drop) into a tape. */
export function openTapeFiles(side: Side, files: OpenedFile[], insertAtCursor = false) {
  for (const f of files) loadBytes(side, f.name, f.bytes, insertAtCursor);
}

/** HTML5 drag & drop hands us File objects (no path). */
export async function openFiles(side: Side, files: FileList | File[], insertAtCursor = false) {
  openTapeFiles(side, await Promise.all(Array.from(files).map(fileToOpened)), insertAtCursor);
}

/** Show the platform's open dialog and load the chosen tapes. */
export async function pickAndOpen(side: Side, insertAtCursor = false) {
  const p = await platform();
  const files = await p.openFiles({ filters: TAPE_FILTERS, multiple: insertAtCursor });
  openTapeFiles(side, files, insertAtCursor);
}

/** Pick one arbitrary file (data window append/replace). */
export async function pickFile(): Promise<OpenedFile | null> {
  const p = await platform();
  const files = await p.openFiles({ filters: [{ name: 'All files', extensions: ['*'] }], multiple: false });
  return files[0] ?? null;
}

/** Run `then` now, or after the user agrees to drop the tape's unsaved changes. */
export function confirmDiscard(side: Side, then: () => void) {
  const t = tapes[side].value;
  if (!t.dirty) {
    then();
    return;
  }
  dialog.value = { kind: 'confirm', title: 'Discard changes?', lines: [`The ${side === 0 ? 'left' : 'right'} tape "${t.name}" has unsaved changes.`], onOk: then };
}
