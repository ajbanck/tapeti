// The TypeScript spectrum/basic.ts as it stood before the Rust core replaced
// it, kept as the reference implementation the core is tested against
// (test/core.test.ts). Not part of the app bundle, and not to gain features.
import { BasicLine, BasicOptions, BasicToken, VariableEntry } from '../../../src/tzx/types';
import { tokenName, zxChar } from './charset';



/** Decode a 5-byte Sinclair floating point number. */
export function decodeNumber(d: Uint8Array, o: number): number {
  if (o + 5 > d.length) return NaN;
  const b0 = d[o], b1 = d[o + 1], b2 = d[o + 2], b3 = d[o + 3], b4 = d[o + 4];
  if (b0 === 0) {
    // "small integer" form; the real interpreter ignores byte 4
    const v = b2 | (b3 << 8);
    return b1 === 0xff ? v - 65536 : v;
  }
  const exp = b0 - 128;
  const sign = b1 & 0x80 ? -1 : 1;
  const mant = ((b1 | 0x80) * 0x1000000 + (b2 << 16) + (b3 << 8) + b4) / 0x100000000;
  return sign * mant * Math.pow(2, exp);
}

export function formatNumber(v: number): string {
  if (Number.isNaN(v)) return '?';
  if (Number.isInteger(v) && Math.abs(v) < 1e15) return v.toString();
  let s = v.toPrecision(8);
  if (s.includes('e')) return s;
  s = s.replace(/\.?0+$/, '');
  return s;
}

/** List a BASIC program area. `data` is the raw bytes starting at PROG. */
export function listBasic(data: Uint8Array, start: number, end: number, opts: BasicOptions): BasicLine[] {
  const lines: BasicLine[] = [];
  let p = start;
  while (p + 4 <= end) {
    const number = (data[p] << 8) | data[p + 1];
    if (number >= 0x4000) break; // reached the variables area or garbage
    const length = data[p + 2] | (data[p + 3] << 8);
    const lineStart = p + 4;
    const lineEnd = Math.min(end, lineStart + length);
    const line: BasicLine = { number, length, offset: p, tokens: [] };
    let q = lineStart;
    let text = '';
    const flush = () => {
      if (text) line.tokens.push({ text, kind: 'text' });
      text = '';
    };
    let lastWasSpace = true;
    while (q < lineEnd) {
      const c = data[q++];
      if (c === 0x0d) break;
      if (c === 0x0e) {
        // hidden 5-byte number follows
        const v = decodeNumber(data, q);
        // Compare with the textual form to spot protected/altered numbers.
        const m = /(-?[0-9]*\.?[0-9]+(?:[eE][-+]?[0-9]+)?)$/.exec(text);
        const textual = m ? Number(m[1]) : NaN;
        flush();
        if (opts.showNumbers || (m && Math.abs(textual - v) > 1e-6 * Math.max(1, Math.abs(v)))) {
          line.tokens.push({ text: `{${formatNumber(v)}}`, kind: 'number' });
        }
        q += 5;
        continue;
      }
      const tok = tokenName(c, opts.basic128);
      if (tok) {
        flush();
        // Sinclair ROM prints a leading space unless preceded by a space, and a trailing space
        let s = tok;
        if (!lastWasSpace) s = ' ' + s;
        line.tokens.push({ text: s + ' ', kind: 'token' });
        lastWasSpace = true;
        continue;
      }
      if (c < 0x20) {
        flush();
        if (c >= 0x10 && c <= 0x15 && q < lineEnd) {
          const names = ['INK', 'PAPER', 'FLASH', 'BRIGHT', 'INVERSE', 'OVER'];
          line.tokens.push({ text: opts.speccyFormat ? '' : `[${names[c - 0x10]} ${data[q]}]`, kind: 'ctrl' });
          q += 1;
        } else if ((c === 0x16 || c === 0x17) && q + 1 < lineEnd) {
          line.tokens.push({ text: opts.speccyFormat ? '' : `[${c === 0x16 ? 'AT' : 'TAB'} ${data[q]},${data[q + 1]}]`, kind: 'ctrl' });
          q += 2;
        } else if (c === 0x06) {
          line.tokens.push({ text: opts.speccyFormat ? '\t' : '[,]', kind: 'ctrl' });
        } else if (c === 0x08) {
          // cursor back: approximate by deleting the last character
          line.tokens.push({ text: opts.speccyFormat ? '\b' : '[<]', kind: 'ctrl' });
        } else {
          line.tokens.push({ text: `[${c.toString(16).padStart(2, '0')}]`, kind: 'ctrl' });
        }
        lastWasSpace = false;
        continue;
      }
      const ch = zxChar(c, false);
      text += ch;
      lastWasSpace = ch === ' ';
    }
    flush();
    lines.push(line);
    p = lineStart + length;
    if (length === 0) {
      line.error = 'zero length line';
      p += 1;
    }
  }
  return lines;
}

