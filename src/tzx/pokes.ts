// POKEs text syntax <-> the standardized 'POKEs' custom info block. The parsing
// and formatting are the Rust core in core/ (pokes.rs).
//
// test/reference/pokes.ts is the frozen implementation the differential tests
// compare this against.
import { Poke, PokesInfo, Trainer } from './types';
import { decodePokesCore, encodePokesCore, pokesToTextCore, textToPokesCore } from './core';

export type { Poke, PokesInfo, Trainer };

export function decodePokes(data: Uint8Array): PokesInfo {
  return decodePokesCore(data);
}

export function encodePokes(info: PokesInfo): Uint8Array {
  return encodePokesCore(info);
}

export function pokesToText(info: PokesInfo, hex: boolean): string {
  return pokesToTextCore(info, hex);
}

/** Parses the editor's text; a line that makes no sense throws. */
export function textToPokes(text: string, hex: boolean): PokesInfo {
  return textToPokesCore(text, hex);
}
