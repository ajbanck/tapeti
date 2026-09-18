// The TypeScript spectrum/z80dis.ts as it stood before the Rust core replaced
// it, kept as the reference implementation the core is tested against
// (test/core.test.ts). Not part of the app bundle, and not to gain features.
import { DisLine, DisOptions } from '../../../src/tzx/types';
// Table-free Z80 disassembler following the x/y/z/p/q decoding scheme.
// Handles CB, ED, DD, FD, DDCB and FDCB prefixes including undocumented forms.


const R = ['B', 'C', 'D', 'E', 'H', 'L', '(HL)', 'A'];
const RP = ['BC', 'DE', 'HL', 'SP'];
const RP2 = ['BC', 'DE', 'HL', 'AF'];
const CC = ['NZ', 'Z', 'NC', 'C', 'PO', 'PE', 'P', 'M'];
const ALU = ['ADD A,', 'ADC A,', 'SUB ', 'SBC A,', 'AND ', 'XOR ', 'OR ', 'CP '];
const ROT = ['RLC', 'RRC', 'RL', 'RR', 'SLA', 'SRA', 'SLL', 'SRL'];
const IM = ['0', '0/1', '1', '2', '0', '0/1', '1', '2'];
const BLI: string[][] = [
  ['LDI', 'CPI', 'INI', 'OUTI'],
  ['LDD', 'CPD', 'IND', 'OUTD'],
  ['LDIR', 'CPIR', 'INIR', 'OTIR'],
  ['LDDR', 'CPDR', 'INDR', 'OTDR'],
];