/** Render a listing as plain text. */
export function basicToText(lines: BasicLine[], opts: BasicOptions): string {
  return lines
    .map((l) => {
      let s = l.number.toString().padStart(4, ' ') + ' ';
      for (const t of l.tokens) {
        if (t.text === '\b') s = s.slice(0, -1);
        else if (t.text === '\t') s += ' '.repeat(16 - (s.length % 16));
        else s += t.text;
      }
      if (opts.speccyFormat) {
        // wrap at 32 characters like the ROM's LIST
        const out: string[] = [];
        for (let i = 0; i < s.length; i += 32) out.push(s.slice(i, i + 32));
        return out.join('\n');
      }
      return s.replace(/\s+$/, '');
    })
    .join('\n');
}


/** List the variables area (starting at VARS) until the 0x80 end marker. */
export function listVariables(data: Uint8Array, start: number, end: number): VariableEntry[] {
  const out: VariableEntry[] = [];
  let p = start;
  let guard = 0;
  while (p < end && guard++ < 10000) {
    const h = data[p];
    if (h === 0x80) break;
    const type = h >> 5;
    const letter = String.fromCharCode((h & 0x1f) + 0x60);
    const at = p;
    try {
      switch (type) {
        case 0b011: {
          const len = data[p + 1] | (data[p + 2] << 8);
          let s = '';
          for (let i = 0; i < len && p + 3 + i < end; i++) s += zxChar(data[p + 3 + i], false);
          out.push({ name: letter + '$', type: 'string', value: JSON.stringify(s), offset: at, size: 3 + len });
          p += 3 + len;
          break;
        }
        case 0b010:
        case 0b110: {
          const len = data[p + 1] | (data[p + 2] << 8);
          const dims = data[p + 3];
          if (dims === 0) throw new Error('zero dimensions');
          const dimList: number[] = [];
          for (let i = 0; i < dims; i++) dimList.push(data[p + 4 + i * 2] | (data[p + 5 + i * 2] << 8));
          const isChar = type === 0b110;
          const elemStart = p + 4 + dims * 2;
          const total = dimList.reduce((a, b) => a * b, 1);
          let preview = '';
          if (isChar) {
            for (let i = 0; i < Math.min(total, 64); i++) preview += zxChar(data[elemStart + i], false);
            if (total > 64) preview += '…';
            preview = JSON.stringify(preview);
          } else {
            const vals: string[] = [];
            for (let i = 0; i < Math.min(total, 16); i++) vals.push(formatNumber(decodeNumber(data, elemStart + i * 5)));
            preview = vals.join(', ') + (total > 16 ? ', …' : '');
          }
          out.push({
            name: letter + (isChar ? '$' : '') + '(' + dimList.join(',') + ')',
            type: isChar ? 'char array' : 'number array', value: preview, offset: at, size: 3 + len,
          });
          p += 3 + len;
          break;
        }
        case 0b100: {
          out.push({ name: letter, type: 'number', value: formatNumber(decodeNumber(data, p + 1)), offset: at, size: 6 });
          p += 6;
          break;
        }
        case 0b101: {
          let name = letter;
          let q = p + 1;
          while (q < end) {
            const c = data[q++];
            name += String.fromCharCode((c & 0x7f) === 0 ? 0x3f : c & 0x7f);
            if (c & 0x80) break;
          }
          out.push({ name, type: 'number', value: formatNumber(decodeNumber(data, q)), offset: at, size: q + 5 - p });
          p = q + 5;
          break;
        }
        case 0b111: {
          const v = decodeNumber(data, p + 1);
          const lim = decodeNumber(data, p + 6);
          const step = decodeNumber(data, p + 11);
          const line = data[p + 16] | (data[p + 17] << 8);
          const stmt = data[p + 18];
          out.push({
            name: letter, type: 'FOR control',
            value: `value ${formatNumber(v)}, limit ${formatNumber(lim)}, step ${formatNumber(step)}, loop line ${line}:${stmt}`,
            offset: at, size: 19,
          });
          p += 19;
          break;
        }
        default:
          out.push({ name: '?', type: 'garbage', value: `byte ${h.toString(16)} at ${at}`, offset: at, size: 1 });
          return out;
      }
    } catch (e) {
      out.push({ name: '?', type: 'error', value: (e as Error).message, offset: at, size: 0 });
      return out;
    }
  }
  return out;
}
