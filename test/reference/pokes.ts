// The TypeScript pokes.ts as it stood before the Rust core replaced it, kept as
// the reference implementation the core is tested against (test/core.test.ts).
// Not part of the app bundle, and not to gain features. See test/reference/parser.ts.
// POKEs text syntax <-> the standardized 'POKEs' custom info block.
//   Each POKE on its own line:  [POKE] [page:]adr,val[/orgval]
//   'val' may be '?' meaning "ask the user".
//   Lines starting with ';' are trainer descriptions; the first ';' lines before any
//   trainer form the general description.

import { bytesToLatin1, latin1ToBytes, Writer, Reader } from '../../src/tzx/bytes';
import { Poke, PokesInfo, Trainer } from '../../src/tzx/types';


export function decodePokes(data: Uint8Array): PokesInfo {
  const r = new Reader(data);
  const dl = r.u8();
  const description = r.str(dl).replace(/\r/g, '\n');
  const n = r.u8();
  const trainers: Trainer[] = [];
  for (let i = 0; i < n; i++) {
    const tl = r.u8();
    const desc = r.str(tl).replace(/\r/g, '\n');
    const np = r.u8();
    const pokes: Poke[] = [];
    for (let k = 0; k < np; k++) {
      const type = r.u8();
      const addr = r.u16();
      const value = r.u8();
      const original = r.u8();
      pokes.push({
        page: type & 8 ? null : type & 7,
        addr,
        value: type & 0x10 ? null : value,
        original: type & 0x20 ? null : original,
      });
    }
    trainers.push({ description: desc, pokes });
  }
  return { description, trainers };
}

export function encodePokes(info: PokesInfo): Uint8Array {
  const w = new Writer();
  const d = info.description.replace(/\n/g, '\r').slice(0, 255);
  w.u8(d.length);
  w.str(d);
  w.u8(info.trainers.length);
  for (const t of info.trainers) {
    const td = t.description.replace(/\n/g, '\r').slice(0, 255);
    w.u8(td.length);
    w.str(td);
    w.u8(t.pokes.length);
    for (const p of t.pokes) {
      let type = 0;
      if (p.page === null) type |= 8;
      else type |= p.page & 7;
      if (p.value === null) type |= 0x10;
      if (p.original === null) type |= 0x20;
      w.u8(type);
      w.u16(p.addr);
      w.u8(p.value ?? 0);
      w.u8(p.original ?? 0);
    }
  }
  return w.toUint8Array();
}

export function pokesToText(info: PokesInfo, hex: boolean): string {
  const f = (n: number) => (hex ? n.toString(16).toUpperCase() : n.toString());
  const lines: string[] = [];
  for (const l of info.description.split('\n')) lines.push('; ' + l);
  for (const t of info.trainers) {
    lines.push('');
    lines.push('[' + t.description.replace(/\n/g, ' | ') + ']');
    for (const p of t.pokes) {
      let s = 'POKE ';
      if (p.page !== null) s += f(p.page) + ':';
      s += f(p.addr) + ',' + (p.value === null ? '?' : f(p.value));
      if (p.original !== null) s += '/' + f(p.original);
      lines.push(s);
    }
  }
  return lines.join('\n');
}

export function textToPokes(text: string, hex: boolean): PokesInfo {
  const parse = (s: string) => {
    s = s.trim();
    if (/^(\$|0x)/i.test(s)) return parseInt(s.replace(/^(\$|0x)/i, ''), 16);
    if (/^#/.test(s)) return parseInt(s.slice(1), 10);
    const v = parseInt(s, hex ? 16 : 10);
    if (Number.isNaN(v)) throw new Error(`Bad number "${s}"`);
    return v;
  };
  const info: PokesInfo = { description: '', trainers: [] };
  const desc: string[] = [];
  let cur: Trainer | null = null;
  text.split(/\r?\n/).forEach((raw, ln) => {
    const line = raw.trim();
    if (!line) return;
    if (line.startsWith(';')) {
      if (cur) cur.description += (cur.description ? '\n' : '') + line.slice(1).trim();
      else desc.push(line.slice(1).trim());
      return;
    }
    if (line.startsWith('[')) {
      cur = { description: line.replace(/^\[|\]$/g, '').replace(/ \| /g, '\n'), pokes: [] };
      info.trainers.push(cur);
      return;
    }
    const m = /^(?:POKE\s+)?(?:([0-9a-fx$#]+):)?([0-9a-fx$#]+)\s*,\s*(\?|[0-9a-fx$#]+)(?:\s*\/\s*([0-9a-fx$#]+))?$/i.exec(line);
    if (!m) throw new Error(`Line ${ln + 1}: cannot parse "${raw}"`);
    if (!cur) {
      cur = { description: '', pokes: [] };
      info.trainers.push(cur);
    }
    cur.pokes.push({
      page: m[1] ? parse(m[1]) : null,
      addr: parse(m[2]),
      value: m[3] === '?' ? null : parse(m[3]),
      original: m[4] ? parse(m[4]) : null,
    });
  });
  info.description = desc.join('\n');
  return info;
}

export const _latin = { bytesToLatin1, latin1ToBytes };
