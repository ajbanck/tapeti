// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

import { describe, it, expect, beforeEach } from 'vitest';
import {
  tapes, locked, clipboard, insertBlocks, unitIndices, moveUnit, undo, redo, markSaved, toggleCollapse, groupSelection,
  copyUnit, paste, deleteUnit, setCursor, selectUids, commit, collapseAll, dialog,
} from '../src/state/store';
import { groupRanges, serializeTzx, fileHashes, renderWav, coreModule, recordingRequest } from '../src/tzx/core';
import { decodeTape } from '../src/tzx/wire';
import { MAX_RECORDING_BYTES } from '../src/state/recordings';
import type { Dialog } from '../src/state/store';
import type { DecodeAnswer, DecodeJob } from '../src/tzx/decode-job';
import { newTape, loadBytes } from '../src/state/files';
import { createBlock, Block, StandardBlock } from '../src/tzx/types';

/** A short ROM data block: flag, bytes, checksum. */
function recordingBlock(): StandardBlock {
  const data = [0xff, 1, 2, 3, 0x55, 0xaa];
  data.push(data.reduce((x, d) => x ^ d, 0));
  return { ...(createBlock(0x10) as StandardBlock), pause: 1000, data: new Uint8Array(data) };
}

const ids = () => tapes[0].value.blocks.map((b) => b.id);
const uidAt = (i: number) => tapes[0].value.blocks[i].uid;

/** Tape: pause, [group start, text, group end], pause. */
function groupedTape(): Block[] {
  return [createBlock(0x20), createBlock(0x21), createBlock(0x30), createBlock(0x22), createBlock(0x20)];
}

beforeEach(() => {
  locked.value = false;
  clipboard.value = [];
  newTape(0);
  newTape(1);
});

describe('store: selection units', () => {
  it('a collapsed group acts as one block', () => {
    insertBlocks(0, 0, groupedTape());
    toggleCollapse(0, uidAt(1));
    setCursor(0, 1);
    expect(unitIndices(tapes[0].value, 1)).toEqual([1, 2, 3]);
    // an expanded group start is just itself
    toggleCollapse(0, uidAt(1));
    expect(unitIndices(tapes[0].value, 1)).toEqual([1]);
  });

  it('the unit is the whole selection when the grabbed block is selected', () => {
    insertBlocks(0, 0, groupedTape());
    selectUids(0, [uidAt(0), uidAt(4)]);
    expect(unitIndices(tapes[0].value, 4)).toEqual([0, 4]);
    expect(unitIndices(tapes[0].value, 2)).toEqual([2]);
  });

  it("'keep' moves the cursor without touching the selection", () => {
    insertBlocks(0, 0, groupedTape());
    selectUids(0, [uidAt(0), uidAt(4)]);
    setCursor(0, 4, 'keep');
    expect(tapes[0].value.cursor).toBe(4);
    expect([...tapes[0].value.selected]).toEqual([uidAt(0), uidAt(4)]);
  });
});

describe('store: moving and grouping', () => {
  it('moving a block down past a collapsed group jumps over the whole group', () => {
    insertBlocks(0, 0, groupedTape());
    toggleCollapse(0, uidAt(1));
    setCursor(0, 0);
    const pause = uidAt(0);
    moveUnit(0, 1);
    expect(ids()).toEqual([0x21, 0x30, 0x22, 0x20, 0x20]);
    expect(uidAt(3)).toBe(pause);
    moveUnit(0, -1);
    expect(ids()).toEqual([0x20, 0x21, 0x30, 0x22, 0x20]);
  });

  it('groupSelection wraps the selection in a named group', () => {
    insertBlocks(0, 0, [createBlock(0x20), createBlock(0x30), createBlock(0x20)]);
    selectUids(0, [uidAt(1), uidAt(2)]);
    setCursor(0, 1, 'keep');
    groupSelection(0, 'Level 1');
    expect(ids()).toEqual([0x20, 0x21, 0x30, 0x20, 0x22]);
    expect((tapes[0].value.blocks[1] as { name: string }).name).toBe('Level 1');
    expect(groupRanges(tapes[0].value.blocks).get(1)).toBe(4);
  });

  it('collapseAll collapses every group and expandAll clears it', () => {
    insertBlocks(0, 0, [...groupedTape(), createBlock(0x24), createBlock(0x25)]);
    collapseAll(0, true);
    expect(tapes[0].value.collapsed).toEqual(new Set([uidAt(1), uidAt(5)]));
    collapseAll(0, false);
    expect(tapes[0].value.collapsed.size).toBe(0);
  });
});

