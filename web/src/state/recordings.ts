// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

// Tape recordings: described on the page for the import dialog, then decoded, or kept as raw
// signal, in a Worker, so a long one does not stop the page. One at a time.
import { signal } from '@preact/signals';
import { Side, dialog, setStatus, showMessage, blockNo } from './store';
import { loadParsed } from './files';
import { coreModule, describeRecording, recordingRequest } from '../tzx/core';
import { decodeTape } from '../tzx/wire';
import type { DecodeAnswer, DecodeJob, DecodeProgress } from '../tzx/decode-job';

/**
 * The largest recording the browser takes: a tab holds the file, its copy in the module and its
 * pulses, and the module's memory cannot grow past what the browser allows a tab. 256 MB is about
 * 50 minutes of 16-bit mono at 44.1 kHz. Measured in Chrome on macOS (2026-10-15), mono at
 * 44.1 kHz: 256 MB decoded in 17 s and 512 MB in 37 s, and a 768 MB file brought the tab down.
 * Half of what decoded leaves room for a stereo file, decoded three times, and a smaller machine.
 */
export const MAX_RECORDING_BYTES = 256 * 1024 * 1024;

/** The recording being taken in, how far its decode has come (null while kept as raw signal), and the way to stop it. */
export const decoding = signal<{ name: string; progress: DecodeProgress | null; cancel: () => void } | null>(null);

/** A recording was opened: ask how to take it in. */
export function openRecording(side: Side, name: string, bytes: Uint8Array, insertAtCursor: boolean) {
  if (bytes.length > MAX_RECORDING_BYTES) {
    const mb = (n: number) => `${Math.round(n / (1024 * 1024))} MB`;
    showMessage('Cannot load file', [
      `${name} is ${mb(bytes.length)}; the browser takes recordings of up to ${mb(MAX_RECORDING_BYTES)}.`,
      'The desktop app opens longer ones.',
    ]);
    return;
  }
  try {
    const request = recordingRequest(name, bytes);
    dialog.value = { kind: 'recording', side, name, request, summary: describeRecording(request), insertAtCursor };
  } catch (e) {
    showMessage('Cannot load file', (e as Error).message);
  }
}

export interface RecordingChoice {
  /** Into blocks, or kept as raw signal. */
  decode: boolean;
  /** 0 both mixed, 1 left, 2 right, 3 whichever decodes best (both mixed for raw signal). */
  channel: number;
  filter: boolean;
}

/** The recording dialog's Import: the work goes to a Worker, and the tape lands when it is done. */
export function importRecording(side: Side, name: string, request: Uint8Array, choice: RecordingChoice, insertAtCursor: boolean) {
  decoding.value?.cancel();
  // The build's worker is a classic script: Safari 14.1 runs no module workers. The dev server
  // can only serve a module worker (it puts an ES module in front of a classic one), and the
  // build drops this branch.
  const worker = import.meta.env.DEV
    ? new Worker(new URL('../tzx/decode.worker.ts', import.meta.url), { type: 'module' })
    : new Worker(new URL('../tzx/decode.worker.ts', import.meta.url));
  let live = true;
  const done = () => {
    live = false;
    worker.terminate();
    decoding.value = null;
  };
  decoding.value = {
    name,
    progress: choice.decode ? { blocks: 0, done: 0 } : null,
    cancel: () => {
      done();
      setStatus(`Stopped decoding ${name}`);
    },
  };
  setStatus(choice.decode ? `Decoding ${name}…` : `Reading ${name}…`);
  worker.onmessage = (e: MessageEvent<DecodeAnswer>) => {
    if ('progress' in e.data) {
      if (live && decoding.value) decoding.value = { ...decoding.value, progress: e.data.progress };
      return;
    }
    done();
    if ('error' in e.data) {
      setStatus(`Could not import ${name}`);
      showMessage('Cannot import recording', e.data.error);
      return;
    }
    let parsed;
    try {
      parsed = decodeTape(e.data.payload);
    } catch (err) {
      showMessage('Cannot import recording', (err as Error).message);
      return;
    }
    // Findings travel as the tape's warnings: class, block, text.
    const findings = parsed.warnings.map((w) => w.split('\t'));
    const problems = findings
      .filter(([cls]) => cls === 'P')
      .map(([, block, text]) => (block === '-' ? text : `Block ${blockNo(Number(block))}: ${text}`));
    const raw = findings.filter(([, , text]) => text === 'Kept as raw signal.').length;
    loadParsed(side, name, { ...parsed, warnings: [] }, insertAtCursor, false, null);
    if (problems.length) showMessage(`Decoded ${name}`, problems);
    const n = parsed.blocks.length;
    setStatus(choice.decode
      ? `Decoded ${name}: ${n} block${n === 1 ? '' : 's'}${raw ? `, ${raw} kept as raw signal` : ''}`
      : `Imported ${name} as raw signal: ${n} block${n === 1 ? '' : 's'}`);
  };
  worker.onerror = (e) => {
    done();
    setStatus(`Could not import ${name}`);
    showMessage('Cannot import recording', e.message || 'The decoder stopped unexpectedly.');
  };
  const job: DecodeJob = {
    module: coreModule(),
    request,
    decode: choice.decode ? 1 : 0,
    channel: choice.channel,
    filter: choice.filter ? 1 : 0,
  };
  worker.postMessage(job);
}