export const ROM_LABELS: Record<number, string> = {
  0x0000: 'START', 0x0008: 'ERROR-1', 0x0010: 'PRINT-A', 0x0018: 'GET-CHAR', 0x0020: 'NEXT-CHAR', 0x0028: 'FP-CALC',
  0x0030: 'BC-SPACES', 0x0038: 'MASK-INT', 0x0053: 'ERROR-2', 0x0066: 'RESET', 0x0074: 'CH-ADD+1', 0x007d: 'SKIP-OVER',
  0x0095: 'TOKENS', 0x028e: 'KEY-SCAN', 0x02bf: 'KEYBOARD', 0x031e: 'K-TEST', 0x0333: 'K-DECODE', 0x03b5: 'BEEPER',
  0x03f8: 'BEEP', 0x04c2: 'SA-BYTES', 0x04d0: 'SA-FLAG', 0x053f: 'SA/LD-RET', 0x0556: 'LD-BYTES', 0x056b: 'LD-START',
  0x05e3: 'LD-EDGE-2', 0x05e7: 'LD-EDGE-1', 0x0605: 'SAVE-ETC', 0x0802: 'LD-BLOCK', 0x0808: 'LD-CONTRL',
  0x08b6: 'ME-CONTRL', 0x0970: 'SA-CONTRL', 0x09a1: 'SA-ALL', 0x0a11: 'PO-TAB', 0x0adc: 'PO-STORE', 0x0b03: 'PO-FETCH',
  0x0b24: 'PO-ANY', 0x0b7f: 'PR-ALL', 0x0bdb: 'PO-ATTR', 0x0c0a: 'PO-MSG', 0x0c3b: 'PO-SAVE', 0x0c55: 'PO-SCR',
  0x0d4d: 'TEMPS', 0x0d6b: 'CLS', 0x0daf: 'CL-ALL', 0x0dd9: 'CL-SET', 0x0dfe: 'CL-SC-ALL', 0x0e44: 'CL-LINE',
  0x0e9b: 'CL-ADDR', 0x0eac: 'COPY', 0x0edf: 'CLEAR-PRB', 0x0f2c: 'EDITOR', 0x0f81: 'ADD-CHAR', 0x111d: 'ED-COPY',
  0x1219: 'RAM-CHECK', 0x11cb: 'START/NEW', 0x12a2: 'MAIN-EXEC', 0x12cf: 'MAIN-1', 0x1303: 'MAIN-4', 0x1391: 'REPORT-G',
  0x155d: 'MAIN-ADD', 0x15c4: 'REPORT-J', 0x15d4: 'WAIT-KEY', 0x15e6: 'INPUT-AD', 0x15ef: 'OUT-CODE', 0x15f2: 'PRINT-A-2',
  0x1601: 'CHAN-OPEN', 0x1615: 'CHAN-FLAG', 0x1655: 'MAKE-ROOM', 0x1664: 'POINTERS', 0x16b0: 'SET-MIN', 0x16e5: 'CLOSE',
  0x1736: 'OPEN', 0x1795: 'AUTO-LIST', 0x17f9: 'LLIST', 0x17f5: 'LIST', 0x1855: 'OUT-LINE', 0x196e: 'LINE-ADDR',
  0x1980: 'CP-LINES', 0x19b8: 'NEXT-ONE', 0x19e8: 'RECLAIM-2', 0x1a1b: 'OUT-NUM-1', 0x1b17: 'LINE-SCAN', 0x1b8a: 'LINE-RUN',
  0x1bb2: 'REM', 0x1c79: 'CLASS-06', 0x1cdb: 'IF', 0x1d03: 'FOR', 0x1e42: 'RESTORE', 0x1e4f: 'RANDOMIZE', 0x1e5f: 'CONTINUE',
  0x1e67: 'GO-TO', 0x1e7a: 'OUT', 0x1e80: 'POKE', 0x1e94: 'FIND-INT1', 0x1e99: 'FIND-INT2', 0x1ea1: 'RUN', 0x1eac: 'CLEAR',
  0x1eed: 'GO-SUB', 0x1f05: 'TEST-ROOM', 0x1f23: 'RETURN', 0x1f3a: 'PAUSE', 0x1f54: 'BREAK-KEY', 0x1fc3: 'LPRINT',
  0x1fcd: 'PRINT', 0x203c: 'PR-STRING', 0x2070: 'INPUT', 0x2294: 'BORDER', 0x22aa: 'PIXEL-ADD', 0x22cb: 'POINT-SUB',
  0x22dc: 'PLOT', 0x2307: 'STK-TO-BC', 0x2314: 'STK-TO-A', 0x2320: 'CIRCLE', 0x2382: 'DRAW', 0x24b7: 'DRAW-LINE',
  0x24fb: 'SCANNING', 0x2530: 'SYNTAX-Z', 0x2634: 'STK-CONST', 0x2c88: 'ALPHANUM', 0x2d1b: 'NUMERIC', 0x2d28: 'STACK-A',
  0x2d2b: 'STACK-BC', 0x2d3b: 'INT-TO-FP', 0x2da2: 'FP-TO-BC', 0x2dd5: 'FP-TO-A', 0x2de3: 'PRINT-FP', 0x335e: 'CALCULATE',
  0x33c0: 'STK-DATA', 0x3d00: 'CHAR-SET',
};


function h8(v: number, hex: boolean) {
  return hex ? '0x' + v.toString(16).toUpperCase().padStart(2, '0') : v.toString();
}
function h16(v: number, hex: boolean) {
  return hex ? '0x' + v.toString(16).toUpperCase().padStart(4, '0') : v.toString();
}
function disp(d: number, hex: boolean) {
  const s = d < 0 ? '-' : '+';
  return s + h8(Math.abs(d), hex);
}

