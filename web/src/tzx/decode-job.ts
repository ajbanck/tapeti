// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

// What the page and the decode worker (decode.worker.ts) send each other.

export interface DecodeJob {
  module: WebAssembly.Module;
  /** As `recordingRequest` makes it. */
  request: Uint8Array;
  /** 1 to decode into blocks, 0 to keep as raw signal. */
  decode: number;
  /** 0 both mixed, 1 left, 2 right, 3 whichever decodes best (both mixed for raw signal). */
  channel: number;
  /** 1 for the tape band-pass. */
  filter: number;
}

/** How far a decode has come: the blocks read so far, and the share of the work done, 0 to 1. */
export interface DecodeProgress {
  blocks: number;
  done: number;
}

/** The tape (once), or why there is none; before either, how far the decode has come. */
export type DecodeAnswer = { payload: Uint8Array } | { error: string } | { progress: DecodeProgress };
