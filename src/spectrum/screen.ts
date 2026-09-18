// Rendering a Spectrum screen dump. The renderer is the Rust core in core/
// (spectrum/screen.rs).
//
// The previous implementation lives on as test/reference/spectrum/screen.ts.
import { hasFlashCore, renderScreenCore } from '../tzx/core';

export const SCREEN_SIZE = 6912;

/** Attribute used where the data ends before the attribute area: black ink on white paper, as after NEW. */
export const DEFAULT_ATTR = 0x38;

export interface ScreenOptions {
  hideAttributes?: boolean;
  flashPhase?: boolean; // true = swap ink/paper for FLASH cells
}

/** Render a 6912-byte screen dump into RGBA pixels (256x192). Missing bitmap bytes are treated as 0,
 *  missing attributes as DEFAULT_ATTR so partial screens stay visible. */
export function renderScreen(data: Uint8Array, offset: number, opts: ScreenOptions = {}): Uint8ClampedArray {
  return renderScreenCore(data, offset, opts);
}

export function hasFlash(data: Uint8Array, offset: number): boolean {
  return hasFlashCore(data, offset);
}
