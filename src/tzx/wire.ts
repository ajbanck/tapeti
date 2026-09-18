// Decoder for the byte format the Rust core answers in; `core/src/wire.rs` is
// the encoder and documents the layout. Fields are read in the order the block
// literals below declare them, which is the order `parser.ts` used to build
// them in, so key order (and therefore anything comparing JSON) is unchanged.
import { Reader, Writer } from './bytes';
import {
  ArchiveEntry, BasicLine, BasicToken, BitData, Block, CompareResult, ContentInfo, ContentKind,
  DisLine, HardwareEntry, HeaderInfo, Issue, isUnknown, newUid, ParsedTape, PilotRun, Poke,
  PokesInfo, Program, SelectEntry, SymDef, Trainer, VariableEntry,
} from './types';

export const WIRE_VERSION = 1;
const UNKNOWN_TAG = 0xff;

/** The header every answer starts with; throws on an error payload. */
function readHeader(r: Reader): void {
  const version = r.u8();
  if (version !== WIRE_VERSION) {
    throw new Error(`Tape core speaks wire format ${version}, this build expects ${WIRE_VERSION}`);
  }
  if (r.u8() === 1) throw new Error(str(r));
}

/** A byte string answer: a serialized tape or block. */
export function decodeBytes(buf: Uint8Array): Uint8Array {
  const r = new Reader(buf);
  readHeader(r);
  return bytes(r);
}

/** A TAP answer: the bytes plus the indices of the blocks left out. */
export function decodeTap(buf: Uint8Array): { bytes: Uint8Array; skipped: number[] } {
  const r = new Reader(buf);
  readHeader(r);
  const out = bytes(r);
  const skipped: number[] = [];
  for (let n = r.u32(); n > 0; n--) skipped.push(r.u32());
  return { bytes: out, skipped };
}

/** A TZX version answer. */
export function decodeVersion(buf: Uint8Array): { major: number; minor: number } {
  const r = new Reader(buf);
  readHeader(r);
  return { major: r.u8(), minor: r.u8() };
}

export function decodeTape(buf: Uint8Array): ParsedTape {
  const r = new Reader(buf);
  readHeader(r);
  const major = r.u8();
  const minor = r.u8();
  const warnings: string[] = [];
  for (let n = r.u32(); n > 0; n--) warnings.push(str(r));
  const blocks: Block[] = [];
  for (let n = r.u32(); n > 0; n--) blocks.push(block(r));
  return { blocks, major, minor, warnings };
}

// Strings on the wire are UTF-8, not the Latin-1 of the tape itself: the
// character table carries block glyphs. `Reader.str` stays Latin-1 for tape bytes.
const utf8 = { decode: new TextDecoder(), encode: new TextEncoder() };

function str(r: Reader): string {
  return utf8.decode.decode(r.bytes(r.u32()));
}

function bytes(r: Reader): Uint8Array {
  return r.bytes(r.u32());
}

function u16s(r: Reader): number[] {
  const out: number[] = [];
  for (let n = r.u32(); n > 0; n--) out.push(r.u16());
  return out;
}

function symDefs(r: Reader): SymDef[] {
  const out: SymDef[] = [];
  for (let n = r.u32(); n > 0; n--) out.push({ flags: r.u8(), pulses: u16s(r) });
  return out;
}

