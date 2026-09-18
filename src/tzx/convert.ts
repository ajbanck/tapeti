// Changing a block's type while keeping whatever fields carry over (block editor
// type menu). The field matrix is the Rust core in core/ (convert.rs).
//
// The previous implementation lives on as test/reference/convert.ts.
import { Block, isUnknown } from './types';
import { convertBlockCore } from './core';

/**
 * Convert `b` to block type `id`, keeping its uid and every field the new type shares with
 * the old one (pause, data, used bits, timings, text). Unknown blocks are returned unchanged.
 */
export function convertBlock(b: Block, id: number): Block {
  // Nothing to do, and the caller expects the same object back.
  if (isUnknown(b) || b.id === id) return b;
  const converted = convertBlockCore(b, id);
  // The core carries the data across unchanged, so keep the array the caller
  // already had instead of the copy that came back over the wire.
  if ('data' in converted && 'data' in b) (converted as { data: Uint8Array }).data = b.data;
  return converted;
}
