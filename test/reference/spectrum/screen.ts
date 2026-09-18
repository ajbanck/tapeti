// The TypeScript spectrum/screen.ts as it stood before the Rust core replaced
// it, kept as the reference implementation the core is tested against
// (test/core.test.ts). Not part of the app bundle, and not to gain features.
export const SCREEN_SIZE = 6912;

const PALETTE: [number, number, number][] = [
  [0, 0, 0], [0, 0, 0xd7], [0xd7, 0, 0], [0xd7, 0, 0xd7], [0, 0xd7, 0], [0, 0xd7, 0xd7], [0xd7, 0xd7, 0], [0xd7, 0xd7, 0xd7],
  [0, 0, 0], [0, 0, 0xff], [0xff, 0, 0], [0xff, 0, 0xff], [0, 0xff, 0], [0, 0xff, 0xff], [0xff, 0xff, 0], [0xff, 0xff, 0xff],
];

export interface ScreenOptions {
  hideAttributes?: boolean;
  flashPhase?: boolean; // true = swap ink/paper for FLASH cells
}

/** Attribute used where the data ends before the attribute area: black ink on white paper, as after NEW. */
export const DEFAULT_ATTR = 0x38;

/** Render a 6912-byte screen dump into RGBA pixels (256x192). Missing bitmap bytes are treated as 0,
 *  missing attributes as DEFAULT_ATTR so partial screens stay visible. */
export function renderScreen(data: Uint8Array, offset: number, opts: ScreenOptions = {}): Uint8ClampedArray {
  const px = new Uint8ClampedArray(256 * 192 * 4);
  const get = (i: number) => (offset + i >= 0 && offset + i < data.length ? data[offset + i] : i >= 6144 ? DEFAULT_ATTR : 0);
  for (let y = 0; y < 192; y++) {
    const rowAddr = ((y & 0xc0) << 5) | ((y & 7) << 8) | ((y & 0x38) << 2);
    for (let cx = 0; cx < 32; cx++) {
      const bits = get(rowAddr + cx);
      const attr = get(6144 + (y >> 3) * 32 + cx);
      let ink = attr & 7;
      let paper = (attr >> 3) & 7;
      const bright = attr & 0x40 ? 8 : 0;
      if (attr & 0x80 && opts.flashPhase) [ink, paper] = [paper, ink];
      const inkC = opts.hideAttributes ? [0, 0, 0] : PALETTE[ink + bright];
      const papC = opts.hideAttributes ? [0xff, 0xff, 0xff] : PALETTE[paper + bright];
      for (let b = 0; b < 8; b++) {
        const on = (bits >> (7 - b)) & 1;
        const c = on ? inkC : papC;
        const o = (y * 256 + cx * 8 + b) * 4;
        px[o] = c[0];
        px[o + 1] = c[1];
        px[o + 2] = c[2];
        px[o + 3] = 255;
      }
    }
  }
  return px;
}

export function hasFlash(data: Uint8Array, offset: number): boolean {
  for (let i = 6144; i < 6912; i++) {
    const v = data[offset + i];
    if (v !== undefined && v & 0x80) return true;
  }
  return false;
}
