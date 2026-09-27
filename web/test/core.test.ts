// Golden tests of the core's TypeScript face: parsing, writing, descriptions, content
// detection, consistency, programs, comparison, conversion, POKEs, bits, the Spectrum
// side (charset, screen, BASIC, disassembler) and audio.
//
// The expected values are `__snapshots__/core.test.ts.snap`, recorded from the core. A
// changed answer fails here until `npm test -- -u` records the new one, so the diff is
// the review. Bulky answers (rendered screens and samples, the opcode sweep) are kept as
// SHA-1 digests.
import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { deflate } from 'pako';
import * as core from '../src/tzx/core';
import {
  checkConsistency,
  convertBlock,
  disassemble,
  serializeTzx,
  serializeTap,
  encodeHeader,
} from '../src/tzx/core';
import { Block, CREATABLE_IDS, createBlock, isDataBlock } from '../src/tzx/types';

const SAMPLES = fileURLToPath(new URL('../../core/tests/samples', import.meta.url));
const sample = (name: string) => new Uint8Array(fs.readFileSync(path.join(SAMPLES, name)));
const sampleNames = () => fs.readdirSync(SAMPLES).sort();

const hex = (b: Uint8Array) => Buffer.from(b.buffer, b.byteOffset, b.byteLength).toString('hex');

/** SHA-1 of a buffer or a string, for answers too large to keep in the snapshot. */
function digest(x: Uint8Array | Uint8ClampedArray | Float32Array | string): string {
  const h = createHash('sha1');
  h.update(typeof x === 'string' ? x : Buffer.from(x.buffer, x.byteOffset, x.byteLength));
  return h.digest('hex');
}

/**
 * A value as the snapshot keeps it: byte arrays as hex strings, maps as sorted
 * entries, and no uids, which are handed out per parse and always differ.
 */
function plain(v: unknown): unknown {
  if (v instanceof Uint8Array) return hex(v);
  if (v instanceof Map) return [...v.entries()].sort((a, b) => a[0] - b[0]);
  if (Array.isArray(v)) return v.map(plain);
  if (v && typeof v === 'object') {
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v)) if (k !== 'uid') out[k] = plain(x);
    return out;
  }
  return v;
}

const sig = Array.from('ZXTape!\x1a').map((c) => c.charCodeAt(0));
const tzx = (...body: number[]) => new Uint8Array([...sig, 1, 20, ...body]);

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

describe('parsing', () => {
  it('parses the sample tapes', () => {
    for (const name of sampleNames()) {
      const tape = core.parseTape(sample(name));
      expect(tape.blocks.length).toBeGreaterThan(0);
      expect(tape.warnings).toEqual([]);
      expect(plain(tape)).toMatchSnapshot(name);
    }
  });

  it('parses every creatable block type back to what was written', () => {
    const blocks = everyBlock();
    const bytes = serializeTzx(blocks);
    const tape = core.parseTzx(bytes);
    expect(tape.blocks.length).toBe(blocks.length);
    expect(plain(tape.blocks)).toEqual(plain(blocks));
    expect(hex(serializeTzx(tape.blocks))).toBe(hex(bytes));
  });

  it('parses text, entries and high-bit characters', () => {
    const blocks: Block[] = [createBlock(0x21), createBlock(0x30), createBlock(0x31), createBlock(0x32), createBlock(0x33), createBlock(0x35)];
    (blocks[0] as any).name = 'Grüße';           // Latin-1 above 0x7f
    (blocks[1] as any).text = 'Line one\rLine two';
    (blocks[2] as any).text = 'Press \x7fENTER';
    (blocks[3] as any).entries = [{ type: 0, text: 'Full title' }, { type: 0xff, text: 'Comment' }];
    (blocks[4] as any).entries = [{ type: 0, id: 1, info: 0 }, { type: 4, id: 0, info: 3 }];
    (blocks[5] as any).ident = 'POKEs           ';
    expect(plain(core.parseTzx(serializeTzx(blocks)))).toMatchSnapshot();
  });

  it('parses a select block and a call sequence', () => {
    const blocks: Block[] = [createBlock(0x28), createBlock(0x26), createBlock(0x23)];
    (blocks[0] as any).entries = [{ offset: 3, text: 'Side A' }, { offset: -2, text: 'Side B' }];
    (blocks[1] as any).offsets = [1, -1, 300];
    (blocks[2] as any).offset = -5;
    expect(plain(core.parseTzx(serializeTzx(blocks)))).toMatchSnapshot();
  });

  it('parses TAP files, including the broken ones', () => {
    const h = encodeHeader({ type: 0, typeName: '', name: 'TEST      ', length: 10, param1: 10, param2: 10 });
    const cases: [string, Uint8Array][] = [
      ['header and data', new Uint8Array([19, 0, ...h, 3, 0, 0xff, 1, 0xfe])],
      ['truncated block', new Uint8Array([4, 0, 1, 2])],
      ['trailing byte', new Uint8Array([2, 0, 1, 2, 9])],
      ['empty file', new Uint8Array(0)],
    ];
    for (const [what, bytes] of cases) expect(plain(core.parseTap(bytes))).toMatchSnapshot(what);
    // Auto-detection sends a file without the TZX signature to the TAP parser too.
    expect(plain(core.parseTape(cases[0][1]))).toEqual(plain(core.parseTap(cases[0][1])));
  });

  it('parses damaged TZX files', () => {
    const cases: [string, number[]][] = [
      ['unknown block, then a pause', [0x5b, 3, 0, 0, 0, 1, 2, 3, 0x20, 0xe8, 0x03]],
      ['data block that claims more than it has', [0x10, 0xe8, 0x03, 10, 0, 1, 2]],
      ['CSW length below its own header', [0x18, 4, 0, 0, 0, 1, 2, 3, 4]],
      ['deprecated emulation info', [0x34, 1, 2, 3, 4, 5, 6, 7, 8]],
      ['deprecated snapshot', [0x40, 0, 2, 0, 0, 0xaa, 0xbb]],
      ['deprecated C64 block', [0x16, 2, 0, 0, 0, 1, 2]],
      ['group start with a truncated name', [0x21, 5]],
      ['generalized block cut short', [0x19, 8, 0, 0, 0, 0xe8, 0x03, 0, 0]],
    ];
    for (const [what, body] of cases) {
      const tape = core.parseTzx(tzx(...body));
      expect(tape.blocks.length).toBeGreaterThan(0);
      expect(plain(tape)).toMatchSnapshot(what);
    }
  });

  it('parses an empty tape and a file that is only a header', () => {
    expect(core.isTzx(tzx())).toBe(true);
    expect(plain(core.parseTzx(tzx()))).toEqual({ blocks: [], major: 1, minor: 20, warnings: [] });
  });

  it('refuses a TZX parse of something without the signature', () => {
    const bytes = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    expect(() => core.parseTzx(bytes)).toThrow('Not a TZX file (missing ZXTape! signature)');
  });

  it('round-trips a TAP export', () => {
    const tape = core.parseTape(sample('Tapeti demo.tap'));
    expect(hex(serializeTap(tape.blocks).bytes)).toBe(hex(sample('Tapeti demo.tap')));
  });
});

