import { describe, it, expect } from 'vitest';
import { convertBlock } from '../src/tzx/convert';
import { createBlock, ROM_TIMINGS, TurboBlock, GeneralizedBlock, PauseBlock, TextBlock, GroupStartBlock, StandardBlock } from '../src/tzx/types';

describe('block type conversion', () => {
  it('keeps the uid and shared fields', () => {
    const std = { ...createBlock(0x10), pause: 1234, data: new Uint8Array([0, 1, 2]) } as StandardBlock;
    const turbo = convertBlock(std, 0x11) as TurboBlock;
    expect(turbo.id).toBe(0x11);
    expect(turbo.uid).toBe(std.uid);
    expect(turbo.pause).toBe(1234);
    expect(turbo.data).toBe(std.data);
    expect(turbo.pilotLen).toBe(ROM_TIMINGS.pilotHeader); // flag 0 = header
  });

  it('picks the data pilot length for non-header standard blocks', () => {
    const std = { ...createBlock(0x10), data: new Uint8Array([0xff, 1]) } as StandardBlock;
    expect((convertBlock(std, 0x11) as TurboBlock).pilotLen).toBe(ROM_TIMINGS.pilotData);
  });

  it('builds generalized symbols from turbo timings', () => {
    const turbo = { ...createBlock(0x11), pilot: 2000, sync1: 600, sync2: 700, zero: 800, one: 1600, pilotLen: 3000, data: new Uint8Array(4) } as TurboBlock;
    const g = convertBlock(turbo, 0x19) as GeneralizedBlock;
    expect(g.totd).toBe(32);
    expect(g.pilotSymbols).toEqual([{ flags: 0, pulses: [2000] }, { flags: 0, pulses: [600, 700] }]);
    expect(g.pilotStream).toEqual([{ symbol: 0, reps: 3000 }, { symbol: 1, reps: 1 }]);
    expect(g.dataSymbols).toEqual([{ flags: 0, pulses: [800, 800] }, { flags: 0, pulses: [1600, 1600] }]);
  });

  it('carries text between group names and text blocks', () => {
    const grp = { ...createBlock(0x21), name: 'Level 2' } as GroupStartBlock;
    expect((convertBlock(grp, 0x30) as TextBlock).text).toBe('Level 2');
    const txt = { ...createBlock(0x30), text: 'Hello' } as TextBlock;
    expect((convertBlock(txt, 0x21) as GroupStartBlock).name).toBe('Hello');
  });

  it('keeps the pause when converting to a pause block and drops the data', () => {
    const std = { ...createBlock(0x10), pause: 500, data: new Uint8Array(9) } as StandardBlock;
    const p = convertBlock(std, 0x20) as PauseBlock;
    expect(p).toEqual({ uid: std.uid, id: 0x20, pause: 500 });
  });

  it('leaves unknown blocks and same-type conversions untouched', () => {
    const unk = createBlock(0x99);
    expect(convertBlock(unk, 0x10)).toBe(unk);
    const std = createBlock(0x10);
    expect(convertBlock(std, 0x10)).toBe(std);
  });
});
