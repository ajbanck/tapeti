// Differential test from stage 1 of the Rust migration: the wasm core and the
// TypeScript parser it replaced must return the same thing for the same bytes.
// test/reference/parser.ts is that former implementation, frozen.
import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { deflate } from 'pako';
import * as core from '../src/tzx/parser';
import * as ref from './reference/parser';
import * as coreWriter from '../src/tzx/writer';
import * as refWriter from './reference/writer';
import * as coreDescribe from '../src/tzx/describe';
import * as refDescribe from './reference/describe';
import * as coreContent from '../src/tzx/content';
import * as refContent from './reference/content';
import { checkConsistency as coreCheck } from '../src/tzx/consistency';
import { checkConsistency as refCheck } from './reference/consistency';
import * as corePrograms from '../src/tzx/programs';
import * as refPrograms from './reference/programs';
import * as coreCompare from '../src/tzx/compare';
import * as refCompare from './reference/compare';
import { convertBlock as coreConvert } from '../src/tzx/convert';
import { convertBlock as refConvert } from './reference/convert';
import * as coreBits from '../src/tzx/bits';
import * as refBits from './reference/bits';
import * as corePokes from '../src/tzx/pokes';
import * as refPokes from './reference/pokes';
import * as coreCharset from '../src/spectrum/charset';
import * as refCharset from './reference/spectrum/charset';
import * as coreScreen from '../src/spectrum/screen';
import * as refScreen from './reference/spectrum/screen';
import * as coreBasic from '../src/spectrum/basic';
import * as refBasic from './reference/spectrum/basic';
import { disassemble as coreDisassemble } from '../src/spectrum/z80dis';
import { disassemble as refDisassemble } from './reference/spectrum/z80dis';
import * as coreAudio from '../src/tzx/audio';
import * as refAudio from './reference/audio';
import { serializeTzx, serializeTap } from '../src/tzx/writer';
import { Block, CREATABLE_IDS, ParsedTape, createBlock, isDataBlock } from '../src/tzx/types';
import { encodeHeader } from '../src/tzx/describe';

/** Blocks compare by content; uids are handed out per parse and always differ. */
function comparable(t: ParsedTape) {
  return {
    major: t.major,
    minor: t.minor,
    warnings: t.warnings,
    blocks: t.blocks.map(({ uid: _uid, ...rest }) => rest),
  };
}

function sameTape(bytes: Uint8Array, parse: 'parseTape' | 'parseTzx' | 'parseTap' = 'parseTape') {
  const got = comparable(core[parse](bytes));
  const want = comparable(ref[parse](bytes));
  // Deep equality covers key order poorly, so check the shapes too.
  expect(got).toEqual(want);
  expect(JSON.stringify(got, replacer)).toEqual(JSON.stringify(want, replacer));
  return got;
}

/** Uint8Array does not survive JSON.stringify on its own. */
function replacer(_k: string, v: unknown) {
  return v instanceof Uint8Array ? Array.from(v) : v;
}

/** Two typed arrays with the same contents, reported by the first index that differs.
 *  `toEqual` on `Array.from(...)` materialises two JS arrays and walks them as deep
 *  values, which for a tape rendered at 44.1 kHz is millions of elements and made
 *  this the slowest file in the suite — slow enough to time out on CI hardware. */
function expectSameBuffer(got: ArrayLike<number>, want: ArrayLike<number>, what: string) {
  expect(got.length, `${what}: length`).toBe(want.length);
  for (let i = 0; i < want.length; i++) {
    // NaN !== NaN, and toEqual called them equal; no sample should be one, but the
    // comparison this replaces would not have failed on a pair of them.
    if (got[i] !== want[i] && !(Number.isNaN(got[i]) && Number.isNaN(want[i]))) {
      expect.fail(`${what}: differs at index ${i}: got ${got[i]}, want ${want[i]}`);
    }
  }
}

const sample = (name: string) => new Uint8Array(fs.readFileSync(path.resolve('public/samples', name)));

describe('the Rust core against the TypeScript parser it replaced', () => {
  it('agrees on the sample tapes', () => {
    for (const name of fs.readdirSync(path.resolve('public/samples'))) {
      const tape = sameTape(sample(name));
      expect(tape.blocks.length).toBeGreaterThan(0);
      expect(tape.warnings).toEqual([]);
    }
  });

  it('agrees on every creatable block type', () => {
    const blocks: Block[] = CREATABLE_IDS.map((id) => createBlock(id));
    for (const b of blocks) if ('data' in b) (b as any).data = new Uint8Array([0x00, 3, 65, 66, 67, 0x55]);
    const g = blocks.find((b) => b.id === 0x19)!;
    (g as any).totp = 2;
    (g as any).pilotSymbols = [{ flags: 0, pulses: [2168] }, { flags: 1, pulses: [667, 735] }];
    (g as any).pilotStream = [{ symbol: 0, reps: 8063 }, { symbol: 1, reps: 1 }];
    (g as any).totd = 48;
    const bytes = serializeTzx(blocks);
    const tape = sameTape(bytes, 'parseTzx');
    expect(tape.blocks.length).toBe(blocks.length);
    // And the core's blocks still write back to the same file.
    expect(Array.from(serializeTzx(core.parseTzx(bytes).blocks))).toEqual(Array.from(bytes));
  });

  it('agrees on text, entries and high-bit characters', () => {
    const blocks: Block[] = [createBlock(0x21), createBlock(0x30), createBlock(0x31), createBlock(0x32), createBlock(0x33), createBlock(0x35)];
    (blocks[0] as any).name = 'Grüße';           // Latin-1 above 0x7f
    (blocks[1] as any).text = 'Line one\rLine two';
    (blocks[2] as any).text = 'Press \x7fENTER';
    (blocks[3] as any).entries = [{ type: 0, text: 'Full title' }, { type: 0xff, text: 'Comment' }];
    (blocks[4] as any).entries = [{ type: 0, id: 1, info: 0 }, { type: 4, id: 0, info: 3 }];
    (blocks[5] as any).ident = 'POKEs           ';
    sameTape(serializeTzx(blocks), 'parseTzx');
  });

  it('agrees on a select block and a call sequence', () => {
    const blocks: Block[] = [createBlock(0x28), createBlock(0x26), createBlock(0x23)];
    (blocks[0] as any).entries = [{ offset: 3, text: 'Side A' }, { offset: -2, text: 'Side B' }];
    (blocks[1] as any).offsets = [1, -1, 300];
    (blocks[2] as any).offset = -5;
    sameTape(serializeTzx(blocks), 'parseTzx');
  });

  it('agrees on TAP files, including the broken ones', () => {
    const h = encodeHeader({ type: 0, typeName: '', name: 'TEST      ', length: 10, param1: 10, param2: 10 });
    sameTape(new Uint8Array([19, 0, ...h, 3, 0, 0xff, 1, 0xfe]), 'parseTap');
    sameTape(new Uint8Array([4, 0, 1, 2]), 'parseTap');          // truncated block
    sameTape(new Uint8Array([2, 0, 1, 2, 9]), 'parseTap');       // trailing byte
    sameTape(new Uint8Array(0), 'parseTap');                     // empty file
    // Auto-detection sends all of these to the TAP parser too.
    sameTape(new Uint8Array([19, 0, ...h, 3, 0, 0xff, 1, 0xfe]));
  });

  it('agrees on damaged TZX files', () => {
    const sig = Array.from('ZXTape!\x1a').map((c) => c.charCodeAt(0));
    const cases: number[][] = [
      [0x5b, 3, 0, 0, 0, 1, 2, 3, 0x20, 0xe8, 0x03], // unknown block, then a pause
      [0x10, 0xe8, 0x03, 10, 0, 1, 2],               // data block that claims more than it has
      [0x18, 4, 0, 0, 0, 1, 2, 3, 4],                // CSW length below its own header
      [0x34, 1, 2, 3, 4, 5, 6, 7, 8],                // deprecated emulation info
      [0x40, 0, 2, 0, 0, 0xaa, 0xbb],                // deprecated snapshot
      [0x16, 2, 0, 0, 0, 1, 2],                      // deprecated C64 block
      [0x21, 5],                                     // group start with a truncated name
      [0x19, 8, 0, 0, 0, 0xe8, 0x03, 0, 0],          // generalized block cut short
    ];
    for (const body of cases) {
      const file = new Uint8Array([...sig, 1, 20, ...body]);
      const tape = sameTape(file, 'parseTzx');
      expect(tape.blocks.length).toBeGreaterThan(0);
    }
  });

  it('agrees on an empty tape and on a file that is only a header', () => {
    const sig = Array.from('ZXTape!\x1a').map((c) => c.charCodeAt(0));
    sameTape(new Uint8Array([...sig, 1, 20]), 'parseTzx');
    expect(core.isTzx(new Uint8Array([...sig, 1, 20]))).toBe(ref.isTzx(new Uint8Array([...sig, 1, 20])));
  });

  it('refuses a TZX parse of something without the signature', () => {
    const bytes = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    expect(() => core.parseTzx(bytes)).toThrow('Not a TZX file (missing ZXTape! signature)');
    expect(() => ref.parseTzx(bytes)).toThrow('Not a TZX file (missing ZXTape! signature)');
  });

  it('round-trips a TAP export through the core', () => {
    const tape = core.parseTape(sample('Tapeti demo.tap'));
    expect(Array.from(serializeTap(tape.blocks).bytes)).toEqual(Array.from(sample('Tapeti demo.tap')));
  });
});

