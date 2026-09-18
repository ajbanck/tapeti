// Generates the sample tapes in public/samples/ from scratch, using the app's own
// TZX writer. Nothing in them is copied from a real tape: the BASIC program, the
// screen, the machine code and the loader timings are all made up here, so the
// samples can be redistributed under the project licence.
//
//   node scripts/make-samples.mjs
//
// Produces:
//   Tapeti demo.tzx            every block type the editor can create, in one tape
//   Tapeti demo (variant).tzx  a variation of the same program for the compare modes
//   Tapeti demo.tap            the TAP export of the demo tape
import fs from 'node:fs';
import path from 'node:path';
import { createServer } from 'vite';

const OUT = path.resolve('public/samples');
const vite = await createServer({ server: { middlewareMode: true }, appType: 'custom', logLevel: 'error' });
try {
  const { serializeTzx, serializeTap } = await vite.ssrLoadModule('/src/tzx/writer.ts');
  const { createBlock } = await vite.ssrLoadModule('/src/tzx/types.ts');
  const { encodeHeader, checksum } = await vite.ssrLoadModule('/src/tzx/describe.ts');
  const { encodePokes } = await vite.ssrLoadModule('/src/tzx/pokes.ts');
  const { checkConsistency } = await vite.ssrLoadModule('/src/tzx/consistency.ts');
  const { parseTzx } = await vite.ssrLoadModule('/src/tzx/parser.ts');
  // Parsing runs in the Rust core now, which has to be instantiated first.
  await (await vite.ssrLoadModule('/src/tzx/core.ts')).initCore();

  // ---- helpers ---------------------------------------------------------------

  /** Block of the given id with fields overridden. */
  const block = (id, fields = {}) => Object.assign(createBlock(id), fields);

  /** Flag byte + body + XOR checksum, as the ROM loader expects. */
  const romData = (flag, body) => {
    const d = new Uint8Array(body.length + 2);
    d[0] = flag;
    d.set(body, 1);
    d[d.length - 1] = checksum(d, 0, d.length - 1);
    return d;
  };

  const headerBlock = (h) => block(0x10, { pause: 1000, data: romData(0x00, encodeHeader(h).subarray(1, 18)) });
  const latin1 = (s) => Uint8Array.from(s, (c) => c.charCodeAt(0) & 0xff);

  // ---- a small BASIC program ------------------------------------------------

  const T = {
    BORDER: 0xe7, PAPER: 0xda, INK: 0xd9, CLS: 0xfb, PRINT: 0xf5, LOAD: 0xef, SCREEN$: 0xaa, CODE: 0xaf,
    RANDOMIZE: 0xf9, USR: 0xc0, REM: 0xea, PAUSE: 0xf2, AT: 0xac, LET: 0xf1, FOR: 0xeb, TO: 0xcc, NEXT: 0xf3,
    BEEP: 0xb7, GOTO: 0xec,
  };
  /** A number as it appears in a program: its digits, then 0x0E and the 5-byte binary form. */
  const num = (n) => [...latin1(String(n)), 0x0e, 0, 0, n & 0xff, (n >> 8) & 0xff, 0];
  const str = (s) => [0x22, ...latin1(s), 0x22];
  const line = (no, ...parts) => {
    const body = [...parts.flat(), 0x0d];
    return [(no >> 8) & 0xff, no & 0xff, body.length & 0xff, (body.length >> 8) & 0xff, ...body];
  };
  const program = (lines) => Uint8Array.from(lines.flat());

  const demoBasic = program([
    line(10, T.REM, ...latin1(' Tapeti demo tape ')),
    line(20, T.BORDER, num(1), 0x3a, T.PAPER, num(1), 0x3a, T.INK, num(7), 0x3a, T.CLS),
    line(30, T.PRINT, T.AT, num(10), 0x2c, num(9), 0x3b, str('Loading...')),
    line(40, T.LOAD, str(''), T.SCREEN$),
    line(50, T.LOAD, str(''), T.CODE),
    line(60, T.RANDOMIZE, T.USR, num(32768)),
  ]);
  const variantBasic = program([
    line(10, T.REM, ...latin1(' Tapeti demo tape, variant ')),
    line(20, T.BORDER, num(0), 0x3a, T.PAPER, num(0), 0x3a, T.INK, num(6), 0x3a, T.CLS),
    line(30, T.PRINT, T.AT, num(10), 0x2c, num(9), 0x3b, str('Loading...')),
    line(40, T.LOAD, str(''), T.SCREEN$),
    line(45, T.FOR, ...latin1('n='), num(1), T.TO, num(3), 0x3a, T.BEEP, ...latin1('.1,n*4'), 0x3a, T.NEXT, ...latin1('n')),
    line(50, T.LOAD, str(''), T.CODE),
    line(60, T.RANDOMIZE, T.USR, num(32768)),
  ]);

  // ---- a screen: the word TAPETI in a chunky 5x7 font, with colour bands ------------

  const FONT = {
    P: ['11110', '10001', '10001', '11110', '10000', '10000', '10000'],
    E: ['11111', '10000', '10000', '11110', '10000', '10000', '11111'],
    T: ['11111', '00100', '00100', '00100', '00100', '00100', '00100'],
    A: ['01110', '10001', '10001', '11111', '10001', '10001', '10001'],
    I: ['11111', '00100', '00100', '00100', '00100', '00100', '11111'],
  };
  const screen = (word, ink, paper, borderStripes) => {
    const scr = new Uint8Array(6912);
    const plot = (x, y) => {
      if (x < 0 || x >= 256 || y < 0 || y >= 192) return;
      const addr = ((y & 0xc0) << 5) | ((y & 7) << 8) | ((y & 0x38) << 2) | (x >> 3);
      scr[addr] |= 0x80 >> (x & 7);
    };
    // Letters at 4x scale: each letter is 20 px wide plus a 4 px gap.
    const scale = 4;
    const width = word.length * 6 * scale - scale;
    const x0 = Math.floor((256 - width) / 2);
    const y0 = Math.floor((192 - 7 * scale) / 2);
    [...word].forEach((ch, i) => {
      FONT[ch].forEach((row, ry) => {
        [...row].forEach((bit, rx) => {
          if (bit !== '1') return;
          for (let dy = 0; dy < scale; dy++) for (let dx = 0; dx < scale; dx++) plot(x0 + (i * 6 + rx) * scale + dx, y0 + ry * scale + dy);
        });
      });
    });
    // A dotted frame so the screen is not empty around the word.
    for (let x = 0; x < 256; x += 2) { plot(x, 8); plot(x + 1, 183); }
    for (let y = 8; y < 184; y += 2) { plot(8, y); plot(247, y + 1); }
    // Attributes: horizontal colour bands, bright text rows.
    for (let row = 0; row < 24; row++) {
      for (let col = 0; col < 32; col++) {
        const band = borderStripes[row % borderStripes.length];
        const inText = row >= (y0 >> 3) && row < ((y0 + 7 * scale + 7) >> 3);
        scr[6144 + row * 32 + col] = inText ? 0x40 | (paper << 3) | ink : (band << 3) | (band === 0 ? 7 : 0);
      }
    }
    return scr;
  };

  // ---- a little machine code for the disassembler ---------------------------

  // ORG 32768: set the border, fill the attributes, then return to BASIC.
  const code = Uint8Array.from([
    0x3e, 0x02,             // ld a,2
    0xd3, 0xfe,             // out (254),a
    0x21, 0x00, 0x58,       // ld hl,22528
    0x11, 0x01, 0x58,       // ld de,22529
    0x01, 0xff, 0x02,       // ld bc,767
    0x36, 0x47,             // ld (hl),0x47
    0xed, 0xb0,             // ldir
    0x06, 0x32,             // ld b,50
    0x76,                   // halt
    0x10, 0xfd,             // djnz -3
    0xc9,                   // ret
  ]);

  // ---- the demo tape ----------------------------------------------------------

  const demoScreen = screen('TAPETI', 7, 1, [1, 1, 5, 5, 4, 4, 6, 6, 2, 2, 3, 3]);
  const romBits = (len) => len * 8;
  const generalized = (body) => {
    // Standard ROM data timings expressed as a generalized data block: pilot tone,
    // sync pulses, then one two-pulse symbol per bit.
    const data = romData(0xff, body);
    return block(0x19, {
      pause: 1000,
      totp: 2, npp: 2,
      pilotSymbols: [{ flags: 0, pulses: [2168, 0] }, { flags: 0, pulses: [667, 735] }],
      pilotStream: [{ symbol: 0, reps: 3223 }, { symbol: 1, reps: 1 }],
      totd: romBits(data.length), npd: 2,
      dataSymbols: [{ flags: 0, pulses: [855, 855] }, { flags: 0, pulses: [1710, 1710] }],
      data,
    });
  };

  const demo = [
    block(0x32, {
      entries: [
        { type: 0x00, text: 'Tapeti demo' },
        { type: 0x01, text: 'Tapeti project' },
        { type: 0x02, text: 'scripts/make-samples.mjs' },
        { type: 0x03, text: '2026' },
        { type: 0x05, text: 'Demonstration' },
        { type: 0x07, text: 'ROM loader, turbo, pure data, generalized data' },
        { type: 0xff, text: 'Synthetic tape: every block type the editor can create. No copyrighted content.' },
      ],
    }),
    headerBlock({ type: 0, typeName: '', name: 'demo', length: demoBasic.length, param1: 20, param2: demoBasic.length }),
    block(0x10, { pause: 1000, data: romData(0xff, demoBasic) }),
    headerBlock({ type: 3, typeName: '', name: 'demo.scr', length: 6912, param1: 16384, param2: 32768 }),
    block(0x11, { pause: 1500, pilotLen: 3223, data: romData(0xff, demoScreen) }),
    block(0x21, { name: 'Machine code' }),
    headerBlock({ type: 3, typeName: '', name: 'demo.bin', length: code.length, param1: 32768, param2: 32768 }),
    block(0x14, { pause: 1000, data: romData(0xff, code) }),
    block(0x22),
    block(0x2a),
    block(0x24, { count: 3 }),
    block(0x12, { pulseLen: 2168, count: 1000 }),
    block(0x13, { pulses: [667, 735, 667, 735] }),
    block(0x25),
    generalized(latin1('Generalized data block, ROM timings')),
    block(0x35, {
      ident: 'POKEs           ',
      data: encodePokes({
        description: 'Cheats for the demo',
        trainers: [
          { description: 'Blue border', pokes: [{ page: null, addr: 32769, value: 1, original: 2 }] },
          { description: 'Skip the pause', pokes: [{ page: null, addr: 32785, value: 1, original: 50 }] },
        ],
      }),
    }),
    block(0x33, { entries: [{ type: 0, id: 1, info: 0 }, { type: 0, id: 3, info: 0 }, { type: 4, id: 0, info: 0 }] }),
    block(0x30, { text: 'End of the Tapeti demo tape.' }),
    block(0x20, { pause: 0 }),
  ];

  // ---- the variant: same program, different loader, for the compare modes -------------

  const variantScreen = screen('TAPETI', 6, 0, [0, 0, 2, 2, 0, 0, 3, 3]);
  const direct = () => {
    // A 1 kHz-ish square wave sampled at 79 T-states: 44 samples per half period.
    const samples = 44 * 2 * 40;
    const d = new Uint8Array(Math.ceil(samples / 8));
    for (let i = 0; i < samples; i++) if (Math.floor(i / 44) % 2 === 0) d[i >> 3] |= 0x80 >> (i & 7);
    return block(0x15, { tstates: 79, pause: 0, usedBits: 8, data: d });
  };
  const csw = () => {
    // RLE CSW: one byte per pulse length in samples; 0 introduces a 4-byte length.
    const pulses = [];
    for (let i = 0; i < 100; i++) pulses.push(22, 22);
    pulses.push(0, 0x10, 0x27, 0, 0); // one long 10000-sample pulse
    return block(0x18, { pause: 500, sampleRate: 44100, compression: 1, pulseCount: 201, data: Uint8Array.from(pulses) });
  };

  const variant = [
    block(0x32, {
      entries: [
        { type: 0x00, text: 'Tapeti demo (variant)' },
        { type: 0x01, text: 'Tapeti project' },
        { type: 0x03, text: '2026' },
        { type: 0xff, text: 'Same program as the demo tape with a headerless screen, a select block, a direct recording and a CSW block.' },
      ],
    }),
    headerBlock({ type: 0, typeName: '', name: 'demo', length: variantBasic.length, param1: 20, param2: variantBasic.length }),
    block(0x10, { pause: 1000, data: romData(0xff, variantBasic) }),
    block(0x31, { time: 3, text: 'Select the screen colours' }),
    block(0x28, { entries: [{ offset: 2, text: 'Yellow on black' }, { offset: 3, text: 'Skip the screen' }] }),
    block(0x2b, { level: 0 }),
    block(0x10, { pause: 1000, data: romData(0xff, variantScreen) }),
    direct(),
    csw(),
    block(0x20, { pause: 0 }),
  ];

  // ---- write and verify -----------------------------------------------------

  fs.mkdirSync(OUT, { recursive: true });
  let failed = false;
  const write = (name, bytes) => {
    fs.writeFileSync(path.join(OUT, name), bytes);
    console.log(`${name}: ${bytes.length} bytes`);
  };
  const verify = (name, blocks) => {
    const bytes = serializeTzx(blocks);
    const parsed = parseTzx(bytes);
    const issues = [...parsed.warnings.map((w) => `parse: ${w}`), ...checkConsistency(parsed.blocks).filter((i) => i.severity !== 'info').map((i) => `block ${i.block + 1}: ${i.message}`)];
    for (const i of issues) console.log(`  ${name}: ${i}`);
    if (issues.length) failed = true;
    return bytes;
  };
  write('Tapeti demo.tzx', verify('demo', demo));
  write('Tapeti demo (variant).tzx', verify('variant', variant));
  write('Tapeti demo.tap', serializeTap(demo).bytes);
  if (failed) {
    console.error('The generated tapes have consistency problems.');
    process.exitCode = 1;
  }
} finally {
  await vite.close();
}
