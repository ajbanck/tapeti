// The BASIC lister and the variables area. The listing is the Rust core in
// core/ (spectrum/basic.rs), including the JavaScript number formatting the
// view has always shown.
//
// The previous implementation lives on as test/reference/spectrum/basic.ts.
import { BasicLine, BasicOptions, BasicToken, VariableEntry } from '../tzx/types';
import { basicSourceCore, editBasicCore, basicToTextCore, decodeNumberCore, formatNumberCore, listBasicCore, listVariablesCore } from '../tzx/core';

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

export interface SourceOptions {
  basic128?: boolean;
  /** Take `print` for `PRINT`. */
  anyCase?: boolean;
}

export interface SourceError {
  /** Line of the text, from 1; 0 for the program as a whole. */
  line: number;
  message: string;
}

/**
 * A program area as text that can be edited, saved and tokenised again: keywords
 * spelled out and {...} for what a keyboard cannot type. core/src/spectrum/source.rs
 * has the format.
 */
export function basicSource(data: Uint8Array, start: number, end: number, opts: SourceOptions = {}): string {
  return basicSourceCore(data, start, end, opts);
}

/**
 * The program area `text` stands for, given the one it was made from. Lines still
 * as basicSource wrote them keep their bytes; only the others are tokenised.
 */
export function editBasic(data: Uint8Array, start: number, end: number, text: string, opts: SourceOptions = {}): { program: Uint8Array } | { errors: SourceError[] } {
  return editBasicCore(data, start, end, text, opts);
}
