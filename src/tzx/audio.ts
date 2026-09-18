// Converting a block list into an edge/pulse stream and then into PCM samples.
// The rendering is the Rust core in core/ (audio.rs), which follows loops,
// jumps, calls and returns exactly like an emulator would.
//
// Z-RLE CSW blocks are the one thing that stays here: inflating them needs zlib,
// and the crate has no dependencies, so a tape is walked for compressed CSW
// blocks and those are handed over already inflated.
//
// The previous implementation lives on as test/reference/audio.ts.
import { inflate } from 'pako';
import { Block, isUnknown } from './types';
import {
  blockDurationCore, blockPulsesCore, decodeCswRleCore, encodeWavCore, playbackOrderCore,
  playbackTimelineCore, Pulse, renderLengthCore, renderTapeCore, renderWavCore, tapeDurationCore,
} from './core';
import { perTape } from './cache';

export const TSTATES_PER_SEC = 3500000;

/** Silence before and after the rendered tape, in T-states. */
export const LEAD_TSTATES = TSTATES_PER_SEC / 2;

export type { Pulse };

export interface FlowOptions {
  /** Called when a "stop the tape" (pause 0) or 48k stop is reached. */
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
    // does not narrow it away (see CLAUDE.md).
    if (isUnknown(b) || b.id !== 0x18 || b.compression !== 2) return b;
    changed = true;
    return { ...b, compression: 1, data: cswRleData(b.data, 2) };
  });
  return changed ? out : blocks;
});

/** Decode CSW v2 RLE pulse lengths (in samples). Z-RLE must be inflated first. */
export function decodeCswRle(data: Uint8Array): number[] {
  return decodeCswRleCore(data);
}

/** Walk the tape in playback order, yielding the block index sequence. */
export function playbackOrder(blocks: Block[], opts: FlowOptions = {}): number[] {
  return playbackOrderCore(blocks, opts.stopAt48k === true);
}

/** Total T-states for a block in isolation. */
export function blockDuration(b: Block): number {
  return blockDurationCore(playable([b])[0]);
}

/** The pulses one block produces, at the level held during each. */
export function blockPulses(b: Block): Pulse[] {
  return blockPulsesCore(playable([b])[0]);
}

/** Duration in seconds of the whole tape following playback flow. */
export function tapeDuration(blocks: Block[]): { seconds: number; order: number[] } {
  return tapeDurationCore(playable(blocks));
}

/** Where each block of the playback order starts, without rendering. */
export function playbackTimeline(blocks: Block[], order: number[]): { starts: number[]; total: number } {
  return playbackTimelineCore(playable(blocks), order);
}

/** Index into the playback order of the block playing at `tstates` (the first block during lead-in). */
export function positionAt(starts: number[], tstates: number): number {
  // Stays in TypeScript: a binary search over an array the caller already holds,
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
  return renderLengthCore(playable(blocks), sampleRate, order);
}

export function renderTape(blocks: Block[], opts: RenderOptions, indices?: number[]): Float32Array {
  const tape = playable(blocks);
  const order = indices ?? playbackOrder(tape);
  return renderTapeCore(tape, order, opts.sampleRate, opts.mode === 'mic', opts.amplitude ?? 0.8);
}

/**
 * Render straight to a WAV file. The samples of a long tape are tens of
 * megabytes, so this keeps them inside the core instead of copying them out
 * only to send them back in.
 */
export function renderWav(blocks: Block[], opts: RenderOptions, bits: 8 | 16 = 16, indices?: number[]): Uint8Array {
  const tape = playable(blocks);
  const order = indices ?? playbackOrder(tape);
  return renderWavCore(tape, order, opts.sampleRate, opts.mode === 'mic', opts.amplitude ?? 0.8, bits);
}

/** Encode samples as a 16-bit mono PCM WAV file. */
export function encodeWav(samples: Float32Array, sampleRate: number, bits: 8 | 16 = 16): Uint8Array {
  return encodeWavCore(samples, sampleRate, bits);
}
