// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

import { describe, it, expect, beforeEach } from 'vitest';
import {
  snapshotKind,
  snapshotInfo,
  snapshotToTape,
  DEFAULT_SNAPSHOT_SPEED,
  decodeHeader,
  serializeTzx,
  parseTape,
} from '../src/tzx/core';
import { StandardBlock, TextBlock, TurboBlock } from '../src/tzx/types';
import { tapes, dialog } from '../src/state/store';
import { loadBytes, importSnapshot, newTape } from '../src/state/files';

// That the loader works is the core's to prove (core/tests/snapshot.rs runs it);
// this is the trip through the wasm boundary and the store.

/** A 48K .sna of synthetic memory: border 3, a striped screen, the program counter on the stack. */
function sna(): Uint8Array {
  const file = new Uint8Array(27 + 49152);
  file[23] = 0x00; // SP = 8000
  file[24] = 0x80;
  file[26] = 3;
  const ram = file.subarray(27);
  for (let i = 0; i < 6144; i++) ram[i] = i & 1 ? 0xaa : 0x55;
  ram.fill(0x38, 6144, 6912);
  ram[0x4000] = 0x23; // PC = 9123, where SP points
  ram[0x4001] = 0x91;
  return file;
}

const options = { speed: DEFAULT_SNAPSHOT_SPEED, border: 3, compressAll: false, screen: null };

beforeEach(() => {
  dialog.value = null;
  newTape(0);
});

describe('snapshot import', () => {
  it('goes by extension', () => {
    expect(snapshotKind('Game.Z80')).toBe('z80');
    expect(snapshotKind('game.sna')).toBe('sna');
    expect(snapshotKind('game.tzx')).toBeNull();
  });

  it('reads what the dialog shows', () => {
    const info = snapshotInfo(sna(), 'sna');
    expect(info.machine).toBe(0);
    expect(info.border).toBe(3);
    expect(info.screen.length).toBe(6912);
    expect(info.screen[6911]).toBe(0x38);
    expect(() => snapshotInfo(new Uint8Array(100), 'sna')).toThrow(/\.sna/);
    expect(() => snapshotInfo(new Uint8Array(10), 'z80')).toThrow(/\.z80/);
  });

  it('builds a tape that survives being written and read', () => {
    const tape = snapshotToTape(sna(), 'sna', 'Some Game.sna', options);
    // text, header, program, three pages; nothing under the loader to put back
    expect(tape.blocks.map((b) => b.id)).toEqual([0x30, 0x10, 0x10, 0x11, 0x11, 0x11]);
    expect((tape.blocks[0] as TextBlock).text).toBe('48K snapshot - 3000 bps');
    const header = decodeHeader((tape.blocks[1] as StandardBlock).data)!;
    expect(header.name).toBe('Some Game ');
    expect(header.typeName).toBe('Program');
    expect(header.length + 2).toBe((tape.blocks[2] as StandardBlock).data.length);
    const page = tape.blocks[3] as TurboBlock;
    expect([page.pilot, page.zero, page.one, page.data[0]]).toEqual([1900, 389, 778, 0xaa]);
    const again = parseTape(serializeTzx(tape.blocks));
    expect(again.blocks.map((b) => ({ ...b, uid: 0 }))).toEqual(tape.blocks.map((b) => ({ ...b, uid: 0 })));
  });

  it('uses standard blocks at ROM speed and takes a loading screen', () => {
    const screen = new Uint8Array(6912).fill(7);
    const tape = snapshotToTape(sna(), 'sna', 'a.sna', { ...options, speed: 0, screen });
    expect(tape.blocks.map((b) => b.id)).toEqual([0x30, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10]);
    expect(() => snapshotToTape(sna(), 'sna', 'a.sna', { ...options, screen: new Uint8Array(10) })).toThrow(/6912/);
  });

  it('opens the dialog instead of loading, and loads on OK', () => {
    loadBytes(0, 'game.sna', sna());
    const d = dialog.value;
    expect(d?.kind).toBe('snapshot');
    expect(tapes[0].value.blocks.length).toBe(0);
    if (d?.kind !== 'snapshot') return;
    importSnapshot(d.side, d.name, d.bytes, d.format, options, d.insertAtCursor);
    const t = tapes[0].value;
    expect(t.blocks.length).toBe(6);
    expect(t.name).toBe('game.sna');
    expect(t.loadedVersion).toBeNull();
    expect(t.fileHashes).toBeNull(); // the snapshot is not the tape's file
    expect(t.dirty).toBe(false);
  });

  it('says why a bad snapshot will not open', () => {
    loadBytes(0, 'broken.z80', new Uint8Array(5));
    expect(dialog.value).toMatchObject({ kind: 'message', title: 'Cannot load file' });
  });
});
