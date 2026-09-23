// Tape structure: group/loop ranges and the programs (games) a collection tape
// holds. The logic is the Rust core in core/ (programs.rs); this module keeps
// the signature the UI imports.
//
// test/reference/programs.ts is the frozen implementation the differential
// tests compare this against.
import { Block, Program } from './types';
import { detectProgramsCore, groupRangesCore, tapeTitleCore } from './core';
import { perTape } from './cache';

export type { Program };

/**
 * Pairs of start/end indices for groups and loops (nested ones included).
 * Cached per tape: the menus and the store ask for this on every render.
 */
export const groupRanges = perTape(groupRangesCore);

/** Title from an Archive info block, if the tape has one. */
export const tapeTitle = perTape(tapeTitleCore);

/** Split a tape into programs; see `core/src/programs.rs` for what decides a boundary. */
export const detectPrograms = perTape(detectProgramsCore);

/** The program containing block `index`, if any. */
export function programAt(programs: Program[], index: number): Program | undefined {
  return programs.find((p) => index >= p.start && index <= p.end);
}
