// Getting tapes in and out of the store: parsing loaded bytes, saving through the platform
// adapter, and routing files that arrive from dialogs or drops.
import { Side, tapes, active, dialog, emptyTape, markSaved, insertBlocks, setStatus, showMessage, blockNo } from './store';
import {
  parseTape,
  isTzx,
  serializeTzx,
  serializeTap,
  saveVersion,
  checksum,
  encodeHeader,
  fileHashes,
  snapshotKind,
  snapshotInfo,
  snapshotToTape,
  SnapshotKind,
  SnapshotOptions,
  SNAPSHOT_SPEEDS,
} from '../tzx/core';
import { Block, ParsedTape, StandardBlock, createBlock } from '../tzx/types';
import { platform, OpenedFile, TAPE_FILTERS, filtersForName, FileFilter, fileToOpened } from '../platform';

export function newTape(side: Side) {
  tapes[side].value = emptyTape();
  active.value = side;
}

export function loadBytes(side: Side, name: string, bytes: Uint8Array, insertAtCursor = false) {
  const format = snapshotKind(name);
  // Inserting something that is no tape goes in as a data block, once the
  // dialog has said where it loads. Opening still reads anything as a TAP,
  // which is what a tape with an odd extension needs.
  if (insertAtCursor && !format && !isTzx(bytes) && !/\.tap$/i.test(name)) {
    dialog.value = { kind: 'datafile', side, name, bytes };
    return;
  }
  let parsed;
  try {
    if (format) {
      // A snapshot becomes a tape only once the dialog has its answers.
      dialog.value = { kind: 'snapshot', side, name, bytes, format, info: snapshotInfo(bytes, format), insertAtCursor };
      return;
    }
    parsed = parseTape(bytes);
  } catch (e) {
    showMessage('Cannot load file', (e as Error).message);
    return;
  }
  if (parsed.warnings.length) showMessage(`Warnings while loading ${name}`, parsed.warnings);
  loadParsed(side, name, parsed, insertAtCursor, isTzx(bytes), bytes);
  setStatus(`Loaded ${name}: ${parsed.blocks.length} blocks, TZX v${parsed.major}.${String(parsed.minor).padStart(2, '0')}`);
}

/** The most a standard block's payload can be: its 16 bit length counts flag and checksum too. */
export const MAX_FILE_BYTES = 0xffff - 2;

/** A header's ten-character name made from a file name: the stem, printable ASCII only. `header_name` in the core. */
export function headerName(file: string): string {
  const base = file.split(/[\\/]/).pop() ?? file;
  const stem = base.includes('.') ? base.slice(0, base.lastIndexOf('.')) : base;
  const name = stem.replace(/[^ -~]/g, '').slice(0, 10);
  return name.trim() ? name : 'file';
}

/**
 * The data file dialog's OK: the file as the blocks SAVE "name" CODE would make, at
 * the cursor. `file_blocks` in the core.
 */
export function insertDataFile(side: Side, name: string, bytes: Uint8Array, address: number, withHeader: boolean) {
  if (bytes.length > MAX_FILE_BYTES) {
    showMessage('Cannot insert file', `A data block holds ${MAX_FILE_BYTES} bytes at most; this file has ${bytes.length}`);
    return;
  }
  const data = new Uint8Array(bytes.length + 2);
  data[0] = 0xff;
  data.set(bytes, 1);
  data[data.length - 1] = checksum(data, 0, data.length - 1);
  const blocks: Block[] = [{ ...(createBlock(0x10) as StandardBlock), data }];
  if (withHeader) {
    const header = encodeHeader({ type: 3, typeName: 'Bytes', name, length: bytes.length, param1: address, param2: 32768 });
    blocks.unshift({ ...(createBlock(0x10) as StandardBlock), data: header });
  }
  const t = tapes[side].value;
  insertBlocks(side, t.cursor < 0 ? t.blocks.length : t.cursor, blocks);
  active.value = side;
  setStatus(`Inserted ${bytes.length} bytes as "${name.trimEnd()}"`);
}

/** The snapshot import dialog's OK: build the tape that loads the snapshot. */
export function importSnapshot(side: Side, name: string, bytes: Uint8Array, format: SnapshotKind, opts: SnapshotOptions, insertAtCursor: boolean) {
  let parsed;
  try {
    parsed = snapshotToTape(bytes, format, name, opts);
  } catch (e) {
    showMessage('Cannot import snapshot', (e as Error).message);
    return;
  }
  loadParsed(side, name, parsed, insertAtCursor, false, null);
  setStatus(`Imported ${name}: ${parsed.blocks.length} blocks, loading at ${SNAPSHOT_SPEEDS[opts.speed]} bps`);
}

/** `file` is the bytes of the file the tape was read from; a snapshot's tape is no such file. */
function loadParsed(side: Side, name: string, parsed: ParsedTape, insertAtCursor: boolean, fromTzx: boolean, file: Uint8Array | null) {
  if (insertAtCursor) {
    const t = tapes[side].value;
    insertBlocks(side, t.cursor < 0 ? t.blocks.length : t.cursor, parsed.blocks);
  } else {
    tapes[side].value = {
      ...emptyTape(name), blocks: parsed.blocks, saved: parsed.blocks, cursor: parsed.blocks.length ? 0 : -1,
      // TAP files and snapshots carry no version; only remember it for real TZX headers
      loadedVersion: fromTzx ? { major: parsed.major, minor: parsed.minor } : null,
      fileHashes: file && fileHashes(file),
    };
  }
  active.value = side;
}

/** Save bytes through the platform: a download, named after the tape. */
export async function downloadBytes(bytes: Uint8Array, name: string, mime = 'application/octet-stream', filters?: FileFilter[]) {
  const p = await platform();
  const r = await p.saveFile({ suggestedName: name, filters: filters ?? filtersForName(name), bytes, mime });
  if (r) setStatus(`Saved ${r.name}`);
  return r;
}

function stem(name: string) {
  return name.replace(/\.(tap|tzx|z80|sna)$/i, '') || 'tape';
}

/** Save as TZX. The browser downloads it; the desktop app (desktop/) writes in place. */
export async function saveTzx(side: Side) {
  const t = tapes[side].value;
  const v = saveVersion(t.blocks, t.loadedVersion);
  const bytes = serializeTzx(t.blocks, v);
  const p = await platform();
  const r = await p.saveFile({ suggestedName: stem(t.name) + '.tzx', filters: [{ name: 'TZX tape image', extensions: ['tzx'] }], bytes });
  if (!r) return;
  markSaved(side, { loadedVersion: v, name: r.name, fileHashes: fileHashes(bytes) });
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
    markSaved(side, { name: r.name, fileHashes: fileHashes(bytes) });
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
  // Inserting takes any file: what is no tape goes in as a data block.
  const filters = insertAtCursor ? [...TAPE_FILTERS, { name: 'Any file (inserted as a data block)', extensions: ['*'] }] : TAPE_FILTERS;
  const files = await p.openFiles({ filters, multiple: insertAtCursor });
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
