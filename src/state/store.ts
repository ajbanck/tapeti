// Application state as Preact signals, plus the editing operations on it. File I/O lives in
// files.ts, higher-level actions in actions.ts, and the command table in commands.ts.
import { signal } from '@preact/signals';
import { Block, cloneBlock } from '../tzx/types';
import { BlockCompareMode, TapeCompareMode, CompareResult, compareTapes, findMatches } from '../tzx/compare';
import { groupRanges } from '../tzx/programs';

export type Side = 0 | 1;

export interface TapeState {
  name: string;
  blocks: Block[];
  cursor: number; // index of the current block, -1 if none
  selected: Set<number>; // uids
  collapsed: Set<number>; // uids of group/loop start blocks that are collapsed
  dirty: boolean;
  /** The blocks array as of the last load or save; undo/redo compare against it to keep `dirty` honest. */
  saved: Block[];
  loadedVersion: { major: number; minor: number } | null;
  compare: Map<number, CompareResult>; // uid -> result colour
  undo: Snapshot[];
  redo: Snapshot[];
}

interface Snapshot {
  blocks: Block[];
  cursor: number;
  selected: Set<number>;
}

export function emptyTape(name = 'new'): TapeState {
  const blocks: Block[] = [];
  return {
    name, blocks, cursor: -1, selected: new Set(), collapsed: new Set(), dirty: false, saved: blocks,
    loadedVersion: null, compare: new Map(), undo: [], redo: [],
  };
}

export const tapes = [signal<TapeState>(emptyTape()), signal<TapeState>(emptyTape())] as const;
export const active = signal<Side>(0);
/**
 * The **main window's** Dec/Hex switch, the one in the status bar. It belongs to
 * that screen and to nothing else: it is not remembered between sessions, and
 * the data window has a switch of its own (`useState` there), so turning this
 * one to Hex leaves an open data window as it was.
 */
export const hex = signal(false);
export const locked = signal(false);
export const blockCompare = signal<BlockCompareMode>('data');
export const tapeCompare = signal<TapeCompareMode>('datablocks');
export const audioMode = signal<'mic' | 'square'>('mic');
export const clipboard = signal<Block[]>([]);
export const status = signal<string>('');

export type Theme = 'light' | 'dark' | 'system';
function loadTheme(): Theme {
  try {
    const v = localStorage.getItem('tapeti.theme');
    if (v === 'light' || v === 'dark' || v === 'system') return v;
  } catch { /* storage unavailable */ }
  return 'system';
}
export const theme = signal<Theme>(loadTheme());

function loadFlag(key: string, dflt = false): boolean {
  try {
    const v = localStorage.getItem(key);
    return v === null ? dflt : v === '1';
  } catch { return dflt; }
}
/**
 * Global display options, persisted per browser / desktop install. Blocks are
 * numbered from 0 unless the option is turned off: the block number is an index
 * into the tape, and that is where the file format and the jump targets count from.
 */