describe('writing', () => {
  /** Edge cases the writer must handle: over-long text, short idents, odd glue. */
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

  /** Everything the writer says about a block list, in one snapshot. */
  function written(blocks: Block[], version?: { major: number; minor: number }) {
    const tap = core.serializeTap(blocks);
    return {
      tzx: hex(core.serializeTzx(blocks, version)),
      tap: hex(tap.bytes),
      skipped: tap.skipped,
      requiredVersion: core.requiredVersion(blocks),
      saveVersion: [null, { major: 1, minor: 0 }, { major: 1, minor: 20 }].map((loaded) => core.saveVersion(blocks, loaded)),
      blocks: blocks.map((b) => hex(core.serializeBlock(b))),
    };
  }

  it('writes every creatable block', () => {
    expect(written(everyBlock())).toMatchSnapshot('lowest version');
    expect(written(everyBlock(), { major: 1, minor: 20 })).toMatchSnapshot('version 1.20');
  });

  it('writes the awkward blocks', () => {
    expect(written(awkwardBlocks())).toMatchSnapshot();
  });

  it('writes the sample tapes block by block as it writes them whole', () => {
    for (const name of sampleNames()) {
      const bytes = sample(name);
      const tape = core.parseTape(bytes);
      const parts = tape.blocks.map((b) => core.serializeBlock(b));
      const joined = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
      let at = 0;
      for (const p of parts) {
        joined.set(p, at);
        at += p.length;
      }
      // A TZX file is its 10-byte header and then the blocks; a TAP has no header,
      // and its blocks carry their own length words instead of a block ID.
      if (!name.endsWith('.tap')) expect(hex(joined)).toBe(hex(bytes.subarray(10)));
      expect({
        requiredVersion: core.requiredVersion(tape.blocks),
        saveVersion: core.saveVersion(tape.blocks, { major: tape.major, minor: tape.minor }),
        skipped: core.serializeTap(tape.blocks).skipped,
      }).toMatchSnapshot(name);
    }
  });

  it('writes an unknown block and an empty tape', () => {
    const unknown = core.parseTzx(tzx(0x5b, 3, 0, 0, 0, 1, 2, 3)).blocks;
    expect(written(unknown)).toMatchSnapshot('unknown block');
    expect(written([])).toMatchSnapshot('empty tape');
  });

  it('round-trips the sample tapes byte for byte', () => {
    for (const name of sampleNames()) {
      const bytes = sample(name);
      const tape = core.parseTape(bytes);
      const out = name.endsWith('.tap')
        ? core.serializeTap(tape.blocks).bytes
        : core.serializeTzx(tape.blocks, { major: tape.major, minor: tape.minor });
      expect(hex(out)).toBe(hex(bytes));
    }
  });
});

