// The TypeScript compare.ts as it stood before the Rust core replaced it, kept as
// the reference implementation the core is tested against (test/core.test.ts).
// Not part of the app bundle, and not to gain features. See test/reference/parser.ts.
import { Block, BlockCompareMode, CompareResult, isUnknown, isDataBlock, TapeCompareMode } from '../../src/tzx/types';
import { bytesEqual } from '../../src/tzx/bytes';
import { serializeBlock } from './writer';
import { isMetadata } from './describe';


function stripPause(b: Block): Block {
  if ('pause' in b) return { ...(b as any), pause: 0 };
  return b;
}

/** Compare two blocks according to the block-compare setting. */
export function blocksEqual(a: Block, b: Block, mode: BlockCompareMode): boolean {
  if (mode === 'data' && isDataBlock(a) && isDataBlock(b)) {
    return bytesEqual(a.data, b.data);
  }
  if (isUnknown(a) || isUnknown(b)) {
    return a.id === b.id && isUnknown(a) && isUnknown(b) && bytesEqual(a.raw, b.raw);
  }
  if (a.id !== b.id) return false;
  const x = mode === 'data+timings+pauses' ? a : stripPause(a);
  const y = mode === 'data+timings+pauses' ? b : stripPause(b);
  return bytesEqual(serializeBlock(x), serializeBlock(y));
}

function consideredInTapeCompare(b: Block, mode: TapeCompareMode): boolean {
  switch (mode) {
    case 'datablocks':
      return isDataBlock(b);
    case 'ignore-metadata':
      return !isMetadata(b);
    case 'all':
      return true;
  }
}

/** Compare two tapes block by block, returning a result per block of each tape. */
export function compareTapes(
  left: Block[], right: Block[], blockMode: BlockCompareMode, tapeMode: TapeCompareMode,
): { left: CompareResult[]; right: CompareResult[]; identical: boolean } {
  const li = left.map((b, i) => i).filter((i) => consideredInTapeCompare(left[i], tapeMode));
  const ri = right.map((b, i) => i).filter((i) => consideredInTapeCompare(right[i], tapeMode));
  const lres: CompareResult[] = left.map(() => 'ignored');
  const rres: CompareResult[] = right.map(() => 'ignored');
  const n = Math.max(li.length, ri.length);
  let identical = li.length === ri.length;
  for (let k = 0; k < n; k++) {
    const a = li[k];
    const b = ri[k];
    if (a === undefined) {
      rres[b] = 'diff';
      identical = false;
      continue;
    }
    if (b === undefined) {
      lres[a] = 'diff';
      identical = false;
      continue;
    }
    const eq = blocksEqual(left[a], right[b], blockMode);
    lres[a] = eq ? 'same' : 'diff';
    rres[b] = eq ? 'same' : 'diff';
    if (!eq) identical = false;
  }
  return { left: lres, right: rres, identical };
}

/** Find all blocks in `haystack` matching `needle`. */
export function findMatches(needle: Block, haystack: Block[], mode: BlockCompareMode): number[] {
  const out: number[] = [];
  haystack.forEach((b, i) => {
    if (b !== needle && blocksEqual(needle, b, mode)) out.push(i);
  });
  return out;
}
