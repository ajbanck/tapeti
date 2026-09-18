import { describe, it, expect } from 'vitest';
import { parseTzx, parseTap, isTzx } from '../src/tzx/parser';
import { serializeTzx, serializeTap, requiredVersion, saveVersion } from '../src/tzx/writer';
import { createBlock, Block, CREATABLE_IDS, isUnknown } from '../src/tzx/types';
import { decodeHeader, encodeHeader, describeBlock } from '../src/tzx/describe';
import { checkConsistency } from '../src/tzx/consistency';
import { tapeDuration, playbackOrder, renderTape, renderLength, encodeWav, TSTATES_PER_SEC, playbackTimeline, positionAt, blockDuration, LEAD_TSTATES } from '../src/tzx/audio';
// The sinks moved into the Rust core; the frozen TypeScript still has them, and
// this file's reference rendering is built on that.
import { emitBlock, SampleSink, PulseSink } from './reference/audio';
import { disassemble } from '../src/spectrum/z80dis';
import { listBasic, decodeNumber } from '../src/spectrum/basic';
import { compareTapes } from '../src/tzx/compare';

function header(name: string, type = 3, length = 100, p1 = 32768, p2 = 0) {
  return encodeHeader({ type, typeName: '', name, length, param1: p1, param2: p2 });
}

describe('TZX round trip', () => {
  it('serializes every creatable block and parses it back identically', () => {
    const blocks: Block[] = CREATABLE_IDS.map((id) => createBlock(id));
    // Give data blocks some content
    for (const b of blocks) {
      if ('data' in b) (b as any).data = new Uint8Array([0x00, 3, 65, 66, 67, 0x55]);
    }
    const g = blocks.find((b) => b.id === 0x19)!;
    (g as any).totp = 2;
    (g as any).pilotSymbols = [{ flags: 0, pulses: [2168] }, { flags: 0, pulses: [667, 735] }];
    (g as any).pilotStream = [{ symbol: 0, reps: 8063 }, { symbol: 1, reps: 1 }];
    (g as any).totd = 48;
    const bytes = serializeTzx(blocks);
    expect(isTzx(bytes)).toBe(true);
    const parsed = parseTzx(bytes);
    expect(parsed.warnings).toEqual([]);
    expect(parsed.blocks.length).toBe(blocks.length);
    const again = serializeTzx(parsed.blocks);
    expect(Array.from(again)).toEqual(Array.from(bytes));
    parsed.blocks.forEach((b, i) => {
      const { uid: _a, ...x } = b as any;
      const { uid: _b, ...y } = blocks[i] as any;
      if (x.id === 0x19) {
        expect(x.pilotStream).toEqual(y.pilotStream);
        expect(x.pilotSymbols).toEqual(y.pilotSymbols);
      } else {
        expect(JSON.stringify(x, replacer)).toEqual(JSON.stringify(y, replacer));
      }
    });
  });

  it('preserves unknown blocks', () => {
    const raw = new Uint8Array([0x5a + 1, 3, 0, 0, 0, 1, 2, 3]); // unknown ID 0x5B, dword length 3
    const file = new Uint8Array([...Array.from('ZXTape!\x1a').map((c) => c.charCodeAt(0)), 1, 20, ...raw, 0x20, 0xe8, 0x03]);
    const parsed = parseTzx(file);
    expect(parsed.blocks.length).toBe(2);
    expect(parsed.blocks[0].id).toBe(0x5b);
    expect(parsed.blocks[1].id).toBe(0x20);
    expect(Array.from(serializeTzx(parsed.blocks, { major: 1, minor: 20 }))).toEqual(Array.from(file));
  });

  it('reports a CSW block whose length cannot hold its own header', () => {
    // len 4 is below the 10-byte CSW header; reading len - 10 bytes used to move
    // the read position backwards.
    const raw = new Uint8Array([0x18, 4, 0, 0, 0, 1, 2, 3, 4]);
    const file = new Uint8Array([...Array.from('ZXTape!\x1a').map((c) => c.charCodeAt(0)), 1, 20, ...raw]);
    const parsed = parseTzx(file);
    expect(parsed.warnings).toEqual([
      'Block 1 (ID 18) at offset 10: CSW block length 4 is shorter than its 10-byte header',
    ]);
    expect(parsed.blocks.length).toBe(1);
    expect(parsed.blocks[0].id).toBe(0x18);
    expect(isUnknown(parsed.blocks[0])).toBe(true);
    // The corrupt bytes are kept, so the file still round-trips.
    expect(Array.from(serializeTzx(parsed.blocks, { major: 1, minor: 20 }))).toEqual(Array.from(file));
  });

  it('computes the lowest possible version', () => {
    expect(requiredVersion([createBlock(0x10)])).toEqual({ major: 1, minor: 0 });
    expect(requiredVersion([createBlock(0x24)])).toEqual({ major: 1, minor: 10 });
    expect(requiredVersion([createBlock(0x2a)])).toEqual({ major: 1, minor: 12 });
    expect(requiredVersion([createBlock(0x19)])).toEqual({ major: 1, minor: 20 });
  });

  it('keeps the loaded version when it is newer than the blocks need', () => {
    const plain = [createBlock(0x10)];
    expect(saveVersion(plain, null)).toEqual({ major: 1, minor: 0 });
    expect(saveVersion(plain, { major: 1, minor: 20 })).toEqual({ major: 1, minor: 20 });
    expect(saveVersion([createBlock(0x19)], { major: 1, minor: 0 })).toEqual({ major: 1, minor: 20 });
    // an unaltered 1.20 tape of plain blocks round-trips byte for byte
    const bytes = serializeTzx(plain, { major: 1, minor: 20 });
    const p = parseTzx(bytes);
    expect(Array.from(serializeTzx(p.blocks, saveVersion(p.blocks, { major: p.major, minor: p.minor })))).toEqual(Array.from(bytes));
  });
});

