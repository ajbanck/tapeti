import { describe, it, expect } from 'vitest';
import { detectPrograms, programAt, groupRanges } from '../src/tzx/programs';
import { encodeHeader } from '../src/tzx/describe';
import { createBlock, Block, StandardBlock, GroupStartBlock, TextBlock, SelectBlock, ArchiveBlock } from '../src/tzx/types';

const hdr = (name: string, type = 0) => ({ ...createBlock(0x10), data: encodeHeader({ type, typeName: '', name, length: 10, param1: 0, param2: 10 }) } as StandardBlock);
const data = () => ({ ...createBlock(0x10), data: new Uint8Array([0xff, 1, 2, 3]) } as StandardBlock);
const group = (name: string) => ({ ...createBlock(0x21), name } as GroupStartBlock);
const text = (t: string) => ({ ...createBlock(0x30), text: t } as TextBlock);
const names = (blocks: Block[]) => detectPrograms(blocks).map((p) => [p.name, p.start, p.end, p.source]);

describe('program detection', () => {
  it('starts a program at every BASIC Program header and keeps other headers inside it', () => {
    const blocks = [hdr('GAME A'), data(), hdr('scr', 3), data(), createBlock(0x20), hdr('GAME B'), data()];
    expect(names(blocks)).toEqual([['GAME A', 0, 4, 'header'], ['GAME B', 5, 6, 'header']]);
  });

  it('gives leading metadata to the program it introduces and pauses to the one before', () => {
    const blocks = [createBlock(0x32), hdr('A'), data(), createBlock(0x20), text('Next: B'), createBlock(0x12), hdr('B'), data(), createBlock(0x20)];
    expect(names(blocks)).toEqual([['A', 0, 3, 'header'], ['B', 4, 8, 'header']]);
  });

  it('treats a group holding a Program header as one program named after the group', () => {
    const blocks = [group('Game One'), hdr('one'), data(), hdr('scr', 3), data(), createBlock(0x22), group('Game Two'), hdr('two'), data(), createBlock(0x22)];
    expect(names(blocks)).toEqual([['Game One', 0, 5, 'group'], ['Game Two', 6, 9, 'group']]);
  });

  it('leaves a group without a Program header inside the surrounding program', () => {
    const blocks = [hdr('demo'), data(), group('Machine code'), hdr('bin', 3), data(), createBlock(0x22), hdr('other'), data()];
    expect(names(blocks)).toEqual([['demo', 0, 5, 'header'], ['other', 6, 7, 'header']]);
  });

  it('uses Select block entries as named boundaries', () => {
    const sel = { ...createBlock(0x28), entries: [{ offset: 1, text: 'Easy' }, { offset: 3, text: 'Hard' }] } as SelectBlock;
    const blocks = [sel, data(), data(), data(), data()];
    expect(names(blocks)).toEqual([['Easy', 0, 2, 'select'], ['Hard', 3, 4, 'select']]);
  });

  it('is one program named after the archive info when there are no boundaries', () => {
    const arc = { ...createBlock(0x32), entries: [{ type: 0, text: 'Turbo Thing' }] } as ArchiveBlock;
    expect(names([arc, createBlock(0x12), data(), data()])).toEqual([['Turbo Thing', 0, 3, 'tape']]);
    expect(names([data()])).toEqual([['Untitled', 0, 0, 'tape']]);
    expect(names([])).toEqual([]);
  });

  it('finds the program at an index', () => {
    const ps = detectPrograms([hdr('A'), data(), hdr('B'), data()]);
    expect(programAt(ps, 1)?.name).toBe('A');
    expect(programAt(ps, 2)?.name).toBe('B');
    expect(programAt(ps, 9)).toBeUndefined();
  });

  it('pairs nested groups and loops', () => {
    const blocks = [group('a'), createBlock(0x24), createBlock(0x25), group('b'), createBlock(0x22), createBlock(0x22), createBlock(0x21)];
    expect([...groupRanges(blocks)]).toEqual([[1, 2], [3, 4], [0, 5]]);
  });
});