function block(r: Reader): Block {
  const uid = newUid();
  const tag = r.u8();
  if (tag === UNKNOWN_TAG) {
    const id = r.u8();
    return { uid, id, unknown: true, raw: bytes(r) };
  }
  const id = tag;
  switch (id) {
    case 0x10:
      return { uid, id, pause: r.u16(), data: bytes(r) };
    case 0x11:
      return {
        uid, id, pilot: r.u16(), sync1: r.u16(), sync2: r.u16(), zero: r.u16(), one: r.u16(),
        pilotLen: r.u16(), usedBits: r.u8(), pause: r.u16(), data: bytes(r),
      };
    case 0x12:
      return { uid, id, pulseLen: r.u16(), count: r.u16() };
    case 0x13:
      return { uid, id, pulses: u16s(r) };
    case 0x14:
      return { uid, id, zero: r.u16(), one: r.u16(), usedBits: r.u8(), pause: r.u16(), data: bytes(r) };
    case 0x15:
      return { uid, id, tstates: r.u16(), pause: r.u16(), usedBits: r.u8(), data: bytes(r) };
    case 0x18:
      return {
        uid, id, pause: r.u16(), sampleRate: r.u32(), compression: r.u8(), pulseCount: r.u32(),
        data: bytes(r),
      };
    case 0x19: {
      const pause = r.u16();
      const totp = r.u32();
      const npp = r.u8();
      const pilotSymbols = symDefs(r);
      const pilotStream: PilotRun[] = [];
      for (let n = r.u32(); n > 0; n--) pilotStream.push({ symbol: r.u8(), reps: r.u16() });
      const totd = r.u32();
      const npd = r.u8();
      const dataSymbols = symDefs(r);
      return { uid, id, pause, totp, npp, pilotSymbols, pilotStream, totd, npd, dataSymbols, data: bytes(r) };
    }
    case 0x20:
      return { uid, id, pause: r.u16() };
    case 0x21:
      return { uid, id, name: str(r) };
    case 0x22:
      return { uid, id };
    case 0x23:
      return { uid, id, offset: r.i16() };
    case 0x24:
      return { uid, id, count: r.u16() };
    case 0x25:
      return { uid, id };
    case 0x26: {
      const offsets: number[] = [];
      for (let n = r.u32(); n > 0; n--) offsets.push(r.i16());
      return { uid, id, offsets };
    }
    case 0x27:
      return { uid, id };
    case 0x28: {
      const entries: SelectEntry[] = [];
      for (let n = r.u32(); n > 0; n--) entries.push({ offset: r.i16(), text: str(r) });
      return { uid, id, entries };
    }
    case 0x2a:
      return { uid, id };
    case 0x2b:
      return { uid, id, level: r.u8() };
    case 0x30:
      return { uid, id, text: str(r) };
    case 0x31:
      return { uid, id, time: r.u8(), text: str(r) };
    case 0x32: {
      const entries: ArchiveEntry[] = [];
      for (let n = r.u32(); n > 0; n--) entries.push({ type: r.u8(), text: str(r) });
      return { uid, id, entries };
    }
    case 0x33: {
      const entries: HardwareEntry[] = [];
      for (let n = r.u32(); n > 0; n--) entries.push({ type: r.u8(), id: r.u8(), info: r.u8() });
      return { uid, id, entries };
    }
    case 0x35:
      return { uid, id, ident: str(r), data: bytes(r) };
    case 0x5a:
      return { uid, id, raw: bytes(r) };
    default:
      throw new Error(`Tape core sent block tag ${id.toString(16)}, which this build does not know`);
  }
}

// ---- encoding, for the calls that send blocks to the core ------------------

/**
 * A block list in the format `core/src/wire.rs` decodes.
 *
 * With `withData: false` the byte payloads are left out — the whole point of a
 * tape is its data, so sending it across for a call that only looks at block
 * types costs more than the call. Only for entry points that provably ignore
 * the data (`core_required_version`, `core_save_version`); the differential
 * tests compare them against the reference on blocks that do carry data, so a
 * core that started reading it would fail there.
 */
export function encodeBlocks(blocks: Block[], opts: { withData?: boolean } = {}): Uint8Array {
  const w = new Writer();
  const put = opts.withData === false ? skipBytes : putBytes;
  w.u32(blocks.length);
  for (const b of blocks) writeBlock(w, b, put);
  return w.toUint8Array();
}

/** How a block's byte payloads go on the wire. */
type PutBytes = (w: Writer, b: Uint8Array) => void;

function putBytes(w: Writer, b: Uint8Array): void {
  w.u32(b.length);
  w.bytes(b);
}

function skipBytes(w: Writer, _b: Uint8Array): void {
  w.u32(0);
}

function putStr(w: Writer, s: string): void {
  const b = utf8.encode.encode(s);
  w.u32(b.length);
  w.bytes(b);
}

function putU16s(w: Writer, v: number[]): void {
  w.u32(v.length);
  for (const n of v) w.u16(n);
}

function putSymDefs(w: Writer, defs: SymDef[]): void {
  w.u32(defs.length);
  for (const d of defs) {
    w.u8(d.flags);
    putU16s(w, d.pulses);
  }
}

