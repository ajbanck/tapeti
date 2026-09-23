import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import { parseTape } from '../src/tzx/parser';
import { detectContent } from '../src/tzx/content';
import { basicSource, editBasic, listBasic, basicToText } from '../src/spectrum/basic';
import { disassemblyText } from '../src/spectrum/z80dis';
import { StandardBlock } from '../src/tzx/types';

// The tokeniser is the core's to prove (core/src/spectrum/source.rs and
// core/tests/spectrum.rs); this is the trip through the wasm boundary.

/** The first BASIC program of the demo tape: its body, and where VARS starts in it. */
function program(): { body: Uint8Array; progLen: number } {
  const tape = parseTape(new Uint8Array(fs.readFileSync('test/samples/Tapeti demo.tzx')));
  const i = tape.blocks.findIndex((_, n) => detectContent(tape.blocks, n).kind === 'basic');
  const data = (tape.blocks[i] as StandardBlock).data;
  return { body: data.subarray(1, data.length - 1), progLen: detectContent(tape.blocks, i).progLen! };
}

describe('BASIC as text', () => {
  it('leaves a program it does not change alone', () => {
    const { body, progLen } = program();
    const text = basicSource(body, 0, progLen);
    expect(text.split('\n')[0]).toMatch(/^ +10 REM /);
    const r = editBasic(body, 0, progLen, text);
    expect('program' in r && Array.from(r.program)).toEqual(Array.from(body.subarray(0, progLen)));
  });

  it('tokenises the lines that change', () => {
    const { body, progLen } = program();
    const text = basicSource(body, 0, progLen) + '\n9000 PRINT "bye": GO TO 10';
    const r = editBasic(body, 0, progLen, text);
    if (!('program' in r)) throw new Error('did not tokenise');
    expect(r.program.length).toBeGreaterThan(progLen);
    const listing = basicToText(listBasic(r.program, 0, r.program.length, { showNumbers: false, basic128: false, speccyFormat: false }), { showNumbers: false, basic128: false, speccyFormat: false });
    expect(listing.split('\n').pop()).toBe('9000 PRINT "bye": GO TO 10');
  });

  it('says which lines are wrong', () => {
    const { body, progLen } = program();
    const r = editBasic(body, 0, progLen, '10 CLS\nno number\n20 PRINT "open\n30 print', { anyCase: true });
    expect(r).toEqual({ errors: [{ line: 2, message: expect.stringContaining('number') }, { line: 3, message: expect.stringContaining('string') }] });
  });

  it('checks the syntax of a changed line when asked to', () => {
    const { body, progLen } = program();
    const text = basicSource(body, 0, progLen) + '\n9000 LET a$=1: GO TO';
    // The syntax check defaults to off, so a line only has to be spellable.
    expect('program' in editBasic(body, 0, progLen, text)).toBe(true);
    const r = editBasic(body, 0, progLen, text, { checkSyntax: true });
    expect(r).toEqual({ errors: [{ line: 7, message: expect.stringContaining('string variable takes a string') }] });
    // `editBasic` never checks a line it leaves unchanged, whatever it holds.
    expect('program' in editBasic(body, 0, progLen, basicSource(body, 0, progLen), { checkSyntax: true })).toBe(true);
  });
});

describe('the listing', () => {
  it('shows hidden numbers in hex when asked to', () => {
    const { body, progLen } = program();
    const text = (hexNumbers: boolean) => {
      const opts = { showNumbers: true, basic128: false, speccyFormat: false, hexNumbers };
      return basicToText(listBasic(body, 0, progLen, opts), opts);
    };
    expect(text(false)).toContain('USR 32768{32768}');
    expect(text(true)).toContain('USR 32768{8000}');
  });
});

describe('disassembly as text', () => {
  it('has address, bytes and instruction', () => {
    const text = disassemblyText(new Uint8Array([0xf3, 0xcd, 0x62, 0x05, 0xc9]), 0, 0x8000, 10, { hex: true, romLabels: true });
    const lines = text.split('\n');
    expect(lines[0]).toBe('8000  F3           DI');
    expect(lines[1]).toMatch(/^8001 {2}CD 62 05 {5}CALL .*0562/);
    expect(lines.length).toBe(3);
  });
});