/** Disassemble `count` instructions from `data` starting at byte `offset`, with address base `base`. */
export function disassemble(data: Uint8Array, offset: number, base: number, count: number, opts: DisOptions = {}): DisLine[] {
  const hex = opts.hex ?? true;
  const out: DisLine[] = [];
  let p = offset;
  while (out.length < count && p < data.length) {
    const start = p;
    const bytes: number[] = [];
    const rd = () => {
      const v = p < data.length ? data[p] : 0;
      bytes.push(v);
      p++;
      return v;
    };
    const rd16 = () => {
      const lo = rd();
      return lo | (rd() << 8);
    };
    const addr = base + (start - offset);
    let target: number | undefined;
    let text = '';

    let op = rd();
    let ix: 'IX' | 'IY' | null = null;
    while (op === 0xdd || op === 0xfd) {
      ix = op === 0xdd ? 'IX' : 'IY';
      if (p >= data.length) break;
      op = rd();
    }
    const HL = ix ?? 'HL';
    const Hh = ix ? ix + 'H' : 'H';
    const Ll = ix ? ix + 'L' : 'L';
    const x = op >> 6, y = (op >> 3) & 7, z = op & 7, p_ = y >> 1, q = y & 1;
    const rname = (i: number, d?: number) => {
      if (i === 6) return ix ? `(${ix}${disp(d ?? 0, hex)})` : '(HL)';
      if (ix && i === 4) return Hh;
      if (ix && i === 5) return Ll;
      return R[i];
    };
    const rp = (i: number) => (i === 2 ? HL : RP[i]);
    const rp2 = (i: number) => (i === 2 ? HL : RP2[i]);
    const rel = () => {
      const d = rd();
      const t = (addr + bytes.length + (d >= 128 ? d - 256 : d)) & 0xffff;
      target = t;
      return h16(t, hex);
    };
    const imm16 = () => {
      const v = rd16();
      return h16(v, hex);
    };
    const abs16 = () => {
      const v = rd16();
      target = v;
      return h16(v, hex);
    };
    const readDisp = () => {
      const d = rd();
      return d >= 128 ? d - 256 : d;
    };

    if (op === 0xcb) {
      let d = 0;
      if (ix) d = readDisp();
      const o = rd();
      const cx = o >> 6, cy = (o >> 3) & 7, cz = o & 7;
      const mem = ix ? `(${ix}${disp(d, hex)})` : R[cz];
      if (ix && cz !== 6) {
        // undocumented: result copied to register
        if (cx === 0) text = `${ROT[cy]} ${mem},${R[cz]}`;
        else if (cx === 1) text = `BIT ${cy},${mem}`;
        else text = `${cx === 2 ? 'RES' : 'SET'} ${cy},${mem},${R[cz]}`;
      } else {
        if (cx === 0) text = `${ROT[cy]} ${mem}`;
        else text = `${['', 'BIT', 'RES', 'SET'][cx]} ${cy},${mem}`;
      }
    } else if (op === 0xed) {
      const o = rd();
      const ex = o >> 6, ey = (o >> 3) & 7, ez = o & 7, ep = ey >> 1, eq = ey & 1;
      if (ex === 1) {
        switch (ez) {
          case 0: text = ey === 6 ? 'IN (C)' : `IN ${R[ey]},(C)`; break;
          case 1: text = ey === 6 ? 'OUT (C),0' : `OUT (C),${R[ey]}`; break;
          case 2: text = `${eq ? 'ADC' : 'SBC'} HL,${RP[ep]}`; break;
          case 3: text = eq ? `LD ${RP[ep]},(${imm16()})` : `LD (${imm16()}),${RP[ep]}`; break;
          case 4: text = 'NEG'; break;
          case 5: text = ey === 1 ? 'RETI' : 'RETN'; break;
          case 6: text = `IM ${IM[ey]}`; break;
          case 7: text = ['LD I,A', 'LD R,A', 'LD A,I', 'LD A,R', 'RRD', 'RLD', 'NOP', 'NOP'][ey]; break;
        }
      } else if (ex === 2 && ez <= 3 && ey >= 4) {
        text = BLI[ey - 4][ez];
      } else {
        text = 'NOP*';
      }
    } else {
      switch (x) {
        case 0:
          switch (z) {
            case 0:
              text = ['NOP', "EX AF,AF'", `DJNZ ${y === 2 ? rel() : ''}`, `JR ${y === 3 ? rel() : ''}`][y] ?? `JR ${CC[y - 4]},${rel()}`;
              break;
            case 1:
              text = q ? `ADD ${HL},${rp(p_)}` : `LD ${rp(p_)},${imm16()}`;
              break;
            case 2:
              if (!q) text = ['LD (BC),A', 'LD (DE),A', `LD (${p_ === 2 ? imm16() : ''}),${HL}`, `LD (${p_ === 3 ? imm16() : ''}),A`][p_];
              else text = ['LD A,(BC)', 'LD A,(DE)', `LD ${HL},(${p_ === 2 ? imm16() : ''})`, `LD A,(${p_ === 3 ? imm16() : ''})`][p_];
              break;
            case 3:
              text = `${q ? 'DEC' : 'INC'} ${rp(p_)}`;
              break;
            case 4:
              text = `INC ${rname(y, y === 6 && ix ? readDisp() : undefined)}`;
              break;
            case 5:
              text = `DEC ${rname(y, y === 6 && ix ? readDisp() : undefined)}`;
              break;
            case 6: {
              const d = y === 6 && ix ? readDisp() : undefined;
              text = `LD ${rname(y, d)},${h8(rd(), hex)}`;
              break;
            }
            case 7:
              text = ['RLCA', 'RRCA', 'RLA', 'RRA', 'DAA', 'CPL', 'SCF', 'CCF'][y];
              break;
          }
          break;
        case 1:
          if (z === 6 && y === 6) text = 'HALT';
          else if (ix && (y === 6 || z === 6)) {
            const d = readDisp();
            // when (IX+d) is involved, H/L stay plain H/L
            const dst = y === 6 ? `(${ix}${disp(d, hex)})` : R[y];
            const src = z === 6 ? `(${ix}${disp(d, hex)})` : R[z];
            text = `LD ${dst},${src}`;
          } else text = `LD ${rname(y)},${rname(z)}`;
          break;
        case 2:
          text = `${ALU[y]}${rname(z, z === 6 && ix ? readDisp() : undefined)}`;
          break;
        case 3:
          switch (z) {
            case 0:
              text = `RET ${CC[y]}`;
              break;
            case 1:
              text = q ? ['RET', 'EXX', `JP (${HL})`, `LD SP,${HL}`][p_] : `POP ${rp2(p_)}`;
              break;
            case 2:
              text = `JP ${CC[y]},${abs16()}`;
              break;
            case 3:
              text = [`JP ${y === 0 ? abs16() : ''}`, '', `OUT (${y === 2 ? h8(rd(), hex) : ''}),A`, `IN A,(${y === 3 ? h8(rd(), hex) : ''})`, `EX (SP),${HL}`, 'EX DE,HL', 'DI', 'EI'][y];
              break;
            case 4:
              text = `CALL ${CC[y]},${abs16()}`;
              break;
            case 5:
              text = q ? (p_ === 0 ? `CALL ${abs16()}` : 'NOP*') : `PUSH ${rp2(p_)}`;
              break;
            case 6:
              text = `${ALU[y]}${h8(rd(), hex)}`;
              break;
            case 7:
              target = y * 8;
              text = `RST ${h8(y * 8, hex)}`;
              break;
          }
          break;
      }
    }
    if (opts.romLabels !== false && target !== undefined && ROM_LABELS[target]) text += `  ; ${ROM_LABELS[target]}`;
    else if (opts.romLabels !== false) {
      // LD HL,nnnn / LD DE,nnnn etc. with a ROM address
      const m = /,\$?([0-9A-F]{4})$/.exec(text);
      if (m) {
        const v = hex ? parseInt(m[1], 16) : parseInt(m[1], 10);
        if (ROM_LABELS[v]) text += `  ; ${ROM_LABELS[v]}`;
      }
    }
    out.push({ addr, bytes, text, target });
  }
  return out;
}
