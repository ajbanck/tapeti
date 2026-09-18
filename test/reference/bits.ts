// The TypeScript bits.ts as it stood before the Rust core replaced it, kept as
// the reference implementation the core is tested against (test/core.test.ts).
// Not part of the app bundle, and not to gain features. See test/reference/parser.ts.
import { BitData } from '../../src/tzx/types';
// Bit-stream helpers for the data window's Drop / Add / Shift operations.

export function totalBits(d: BitData): number {
  if (d.data.length === 0) return 0;
  return (d.data.length - 1) * 8 + Math.max(1, Math.min(8, d.usedBits));
}

export function getBit(d: Uint8Array, i: number): number {
  return (d[i >> 3] >> (7 - (i & 7))) & 1;
}

/** Build a byte array from a bit provider. */
export function fromBits(n: number, bit: (i: number) => number): BitData {
  const len = Math.ceil(n / 8);
  const out = new Uint8Array(len);
  for (let i = 0; i < n; i++) if (bit(i)) out[i >> 3] |= 0x80 >> (i & 7);
  const used = n === 0 ? 8 : n - (len - 1) * 8;
  return { data: out, usedBits: used };
}

export function dropBits(d: BitData, n: number): BitData {
  const total = Math.max(0, totalBits(d) - n);
  return fromBits(total, (i) => getBit(d.data, i));
}
export function addBits(d: BitData, n: number): BitData {
  const before = totalBits(d);
  return fromBits(before + n, (i) => (i < before ? getBit(d.data, i) : 0));
}
export function shiftLeftBits(d: BitData, n: number): BitData {
  const total = Math.max(0, totalBits(d) - n);
  return fromBits(total, (i) => getBit(d.data, i + n));
}
export function shiftRightBits(d: BitData, n: number): BitData {
  const before = totalBits(d);
  return fromBits(before + n, (i) => (i < n ? 0 : getBit(d.data, i - n)));
}

/** Concatenate several bit streams (used by "view selected as one"). */
export function joinBits(parts: BitData[]): BitData {
  const lens = parts.map(totalBits);
  const total = lens.reduce((a, b) => a + b, 0);
  return fromBits(total, (i) => {
    let k = 0;
    while (k < parts.length && i >= lens[k]) {
      i -= lens[k];
      k++;
    }
    return getBit(parts[k].data, i);
  });
}

export function flipBytes(d: Uint8Array): Uint8Array {
  const out = new Uint8Array(d.length);
  for (let i = 0; i < d.length; i++) {
    let v = d[i];
    v = ((v & 0xf0) >> 4) | ((v & 0x0f) << 4);
    v = ((v & 0xcc) >> 2) | ((v & 0x33) << 2);
    v = ((v & 0xaa) >> 1) | ((v & 0x55) << 1);
    out[i] = v;
  }
  return out;
}