describe('descriptions, detection and structure', () => {
  /** Every creatable block plus a few with realistic core. */
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

  /** One line per block: ID, both spellings of the description, the length column. */
  const described = (blocks: Block[]) =>
    blocks.map((b) => `${b.id.toString(16)} | ${core.describeBlock(b, false)} | ${core.describeBlock(b, true)} | ${core.blockLength(b)}`);

  it('describes every block, in both number bases', () => {
    expect(described(blocksForDescription())).toMatchSnapshot();
  });

  it('describes the blocks of the sample tapes', () => {
    for (const name of sampleNames()) {
      expect(described(core.parseTape(sample(name)).blocks)).toMatchSnapshot(name);
    }
  });

  it('decodes and encodes ROM headers', () => {
    for (const type of [0, 1, 2, 3, 4]) {
      const h = { type, typeName: '', name: 'name      ', length: 100, param1: 32768, param2: 10 };
      const bytes = core.encodeHeader(h);
      const decoded = core.decodeHeader(bytes);
      if (type < 4) expect({ ...decoded, typeName: '' }).toEqual(h);
      expect({
        bytes: hex(bytes),
        decoded,
        checksum: core.checksum(bytes),
        partial: core.checksum(bytes, 1, 5),
      }).toMatchSnapshot(`type ${type}`);
    }
    // Not a header: wrong length, wrong flag, unknown type.
    for (const bad of [new Uint8Array(0), new Uint8Array(19).fill(9), new Uint8Array(18)]) {
      expect(core.decodeHeader(bad)).toBeNull();
    }
  });

  it('detects the content of every block of every sample tape', () => {
    for (const name of sampleNames()) {
      const blocks = core.parseTape(sample(name)).blocks;
      const labels = core.contentLabels(blocks);
      const found = blocks.map((b, i) => {
        const info = core.detectContent(blocks, i);
        // A direct recording's bytes are samples, not a ROM block: nothing is detected
        // and nothing is stripped, as for a CSW block.
        if (b.id === 0x15) expect([info.skipFlag, info.skipChecksum, labels[i]]).toEqual([false, false, '']);
        expect(labels[i]).toBe(isDataBlock(b) ? info.label : '');
        return info;
      });
      expect(found).toMatchSnapshot(name);
    }
  });

  it('detects the content of the awkward cases', () => {
    const withFlag = (body: number[]) => new Uint8Array([0xff, ...body, 0]);
    const std = (data: Uint8Array) => ({ ...createBlock(0x10), data } as Block);
    const hdr = (type: number, length: number, p1: number) =>
      std(new Uint8Array([0x00, ...encodeHeader({ type, typeName: '', name: 'x         ', length, param1: p1, param2: 0 }).subarray(1, 18), 0x55]));
    const cases: [string, Block[]][] = [
      ['screen at 16384', [hdr(3, 6912, 16384), std(withFlag(new Array(6912).fill(0)))]],
      ['screen at 40000', [hdr(3, 6912, 40000), std(withFlag(new Array(6912).fill(0)))]],
      ['short', [hdr(3, 6912, 16384), std(withFlag(new Array(100).fill(0)))]],
      ['long', [hdr(3, 120, 32768), std(withFlag(new Array(130).fill(0)))]],
      ['BASIC', [hdr(0, 40, 0), std(withFlag(new Array(40).fill(0)))]],
      ['number array', [hdr(1, 40, 0x4100), std(withFlag(new Array(40).fill(0)))]],
      ['character array', [hdr(2, 40, 0x0100), std(withFlag(new Array(40).fill(0)))]],
      ['BASIC by heuristic', [std(withFlag([0x00, 0x0a, 0x05, 0x00, 1, 2, 3, 4, 0x0d]))]],
      ['screen by heuristic', [std(withFlag(new Array(6912).fill(1)))]],
      ['plain data', [std(withFlag([0xc3, 0x00, 0x80, 0x21, 0x00, 0x40, 0x11]))]],
      ['empty', [std(new Uint8Array(0))]],
      ['pure data', [{ ...createBlock(0x14), data: new Uint8Array([1, 2, 3]) } as Block]],
      ['not a data block', [createBlock(0x20)]],
    ];
    for (const [what, blocks] of cases) {
      const infos = blocks.map((_b, i) => core.detectContent(blocks, i));
      expect(core.contentLabels(blocks)).toEqual(blocks.map((b, i) => (isDataBlock(b) ? infos[i].label : '')));
      const first = blocks[0].id === 0x10 ? (blocks[0] as any).data : new Uint8Array(0);
      expect({ infos, basicScore: core.basicScore(first) }).toMatchSnapshot(what);
    }
    // Out of range indices answer with the default.
    expect(core.detectContent([], 0)).toEqual(core.detectContent(cases[0][1], 9));
    expect(core.detectContent([], 0)).toMatchSnapshot('out of range');
  });

  it('finds consistency issues', () => {
    const b = (id: number, fields: Record<string, unknown> = {}) => Object.assign(createBlock(id), fields) as Block;
    const cases: [string, Block[]][] = [
      ['empty tape', []],
      ['crossing', [b(0x21), b(0x24), b(0x22)]],
      ['nested group', [b(0x21), b(0x21), b(0x22), b(0x22)]],
      ['loop counts', [b(0x24, { count: 0 }), b(0x25), b(0x24, { count: 1 }), b(0x25)]],
      ['ends without starts', [b(0x22), b(0x25)]],
      ['jump to itself', [b(0x23, { offset: 0 })]],
      ['jump outside the tape', [b(0x23, { offset: 99 })]],
      ['call sequences', [b(0x26, { offsets: [] }), b(0x26, { offsets: [0, 99] })]],
      ['select outside the tape', [b(0x28, { entries: [{ offset: 99, text: 'x' }] })]],
      ['turbo block with no data', [b(0x11, { usedBits: 0, data: new Uint8Array(0) })]],
      ['direct recording out of range', [b(0x15, { usedBits: 9, tstates: 0 })]],
      ['bad checksum', [b(0x10, { data: new Uint8Array([0x00, 1, 2, 3]) })]],
      ['empty tone and sequence', [b(0x12, { count: 0 }), b(0x13, { pulses: [] })]],
      ['generalized block', [b(0x19, { totp: 2, pilotStream: [{ symbol: 5, reps: 1 }], pilotSymbols: [], totd: 8, dataSymbols: [], data: new Uint8Array(0) })]],
      ['empty info blocks', [b(0x32, { entries: [] }), b(0x33, { entries: [] })]],
      ['a healthy loop', [b(0x24, { count: 3 }), b(0x12), b(0x25), b(0x20)]],
      ['a call that returns', [b(0x26, { offsets: [1] }), b(0x12), b(0x27)]],
      ['a call that never returns', [b(0x26, { offsets: [1] }), b(0x12)]],
      ['never closed', [b(0x21)]],
      ['infinite jump loop', [b(0x23, { offset: 1 }), b(0x23, { offset: -1 })]],
    ];
    for (const [what, blocks] of cases) {
      expect({ from0: checkConsistency(blocks, 0), from1: checkConsistency(blocks, 1) }).toMatchSnapshot(what);
    }
    for (const name of sampleNames()) {
      expect(checkConsistency(core.parseTape(sample(name)).blocks)).toMatchSnapshot(name);
    }
  });

  // The two predicates the TypeScript implements itself. The same tables are
  // asserted in core/tests/logic.rs, so the copies cannot drift apart.
  it('classifies metadata blocks the way the core does', () => {
    const metadata = [0x21, 0x22, 0x30, 0x31, 0x32, 0x33, 0x35, 0x5a];
    for (const id of CREATABLE_IDS) {
      expect(core.isMetadata(createBlock(id))).toBe(metadata.includes(id));
    }
    const unknown = (id: number) => core.parseTzx(tzx(id, 0, 0, 0, 0)).blocks[0];
    expect(core.isMetadata(unknown(0x5b))).toBe(true);
    expect(core.isMetadata(unknown(0x16))).toBe(false);
    expect(core.isMetadata(unknown(0x17))).toBe(false);
  });

  it('strips flag and checksum bytes the way the core does', () => {
    const d = new Uint8Array([0xff, 1, 2, 3, 0x55]);
    expect(Array.from(core.blockBody(d, true, true))).toEqual([1, 2, 3]);
    expect(Array.from(core.blockBody(d, true, false))).toEqual([1, 2, 3, 0x55]);
    expect(Array.from(core.blockBody(d, false, true))).toEqual([0xff, 1, 2, 3]);
    expect(Array.from(core.blockBody(d, false, false))).toEqual([0xff, 1, 2, 3, 0x55]);
    expect(Array.from(core.blockBody(new Uint8Array(0), true, true))).toEqual([]);
    expect(Array.from(core.blockBody(new Uint8Array([7]), true, true))).toEqual([]);
  });

  it('finds groups, programs and titles', () => {
    const b = (id: number, fields: Record<string, unknown> = {}) => Object.assign(createBlock(id), fields) as Block;
    const prog = (name: string) =>
      b(0x10, { data: new Uint8Array([0x00, ...encodeHeader({ type: 0, typeName: '', name, length: 10, param1: 0, param2: 10 }).subarray(1, 18), 0x55]) });
    const cases: [string, Block[]][] = [
      ['empty tape', []],
      ['two programs', [prog('A         '), b(0x10), prog('B         '), b(0x10)]],
      ['groups', [b(0x21, { name: 'Game' }), prog('A         '), b(0x22), b(0x21, { name: 'Empty' }), b(0x22)]],
      ['loop in a group', [b(0x21), b(0x24), b(0x25), b(0x22)]],
      ['groups in a loop', [b(0x24), b(0x21), b(0x22), b(0x25), b(0x21), b(0x22)]],
      ['select', [b(0x28, { entries: [{ offset: 2, text: 'Side B' }, { offset: 99, text: 'Nope' }] }), b(0x10), prog('C         ')]],
      ['archive title', [b(0x32, { entries: [{ type: 0, text: 'The Tape' }] }), b(0x10)]],
      ['archive without a title', [b(0x32, { entries: [{ type: 1, text: 'No title' }] }), b(0x10)]],
      ['lead in', [b(0x30, { text: 'lead in' }), prog('A         '), b(0x20), prog('B         ')]],
      ['blank names', [b(0x21, { name: '' }), prog('          '), b(0x22)]],
    ];
    const structure = (blocks: Block[]) => ({
      ranges: plain(core.groupRanges(blocks)),
      programs: core.detectPrograms(blocks),
      title: core.tapeTitle(blocks),
    });
    for (const [what, blocks] of cases) expect(structure(blocks)).toMatchSnapshot(what);
    for (const name of sampleNames()) expect(structure(core.parseTape(sample(name)).blocks)).toMatchSnapshot(name);
  });
});

