// The TypeScript content.ts as it stood before the Rust core replaced it, kept as
// the reference implementation the core is tested against (test/core.test.ts).
// Not part of the app bundle, and not to gain features. See test/reference/parser.ts.
// Detects what a data block contains: a ROM header, a BASIC program, a screen, machine code,
// an array or plain data. Uses the preceding header when there is one, otherwise heuristics.
import { Block, isDataBlock, ContentInfo, ContentKind, HeaderInfo } from '../../src/tzx/types';
import { decodeHeader } from './describe';
import { SCREEN_SIZE } from '../../src/spectrum/screen';

const NONE: ContentInfo = { kind: 'data', label: '', base: 0x8000, skipFlag: false, skipChecksum: false, progLen: null, header: null, source: 'none', expectedLength: null };

/** Body of a data block after removing flag/checksum bytes. */
export function blockBody(data: Uint8Array, skipFlag: boolean, skipChecksum: boolean): Uint8Array {
  let d = data;
  if (skipFlag && d.length > 0) d = d.subarray(1);
  if (skipChecksum && d.length > 0) d = d.subarray(0, d.length - 1);
  return d;
}

/** Does the byte stream look like a BASIC program area? Returns the fraction of bytes that parse as lines. */
export function basicScore(d: Uint8Array): number {
  let p = 0;
  let lines = 0;
  let lastNo = -1;
  while (p + 4 <= d.length) {
    const no = (d[p] << 8) | d[p + 1];
    const len = d[p + 2] | (d[p + 3] << 8);
    if (no > 9999 || no <= lastNo || len === 0 || p + 4 + len > d.length) break;
    if (d[p + 4 + len - 1] !== 0x0d) break;
    lastNo = no;
    lines++;
    p += 4 + len;
  }
  if (lines === 0) return 0;
  // A program is usually followed by the variables area; count the parsed part.
  return p / d.length;
}

function looksLikeScreen(len: number): boolean {
  return len === SCREEN_SIZE;
}

/** Analyse block `index`; the previous block is consulted for a ROM header. */
export function detectContent(blocks: Block[], index: number): ContentInfo {
  const b = blocks[index];
  if (!b || !isDataBlock(b)) return NONE;
  const data = b.data;
  if (data.length === 0) return { ...NONE, kind: 'empty', label: '' };

  // ROM header block?
  const own = decodeHeader(data);
  if (own && (b.id === 0x10 || b.id === 0x11 || b.id === 0x19)) {
    return { ...NONE, kind: 'header', label: 'header', header: own, skipFlag: true, skipChecksum: true, source: 'header' };
  }

  // Flag/checksum convention: standard/turbo blocks always have them; other block types only
  // when the first byte is a ROM-style flag.
  const hasFlag = b.id === 0x10 || b.id === 0x11 || data[0] === 0xff || data[0] === 0x00;
  const body = blockBody(data, hasFlag, hasFlag);

  // Preceding header tells us what this is.
  const prev = blocks[index - 1];
  // A length mismatch (truncated or over-long block) still uses the header, marked in the label.
  const hdr = prev && isDataBlock(prev) && (prev.id === 0x10 || prev.id === 0x11 || prev.id === 0x19) ? decodeHeader(prev.data) : null;
  if (hdr && (hdr.length === body.length || !looksLikeScreen(body.length))) {
    const suffix = body.length < hdr.length ? ' (short)' : body.length > hdr.length ? ' (long)' : '';
    const fromHdr = { ...NONE, skipFlag: hasFlag, skipChecksum: hasFlag, source: 'header' as const, expectedLength: hdr.length };
    switch (hdr.type) {
      case 0:
        return { ...fromHdr, kind: 'basic', label: 'BASIC' + suffix, base: 23755, progLen: hdr.param2 <= Math.min(hdr.length, body.length) ? hdr.param2 : null };
      case 3:
        // A 6912-byte CODE block is a screen even when loaded elsewhere (loaders often load it
        // out of sight and copy it to 16384); view it at the screen address either way.
        if (looksLikeScreen(hdr.length)) {
          const label = (hdr.param1 === 16384 ? 'SCREEN' : `SCREEN ${hdr.param1}`) + suffix;
          return { ...fromHdr, kind: 'screen', label, base: 16384 };
        }
        return { ...fromHdr, kind: 'code', label: `CODE ${hdr.param1}` + suffix, base: hdr.param1 };
      case 1:
      case 2:
        return { ...fromHdr, kind: 'array', label: (hdr.type === 1 ? 'NUM ARRAY' : 'CHAR ARRAY') + suffix, base: 0x8000 };
    }
  }

  // Heuristics on the bytes. Loaders differ in whether a checksum follows the screen,
  // so try the plausible flag/checksum combinations.
  for (const [sf, sc] of [[hasFlag, hasFlag], [hasFlag, false], [false, false]] as [boolean, boolean][]) {
    if (looksLikeScreen(blockBody(data, sf, sc).length)) {
      return { ...NONE, kind: 'screen', label: 'SCREEN?', base: 16384, skipFlag: sf, skipChecksum: sc, source: 'heuristic' };
    }
  }
  const score = basicScore(body);
  if (score > 0.5 || (score > 0 && body.length < 64)) {
    return { ...NONE, kind: 'basic', label: 'BASIC?', base: 23755, skipFlag: hasFlag, skipChecksum: hasFlag, source: 'heuristic' };
  }
  return { ...NONE, kind: 'data', label: '', base: 0x8000, skipFlag: hasFlag, skipChecksum: hasFlag, source: 'none' };
}
