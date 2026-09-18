// The TypeScript convert.ts as it stood before the Rust core replaced it, kept as
// the reference implementation the core is tested against (test/core.test.ts).
// Not part of the app bundle, and not to gain features. See test/reference/parser.ts.
// Changing a block's type while keeping whatever fields carry over (block editor type menu).
import { Block, createBlock, isUnknown, ROM_TIMINGS, SymDef, PilotRun } from '../../src/tzx/types';

/** ROM pilot length for a standard block: headers (flag < 128) use the longer pilot. */
function romPilotLen(data: Uint8Array): number {
  return data.length > 0 && data[0] < 128 ? ROM_TIMINGS.pilotHeader : ROM_TIMINGS.pilotData;
}

function romPilot(reps: number): { pilotSymbols: SymDef[]; pilotStream: PilotRun[]; totp: number; npp: number } {
  return {
    npp: 2,
    totp: 2,
    pilotSymbols: [{ flags: 0, pulses: [ROM_TIMINGS.pilot] }, { flags: 0, pulses: [ROM_TIMINGS.sync1, ROM_TIMINGS.sync2] }],
    pilotStream: [{ symbol: 0, reps }, { symbol: 1, reps: 1 }],
  };
}

function dataSymbols(zero: number, one: number): SymDef[] {
  return [{ flags: 0, pulses: [zero, zero] }, { flags: 0, pulses: [one, one] }];
}

/**
 * Convert `b` to block type `id`, keeping its uid and every field the new type shares with
 * the old one (pause, data, used bits, timings, text). Unknown blocks are returned unchanged.
 */
export function convertBlock(b: Block, id: number): Block {
  if (isUnknown(b)) return b;
  if (b.id === id) return b;
  const nb = createBlock(id) as Record<string, unknown> & Block;
  const o = b as Record<string, unknown> & Block;
  nb.uid = b.uid;
  for (const k of ['pause', 'usedBits', 'pilot', 'sync1', 'sync2', 'zero', 'one', 'pilotLen']) {
    if (k in nb && typeof o[k] === 'number') nb[k] = o[k];
  }
  if ('data' in nb && o.data instanceof Uint8Array) nb.data = o.data;
  if (id === 0x11 && b.id === 0x10) nb.pilotLen = romPilotLen(b.data);
  if (id === 0x19 && o.data instanceof Uint8Array) {
    nb.totd = o.data.length * 8;
    if (b.id === 0x11) {
      Object.assign(nb, {
        npp: 2, totp: 2,
        pilotSymbols: [{ flags: 0, pulses: [b.pilot] }, { flags: 0, pulses: [b.sync1, b.sync2] }],
        pilotStream: [{ symbol: 0, reps: b.pilotLen }, { symbol: 1, reps: 1 }],
        dataSymbols: dataSymbols(b.zero, b.one),
      });
    } else if (b.id === 0x10) {
      Object.assign(nb, romPilot(romPilotLen(b.data)));
    } else if (b.id === 0x14) {
      nb.dataSymbols = dataSymbols(b.zero, b.one);
    }
  }
  // Text-like fields: description/message text and group names carry over both ways.
  if ('text' in nb && typeof o.text === 'string') nb.text = o.text;
  if ('text' in nb && typeof o.name === 'string') nb.text = o.name;
  if ('name' in nb && typeof o.text === 'string') nb.name = o.text;
  return nb;
}