describe('the dropdown tables', () => {
  it('offers the archive kinds, hardware classes and wordings', () => {
    expect({ archive: core.archiveTypes(), info: core.hardwareInfo(), hardware: core.hardwareTypes() }).toMatchSnapshot();
  });
});

describe('conversion, comparison, bits and POKEs', () => {
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
      ...core.parseTzx(tzx(0x5b, 2, 0, 0, 0, 7, 8)).blocks,
    ];
  }

  it('converts between every pair of block types', () => {
    specimens().forEach((b, i) => {
      const converted = CREATABLE_IDS.map((id) => {
        const got = convertBlock(b, id);
        expect(got.uid).toBe(b.uid);
        return plain(got);
      });
      // Its own type, or an unknown block: the same object comes back.
      if (CREATABLE_IDS.includes(b.id)) expect(convertBlock(b, b.id)).toBe(b);
      expect(converted).toMatchSnapshot(`specimen ${i}, id ${b.id.toString(16)}`);
    });
  });

  it('compares blocks in every mode', () => {
    const blocks = specimens();
    const modes = ['data', 'data+timings', 'data+timings+pauses'] as const;
    for (const mode of modes) {
      // One row per block, one character per block it is compared with.
      const matrix = blocks.map((a) => blocks.map((b) => (core.blocksEqual(a, b, mode) ? '1' : '.')).join(''));
      expect(matrix).toMatchSnapshot(mode);
    }
    // Same block type, different pause: equal until pauses count.
    const x = { ...createBlock(0x10), pause: 1, data: new Uint8Array([1]) } as Block;
    const y = { ...createBlock(0x10), pause: 2, data: new Uint8Array([1]) } as Block;
    expect(core.blocksEqual(x, y, 'data+timings')).toBe(true);
    expect(core.blocksEqual(x, y, 'data+timings+pauses')).toBe(false);
  });

  it('compares tapes in every mode', () => {
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
    // Each result as one line: the first letter of every block's verdict on each side.
    const initials = (r: core.CompareResult[]) => r.map((x) => x[0]).join('');
    const lines: string[] = [];
    tapes.forEach((left, l) => {
      tapes.forEach((right, r) => {
        for (const bm of blockModes) {
          for (const tm of tapeModes) {
            const got = core.compareTapes(left, right, bm, tm);
            lines.push(`${l} vs ${r} ${bm} ${tm}: ${got.identical ? 'identical' : 'differ'} ${initials(got.left)} | ${initials(got.right)}`);
          }
        }
      });
    });
    expect(lines).toMatchSnapshot();
  });

  it('finds matching blocks', () => {
    const blocks = core.parseTape(sample('Tapeti demo.tzx')).blocks;
    expect(blocks.map((needle) => core.findMatches(needle, blocks, 'data'))).toMatchSnapshot('data');
    expect(blocks.map((needle) => core.findMatches(needle, blocks, 'data+timings'))).toMatchSnapshot('data+timings');
    // A needle from somewhere else is not skipped, so an identical block matches.
    const other = core.parseTape(sample('Tapeti demo.tzx')).blocks[1];
    expect(core.findMatches(other, blocks, 'data')).toContain(1);
  });

  it('drops, adds, shifts, joins and flips bits', () => {
    const streams: core.BitData[] = [
      { data: new Uint8Array(0), usedBits: 8 },
      { data: new Uint8Array([0b10110011]), usedBits: 8 },
      { data: new Uint8Array([0b10110011]), usedBits: 3 },
      { data: new Uint8Array([0xff, 0x00, 0xa5]), usedBits: 5 },
      { data: new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9]), usedBits: 1 },
    ];
    const show = (d: core.BitData) => `${hex(d.data)}/${d.usedBits}`;
    const ops = ['dropBits', 'addBits', 'shiftLeftBits', 'shiftRightBits'] as const;
    const lines: string[] = [];
    for (const d of streams) {
      lines.push(`${show(d)}: ${core.totalBits(d)} bits, flipped ${hex(core.flipBytes(d.data))}`);
      for (const op of ops) {
        lines.push(`  ${op}: ${[0, 1, 3, 8, 17, 100].map((n) => `${n} -> ${show(core[op](d, n))}`).join(', ')}`);
      }
    }
    for (const parts of [streams, streams.slice(1, 3), [streams[1]], streams.slice(0, 1)]) {
      lines.push(`join ${parts.map(show).join(' + ')} -> ${show(core.joinBits(parts))}`);
    }
    expect(lines).toMatchSnapshot();
  });

  it('reads and writes POKEs blocks', () => {
    const infos: core.PokesInfo[] = [
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
      const bytes = core.encodePokes(info);
      expect(core.decodePokes(bytes)).toEqual(info);
      expect({ bytes: hex(bytes), dec: core.pokesToText(info, false), hex: core.pokesToText(info, true) }).toMatchSnapshot();
    }
    // A truncated block fails with the offset it ran out at.
    expect(() => core.decodePokes(new Uint8Array([3, 65, 66]))).toThrow('Unexpected end of file at offset 1');
  });

  it('parses POKEs text, including the ways it can go wrong', () => {
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
      for (const hexMode of [false, true]) {
        const info = core.textToPokes(text, hexMode);
        expect({ info, bytes: hex(core.encodePokes(info)) }).toMatchSnapshot(`${JSON.stringify(text)} ${hexMode ? 'hex' : 'dec'}`);
      }
    }
    const errors = ['nonsense', 'POKE', 'POKE 1', 'POKE 1,', 'POKE ,2', 'POKE 1,2,3', 'POKE zz,2'].map((bad) => {
      try {
        core.textToPokes(bad, false);
        return `${bad}: accepted`;
      } catch (e) {
        return `${bad}: ${(e as Error).message}`;
      }
    });
    expect(errors.some((e) => e.endsWith(': accepted'))).toBe(false);
    expect(errors).toMatchSnapshot();
  });
});

