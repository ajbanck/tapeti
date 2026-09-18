// Prints the canonical block dump of a tape, as read by the TypeScript parser
// that the Rust core replaced (test/reference/parser.ts, frozen). The Rust core
// prints the same format (core/src/dump.rs), so these dumps are the fixtures of
// a differential test between two independent implementations:
//
//   node scripts/dump-blocks.mjs public/samples/*.tzx public/samples/*.tap
//
// writes core/tests/fixtures/<name>.dump for each tape, which `cargo test` in
// core/ compares the Rust parser against. The reference parser is frozen, so
// these only need regenerating when a tape is added or the dump format changes.
import fs from 'node:fs';
import path from 'node:path';
import { createServer } from 'vite';

const files = process.argv.slice(2);
if (files.length === 0) {
  console.error('usage: node scripts/dump-blocks.mjs <tape>...');
  process.exit(2);
}

const OUT = path.resolve('core/tests/fixtures');
const vite = await createServer({ server: { middlewareMode: true }, appType: 'custom', logLevel: 'error' });
try {
  const { parseTape } = await vite.ssrLoadModule('/test/reference/parser.ts');

  const hex = (b) => Array.from(b, (v) => v.toString(16).padStart(2, '0')).join('');
  const join = (parts) => parts.join(';');
  const nums = (v) => join(Array.from(v, (n) => String(n)));
  const syms = (v) => join(v.map((s) => `${s.flags}/${nums(s.pulses)}`));
  const quote = (s) => {
    let out = '"';
    for (const c of s) {
      const v = c.codePointAt(0);
      out += c === '"' || c === '\\' || v < 0x20 || v > 0x7e ? `\\x${v.toString(16).padStart(2, '0')}` : c;
    }
    return out + '"';
  };

  // Field name -> how to render it. The order per block id is fixed below.
  const render = {
    data: hex, raw: hex,
    pulses: nums, offsets: nums,
    pilotSymbols: syms, dataSymbols: syms,
    pilotStream: (v) => join(v.map((p) => `${p.symbol}*${p.reps}`)),
    name: quote, text: quote, ident: quote,
    unknown: () => 'true',
  };
  const FIELDS = {
    0x10: ['pause', 'data'],
    0x11: ['pilot', 'sync1', 'sync2', 'zero', 'one', 'pilotLen', 'usedBits', 'pause', 'data'],
    0x12: ['pulseLen', 'count'],
    0x13: ['pulses'],
    0x14: ['zero', 'one', 'usedBits', 'pause', 'data'],
    0x15: ['tstates', 'pause', 'usedBits', 'data'],
    0x18: ['pause', 'sampleRate', 'compression', 'pulseCount', 'data'],
    0x19: ['pause', 'totp', 'npp', 'pilotSymbols', 'pilotStream', 'totd', 'npd', 'dataSymbols', 'data'],
    0x20: ['pause'],
    0x21: ['name'],
    0x22: [],
    0x23: ['offset'],
    0x24: ['count'],
    0x25: [],
    0x26: ['offsets'],
    0x27: [],
    0x28: ['entries:select'],
    0x2a: [],
    0x2b: ['level'],
    0x30: ['text'],
    0x31: ['time', 'text'],
    0x32: ['entries:archive'],
    0x33: ['entries:hardware'],
    0x35: ['ident', 'data'],
    0x5a: ['raw'],
  };
  const ENTRIES = {
    select: (v) => join(v.map((e) => `${e.offset}/${quote(e.text)}`)),
    archive: (v) => join(v.map((e) => `${e.type}/${quote(e.text)}`)),
    hardware: (v) => join(v.map((e) => `${e.type}/${e.id}/${e.info}`)),
  };

  const dumpBlock = (i, b) => {
    const fields = b.unknown ? ['unknown', 'raw'] : FIELDS[b.id];
    if (!fields) throw new Error(`no field list for block id ${b.id.toString(16)}`);
    let s = `${i} ${b.id.toString(16).padStart(2, '0')}`;
    for (const spec of fields) {
      const [name, kind] = spec.split(':');
      const value = kind ? ENTRIES[kind](b[name]) : (render[name] ?? String)(b[name]);
      s += ` ${name}=${value}`;
    }
    return s;
  };

  for (const file of files) {
    const bytes = new Uint8Array(fs.readFileSync(file));
    const tape = parseTape(bytes);
    // First line names the source tape, so the Rust test can find it; the
    // dump itself starts at the `tape` line.
    let out = `file ${path.relative(process.cwd(), file)}\n`;
    out += `tape ${tape.major}.${tape.minor}\n`;
    for (const w of tape.warnings) out += `warn ${w}\n`;
    tape.blocks.forEach((b, i) => {
      out += dumpBlock(i, b) + '\n';
    });
    const name = path.basename(file).replace(/\s+/g, '-') + '.dump';
    fs.mkdirSync(OUT, { recursive: true });
    fs.writeFileSync(path.join(OUT, name), out);
    console.log(`${name}: ${tape.blocks.length} blocks, ${tape.warnings.length} warnings`);
  }
} finally {
  await vite.close();
}