describe('TAP', () => {
  it('parses and writes TAP files', () => {
    const h = header('TEST      ', 0, 10, 10, 10);
    const tap = new Uint8Array([19, 0, ...h, 3, 0, 0xff, 1, 0xfe]);
    const parsed = parseTap(tap);
    expect(parsed.blocks.length).toBe(2);
    expect(describeBlock(parsed.blocks[0], false)).toBe('Std speed Prog: TEST; L: 10, P: 10, S: 10');
    const out = serializeTap(parsed.blocks);
    expect(Array.from(out.bytes)).toEqual(Array.from(tap));
  });
});

describe('headers', () => {
  it('encodes and decodes headers', () => {
    const h = decodeHeader(header('SCREEN$   ', 3, 6912, 16384, 32768))!;
    expect(h.type).toBe(3);
    expect(h.name.trim()).toBe('SCREEN$');
    expect(h.length).toBe(6912);
    expect(h.param1).toBe(16384);
  });
});

describe('flow', () => {
  it('follows loops and jumps', () => {
    const blocks: Block[] = [createBlock(0x24), createBlock(0x12), createBlock(0x25), createBlock(0x20)];
    (blocks[0] as any).count = 3;
    const order = playbackOrder(blocks);
    expect(order).toEqual([0, 1, 2, 1, 2, 1, 2, 3]);
  });
  it('estimates duration of a standard block', () => {
    const b = createBlock(0x10) as any;
    b.data = header('X');
    b.pause = 1000;
    const { seconds } = tapeDuration([b]);
    // pilot 8063*2168 + sync + 19*8 bits*2*~... + 1s pause -> roughly 6.1 s
    expect(seconds).toBeGreaterThan(5.5);
    expect(seconds).toBeLessThan(6.6);
  });
  it('finds structural problems', () => {
    const blocks: Block[] = [createBlock(0x21), createBlock(0x24), createBlock(0x22)];
    const issues = checkConsistency(blocks);
    expect(issues.some((i) => i.message.includes('crosses'))).toBe(true);
    expect(issues.some((i) => i.message.includes('never closed'))).toBe(true);
    const inf: Block[] = [createBlock(0x23)];
    (inf[0] as any).offset = 0;
    expect(checkConsistency(inf).some((i) => i.severity === 'error')).toBe(true);
  });
  it('renders WAV', () => {
    const b = createBlock(0x12) as any;
    b.count = 100;
    const s = renderTape([b], { sampleRate: 8000, mode: 'square' });
    const wav = encodeWav(s, 8000);
    expect(wav.length).toBe(44 + s.length * 2);
    expect(String.fromCharCode(...wav.subarray(0, 4))).toBe('RIFF');
  });
});

