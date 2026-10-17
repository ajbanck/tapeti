// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

// Decodes a recording off the page's thread. It is handed the module the page compiled, so the
// wasm is neither downloaded nor compiled twice, and answers with the entry point's payload as
// it is: the page decodes it with the wire reader it already has, after reports of how far the
// decode has come. Cancelling is terminating it.
// It exports nothing and imports only types: it runs as a classic worker, which Safari 14.1
// needs, and a classic worker cannot hold an `export`.

import type { DecodeAnswer, DecodeJob } from './decode-job';

interface DecodeExports {
  memory: WebAssembly.Memory;
  core_alloc(len: number): number;
  core_free(ptr: number, len: number): void;
  decode_recording(ptr: number, len: number, decode: number, channel: number, filter: number): number;
}

interface Scope {
  onmessage: ((e: MessageEvent<DecodeJob>) => void) | null;
  postMessage(message: DecodeAnswer, transfer?: Transferable[]): void;
}
const scope = self as unknown as Scope;

scope.onmessage = async (e) => {
  const job = e.data;
  try {
    const progress = (blocks: number, done: number) => scope.postMessage({ progress: { blocks, done: done / 1000 } });
    const instance = await WebAssembly.instantiate(job.module, { decode: { decode_progress: progress } });
    const x = instance.exports as unknown as DecodeExports;
    const at = x.core_alloc(job.request.length);
    new Uint8Array(x.memory.buffer, at, job.request.length).set(job.request);
    const out = x.decode_recording(at, job.request.length, job.decode, job.channel, job.filter);
    x.core_free(at, job.request.length);
    if (out === 0) throw new Error('The decoder could not allocate a result');
    const len = new DataView(x.memory.buffer).getUint32(out, true);
    const payload = new Uint8Array(x.memory.buffer, out + 4, len).slice();
    scope.postMessage({ payload }, [payload.buffer]);
  } catch (err) {
    // A wasm trap here is most often the memory a long recording needs.
    scope.postMessage({ error: err instanceof Error ? err.message : String(err) });
  }
};
