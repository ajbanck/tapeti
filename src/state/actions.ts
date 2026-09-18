import { Side, tapes, commit, unitIndices, setStatus, dialog, dataWindow, active, blockNo, selectUids, setCursor, insertBlocks } from './store';
import { downloadBytes, newTape, confirmDiscard } from './files';
import { Program, detectPrograms, programAt } from '../tzx/programs';
import { checkConsistency } from '../tzx/consistency';
import { serializeTzx, requiredVersion } from '../tzx/writer';
import { isDataBlock, isUnknown, deepClone, cloneBlock } from '../tzx/types';
import { playBlocks } from './player';
import { playbackOrder } from '../tzx/audio';

/** "Set selection timings to current" copies all timings (not the pause) to selected data blocks. */
export function setSelectionTimings(side: Side) {
  const t = tapes[side].value;
  const cur = t.blocks[t.cursor];
  if (!cur || !isDataBlock(cur)) {
    setStatus('Current block is not a data block');
    return;
  }
  let n = 0;
  commit(side, (bl) => {
    for (let i = 0; i < bl.length; i++) {
      const b = bl[i];
      if (!t.selected.has(b.uid) || b === cur || isUnknown(b)) continue;
      if (b.id === 0x11 && cur.id === 0x11) {
        bl[i] = { ...b, pilot: cur.pilot, sync1: cur.sync1, sync2: cur.sync2, zero: cur.zero, one: cur.one, pilotLen: cur.pilotLen };
        n++;
      } else if (b.id === 0x11 && cur.id === 0x14) {
        bl[i] = { ...b, zero: cur.zero, one: cur.one };
        n++;
      } else if (b.id === 0x14 && (cur.id === 0x11 || cur.id === 0x14)) {
        bl[i] = { ...b, zero: cur.zero, one: cur.one };
        n++;
      } else if (b.id === 0x19 && cur.id === 0x19) {
        bl[i] = { ...b, npp: cur.npp, npd: cur.npd, pilotSymbols: deepClone(cur.pilotSymbols), dataSymbols: deepClone(cur.dataSymbols) };
        n++;
      } else if (b.id === 0x15 && cur.id === 0x15) {
        bl[i] = { ...b, tstates: cur.tstates };
        n++;
      }
    }
    return { blocks: bl, cursor: t.cursor, selected: t.selected };
  });
  setStatus(`Timings copied to ${n} block(s)`);
}

export function viewData(side: Side, asOne = false) {
  const t = tapes[side].value;
  if (t.cursor < 0) return;
  const idx = asOne ? unitIndices(t, t.cursor) : [t.cursor];
  const uids = idx.map((i) => t.blocks[i].uid).filter((u) => {
    const b = t.blocks.find((x) => x.uid === u)!;
    return isDataBlock(b) || b.id === 0x35 || b.id === 0x18;
  });
  if (uids.length === 0) {
    setStatus('This block has no data to view');
    return;
  }
  active.value = side;
  dataWindow.value = { side, uids };
}

export function playTape(side: Side, fromCursor = false) {
  const t = tapes[side].value;
  if (t.blocks.length === 0) return;
  let order = playbackOrder(t.blocks);
  if (fromCursor && t.cursor >= 0) {
    const at = order.indexOf(t.cursor);
    order = at >= 0 ? order.slice(at) : [t.cursor];
  }
  playBlocks(t.blocks, order, side);
}

export function playSelection(side: Side) {
  const t = tapes[side].value;
  const idx = unitIndices(t, t.cursor);
  if (idx.length === 0) return;
  playBlocks(t.blocks, idx, side);
}

export function openInsertDialog(side: Side) {
  dialog.value = { kind: 'insert', side };
}

export type EmulatorScope = 'tape' | 'cursor' | 'selection';

/** Indices of the blocks "Open in emulator" sends for `scope`. */
export function emulatorIndices(side: Side, scope: EmulatorScope): number[] {
  const t = tapes[side].value;
  if (scope === 'tape') return t.blocks.map((_, i) => i);
  if (t.cursor < 0) return [];
  if (scope === 'cursor') return t.blocks.map((_, i) => i).slice(t.cursor);
  return unitIndices(t, t.cursor);
}

