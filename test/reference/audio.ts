// The TypeScript audio.ts as it stood before the Rust core replaced it, kept as
// the reference implementation the core is tested against (test/core.test.ts).
// Not part of the app bundle, and not to gain features. See test/reference/parser.ts.
// Converts a block list into an edge/pulse stream, then into PCM samples.
// Loops, jumps, calls and returns are followed exactly like an emulator would.
import { Block, isUnknown, SymDef } from '../../src/tzx/types';
import { bitsPerSymbol } from '../../src/tzx/parser';
import { inflate } from 'pako';

export const TSTATES_PER_SEC = 3500000;

export interface PulseSink {
  /** Add a pulse of `tstates` length at the current level, then toggle the level. */
  pulse(tstates: number): void;
  /** Hold the current level for the given time without toggling. */
  hold(tstates: number): void;
  setLevel(level: 0 | 1): void;
  level: 0 | 1;
}

export interface FlowOptions {
  /** Called when a "stop the tape" (pause 0) or 48k stop is reached. */
  stopAt48k?: boolean;
  /** Safety valve for infinite loops via jumps. */
  maxSteps?: number;
}

/** Walk the tape in playback order, yielding the block index sequence. */
export function playbackOrder(blocks: Block[], opts: FlowOptions = {}): number[] {
  const order: number[] = [];
  const maxSteps = opts.maxSteps ?? 100000;
  const loopStack: { start: number; remaining: number }[] = [];
  const callStack: { block: number; next: number }[] = [];
  let i = 0;
  let steps = 0;
  while (i >= 0 && i < blocks.length && steps++ < maxSteps) {
    const b = blocks[i];
    order.push(i);
    switch (b.id) {
      case 0x23:
        i += (b as any).offset;
        continue;
      case 0x24:
        loopStack.push({ start: i, remaining: (b as any).count });
        break;
      case 0x25: {
        const l = loopStack[loopStack.length - 1];
        if (l) {
          l.remaining--;
          if (l.remaining > 0) {
            i = l.start + 1;
            continue;
          }
          loopStack.pop();
        }
        break;
      }
      case 0x26: {
        const offs: number[] = (b as any).offsets;
        if (offs.length > 0) {
          callStack.push({ block: i, next: 0 });
          i = i + offs[0];
          continue;
        }
        break;
      }
      case 0x27: {
        const c = callStack[callStack.length - 1];
        if (c) {
          c.next++;
          const offs: number[] = (blocks[c.block] as any).offsets;
          if (c.next < offs.length) {
            i = c.block + offs[c.next];
          } else {
            callStack.pop();
            i = c.block + 1;
          }
          continue;
        }
        break;
      }
      case 0x2a:
        if (opts.stopAt48k) return order;
        break;
      case 0x20:
        if ((b as any).pause === 0) return order;
        break;
    }
    i++;
  }
  return order;
}

function dataBits(data: Uint8Array, usedBits: number): number {
  if (data.length === 0) return 0;
  return (data.length - 1) * 8 + Math.max(1, Math.min(8, usedBits));
}

function emitPause(sink: PulseSink, ms: number) {
  if (ms <= 0) return;
  // Per spec: a pause forces the level low after at least 1 ms at the current level.
  const t = Math.round((ms * TSTATES_PER_SEC) / 1000);
  if (sink.level === 1) {
    sink.pulse(Math.min(t, 3500)); // end current high pulse after ~1ms
    sink.hold(Math.max(0, t - 3500));
  } else {
    sink.hold(t);
  }
}

function emitSymbol(sink: PulseSink, sym: SymDef) {
  switch (sym.flags & 3) {
    case 1:
      // same as current level: no edge -> first pulse prolongs previous; emulate by holding
      if (sym.pulses.length > 0) {
        sink.hold(sym.pulses[0]);
        for (let i = 1; i < sym.pulses.length; i++) sink.pulse(sym.pulses[i]);
      }
      return;
    case 2:
      sink.setLevel(0);
      break;
    case 3:
      sink.setLevel(1);
      break;
  }
  for (const p of sym.pulses) if (p > 0) sink.pulse(p);
}