describe('the Spectrum side', () => {
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

  it('maps every character', () => {
    const codes = Array.from({ length: 256 }, (_, c) => c);
    expect(codes.map((c) => core.zxChar(c))).toMatchSnapshot('zx');
    expect(codes.map((c) => core.zxChar(c, false))).toMatchSnapshot('zx plain');
    expect(codes.map((c) => core.dumpChar(c))).toMatchSnapshot('dump');
  });

  it('renders screens', () => {
    const screens: [string, Uint8Array, number][] = [
      ['noise', pseudoRandom(6912), 0],
      ['other noise', pseudoRandom(6912, 99), 0],
      ['blank', new Uint8Array(6912), 0],
      ['far too short', pseudoRandom(100), 0],
      ['offset into the data', pseudoRandom(6912), 17],
      ['base address before the block', pseudoRandom(6912), -50],
      ['empty', new Uint8Array(0), 0],
    ];
    for (const [what, data, offset] of screens) {
      const renders: Record<string, string> = {};
      for (const opts of [{}, { hideAttributes: true }, { flashPhase: true }, { hideAttributes: true, flashPhase: true }]) {
        const pixels = core.renderScreen(data, offset, opts);
        expect(pixels.length).toBe(256 * 192 * 4);
        renders[JSON.stringify(opts)] = digest(pixels);
      }
      expect({ renders, flash: core.hasFlash(data, offset) }).toMatchSnapshot(what);
    }
  });

  it('decodes and formats Sinclair numbers', () => {
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
    expect(cases.map((c) => {
      const v = core.decodeNumber(new Uint8Array(c), 0);
      return `${hex(new Uint8Array(c))} = ${v} = ${core.formatNumber(v)}`;
    })).toMatchSnapshot('decoded');
    // Out of range gives NaN, which formats as '?'.
    expect(core.decodeNumber(new Uint8Array(3), 0)).toBeNaN();
    expect(core.formatNumber(NaN)).toBe('?');
    // And the formatting itself, over the shapes it has to get right.
    const numbers = [
      0, 1, -1, 42, -42, 1e14, -1e14, 1e15, 1e16, 0.5, -0.5, 1 / 3, 2 / 3, Math.PI, Math.E,
      1e-7, 1e-6, 1.5e-7, 123456789, 12345678.5, 0.1, 0.2, 0.30000000000000004, 1e21, -1e21,
      1e-21, 255.00390625, 6.02e23, Infinity, -Infinity,
    ];
    expect(numbers.map((v) => `${v} -> ${core.formatNumber(v)}`)).toMatchSnapshot('formatted');
  });

  it('lists a BASIC program', () => {
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
      const lines = core.listBasic(prog, 0, prog.length, opts);
      expect({ lines, text: core.basicToText(lines, opts) }).toMatchSnapshot(JSON.stringify(opts));
    }
    // A slice of noise must not throw, and lists the same way every time.
    const noise = pseudoRandom(400, 7);
    expect(allOpts.map((opts) => digest(core.basicToText(core.listBasic(noise, 0, noise.length, opts), opts)))).toMatchSnapshot('noise');
  });

  it('lists variables', () => {
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
    expect(core.listVariables(vars, 0, vars.length)).toMatchSnapshot('every type');
    // Character arrays, a zero-dimension array and garbage.
    const chars = new Uint8Array([head(0b110, 'f'), 0x07, 0x00, 0x01, 0x04, 0x00, 0x68, 0x69, 0x21, 0x3f, 0x80]);
    expect(core.listVariables(chars, 0, chars.length)).toMatchSnapshot('character array');
    const broken = new Uint8Array([head(0b010, 'g'), 0x03, 0x00, 0x00, 0x80]);
    expect(core.listVariables(broken, 0, broken.length)).toMatchSnapshot('zero dimensions');
    const garbage = new Uint8Array([0x01, 0x02, 0x03]);
    expect(core.listVariables(garbage, 0, garbage.length)).toMatchSnapshot('garbage');
    expect(core.listVariables(new Uint8Array(0), 0, 0)).toEqual([]);
  });

  it('disassembles every opcode', () => {
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
    const asText = (lines: ReturnType<typeof disassemble>) =>
      lines.map((l) => `${l.addr.toString(16)} ${hex(new Uint8Array(l.bytes))} ${l.text}${l.target === undefined ? '' : ` -> ${l.target.toString(16)}`}`);
    // The sweep is thousands of lines: kept as a digest per option set, with the
    // first few cases in the clear so a change shows what kind it is.
    for (const hexMode of [true, false]) {
      for (const romLabels of [true, false]) {
        const all = cases.map((bytes) => asText(disassemble(new Uint8Array(bytes), 0, 0x8000, 4, { hex: hexMode, romLabels })).join('\n'));
        expect({ digest: digest(all.join('\n\n')), first: all.slice(0, 8) }).toMatchSnapshot(`hex ${hexMode}, romLabels ${romLabels}`);
      }
    }
    // Prefix chains and a run that falls off the end of the data.
    for (const bytes of [[0xdd, 0xfd, 0xdd, 0x21, 0x00, 0x40], [0xdd], [0xed], [0xcb], [0x21], [0x18]]) {
      expect(asText(disassemble(new Uint8Array(bytes), 0, 0x8000, 4, {}))).toMatchSnapshot(hex(new Uint8Array(bytes)));
    }
    // A stretch of real-looking code at a ROM-ish base, where labels apply.
    const rom = pseudoRandom(600, 3);
    expect(asText(disassemble(rom, 0, 0x0000, 200, {}))).toMatchSnapshot('noise at 0');
    expect(asText(disassemble(rom, 13, 0x1234, 50, { hex: false }))).toMatchSnapshot('noise at 1234, decimal');
    // Calls and jumps into the ROM, which is what the labels are for.
    const calls = new Uint8Array([0xcd, 0x56, 0x05, 0xc3, 0x00, 0x00, 0xc7, 0x21, 0x56, 0x05, 0x18, 0xfe]);
    for (const hexMode of [true, false]) {
      expect(asText(disassemble(calls, 0, 0x8000, 6, { hex: hexMode }))).toMatchSnapshot(`ROM calls, hex ${hexMode}`);
    }
  });
});