function writeBlock(w: Writer, b: Block, putBytes: PutBytes): void {
  if (isUnknown(b)) {
    w.u8(UNKNOWN_TAG);
    w.u8(b.id);
    putBytes(w, b.raw);
    return;
  }
  w.u8(b.id);
  switch (b.id) {
    case 0x10:
      w.u16(b.pause);
      putBytes(w, b.data);
      break;
    case 0x11:
      for (const v of [b.pilot, b.sync1, b.sync2, b.zero, b.one, b.pilotLen]) w.u16(v);
      w.u8(b.usedBits);
      w.u16(b.pause);
      putBytes(w, b.data);
      break;
    case 0x12:
      w.u16(b.pulseLen);
      w.u16(b.count);
      break;
    case 0x13:
      putU16s(w, b.pulses);
      break;
    case 0x14:
      w.u16(b.zero);
      w.u16(b.one);
      w.u8(b.usedBits);
      w.u16(b.pause);
      putBytes(w, b.data);
      break;
    case 0x15:
      w.u16(b.tstates);
      w.u16(b.pause);
      w.u8(b.usedBits);
      putBytes(w, b.data);
      break;
    case 0x18:
      w.u16(b.pause);
      w.u32(b.sampleRate);
      w.u8(b.compression);
      w.u32(b.pulseCount);
      putBytes(w, b.data);
      break;
    case 0x19:
      w.u16(b.pause);
      w.u32(b.totp);
      w.u8(b.npp);
      putSymDefs(w, b.pilotSymbols);
      w.u32(b.pilotStream.length);
      for (const run of b.pilotStream) {
        w.u8(run.symbol);
        w.u16(run.reps);
      }
      w.u32(b.totd);
      w.u8(b.npd);
      putSymDefs(w, b.dataSymbols);
      putBytes(w, b.data);
      break;
    case 0x20:
      w.u16(b.pause);
      break;
    case 0x21:
      putStr(w, b.name);
      break;
    case 0x22:
      break;
    case 0x23:
      w.i16(b.offset);
      break;
    case 0x24:
      w.u16(b.count);
      break;
    case 0x25:
      break;
    case 0x26:
      w.u32(b.offsets.length);
      for (const o of b.offsets) w.i16(o);
      break;
    case 0x27:
      break;
    case 0x28:
      w.u32(b.entries.length);
      for (const e of b.entries) {
        w.i16(e.offset);
        putStr(w, e.text);
      }
      break;
    case 0x2a:
      break;
    case 0x2b:
      w.u8(b.level);
      break;
    case 0x30:
      putStr(w, b.text);
      break;
    case 0x31:
      w.u8(b.time);
      putStr(w, b.text);
      break;
    case 0x32:
      w.u32(b.entries.length);
      for (const e of b.entries) {
        w.u8(e.type);
        putStr(w, e.text);
      }
      break;
    case 0x33:
      w.u32(b.entries.length);
      for (const e of b.entries) {
        w.u8(e.type);
        w.u8(e.id);
        w.u8(e.info);
      }
      break;
    case 0x35:
      putStr(w, b.ident);
      putBytes(w, b.data);
      break;
    case 0x5a:
      putBytes(w, b.raw);
      break;
  }
}

// ---- answers for the description, content, consistency and program calls ----

export function decodeStrings(buf: Uint8Array): string[] {
  const r = new Reader(buf);
  readHeader(r);
  const out: string[] = [];
  for (let n = r.u32(); n > 0; n--) out.push(str(r));
  return out;
}

export function decodeOptString(buf: Uint8Array): string | null {
  const r = new Reader(buf);
  readHeader(r);
  return r.u8() === 1 ? str(r) : null;
}

export function decodeDescribed(buf: Uint8Array): { description: string; length: number } {
  const r = new Reader(buf);
  readHeader(r);
  return { description: str(r), length: r.u32() };
}

export function decodeU8(buf: Uint8Array): number {
  const r = new Reader(buf);
  readHeader(r);
  return r.u8();
}

export function decodeF64(buf: Uint8Array): number {
  const r = new Reader(buf);
  readHeader(r);
  return new DataView(buf.buffer, buf.byteOffset + r.pos, 8).getFloat64(0, true);
}

export function decodeRanges(buf: Uint8Array): Map<number, number> {
  const r = new Reader(buf);
  readHeader(r);
  const out = new Map<number, number>();
  for (let n = r.u32(); n > 0; n--) out.set(r.u32(), r.u32());
  return out;
}

export function decodeContent(buf: Uint8Array): ContentInfo {
  const r = new Reader(buf);
  readHeader(r);
  return {
    kind: str(r) as ContentKind,
    label: str(r),
    base: r.u16(),
    skipFlag: r.u8() === 1,
    skipChecksum: r.u8() === 1,
    progLen: optU16(r),
    header: r.u8() === 1 ? headerInfo(r) : null,
    source: str(r) as ContentInfo['source'],
    expectedLength: optU16(r),
  };
}