describe('store: clipboard and delete', () => {
  it('paste inserts clones after the cursor with fresh uids', () => {
    insertBlocks(0, 0, [createBlock(0x20), createBlock(0x30)]);
    setCursor(0, 0);
    copyUnit(0);
    paste(0);
    expect(ids()).toEqual([0x20, 0x20, 0x30]);
    expect(uidAt(1)).not.toBe(uidAt(0));
    expect(clipboard.value[0].uid).not.toBe(uidAt(1)); // the clipboard keeps its own copy
  });

  it('deleteUnit removes the whole selection and moves the cursor to the gap', () => {
    insertBlocks(0, 0, groupedTape());
    selectUids(0, [uidAt(1), uidAt(3)]);
    setCursor(0, 3, 'keep');
    deleteUnit(0);
    expect(ids()).toEqual([0x20, 0x30, 0x20]);
    expect(tapes[0].value.cursor).toBe(1);
  });

  it('refuses edits while locked', () => {
    locked.value = true;
    expect(commit(0, (bl) => ({ blocks: [...bl, createBlock(0x20)] }))).toBe(false);
    expect(ids()).toEqual([]);
  });
});

describe('store: undo and dirty tracking', () => {
  it('undo back to the saved state clears dirty; redo sets it again', () => {
    insertBlocks(0, 0, [createBlock(0x20)]);
    markSaved(0, { name: 'a.tzx' });
    expect(tapes[0].value.dirty).toBe(false);
    insertBlocks(0, 1, [createBlock(0x30)]);
    expect(tapes[0].value.dirty).toBe(true);
    undo(0);
    expect(ids()).toEqual([0x20]);
    expect(tapes[0].value.dirty).toBe(false);
    redo(0);
    expect(ids()).toEqual([0x20, 0x30]);
    expect(tapes[0].value.dirty).toBe(true);
    undo(0);
    undo(0); // before the save: still not what is on disk
    expect(ids()).toEqual([]);
    expect(tapes[0].value.dirty).toBe(true);
  });

  it('hashes a file the way everything else does', () => {
    expect(fileHashes(new TextEncoder().encode('abc'))).toEqual({
      crc32: '352441c2', md5: '900150983cd24fb0d6963f7d28e17f72', sha1: 'a9993e364706816aba3e25717850c26c9cd0d89d',
    });
  });

  it('keeps the checksums of the file it read, through edits, emptying and undo', () => {
    const file = serializeTzx([createBlock(0x10)], { major: 1, minor: 20 });
    loadBytes(0, 'game.tzx', file);
    const read = fileHashes(file);
    expect(tapes[0].value.fileHashes).toEqual(read);

    insertBlocks(0, 1, [createBlock(0x20)]);
    expect(tapes[0].value.dirty).toBe(true);
    expect(tapes[0].value.fileHashes).toEqual(read);
    selectUids(0, [uidAt(0), uidAt(1)]);
    setCursor(0, 0, 'keep');
    deleteUnit(0);
    expect(tapes[0].value.fileHashes).toBe(null);
    undo(0);
    expect(tapes[0].value.fileHashes).toEqual(read);

    newTape(0);
    expect(tapes[0].value.fileHashes).toBe(null);
  });

  it('asks how to take a recording in, and refuses one it cannot read', () => {
    loadBytes(0, 'side-a.WAV', new Uint8Array([82, 73, 70, 70]));
    expect(tapes[0].value.blocks).toEqual([]);
    expect(dialog.value?.kind).toBe('message');
    dialog.value = null;

    const wav = renderWav([recordingBlock()], { sampleRate: 22050, mode: 'mic', amplitude: 0.5 });
    loadBytes(1, 'side-b.wav', wav);
    const d = dialog.value as Dialog | null;
    expect(d?.kind).toBe('recording');
    if (d?.kind === 'recording') {
      expect(d.summary).toMatchObject({ format: 'WAV', rateHz: 22050, channels: 1, keptCsw: false, blocks: 1 });
      expect(d.summary.samples).toBe((wav.length - 44) / 2);
    }
    dialog.value = null;

    loadBytes(0, 'huge.wav', new Uint8Array(MAX_RECORDING_BYTES + 1));
    expect(dialog.value).toMatchObject({ kind: 'message', title: 'Cannot load file' });
    dialog.value = null;
  });

  it('decodes a recording in the worker, off the module the page compiled', async () => {
    const block = recordingBlock();
    const wav = renderWav([block], { sampleRate: 44100, mode: 'mic', amplitude: 0.5 });
    const answers: DecodeAnswer[] = [];
    const scope = globalThis as unknown as { self: unknown };
    const before = scope.self;
    const fake = { onmessage: null as ((e: { data: DecodeJob }) => Promise<void>) | null, postMessage: (m: DecodeAnswer) => answers.push(m) };
    scope.self = fake;
    try {
      await import('../src/tzx/decode.worker');
      const job = { module: coreModule(), request: recordingRequest('t.wav', wav), channel: 0, filter: 0 };
      await fake.onmessage!({ data: { ...job, decode: 1 } });
      await fake.onmessage!({ data: { ...job, decode: 0 } });
    } finally {
      scope.self = before;
    }
    // The decode reports how far it has come as it goes, ending on its one block read, all done;
    // the tape comes last.
    const reports = answers.flatMap((a) => ('progress' in a ? [a.progress] : []));
    expect(reports[0].blocks).toBe(0);
    expect(reports[0].done).toBeGreaterThan(0);
    expect(reports.every((p, i) => i === 0 || (p.done >= reports[i - 1].done && p.blocks >= reports[i - 1].blocks))).toBe(true);
    expect(reports[reports.length - 1]).toEqual({ blocks: 1, done: 1 });
    const tapes = answers.filter((a) => !('progress' in a));
    const tape = (a: DecodeAnswer) => {
      if ('error' in a) throw new Error(a.error);
      if ('progress' in a) throw new Error('a count, not a tape');
      return decodeTape(a.payload);
    };
    const decoded = tape(tapes[0]);
    expect(decoded.blocks.map((b) => b.id)).toEqual([0x10]);
    expect((decoded.blocks[0] as StandardBlock).data).toEqual(block.data);
    expect(decoded.warnings).toEqual(['I\t-\tlead-in: 0.2 s of silence before the first block, left out']);
    expect(tape(tapes[1]).blocks.map((b) => b.id)).toEqual([0x15]);
  });

  it('a tape with nothing left on it becomes a new tape, and undo brings it back', () => {
    insertBlocks(0, 0, [createBlock(0x20), createBlock(0x30)]);
    markSaved(0, { name: 'demo.tzx', loadedVersion: { major: 1, minor: 20 } });
    selectUids(0, [uidAt(0), uidAt(1)]);
    setCursor(0, 0, 'keep');
    deleteUnit(0);

    expect(ids()).toEqual([]);
    expect(tapes[0].value.name).toBe('new');
    expect(tapes[0].value.loadedVersion).toBe(null);
    expect(tapes[0].value.cursor).toBe(-1);
    // Nothing to save, so nothing to ask about before a New or an Open.
    expect(tapes[0].value.dirty).toBe(false);

    undo(0);
    expect(ids()).toEqual([0x20, 0x30]);
    expect(tapes[0].value.name).toBe('demo.tzx');
    expect(tapes[0].value.loadedVersion).toEqual({ major: 1, minor: 20 });
    expect(tapes[0].value.dirty).toBe(false);

    redo(0);
    expect(ids()).toEqual([]);
    expect(tapes[0].value.name).toBe('new');
    expect(tapes[0].value.dirty).toBe(false);
  });

  it('a save re-bases the history: undoing past it is dirty and keeps the name', () => {
    insertBlocks(0, 0, [createBlock(0x20), createBlock(0x30)]);
    setCursor(0, 0);
    deleteUnit(0);
    markSaved(0, { name: 'saved.tzx' });
    expect(tapes[0].value.dirty).toBe(false);
    undo(0);
    expect(ids()).toEqual([0x20, 0x30]);
    expect(tapes[0].value.dirty).toBe(true);
    expect(tapes[0].value.name).toBe('saved.tzx');
  });

  it('undo restores cursor and selection', () => {
    insertBlocks(0, 0, groupedTape());
    setCursor(0, 2);
    deleteUnit(0);
    undo(0);
    expect(tapes[0].value.cursor).toBe(2);
    expect(tapes[0].value.selected).toEqual(new Set([uidAt(2)]));
  });
});
