// The BASIC lister and the variables area. The listing is the Rust core in
// core/ (spectrum/basic.rs), including the JavaScript number formatting the
// view has always shown.
//
// The previous implementation lives on as test/reference/spectrum/basic.ts.
import { BasicLine, BasicOptions, BasicToken, VariableEntry } from '../tzx/types';
import { basicToTextCore, decodeNumberCore, formatNumberCore, listBasicCore, listVariablesCore } from '../tzx/core';

export type { BasicToken, BasicLine, BasicOptions, VariableEntry };

/** Decode a 5-byte Sinclair floating point number. */
export function decodeNumber(d: Uint8Array, o: number): number {
  return decodeNumberCore(d, o);
}

export function formatNumber(v: number): string {
  return formatNumberCore(v);
}

/** List a BASIC program area. `data` is the raw bytes starting at PROG. */
export function listBasic(data: Uint8Array, start: number, end: number, opts: BasicOptions): BasicLine[] {
  return listBasicCore(data, start, end, opts);
}

/** Render a listing as plain text. */
export function basicToText(lines: BasicLine[], opts: BasicOptions): string {
  return basicToTextCore(lines, opts);
}

/** List the variables area (starting at VARS) until the 0x80 end marker. */
export function listVariables(data: Uint8Array, start: number, end: number): VariableEntry[] {
  return listVariablesCore(data, start, end);
}
