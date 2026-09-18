// The TypeScript tape parser as it stood before the Rust core replaced it, kept
// as the reference implementation the core is tested against (test/core.test.ts),
// the way the audio tests keep the original sample sink. It is not part of the
// app bundle and must not gain features: it is the frozen second opinion.
//
// `scripts/dump-blocks.mjs` dumps tapes through this parser to generate the
// fixtures of the Rust differential test, so the two implementations stay
// independent of each other.
import { Reader } from '../../src/tzx/bytes';
import { Block, newUid, ParsedTape, SymDef, PilotRun } from '../../src/tzx/types';

const TZX_SIGNATURE = 'ZXTape!\x1a';

export function isTzx(buf: Uint8Array): boolean {
  if (buf.length < 10) return false;
  for (let i = 0; i < 8; i++) if (buf[i] !== TZX_SIGNATURE.charCodeAt(i)) return false;
  return true;
}

export function parseTzx(buf: Uint8Array): ParsedTape {
  if (!isTzx(buf)) throw new Error('Not a TZX file (missing ZXTape! signature)');
  const r = new Reader(buf);
  r.pos = 8;
  const major = r.u8();
  const minor = r.u8();
  const blocks: Block[] = [];
  const warnings: string[] = [];
  while (!r.eof()) {
    const start = r.pos;
    const id = r.u8();
    try {
      blocks.push(parseBlock(id, r));
    } catch (e) {
      warnings.push(`Block ${blocks.length + 1} (ID ${id.toString(16).padStart(2, '0')}) at offset ${start}: ${(e as Error).message}`);
      // Keep whatever is left as an unknown block so nothing is silently lost.
      r.pos = start + 1;
      blocks.push({ uid: newUid(), id, unknown: true, raw: r.bytes(r.remaining) });
      break;
    }
  }
  return { blocks, major, minor, warnings };
}

function readSymDefs(r: Reader, count: number, maxPulses: number): SymDef[] {
  const out: SymDef[] = [];
  for (let i = 0; i < count; i++) {
    const flags = r.u8();
    const pulses: number[] = [];
    for (let p = 0; p < maxPulses; p++) pulses.push(r.u16());
    // trim trailing zero pulses
    while (pulses.length > 0 && pulses[pulses.length - 1] === 0) pulses.pop();
    out.push({ flags, pulses });
  }
  return out;
}

export function bitsPerSymbol(alphabetSize: number): number {
  if (alphabetSize <= 1) return 1;
  return Math.ceil(Math.log2(alphabetSize));
}

