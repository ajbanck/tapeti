// Tape parsing. The implementation is the Rust core in core/, compiled to wasm
// and loaded by core.ts; this module is the signature the rest of the app has
// always used. `initCore()` must have resolved before any of these are called —
// src/main.tsx awaits it before the first render, tests in test/setup.ts.
//
// The previous TypeScript implementation lives on as test/reference/parser.ts,
// which test/core.test.ts checks the core against.
import { ParsedTape } from './types';
import { parseTapCore, parseTapeCore, parseTzxCore } from './core';

export type { ParsedTape };

const TZX_SIGNATURE = 'ZXTape!\x1a';

export function isTzx(buf: Uint8Array): boolean {
  if (buf.length < 10) return false;
  for (let i = 0; i < 8; i++) if (buf[i] !== TZX_SIGNATURE.charCodeAt(i)) return false;
  return true;
}

export function parseTzx(buf: Uint8Array): ParsedTape {
  return parseTzxCore(buf);
}

/** Parse a TAP file into standard speed blocks. */
export function parseTap(buf: Uint8Array): ParsedTape {
  return parseTapCore(buf);
}

/** Auto-detect TZX vs TAP by signature. */
export function parseTape(buf: Uint8Array): ParsedTape {
  return parseTapeCore(buf);
}

/**
 * Bits per symbol of an alphabet of `alphabetSize` symbols. Still TypeScript
 * because the writer, the audio renderer and the consistency check use it; it
 * moves to the core with them in stage 2.
 */
export function bitsPerSymbol(alphabetSize: number): number {
  if (alphabetSize <= 1) return 1;
  return Math.ceil(Math.log2(alphabetSize));
}
