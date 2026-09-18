import { describe, it, expect } from 'vitest';
import { fmtKey } from '../src/state/commands';

describe('fmtKey', () => {
  it('uses Ctrl names on Windows and Linux', () => {
    expect(fmtKey('Mod+Z', false)).toBe('Ctrl+Z');
    expect(fmtKey('Mod+Shift+Z', false)).toBe('Ctrl+Shift+Z');
    expect(fmtKey('Mod+↑', false)).toBe('Ctrl+↑');
  });

  it('uses symbols in Apple order on macOS', () => {
    expect(fmtKey('Mod+Z', true)).toBe('⌘Z');
    expect(fmtKey('Mod+Shift+Z', true)).toBe('⇧⌘Z');
    expect(fmtKey('Shift+Mod+A', true)).toBe('⇧⌘A');
    expect(fmtKey('Mod+Alt+Ctrl+X', true)).toBe('⌃⌥⌘X');
  });
});