/** Emit the pulses of one block (no flow control). */
export function emitBlock(sink: PulseSink, b: Block): void {
  if (isUnknown(b)) return;
  switch (b.id) {
    case 0x10: {
      const isHeader = b.data.length > 0 && b.data[0] < 128;
      emitStandard(sink, 2168, 667, 735, 855, 1710, isHeader ? 8063 : 3223, b.data, 8, b.pause);
      break;
    }
    case 0x11:
      emitStandard(sink, b.pilot, b.sync1, b.sync2, b.zero, b.one, b.pilotLen, b.data, b.usedBits, b.pause);
      break;
    case 0x12:
      for (let i = 0; i < b.count; i++) sink.pulse(b.pulseLen);
      break;
    case 0x13:
      for (const p of b.pulses) sink.pulse(p);
      break;
    case 0x14:
      emitStandard(sink, 0, 0, 0, b.zero, b.one, 0, b.data, b.usedBits, b.pause);
      break;
    case 0x15: {
      const bits = dataBits(b.data, b.usedBits);
      let run = 0;
      let cur: 0 | 1 = sink.level;
      for (let i = 0; i < bits; i++) {
        const bit = (b.data[i >> 3] >> (7 - (i & 7))) & 1;
        if (bit !== cur) {
          if (run > 0) {
            if (sink.level !== cur) sink.setLevel(cur);
            sink.hold(run * b.tstates);
          }
          cur = bit as 0 | 1;
          run = 0;
        }
        run++;
      }
      if (run > 0) {
        if (sink.level !== cur) sink.setLevel(cur);
        sink.hold(run * b.tstates);
      }
      emitPause(sink, b.pause);
      break;
    }
    case 0x18: {
      const pulses = decodeCswRle(cswRleData(b.data, b.compression));
      const scale = TSTATES_PER_SEC / (b.sampleRate || 44100);
      for (const p of pulses) sink.pulse(Math.round(p * scale));
      emitPause(sink, b.pause);
      break;
    }
    case 0x19: {
      if (b.totp > 0) {
        for (const run of b.pilotStream) {
          const sym = b.pilotSymbols[run.symbol];
          if (!sym) continue;
          for (let r = 0; r < run.reps; r++) emitSymbol(sink, sym);
        }
      }
      if (b.totd > 0 && b.dataSymbols.length > 0) {
        const nb = bitsPerSymbol(b.dataSymbols.length);
        let bitPos = 0;
        for (let s = 0; s < b.totd; s++) {
          let code = 0;
          for (let k = 0; k < nb; k++) {
            const byte = b.data[bitPos >> 3] ?? 0;
            code = (code << 1) | ((byte >> (7 - (bitPos & 7))) & 1);
            bitPos++;
          }
          const sym = b.dataSymbols[code];
          if (sym) emitSymbol(sink, sym);
        }
      }
      emitPause(sink, b.pause);
      break;
    }
    case 0x20:
      emitPause(sink, b.pause);
      break;
    case 0x2b:
      sink.setLevel(b.level ? 1 : 0);
      break;
  }
}

function emitStandard(
  sink: PulseSink, pilot: number, sync1: number, sync2: number, zero: number, one: number,
  pilotLen: number, data: Uint8Array, usedBits: number, pause: number,
) {
  for (let i = 0; i < pilotLen; i++) sink.pulse(pilot);
  if (sync1) sink.pulse(sync1);
  if (sync2) sink.pulse(sync2);
  const bits = dataBits(data, usedBits);
  for (let i = 0; i < bits; i++) {
    const bit = (data[i >> 3] >> (7 - (i & 7))) & 1;
    const len = bit ? one : zero;
    sink.pulse(len);
    sink.pulse(len);
  }
  emitPause(sink, pause);
}

