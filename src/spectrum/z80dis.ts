// Z80 disassembly. The decoder is the Rust core in core/ (spectrum/z80dis.rs).
//
// The previous implementation lives on as test/reference/spectrum/z80dis.ts.
import { DisLine, DisOptions } from '../tzx/types';
import { disassembleCore } from '../tzx/core';

export type { DisLine, DisOptions };

/** Disassemble `count` instructions from `data` starting at byte `offset`, with address base `base`. */
export function disassemble(data: Uint8Array, offset: number, base: number, count: number, opts: DisOptions = {}): DisLine[] {
  return disassembleCore(data, offset, base, count, opts);
}