describe('the Rust writer against the TypeScript writer it replaced', () => {
  /** Every creatable block, with content in the ones that carry data. */
  function everyBlock(): Block[] {
    const blocks: Block[] = CREATABLE_IDS.map((id) => createBlock(id));
    for (const b of blocks) if ('data' in b) (b as any).data = new Uint8Array([0x00, 3, 65, 66, 67, 0x55]);
    const g = blocks.find((b) => b.id === 0x19)!;
    (g as any).totp = 2;
    (g as any).pilotSymbols = [{ flags: 0, pulses: [2168] }, { flags: 1, pulses: [667, 735] }];
    (g as any).pilotStream = [{ symbol: 0, reps: 8063 }, { symbol: 1, reps: 1 }];
    (g as any).totd = 48;
    return blocks;
  }

  /** Blocks the old writer had to paper over: over-long text, short idents, odd glue. */
  function awkwardBlocks(): Block[] {
    const blocks: Block[] = [
      createBlock(0x21), createBlock(0x30), createBlock(0x31), createBlock(0x28),
      createBlock(0x32), createBlock(0x33), createBlock(0x35), createBlock(0x5a),
      createBlock(0x19), createBlock(0x13), createBlock(0x26),
    ];
    (blocks[0] as any).name = 'x'.repeat(300);
    (blocks[1] as any).text = 'y'.repeat(300);
    (blocks[2] as any).text = 'Grüße\r\nfrom 1982';
    (blocks[3] as any).entries = [{ offset: 2, text: 'z'.repeat(300) }, { offset: -3, text: '' }];
    (blocks[4] as any).entries = [{ type: 4, text: 'en' }, { type: 0xff, text: 'two\nlines' }];
    (blocks[5] as any).entries = [{ type: 0, id: 0x1c, info: 1 }, { type: 0x10, id: 0, info: 0 }];
    (blocks[6] as any).ident = 'POKEs';
    (blocks[7] as any).raw = new Uint8Array([1, 2]);
    Object.assign(blocks[8], {
      pause: 0, totp: 0, npp: 2, pilotSymbols: [], pilotStream: [], totd: 16, npd: 3,
      dataSymbols: [{ flags: 0, pulses: [855] }, { flags: 0, pulses: [] }],
      data: new Uint8Array([0xaa]),
    });
    (blocks[9] as any).pulses = [667, 735, 1000];
    (blocks[10] as any).offsets = [1, -1, 300];
    return blocks;
  }

  function sameOutput(blocks: Block[], version?: { major: number; minor: number }) {
    expect(Array.from(coreWriter.serializeTzx(blocks, version))).toEqual(Array.from(refWriter.serializeTzx(blocks, version)));
    const coreTap = coreWriter.serializeTap(blocks);
    const refTap = refWriter.serializeTap(blocks);
    expect(Array.from(coreTap.bytes)).toEqual(Array.from(refTap.bytes));
    expect(coreTap.skipped).toEqual(refTap.skipped);
    expect(coreWriter.requiredVersion(blocks)).toEqual(refWriter.requiredVersion(blocks));
    for (const loaded of [null, { major: 1, minor: 0 }, { major: 1, minor: 20 }]) {
      expect(coreWriter.saveVersion(blocks, loaded)).toEqual(refWriter.saveVersion(blocks, loaded));
    }
    for (const b of blocks) {
      expect(Array.from(coreWriter.serializeBlock(b))).toEqual(Array.from(refWriter.serializeBlock(b)));
    }
  }

  it('writes every creatable block the same way', () => {
    sameOutput(everyBlock());
    sameOutput(everyBlock(), { major: 1, minor: 20 });
  });

  it('writes the awkward blocks the same way', () => {
    sameOutput(awkwardBlocks());
  });

  it('writes the sample tapes the same way', () => {
    for (const name of fs.readdirSync(path.resolve('public/samples'))) {
      sameOutput(core.parseTape(sample(name)).blocks);
    }
  });

  it('writes an unknown block and an empty tape the same way', () => {
    const unknown = core.parseTzx(new Uint8Array([
      ...Array.from('ZXTape!\x1a').map((c) => c.charCodeAt(0)), 1, 20, 0x5b, 3, 0, 0, 0, 1, 2, 3,
    ])).blocks;
    sameOutput(unknown);
    sameOutput([]);
  });

  it('round-trips the sample tapes byte for byte', () => {
    for (const name of fs.readdirSync(path.resolve('public/samples'))) {
      const bytes = sample(name);
      const tape = core.parseTape(bytes);
      const out = name.endsWith('.tap')
        ? coreWriter.serializeTap(tape.blocks).bytes
        : coreWriter.serializeTzx(tape.blocks, { major: tape.major, minor: tape.minor });
      expect(Array.from(out)).toEqual(Array.from(bytes));
    }
  });
});

