// Loader for the Rust tape core. The wasm module is inlined as base64 by
// scripts/build-wasm.mjs, so the same code path works in the browser and in
// vitest under node. (The desktop app links the core directly — no wasm.)
//
// Compiling wasm is asynchronous (browsers refuse a synchronous compile of
// anything but a tiny module on the main thread), but the app parses tapes
// synchronously, so `initCore` runs once at startup — see src/main.tsx — and
// the parse functions are sync from then on.
import { CORE_WASM_BASE64 } from './core.wasm';
import {
  BasicLine, BasicOptions, BitData, Block, BlockCompareMode, CompareResult, ContentInfo, DisLine,
  DisOptions, HeaderInfo, Issue, ParsedTape, PokesInfo, Program, TapeCompareMode, VariableEntry,
} from './types';
import {
  decodeBasicLines, decodeBitData, decodeBlocks, decodeBytes, decodeComparison, decodeContent,
  decodeDuration, decodePulses, decodeSamples, decodeTimeline, encodeBlocksAndOrder,
  decodeDescribed, decodeDisLines, decodeF64, decodeHeaderInfo, decodeIssues, decodeOptString,
  decodePokesInfo, decodePrograms, decodeRanges, decodeStrings, decodeTap, decodeTape, decodeU32s,
  decodeU8, decodeVariables, decodeVersion, encodeBasicLines, encodeBitData, encodeBlocks,
  encodeHeaderInfo, encodePokesInfo, WIRE_VERSION,
} from './wire';