describe('sample rendering', () => {
  // Reference implementation: the original sink that collected samples in a plain array
  // and filtered afterwards. The rewritten sink must produce identical output.
  function reference(blocks: Block[], opts: { sampleRate: number; mode: 'square' | 'mic'; amplitude?: number }): Float32Array {
    const samples: number[] = [];
    let level: 0 | 1 = 0;
    let acc = 0;
    const tPerSample = TSTATES_PER_SEC / opts.sampleRate;
    const sink: PulseSink = {
      get level() { return level; },
      set level(l) { level = l; },
      pulse(t) { this.hold(t); level = level ? 0 : 1; },
      hold(t) {
        acc += t;
        const n = Math.floor(acc / tPerSample);
        acc -= n * tPerSample;
        for (let i = 0; i < n; i++) samples.push(level ? 1 : -1);
      },
      setLevel(l) { level = l; },
    };
    sink.hold(TSTATES_PER_SEC / 2);
    for (const i of playbackOrder(blocks)) emitBlock(sink, blocks[i]);
    sink.hold(TSTATES_PER_SEC / 2);
    const amp = opts.amplitude ?? 0.8;
    const out = new Float32Array(samples.length);
    if (opts.mode === 'square') {
      for (let i = 0; i < out.length; i++) out[i] = samples[i] * amp;
      return out;
    }
    const rc = 0.0013;
    const dt = 1 / opts.sampleRate;
    const alpha = rc / (rc + dt);
    let prevIn = 0;
    let prevOut = 0;
    for (let i = 0; i < out.length; i++) {
      const y = alpha * (prevOut + samples[i] - prevIn);
      prevIn = samples[i];
      prevOut = y;
      out[i] = Math.max(-1, Math.min(1, y)) * amp;
    }
    return out;
  }

  function mixedTape(): Block[] {
    const std = createBlock(0x10) as any;
    std.data = header('MIX');
    std.pause = 50;
    const tone = createBlock(0x12) as any;
    tone.count = 33;
    tone.pulseLen = 1000;
    const seq = createBlock(0x13) as any;
    seq.pulses = [300, 1234, 5];
    const direct = createBlock(0x15) as any;
    direct.data = new Uint8Array([0b10110010, 0b00001111]);
    direct.usedBits = 8;
    direct.tstates = 79;
    direct.pause = 3;
    const lvl = createBlock(0x2b) as any;
    lvl.level = 1;
    const pause = createBlock(0x20) as any;
    pause.pause = 7;
    return [std, tone, seq, lvl, pause, direct];
  }

  it('matches the reference renderer sample for sample', () => {
    const blocks = mixedTape();
    for (const mode of ['square', 'mic'] as const) {
      for (const sampleRate of [8000, 44100]) {
        const got = renderTape(blocks, { sampleRate, mode });
        const want = reference(blocks, { sampleRate, mode });
        expect(got.length).toBe(want.length);
        expect(got).toEqual(want);
      }
    }
  });
  it('preallocates the exact length and grows when the guess is short', () => {
    const blocks = mixedTape();
    const order = playbackOrder(blocks);
    const rendered = renderTape(blocks, { sampleRate: 8000, mode: 'square' });
    expect(Math.abs(rendered.length - renderLength(blocks, 8000, order))).toBeLessThanOrEqual(1);
    // A sink with no capacity must grow and still end up identical.
    const small = new SampleSink({ sampleRate: 8000, mode: 'mic' }, 0);
    small.hold(TSTATES_PER_SEC / 2);
    for (const i of order) emitBlock(small, blocks[i]);
    small.hold(TSTATES_PER_SEC / 2);
    expect(small.length).toBe(rendered.length);
    expect(small.finish()).toEqual(renderTape(blocks, { sampleRate: 8000, mode: 'mic' }));
  });
  it('MIC mode decays after an edge, square mode holds', () => {
    const tone = createBlock(0x12) as any;
    tone.count = 2;
    tone.pulseLen = 35000; // 10 ms per pulse
    const sq = renderTape([tone], { sampleRate: 8000, mode: 'square' });
    const mic = renderTape([tone], { sampleRate: 8000, mode: 'mic' });
    // 0.5 s lead-in (4000 samples) and one low pulse (80 samples), then the level goes high.
    const edge = 4080;
    expect(sq[edge - 1]).toBeCloseTo(-0.8);
    expect(sq[edge]).toBeCloseTo(0.8);
    expect(sq[edge + 40]).toBeCloseTo(0.8);
    expect(Math.abs(mic[edge])).toBeGreaterThan(Math.abs(mic[edge + 40]));
    expect(Math.abs(mic[edge + 40])).toBeGreaterThan(0);
  });
  it('lays blocks out on a timeline that matches the rendered length', () => {
    const blocks = mixedTape();
    const order = playbackOrder(blocks);
    const { starts, total } = playbackTimeline(blocks, order);
    expect(starts.length).toBe(order.length);
    expect(starts[0]).toBe(LEAD_TSTATES);
    let t = LEAD_TSTATES;
    order.forEach((i, k) => {
      expect(starts[k]).toBe(t);
      t += blockDuration(blocks[i]);
    });
    expect(total).toBe(t + LEAD_TSTATES);
    const rendered = renderTape(blocks, { sampleRate: 8000, mode: 'square' });
    expect(Math.abs(rendered.length - total / (TSTATES_PER_SEC / 8000))).toBeLessThan(1);
    // Repeated blocks (loops) get one entry per pass.
    const loop: Block[] = [createBlock(0x24), createBlock(0x12), createBlock(0x25)];
    (loop[0] as any).count = 2;
    const twice = playbackTimeline(loop, playbackOrder(loop));
    expect(twice.starts.length).toBe(5); // 0, 1, 2, 1, 2
    expect(twice.starts[3] - twice.starts[1]).toBe(blockDuration(loop[1]) + blockDuration(loop[2]));
  });
  it('finds the block playing at a time', () => {
    const starts = [100, 250, 250, 900];
    expect(positionAt(starts, 0)).toBe(0); // lead-in counts as the first block
    expect(positionAt(starts, 100)).toBe(0);
    expect(positionAt(starts, 249)).toBe(0);
    expect(positionAt(starts, 250)).toBe(2); // zero-length block 1 is skipped over
    expect(positionAt(starts, 899)).toBe(2);
    expect(positionAt(starts, 5000)).toBe(3);
    expect(positionAt([], 5)).toBe(0);
  });
  it('encodes 16-bit and 8-bit PCM little-endian', () => {
    const s = new Float32Array([0, 1, -1, 0.5, 2, -2]);
    const w16 = encodeWav(s, 8000, 16);
    const v = new DataView(w16.buffer);
    expect(v.getUint16(34, true)).toBe(16);
    expect(v.getUint32(40, true)).toBe(12);
    expect([0, 1, 2, 3, 4, 5].map((i) => v.getInt16(44 + i * 2, true))).toEqual([0, 32767, -32767, 16384, 32767, -32767]);
    const w8 = encodeWav(s, 8000, 8);
    expect(w8.length).toBe(44 + 6);
    expect(Array.from(w8.subarray(44))).toEqual([128, 255, 0, 191, 255, 0]);
  });
});

