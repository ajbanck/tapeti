// What a data block contains: a ROM header, a BASIC program, a screen, machine
// code, an array or plain data. The detection is the Rust core in core/
// (content.rs); this module is the signature the UI has always used.
//
// The previous implementation lives on as test/reference/content.ts.
import { Block, ContentInfo, ContentKind } from './types';
import { basicScoreCore, contentLabelsCore, detectContentCore } from './core';
import { perTape } from './cache';

export type { ContentInfo, ContentKind };

/**
 * Body of a data block after removing flag/checksum bytes.
 *
 * Stays in TypeScript: it is a view of bytes the editor already holds, not
 * logic, and crossing into wasm to drop two bytes would cost more than it
 * saves. `content.rs` has the same slice for the core's own use, and a
 * differential test holds the two together.
 */
export function blockBody(data: Uint8Array, skipFlag: boolean, skipChecksum: boolean): Uint8Array {
  let d = data;
  if (skipFlag && d.length > 0) d = d.subarray(1);
  if (skipChecksum && d.length > 0) d = d.subarray(0, d.length - 1);
  return d;
}

/** Does the byte stream look like a BASIC program area? Returns the fraction of bytes that parse as lines. */
export function basicScore(d: Uint8Array): number {
  return basicScoreCore(d);
}

/** Analyse block `index`; the previous block is consulted for a ROM header. */
export function detectContent(blocks: Block[], index: number): ContentInfo {
  return detectContentCore(blocks, index);
}

/** The list label of every block, in one call: the block list asks for all of them at once. */
export const contentLabels = perTape(contentLabelsCore);
