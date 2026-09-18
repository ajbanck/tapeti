// TZX and TAP writing. The implementation is the Rust core in core/ (writer.rs);
// this module is the signature the rest of the app has always used, and it
// encodes the blocks onto the wire for each call — see src/tzx/core.ts.
//
// The previous TypeScript implementation lives on as test/reference/writer.ts,
// which test/core.test.ts checks the core against.
import { Block } from './types';
import {
  requiredVersionCore,
  saveVersionCore,
  serializeBlocksCore,
  serializeTapCore,
  serializeTzxCore,
} from './core';

/** Lowest TZX version able to represent these blocks. */
export function requiredVersion(blocks: Block[]): { major: number; minor: number } {
  return requiredVersionCore(blocks);
}

/**
 * Version to write in the header: the lowest one the blocks need, but never below the
 * version the tape was loaded with, so an unaltered file round-trips byte for byte.
 */
export function saveVersion(blocks: Block[], loaded: { major: number; minor: number } | null): { major: number; minor: number } {
  return saveVersionCore(blocks, loaded);
}

export function serializeTzx(blocks: Block[], version?: { major: number; minor: number }): Uint8Array {
  return serializeTzxCore(blocks, version);
}

/** Serialize a single block to bytes (ID + body); used for comparison and size display. */
export function serializeBlock(b: Block): Uint8Array {
  return serializeBlocksCore([b]);
}

/** TAP export: only standard-speed blocks can be represented. Returns skipped block indices. */
export function serializeTap(blocks: Block[]): { bytes: Uint8Array; skipped: number[] } {
  return serializeTapCore(blocks);
}