export const zeroBased = signal(loadFlag('tapeti.zeroBased', true));
export const hexBytes = signal(loadFlag('tapeti.hexBytes'));
export function setOption(opt: 'zeroBased' | 'hexBytes', on: boolean) {
  (opt === 'zeroBased' ? zeroBased : hexBytes).value = on;
  try { localStorage.setItem('tapeti.' + opt, on ? '1' : '0'); } catch { /* ignore */ }
}
export function applyTheme(t: Theme) {
  theme.value = t;
  try { localStorage.setItem('tapeti.theme', t); } catch { /* ignore */ }
  const dark = t === 'dark' || (t === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
}
export function cycleTheme() {
  const order: Theme[] = ['system', 'light', 'dark'];
  applyTheme(order[(order.indexOf(theme.value) + 1) % order.length]);
}

export interface DataWindowRequest {
  side: Side;
  uids: number[]; // one or more blocks viewed as one
}
export const dataWindow = signal<DataWindowRequest | null>(null);

export type Dialog =
  | { kind: 'insert'; side: Side }
  | { kind: 'tapeinfo'; side: Side }
  | { kind: 'consistency'; side: Side }
  | { kind: 'about' }
  | { kind: 'message'; title: string; lines: string[] }
  | { kind: 'wav'; side: Side }
  | { kind: 'programs'; side: Side }
  | { kind: 'confirm'; title: string; lines: string[]; onOk: () => void }
  | { kind: 'emulator' };
export const dialog = signal<Dialog | null>(null);

export function showMessage(title: string, lines: string[] | string) {
  dialog.value = { kind: 'message', title, lines: Array.isArray(lines) ? lines : [lines] };
}

export function setStatus(s: string) {
  status.value = s;
  if (s) setTimeout(() => { if (status.value === s) status.value = ''; }, 6000);
}

/**
 * A number in the base a screen's switch selects. `hex` is the main window's,
 * which is what a number drawn there follows; a screen with a switch of its own
 * passes it in.
 */
export function fmtNum(n: number, h = hex.value): string {
  if (h) return n.toString(16).toUpperCase();
  return n.toString(10);
}
/** Display number of the block at 0-based index `i`, honouring the zero-based option. */
export function blockNo(i: number): number {
  return zeroBased.value ? i : i + 1;
}
/** Seconds as m:ss.s, e.g. 1:05.3. */
export function fmtTime(sec: number): string {
  const m = Math.floor(sec / 60);
  const s = sec - m * 60;
  return `${m}:${s.toFixed(1).padStart(4, '0')}`;
}

/**
 * A flag or checksum byte: hex under the screen's switch or the "flag and
 * checksum bytes in hex" option. The `0x` is part of it either way — these
 * bytes are read out of a sentence ("Checksum byte 0x17"), where a bare 17
 * would not say which base it is in.
 */
export function fmtByte(n: number, h = hex.value): string {
  if (h || hexBytes.value) return '0x' + n.toString(16).toUpperCase().padStart(2, '0');
  return n.toString(10).padStart(3, '0');
}
/** `$`/`0x` force hex and `#` forces decimal; anything else follows the screen's switch. */
export function parseNum(s: string, h = hex.value): number {
  s = s.trim();
  if (s === '') return NaN;
  const neg = s.startsWith('-');
  if (neg) s = s.slice(1);
  let v: number;
  if (/^(\$|0x)/i.test(s)) v = parseInt(s.replace(/^(\$|0x)/i, ''), 16);
  else if (/^#/.test(s)) v = parseInt(s.slice(1), 10);
  else v = parseInt(s, h ? 16 : 10);
  return neg ? -v : v;
}

/** Replace fields of a tape without touching undo. Structural edits must go through `commit`. */
export function patch(side: Side, p: Partial<TapeState>) {
  tapes[side].value = { ...tapes[side].value, ...p };
}

/** Record that the current blocks are what is on disk (called after a successful save). */
export function markSaved(side: Side, p: Partial<TapeState> = {}) {
  patch(side, { ...p, dirty: false, saved: tapes[side].value.blocks });
}

export function toggleLock() {
  locked.value = !locked.value;
  setStatus(locked.value ? 'Tapes locked' : 'Tapes unlocked');
}

/** Apply a structural change with undo support. */
export function commit(side: Side, fn: (blocks: Block[]) => { blocks: Block[]; cursor?: number; selected?: Set<number> }) {
  if (locked.value) {
    setStatus('Tape is locked. Unlock it in the status bar to edit.');
    return false;
  }
  const t = tapes[side].value;
  const snap: Snapshot = { blocks: t.blocks, cursor: t.cursor, selected: t.selected };
  const r = fn(t.blocks.slice());
  const cursor = r.cursor ?? Math.min(t.cursor, r.blocks.length - 1);
  patch(side, {
    blocks: r.blocks,
    cursor,
    selected: r.selected ?? new Set([...t.selected].filter((u) => r.blocks.some((b) => b.uid === u))),
    dirty: true,
    undo: [...t.undo.slice(-100), snap],
    redo: [],
    compare: new Map(),
  });
  return true;
}

export function undo(side: Side) {
  const t = tapes[side].value;
  const snap = t.undo[t.undo.length - 1];
  if (!snap) return;
  patch(side, {
    blocks: snap.blocks, cursor: snap.cursor, selected: snap.selected, dirty: snap.blocks !== t.saved,
    undo: t.undo.slice(0, -1), redo: [...t.redo, { blocks: t.blocks, cursor: t.cursor, selected: t.selected }],
  });
}
export function redo(side: Side) {
  const t = tapes[side].value;
  const snap = t.redo[t.redo.length - 1];
  if (!snap) return;
  patch(side, {
    blocks: snap.blocks, cursor: snap.cursor, selected: snap.selected, dirty: snap.blocks !== t.saved,
    redo: t.redo.slice(0, -1), undo: [...t.undo, { blocks: t.blocks, cursor: t.cursor, selected: t.selected }],
  });
}

// ---- selection --------------------------------------------------------

/** Move the cursor. 'single' selects only that block, 'toggle'/'range' extend the selection,
 *  'keep' leaves the selection alone (right-click inside a selection). */
export function setCursor(side: Side, index: number, mode: 'single' | 'toggle' | 'range' | 'keep' = 'single') {
  const t = tapes[side].value;
  active.value = side;
  if (index < 0 || index >= t.blocks.length) {
    patch(side, { cursor: -1, selected: new Set() });
    return;
  }
  const uid = t.blocks[index].uid;
  let selected: Set<number>;
  if (mode === 'single') selected = new Set([uid]);
  else if (mode === 'keep') selected = t.selected;
  else if (mode === 'toggle') {
    selected = new Set(t.selected);
    if (selected.has(uid)) selected.delete(uid);
    else selected.add(uid);
  } else {
    selected = new Set(t.selected);
    const from = t.cursor < 0 ? index : t.cursor;
    const [a, b] = from < index ? [from, index] : [index, from];
    for (let i = a; i <= b; i++) selected.add(t.blocks[i].uid);
  }
  patch(side, { cursor: index, selected });
}

export function selectAll(side: Side) {
  const t = tapes[side].value;
  patch(side, { selected: new Set(t.blocks.map((b) => b.uid)) });
}

export function selectUids(side: Side, uids: number[]) {
  patch(side, { selected: new Set(uids) });
}

/**
 * The set of indices that act as one unit for drag/delete/copy when `index` is the
 * grabbed block: the selection if the block is selected, else the block itself,
 * always expanded to whole collapsed groups.
 */
export function unitIndices(t: TapeState, index: number): number[] {
  const ranges = groupRanges(t.blocks);
  const set = new Set<number>();
  const add = (i: number) => {
    set.add(i);
    const end = ranges.get(i);
    if (end !== undefined && t.collapsed.has(t.blocks[i].uid)) for (let k = i; k <= end; k++) set.add(k);
  };
  if (index >= 0 && t.selected.has(t.blocks[index].uid)) {
    t.blocks.forEach((b, i) => { if (t.selected.has(b.uid)) add(i); });
  } else if (index >= 0) add(index);
  return [...set].sort((a, b) => a - b);
}

export function toggleCollapse(side: Side, uid: number) {
  const t = tapes[side].value;
  const c = new Set(t.collapsed);
  if (c.has(uid)) c.delete(uid);
  else c.add(uid);
  patch(side, { collapsed: c });
}

/** Collapse every group and loop, or expand them all. */
export function collapseAll(side: Side, collapse: boolean) {
  const t = tapes[side].value;
  const set = new Set<number>();
  if (collapse) for (const s of groupRanges(t.blocks).keys()) set.add(t.blocks[s].uid);
  patch(side, { collapsed: set });
}

// ---- block operations ----------------------------------------------------

export function insertBlocks(side: Side, at: number, blocks: Block[]) {
  commit(side, (bl) => {
    const idx = Math.max(0, Math.min(at, bl.length));
    bl.splice(idx, 0, ...blocks);
    return { blocks: bl, cursor: idx, selected: new Set(blocks.map((b) => b.uid)) };
  });
}

export function replaceBlock(side: Side, uid: number, nb: Block) {
  commit(side, (bl) => {
    const i = bl.findIndex((b) => b.uid === uid);
    if (i >= 0) bl[i] = { ...nb, uid } as Block;
    return { blocks: bl, cursor: tapes[side].value.cursor };
  });
}

export function deleteIndices(side: Side, indices: number[]) {
  if (indices.length === 0) return;
  const set = new Set(indices);
  commit(side, (bl) => {
    const out = bl.filter((_, i) => !set.has(i));
    const first = Math.min(...indices);
    return { blocks: out, cursor: Math.min(first, out.length - 1), selected: new Set() };
  });
}

export function deleteUnit(side: Side) {
  const t = tapes[side].value;
  deleteIndices(side, unitIndices(t, t.cursor));
}

export function copyUnit(side: Side) {
  const t = tapes[side].value;
  const idx = unitIndices(t, t.cursor);
  clipboard.value = idx.map((i) => cloneBlock(t.blocks[i]));
  setStatus(`${idx.length} block(s) copied`);
}

export function cutUnit(side: Side) {
  const t = tapes[side].value;
  const idx = unitIndices(t, t.cursor);
  if (idx.length === 0) return;
  clipboard.value = idx.map((i) => cloneBlock(t.blocks[i]));
  deleteIndices(side, idx);
}

export function paste(side: Side, after = true) {
  const t = tapes[side].value;
  if (clipboard.value.length === 0) return;
  const at = t.cursor < 0 ? t.blocks.length : after ? t.cursor + 1 : t.cursor;
  insertBlocks(side, at, clipboard.value.map(cloneBlock));
}

export function duplicateUnit(side: Side) {
  const t = tapes[side].value;
  const idx = unitIndices(t, t.cursor);
  if (idx.length === 0) return;
  insertBlocks(side, idx[idx.length - 1] + 1, idx.map((i) => cloneBlock(t.blocks[i])));
}

/** Move a unit of blocks (from `fromSide`) so they land before index `to` in `toSide`. */
export function moveBlocks(fromSide: Side, indices: number[], toSide: Side, to: number, copy: boolean) {
  const src = tapes[fromSide].value;
  const moving = indices.map((i) => src.blocks[i]);
  if (moving.length === 0) return;
  if (fromSide === toSide && !copy) {
    commit(fromSide, (bl) => {
      const set = new Set(indices);
      const before = bl.slice(0, to).filter((_, i) => !set.has(i));
      const after = bl.slice(to).filter((_, i) => !set.has(i + to));
      const out = [...before, ...moving, ...after];
      return { blocks: out, cursor: before.length, selected: new Set(moving.map((b) => b.uid)) };
    });
    return;
  }
  const clones = moving.map(cloneBlock);
  if (!copy) {
    deleteIndices(fromSide, indices);
  }
  insertBlocks(toSide, to, clones);
  active.value = toSide;
}

export function moveUnit(side: Side, delta: -1 | 1) {
  const t = tapes[side].value;
  const idx = unitIndices(t, t.cursor);
  if (idx.length === 0) return;
  const first = idx[0];
  const last = idx[idx.length - 1];
  if (delta < 0 && first === 0) return;
  if (delta > 0 && last >= t.blocks.length - 1) return;
  // Skip over a whole collapsed group when moving past its start/end.
  const ranges = groupRanges(t.blocks);
  let to: number;
  if (delta < 0) {
    to = first - 1;
    for (const [s, e] of ranges) if (e === first - 1 && t.collapsed.has(t.blocks[s].uid)) to = s;
  } else {
    to = last + 2;
    const r = ranges.get(last + 1);
    if (r !== undefined && t.collapsed.has(t.blocks[last + 1].uid)) to = r + 1;
  }
  moveBlocks(side, idx, side, to, false);
}

/** Wrap the selection (or cursor block) in a group. */
export function groupSelection(side: Side, name: string) {
  const t = tapes[side].value;
  const idx = unitIndices(t, t.cursor);
  if (idx.length === 0) return;
  commit(side, (bl) => {
    const start: Block = { uid: -1, id: 0x21, name } as Block;
    const end: Block = { uid: -1, id: 0x22 } as Block;
    const s = cloneBlock(start);
    const e = cloneBlock(end);
    bl.splice(idx[idx.length - 1] + 1, 0, e);
    bl.splice(idx[0], 0, s);
    return { blocks: bl, cursor: idx[0], selected: new Set([s.uid]) };
  });
}

// ---- compare --------------------------------------------------------------

export function runCompareTapes() {
  const l = tapes[0].value;
  const r = tapes[1].value;
  const res = compareTapes(l.blocks, r.blocks, blockCompare.value, tapeCompare.value);
  patch(0, { compare: new Map(l.blocks.map((b, i) => [b.uid, res.left[i]])) });
  patch(1, { compare: new Map(r.blocks.map((b, i) => [b.uid, res.right[i]])) });
  setStatus(res.identical ? 'Tapes are identical (with the current compare settings)' : 'Tapes differ: differing blocks shown in magenta');
}

export function runFindMatch(side: Side) {
  const t = tapes[side].value;
  if (t.cursor < 0) return;
  const needle = t.blocks[t.cursor];
  let total = 0;
  for (const s of [0, 1] as Side[]) {
    const tt = tapes[s].value;
    const m = findMatches(needle, tt.blocks, blockCompare.value);
    total += m.length;
    patch(s, { compare: new Map(m.map((i) => [tt.blocks[i].uid, 'match'])) });
  }
  setStatus(total ? `${total} matching block(s) shown in green` : 'No matching blocks found');
}

export function clearCompare() {
  patch(0, { compare: new Map() });
  patch(1, { compare: new Map() });
}