export function decodeHeaderInfo(buf: Uint8Array): HeaderInfo | null {
  const r = new Reader(buf);
  readHeader(r);
  return r.u8() === 1 ? headerInfo(r) : null;
}

export function decodeIssues(buf: Uint8Array): Issue[] {
  const r = new Reader(buf);
  readHeader(r);
  const out: Issue[] = [];
  for (let n = r.u32(); n > 0; n--) {
    const block = r.u32() | 0; // signed: -1 means the whole tape
    out.push({ block, severity: str(r) as Issue['severity'], message: str(r) });
  }
  return out;
}

export function decodePrograms(buf: Uint8Array): Program[] {
  const r = new Reader(buf);
  readHeader(r);
  const out: Program[] = [];
  for (let n = r.u32(); n > 0; n--) {
    out.push({ name: str(r), start: r.u32(), end: r.u32(), source: str(r) as Program['source'] });
  }
  return out;
}

/** A ROM header on its way to `core_encode_header`. */
export function encodeHeaderInfo(h: HeaderInfo): Uint8Array {
  const w = new Writer();
  w.u8(h.type);
  putStr(w, h.name);
  w.u16(h.length);
  w.u16(h.param1);
  w.u16(h.param2);
  return w.toUint8Array();
}

function optU16(r: Reader): number | null {
  return r.u8() === 1 ? r.u16() : null;
}

function headerInfo(r: Reader): HeaderInfo {
  const type = r.u8();
  return { type, typeName: str(r), name: str(r), length: r.u16(), param1: r.u16(), param2: r.u16() };
}

// ---- converting, comparing, bits and POKEs ---------------------------------

/** Blocks handed back by the core, for the calls that answer with them. */
export function decodeBlocks(buf: Uint8Array): Block[] {
  const r = new Reader(buf);
  readHeader(r);
  const out: Block[] = [];
  for (let n = r.u32(); n > 0; n--) out.push(block(r));
  return out;
}

export function decodeU32s(buf: Uint8Array): number[] {
  const r = new Reader(buf);
  readHeader(r);
  const out: number[] = [];
  for (let n = r.u32(); n > 0; n--) out.push(r.u32());
  return out;
}

export function decodeComparison(buf: Uint8Array): { left: CompareResult[]; right: CompareResult[]; identical: boolean } {
  const r = new Reader(buf);
  readHeader(r);
  const names: CompareResult[] = ['same', 'diff', 'ignored'];
  const side = () => {
    const out: CompareResult[] = [];
    for (let n = r.u32(); n > 0; n--) out.push(names[r.u8()]);
    return out;
  };
  const left = side();
  const right = side();
  return { left, right, identical: r.u8() === 1 };
}

export function decodeBitData(buf: Uint8Array): BitData {
  const r = new Reader(buf);
  readHeader(r);
  return { data: bytes(r), usedBits: r.u8() };
}

/** One or more bit streams on their way to the core. */
export function encodeBitData(parts: BitData[]): Uint8Array {
  const w = new Writer();
  w.u32(parts.length);
  for (const p of parts) {
    putBytes(w, p.data);
    w.u8(p.usedBits);
  }
  return w.toUint8Array();
}

export function decodePokesInfo(buf: Uint8Array): PokesInfo {
  const r = new Reader(buf);
  readHeader(r);
  const description = str(r);
  const trainers: Trainer[] = [];
  for (let n = r.u32(); n > 0; n--) {
    const desc = str(r);
    const pokes: Poke[] = [];
    for (let k = r.u32(); k > 0; k--) {
      const opt = () => {
        const present = r.u8() === 1;
        const value = r.u32();
        return present ? value : null;
      };
      pokes.push({ page: opt(), addr: r.u32(), value: opt(), original: opt() });
    }
    trainers.push({ description: desc, pokes });
  }
  return { description, trainers };
}

export function encodePokesInfo(info: PokesInfo): Uint8Array {
  const w = new Writer();
  putStr(w, info.description);
  w.u32(info.trainers.length);
  for (const t of info.trainers) {
    putStr(w, t.description);
    w.u32(t.pokes.length);
    for (const p of t.pokes) {
      const opt = (v: number | null) => {
        w.u8(v === null ? 0 : 1);
        w.u32(v ?? 0);
      };
      opt(p.page);
      w.u32(p.addr);
      opt(p.value);
      opt(p.original);
    }
  }
  return w.toUint8Array();
}