describe('the Rust descriptions, detection and structure against the TypeScript they replaced', () => {
  const sig = Array.from('ZXTape!\x1a').map((c) => c.charCodeAt(0));
  const tzx = (...body: number[]) => new Uint8Array([...sig, 1, 20, ...body]);

  /** Every creatable block plus a few with realistic content. */
  function blocksForDescription(): Block[] {
    const blocks: Block[] = CREATABLE_IDS.map((id) => createBlock(id));
    for (const b of blocks) if ('data' in b) (b as any).data = new Uint8Array([0x00, 3, 65, 66, 67, 0x55]);
    const header = (type: number, name: string, length: number, p1: number, p2: number) =>
      new Uint8Array([0x00, ...encodeHeader({ type, typeName: '', name, length, param1: p1, param2: p2 }).subarray(1, 18), 0x55]);
    return [
      ...blocks,
      { ...createBlock(0x10), data: header(0, 'demo      ', 131, 131, 20) } as Block,
      { ...createBlock(0x10), data: header(3, 'demo.scr  ', 6912, 16384, 32768) } as Block,
      { ...createBlock(0x11), data: header(1, 'nums      ', 40, 0x4100, 0) } as Block,
      { ...createBlock(0x14), data: header(2, 'chars     ', 40, 0x0100, 0) } as Block,
      { ...createBlock(0x21), name: 'Machine code' } as Block,
      { ...createBlock(0x23), offset: -3 } as Block,
      { ...createBlock(0x30), text: 'Two\r\nlines' } as Block,
      { ...createBlock(0x31), time: 3, text: 'Grüße\nfrom 1982' } as Block,
      { ...createBlock(0x35), ident: '  POKEs  ' } as Block,
      { ...createBlock(0x20), pause: 0 } as Block,
      { ...createBlock(0x2b), level: 1 } as Block,
      ...core.parseTzx(tzx(0x5b, 3, 0, 0, 0, 1, 2, 3, 0x34, 1, 2, 3, 4, 5, 6, 7, 8)).blocks,
    ];
  }

  it('describes every block the same way, in both number bases', () => {
    for (const b of blocksForDescription()) {
      for (const hex of [false, true]) {
        expect(coreDescribe.describeBlock(b, hex)).toBe(refDescribe.describeBlock(b, hex));
      }
      expect(coreDescribe.blockLength(b)).toBe(refDescribe.blockLength(b));
      expect(coreDescribe.isMetadata(b)).toBe(refDescribe.isMetadata(b));
    }
  });

  it('describes the blocks of the sample tapes the same way', () => {
    for (const name of fs.readdirSync(path.resolve('public/samples'))) {
      for (const b of core.parseTape(sample(name)).blocks) {
        expect(coreDescribe.describeBlock(b, false)).toBe(refDescribe.describeBlock(b, false));
        expect(coreDescribe.describeBlock(b, true)).toBe(refDescribe.describeBlock(b, true));
        expect(coreDescribe.blockLength(b)).toBe(refDescribe.blockLength(b));
      }
    }
  });

  it('decodes and encodes ROM headers the same way', () => {
    for (const type of [0, 1, 2, 3, 4]) {
      const h = { type, typeName: '', name: 'name      ', length: 100, param1: 32768, param2: 10 };
      const bytes = refDescribe.encodeHeader(h);
      expect(Array.from(coreDescribe.encodeHeader(h))).toEqual(Array.from(bytes));
      expect(coreDescribe.decodeHeader(bytes)).toEqual(refDescribe.decodeHeader(bytes));
      expect(coreDescribe.checksum(bytes)).toBe(refDescribe.checksum(bytes));
      expect(coreDescribe.checksum(bytes, 1, 5)).toBe(refDescribe.checksum(bytes, 1, 5));
    }
    // Not a header: wrong length, wrong flag, unknown type.
    for (const bad of [new Uint8Array(0), new Uint8Array(19).fill(9), new Uint8Array(18)]) {
      expect(coreDescribe.decodeHeader(bad)).toEqual(refDescribe.decodeHeader(bad));
    }
  });

  it('detects the same content for every block of every sample tape', () => {
    for (const name of fs.readdirSync(path.resolve('public/samples'))) {
      const blocks = core.parseTape(sample(name)).blocks;
      const labels = coreContent.contentLabels(blocks);
      blocks.forEach((b, i) => {
        expect(coreContent.detectContent(blocks, i)).toEqual(refContent.detectContent(blocks, i));
        const refLabel = isDataBlock(b) ? refContent.detectContent(blocks, i).label : '';
        expect(labels[i]).toBe(refLabel);
      });
    }
  });

  it('detects the same content for the awkward cases', () => {
    const withFlag = (body: number[]) => new Uint8Array([0xff, ...body, 0]);
    const std = (data: Uint8Array) => ({ ...createBlock(0x10), data } as Block);
    const hdr = (type: number, length: number, p1: number) =>
      std(new Uint8Array([0x00, ...encodeHeader({ type, typeName: '', name: 'x         ', length, param1: p1, param2: 0 }).subarray(1, 18), 0x55]));
    const cases: Block[][] = [
      [hdr(3, 6912, 16384), std(withFlag(new Array(6912).fill(0)))],
      [hdr(3, 6912, 40000), std(withFlag(new Array(6912).fill(0)))],
      [hdr(3, 6912, 16384), std(withFlag(new Array(100).fill(0)))],       // short
      [hdr(3, 120, 32768), std(withFlag(new Array(130).fill(0)))],        // long
      [hdr(0, 40, 0), std(withFlag(new Array(40).fill(0)))],              // BASIC
      [hdr(1, 40, 0x4100), std(withFlag(new Array(40).fill(0)))],         // number array
      [hdr(2, 40, 0x0100), std(withFlag(new Array(40).fill(0)))],         // char array
      [std(withFlag([0x00, 0x0a, 0x05, 0x00, 1, 2, 3, 4, 0x0d]))],        // BASIC by heuristic
      [std(withFlag(new Array(6912).fill(1)))],                           // screen by heuristic
      [std(withFlag([0xc3, 0x00, 0x80, 0x21, 0x00, 0x40, 0x11]))],        // plain data
      [std(new Uint8Array(0))],                                           // empty
      [{ ...createBlock(0x14), data: new Uint8Array([1, 2, 3]) } as Block],
      [createBlock(0x20)],                                                // not a data block
    ];
    for (const blocks of cases) {
      blocks.forEach((_b, i) => {
        expect(coreContent.detectContent(blocks, i)).toEqual(refContent.detectContent(blocks, i));
      });
      expect(coreContent.contentLabels(blocks)).toEqual(
        blocks.map((b, i) => (isDataBlock(b) ? refContent.detectContent(blocks, i).label : '')),
      );
      expect(coreContent.basicScore(blocks[0].id === 0x10 ? (blocks[0] as any).data : new Uint8Array(0)))
        .toBe(refContent.basicScore(blocks[0].id === 0x10 ? (blocks[0] as any).data : new Uint8Array(0)));
    }
    // Out of range indices answer with the default.
    expect(coreContent.detectContent([], 0)).toEqual(refContent.detectContent([], 0));
    expect(coreContent.detectContent(cases[0], 9)).toEqual(refContent.detectContent(cases[0], 9));
  });

  it('finds the same consistency issues', () => {
    const b = (id: number, fields: Record<string, unknown> = {}) => Object.assign(createBlock(id), fields) as Block;
    const cases: Block[][] = [
      [],
      [b(0x21), b(0x24), b(0x22)],                                   // crossing
      [b(0x21), b(0x21), b(0x22), b(0x22)],                          // nested group
      [b(0x24, { count: 0 }), b(0x25), b(0x24, { count: 1 }), b(0x25)],
      [b(0x22), b(0x25)],                                            // ends without starts
      [b(0x23, { offset: 0 })],                                      // jump to itself
      [b(0x23, { offset: 99 })],                                     // outside the tape
      [b(0x26, { offsets: [] }), b(0x26, { offsets: [0, 99] })],
      [b(0x28, { entries: [{ offset: 99, text: 'x' }] })],
      [b(0x11, { usedBits: 0, data: new Uint8Array(0) })],
      [b(0x15, { usedBits: 9, tstates: 0 })],
      [b(0x10, { data: new Uint8Array([0x00, 1, 2, 3]) })],           // bad checksum
      [b(0x12, { count: 0 }), b(0x13, { pulses: [] })],
      [b(0x19, { totp: 2, pilotStream: [{ symbol: 5, reps: 1 }], pilotSymbols: [], totd: 8, dataSymbols: [], data: new Uint8Array(0) })],
      [b(0x32, { entries: [] }), b(0x33, { entries: [] })],
      [b(0x24, { count: 3 }), b(0x12), b(0x25), b(0x20)],             // a healthy loop
      [b(0x26, { offsets: [1] }), b(0x12), b(0x27)],                  // a call that returns
      [b(0x26, { offsets: [1] }), b(0x12)],                           // a call that never returns
      [b(0x21)],                                                      // never closed
      [b(0x23, { offset: 1 }), b(0x23, { offset: -1 })],              // infinite jump loop
    ];
    for (const blocks of cases) {
      for (const base of [0, 1]) expect(coreCheck(blocks, base)).toEqual(refCheck(blocks, base));
    }
    for (const name of fs.readdirSync(path.resolve('public/samples'))) {
      const blocks = core.parseTape(sample(name)).blocks;
      expect(coreCheck(blocks)).toEqual(refCheck(blocks));
    }
  });

  // The two predicates the TypeScript still implements itself. The same tables
  // are asserted in core/tests/logic.rs, so the copies cannot drift apart.
  it('classifies metadata blocks the way the core does', () => {
    const metadata = [0x21, 0x22, 0x30, 0x31, 0x32, 0x33, 0x35, 0x5a];
    for (const id of CREATABLE_IDS) {
      expect(coreDescribe.isMetadata(createBlock(id))).toBe(metadata.includes(id));
    }
    const unknown = (id: number) => core.parseTzx(tzx(id, 0, 0, 0, 0)).blocks[0];
    expect(coreDescribe.isMetadata(unknown(0x5b))).toBe(true);
    expect(coreDescribe.isMetadata(unknown(0x16))).toBe(false);
    expect(coreDescribe.isMetadata(unknown(0x17))).toBe(false);
  });

  it('strips flag and checksum bytes the way the core does', () => {
    const d = new Uint8Array([0xff, 1, 2, 3, 0x55]);
    expect(Array.from(coreContent.blockBody(d, true, true))).toEqual([1, 2, 3]);
    expect(Array.from(coreContent.blockBody(d, true, false))).toEqual([1, 2, 3, 0x55]);
    expect(Array.from(coreContent.blockBody(d, false, true))).toEqual([0xff, 1, 2, 3]);
    expect(Array.from(coreContent.blockBody(d, false, false))).toEqual([0xff, 1, 2, 3, 0x55]);
    expect(Array.from(coreContent.blockBody(new Uint8Array(0), true, true))).toEqual([]);
    expect(Array.from(coreContent.blockBody(new Uint8Array([7]), true, true))).toEqual([]);
  });

  it('finds the same groups, programs and titles', () => {
    const b = (id: number, fields: Record<string, unknown> = {}) => Object.assign(createBlock(id), fields) as Block;
    const prog = (name: string) =>
      b(0x10, { data: new Uint8Array([0x00, ...encodeHeader({ type: 0, typeName: '', name, length: 10, param1: 0, param2: 10 }).subarray(1, 18), 0x55]) });
    const cases: Block[][] = [
      [],
      [prog('A         '), b(0x10), prog('B         '), b(0x10)],
      [b(0x21, { name: 'Game' }), prog('A         '), b(0x22), b(0x21, { name: 'Empty' }), b(0x22)],
      [b(0x21), b(0x24), b(0x25), b(0x22)],
      [b(0x24), b(0x21), b(0x22), b(0x25), b(0x21), b(0x22)],
      [b(0x28, { entries: [{ offset: 2, text: 'Side B' }, { offset: 99, text: 'Nope' }] }), b(0x10), prog('C         ')],
      [b(0x32, { entries: [{ type: 0, text: 'The Tape' }] }), b(0x10)],
      [b(0x32, { entries: [{ type: 1, text: 'No title' }] }), b(0x10)],
      [b(0x30, { text: 'lead in' }), prog('A         '), b(0x20), prog('B         ')],
      [b(0x21, { name: '' }), prog('          '), b(0x22)],
    ];
    for (const blocks of cases) {
      expect([...corePrograms.groupRanges(blocks)]).toEqual([...refPrograms.groupRanges(blocks)]);
      expect(corePrograms.detectPrograms(blocks)).toEqual(refPrograms.detectPrograms(blocks));
      expect(corePrograms.tapeTitle(blocks)).toEqual(refPrograms.tapeTitle(blocks));
    }
    for (const name of fs.readdirSync(path.resolve('public/samples'))) {
      const blocks = core.parseTape(sample(name)).blocks;
      expect([...corePrograms.groupRanges(blocks)]).toEqual([...refPrograms.groupRanges(blocks)]);
      expect(corePrograms.detectPrograms(blocks)).toEqual(refPrograms.detectPrograms(blocks));
      expect(corePrograms.tapeTitle(blocks)).toEqual(refPrograms.tapeTitle(blocks));
    }
  });
});