/** Decode CSW v2 RLE pulse lengths (in samples). Z-RLE must be inflated first. */
export function decodeCswRle(data: Uint8Array): number[] {
  const out: number[] = [];
  let i = 0;
  while (i < data.length) {
    const v = data[i++];
    if (v !== 0) out.push(v);
    else {
      if (i + 4 > data.length) break;
      out.push((data[i] | (data[i + 1] << 8) | (data[i + 2] << 16)) + data[i + 3] * 0x1000000);
      i += 4;
    }
  }
  return out;
}

const inflated = new WeakMap<Uint8Array, Uint8Array>();

/** RLE bytes of a CSW block; Z-RLE (compression 2) is inflated with zlib and cached. */
export function cswRleData(data: Uint8Array, compression: number): Uint8Array {
  if (compression !== 2) return data;
  let out = inflated.get(data);
  if (!out) {
    try {
      out = inflate(data);
    } catch {
      out = new Uint8Array(0); // corrupt block: play silence rather than garbage
    }
    inflated.set(data, out);
  }
  return out;
}

/** Total T-states for a block in isolation. */
export function blockDuration(b: Block): number {
  const sink = new CountingSink();
  emitBlock(sink, b);
  return sink.total;
}

export class CountingSink implements PulseSink {
  total = 0;
  level: 0 | 1 = 0;
  pulse(t: number) {
    this.total += t;
    this.level = this.level ? 0 : 1;
  }
  hold(t: number) {
    this.total += t;
  }
  setLevel(l: 0 | 1) {
    this.level = l;
  }
}

/** Duration in seconds of the whole tape following playback flow. */
export function tapeDuration(blocks: Block[]): { seconds: number; order: number[] } {
  const order = playbackOrder(blocks);
  const sink = new CountingSink();
  for (const i of order) emitBlock(sink, blocks[i]);
  return { seconds: sink.total / TSTATES_PER_SEC, order };
}

export interface RenderOptions {
  sampleRate: number;
  /** 'square' = plain square wave, 'mic' = emulate the Spectrum MIC output high-pass response. */
  mode: 'square' | 'mic';
  amplitude?: number; // 0..1
}

/** Render pulses straight into a Float32Array of samples in [-1, 1]. */
export class SampleSink implements PulseSink {
  level: 0 | 1 = 0;
  private out: Float32Array;
  private n = 0;
  private acc = 0; // fractional sample accumulator in T-states
  private tPerSample: number;
  private amp: number;
  // MIC emulation: the Spectrum MIC socket is AC-coupled through a small capacitor,
  // so a step decays exponentially. Model as a first-order high-pass filter.
  private mic: boolean;
  private alpha: number;
  private prevIn = 0;
  private prevOut = 0;

  /** `capacity` is the expected sample count; the buffer grows if it is exceeded. */
  constructor(opts: RenderOptions, capacity = 0) {
    this.tPerSample = TSTATES_PER_SEC / opts.sampleRate;
    this.amp = opts.amplitude ?? 0.8;
    this.mic = opts.mode === 'mic';
    const rc = 0.0013; // seconds, roughly matches the observed decay
    const dt = 1 / opts.sampleRate;
    this.alpha = rc / (rc + dt);
    this.out = new Float32Array(Math.max(0, capacity));
  }
  private emit(tstates: number) {
    this.acc += tstates;
    const n = Math.floor(this.acc / this.tPerSample);
    this.acc -= n * this.tPerSample;
    if (n <= 0) return;
    const end = this.n + n;
    if (end > this.out.length) {
      const grown = new Float32Array(Math.max(end, this.out.length * 2, 1024));
      grown.set(this.out);
      this.out = grown;
    }
    const x = this.level ? 1 : -1;
    const out = this.out;
    if (!this.mic) {
      out.fill(x * this.amp, this.n, end);
    } else {
      const alpha = this.alpha;
      const amp = this.amp;
      let prevIn = this.prevIn;
      let prevOut = this.prevOut;
      for (let i = this.n; i < end; i++) {
        const y = alpha * (prevOut + x - prevIn);
        prevIn = x;
        prevOut = y;
        out[i] = Math.max(-1, Math.min(1, y)) * amp;
      }
      this.prevIn = prevIn;
      this.prevOut = prevOut;
    }
    this.n = end;
  }
  pulse(t: number) {
    this.emit(t);
    this.level = this.level ? 0 : 1;
  }
  hold(t: number) {
    this.emit(t);
  }
  setLevel(l: 0 | 1) {
    this.level = l;
  }
  /** Number of samples written so far. */
  get length(): number {
    return this.n;
  }
  finish(): Float32Array {
    return this.n === this.out.length ? this.out : this.out.slice(0, this.n);
  }
}

