// ZX Spectrum character set to Unicode. The tables are the Rust core in core/
// (spectrum/charset.rs); the hex dump asks per byte, so the app fetches all 256
// characters once and indexes the result instead of crossing per character.
//
// The previous implementation lives on as test/reference/spectrum/charset.ts.
import { charTableCore, CharTable } from '../tzx/core';

const tables = new Map<CharTable, string[]>();

function table(kind: CharTable): string[] {
  let t = tables.get(kind);
  if (!t) {
    t = charTableCore(kind);
    tables.set(kind, t);
  }
  return t;
}

/** Printable form of a single ZX character, for the dump / text views (no tokens expanded). */
export function zxChar(code: number, expandTokens = true): string {
  return table(expandTokens ? 'zx' : 'zxPlain')[code & 0xff];
}

/** Single-cell character for the hex dump ASCII column. */
export function dumpChar(code: number): string {
  return table('dump')[code & 0xff];
}
