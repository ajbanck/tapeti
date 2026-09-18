import { signal } from '@preact/signals';
import { Block } from '../tzx/types';
import { renderTape, playbackTimeline, playbackOrder, positionAt, TSTATES_PER_SEC } from '../tzx/audio';
import { Side, audioMode, setStatus } from './store';

export const playing = signal(false);
/** Which pane's tape is playing, so only its list shows the marker. */
export const playingSide = signal<Side | null>(null);
/** Block index (in the tape) currently being played, or -1. */
export const playingBlock = signal(-1);
/** Elapsed and total seconds of the current playback; both 0 when idle. */
export const playPos = signal({ elapsed: 0, total: 0 });

let ctx: AudioContext | null = null;
let source: AudioBufferSourceNode | null = null;
let frame = 0;

export function playBlocks(blocks: Block[], indices?: number[], side: Side | null = null) {
  stopPlayback();
  if (!ctx) ctx = new AudioContext();
  if (ctx.state === 'suspended') void ctx.resume(); // browsers start it suspended until a user gesture
  const rate = ctx.sampleRate;
  const order = indices ?? playbackOrder(blocks);
  const samples = renderTape(blocks, { sampleRate: rate, mode: audioMode.value }, order);
  if (samples.length === 0) {
    setStatus('Nothing to play');
    return;
  }
  const buffer = ctx.createBuffer(1, samples.length, rate);
  buffer.copyToChannel(samples as Float32Array<ArrayBuffer>, 0);
  const node = ctx.createBufferSource();
  source = node;
  node.buffer = buffer;
  node.connect(ctx.destination);
  node.onended = () => {
    if (source === node) stopPlayback();
  };
  const startAt = ctx.currentTime;
  node.start(startAt);
  const total = samples.length / rate;
  const { starts } = playbackTimeline(blocks, order);
  playing.value = true;
  playingSide.value = side;
  playingBlock.value = order[0] ?? -1;
  playPos.value = { elapsed: 0, total };
  setStatus('');
  const tick = () => {
    if (source !== node || !ctx) return;
    // Output latency is not exposed by every WebKit; fall back to the context clock alone.
    const latency = (ctx as AudioContext & { outputLatency?: number }).outputLatency ?? 0;
    const elapsed = Math.max(0, Math.min(total, ctx.currentTime - startAt - latency));
    const at = order[positionAt(starts, elapsed * TSTATES_PER_SEC)] ?? -1;
    if (at !== playingBlock.value) playingBlock.value = at;
    playPos.value = { elapsed, total };
    frame = requestAnimationFrame(tick);
  };
  frame = requestAnimationFrame(tick);
}

export function stopPlayback() {
  cancelAnimationFrame(frame);
  if (source) {
    const node = source;
    source = null;
    try { node.stop(); } catch { /* already stopped */ }
  }
  playing.value = false;
  playingSide.value = null;
  playingBlock.value = -1;
  playPos.value = { elapsed: 0, total: 0 };
}