describe('the Rust conversion, comparison, bits and POKEs against the TypeScript they replaced', () => {
  const sig = Array.from('ZXTape!\x1a').map((c) => c.charCodeAt(0));

  /** One block of every type, with content, plus an unknown one. */
  function specimens(): Block[] {
    const blocks: Block[] = CREATABLE_IDS.map((id) => createBlock(id));
    for (const b of blocks) if ('data' in b) (b as any).data = new Uint8Array([0x00, 1, 2, 3, 0x55]);
    const turbo = blocks.find((b) => b.id === 0x11)! as any;
    Object.assign(turbo, { pilot: 2000, sync1: 600, sync2: 700, zero: 800, one: 1600, pilotLen: 3000 });
    (blocks.find((b) => b.id === 0x21) as any).name = 'Level 2';
    (blocks.find((b) => b.id === 0x30) as any).text = 'Hello';
    (blocks.find((b) => b.id === 0x31) as any).text = 'Press play';
    return [
      ...blocks,
      { ...createBlock(0x10), data: new Uint8Array([0xff, 1]) } as Block,  // data, not header
      { ...createBlock(0x10), pause: 1234, data: new Uint8Array(0) } as Block,
      ...core.parseTzx(new Uint8Array([...sig, 1, 20, 0x5b, 2, 0, 0, 0, 7, 8])).blocks,
    ];
  }

  it('converts between every pair of block types the same way', () => {
    for (const b of specimens()) {
      for (const id of CREATABLE_IDS) {
        const got = coreConvert(b, id);
        const want = refConvert(b, id);
        expect(JSON.stringify(got, replacer)).toEqual(JSON.stringify(want, replacer));
        expect(got.uid).toBe(b.uid);
      }
    }
  });

  it('compares blocks the same way in every mode', () => {
    const blocks = specimens();
    const modes = ['data', 'data+timings', 'data+timings+pauses'] as const;
    for (const mode of modes) {
      for (const a of blocks) {
        for (const b of blocks) {
          expect(coreCompare.blocksEqual(a, b, mode)).toBe(refCompare.blocksEqual(a, b, mode));
        }
      }
    }
    // Same block type, different pause: equal until pauses count.
    const x = { ...createBlock(0x10), pause: 1, data: new Uint8Array([1]) } as Block;
    const y = { ...createBlock(0x10), pause: 2, data: new Uint8Array([1]) } as Block;
    expect(coreCompare.blocksEqual(x, y, 'data+timings')).toBe(true);
    expect(coreCompare.blocksEqual(x, y, 'data+timings+pauses')).toBe(false);
  });

  it('compares tapes the same way in every mode', () => {
    const b = (id: number, fields: Record<string, unknown> = {}) => Object.assign(createBlock(id), fields) as Block;
    const tapes: Block[][] = [
      [],
      [b(0x10, { data: new Uint8Array([1, 2]) })],
      [b(0x10, { data: new Uint8Array([1, 2]) }), b(0x20)],
      [b(0x30, { text: 'note' }), b(0x10, { data: new Uint8Array([1, 2]) })],
      [b(0x10, { data: new Uint8Array([9]) }), b(0x10, { data: new Uint8Array([1, 2]) })],
      core.parseTape(sample('Tapeti demo.tzx')).blocks,
      core.parseTape(sample('Tapeti demo (variant).tzx')).blocks,
    ];
    const blockModes = ['data', 'data+timings', 'data+timings+pauses'] as const;
    const tapeModes = ['datablocks', 'ignore-metadata', 'all'] as const;
    for (const left of tapes) {
      for (const right of tapes) {
        for (const bm of blockModes) {
          for (const tm of tapeModes) {
            expect(coreCompare.compareTapes(left, right, bm, tm)).toEqual(refCompare.compareTapes(left, right, bm, tm));
          }
        }
      }
    }
  });

  it('finds the same matching blocks', () => {
    const blocks = core.parseTape(sample('Tapeti demo.tzx')).blocks;
    for (const needle of blocks) {
      expect(coreCompare.findMatches(needle, blocks, 'data')).toEqual(refCompare.findMatches(needle, blocks, 'data'));
      expect(coreCompare.findMatches(needle, blocks, 'data+timings')).toEqual(refCompare.findMatches(needle, blocks, 'data+timings'));
    }
    // A needle from somewhere else is not skipped, so an identical block matches.
    const other = core.parseTape(sample('Tapeti demo.tzx')).blocks[1];
    expect(coreCompare.findMatches(other, blocks, 'data')).toEqual(refCompare.findMatches(other, blocks, 'data'));
  });

  it('drops, adds, shifts, joins and flips bits the same way', () => {
    const streams: coreBits.BitData[] = [
      { data: new Uint8Array(0), usedBits: 8 },
      { data: new Uint8Array([0b10110011]), usedBits: 8 },
      { data: new Uint8Array([0b10110011]), usedBits: 3 },
      { data: new Uint8Array([0xff, 0x00, 0xa5]), usedBits: 5 },
      { data: new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9]), usedBits: 1 },
    ];
    const ops = ['dropBits', 'addBits', 'shiftLeftBits', 'shiftRightBits'] as const;
    for (const d of streams) {
      expect(coreBits.totalBits(d)).toBe(refBits.totalBits(d));
      expect(Array.from(coreBits.flipBytes(d.data))).toEqual(Array.from(refBits.flipBytes(d.data)));
      for (const op of ops) {
        for (const n of [0, 1, 3, 8, 17, 100]) {
          const got = coreBits[op](d, n);
          const want = refBits[op](d, n);
          expect({ data: Array.from(got.data), usedBits: got.usedBits })
            .toEqual({ data: Array.from(want.data), usedBits: want.usedBits });
        }
      }
    }
    for (const parts of [streams, streams.slice(1, 3), [streams[1]], streams.slice(0, 1)]) {
      const got = coreBits.joinBits(parts);
      const want = refBits.joinBits(parts);
      expect({ data: Array.from(got.data), usedBits: got.usedBits })
        .toEqual({ data: Array.from(want.data), usedBits: want.usedBits });
    }
  });

  it('reads and writes POKEs blocks the same way', () => {
    const infos: corePokes.PokesInfo[] = [
      { description: '', trainers: [] },
      { description: 'Cheats for the demo', trainers: [] },
      {
        description: 'Line one\nline two',
        trainers: [
          { description: 'Infinite lives', pokes: [{ page: null, addr: 32768, value: 255, original: 12 }] },
          { description: 'Ask me', pokes: [{ page: 3, addr: 65535, value: null, original: null }] },
          { description: 'Two\nline name', pokes: [] },
        ],
      },
    ];
    for (const info of infos) {
      const bytes = refPokes.encodePokes(info);
      expect(Array.from(corePokes.encodePokes(info))).toEqual(Array.from(bytes));
      expect(corePokes.decodePokes(bytes)).toEqual(refPokes.decodePokes(bytes));
      for (const hex of [false, true]) {
        expect(corePokes.pokesToText(info, hex)).toBe(refPokes.pokesToText(info, hex));
      }
    }
    // A truncated block fails the same way on both sides.
    const truncated = new Uint8Array([3, 65, 66]);
    expect(() => corePokes.decodePokes(truncated)).toThrow('Unexpected end of file at offset 1');
    expect(() => refPokes.decodePokes(truncated)).toThrow('Unexpected end of file at offset 1');
  });

  it('parses POKEs text the same way, including the ways it can go wrong', () => {
    const texts = [
      '',
      '; a note\n; another\n\n[Trainer]\nPOKE 32768,255',
      'POKE 1:32768,255/12',
      '32768,255',
      'poke 3:$8000,?\nPOKE #65535,0x10',
      '[Name | with pipes]\n; trainer note\nPOKE 1,2',
      'POKE 1,2\nPOKE 3,4\n\n[Second]\nPOKE 5,6',
      '  POKE   1 , 2 / 3  ',
    ];
    for (const text of texts) {
      for (const hex of [false, true]) {
        expect(corePokes.textToPokes(text, hex)).toEqual(refPokes.textToPokes(text, hex));
        // And what comes out writes back to the same bytes.
        const info = corePokes.textToPokes(text, hex);
        expect(Array.from(corePokes.encodePokes(info))).toEqual(Array.from(refPokes.encodePokes(info)));
      }
    }
    for (const bad of ['nonsense', 'POKE', 'POKE 1', 'POKE 1,', 'POKE ,2', 'POKE 1,2,3', 'POKE zz,2']) {
      let coreError = '';
      let refError = '';
      try { corePokes.textToPokes(bad, false); } catch (e) { coreError = (e as Error).message; }
      try { refPokes.textToPokes(bad, false); } catch (e) { refError = (e as Error).message; }
      expect(coreError).toBe(refError);
      expect(coreError).not.toBe('');
    }
  });
});