export function parseBlock(id: number, r: Reader): Block {
  const uid = newUid();
  switch (id) {
    case 0x10: {
      const pause = r.u16();
      const len = r.u16();
      return { uid, id, pause, data: r.bytes(len) };
    }
    case 0x11: {
      const pilot = r.u16();
      const sync1 = r.u16();
      const sync2 = r.u16();
      const zero = r.u16();
      const one = r.u16();
      const pilotLen = r.u16();
      const usedBits = r.u8();
      const pause = r.u16();
      const len = r.u24();
      return { uid, id, pilot, sync1, sync2, zero, one, pilotLen, usedBits, pause, data: r.bytes(len) };
    }
    case 0x12: {
      const pulseLen = r.u16();
      const count = r.u16();
      return { uid, id, pulseLen, count };
    }
    case 0x13: {
      const n = r.u8();
      const pulses: number[] = [];
      for (let i = 0; i < n; i++) pulses.push(r.u16());
      return { uid, id, pulses };
    }
    case 0x14: {
      const zero = r.u16();
      const one = r.u16();
      const usedBits = r.u8();
      const pause = r.u16();
      const len = r.u24();
      return { uid, id, zero, one, usedBits, pause, data: r.bytes(len) };
    }
    case 0x15: {
      const tstates = r.u16();
      const pause = r.u16();
      const usedBits = r.u8();
      const len = r.u24();
      return { uid, id, tstates, pause, usedBits, data: r.bytes(len) };
    }
    case 0x18: {
      const len = r.u32();
      // A length under the 10-byte header is corrupt; reading len - 10 bytes would
      // move the read position backwards. Report it and keep the rest verbatim.
      if (len < 10) throw new Error(`CSW block length ${len} is shorter than its 10-byte header`);
      const pause = r.u16();
      const sampleRate = r.u24();
      const compression = r.u8();
      const pulseCount = r.u32();
      return { uid, id, pause, sampleRate, compression, pulseCount, data: r.bytes(len - 10) };
    }
    case 0x19: {
      const len = r.u32();
      const end = r.pos + len;
      const pause = r.u16();
      const totp = r.u32();
      const npp = r.u8();
      let asp = r.u8();
      const totd = r.u32();
      const npd = r.u8();
      let asd = r.u8();
      if (asp === 0) asp = 256;
      if (asd === 0) asd = 256;
      let pilotSymbols: SymDef[] = [];
      let pilotStream: PilotRun[] = [];
      if (totp > 0) {
        pilotSymbols = readSymDefs(r, asp, npp);
        for (let i = 0; i < totp; i++) {
          const symbol = r.u8();
          const reps = r.u16();
          pilotStream.push({ symbol, reps });
        }
      }
      let dataSymbols: SymDef[] = [];
      let data = new Uint8Array(0);
      if (totd > 0) {
        dataSymbols = readSymDefs(r, asd, npd);
        const ds = Math.ceil((bitsPerSymbol(asd) * totd) / 8);
        data = r.bytes(ds);
      }
      if (r.pos !== end) {
        // Some files are sloppy; trust the declared block length.
        r.pos = end;
      }
      return { uid, id, pause, totp, npp, pilotSymbols, pilotStream, totd, npd, dataSymbols, data };
    }
    case 0x20:
      return { uid, id, pause: r.u16() };
    case 0x21: {
      const n = r.u8();
      return { uid, id, name: r.str(n) };
    }
    case 0x22:
      return { uid, id };
    case 0x23:
      return { uid, id, offset: r.i16() };
    case 0x24:
      return { uid, id, count: r.u16() };
    case 0x25:
      return { uid, id };
    case 0x26: {
      const n = r.u16();
      const offsets: number[] = [];
      for (let i = 0; i < n; i++) offsets.push(r.i16());
      return { uid, id, offsets };
    }
    case 0x27:
      return { uid, id };
    case 0x28: {
      const len = r.u16();
      const end = r.pos + len;
      const n = r.u8();
      const entries = [];
      for (let i = 0; i < n; i++) {
        const offset = r.i16();
        const l = r.u8();
        entries.push({ offset, text: r.str(l) });
      }
      r.pos = end;
      return { uid, id, entries };
    }
    case 0x2a: {
      r.u32(); // length, always 0
      return { uid, id };
    }
    case 0x2b: {
      r.u32(); // length, always 1
      return { uid, id, level: r.u8() };
    }
    case 0x30: {
      const n = r.u8();
      return { uid, id, text: r.str(n) };
    }
    case 0x31: {
      const time = r.u8();
      const n = r.u8();
      return { uid, id, time, text: r.str(n) };
    }
    case 0x32: {
      const len = r.u16();
      const end = r.pos + len;
      const n = r.u8();
      const entries = [];
      for (let i = 0; i < n; i++) {
        const type = r.u8();
        const l = r.u8();
        entries.push({ type, text: r.str(l) });
      }
      r.pos = end;
      return { uid, id, entries };
    }
    case 0x33: {
      const n = r.u8();
      const entries = [];
      for (let i = 0; i < n; i++) entries.push({ type: r.u8(), id: r.u8(), info: r.u8() });
      return { uid, id, entries };
    }
    case 0x35: {
      const ident = r.str(16);
      const len = r.u32();
      return { uid, id, ident, data: r.bytes(len) };
    }
    case 0x5a:
      return { uid, id, raw: r.bytes(9) };
    // Deprecated blocks: keep raw so they round-trip.
    case 0x16:
    case 0x17: {
      const start = r.pos;
      const len = r.u32();
      r.pos = start;
      return { uid, id, unknown: true, raw: r.bytes(4 + len) };
    }
    case 0x34:
      return { uid, id, unknown: true, raw: r.bytes(8) };
    case 0x40: {
      const start = r.pos;
      r.u8();
      const len = r.u24();
      r.pos = start;
      return { uid, id, unknown: true, raw: r.bytes(4 + len) };
    }
    default: {
      // General extension rule: any unknown block has a DWORD length.
      const start = r.pos;
      const len = r.u32();
      r.pos = start;
      return { uid, id, unknown: true, raw: r.bytes(4 + len) };
    }
  }
}

/** Parse a TAP file into standard speed blocks. */
export function parseTap(buf: Uint8Array): ParsedTape {
  const r = new Reader(buf);
  const blocks: Block[] = [];
  const warnings: string[] = [];
  while (!r.eof()) {
    if (r.remaining < 2) {
      warnings.push('Trailing byte ignored at end of TAP file');
      break;
    }
    const len = r.u16();
    if (len > r.remaining) {
      warnings.push(`Truncated TAP block ${blocks.length + 1}: declared ${len} bytes, ${r.remaining} available`);
      blocks.push({ uid: newUid(), id: 0x10, pause: 1000, data: r.bytes(r.remaining) });
      break;
    }
    blocks.push({ uid: newUid(), id: 0x10, pause: 1000, data: r.bytes(len) });
  }
  return { blocks, major: 1, minor: 20, warnings };
}

/** Auto-detect TZX vs TAP by signature. */
export function parseTape(buf: Uint8Array): ParsedTape {
  return isTzx(buf) ? parseTzx(buf) : parseTap(buf);
}