interface CoreExports {
  memory: WebAssembly.Memory;
  core_wire_version(): number;
  core_alloc(len: number): number;
  core_free(ptr: number, len: number): void;
  core_parse_tape(ptr: number, len: number): number;
  core_parse_tzx(ptr: number, len: number): number;
  core_parse_tap(ptr: number, len: number): number;
  core_serialize_tzx(ptr: number, len: number, major: number, minor: number): number;
  core_serialize_tap(ptr: number, len: number): number;
  core_serialize_blocks(ptr: number, len: number): number;
  core_required_version(ptr: number, len: number): number;
  core_save_version(ptr: number, len: number, major: number, minor: number): number;
  core_describe_block(ptr: number, len: number, hex: number): number;
  core_content_labels(ptr: number, len: number): number;
  core_detect_content(ptr: number, len: number, index: number): number;
  core_check_consistency(ptr: number, len: number, base: number): number;
  core_detect_programs(ptr: number, len: number): number;
  core_group_ranges(ptr: number, len: number): number;
  core_tape_title(ptr: number, len: number): number;
  core_decode_header(ptr: number, len: number): number;
  core_encode_header(ptr: number, len: number): number;
  core_checksum(ptr: number, len: number): number;
  core_basic_score(ptr: number, len: number): number;
  core_convert_block(ptr: number, len: number, id: number): number;
  core_blocks_equal(ptr: number, len: number, mode: number): number;
  core_compare_tapes(ptr: number, len: number, split: number, blockMode: number, tapeMode: number): number;
  core_find_matches(ptr: number, len: number, skip: number, mode: number): number;
  core_bits(ptr: number, len: number, op: number, n: number): number;
  core_flip_bytes(ptr: number, len: number): number;
  core_decode_pokes(ptr: number, len: number): number;
  core_encode_pokes(ptr: number, len: number): number;
  core_pokes_to_text(ptr: number, len: number, hex: number): number;
  core_text_to_pokes(ptr: number, len: number, hex: number): number;
  core_char_table(ptr: number, len: number, kind: number): number;
  core_render_screen(ptr: number, len: number, offset: number, flags: number): number;
  core_has_flash(ptr: number, len: number, offset: number): number;
  core_list_basic(ptr: number, len: number, start: number, end: number, flags: number): number;
  core_basic_to_text(ptr: number, len: number, flags: number): number;
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

/** The compare modes as `core/src/wasm.rs` numbers them. */
const BLOCK_MODES: BlockCompareMode[] = ['data', 'data+timings', 'data+timings+pauses'];
const TAPE_MODES: TapeCompareMode[] = ['datablocks', 'ignore-metadata', 'all'];
/** Bit operations, in the order `core_bits` expects. */
export type BitOp = 'drop' | 'add' | 'shiftLeft' | 'shiftRight' | 'join';
const BIT_OPS: BitOp[] = ['drop', 'add', 'shiftLeft', 'shiftRight', 'join'];

/** Everything but the bookkeeping exports takes a buffer and returns one. */
type Entry = Exclude<keyof CoreExports, 'memory' | 'core_wire_version' | 'core_alloc' | 'core_free'>;

/** What `core_serialize_tzx` and `core_save_version` take for "no version". */
const NO_VERSION = 0xffff;

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

export function parseTapeCore(buf: Uint8Array): ParsedTape {
  return decodeTape(call('core_parse_tape', buf));
}

export function parseTzxCore(buf: Uint8Array): ParsedTape {
  return decodeTape(call('core_parse_tzx', buf));
}

export function parseTapCore(buf: Uint8Array): ParsedTape {
  return decodeTape(call('core_parse_tap', buf));
}

export function serializeTzxCore(blocks: Block[], version?: { major: number; minor: number }): Uint8Array {
  const v = version ?? { major: NO_VERSION, minor: NO_VERSION };
  return decodeBytes(call('core_serialize_tzx', encodeBlocks(blocks), v.major, v.minor));
}

export function serializeTapCore(blocks: Block[]): { bytes: Uint8Array; skipped: number[] } {
  return decodeTap(call('core_serialize_tap', encodeBlocks(blocks)));
}

/** The blocks' own bytes, ID and body, with no file header. */
export function serializeBlocksCore(blocks: Block[]): Uint8Array {
  return decodeBytes(call('core_serialize_blocks', encodeBlocks(blocks)));
}

export function requiredVersionCore(blocks: Block[]): { major: number; minor: number } {
  return decodeVersion(call('core_required_version', encodeBlocks(blocks, { withData: false })));
}

export function saveVersionCore(
  blocks: Block[],
  loaded: { major: number; minor: number } | null,
): { major: number; minor: number } {
  const l = loaded ?? { major: NO_VERSION, minor: NO_VERSION };
  const payload = encodeBlocks(blocks, { withData: false });
  return decodeVersion(call('core_save_version', payload, l.major, l.minor));
}

/** Description and length column for one block, as the list shows them. */
export function describeBlockCore(block: Block, hex: boolean): { description: string; length: number } {
  return decodeDescribed(call('core_describe_block', encodeBlocks([block]), hex ? 1 : 0));
}

/** Content labels for a whole tape, one call rather than one per row. */
export function contentLabelsCore(blocks: Block[]): string[] {
  return decodeStrings(call('core_content_labels', encodeBlocks(blocks)));
}

/**
 * What a block contains. Only the block and the one before it matter, so that
 * is all that goes over the wire.
 */
export function detectContentCore(blocks: Block[], index: number): ContentInfo {
  const block = blocks[index];
  // Out of range: the core answers with the "nothing detected" default.
  if (!block) return decodeContent(call('core_detect_content', encodeBlocks([]), 0));
  const prev = index > 0 ? blocks[index - 1] : null;
  const window = prev ? [prev, block] : [block];
  return decodeContent(call('core_detect_content', encodeBlocks(window), prev ? 1 : 0));
}

export function checkConsistencyCore(blocks: Block[], base: number): Issue[] {
  return decodeIssues(call('core_check_consistency', encodeBlocks(blocks), base));
}

export function detectProgramsCore(blocks: Block[]): Program[] {
  return decodePrograms(call('core_detect_programs', encodeBlocks(blocks)));
}

export function groupRangesCore(blocks: Block[]): Map<number, number> {
  return decodeRanges(call('core_group_ranges', encodeBlocks(blocks, { withData: false })));
}

export function tapeTitleCore(blocks: Block[]): string | null {
  return decodeOptString(call('core_tape_title', encodeBlocks(blocks, { withData: false })));
}

export function decodeHeaderCore(data: Uint8Array): HeaderInfo | null {
  return decodeHeaderInfo(call('core_decode_header', data));
}

export function encodeHeaderCore(h: HeaderInfo): Uint8Array {
  return decodeBytes(call('core_encode_header', encodeHeaderInfo(h)));
}

export function checksumCore(data: Uint8Array): number {
  return decodeU8(call('core_checksum', data));
}

export function basicScoreCore(data: Uint8Array): number {
  return decodeF64(call('core_basic_score', data));
}

/** Change a block's type, keeping its uid and the fields the new type shares. */
export function convertBlockCore(b: Block, id: number): Block {
  const [converted] = decodeBlocks(call('core_convert_block', encodeBlocks([b]), id));
  return { ...converted, uid: b.uid };
}

export function blocksEqualCore(a: Block, b: Block, mode: BlockCompareMode): boolean {
  return decodeU8(call('core_blocks_equal', encodeBlocks([a, b]), BLOCK_MODES.indexOf(mode))) === 1;
}

export function compareTapesCore(
  left: Block[], right: Block[], blockMode: BlockCompareMode, tapeMode: TapeCompareMode,
): { left: CompareResult[]; right: CompareResult[]; identical: boolean } {
  const payload = encodeBlocks([...left, ...right]);
  return decodeComparison(
    call('core_compare_tapes', payload, left.length, BLOCK_MODES.indexOf(blockMode), TAPE_MODES.indexOf(tapeMode)),
  );
}

/** `skip` is where the needle sits in the haystack, or -1 when it is not in it. */
export function findMatchesCore(needle: Block, haystack: Block[], mode: BlockCompareMode, skip: number): number[] {
  const payload = encodeBlocks([needle, ...haystack]);
  return decodeU32s(call('core_find_matches', payload, skip < 0 ? 0xffffffff : skip, BLOCK_MODES.indexOf(mode)));
}

export function bitsCore(op: BitOp, parts: BitData[], n = 0): BitData {
  return decodeBitData(call('core_bits', encodeBitData(parts), BIT_OPS.indexOf(op), n));
}

export function flipBytesCore(data: Uint8Array): Uint8Array {
  return decodeBytes(call('core_flip_bytes', data));
}

export function decodePokesCore(data: Uint8Array): PokesInfo {
  return decodePokesInfo(call('core_decode_pokes', data));
}

export function encodePokesCore(info: PokesInfo): Uint8Array {
  return decodeBytes(call('core_encode_pokes', encodePokesInfo(info)));
}

export function pokesToTextCore(info: PokesInfo, hex: boolean): string {
  return decodeOptString(call('core_pokes_to_text', encodePokesInfo(info), hex ? 1 : 0)) ?? '';
}

export function textToPokesCore(text: string, hex: boolean): PokesInfo {
  return decodePokesInfo(call('core_text_to_pokes', new TextEncoder().encode(text), hex ? 1 : 0));
}

/** Which of the three character tables to fetch; see `core_char_table`. */
export type CharTable = 'zx' | 'zxPlain' | 'dump';

const NO_INPUT = new Uint8Array(0);

export function charTableCore(kind: CharTable): string[] {
  const id = kind === 'zx' ? 0 : kind === 'zxPlain' ? 1 : 2;
  return decodeStrings(call('core_char_table', NO_INPUT, id));
}

function basicFlags(opts: BasicOptions): number {
  return (opts.showNumbers ? 1 : 0) | (opts.basic128 ? 2 : 0) | (opts.speccyFormat ? 4 : 0);
}

export function renderScreenCore(data: Uint8Array, offset: number, opts: { hideAttributes?: boolean; flashPhase?: boolean }): Uint8ClampedArray {
  const flags = (opts.hideAttributes ? 1 : 0) | (opts.flashPhase ? 2 : 0);
  return new Uint8ClampedArray(decodeBytes(call('core_render_screen', data, offset, flags)));
}

export function hasFlashCore(data: Uint8Array, offset: number): boolean {
  return decodeU8(call('core_has_flash', data, offset)) === 1;
}

export function listBasicCore(data: Uint8Array, start: number, end: number, opts: BasicOptions): BasicLine[] {
  return decodeBasicLines(call('core_list_basic', data, start, end, basicFlags(opts)));
}

export function basicToTextCore(lines: BasicLine[], opts: BasicOptions): string {
  return decodeOptString(call('core_basic_to_text', encodeBasicLines(lines), basicFlags(opts))) ?? '';
}

export function listVariablesCore(data: Uint8Array, start: number, end: number): VariableEntry[] {
  return decodeVariables(call('core_list_variables', data, start, end));
}

export function disassembleCore(data: Uint8Array, offset: number, base: number, count: number, opts: DisOptions): DisLine[] {
  const flags = ((opts.hex ?? true) ? 1 : 0) | ((opts.romLabels !== false) ? 2 : 0);
  return decodeDisLines(call('core_disassemble', data, offset, base, count, flags));
}

export function decodeNumberCore(data: Uint8Array, offset: number): number {
  return decodeF64(call('core_decode_number', data, offset));
}

export function formatNumberCore(v: number): string {
  return decodeOptString(call('core_format_number', NO_INPUT, v)) ?? '';
}

/** How a block renders as pulses: what a PZX export or a waveform view would want. */
export interface Pulse {
  tstates: number;
  level: 0 | 1;
}

export function playbackOrderCore(blocks: Block[], stopAt48k: boolean): number[] {
  return decodeU32s(call('core_playback_order', encodeBlocks(blocks), stopAt48k ? 1 : 0));
}

export function blockDurationCore(block: Block): number {
  return decodeF64(call('core_block_duration', encodeBlocks([block])));
}

export function tapeDurationCore(blocks: Block[]): { seconds: number; order: number[] } {
  return decodeDuration(call('core_tape_duration', encodeBlocks(blocks)));
}

export function playbackTimelineCore(blocks: Block[], order: number[]): { starts: number[]; total: number } {
  return decodeTimeline(call('core_playback_timeline', encodeBlocksAndOrder(blocks, order)));
}

export function renderLengthCore(blocks: Block[], sampleRate: number, order: number[]): number {
  return decodeF64(call('core_render_length', encodeBlocksAndOrder(blocks, order), sampleRate));
}

export function renderTapeCore(
  blocks: Block[], order: number[], sampleRate: number, mic: boolean, amplitude: number,
): Float32Array {
  const payload = encodeBlocksAndOrder(blocks, order);
  return decodeSamples(call('core_render_tape', payload, sampleRate, mic ? 1 : 0, amplitude));
}

/** Render and encode in one call, so a long tape does not cross twice. */
export function renderWavCore(
  blocks: Block[], order: number[], sampleRate: number, mic: boolean, amplitude: number, bits: number,
): Uint8Array {
  const payload = encodeBlocksAndOrder(blocks, order);
  return decodeBytes(call('core_render_wav', payload, sampleRate, mic ? 1 : 0, amplitude, bits));
}

export function encodeWavCore(samples: Float32Array, sampleRate: number, bits: number): Uint8Array {
  const raw = new Uint8Array(samples.buffer, samples.byteOffset, samples.length * 4);
  return decodeBytes(call('core_encode_wav', raw, sampleRate, bits));
}

export function blockPulsesCore(block: Block): Pulse[] {
  return decodePulses(call('core_block_pulses', encodeBlocks([block])));
}

export function decodeCswRleCore(data: Uint8Array): number[] {
  return decodeU32s(call('core_decode_csw_rle', data));
}