describe('the Rust Spectrum side against the TypeScript it replaced', () => {
  /** A deterministic byte stream, so a failure is reproducible. */
  function pseudoRandom(n: number, seed = 1): Uint8Array {
    const out = new Uint8Array(n);
    let x = seed;
    for (let i = 0; i < n; i++) {
      x = (x * 1103515245 + 12345) & 0x7fffffff;
      out[i] = (x >> 16) & 0xff;
    }
    return out;
  }

  it('maps every character the same way', () => {
    for (let c = 0; c < 256; c++) {
      expect(coreCharset.zxChar(c)).toBe(refCharset.zxChar(c));
      expect(coreCharset.zxChar(c, false)).toBe(refCharset.zxChar(c, false));
      expect(coreCharset.dumpChar(c)).toBe(refCharset.dumpChar(c));
    }
  });

  it('renders screens pixel for pixel', () => {
    const screens: [Uint8Array, number][] = [
      [pseudoRandom(6912), 0],
      [pseudoRandom(6912, 99), 0],
      [new Uint8Array(6912), 0],
      [pseudoRandom(100), 0],            // far too short
      [pseudoRandom(6912), 17],          // offset into the data
      [pseudoRandom(6912), -50],         // base address before the block
      [new Uint8Array(0), 0],
    ];
    for (const [data, offset] of screens) {
      for (const opts of [{}, { hideAttributes: true }, { flashPhase: true }, { hideAttributes: true, flashPhase: true }]) {
        const got = coreScreen.renderScreen(data, offset, opts);
        const want = refScreen.renderScreen(data, offset, opts);
        expectSameBuffer(got, want, `screen ${data.length} bytes at ${offset}, ${JSON.stringify(opts)}`);
      }
      expect(coreScreen.hasFlash(data, offset)).toBe(refScreen.hasFlash(data, offset));
    }
  });

  it('decodes and formats Sinclair numbers the same way', () => {
    const cases: number[][] = [
      [0x00, 0x00, 0x01, 0x00, 0x00],     // small integer 1
      [0x00, 0xff, 0xff, 0xff, 0x00],     // small integer -1
      [0x00, 0x00, 0x39, 0x30, 0x00],     // 12345
      [0x81, 0x00, 0x00, 0x00, 0x00],     // 1.0
      [0x82, 0x49, 0x0f, 0xda, 0xa2],     // pi-ish
      [0x68, 0x00, 0x00, 0x00, 0x00],     // very small
      [0xff, 0x7f, 0xff, 0xff, 0xff],     // very large
      [0x80, 0x80, 0x00, 0x00, 0x00],     // negative
      [0x91, 0x1c, 0x00, 0x00, 0x00],
    ];
    for (const c of cases) {
      const d = new Uint8Array(c);
      expect(coreBasic.decodeNumber(d, 0)).toBe(refBasic.decodeNumber(d, 0));
      expect(coreBasic.formatNumber(coreBasic.decodeNumber(d, 0))).toBe(refBasic.formatNumber(refBasic.decodeNumber(d, 0)));
    }
    // Out of range gives NaN on both sides, which formats as '?'.
    expect(coreBasic.decodeNumber(new Uint8Array(3), 0)).toBeNaN();
    expect(coreBasic.formatNumber(NaN)).toBe(refBasic.formatNumber(NaN));
    // And the formatting itself, over the shapes it has to get right.
    const numbers = [
      0, 1, -1, 42, -42, 1e14, -1e14, 1e15, 1e16, 0.5, -0.5, 1 / 3, 2 / 3, Math.PI, Math.E,
      1e-7, 1e-6, 1.5e-7, 123456789, 12345678.5, 0.1, 0.2, 0.30000000000000004, 1e21, -1e21,
      1e-21, 255.00390625, 6.02e23, Infinity, -Infinity,
    ];
    for (const v of numbers) {
      expect(coreBasic.formatNumber(v)).toBe(refBasic.formatNumber(v));
    }
  });

  it('lists the same BASIC program', () => {
    // 10 PRINT "hi": 20 LET a=1 (with a hidden 5-byte number), plus control codes.
    const line = (no: number, body: number[]) => [no >> 8, no & 0xff, (body.length + 1) & 0xff, (body.length + 1) >> 8, ...body, 0x0d];
    const prog = new Uint8Array([
      ...line(10, [0xf5, 0x22, 0x68, 0x69, 0x22]),                        // PRINT "hi"
      ...line(20, [0xf1, 0x61, 0x3d, 0x31, 0x0e, 0x00, 0x00, 0x01, 0x00, 0x00]), // LET a=1
      ...line(30, [0xf5, 0x10, 0x02, 0x16, 0x05, 0x0a, 0x06, 0x08, 0x01]), // control codes
      ...line(40, [0xea, 0x20, 0x80, 0x90, 0xa5]),                        // REM with odd bytes
      ...line(50, [0xf1, 0x62, 0x3d, 0x32, 0x0e, 0x00, 0x00, 0x63, 0x00, 0x00]), // altered number
      0x00, 0x3c, 0x00, 0x00,                                             // zero length line
    ]);
    const allOpts = [
      { showNumbers: false, basic128: false, speccyFormat: false },
      { showNumbers: true, basic128: false, speccyFormat: false },
      { showNumbers: false, basic128: true, speccyFormat: false },
      { showNumbers: false, basic128: false, speccyFormat: true },
      { showNumbers: true, basic128: true, speccyFormat: true },
    ];
    for (const opts of allOpts) {
      const got = coreBasic.listBasic(prog, 0, prog.length, opts);
      const want = refBasic.listBasic(prog, 0, prog.length, opts);
      expect(got).toEqual(want);
      expect(coreBasic.basicToText(got, opts)).toBe(refBasic.basicToText(want, opts));
    }
    // A slice of noise must not throw on either side.
    const noise = pseudoRandom(400, 7);
    for (const opts of allOpts) {
      expect(coreBasic.listBasic(noise, 0, noise.length, opts)).toEqual(refBasic.listBasic(noise, 0, noise.length, opts));
    }
  });

  it('lists the same variables', () => {
    // A variable's header byte is its type in the top three bits and the
    // letter's place in the alphabet in the low five.
    const head = (kind: number, letter: string) => (kind << 5) | (letter.charCodeAt(0) - 0x60);
    const num = (v: number[]) => v;
    const vars = new Uint8Array([
      head(0b100, 'a'), ...num([0x00, 0x00, 0x2a, 0x00, 0x00]),           // a = 42
      head(0b011, 'b'), 0x03, 0x00, 0x68, 0x69, 0x21,                     // b$ = "hi!"
      head(0b101, 'c'), 0xf2, ...num([0x00, 0x00, 0x07, 0x00, 0x00]),     // cr = 7 (long name)
      head(0b111, 'd'), ...num([0x00, 0x00, 0x01, 0x00, 0x00]), ...num([0x00, 0x00, 0x0a, 0x00, 0x00]),
      ...num([0x00, 0x00, 0x01, 0x00, 0x00]), 0x0a, 0x00, 0x01,           // FOR control
      head(0b010, 'e'), 0x0b, 0x00, 0x01, 0x02, 0x00,                     // number array
      ...num([0x00, 0x00, 0x01, 0x00, 0x00]), ...num([0x00, 0x00, 0x02, 0x00, 0x00]),
      0x80,
    ]);
    expect(coreBasic.listVariables(vars, 0, vars.length)).toEqual(refBasic.listVariables(vars, 0, vars.length));
    // Character arrays, a zero-dimension array and garbage.
    const chars = new Uint8Array([head(0b110, 'f'), 0x07, 0x00, 0x01, 0x04, 0x00, 0x68, 0x69, 0x21, 0x3f, 0x80]);
    expect(coreBasic.listVariables(chars, 0, chars.length)).toEqual(refBasic.listVariables(chars, 0, chars.length));
    const broken = new Uint8Array([head(0b010, 'g'), 0x03, 0x00, 0x00, 0x80]);
    expect(coreBasic.listVariables(broken, 0, broken.length)).toEqual(refBasic.listVariables(broken, 0, broken.length));
    const garbage = new Uint8Array([0x01, 0x02, 0x03]);
    expect(coreBasic.listVariables(garbage, 0, garbage.length)).toEqual(refBasic.listVariables(garbage, 0, garbage.length));
    expect(coreBasic.listVariables(new Uint8Array(0), 0, 0)).toEqual(refBasic.listVariables(new Uint8Array(0), 0, 0));
  });

  it('disassembles every opcode the same way', () => {
    // Every single-byte opcode, then the same under each prefix, with two
    // operand bytes after it so the immediates and displacements differ.
    const cases: number[][] = [];
    for (let op = 0; op < 256; op++) {
      cases.push([op, 0x34, 0x12]);
      cases.push([0xcb, op, 0x34, 0x12]);
      cases.push([0xed, op, 0x34, 0x12]);
      for (const prefix of [0xdd, 0xfd]) {
        cases.push([prefix, op, 0x34, 0x12]);
        cases.push([prefix, 0xcb, 0x05, op, 0x34]);
        cases.push([prefix, 0xcb, 0xfb, op, 0x34]);  // negative displacement
      }
    }
    // Prefix chains and a run that falls off the end of the data.
    cases.push([0xdd, 0xfd, 0xdd, 0x21, 0x00, 0x40]);
    cases.push([0xdd], [0xed], [0xcb], [0x21], [0x18]);
    for (const hex of [true, false]) {
      for (const romLabels of [true, false]) {
        for (const bytes of cases) {
          const data = new Uint8Array(bytes);
          const got = coreDisassemble(data, 0, 0x8000, 4, { hex, romLabels });
          const want = refDisassemble(data, 0, 0x8000, 4, { hex, romLabels });
          expect(got).toEqual(want);
        }
      }
    }
    // A stretch of real-looking code at a ROM-ish base, where labels apply.
    const rom = pseudoRandom(600, 3);
    expect(coreDisassemble(rom, 0, 0x0000, 200, {})).toEqual(refDisassemble(rom, 0, 0x0000, 200, {}));
    expect(coreDisassemble(rom, 13, 0x1234, 50, { hex: false })).toEqual(refDisassemble(rom, 13, 0x1234, 50, { hex: false }));
    // Calls and jumps into the ROM, which is what the labels are for.
    const calls = new Uint8Array([0xcd, 0x56, 0x05, 0xc3, 0x00, 0x00, 0xc7, 0x21, 0x56, 0x05, 0x18, 0xfe]);
    for (const hex of [true, false]) {
      expect(coreDisassemble(calls, 0, 0x8000, 6, { hex })).toEqual(refDisassemble(calls, 0, 0x8000, 6, { hex }));
    }
  });
});

