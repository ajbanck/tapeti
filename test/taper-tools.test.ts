import { describe, it, expect, beforeEach } from 'vitest';
import { disassemble, disassemblyText, checkSymbols } from '../src/spectrum/z80dis';
import { decryptBytes, encryptBytes, CRYPT_PRESETS } from '../src/tzx/bits';
import { decodeHeader } from '../src/tzx/describe';
import { detectContent } from '../src/tzx/content';
import { checkConsistency } from '../src/tzx/consistency';
import { tapes, dialog } from '../src/state/store';
import { loadBytes, insertDataFile, headerName, newTape, MAX_FILE_BYTES } from '../src/state/files';
import { StandardBlock } from '../src/tzx/types';

// What these do is the core's to prove (core/tests/spectrum.rs, logic.rs); this
// is the trip through the wasm boundary, and the store's side of inserting a file.

beforeEach(() => {
  dialog.value = null;
  newTape(0);
});

describe('disassembler: ROM data, system variables, symbols', () => {
  const code = new Uint8Array([0xcd, 0x10, 0x80, 0x2a, 0x4b, 0x5c, 0xcf, 0x1a, 0xef, 0xa1, 0x38, 0xc9]);
  const full = { hex: true, romLabels: true, sysvars: true, literals: true, symbols: '$8010 DRAW_IT\n$800B DONE' };

  it('reads what is behind the restarts, and names what it can', () => {
    expect(disassemble(code, 0, 0x8000, 100, full).map((l) => l.text)).toEqual([
      'CALL 0x8010  ; DRAW_IT',
      'LD HL,(0x5C4B)  ; VARS',
      'RST 0x08  ; ERROR-1',
      'DEFB 0x1A  ; R Tape loading error',
      'RST 0x28  ; FP-CALC',
      'DEFB 0xA1  ; stk-one',
      'DEFB 0x38  ; end-calc',
      'DONE:',
      'RET',
    ]);
  });

  it('is what it was with the new options off', () => {
    const plain = disassemble(code, 0, 0x8000, 100, { hex: true, romLabels: true }).map((l) => l.text);
    expect(plain.slice(0, 4)).toEqual(['CALL 0x8010', 'LD HL,(0x5C4B)', 'RST 0x08  ; ERROR-1', 'LD A,(DE)']);
  });

  it('saves the same, names at the margin', () => {
    const lines = disassemblyText(code, 0, 0x8000, 100, full).split('\n');
    expect(lines[0]).toBe('8000  CD 10 80     CALL 0x8010  ; DRAW_IT');
    expect(lines[7]).toBe('DONE:');
  });

  it('says which symbol lines it cannot read', () => {
    expect(checkSymbols('$8000 A\nnonsense\n32768 B ; fine\n99999 far')).toEqual([2, 4]);
  });
});

describe('decrypt modifier', () => {
  it('is undone by encrypting', () => {
    const all = Uint8Array.from({ length: 256 }, (_, i) => i);
    for (const p of CRYPT_PRESETS) expect(encryptBytes(decryptBytes(all, p.xor, p.add), p.xor, p.add)).toEqual(all);
    expect(Array.from(decryptBytes(new Uint8Array([0x00, 0xff]), 0x98, 0x0b))).toEqual([0xa3, 0x72]);
  });
});

describe('inserting a file as data', () => {
  it('asks first, then makes the header and the block', () => {
    const screen = new Uint8Array(6912).fill(0x38);
    loadBytes(0, 'Loading Screen.scr', screen, true);
    expect(dialog.value).toMatchObject({ kind: 'datafile', name: 'Loading Screen.scr' });
    expect(tapes[0].value.blocks.length).toBe(0);
    expect(headerName('Loading Screen.scr')).toBe('Loading Sc');

    insertDataFile(0, 'picture', screen, 16384, true);
    const t = tapes[0].value;
    expect(t.blocks.length).toBe(2);
    expect(decodeHeader((t.blocks[0] as StandardBlock).data)).toMatchObject({ type: 3, length: 6912, param1: 16384, name: 'picture   ' });
    expect(detectContent(t.blocks, 1).kind).toBe('screen');
    expect(checkConsistency(t.blocks)).toEqual([]);
    expect(t.dirty).toBe(true);
  });

  it('leaves tapes to be tapes, and opening as it was', () => {
    const tap = new Uint8Array([2, 0, 0xff, 0xff]);
    loadBytes(0, 'x.TAP', tap, true);
    expect(dialog.value).toBeNull();
    loadBytes(0, 'odd.bin', tap, false);
    expect(dialog.value).toBeNull();
    expect(tapes[0].value.blocks.length).toBe(1);
  });

  it('refuses what a block cannot hold', () => {
    insertDataFile(0, 'big', new Uint8Array(MAX_FILE_BYTES + 1), 32768, true);
    expect(dialog.value).toMatchObject({ kind: 'message', title: 'Cannot insert file' });
    expect(tapes[0].value.blocks.length).toBe(0);
  });
});
