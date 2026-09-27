// The Rust tape core's TypeScript face: the loader, and every call the app makes
// into it, under the name the app knows it by.
//
// The wasm module is inlined as base64 by scripts/build-wasm.mjs, so the same code
// path works in the browser and in vitest under node. The desktop app links the
// core directly, with no wasm. Compiling wasm is asynchronous (browsers refuse a
// synchronous compile of anything but a tiny module on the main thread), and the
// app parses tapes synchronously, so `initCore` runs once at startup, in
// src/main.tsx, and everything below is sync from then on.
//
// What stays in TypeScript is the little that is cheaper here than across the
// boundary: slicing bytes the caller already holds, a binary search per animation
// frame, the caches, and inflating Z-RLE CSW blocks, which needs zlib and the
// dependency-free crate cannot have.
import { inflate } from 'pako';
import { CORE_WASM_BASE64 } from './core.wasm';
import {
  AsLoaded, BasicLine, BasicOptions, BitData, Block, BlockCompareMode, CompareResult, ContentInfo, DataBlock,
  DisLine, DisOptions, HeaderInfo, Issue, isUnknown, ParsedTape, PokesInfo, Program, TapeCompareMode,
  VariableEntry,
} from './types';
import {
  decodeBasicLines, decodeBitData, decodeBlocks, decodeBytes, decodeComparison, decodeContent,
  decodeContentAsLoaded,
  decodeDuration, decodePulses, decodeSamples, decodeTimeline, encodeBlocksAndOrder,
  decodeDescribed, decodeDisLines, decodeF64, decodeHeaderInfo, decodeIssues, decodeOptString,
  decodeBasicEdit, encodeBasicEdit, decodePokesInfo, decodePrograms, decodeRanges, decodeSnapshotInfo, decodeStrings, decodeTap,
  decodeTape, decodeU32s, decodeU8, decodeVariables, decodeVersion, encodeBasicLines, encodeBitData,
  encodeBlocks, encodeHeaderInfo, encodePokesInfo, encodeSnapshotRequest, WIRE_VERSION,
} from './wire';

export type {
  AsLoaded, BasicLine, BasicOptions, BasicToken, BitData, BlockCompareMode, CompareResult, ContentInfo, ContentKind,
  DisLine, DisOptions, HeaderInfo, Issue, ParsedTape, Poke, PokesInfo, Program, TapeCompareMode, Trainer,
  VariableEntry,
} from './types';

interface CoreExports {
  memory: WebAssembly.Memory;
  core_wire_version(): number;
  core_alloc(len: number): number;
  core_free(ptr: number, len: number): void;
  core_parse_tape(ptr: number, len: number): number;
  core_parse_tzx(ptr: number, len: number): number;
  core_parse_tap(ptr: number, len: number): number;
  core_snapshot_info(ptr: number, len: number, kind: number): number;
  core_snapshot_to_tape(ptr: number, len: number, kind: number, speed: number, border: number, flags: number): number;
  core_serialize_tzx(ptr: number, len: number, major: number, minor: number): number;
  core_serialize_tap(ptr: number, len: number): number;
  core_serialize_blocks(ptr: number, len: number): number;
  core_required_version(ptr: number, len: number): number;
  core_save_version(ptr: number, len: number, major: number, minor: number): number;
  core_describe_block(ptr: number, len: number, hex: number): number;
  core_content_labels(ptr: number, len: number): number;
  core_detect_content(ptr: number, len: number, index: number): number;
  core_detect_content_as_loaded(ptr: number, len: number, index: number): number;
  core_check_consistency(ptr: number, len: number, base: number): number;
  core_detect_programs(ptr: number, len: number): number;
  core_group_ranges(ptr: number, len: number): number;
  core_tape_title(ptr: number, len: number): number;
  core_decode_header(ptr: number, len: number): number;
  core_encode_header(ptr: number, len: number): number;
  core_checksum(ptr: number, len: number): number;
  core_basic_score(ptr: number, len: number): number;
  core_file_hashes(ptr: number, len: number): number;
  core_convert_block(ptr: number, len: number, id: number): number;
  core_blocks_equal(ptr: number, len: number, mode: number): number;
  core_compare_tapes(ptr: number, len: number, split: number, blockMode: number, tapeMode: number): number;
  core_find_matches(ptr: number, len: number, skip: number, mode: number): number;
  core_bits(ptr: number, len: number, op: number, n: number): number;
  core_flip_bytes(ptr: number, len: number): number;
  core_crypt(ptr: number, len: number, xor: number, add: number, encrypt: number): number;
  core_decode_pokes(ptr: number, len: number): number;
  core_encode_pokes(ptr: number, len: number): number;
  core_pokes_to_text(ptr: number, len: number, hex: number): number;
  core_text_to_pokes(ptr: number, len: number, hex: number): number;
  core_char_table(ptr: number, len: number, kind: number): number;
  core_tables(ptr: number, len: number, kind: number): number;
  core_render_screen(ptr: number, len: number, offset: number, flags: number): number;
  core_has_flash(ptr: number, len: number, offset: number): number;
  core_list_basic(ptr: number, len: number, start: number, end: number, flags: number): number;
  core_basic_to_text(ptr: number, len: number, flags: number): number;
  core_basic_source(ptr: number, len: number, start: number, end: number, flags: number): number;
  core_edit_basic(ptr: number, len: number, start: number, end: number, flags: number): number;
  core_disassembly_text(ptr: number, len: number, offset: number, base: number, count: number, flags: number): number;
  core_check_symbols(ptr: number, len: number): number;
  core_list_variables(ptr: number, len: number, start: number, end: number): number;
  core_disassemble(ptr: number, len: number, offset: number, base: number, count: number, flags: number): number;
  core_decode_number(ptr: number, len: number, offset: number): number;
  core_format_number(ptr: number, len: number, v: number): number;
  core_playback_order(ptr: number, len: number, flags: number): number;
  core_block_duration(ptr: number, len: number): number;
  core_tape_duration(ptr: number, len: number): number;
  core_playback_timeline(ptr: number, len: number): number;
  core_render_length(ptr: number, len: number, sampleRate: number): number;
  core_render_tape(ptr: number, len: number, sampleRate: number, mic: number, amplitude: number): number;
  core_render_wav(ptr: number, len: number, sampleRate: number, mic: number, amplitude: number, bits: number): number;
  core_encode_wav(ptr: number, len: number, sampleRate: number, bits: number): number;
  core_block_pulses(ptr: number, len: number): number;
  core_decode_csw_rle(ptr: number, len: number): number;
}

