// Block descriptions and ROM headers. The logic is the Rust core in core/
// (describe.rs); this module is the signature the UI has always used.
//
// The list asks for a description and a length per row, so both are cached per
// block object: blocks are immutable, an edit makes a new one, and a WeakMap
// keyed on the object cannot go stale. Without it every render would send the
// tape across the wasm boundary again.
//
// The previous implementation lives on as test/reference/describe.ts.
import { Block, DataBlock, HeaderInfo, isUnknown } from './types';
import { checksumCore, decodeHeaderCore, describeBlockCore, encodeHeaderCore } from './core';

export type { HeaderInfo };

export const HEADER_TYPE_NAMES = ['Program', 'Number array', 'Character array', 'Bytes'];

/** Decode a standard ROM header if this block looks like one. */
export function decodeHeader(data: Uint8Array): HeaderInfo | null {
  return decodeHeaderCore(data);
}

export function encodeHeader(h: HeaderInfo): Uint8Array {
  return encodeHeaderCore(h);
}

export function checksum(data: Uint8Array, start = 0, end = data.length): number {
  return checksumCore(data.subarray(start, end));
}

/** Both spellings of the description are kept: the Dec/Hex switch flips between them. */
interface Described {
  dec?: string;
  hex?: string;
  length: number;
}

const described = new WeakMap<Block, Described>();

function describe(b: Block, hex: boolean): Described {
  const hit = described.get(b);
  if (hit && (hex ? hit.hex : hit.dec) !== undefined) return hit;
  const answer = describeBlockCore(b, hex);
  const entry: Described = hit ?? { length: answer.length };
  if (hex) entry.hex = answer.description;
  else entry.dec = answer.description;
  described.set(b, entry);
  return entry;
}

/** One-line description used in the block list. */
export function describeBlock(b: Block, hex: boolean): string {
  const entry = describe(b, hex);
  return (hex ? entry.hex : entry.dec) ?? '';
}

/** Right-hand column in the block list: data length for data blocks, else serialized size. */
export function blockLength(b: Block): number {
  const hit = described.get(b);
  return hit ? hit.length : describe(b, false).length;
}

/**
 * Metadata blocks are not part of the tape signal.
 *
 * One of the few pieces of logic kept in TypeScript: it is a classification of
 * the block model, like `isDataBlock`, and the list asks it per row. The core
 * has its own copy in `describe.rs`, and a differential test holds the two
 * together for every block ID.
 */
export function isMetadata(b: Block): boolean {
  return b.id === 0x21 || b.id === 0x22 || b.id === 0x30 || b.id === 0x31 || b.id === 0x32 || b.id === 0x33 || b.id === 0x35 || b.id === 0x5a || (isUnknown(b) && b.id !== 0x16 && b.id !== 0x17);
}

/** Byte payload of a data block. */
export function payload(b: DataBlock): Uint8Array {
  return b.data;
}
