/** Little helper for sequential little-endian reads. */
export class Reader {
  pos = 0;
  constructor(public buf: Uint8Array) {}
  get remaining() {
    return this.buf.length - this.pos;
  }
  eof() {
    return this.pos >= this.buf.length;
  }
  need(n: number) {
    if (this.pos + n > this.buf.length) throw new Error(`Unexpected end of file at offset ${this.pos}`);
  }
  u8() {
    this.need(1);
    return this.buf[this.pos++];
  }
  u16() {
    this.need(2);
    const v = this.buf[this.pos] | (this.buf[this.pos + 1] << 8);
    this.pos += 2;
    return v;
  }
  i16() {
    const v = this.u16();
    return v >= 0x8000 ? v - 0x10000 : v;
  }
  u24() {
    this.need(3);
    const v = this.buf[this.pos] | (this.buf[this.pos + 1] << 8) | (this.buf[this.pos + 2] << 16);
    this.pos += 3;
    return v;
  }
  u32() {
    this.need(4);
    const v =
      (this.buf[this.pos] | (this.buf[this.pos + 1] << 8) | (this.buf[this.pos + 2] << 16)) +
      this.buf[this.pos + 3] * 0x1000000;
    this.pos += 4;
    return v;
  }
  bytes(n: number) {
    this.need(n);
    const v = this.buf.slice(this.pos, this.pos + n);
    this.pos += n;
    return v;
  }
  str(n: number) {
    return bytesToLatin1(this.bytes(n));
  }
}

export class Writer {
  private chunks: number[] = [];
  u8(v: number) {
    this.chunks.push(v & 0xff);
  }
  u16(v: number) {
    this.u8(v);
    this.u8(v >> 8);
  }
  i16(v: number) {
    this.u16(v < 0 ? v + 0x10000 : v);
  }
  u24(v: number) {
    this.u8(v);
    this.u8(v >> 8);
    this.u8(v >> 16);
  }
  u32(v: number) {
    this.u24(v);
    this.u8(Math.floor(v / 0x1000000));
  }
  bytes(b: Uint8Array | number[]) {
    for (let i = 0; i < b.length; i++) this.chunks.push(b[i] & 0xff);
  }
  str(s: string) {
    this.bytes(latin1ToBytes(s));
  }
  get length() {
    return this.chunks.length;
  }
  toUint8Array() {
    return Uint8Array.from(this.chunks);
  }
}

export function bytesToLatin1(b: Uint8Array): string {
  let s = '';
  for (let i = 0; i < b.length; i++) s += String.fromCharCode(b[i]);
  return s;
}

export function latin1ToBytes(s: string): Uint8Array {
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i) & 0xff;
  return out;
}

export function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}