describe('spectrum', () => {
  it('disassembles common instructions', () => {
    const code = new Uint8Array([0x21, 0x00, 0x40, 0xcd, 0x56, 0x05, 0xdd, 0x7e, 0x02, 0xdd, 0xcb, 0x01, 0x46, 0xed, 0xb0, 0x18, 0xfe, 0xc9]);
    const lines = disassemble(code, 0, 0x8000, 10);
    expect(lines[0].text).toBe('LD HL,0x4000');
    expect(lines[1].text).toBe('CALL 0x0556  ; LD-BYTES');
    expect(lines[2].text).toBe('LD A,(IX+0x02)');
    expect(lines[3].text).toBe('BIT 0,(IX+0x01)');
    expect(lines[4].text).toBe('LDIR');
    expect(lines[5].text).toBe('JR 0x800F');
    expect(lines[6].text).toBe('RET');
  });
  it('decodes numbers and lists BASIC', () => {
    expect(decodeNumber(new Uint8Array([0, 0, 10, 0, 0]), 0)).toBe(10);
    expect(decodeNumber(new Uint8Array([0, 0xff, 0xf6, 0xff, 0]), 0)).toBe(-10);
    expect(decodeNumber(new Uint8Array([0x81, 0x00, 0, 0, 0]), 0)).toBe(1);
    expect(decodeNumber(new Uint8Array([0x80, 0x00, 0, 0, 0]), 0)).toBe(0.5);
    // 10 PRINT "HI": GO TO 10
    const line = [0, 10, 0, 0, 0xf5, 0x22, 0x48, 0x49, 0x22, 0x3a, 0xec, 0x31, 0x30, 0x0e, 0, 0, 10, 0, 0, 0x0d];
    line[2] = line.length - 4;
    const lines = listBasic(new Uint8Array(line), 0, line.length, { showNumbers: false, basic128: false, speccyFormat: false });
    expect(lines.length).toBe(1);
    expect(lines[0].number).toBe(10);
    expect(lines[0].tokens.map((t) => t.text).join('')).toBe('PRINT "HI": GO TO 10');
  });
});

