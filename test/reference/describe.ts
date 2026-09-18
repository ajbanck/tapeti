// The TypeScript describe.ts as it stood before the Rust core replaced it, kept as
// the reference implementation the core is tested against (test/core.test.ts).
// Not part of the app bundle, and not to gain features. See test/reference/parser.ts.
import { Block, BLOCK_NAMES, isUnknown, DataBlock, HeaderInfo } from '../../src/tzx/types';
import { bytesToLatin1 } from '../../src/tzx/bytes';
import { serializeBlock } from './writer';

export const HEADER_TYPE_NAMES = ['Program', 'Number array', 'Character array', 'Bytes'];

/** Decode a standard ROM header if this block looks like one. */
export function decodeHeader(data: Uint8Array): HeaderInfo | null {
  if (data.length !== 19 || data[0] !== 0x00) return null;
  const type = data[1];
  if (type > 3) return null;
  return {
    type,
    typeName: HEADER_TYPE_NAMES[type],
    name: bytesToLatin1(data.subarray(2, 12)),
    length: data[12] | (data[13] << 8),
    param1: data[14] | (data[15] << 8),
    param2: data[16] | (data[17] << 8),
  };
}

export function encodeHeader(h: HeaderInfo): Uint8Array {
  const d = new Uint8Array(19);
  d[0] = 0;
  d[1] = h.type;
  const name = (h.name + '          ').slice(0, 10);
  for (let i = 0; i < 10; i++) d[2 + i] = name.charCodeAt(i) & 0xff;
  d[12] = h.length & 0xff;
  d[13] = (h.length >> 8) & 0xff;
  d[14] = h.param1 & 0xff;
  d[15] = (h.param1 >> 8) & 0xff;
  d[16] = h.param2 & 0xff;
  d[17] = (h.param2 >> 8) & 0xff;
  d[18] = checksum(d.subarray(0, 18));
  return d;
}

export function checksum(data: Uint8Array, start = 0, end = data.length): number {
  let c = 0;
  for (let i = start; i < end; i++) c ^= data[i];
  return c;
}

export function fmt(n: number, hex: boolean, pad = 0): string {
  if (hex) return n.toString(16).toUpperCase().padStart(pad, '0');
  return n.toString(10);
}

function headerDesc(prefix: string, h: HeaderInfo, hex: boolean): string {
  const name = h.name.replace(/\s+$/, '');
  switch (h.type) {
    case 0: {
      const auto = h.param1 >= 32768 ? 'none' : fmt(h.param1, hex);
      return `${prefix} Prog: ${name}; L: ${fmt(h.length, hex)}, P: ${fmt(h.param2, hex)}, S: ${auto}`;
    }
    case 3:
      return `${prefix} Bytes: ${name}; S: ${fmt(h.param1, hex)}, L: ${fmt(h.length, hex)}`;
    case 1:
      return `${prefix} Num: ${name}; L: ${fmt(h.length, hex)}, ${arrayVarName(h.param1)}`;
    case 2:
      return `${prefix} Char: ${name}; L: ${fmt(h.length, hex)}, ${arrayVarName(h.param1)}`;
  }
  return prefix;
}

function arrayVarName(p: number): string {
  const letter = String.fromCharCode(((p >> 8) & 0x1f) + 0x60);
  return `var ${letter}$`.replace('$', (p >> 8) & 0x40 ? '$' : '');
}

function speedPrefix(b: Block): string {
  switch (b.id) {
    case 0x10:
      return 'Std speed';
    case 0x11:
      return 'Turbo speed';
    case 0x14:
      return 'Pure data';
    case 0x19:
      return 'Generalized';
    default:
      return '';
  }
}

/** One-line description used in the block list. */
export function describeBlock(b: Block, hex: boolean): string {
  if (isUnknown(b)) return `${BLOCK_NAMES[b.id] ?? 'Unknown block'} (ID ${b.id.toString(16).toUpperCase().padStart(2, '0')})`;
  switch (b.id) {
    case 0x10:
    case 0x11:
    case 0x14: {
      const h = decodeHeader(b.data);
      if (h) return headerDesc(speedPrefix(b), h, hex);
      if (b.id === 0x10) return 'Standard speed data';
      if (b.id === 0x11) return 'Turbo loading data';
      return 'Pure data';
    }
    case 0x19: {
      const h = decodeHeader(b.data);
      if (h) return headerDesc('Generalized', h, hex);
      return 'Generalized data';
    }
    case 0x12:
      return `Pure tone ${fmt(b.pulseLen, hex)} x ${fmt(b.count, hex)}`;
    case 0x13:
      return `Pulse sequence (${fmt(b.pulses.length, hex)})`;
    case 0x15:
      return `Direct recording ${fmt(b.tstates, hex)} T/sample`;
    case 0x18:
      return `CSW recording ${fmt(b.sampleRate, hex)} Hz`;
    case 0x20:
      return b.pause === 0 ? "Stop the tape" : `Pause ${fmt(b.pause, hex)} ms`;
    case 0x21:
      return `Group ${b.name}`;
    case 0x22:
      return 'Group end';
    case 0x23:
      return `Jump ${b.offset >= 0 ? '+' : ''}${fmt(Math.abs(b.offset), hex).replace(/^/, b.offset < 0 ? '-' : '')}`;
    case 0x24:
      return `Loop ${fmt(b.count, hex)} times`;
    case 0x25:
      return 'Loop end';
    case 0x26:
      return `Call sequence (${fmt(b.offsets.length, hex)})`;
    case 0x27:
      return 'Return from sequence';
    case 0x28:
      return `Select block (${fmt(b.entries.length, hex)})`;
    case 0x2a:
      return 'Stop the tape if in 48K mode';
    case 0x2b:
      return `Set signal level ${b.level ? 'high' : 'low'}`;
    case 0x30:
      return `Text: ${b.text.split(/\r?\n/)[0]}`;
    case 0x31:
      return `Message: ${b.text.split(/\r?\n/)[0]}`;
    case 0x32:
      return 'Archive info';
    case 0x33:
      return 'Hardware type';
    case 0x35:
      return `Custom info ${b.ident.trim()}`;
    case 0x5a:
      return 'Glue block';
  }
  return BLOCK_NAMES[(b as Block).id] ?? 'Unknown';
}

/** Right-hand column in the block list: data length for data blocks, else serialized size. */
export function blockLength(b: Block): number {
  if (isUnknown(b)) return b.raw.length;
  switch (b.id) {
    case 0x10:
    case 0x11:
    case 0x14:
    case 0x15:
    case 0x18:
    case 0x19:
    case 0x35:
      return b.data.length;
    case 0x12:
      return b.count;
    case 0x13:
      return b.pulses.length;
    default:
      return serializeBlock(b).length - 1;
  }
}

/** Metadata blocks are not part of the tape signal. */
export function isMetadata(b: Block): boolean {
  return b.id === 0x21 || b.id === 0x22 || b.id === 0x30 || b.id === 0x31 || b.id === 0x32 || b.id === 0x33 || b.id === 0x35 || b.id === 0x5a || (isUnknown(b) && b.id !== 0x16 && b.id !== 0x17);
}

/** Byte payload of a data block. */
export function payload(b: DataBlock): Uint8Array {
  return b.data;
}
