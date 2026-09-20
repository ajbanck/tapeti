// Bit-stream helpers for the data window's Drop / Add / Shift operations. The
// operations are the Rust core in core/ (bits.rs).
//
// The previous implementation lives on as test/reference/bits.ts.
import { BitData } from './types';
import { bitsCore, cryptCore, flipBytesCore } from './core';

export type { BitData };

/**
 * How many bits the stream holds.
 *
 * Stays in TypeScript: arithmetic on two numbers the caller already has, which
 * the data window asks for on every render. `bits.rs` has the same function for
 * the core's own use, and both sides assert the same table.
 */
export function totalBits(d: BitData): number {
  if (d.data.length === 0) return 0;
  return (d.data.length - 1) * 8 + Math.max(1, Math.min(8, d.usedBits));
}

export function dropBits(d: BitData, n: number): BitData {
  return bitsCore('drop', [d], n);
}

export function addBits(d: BitData, n: number): BitData {
  return bitsCore('add', [d], n);
}

export function shiftLeftBits(d: BitData, n: number): BitData {
  return bitsCore('shiftLeft', [d], n);
}

export function shiftRightBits(d: BitData, n: number): BitData {
  return bitsCore('shiftRight', [d], n);
}

/** Concatenate several bit streams (used by "view selected as one"). */
export function joinBits(parts: BitData[]): BitData {
  return bitsCore('join', parts);
}

/** Reverse the bits of every byte. */
export function flipBytes(d: Uint8Array): Uint8Array {
  return flipBytesCore(d);
}

/**
 * The usual turbo loader encryption, SpeedLock's among them: what ends up in
 * memory is `(loaded byte XOR x) + y` (LD A,x: XOR L: ADD A,y: LD (IX+0),A).
 */
export function decryptBytes(d: Uint8Array, xor: number, add: number): Uint8Array {
  return cryptCore(d, xor, add, false);
}

/** The other way: the bytes a tape must hold for the loader to store these. */
export function encryptBytes(d: Uint8Array, xor: number, add: number): Uint8Array {
  return cryptCore(d, xor, add, true);
}

/** The values the SpeedLock versions use unless a game changes them. `CRYPT_PRESETS` in the core. */
export const CRYPT_PRESETS = [
  { name: 'SpeedLock 2/3', xor: 0x98, add: 0x0b },
  { name: 'SpeedLock 4-7', xor: 0xc1, add: 0x11 },
];