// ---- the Spectrum side: BASIC listings, variables and disassembly -----------

export function decodeBasicLines(buf: Uint8Array): BasicLine[] {
  const r = new Reader(buf);
  readHeader(r);
  const out: BasicLine[] = [];
  for (let n = r.u32(); n > 0; n--) {
    const number = r.u32();
    const length = r.u32();
    const offset = r.u32();
    const error = r.u8() === 1 ? str(r) : undefined;
    const tokens: BasicToken[] = [];
    for (let k = r.u32(); k > 0; k--) tokens.push({ text: str(r), kind: str(r) as BasicToken['kind'] });
    const line: BasicLine = { number, length, offset, tokens };
    if (error !== undefined) line.error = error;
    out.push(line);
  }
  return out;
}

/** A listing on its way back to the core, for `basicToText`. */
export function encodeBasicLines(lines: BasicLine[]): Uint8Array {
  const w = new Writer();
  w.u32(lines.length);
  for (const l of lines) {
    w.u32(l.number);
    w.u32(l.length);
    w.u32(l.offset);
    if (l.error === undefined) w.u8(0);
    else {
      w.u8(1);
      putStr(w, l.error);
    }
    w.u32(l.tokens.length);
    for (const t of l.tokens) {
      putStr(w, t.text);
      putStr(w, t.kind);
    }
  }
  return w.toUint8Array();
}

export function decodeVariables(buf: Uint8Array): VariableEntry[] {
  const r = new Reader(buf);
  readHeader(r);
  const out: VariableEntry[] = [];
  for (let n = r.u32(); n > 0; n--) {
    out.push({ name: str(r), type: str(r), value: str(r), offset: r.u32(), size: r.u32() });
  }
  return out;
}

export function decodeDisLines(buf: Uint8Array): DisLine[] {
  const r = new Reader(buf);
  readHeader(r);
  const out: DisLine[] = [];
  for (let n = r.u32(); n > 0; n--) {
    const addr = r.u32();
    const lineBytes = Array.from(bytes(r));
    const text = str(r);
    const line: DisLine = { addr, bytes: lineBytes, text };
    if (r.u8() === 1) line.target = r.u32();
    out.push(line);
  }
  return out;
}

// ---- audio -----------------------------------------------------------------

/** A block list followed by a playback order, as the render calls send one. */
export function encodeBlocksAndOrder(blocks: Block[], order: number[]): Uint8Array {
  const head = encodeBlocks(blocks);
  const tail = new Writer();
  tail.u32(order.length);
  for (const i of order) tail.u32(i);
  const rest = tail.toUint8Array();
  const out = new Uint8Array(head.length + rest.length);
  out.set(head);
  out.set(rest, head.length);
  return out;
}

function f64(r: Reader, buf: Uint8Array): number {
  const v = new DataView(buf.buffer, buf.byteOffset + r.pos, 8).getFloat64(0, true);
  r.pos += 8;
  return v;
}

export function decodeTimeline(buf: Uint8Array): { starts: number[]; total: number } {
  const r = new Reader(buf);
  readHeader(r);
  const starts: number[] = [];
  for (let n = r.u32(); n > 0; n--) starts.push(f64(r, buf));
  return { starts, total: f64(r, buf) };
}

export function decodeDuration(buf: Uint8Array): { seconds: number; order: number[] } {
  const r = new Reader(buf);
  readHeader(r);
  const seconds = f64(r, buf);
  const order: number[] = [];
  for (let n = r.u32(); n > 0; n--) order.push(r.u32());
  return { seconds, order };
}

export function decodePulses(buf: Uint8Array): { tstates: number; level: 0 | 1 }[] {
  const r = new Reader(buf);
  readHeader(r);
  const out: { tstates: number; level: 0 | 1 }[] = [];
  for (let n = r.u32(); n > 0; n--) out.push({ tstates: f64(r, buf), level: r.u8() as 0 | 1 });
  return out;
}

/** Rendered samples, which arrive as little-endian `f32`. */
export function decodeSamples(buf: Uint8Array): Float32Array {
  const r = new Reader(buf);
  readHeader(r);
  const len = r.u32();
  // The payload is a copy already, but it may not be 4-byte aligned.
  const start = buf.byteOffset + r.pos;
  if (start % 4 === 0) return new Float32Array(buf.buffer, start, len / 4);
  return new Float32Array(buf.slice(r.pos, r.pos + len).buffer);
}