describe('compare', () => {
  it('compares tapes by data', () => {
    const a = createBlock(0x10) as any;
    a.data = new Uint8Array([1, 2, 3]);
    const b = createBlock(0x11) as any;
    b.data = new Uint8Array([1, 2, 3]);
    const r = compareTapes([a], [createBlock(0x30), b], 'data', 'datablocks');
    expect(r.left).toEqual(['same']);
    expect(r.right).toEqual(['ignored', 'same']);
    expect(compareTapes([a], [b], 'data+timings', 'all').identical).toBe(false);
  });
});

function replacer(_k: string, v: any) {
  if (v instanceof Uint8Array) return Array.from(v);
  return v;
}

import { detectContent, basicScore } from '../src/tzx/content';
describe('content detection', () => {
  const std = (data: Uint8Array) => { const b = createBlock(0x10) as any; b.data = data; return b as Block; };
  const withFlag = (body: number[]) => { const d = new Uint8Array(body.length + 2); d[0] = 0xff; d.set(body, 1); d[d.length - 1] = 0; return d; };
  const basicLine = [0, 10, 2, 0, 0xf5, 0x0d];
  it('detects headers, BASIC and screens from a preceding header', () => {
    const hdr = std(header('PROG      ', 0, 6, 10, 6));
    const prog = std(withFlag(basicLine));
    const scrHdr = std(header('SCREEN$   ', 3, 6912, 16384, 32768));
    const scr = std(withFlag(new Array(6912).fill(0)));
    const blocks = [hdr, prog, scrHdr, scr];
    expect(detectContent(blocks, 0).kind).toBe('header');
    expect(detectContent(blocks, 1)).toMatchObject({ kind: 'basic', base: 23755, progLen: 6, source: 'header' });
    expect(detectContent(blocks, 3)).toMatchObject({ kind: 'screen', base: 16384, source: 'header' });
  });
  it('detects a screen loaded away from 16384', () => {
    const hdr = std(header('P         ', 3, 6912, 40000, 32768));
    const scr = std(withFlag(new Array(6912).fill(0)));
    expect(detectContent([hdr, scr], 1)).toMatchObject({ kind: 'screen', label: 'SCREEN 40000', base: 16384, source: 'header' });
  });
  it('uses the header when the data length differs, and marks it', () => {
    const hdr = std(header('d         ', 3, 6912, 16384, 32768));
    const short = std(withFlag(new Array(5643).fill(0)));
    expect(detectContent([hdr, short], 1)).toMatchObject({ kind: 'screen', label: 'SCREEN (short)', base: 16384, expectedLength: 6912 });
    const code = std(header('c         ', 3, 100, 32768, 32768));
    expect(detectContent([code, std(withFlag(new Array(120).fill(0)))], 1)).toMatchObject({ kind: 'code', label: 'CODE 32768 (long)' });
    // A block that is exactly screen-sized is still guessed as a screen
    expect(detectContent([code, std(withFlag(new Array(6912).fill(0)))], 1)).toMatchObject({ kind: 'screen', label: 'SCREEN?' });
    const issues = checkConsistency([hdr, short]);
    expect(issues.some((i) => i.block === 1 && /1269 bytes shorter/.test(i.message))).toBe(true);
  });
  it('falls back to heuristics without a header', () => {
    expect(detectContent([std(withFlag(basicLine))], 0)).toMatchObject({ kind: 'basic', source: 'heuristic' });
    expect(detectContent([std(withFlag(new Array(6912).fill(1)))], 0)).toMatchObject({ kind: 'screen', source: 'heuristic' });
    expect(detectContent([std(withFlag([0xc3, 0x00, 0x80, 0x21, 0x00, 0x40, 0x11]))], 0).kind).toBe('data');
    expect(basicScore(new Uint8Array([0x27, 0x10, 5, 0, 1, 2, 3, 4, 5]))).toBe(0);
  });
});

import { renderScreen } from '../src/spectrum/screen';
describe('screen rendering', () => {
  it('uses the default attribute where the data ends before the attribute area', () => {
    const bitmap = new Uint8Array(6144);
    bitmap[0] = 0x80; // top-left pixel set
    const px = renderScreen(bitmap, 0);
    expect(Array.from(px.slice(0, 3))).toEqual([0, 0, 0]); // ink: black
    expect(Array.from(px.slice(4, 7))).toEqual([0xd7, 0xd7, 0xd7]); // paper: white
  });
});
