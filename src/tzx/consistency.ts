// "Check consistency": structure, useless blocks, infinite loops, cross nesting.
// The checks are the Rust core in core/ (consistency.rs); this module keeps the
// signature the UI imports.
//
// test/reference/consistency.ts is the frozen implementation the differential
// tests compare this against.
import { Block, Issue } from './types';
import { checkConsistencyCore } from './core';
import { perTapeWith } from './cache';

export type { Issue };

const checked = perTapeWith(checkConsistencyCore);

/** `base` is the number shown for the first block (1, or 0 with zero-based numbering). */
export function checkConsistency(blocks: Block[], base = 1): Issue[] {
  return checked(blocks, base);
}