/** Everything but the bookkeeping exports takes a buffer and returns one. */
type Entry = Exclude<keyof CoreExports, 'memory' | 'core_wire_version' | 'core_alloc' | 'core_free'>;

/** What `core_serialize_tzx` and `core_save_version` take for "no version". */
const NO_VERSION = 0xffff;
const NO_INPUT = new Uint8Array(0);
const utf8 = new TextEncoder();

let core: CoreExports | null = null;
let loading: Promise<void> | null = null;
let failure: Error | null = null;

// Typed as backed by an ArrayBuffer so it satisfies BufferSource.
function wasmBytes(): Uint8Array<ArrayBuffer> {
  const binary = atob(CORE_WASM_BASE64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

/** Compile and instantiate the core. Safe to call repeatedly; only the first call works. */
export function initCore(): Promise<void> {
  if (core) return Promise.resolve();
  if (!loading) {
    loading = WebAssembly.instantiate(wasmBytes()).then(
      ({ instance }) => {
        const exports = instance.exports as unknown as CoreExports;
        const version = exports.core_wire_version();
        if (version !== WIRE_VERSION) {
          throw new Error(`Tape core speaks wire format ${version}, this build expects ${WIRE_VERSION}`);
        }
        core = exports;
      },
      (e: unknown) => {
        // Remember why, so a later parse can say something better than "still loading".
        failure = e instanceof Error ? e : new Error(String(e));
        throw failure;
      },
    );
  }
  return loading;
}

function requireCore(): CoreExports {
  if (core) return core;
  throw new Error(
    failure
      ? `The tape core failed to load: ${failure.message}`
      : 'The tape core is still loading; await initCore() before parsing',
  );
}

/** Hand a buffer to one of the core's entry points and copy the answer out. */
function call(entry: Entry, buf: Uint8Array, ...extra: number[]): Uint8Array {
  const c = requireCore();
  const inPtr = c.core_alloc(buf.length);
  try {
    if (buf.length > 0) new Uint8Array(c.memory.buffer, inPtr, buf.length).set(buf);
    const out = (c[entry] as (...a: number[]) => number)(inPtr, buf.length, ...extra);
    if (out === 0) throw new Error('The tape core could not allocate a result');
    // memory.buffer is replaced when wasm memory grows, so read it again here.
    const len = new DataView(c.memory.buffer).getUint32(out, true);
    const payload = new Uint8Array(c.memory.buffer, out + 4, len).slice();
    c.core_free(out, 4 + len);
    return payload;
  } finally {
    c.core_free(inPtr, buf.length);
  }
}

// ---------------------------------------------------------------------------
// Answers remembered per tape.
//
// Every call sends the blocks across the boundary, so asking twice for the same
// tape costs twice. Blocks and the arrays holding them are immutable: every edit
// makes a new array (see src/state/store.ts), so a WeakMap keyed on the array
// itself cannot go stale, and the entry disappears with the tape. Memoizing here
// rather than in each component means two components asking the same question
// only pay once.

/** Memoize a function of a block list on the identity of that list. */
function perTape<T>(fn: (blocks: Block[]) => T): (blocks: Block[]) => T {
  const cache = new WeakMap<Block[], T>();
  return (blocks) => {
    if (cache.has(blocks)) return cache.get(blocks) as T;
    const value = fn(blocks);
    cache.set(blocks, value);
    return value;
  };
}

/** As `perTape`, for a function that also takes a small extra argument. */
function perTapeWith<A, T>(fn: (blocks: Block[], arg: A) => T): (blocks: Block[], arg: A) => T {
  const cache = new WeakMap<Block[], Map<A, T>>();
  return (blocks, arg) => {
    let byArg = cache.get(blocks);
    if (!byArg) {
      byArg = new Map<A, T>();
      cache.set(blocks, byArg);
    }
    if (byArg.has(arg)) return byArg.get(arg) as T;
    const value = fn(blocks, arg);
    byArg.set(arg, value);
    return value;
  };
}

// ---------------------------------------------------------------------------
// Parsing (core/src/parser.rs).

const TZX_SIGNATURE = 'ZXTape!\x1a';

export function isTzx(buf: Uint8Array): boolean {
  if (buf.length < 10) return false;
  for (let i = 0; i < 8; i++) if (buf[i] !== TZX_SIGNATURE.charCodeAt(i)) return false;
  return true;
}

export function parseTzx(buf: Uint8Array): ParsedTape {
  return decodeTape(call('core_parse_tzx', buf));
}

/** Parse a TAP file into standard speed blocks. */
export function parseTap(buf: Uint8Array): ParsedTape {
  return decodeTape(call('core_parse_tap', buf));
}

/** Auto-detect TZX vs TAP by signature. */
export function parseTape(buf: Uint8Array): ParsedTape {
  return decodeTape(call('core_parse_tape', buf));
}

// ---------------------------------------------------------------------------
// Snapshots (core/src/snapshot.rs): a .z80 or .sna turned into a tape that
// loads it, a BASIC loader followed by the memory as packed turbo blocks.

export type SnapshotKind = 'z80' | 'sna';

/** Bits per second of each loading speed; the first is ROM timing in standard blocks. */
export const SNAPSHOT_SPEEDS = [1500, 2250, 3000, 6000];
export const DEFAULT_SNAPSHOT_SPEED = 2;
export const SNAPSHOT_SPEED_NAMES = ['Normal', 'High', 'Turbo', 'Ludicrous'];
export const SNAPSHOT_MACHINES = ['48K', '128K', 'Scorpion 256K'];

export interface SnapshotInfo {
  /** Index into SNAPSHOT_MACHINES. */
  machine: number;
  border: number;
  screen: Uint8Array;
}

export interface SnapshotOptions {
  speed: number;
  border: number;
  /** Pack the screen's page even when the packed bytes show on screen while they load. */
  compressAll: boolean;
  /** A 6912 byte loading screen to show instead of the snapshot's own. */
  screen: Uint8Array | null;
}

/** By extension: neither format has a signature. */
export function snapshotKind(name: string): SnapshotKind | null {
  const ext = name.split('.').pop()?.toLowerCase();
  return ext === 'z80' || ext === 'sna' ? ext : null;
}

const kindNumber = (kind: SnapshotKind) => (kind === 'sna' ? 1 : 0);

/** Throws when the file is not a snapshot the core can read. */
export function snapshotInfo(file: Uint8Array, kind: SnapshotKind): SnapshotInfo {
  return decodeSnapshotInfo(call('core_snapshot_info', file, kindNumber(kind)));
}

export function snapshotToTape(file: Uint8Array, kind: SnapshotKind, name: string, opts: SnapshotOptions): ParsedTape {
  const request = encodeSnapshotRequest(file, opts.screen, name);
  return decodeTape(call('core_snapshot_to_tape', request, kindNumber(kind), opts.speed, opts.border, opts.compressAll ? 1 : 0));
}

// ---------------------------------------------------------------------------
// Writing (core/src/writer.rs).

/** Lowest TZX version able to represent these blocks. */
export function requiredVersion(blocks: Block[]): { major: number; minor: number } {
  return decodeVersion(call('core_required_version', encodeBlocks(blocks, { withData: false })));
}

/**
 * Version to write in the header: the lowest one the blocks need, but never below the
 * version the tape was loaded with, so an unaltered file round-trips byte for byte.
 */
export function saveVersion(blocks: Block[], loaded: { major: number; minor: number } | null): { major: number; minor: number } {
  const l = loaded ?? { major: NO_VERSION, minor: NO_VERSION };
  return decodeVersion(call('core_save_version', encodeBlocks(blocks, { withData: false }), l.major, l.minor));
}

export function serializeTzx(blocks: Block[], version?: { major: number; minor: number }): Uint8Array {
  const v = version ?? { major: NO_VERSION, minor: NO_VERSION };
  return decodeBytes(call('core_serialize_tzx', encodeBlocks(blocks), v.major, v.minor));
}

/** Serialize a single block to bytes (ID + body); used for comparison and size display. */
export function serializeBlock(b: Block): Uint8Array {
  return decodeBytes(call('core_serialize_blocks', encodeBlocks([b])));
}

/** TAP export: only standard-speed blocks can be represented. Returns skipped block indices. */
export function serializeTap(blocks: Block[]): { bytes: Uint8Array; skipped: number[] } {
  return decodeTap(call('core_serialize_tap', encodeBlocks(blocks)));
}

// ---------------------------------------------------------------------------
// Descriptions and ROM headers (core/src/describe.rs).

export const HEADER_TYPE_NAMES = ['Program', 'Number array', 'Character array', 'Bytes'];

/** Decode a standard ROM header if this block looks like one. */
export function decodeHeader(data: Uint8Array): HeaderInfo | null {
  return decodeHeaderInfo(call('core_decode_header', data));
}

export function encodeHeader(h: HeaderInfo): Uint8Array {
  return decodeBytes(call('core_encode_header', encodeHeaderInfo(h)));
}

export function checksum(data: Uint8Array, start = 0, end = data.length): number {
  return decodeU8(call('core_checksum', data.subarray(start, end)));
}

/**
 * Both spellings of the description are kept: the Dec/Hex switch flips between
 * them. The list asks for a description and a length per row, so both are cached
 * per block object; blocks are immutable, an edit makes a new one, and a WeakMap
 * keyed on the object cannot go stale.
 */
interface Described {
  dec?: string;
  hex?: string;
  length: number;
}

const described = new WeakMap<Block, Described>();

function describe(b: Block, hex: boolean): Described {
  const hit = described.get(b);
  if (hit && (hex ? hit.hex : hit.dec) !== undefined) return hit;
  const answer = decodeDescribed(call('core_describe_block', encodeBlocks([b]), hex ? 1 : 0));
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
 * Kept in TypeScript: it is a classification of the block model, like
 * `isDataBlock`, and the list asks it per row. The core has its own copy in
 * `describe.rs`, and `test/core.test.ts` holds both to the same table.
 */
export function isMetadata(b: Block): boolean {
  return b.id === 0x21 || b.id === 0x22 || b.id === 0x30 || b.id === 0x31 || b.id === 0x32 || b.id === 0x33 || b.id === 0x35 || b.id === 0x5a || (isUnknown(b) && b.id !== 0x16 && b.id !== 0x17);
}

/** Byte payload of a data block. */
export function payload(b: DataBlock): Uint8Array {
  return b.data;
}

// ---------------------------------------------------------------------------
// What a data block contains (core/src/content.rs): a ROM header, a BASIC
// program, a screen, machine code, an array or plain data.

/**
 * Body of a data block after removing flag/checksum bytes.
 *
 * Kept in TypeScript: a view of bytes the editor already holds, not logic, and
 * crossing into wasm to drop two bytes would cost more than it saves.
 * `content.rs` has the same slice for the core's own use, and `test/core.test.ts`
 * holds the two together.
 */
export function blockBody(data: Uint8Array, skipFlag: boolean, skipChecksum: boolean): Uint8Array {
  let d = data;
  if (skipFlag && d.length > 0) d = d.subarray(1);
  if (skipChecksum && d.length > 0) d = d.subarray(0, d.length - 1);
  return d;
}

/** Does the byte stream look like a BASIC program area? Returns the fraction of bytes that parse as lines. */
export function basicScore(d: Uint8Array): number {
  return decodeF64(call('core_basic_score', d));
}

/**
 * Analyse block `index`; the previous block is consulted for a ROM header. Only
 * the block and the one before it matter, so that is all that goes over the wire.
 */
export function detectContent(blocks: Block[], index: number): ContentInfo {
  const block = blocks[index];
  // Out of range: the core answers with the "nothing detected" default.
  if (!block) return decodeContent(call('core_detect_content', encodeBlocks([]), 0));
  const prev = index > 0 ? blocks[index - 1] : null;
  const window = prev ? [prev, block] : [block];
  return decodeContent(call('core_detect_content', encodeBlocks(window), prev ? 1 : 0));
}

/**
 * What a data window should open block `index` with: the content guess, made on
 * the decrypted bytes when the group around the block names a loader that
 * encrypts (SpeedLock), and the values it used. The whole tape goes over rather
 * than a two-block window, since the group is what decides, and a data window is
 * opened by hand, not drawn per frame.
 */
export function detectContentAsLoaded(blocks: Block[], index: number): AsLoaded {
  return decodeContentAsLoaded(call('core_detect_content_as_loaded', encodeBlocks(blocks), index));
}

/** The list label of every block, in one call: the block list asks for all of them at once. */
export const contentLabels = perTape((blocks: Block[]): string[] =>
  decodeStrings(call('core_content_labels', encodeBlocks(blocks))));

// ---------------------------------------------------------------------------
// "Check consistency" (core/src/consistency.rs): structure, useless blocks,
// infinite loops, cross nesting.

const checked = perTapeWith((blocks: Block[], base: number): Issue[] =>
  decodeIssues(call('core_check_consistency', encodeBlocks(blocks), base)));

/** `base` is the number shown for the first block (1, or 0 with zero-based numbering). */
export function checkConsistency(blocks: Block[], base = 1): Issue[] {
  return checked(blocks, base);
}

// ---------------------------------------------------------------------------
// Tape structure (core/src/programs.rs): group/loop ranges and the programs
// (games) a collection tape holds.

/**
 * Pairs of start/end indices for groups and loops (nested ones included).
 * Cached per tape: the menus and the store ask for this on every render.
 */
export const groupRanges = perTape((blocks: Block[]): Map<number, number> =>
  decodeRanges(call('core_group_ranges', encodeBlocks(blocks, { withData: false }))));

/** Title from an Archive info block, if the tape has one. */
export const tapeTitle = perTape((blocks: Block[]): string | null =>
  decodeOptString(call('core_tape_title', encodeBlocks(blocks, { withData: false }))));

/** Split a tape into programs; see `core/src/programs.rs` for what decides a boundary. */
export const detectPrograms = perTape((blocks: Block[]): Program[] =>
  decodePrograms(call('core_detect_programs', encodeBlocks(blocks))));

/** The program containing block `index`, if any. */
export function programAt(programs: Program[], index: number): Program | undefined {
  return programs.find((p) => index >= p.start && index <= p.end);
}

// ---------------------------------------------------------------------------
// The checksums a tape file is known by (core/src/hash.rs), over the file's
// bytes as read.

/** A file's CRC32, MD5 and SHA-1, in lowercase hex. */
export interface FileHashes {
  crc32: string;
  md5: string;
  sha1: string;
}

export function fileHashes(bytes: Uint8Array): FileHashes {
  const [crc32, md5, sha1] = decodeStrings(call('core_file_hashes', bytes));
  return { crc32, md5, sha1 };
}

// ---------------------------------------------------------------------------
// Changing a block's type (core/src/convert.rs), the block editor's type menu.

/**
 * Convert `b` to block type `id`, keeping its uid and every field the new type shares with
 * the old one (pause, data, used bits, timings, text). Unknown blocks are returned unchanged.
 */
export function convertBlock(b: Block, id: number): Block {
  // Nothing to do, and the caller expects the same object back.
  if (isUnknown(b) || b.id === id) return b;
  const [decoded] = decodeBlocks(call('core_convert_block', encodeBlocks([b]), id));
  const converted = { ...decoded, uid: b.uid };
  // The core carries the data across unchanged, so keep the array the caller
  // already had instead of the copy that came back over the wire.
  if ('data' in converted && 'data' in b) (converted as { data: Uint8Array }).data = b.data;
  return converted;
}

// ---------------------------------------------------------------------------
// Comparing blocks and tapes (core/src/compare.rs).

/** The compare modes as `core/src/wasm.rs` numbers them. */
const BLOCK_MODES: BlockCompareMode[] = ['data', 'data+timings', 'data+timings+pauses'];
const TAPE_MODES: TapeCompareMode[] = ['datablocks', 'ignore-metadata', 'all'];

/** Compare two blocks according to the block-compare setting. */
export function blocksEqual(a: Block, b: Block, mode: BlockCompareMode): boolean {
  return decodeU8(call('core_blocks_equal', encodeBlocks([a, b]), BLOCK_MODES.indexOf(mode))) === 1;
}

/** Compare two tapes block by block, returning a result per block of each tape. */
export function compareTapes(
  left: Block[], right: Block[], blockMode: BlockCompareMode, tapeMode: TapeCompareMode,
): { left: CompareResult[]; right: CompareResult[]; identical: boolean } {
  const payload = encodeBlocks([...left, ...right]);
  return decodeComparison(
    call('core_compare_tapes', payload, left.length, BLOCK_MODES.indexOf(blockMode), TAPE_MODES.indexOf(tapeMode)),
  );
}

/** Find all blocks in `haystack` matching `needle`. */
export function findMatches(needle: Block, haystack: Block[], mode: BlockCompareMode): number[] {
  // The core compares by index, not object identity, so pass where `needle`
  // sits in `haystack` (0xffffffff if it is not there).
  const skip = haystack.indexOf(needle);
  const payload = encodeBlocks([needle, ...haystack]);
  return decodeU32s(call('core_find_matches', payload, skip < 0 ? 0xffffffff : skip, BLOCK_MODES.indexOf(mode)));
}

// ---------------------------------------------------------------------------
// Bit streams (core/src/bits.rs): the data window's Drop / Add / Shift
// operations, and the loader encryption.

/** Bit operations, in the order `core_bits` expects. */
type BitOp = 'drop' | 'add' | 'shiftLeft' | 'shiftRight' | 'join';
const BIT_OPS: BitOp[] = ['drop', 'add', 'shiftLeft', 'shiftRight', 'join'];

function bits(op: BitOp, parts: BitData[], n = 0): BitData {
  return decodeBitData(call('core_bits', encodeBitData(parts), BIT_OPS.indexOf(op), n));
}

/**
 * How many bits the stream holds.
 *
 * Kept in TypeScript: arithmetic on two numbers the caller already has, which
 * the data window asks for on every render. `bits.rs` has the same function for
 * the core's own use, and both sides assert the same table.
 */
export function totalBits(d: BitData): number {
  if (d.data.length === 0) return 0;
  return (d.data.length - 1) * 8 + Math.max(1, Math.min(8, d.usedBits));
}

export function dropBits(d: BitData, n: number): BitData {
  return bits('drop', [d], n);
}

export function addBits(d: BitData, n: number): BitData {
  return bits('add', [d], n);
}

export function shiftLeftBits(d: BitData, n: number): BitData {
  return bits('shiftLeft', [d], n);
}

export function shiftRightBits(d: BitData, n: number): BitData {
  return bits('shiftRight', [d], n);
}

/** Concatenates several bit streams. Used by "view selected as one". */
export function joinBits(parts: BitData[]): BitData {
  return bits('join', parts);
}

/** Reverse the bits of every byte. */
export function flipBytes(d: Uint8Array): Uint8Array {
  return decodeBytes(call('core_flip_bytes', d));
}

/** A loader's `(byte XOR x) + y` over every byte, or the inverse. */
function crypt(data: Uint8Array, xor: number, add: number, encrypt: boolean): Uint8Array {
  return decodeBytes(call('core_crypt', data, xor & 0xff, add & 0xff, encrypt ? 1 : 0));
}

/**
 * The usual turbo loader encryption, SpeedLock's among them: what ends up in
 * memory is `(loaded byte XOR x) + y` (LD A,x: XOR L: ADD A,y: LD (IX+0),A).
 */
export function decryptBytes(d: Uint8Array, xor: number, add: number): Uint8Array {
  return crypt(d, xor, add, false);
}

/** The other way: the bytes a tape must hold for the loader to store these. */
export function encryptBytes(d: Uint8Array, xor: number, add: number): Uint8Array {
  return crypt(d, xor, add, true);
}

/** The values the SpeedLock versions use unless a game changes them. `CRYPT_PRESETS` in the core. */
export const CRYPT_PRESETS = [
  { name: 'SpeedLock 2/3', xor: 0x98, add: 0x0b },
  { name: 'SpeedLock 4-7', xor: 0xc1, add: 0x11 },
];

// ---------------------------------------------------------------------------
// POKEs (core/src/pokes.rs): the text syntax <-> the standardized 'POKEs'
// custom info block.

export function decodePokes(data: Uint8Array): PokesInfo {
  return decodePokesInfo(call('core_decode_pokes', data));
}

export function encodePokes(info: PokesInfo): Uint8Array {
  return decodeBytes(call('core_encode_pokes', encodePokesInfo(info)));
}

export function pokesToText(info: PokesInfo, hex: boolean): string {
  return decodeOptString(call('core_pokes_to_text', encodePokesInfo(info), hex ? 1 : 0)) ?? '';
}

/** Parses the editor's text; a line that makes no sense throws. */
export function textToPokes(text: string, hex: boolean): PokesInfo {
  return decodePokesInfo(call('core_text_to_pokes', utf8.encode(text), hex ? 1 : 0));
}

// ---------------------------------------------------------------------------
// The ZX Spectrum character set (core/src/spectrum/charset.rs). The hex dump
// asks per byte, so each of the three tables is fetched once, all 256
// characters, and indexed from then on.

type CharTable = 'zx' | 'zxPlain' | 'dump';
const tables = new Map<CharTable, string[]>();

function charTable(kind: CharTable): string[] {
  let t = tables.get(kind);
  if (!t) {
    const id = kind === 'zx' ? 0 : kind === 'zxPlain' ? 1 : 2;
    t = decodeStrings(call('core_char_table', NO_INPUT, id));
    tables.set(kind, t);
  }
  return t;
}

/** Printable form of a single ZX character, for the dump / text views. */
export function zxChar(code: number, expandTokens = true): string {
  return charTable(expandTokens ? 'zx' : 'zxPlain')[code & 0xff];
}

/** Single-cell character for the hex dump ASCII column. */
export function dumpChar(code: number): string {
  return charTable('dump')[code & 0xff];
}

// ---------------------------------------------------------------------------
// The dropdown tables of the archive-info and hardware editors
// (core/src/tables.rs), fetched once each.

const fetched = new Map<number, string[][]>();

/** Table `kind` of `core_tables`, each entry split into its tab-separated fields. */
function tableRows(kind: number): string[][] {
  let rows = fetched.get(kind);
  if (!rows) {
    rows = decodeStrings(call('core_tables', NO_INPUT, kind)).map((row) => row.split('\t'));
    fetched.set(kind, rows);
  }
  return rows;
}

/** The kind byte of an Archive info entry and its label. */
export function archiveTypes(): { kind: number; name: string }[] {
  return tableRows(0).map(([kind, name]) => ({ kind: Number(kind), name }));
}

/** What a hardware entry says about the hardware, by info byte. */
export function hardwareInfo(): string[] {
  return tableRows(1).map(([name]) => name);
}

/** The hardware classes, each with its devices in id order. */
export function hardwareTypes(): { name: string; ids: string[] }[] {
  return tableRows(2).map(([name, ...ids]) => ({ name, ids }));
}

// ---------------------------------------------------------------------------
// Screen dumps (core/src/spectrum/screen.rs).

export const SCREEN_SIZE = 6912;

/** Attribute used where the data ends before the attribute area: black ink on white paper, as after NEW. */
export const DEFAULT_ATTR = 0x38;

export interface ScreenOptions {
  hideAttributes?: boolean;
  flashPhase?: boolean; // true = swap ink/paper for FLASH cells
}

/** Render a 6912-byte screen dump into RGBA pixels (256x192). Missing bitmap bytes are treated as 0,
 *  missing attributes as DEFAULT_ATTR so partial screens stay visible. */
export function renderScreen(data: Uint8Array, offset: number, opts: ScreenOptions = {}): Uint8ClampedArray {
  const flags = (opts.hideAttributes ? 1 : 0) | (opts.flashPhase ? 2 : 0);
  return new Uint8ClampedArray(decodeBytes(call('core_render_screen', data, offset, flags)));
}

export function hasFlash(data: Uint8Array, offset: number): boolean {
  return decodeU8(call('core_has_flash', data, offset)) === 1;
}

// ---------------------------------------------------------------------------
// BASIC (core/src/spectrum/basic.rs, source.rs): the lister, the variables
// area, and the program as editable text.

/** Decode a 5-byte Sinclair floating point number. */
export function decodeNumber(d: Uint8Array, o: number): number {
  return decodeF64(call('core_decode_number', d, o));
}

export function formatNumber(v: number): string {
  return decodeOptString(call('core_format_number', NO_INPUT, v)) ?? '';
}

function basicFlags(opts: BasicOptions): number {
  return (opts.showNumbers ? 1 : 0) | (opts.basic128 ? 2 : 0) | (opts.speccyFormat ? 4 : 0) | (opts.dropColours ? 16 : 0) | (opts.hexNumbers ? 64 : 0);
}

/** List a BASIC program area. `data` is the raw bytes starting at PROG. */
export function listBasic(data: Uint8Array, start: number, end: number, opts: BasicOptions): BasicLine[] {
  return decodeBasicLines(call('core_list_basic', data, start, end, basicFlags(opts)));
}

/** Render a listing as plain text. */
export function basicToText(lines: BasicLine[], opts: BasicOptions): string {
  return decodeOptString(call('core_basic_to_text', encodeBasicLines(lines), basicFlags(opts))) ?? '';
}

/** List the variables area (starting at VARS) until the 0x80 end marker. */
export function listVariables(data: Uint8Array, start: number, end: number): VariableEntry[] {
  return decodeVariables(call('core_list_variables', data, start, end));
}

export interface SourceOptions {
  basic128?: boolean;
  /** Take `print` for `PRINT`. */
  anyCase?: boolean;
  /** Hold a tokenised line to the 48K ROM's syntax as well as its spelling. */
  checkSyntax?: boolean;
}

export interface SourceError {
  /** Line of the text, from 1; 0 for the program as a whole. */
  line: number;
  message: string;
}

/** Flags of the two calls below: 2 for 128k tokens, 8 for keywords in any case, 32 to check the syntax. */
const sourceFlags = (o: SourceOptions) => (o.basic128 ? 2 : 0) | (o.anyCase ? 8 : 0) | (o.checkSyntax ? 32 : 0);

/**
 * A program area as text that can be edited, saved and tokenised again: keywords
 * spelled out and {...} for what a keyboard cannot type. core/src/spectrum/source.rs
 * has the format.
 */
export function basicSource(data: Uint8Array, start: number, end: number, opts: SourceOptions = {}): string {
  return decodeOptString(call('core_basic_source', data, start, end, sourceFlags(opts))) ?? '';
}

/**
 * The program area `text` stands for, given the one it was made from. Lines still
 * as basicSource wrote them keep their bytes; only the others are tokenised.
 */
export function editBasic(data: Uint8Array, start: number, end: number, text: string, opts: SourceOptions = {}): { program: Uint8Array } | { errors: SourceError[] } {
  return decodeBasicEdit(call('core_edit_basic', encodeBasicEdit(data, text), start, end, sourceFlags(opts)));
}

// ---------------------------------------------------------------------------
// Z80 disassembly (core/src/spectrum/z80dis.rs).

/** The buffer and flags both disassembly calls take; with symbols the buffer carries them too. */
function disassemblyRequest(data: Uint8Array, opts: DisOptions): [Uint8Array, number] {
  const flags = ((opts.hex ?? true) ? 1 : 0) | ((opts.romLabels !== false) ? 2 : 0) | (opts.sysvars ? 4 : 0) | (opts.literals ? 8 : 0);
  return opts.symbols?.trim() ? [encodeBasicEdit(data, opts.symbols), flags | 16] : [data, flags];
}

/** Disassemble `count` instructions from `data` starting at byte `offset`, with address base `base`. */
export function disassemble(data: Uint8Array, offset: number, base: number, count: number, opts: DisOptions = {}): DisLine[] {
  const [buf, flags] = disassemblyRequest(data, opts);
  return decodeDisLines(call('core_disassemble', buf, offset, base, count, flags));
}

/** The same as text, for saving: address, bytes, instruction. */
export function disassemblyText(data: Uint8Array, offset: number, base: number, count: number, opts: DisOptions = {}): string {
  const [buf, flags] = disassemblyRequest(data, opts);
  return decodeOptString(call('core_disassembly_text', buf, offset, base, count, flags)) ?? '';
}

/** The lines (from 1) of a symbol table that are not "address name". */
export function checkSymbols(text: string): number[] {
  return decodeU32s(call('core_check_symbols', utf8.encode(text)));
}

// ---------------------------------------------------------------------------
// Audio (core/src/audio.rs): a block list as an edge/pulse stream and then as
// PCM samples, following loops, jumps, calls and returns like an emulator would.

export const TSTATES_PER_SEC = 3500000;

/** Silence before and after the rendered tape, in T-states. */
export const LEAD_TSTATES = TSTATES_PER_SEC / 2;

/** How a block renders as pulses: what a PZX export or a waveform view would want. */
export interface Pulse {
  tstates: number;
  level: 0 | 1;
}

export interface FlowOptions {
  /** Stop at a "stop the tape if in 48k mode" block. */
  stopAt48k?: boolean;
  /** Safety valve for infinite loops via jumps. */
  maxSteps?: number;
}

export interface RenderOptions {
  sampleRate: number;
  /** 'square' = plain square wave, 'mic' = emulate the Spectrum MIC output high-pass response. */
  mode: 'square' | 'mic';
  amplitude?: number; // 0..1
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

/**
 * The tape as the core should see it: compressed CSW blocks with their data
 * already inflated. Everything else is passed straight through, so a tape
 * without CSW blocks is the same array it came in as.
 */
const playable = perTape((blocks: Block[]): Block[] => {
  let changed = false;
  const out = blocks.map((b) => {
    // isUnknown first: an unknown block's id is a plain number, so `b.id !== 0x18`
    // does not narrow it away.
    if (isUnknown(b) || b.id !== 0x18 || b.compression !== 2) return b;
    changed = true;
    return { ...b, compression: 1, data: cswRleData(b.data, 2) };
  });
  return changed ? out : blocks;
});

/** Decode CSW v2 RLE pulse lengths (in samples). Z-RLE must be inflated first. */
export function decodeCswRle(data: Uint8Array): number[] {
  return decodeU32s(call('core_decode_csw_rle', data));
}

/** Walk the tape in playback order, yielding the block index sequence. */
export function playbackOrder(blocks: Block[], opts: FlowOptions = {}): number[] {
  return decodeU32s(call('core_playback_order', encodeBlocks(blocks), opts.stopAt48k === true ? 1 : 0));
}

/** Total T-states for a block in isolation. */
export function blockDuration(b: Block): number {
  return decodeF64(call('core_block_duration', encodeBlocks(playable([b]))));
}

/** The pulses one block produces, at the level held during each. */
export function blockPulses(b: Block): Pulse[] {
  return decodePulses(call('core_block_pulses', encodeBlocks(playable([b]))));
}

/** Duration in seconds of the whole tape following playback flow. */
export function tapeDuration(blocks: Block[]): { seconds: number; order: number[] } {
  return decodeDuration(call('core_tape_duration', encodeBlocks(playable(blocks))));
}

/** Where each block of the playback order starts, without rendering. */
export function playbackTimeline(blocks: Block[], order: number[]): { starts: number[]; total: number } {
  return decodeTimeline(call('core_playback_timeline', encodeBlocksAndOrder(playable(blocks), order)));
}

/** Index into the playback order of the block playing at `tstates` (the first block during lead-in). */
export function positionAt(starts: number[], tstates: number): number {
  // Kept in TypeScript: a binary search over an array the caller already holds,
  // which the progress indicator runs on every animation frame.
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
  return decodeF64(call('core_render_length', encodeBlocksAndOrder(playable(blocks), order), sampleRate));
}

export function renderTape(blocks: Block[], opts: RenderOptions, indices?: number[]): Float32Array {
  const tape = playable(blocks);
  const order = indices ?? playbackOrder(tape);
  const payload = encodeBlocksAndOrder(tape, order);
  return decodeSamples(call('core_render_tape', payload, opts.sampleRate, opts.mode === 'mic' ? 1 : 0, opts.amplitude ?? 0.8));
}

/**
 * Render straight to a WAV file. The samples of a long tape are tens of
 * megabytes, so this keeps them inside the core instead of copying them out
 * only to send them back in.
 */
export function renderWav(blocks: Block[], opts: RenderOptions, bits: 8 | 16 = 16, indices?: number[]): Uint8Array {
  const tape = playable(blocks);
  const order = indices ?? playbackOrder(tape);
  const payload = encodeBlocksAndOrder(tape, order);
  return decodeBytes(call('core_render_wav', payload, opts.sampleRate, opts.mode === 'mic' ? 1 : 0, opts.amplitude ?? 0.8, bits));
}

/** Encode samples as an 8- or 16-bit mono PCM WAV file (16-bit by default). */
export function encodeWav(samples: Float32Array, sampleRate: number, bits: 8 | 16 = 16): Uint8Array {
  const raw = new Uint8Array(samples.buffer, samples.byteOffset, samples.length * 4);
  return decodeBytes(call('core_encode_wav', raw, sampleRate, bits));
}
