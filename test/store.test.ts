import { describe, it, expect, beforeEach } from 'vitest';
import {
  tapes, locked, clipboard, insertBlocks, unitIndices, moveUnit, undo, redo, markSaved, toggleCollapse, groupSelection,
  copyUnit, paste, deleteUnit, setCursor, selectUids, commit, collapseAll,
} from '../src/state/store';
import { groupRanges } from '../src/tzx/programs';
import { newTape } from '../src/state/files';
import { createBlock, Block } from '../src/tzx/types';

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

  it('undo restores cursor and selection', () => {
    insertBlocks(0, 0, groupedTape());
    setCursor(0, 2);
    deleteUnit(0);
    undo(0);
    expect(tapes[0].value.cursor).toBe(2);
    expect(tapes[0].value.selected).toEqual(new Set([uidAt(2)]));
  });
});