/** Problems the exported part would have on its own, numbered as in the full tape. */
function partProblems(side: Side, idx: number[]): string[] {
  const t = tapes[side].value;
  if (idx.length === t.blocks.length) return [];
  const part = idx.map((i) => t.blocks[i]);
  // Only problems the cut introduces; target numbers differ between the two checks, so drop them.
  const key = (block: number, message: string) => `${block}:${message.replace(/target \d+/, 'target')}`;
  const already = new Set(checkConsistency(t.blocks).map((is) => key(is.block, is.message)));
  const out = checkConsistency(part)
    .filter((is) => is.severity === 'error' && is.block >= 0 && !already.has(key(idx[is.block], is.message)))
    .map((is) => `#${blockNo(idx[is.block])}: ${is.message.replace(/target \d+/, 'target')}`);
  const gaps = idx.some((v, k) => k > 0 && v !== idx[k - 1] + 1);
  if (gaps && part.some((b) => b.id === 0x23 || b.id === 0x26 || b.id === 0x28)) {
    out.push('The selection has gaps, so relative jump/call/select offsets point to different blocks.');
  }
  return out;
}

/** Download the tape, the part from the cursor, or the selection as a TZX to load in an
 *  emulator. The desktop app (desktop/) starts the emulator itself. */
export function openInEmulator(side: Side, scope: EmulatorScope) {
  const t = tapes[side].value;
  const idx = emulatorIndices(side, scope);
  if (idx.length === 0) {
    setStatus(t.blocks.length ? 'No current block' : 'The tape is empty');
    return;
  }
  const problems = partProblems(side, idx);
  const go = () => launchEmulator(side, scope, idx);
  if (problems.length) {
    dialog.value = { kind: 'confirm', title: 'Open in emulator', lines: ['The exported blocks may not load or play correctly:', ...problems, 'Open anyway?'], onOk: go };
  } else go();
}

async function launchEmulator(side: Side, scope: EmulatorScope, idx: number[]) {
  const t = tapes[side].value;
  const blocks = idx.map((i) => t.blocks[i]);
  const bytes = serializeTzx(blocks, requiredVersion(blocks));
  const stem = t.name.replace(/\.(tap|tzx)$/i, '') || 'tape';
  const name = scope === 'tape' ? stem : `${stem}-${scope === 'cursor' ? 'from' : 'sel'}${blockNo(idx[0])}`;
  const r = await downloadBytes(bytes, name + '.tzx');
  if (r) setStatus(`Downloaded ${r.name}: open it in your emulator`);
}

// ---- programs on a collection tape ------------------------------------------

function selectRange(side: Side, p: Program) {
  const t = tapes[side].value;
  selectUids(side, t.blocks.slice(p.start, p.end + 1).map((b) => b.uid));
  setStatus(`${p.name}: blocks #${blockNo(p.start)}–#${blockNo(p.end)} selected`);
}

/** Select the whole program (game) the cursor is in. */
export function selectProgram(side: Side) {
  const t = tapes[side].value;
  const p = programAt(detectPrograms(t.blocks), t.cursor);
  if (!p) return;
  setCursor(side, t.cursor, 'keep');
  selectRange(side, p);
}

/** Move the cursor to a program's first block and select the program (program picker). */
export function jumpToProgram(side: Side, p: Program) {
  setCursor(side, p.start, 'single');
  selectRange(side, p);
}

export function openProgramPicker(side: Side) {
  if (tapes[side].value.blocks.length) dialog.value = { kind: 'programs', side };
}

/** Copy the unit at the cursor (selection or block, whole collapsed groups) into the other
 *  pane as a new tape, named after the program when the unit is exactly one program. */
export function extractToOtherPane(side: Side) {
  const t = tapes[side].value;
  const idx = unitIndices(t, t.cursor);
  if (idx.length === 0) return;
  const other: Side = side === 0 ? 1 : 0;
  const p = programAt(detectPrograms(t.blocks), idx[0]);
  const stem = t.name.replace(/\.(tap|tzx)$/i, '') || 'tape';
  const whole = p && p.start === idx[0] && p.end === idx[idx.length - 1] && idx.length === p.end - p.start + 1;
  const name = (whole ? p.name.replace(/[\\/:*?"<>|]/g, '_') : `${stem}-sel${blockNo(idx[0])}`) + '.tzx';
  const go = () => {
    newTape(other);
    tapes[other].value = { ...tapes[other].value, name };
    insertBlocks(other, 0, idx.map((i) => cloneBlock(t.blocks[i])));
    setStatus(`Extracted ${idx.length} block(s) to the ${other === 0 ? 'left' : 'right'} pane as ${name}`);
  };
  const problems = partProblems(side, idx);
  const start = () => (problems.length
    ? (dialog.value = { kind: 'confirm', title: 'Extract to other pane', lines: ['The extracted blocks may not load or play correctly on their own:', ...problems, 'Extract anyway?'], onOk: go })
    : go());
  confirmDiscard(other, start);
}
