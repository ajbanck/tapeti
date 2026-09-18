// The TypeScript programs.ts as it stood before the Rust core replaced it, kept as
// the reference implementation the core is tested against (test/core.test.ts).
// Not part of the app bundle, and not to gain features. See test/reference/parser.ts.
// Tape structure: group/loop ranges and the programs (games) a collection tape holds.
import { Block, isUnknown, isDataBlock, Program } from '../../src/tzx/types';
import { decodeHeader } from './describe';

/** Pairs of start/end indices for groups and loops (nested ones included). */
export function groupRanges(blocks: Block[]): Map<number, number> {
  const ranges = new Map<number, number>();
  const stack: { id: number; at: number }[] = [];
  blocks.forEach((b, i) => {
    if (b.id === 0x21 || b.id === 0x24) stack.push({ id: b.id, at: i });
    else if (b.id === 0x22 || b.id === 0x25) {
      const want = b.id === 0x22 ? 0x21 : 0x24;
      for (let k = stack.length - 1; k >= 0; k--) {
        if (stack[k].id === want) {
          ranges.set(stack[k].at, i);
          stack.splice(k);
          break;
        }
      }
    }
  });
  return ranges;
}

/** Blocks that introduce the program that follows them (text, tones, loader loops), as
 *  opposed to pauses and stops, which close the program before them. */
const LEADING = new Set([0x12, 0x13, 0x24, 0x25, 0x2b, 0x30, 0x31, 0x32, 0x33, 0x35, 0x5a]);

function programHeader(b: Block) {
  if (isUnknown(b) || !isDataBlock(b) || b.id === 0x14 || b.id === 0x15) return null;
  const h = decodeHeader(b.data);
  return h && h.type === 0 ? h : null;
}

function hasProgramHeader(blocks: Block[], from: number, to: number): boolean {
  for (let i = from; i <= to; i++) if (programHeader(blocks[i])) return true;
  return false;
}

/** Title from an Archive info block, if the tape has one. */
export function tapeTitle(blocks: Block[]): string | null {
  for (const b of blocks) {
    if (b.id === 0x32 && !isUnknown(b)) {
      const t = b.entries.find((e) => e.type === 0)?.text.trim();
      if (t) return t;
    }
  }
  return null;
}

/**
 * Split a tape into programs. A program starts at every BASIC Program header (the loader of a
 * game), at every top-level group that contains one (grouped collections keep their names),
 * and at every Select block target. Anything else, including headerless custom loaders and
 * "stop the tape" blocks between the parts of a multi-load game, stays with the program it
 * follows. Metadata right before a boundary (text, tones, loader loops) belongs to the program
 * it introduces. A tape without any boundary is one program named after its archive info.
 * The result always covers every block.
 */
export function detectPrograms(blocks: Block[]): Program[] {
  const n = blocks.length;
  if (n === 0) return [];
  const starts = new Map<number, { name: string; source: Program['source'] }>();
  const ranges = groupRanges(blocks);
  for (let i = 0; i < n; ) {
    const b = blocks[i];
    if (b.id === 0x21 && !isUnknown(b)) {
      const end = ranges.get(i);
      if (end !== undefined) {
        if (hasProgramHeader(blocks, i + 1, end - 1)) starts.set(i, { name: b.name.trim() || 'Group', source: 'group' });
        i = end + 1;
        continue;
      }
    }
    const h = programHeader(b);
    if (h) starts.set(i, { name: h.name.trim() || 'Untitled', source: 'header' });
    i++;
  }
  blocks.forEach((b, i) => {
    if (b.id !== 0x28 || isUnknown(b)) return;
    for (const e of b.entries) {
      const t = i + e.offset;
      if (t > 0 && t < n) starts.set(t, { name: e.text.trim() || `Block ${t + 1}`, source: 'select' });
    }
  });
  if (starts.size === 0) return [{ name: tapeTitle(blocks) ?? 'Untitled', start: 0, end: n - 1, source: 'tape' }];

  const sorted = [...starts.keys()].sort((a, b) => a - b);
  const programs: Program[] = sorted.map((at, k) => {
    const { name, source } = starts.get(at)!;
    let start = at;
    if (k === 0) start = 0;
    else if (source !== 'select') {
      const floor = sorted[k - 1] + 1;
      while (start > floor && LEADING.has(blocks[start - 1].id) && !isUnknown(blocks[start - 1])) start--;
    }
    return { name, start, end: n - 1, source };
  });
  for (let k = 0; k + 1 < programs.length; k++) programs[k].end = programs[k + 1].start - 1;
  return programs;
}

/** The program containing block `index`, if any. */
export function programAt(programs: Program[], index: number): Program | undefined {
  return programs.find((p) => index >= p.start && index <= p.end);
}
