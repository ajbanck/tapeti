import { describe, it, expect, beforeEach } from 'vitest';
import { disassemble, disassemblyText, checkSymbols } from '../src/spectrum/z80dis';
import { decryptBytes, encryptBytes, CRYPT_PRESETS } from '../src/tzx/bits';
import { decodeHeader } from '../src/tzx/describe';
import { detectContent, detectContentAsLoaded } from '../src/tzx/content';
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

  it('comes on by itself for a block in a SpeedLock group', () => {
    const screen = new Uint8Array(6912).fill(0x38);
    const { xor, add } = CRYPT_PRESETS[0];
    const blocks: Block[] = [0x21, 0x14, 0x22].map((id) => createBlock(id));
    (blocks[0] as GroupStartBlock).name = 'SpeedLock 3 block 1';
    (blocks[1] as PureDataBlock).data = encryptBytes(screen, xor, add);
    const loaded = detectContentAsLoaded(blocks, 1);
    expect(loaded.crypt).toEqual({ xor, add });
    // The guess is made on the decrypted bytes: encrypted, this is not a screen
    // the app would open on, and the label would say so.
    expect(loaded.content.kind).toBe('screen');
    expect(loaded.content.base).toBe(16384);
    expect(detectContent(blocks, 1).label).toBe('SCREEN?');

    // Outside the group the same bytes are read as they lie on the tape.
    const loose = [blocks[1]];
    expect(detectContentAsLoaded(loose, 0).crypt).toBe(null);
    expect(detectContentAsLoaded(loose, 0).content).toEqual(detectContent(loose, 0));
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

// ---- the smaller things: stepping, loops, inverting, parity, colours ----------

import { insertBlocks, setCursor, toggleCollapse, undo, status, backup } from '../src/state/store';
import { runCommand } from '../src/state/commands';
import { createBlock, Block, PureDataBlock, GroupStartBlock, LoopStartBlock } from '../src/tzx/types';
import { listBasic, basicToText } from '../src/spectrum/basic';

describe('stepping through the play order', () => {
  it('goes twice round a loop, opens a collapsed group, and says when the tape is over', () => {
    const blocks: Block[] = [0x20, 0x24, 0x20, 0x25, 0x21, 0x20, 0x22].map((id) => createBlock(id));
    (blocks[1] as LoopStartBlock).count = 2;
    insertBlocks(0, 0, blocks);
    toggleCollapse(0, tapes[0].value.blocks[4].uid);
    setCursor(0, 0);
    const walked: number[] = [];
    for (let i = 0; i < 9; i++) {
      expect(runCommand('step-next', 0)).toBe(true);
      walked.push(tapes[0].value.cursor);
    }
    expect(walked).toEqual([1, 2, 3, 2, 3, 4, 5, 6, 6]);
    expect(tapes[0].value.collapsed.size).toBe(0);
    expect(status.value).toBe('The tape ends here');
    // From a block the cursor was put on by hand, it is that block's first turn.
    setCursor(0, 2);
    runCommand('step-next', 0);
    runCommand('step-next', 0);
    expect(tapes[0].value.cursor).toBe(2);
    expect(runCommand('step-reset', 0)).toBe(true);
  });
});

describe('loop and invert selection', () => {
  it('wraps the selection in a loop, as one step to undo', () => {
    insertBlocks(0, 0, [createBlock(0x20), createBlock(0x30), createBlock(0x20)]);
    setCursor(0, 1);
    runCommand('invert-selection', 0);
    expect(tapes[0].value.selected.size).toBe(2);
    runCommand('invert-selection', 0);
    expect(tapes[0].value.selected.size).toBe(1);
    const wrapped = tapes[0].value.blocks[1].uid;
    runCommand('loop', 0);
    const t = tapes[0].value;
    expect(t.blocks.map((b) => b.id)).toEqual([0x20, 0x24, 0x30, 0x25, 0x20]);
    expect((t.blocks[1] as LoopStartBlock).count).toBe(2);
    expect(t.blocks[2].uid).toBe(wrapped);
    undo(0);
    expect(tapes[0].value.blocks.length).toBe(3);
  });

  it('inverts over the rows, so a collapsed group goes as one', () => {
    // A pause, a group of two, a pause: with the group collapsed the list shows
    // three rows, and inverting a selected first row must select the other two.
    const blocks: Block[] = [0x20, 0x21, 0x30, 0x22, 0x20].map((id) => createBlock(id));
    insertBlocks(0, 0, blocks);
    const uids = tapes[0].value.blocks.map((b) => b.uid);
    toggleCollapse(0, uids[1]);

    // A collapsed group is one row: inverting the selected row leaves the two
    // pauses selected, not the group's contents.
    setCursor(0, 1);
    runCommand('invert-selection', 0);
    expect([...tapes[0].value.selected].sort()).toEqual([uids[0], uids[4]].sort());

    // Inverting again selects the group whole, matching `unitIndices`.
    runCommand('invert-selection', 0);
    expect([...tapes[0].value.selected].sort()).toEqual(uids.slice(1, 4).sort());

    // Expanded, every block is a row of its own again.
    toggleCollapse(0, uids[1]);
    runCommand('invert-selection', 0);
    expect([...tapes[0].value.selected].sort()).toEqual([uids[0], uids[4]].sort());
  });

  it('has the backup switch the desktop app acts on', () => {
    const before = backup.value;
    runCommand('opt-backup', 0);
    expect(backup.value).toBe(!before);
    runCommand('opt-backup', 0);
  });
});

describe('what the consistency check and the lister gained', () => {
  it('holds a SpeedLock group to its parity', () => {
    const pure = (data: number[]) => ({ ...(createBlock(0x14) as PureDataBlock), data: new Uint8Array(data) });
    const tape = (last: number): Block[] => [
      { ...(createBlock(0x21) as GroupStartBlock), name: 'SpeedLock 3 data' },
      pure([0xff, 0x12, 0x34]), pure([0x56, last]), createBlock(0x22),
    ];
    expect(checkConsistency(tape(0xff ^ 0x12 ^ 0x34 ^ 0x56))).toEqual([]);
    const issues = checkConsistency(tape(0));
    expect(issues).toHaveLength(1);
    expect(issues[0]).toMatchObject({ block: 0, severity: 'warning' });
    expect(issues[0].message).toContain('SpeedLock group parity');
  });

  it('can list a program without its colour codes', () => {
    const line = new Uint8Array([0, 10, 9, 0, 0xf5, 0x22, 0x10, 2, 0x68, 0x69, 0x22, 0x0d]);
    const opts = { showNumbers: false, basic128: false, speccyFormat: false };
    expect(basicToText(listBasic(line, 0, line.length, opts), opts)).toBe('  10 PRINT "[INK 2]hi"');
    const plain = { ...opts, dropColours: true };
    expect(basicToText(listBasic(line, 0, line.length, plain), plain)).toBe('  10 PRINT "hi"');
  });
});
