// Comparing blocks and tapes. The comparison is the Rust core in core/
// (compare.rs); this module keeps the signature the store imports.
//
// test/reference/compare.ts is the frozen implementation the differential
// tests compare this against.
import { Block, BlockCompareMode, CompareResult, TapeCompareMode } from './types';
import { blocksEqualCore, compareTapesCore, findMatchesCore } from './core';

export type { BlockCompareMode, TapeCompareMode, CompareResult };

/** Compare two blocks according to the block-compare setting. */
export function blocksEqual(a: Block, b: Block, mode: BlockCompareMode): boolean {
  return blocksEqualCore(a, b, mode);
}

/** Compare two tapes block by block, returning a result per block of each tape. */
export function compareTapes(
  left: Block[], right: Block[], blockMode: BlockCompareMode, tapeMode: TapeCompareMode,
): { left: CompareResult[]; right: CompareResult[]; identical: boolean } {
  return compareTapesCore(left, right, blockMode, tapeMode);
}

/** Find all blocks in `haystack` matching `needle`. */
export function findMatches(needle: Block, haystack: Block[], mode: BlockCompareMode): number[] {
  // The core compares by index, not object identity, so pass where `needle`
  // sits in `haystack` (-1 if it is not there).
  return findMatchesCore(needle, haystack, mode, haystack.indexOf(needle));
}