describe('the Rust audio against the TypeScript it replaced', () => {
  const b = (id: number, fields: Record<string, unknown> = {}) => Object.assign(createBlock(id), fields) as Block;

  /** Every pulse source, plus the flow blocks that decide what plays when. */
  function tapes(): Block[][] {
    const data = new Uint8Array([0x00, 3, 65, 66, 67, 0x55]);
    return [
      [],
      [b(0x10, { data, pause: 100 })],
      [b(0x10, { data: new Uint8Array([0xff, 1, 2]), pause: 0 })],   // data block, no pause
      [b(0x11, { data, pause: 50, pilotLen: 100, usedBits: 3 })],
      [b(0x12, { count: 50 }), b(0x13, { pulses: [100, 200, 300] })],
      [b(0x14, { data, pause: 10, usedBits: 5 })],
      [b(0x15, { data: new Uint8Array([0b10110010, 0xff]), tstates: 79, usedBits: 4, pause: 5 })],
      [b(0x18, { data: new Uint8Array([3, 4, 5, 0, 0x10, 0x27, 0, 0]), sampleRate: 22050, compression: 1, pause: 5 })],
      [b(0x19, {
        pause: 20, totp: 2, npp: 2,
        pilotSymbols: [{ flags: 0, pulses: [2168] }, { flags: 1, pulses: [667, 735] }],
        pilotStream: [{ symbol: 0, reps: 10 }, { symbol: 1, reps: 1 }],
        totd: 16, npd: 2,
        dataSymbols: [{ flags: 2, pulses: [855, 855] }, { flags: 3, pulses: [1710, 1710] }],
        data: new Uint8Array([0xa5, 0x5a]),
      })],
      [b(0x2b, { level: 1 }), b(0x12, { count: 5 }), b(0x20, { pause: 1 })],
      [b(0x24, { count: 3 }), b(0x12, { count: 2 }), b(0x25), b(0x20, { pause: 1 })],
      [b(0x26, { offsets: [2, 3] }), b(0x20, { pause: 1 }), b(0x12, { count: 1 }), b(0x27)],
      [b(0x23, { offset: 2 }), b(0x12, { count: 99 }), b(0x20, { pause: 1 })],
      [b(0x2a), b(0x12, { count: 1 })],
      [b(0x20, { pause: 0 }), b(0x12, { count: 1 })],                 // stop the tape
      core.parseTape(sample('Tapeti demo.tzx')).blocks,
      core.parseTape(sample('Tapeti demo (variant).tzx')).blocks,
    ];
  }

  it('plays the blocks in the same order', () => {
    for (const blocks of tapes()) {
      expect(coreAudio.playbackOrder(blocks)).toEqual(refAudio.playbackOrder(blocks));
      expect(coreAudio.playbackOrder(blocks, { stopAt48k: true })).toEqual(refAudio.playbackOrder(blocks, { stopAt48k: true }));
    }
  });

  it('measures the same durations and timelines', () => {
    for (const blocks of tapes()) {
      for (const block of blocks) {
        expect(coreAudio.blockDuration(block)).toBe(refAudio.blockDuration(block));
      }
      expect(coreAudio.tapeDuration(blocks)).toEqual(refAudio.tapeDuration(blocks));
      const order = coreAudio.playbackOrder(blocks);
      expect(coreAudio.playbackTimeline(blocks, order)).toEqual(refAudio.playbackTimeline(blocks, order));
      for (const rate of [8000, 44100]) {
        expect(coreAudio.renderLength(blocks, rate, order)).toBe(refAudio.renderLength(blocks, rate, order));
      }
    }
  });

  it('emits the same pulses for every block', () => {
    for (const blocks of tapes()) {
      for (const block of blocks) {
        // The reference has no pulse list, so record one with its own sink.
        const recorded: { tstates: number; level: 0 | 1 }[] = [];
        let level: 0 | 1 = 0;
        const sink: refAudio.PulseSink = {
          get level() { return level; },
          set level(l) { level = l; },
          pulse(t) { recorded.push({ tstates: t, level }); level = level ? 0 : 1; },
          hold(t) { recorded.push({ tstates: t, level }); },
          setLevel(l) { level = l; },
        };
        refAudio.emitBlock(sink, block);
        expect(coreAudio.blockPulses(block)).toEqual(recorded);
      }
    }
  });

  it('renders the same samples, bit for bit', () => {
    for (const blocks of tapes()) {
      for (const mode of ['square', 'mic'] as const) {
        for (const sampleRate of [8000, 44100]) {
          const got = coreAudio.renderTape(blocks, { sampleRate, mode });
          const want = refAudio.renderTape(blocks, { sampleRate, mode });
          expectSameBuffer(got, want, `${mode} at ${sampleRate} Hz`);
        }
      }
      // And a non-default amplitude, which scales every sample.
      const got = coreAudio.renderTape(blocks, { sampleRate: 8000, mode: 'square', amplitude: 0.25 });
      const want = refAudio.renderTape(blocks, { sampleRate: 8000, mode: 'square', amplitude: 0.25 });
      expectSameBuffer(got, want, 'square at 8000 Hz, amplitude 0.25');
    }
  });

  it('writes the same WAV files', () => {
    for (const blocks of tapes().slice(0, 8)) {
      const samples = coreAudio.renderTape(blocks, { sampleRate: 8000, mode: 'square' });
      for (const bits of [8, 16] as const) {
        expect(Array.from(coreAudio.encodeWav(samples, 8000, bits))).toEqual(Array.from(refAudio.encodeWav(samples, 8000, bits)));
        // Rendering straight to WAV gives the same file as doing it in two steps.
        expect(Array.from(coreAudio.renderWav(blocks, { sampleRate: 8000, mode: 'square' }, bits)))
          .toEqual(Array.from(refAudio.encodeWav(samples, 8000, bits)));
      }
    }
    // Sample values at the edges round the way JavaScript rounds them.
    const edge = new Float32Array([-1, -0.5, -1 / 32767, 0, 1 / 32767, 0.5, 1, 1.5, -1.5]);
    for (const bits of [8, 16] as const) {
      expect(Array.from(coreAudio.encodeWav(edge, 8000, bits))).toEqual(Array.from(refAudio.encodeWav(edge, 8000, bits)));
    }
  });

  it('inflates and decodes CSW blocks the same way', () => {
    const rle = new Uint8Array([3, 4, 0, 0x10, 0x27, 0, 0, 7]);
    expect(coreAudio.decodeCswRle(rle)).toEqual(refAudio.decodeCswRle(rle));
    expect(coreAudio.decodeCswRle(new Uint8Array([0, 1, 2]))).toEqual(refAudio.decodeCswRle(new Uint8Array([0, 1, 2])));
    // A Z-RLE block is inflated on the TypeScript side and rendered identically.
    const zlib = deflate(rle);
    const csw = b(0x18, { data: zlib, compression: 2, sampleRate: 44100, pause: 0 });
    expect(coreAudio.blockDuration(csw)).toBe(refAudio.blockDuration(csw));
    expect(Array.from(coreAudio.renderTape([csw], { sampleRate: 8000, mode: 'square' })))
      .toEqual(Array.from(refAudio.renderTape([csw], { sampleRate: 8000, mode: 'square' })));
    // A corrupt one plays silence rather than garbage, on both sides.
    const broken = b(0x18, { data: new Uint8Array([1, 2, 3]), compression: 2, sampleRate: 44100, pause: 0 });
    expect(coreAudio.blockDuration(broken)).toBe(refAudio.blockDuration(broken));
  });

  it('finds the same position in the timeline', () => {
    const blocks = core.parseTape(sample('Tapeti demo.tzx')).blocks;
    const { starts, total } = coreAudio.playbackTimeline(blocks, coreAudio.playbackOrder(blocks));
    for (const t of [0, 1, 250, 100000, total - 1, total, total * 2]) {
      expect(coreAudio.positionAt(starts, t)).toBe(refAudio.positionAt(starts, t));
    }
    expect(coreAudio.positionAt([], 5)).toBe(refAudio.positionAt([], 5));
    expect(coreAudio.TSTATES_PER_SEC).toBe(refAudio.TSTATES_PER_SEC);
    expect(coreAudio.LEAD_TSTATES).toBe(refAudio.LEAD_TSTATES);
  });
});
