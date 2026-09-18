// The TypeScript TZX/TAP writer as it stood before the Rust core replaced it,
// kept as the reference implementation the core is tested against
// (test/core.test.ts). Not part of the app bundle, and not to gain features:
// it is the frozen second opinion. See test/reference/parser.ts.
import { Writer } from '../../src/tzx/bytes';
import { Block, isUnknown } from '../../src/tzx/types';
import { bitsPerSymbol } from '../../src/tzx/parser';

/** Lowest TZX version able to represent these blocks. */
export function requiredVersion(blocks: Block[]): { major: number; minor: number } {
  let minor = 0;
  let hasUnknown = false;
  for (const b of blocks) {
    if (isUnknown(b)) {
      hasUnknown = true;
      continue;
    }
    let need = 0;
    switch (b.id) {
      case 0x35:
        need = 1;
        break;
      case 0x24:
      case 0x25:
      case 0x26:
      case 0x27:
      case 0x28:
      case 0x5a:
        need = 10;
        break;
      case 0x2a:
        need = 12;
        break;
      case 0x18:
      case 0x19:
      case 0x2b:
        need = 20;
        break;
      case 0x32:
        // Field types 04+ and multi-line entries need 1.10/1.12.
        for (const e of b.entries) {
          if (e.type >= 5 && e.type <= 8) need = Math.max(need, 12);
          else if (e.type === 4 || e.text.includes('\n') || e.text.includes('\r')) need = Math.max(need, 10);
        }
        break;
      case 0x33:
        for (const e of b.entries) {
          if (e.type === 0 && e.id >= 0x1e && e.id <= 0x2d) need = Math.max(need, 20);
          else if (e.type === 0 && (e.id === 0x1c || e.id === 0x1d)) need = Math.max(need, 13);
          else if (e.type === 0 && (e.id === 0x1a || e.id === 0x1b)) need = Math.max(need, 12);
          else if (e.type === 0 && e.id >= 0x15 && e.id <= 0x19) need = Math.max(need, 2);
          else if (e.type === 1 && e.id >= 0x12) need = Math.max(need, 20);
          else if (e.type === 2 && e.id >= 0x06) need = Math.max(need, 20);
          else if (e.type === 3 && e.id >= 0x06) need = Math.max(need, 20);
          else if (e.type === 6 && e.id >= 0x03) need = Math.max(need, 20);
          else if (e.type === 0x0b && e.id >= 0x03) need = Math.max(need, 20);
          else if (e.type === 0x10) need = Math.max(need, 20);
        }
        break;
    }
    if (need > minor) minor = need;
  }
  // Unknown blocks: we cannot judge; assume the newest spec.
  if (hasUnknown) minor = Math.max(minor, 20);
  return { major: 1, minor };
}

/**
 * Version to write in the header: the lowest one the blocks need, but never below the
 * version the tape was loaded with, so an unaltered file round-trips byte for byte.
 */
export function saveVersion(blocks: Block[], loaded: { major: number; minor: number } | null): { major: number; minor: number } {
  const v = requiredVersion(blocks);
  if (!loaded) return v;
  const newer = loaded.major > v.major || (loaded.major === v.major && loaded.minor > v.minor);
  return newer ? { major: loaded.major, minor: loaded.minor } : v;
}

