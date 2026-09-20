// Z80 disassembly. The decoder is the Rust core in core/ (spectrum/z80dis.rs).
//
// The previous implementation lives on as test/reference/spectrum/z80dis.ts.
import { DisLine, DisOptions } from '../tzx/types';
import { checkSymbolsCore, disassembleCore, disassemblyTextCore } from '../tzx/core';

export type { DisLine, DisOptions };

/** Disassemble `count` instructions from `data` starting at byte `offset`, with address base `base`. */
export function disassemble(data: Uint8Array, offset: number, base: number, count: number, opts: DisOptions = {}): DisLine[] {
  return disassembleCore(data, offset, base, count, opts);
}

/** The same as text, for saving: address, bytes, instruction. */
export function disassemblyText(data: Uint8Array, offset: number, base: number, count: number, opts: DisOptions = {}): string {
  return disassemblyTextCore(data, offset, base, count, opts);
}

/** The lines (from 1) of a symbol table that are not "address name". */
export function checkSymbols(text: string): number[] {
  return checkSymbolsCore(text);
}