describe('audio', () => {
  const b = (id: number, fields: Record<string, unknown> = {}) => Object.assign(createBlock(id), fields) as Block;

  /** Every pulse source, plus the flow blocks that decide what plays when. */
  function tapes(): [string, Block[]][] {
    const data = new Uint8Array([0x00, 3, 65, 66, 67, 0x55]);
    return [
      ['empty', []],
      ['standard', [b(0x10, { data, pause: 100 })]],
      ['standard, no pause', [b(0x10, { data: new Uint8Array([0xff, 1, 2]), pause: 0 })]],
      ['turbo', [b(0x11, { data, pause: 50, pilotLen: 100, usedBits: 3 })]],
      ['tone and sequence', [b(0x12, { count: 50 }), b(0x13, { pulses: [100, 200, 300] })]],
      ['pure data', [b(0x14, { data, pause: 10, usedBits: 5 })]],
      ['direct recording', [b(0x15, { data: new Uint8Array([0b10110010, 0xff]), tstates: 79, usedBits: 4, pause: 5 })]],
      ['CSW', [b(0x18, { data: new Uint8Array([3, 4, 5, 0, 0x10, 0x27, 0, 0]), sampleRate: 22050, compression: 1, pause: 5 })]],
      ['generalized', [b(0x19, {
        pause: 20, totp: 2, npp: 2,
        pilotSymbols: [{ flags: 0, pulses: [2168] }, { flags: 1, pulses: [667, 735] }],
        pilotStream: [{ symbol: 0, reps: 10 }, { symbol: 1, reps: 1 }],
        totd: 16, npd: 2,
        dataSymbols: [{ flags: 2, pulses: [855, 855] }, { flags: 3, pulses: [1710, 1710] }],
        data: new Uint8Array([0xa5, 0x5a]),
      })]],
      ['set level', [b(0x2b, { level: 1 }), b(0x12, { count: 5 }), b(0x20, { pause: 1 })]],
      ['loop', [b(0x24, { count: 3 }), b(0x12, { count: 2 }), b(0x25), b(0x20, { pause: 1 })]],
      ['call', [b(0x26, { offsets: [2, 3] }), b(0x20, { pause: 1 }), b(0x12, { count: 1 }), b(0x27)]],
      ['jump', [b(0x23, { offset: 2 }), b(0x12, { count: 99 }), b(0x20, { pause: 1 })]],
      ['stop in 48k', [b(0x2a), b(0x12, { count: 1 })]],
      ['stop the tape', [b(0x20, { pause: 0 }), b(0x12, { count: 1 })]],
      ['Tapeti demo.tzx', core.parseTape(sample('Tapeti demo.tzx')).blocks],
      ['Tapeti demo (variant).tzx', core.parseTape(sample('Tapeti demo (variant).tzx')).blocks],
    ];
  }

  it('plays the blocks in order', () => {
    for (const [what, blocks] of tapes()) {
      expect({
        order: core.playbackOrder(blocks),
        stopAt48k: core.playbackOrder(blocks, { stopAt48k: true }),
      }).toMatchSnapshot(what);
    }
  });

  it('measures durations and timelines', () => {
    for (const [what, blocks] of tapes()) {
      const order = core.playbackOrder(blocks);
      expect({
        blocks: blocks.map((block) => core.blockDuration(block)),
        tape: core.tapeDuration(blocks),
        timeline: core.playbackTimeline(blocks, order),
        samples: [8000, 44100].map((rate) => core.renderLength(blocks, rate, order)),
      }).toMatchSnapshot(what);
    }
  });

  it('emits the pulses of every block', () => {
    for (const [what, blocks] of tapes()) {
      // A pilot tone is thousands of pulses: those are kept as a count and a digest.
      const pulses = blocks.map((block) => {
        const p = core.blockPulses(block);
        return p.length <= 40 ? p : { count: p.length, digest: digest(JSON.stringify(p)) };
      });
      expect(pulses).toMatchSnapshot(what);
    }
  });

  it('renders samples', () => {
    for (const [what, blocks] of tapes()) {
      const renders: Record<string, string> = {};
      for (const mode of ['square', 'mic'] as const) {
        for (const sampleRate of [8000, 44100]) {
          const samples = core.renderTape(blocks, { sampleRate, mode });
          renders[`${mode} ${sampleRate}`] = `${samples.length} samples ${digest(samples)}`;
        }
      }
      // And a non-default amplitude, which scales every sample.
      const quiet = core.renderTape(blocks, { sampleRate: 8000, mode: 'square', amplitude: 0.25 });
      renders['square 8000 at 0.25'] = `${quiet.length} samples ${digest(quiet)}`;
      expect(renders).toMatchSnapshot(what);
    }
  });

  it('writes WAV files', () => {
    for (const [what, blocks] of tapes().slice(0, 8)) {
      const samples = core.renderTape(blocks, { sampleRate: 8000, mode: 'square' });
      const files: Record<string, string> = {};
      for (const bitsPerSample of [8, 16] as const) {
        const wav = core.encodeWav(samples, 8000, bitsPerSample);
        // Rendering straight to WAV gives the same file as doing it in two steps.
        expect(hex(core.renderWav(blocks, { sampleRate: 8000, mode: 'square' }, bitsPerSample))).toBe(hex(wav));
        files[`${bitsPerSample} bit`] = `${wav.length} bytes, header ${hex(wav.subarray(0, 44))}, ${digest(wav)}`;
      }
      expect(files).toMatchSnapshot(what);
    }
    // Sample values at the edges round the way JavaScript rounds them.
    const edge = new Float32Array([-1, -0.5, -1 / 32767, 0, 1 / 32767, 0.5, 1, 1.5, -1.5]);
    for (const bitsPerSample of [8, 16] as const) {
      expect(hex(core.encodeWav(edge, 8000, bitsPerSample))).toMatchSnapshot(`edge values, ${bitsPerSample} bit`);
    }
  });

  it('inflates and decodes CSW blocks', () => {
    const rle = new Uint8Array([3, 4, 0, 0x10, 0x27, 0, 0, 7]);
    expect(core.decodeCswRle(rle)).toEqual([3, 4, 10000, 7]);
    expect(core.decodeCswRle(new Uint8Array([0, 1, 2]))).toMatchSnapshot('truncated long pulse');
    // A Z-RLE block is inflated on the TypeScript side and rendered like a plain one.
    const zlib = deflate(rle);
    const csw = b(0x18, { data: zlib, compression: 2, sampleRate: 44100, pause: 0 });
    const plainCsw = b(0x18, { data: rle, compression: 1, sampleRate: 44100, pause: 0 });
    expect(core.blockDuration(csw)).toBe(core.blockDuration(plainCsw));
    expect(hex(core.renderWav([csw], { sampleRate: 8000, mode: 'square' })))
      .toBe(hex(core.renderWav([plainCsw], { sampleRate: 8000, mode: 'square' })));
    expect(core.blockDuration(csw)).toMatchSnapshot('duration');
    // A corrupt one plays silence rather than garbage.
    const broken = b(0x18, { data: new Uint8Array([1, 2, 3]), compression: 2, sampleRate: 44100, pause: 0 });
    expect(core.blockDuration(broken)).toBe(0);
  });

  it('finds the position in the timeline', () => {
    const blocks = core.parseTape(sample('Tapeti demo.tzx')).blocks;
    const { starts, total } = core.playbackTimeline(blocks, core.playbackOrder(blocks));
    // The obvious linear scan, which the binary search must agree with.
    const scan = (t: number) => Math.max(0, starts.filter((s) => s <= t).length - 1);
    for (const t of [0, 1, 250, 100000, total - 1, total, total * 2]) {
      expect(core.positionAt(starts, t)).toBe(scan(t));
    }
    expect(core.positionAt([], 5)).toBe(0);
    expect(core.TSTATES_PER_SEC).toBe(3500000);
    expect(core.LEAD_TSTATES).toBe(1750000);
  });
});