/** Silence before and after the rendered tape, in T-states. */
export const LEAD_TSTATES = TSTATES_PER_SEC / 2;

export interface Timeline {
  /** T-state at which each entry of `order` starts playing (after the lead-in). */
  starts: number[];
  /** Total T-states including lead-in and lead-out; equals the rendered length. */
  total: number;
}

/** Where each block of the playback order starts, without rendering. */
export function playbackTimeline(blocks: Block[], order: number[]): Timeline {
  const sink = new CountingSink();
  const starts: number[] = [];
  sink.hold(LEAD_TSTATES);
  for (const i of order) {
    starts.push(sink.total);
    emitBlock(sink, blocks[i]);
  }
  sink.hold(LEAD_TSTATES);
  return { starts, total: sink.total };
}

/** Index into the playback order of the block playing at `tstates` (the first block during lead-in). */
export function positionAt(starts: number[], tstates: number): number {
  let lo = 0;
  let hi = starts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid] <= tstates) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** Sample count `renderTape` will produce for the given order, without rendering. */
export function renderLength(blocks: Block[], sampleRate: number, order: number[]): number {
  return Math.floor(playbackTimeline(blocks, order).total / (TSTATES_PER_SEC / sampleRate));
}

export function renderTape(blocks: Block[], opts: RenderOptions, indices?: number[]): Float32Array {
  const order = indices ?? playbackOrder(blocks);
  // Counting first lets the sink allocate the whole buffer once: a plain array of numbers
  // cannot grow past ~134M elements, which a long tape at 44.1 kHz exceeds.
  const sink = new SampleSink(opts, renderLength(blocks, opts.sampleRate, order) + 1);
  sink.hold(LEAD_TSTATES);
  for (const i of order) emitBlock(sink, blocks[i]);
  sink.hold(LEAD_TSTATES);
  return sink.finish();
}

const LITTLE_ENDIAN = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;

/** Encode samples as a 16-bit mono PCM WAV file. */
export function encodeWav(samples: Float32Array, sampleRate: number, bits: 8 | 16 = 16): Uint8Array {
  const bytesPerSample = bits / 8;
  const dataLen = samples.length * bytesPerSample;
  const buf = new ArrayBuffer(44 + dataLen);
  const v = new DataView(buf);
  const w = (o: number, s: string) => {
    for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i));
  };
  w(0, 'RIFF');
  v.setUint32(4, 36 + dataLen, true);
  w(8, 'WAVE');
  w(12, 'fmt ');
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, 1, true);
  v.setUint32(24, sampleRate, true);
  v.setUint32(28, sampleRate * bytesPerSample, true);
  v.setUint16(32, bytesPerSample, true);
  v.setUint16(34, bits, true);
  w(36, 'data');
  v.setUint32(40, dataLen, true);
  if (bits === 16) {
    const pcm = new Int16Array(buf, 44, samples.length); // 44 is even, so the view is aligned
    for (let i = 0; i < samples.length; i++) {
      const s = Math.max(-1, Math.min(1, samples[i]));
      pcm[i] = Math.round(s * 32767);
    }
    if (!LITTLE_ENDIAN) for (let i = 0; i < samples.length; i++) v.setInt16(44 + i * 2, pcm[i], true);
  } else {
    const pcm = new Uint8Array(buf, 44, samples.length);
    for (let i = 0; i < samples.length; i++) {
      const s = Math.max(-1, Math.min(1, samples[i]));
      pcm[i] = Math.round((s + 1) * 127.5);
    }
  }
  return new Uint8Array(buf);
}