export function writeBlock(w: Writer, b: Block): void {
  w.u8(b.id);
  if (isUnknown(b)) {
    w.bytes(b.raw);
    return;
  }
  switch (b.id) {
    case 0x10:
      w.u16(b.pause);
      w.u16(b.data.length);
      w.bytes(b.data);
      break;
    case 0x11:
      w.u16(b.pilot);
      w.u16(b.sync1);
      w.u16(b.sync2);
      w.u16(b.zero);
      w.u16(b.one);
      w.u16(b.pilotLen);
      w.u8(b.usedBits);
      w.u16(b.pause);
      w.u24(b.data.length);
      w.bytes(b.data);
      break;
    case 0x12:
      w.u16(b.pulseLen);
      w.u16(b.count);
      break;
    case 0x13:
      w.u8(b.pulses.length);
      for (const p of b.pulses) w.u16(p);
      break;
    case 0x14:
      w.u16(b.zero);
      w.u16(b.one);
      w.u8(b.usedBits);
      w.u16(b.pause);
      w.u24(b.data.length);
      w.bytes(b.data);
      break;
    case 0x15:
      w.u16(b.tstates);
      w.u16(b.pause);
      w.u8(b.usedBits);
      w.u24(b.data.length);
      w.bytes(b.data);
      break;
    case 0x18:
      w.u32(10 + b.data.length);
      w.u16(b.pause);
      w.u24(b.sampleRate);
      w.u8(b.compression);
      w.u32(b.pulseCount);
      w.bytes(b.data);
      break;
    case 0x19: {
      const body = new Writer();
      body.u16(b.pause);
      const asp = b.pilotSymbols.length;
      const asd = b.dataSymbols.length;
      const totp = b.totp > 0 && asp > 0 ? b.pilotStream.length : 0;
      const totd = b.totd > 0 && asd > 0 ? b.totd : 0;
      body.u32(totp);
      body.u8(b.npp);
      body.u8(asp >= 256 ? 0 : asp);
      body.u32(totd);
      body.u8(b.npd);
      body.u8(asd >= 256 ? 0 : asd);
      if (totp > 0) {
        for (const s of b.pilotSymbols) {
          body.u8(s.flags);
          for (let i = 0; i < b.npp; i++) body.u16(s.pulses[i] ?? 0);
        }
        for (const run of b.pilotStream) {
          body.u8(run.symbol);
          body.u16(run.reps);
        }
      }
      if (totd > 0) {
        for (const s of b.dataSymbols) {
          body.u8(s.flags);
          for (let i = 0; i < b.npd; i++) body.u16(s.pulses[i] ?? 0);
        }
        const ds = Math.ceil((bitsPerSymbol(asd) * totd) / 8);
        const data = new Uint8Array(ds);
        data.set(b.data.subarray(0, Math.min(ds, b.data.length)));
        body.bytes(data);
      }
      w.u32(body.length);
      w.bytes(body.toUint8Array());
      break;
    }
    case 0x20:
      w.u16(b.pause);
      break;
    case 0x21:
      w.u8(Math.min(255, b.name.length));
      w.str(b.name.slice(0, 255));
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
      w.u16(b.offsets.length);
      for (const o of b.offsets) w.i16(o);
      break;
    case 0x27:
      break;
    case 0x28: {
      const body = new Writer();
      body.u8(b.entries.length);
      for (const e of b.entries) {
        body.i16(e.offset);
        const t = e.text.slice(0, 255);
        body.u8(t.length);
        body.str(t);
      }
      w.u16(body.length);
      w.bytes(body.toUint8Array());
      break;
    }
    case 0x2a:
      w.u32(0);
      break;
    case 0x2b:
      w.u32(1);
      w.u8(b.level);
      break;
    case 0x30: {
      const t = b.text.slice(0, 255);
      w.u8(t.length);
      w.str(t);
      break;
    }
    case 0x31: {
      const t = b.text.slice(0, 255);
      w.u8(b.time);
      w.u8(t.length);
      w.str(t);
      break;
    }
    case 0x32: {
      const body = new Writer();
      body.u8(b.entries.length);
      for (const e of b.entries) {
        const t = e.text.slice(0, 255);
        body.u8(e.type);
        body.u8(t.length);
        body.str(t);
      }
      w.u16(body.length);
      w.bytes(body.toUint8Array());
      break;
    }
    case 0x33:
      w.u8(b.entries.length);
      for (const e of b.entries) {
        w.u8(e.type);
        w.u8(e.id);
        w.u8(e.info);
      }
      break;
    case 0x35:
      w.str((b.ident + '                ').slice(0, 16));
      w.u32(b.data.length);
      w.bytes(b.data);
      break;
    case 0x5a:
      w.bytes(b.raw.length === 9 ? b.raw : new Uint8Array([0x58, 0x54, 0x61, 0x70, 0x65, 0x21, 0x1a, 1, 20]));
      break;
  }
}

export function serializeTzx(blocks: Block[], version?: { major: number; minor: number }): Uint8Array {
  const v = version ?? requiredVersion(blocks);
  const w = new Writer();
  w.str('ZXTape!\x1a');
  w.u8(v.major);
  w.u8(v.minor);
  for (const b of blocks) writeBlock(w, b);
  return w.toUint8Array();
}

/** Serialize a single block to bytes (ID + body); used for comparison and size display. */
export function serializeBlock(b: Block): Uint8Array {
  const w = new Writer();
  writeBlock(w, b);
  return w.toUint8Array();
}

/** TAP export: only standard-speed blocks can be represented. Returns skipped block indices. */
export function serializeTap(blocks: Block[]): { bytes: Uint8Array; skipped: number[] } {
  const w = new Writer();
  const skipped: number[] = [];
  blocks.forEach((b, i) => {
    if (isUnknown(b)) {
      skipped.push(i);
      return;
    }
    if (b.id === 0x10 || b.id === 0x11 || b.id === 0x14) {
      w.u16(b.data.length);
      w.bytes(b.data);
    } else if (b.id === 0x19 && b.data.length > 0) {
      w.u16(b.data.length);
      w.bytes(b.data);
    } else if (b.id === 0x20 || b.id === 0x21 || b.id === 0x22 || b.id === 0x30 || b.id === 0x31 || b.id === 0x32 || b.id === 0x33 || b.id === 0x35 || b.id === 0x2a || b.id === 0x5a) {
      // metadata: silently dropped
    } else {
      skipped.push(i);
    }
  });
  return { bytes: w.toUint8Array(), skipped };
}
