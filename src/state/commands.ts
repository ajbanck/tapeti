// The one table of user commands. The desktop app's menu (menu.rs), the in-app menus and the
// context menu (MenuBar.tsx) and the keyboard shortcuts (App.tsx) all resolve to an entry here,
// so labels, enabled state and behaviour live in a single place. Ids match the item ids in menu.rs.
import {
  Side, tapes, active, dialog, hexBytes, zeroBased, locked, setOption, toggleLock, undo, redo, selectAll,
  deleteUnit, copyUnit, cutUnit, paste, duplicateUnit, moveUnit, groupSelection, toggleCollapse, collapseAll,
  runCompareTapes, runFindMatch, clearCompare, clipboard, theme, applyTheme,
} from './store';
import { groupRanges } from '../tzx/programs';
import { newTape, saveTzx, saveTap, pickAndOpen, confirmDiscard } from './files';
import { setSelectionTimings, viewData, playTape, playSelection, openInsertDialog, openInEmulator, selectProgram, openProgramPicker, extractToOtherPane } from './actions';
import { stopPlayback, playing } from './player';
import { isMac } from '../platform';

export interface Command {
  /** Menu label. A function when it depends on the tape (e.g. collapse vs expand). */
  label: string | ((side: Side) => string);
  /** Shortcut shown in the in-app menus, already formatted with `fmtKey`. */
  key?: string;
  /** Defaults to always enabled. */
  enabled?: (side: Side) => boolean;
  /** Check mark state for toggles. */
  checked?: () => boolean;
  run: (side: Side) => void;
}

const other = (side: Side): Side => (side === 0 ? 1 : 0);
const tape = (side: Side) => tapes[side].value;
const hasBlocks = (side: Side) => tape(side).blocks.length > 0;
const hasCursor = (side: Side) => !!tape(side).blocks[tape(side).cursor];

const MAC_SYMBOLS: Record<string, string> = { Ctrl: '⌃', Alt: '⌥', Shift: '⇧', Mod: '⌘' };

/**
 * Format a shortcut written as `Mod+Shift+Z` for the platform: `⇧⌘Z` on macOS (modifiers in the
 * Apple order ⌃⌥⇧⌘), `Ctrl+Shift+Z` on Windows and Linux. `Mod` is ⌘ on macOS and Ctrl elsewhere.
 */
export function fmtKey(spec: string, mac = isMac): string {
  const parts = spec.split('+');
  const key = parts.pop()!;
  if (!mac) return [...parts.map((p) => (p === 'Mod' ? 'Ctrl' : p)), key].join('+');
  const order = ['Ctrl', 'Alt', 'Shift', 'Mod'];
  return parts.sort((a, b) => order.indexOf(a) - order.indexOf(b)).map((p) => MAC_SYMBOLS[p]).join('') + key;
}

export const COMMANDS = {
  // ---- file
  'new': { label: 'New', run: (s) => confirmDiscard(s, () => newTape(s)) },
  'open': { label: 'Open…', key: fmtKey('Mod+O'), run: (s) => confirmDiscard(s, () => pickAndOpen(s)) },
  'insert-file': { label: 'Insert file at cursor…', run: (s) => pickAndOpen(s, true) },
  /** A browser has nowhere to write back to, so Save is Save as. The desktop app writes
   *  in place — `desktop/src/files.rs` keeps the id and the rule. */
  'save': { label: 'Save', key: fmtKey('Mod+S'), enabled: hasBlocks, run: (s) => saveTzx(s) },
  'save-as': { label: 'Save as TZX (download)', key: fmtKey('Mod+Shift+S'), enabled: hasBlocks, run: (s) => saveTzx(s) },
  'save-tap': { label: 'Save as TAP (download)', enabled: hasBlocks, run: (s) => saveTap(s) },
  'export-wav': { label: 'Export WAV…', enabled: hasBlocks, run: (s) => (dialog.value = { kind: 'wav', side: s }) },
  // ---- edit
  'undo': { label: 'Undo', key: fmtKey('Mod+Z'), enabled: (s) => tape(s).undo.length > 0, run: undo },
  'redo': { label: 'Redo', key: fmtKey('Mod+Shift+Z'), enabled: (s) => tape(s).redo.length > 0, run: redo },
  'cut': { label: 'Cut', key: fmtKey('Mod+X'), enabled: hasCursor, run: cutUnit },
  'copy': { label: 'Copy', key: fmtKey('Mod+C'), enabled: hasCursor, run: copyUnit },
  'paste': { label: 'Paste after cursor', key: fmtKey('Mod+V'), enabled: () => clipboard.value.length > 0, run: (s) => paste(s) },
  'duplicate': { label: 'Duplicate', key: fmtKey('Mod+D'), enabled: hasCursor, run: duplicateUnit },
  'delete': { label: 'Delete', key: isMac ? '⌫' : 'Del', enabled: hasCursor, run: deleteUnit },
  'select-all': { label: 'Select all', key: fmtKey('Mod+A'), enabled: hasBlocks, run: selectAll },
  // ---- block
  // The desktop menu has Mod+Shift+N for this (Mac keyboards have no Insert key).
  'insert': { label: 'Insert block…', key: 'Ins', run: openInsertDialog },
  'view-data': { label: 'View data', key: 'Enter', enabled: hasCursor, run: (s) => viewData(s) },
  'view-as-one': { label: 'View selected as one', enabled: hasCursor, run: (s) => viewData(s, true) },
  'move-up': { label: 'Move up', key: fmtKey('Mod+↑'), enabled: hasCursor, run: (s) => moveUnit(s, -1) },
  'move-down': { label: 'Move down', key: fmtKey('Mod+↓'), enabled: hasCursor, run: (s) => moveUnit(s, 1) },
  'group': { label: 'Group selection', key: fmtKey('Mod+G'), enabled: hasCursor, run: (s) => groupSelection(s, 'Group') },
  'select-program': { label: 'Select program', key: fmtKey('Mod+Shift+A'), enabled: hasCursor, run: selectProgram },
  'extract': { label: 'Extract to other pane', key: fmtKey('Mod+Shift+E'), enabled: hasCursor, run: extractToOtherPane },
  'toggle-collapse': {
    label: (s) => (isCollapsedGroup(s) ? 'Expand group/loop' : 'Collapse group/loop'),
    enabled: (s) => groupRanges(tape(s).blocks).has(tape(s).cursor),
    run: (s) => toggleCollapse(s, tape(s).blocks[tape(s).cursor].uid),
  },
  'collapse-all': { label: 'Collapse all', run: (s) => collapseAll(s, true) },
  'expand-all': { label: 'Expand all', run: (s) => collapseAll(s, false) },
  'find-match': { label: 'Find match', key: fmtKey('Mod+F'), enabled: hasCursor, run: runFindMatch },
  'set-timings': { label: 'Set selection timings to current', enabled: hasCursor, run: setSelectionTimings },
  // ---- tape
  'play': { label: 'Play tape', enabled: hasBlocks, run: (s) => playTape(s) },
  'play-cursor': { label: 'Play from cursor', enabled: hasBlocks, run: (s) => playTape(s, true) },
  'play-selection': { label: 'Play selection', enabled: hasBlocks, run: playSelection },
  'stop': { label: 'Stop playback', enabled: () => playing.value, run: () => stopPlayback() },
  'emu-tape': { label: 'Download tape for emulator', enabled: hasBlocks, run: (s) => openInEmulator(s, 'tape') },
  'emu-cursor': { label: 'Download from cursor for emulator', enabled: hasCursor, run: (s) => openInEmulator(s, 'cursor') },
  'emu-selection': { label: 'Download selection for emulator', enabled: hasCursor, run: (s) => openInEmulator(s, 'selection') },
  'emu-settings': { label: 'Emulator settings…', run: () => (dialog.value = { kind: 'emulator' }) },
  'programs': { label: 'Programs…', key: fmtKey('Mod+J'), enabled: hasBlocks, run: openProgramPicker },
  'tape-info': { label: 'Tape info…', enabled: hasBlocks, run: (s) => (dialog.value = { kind: 'tapeinfo', side: s }) },
  'consistency': { label: 'Check consistency…', enabled: hasBlocks, run: (s) => (dialog.value = { kind: 'consistency', side: s }) },
  'compare': { label: 'Compare tapes', run: () => runCompareTapes() },
  'clear-compare': { label: 'Clear compare marks', run: () => clearCompare() },
  'switch-pane': { label: 'Switch active pane', key: 'Tab', run: (s) => (active.value = other(s)) },
  'toggle-lock': { label: 'Lock tapes', checked: () => locked.value, run: () => toggleLock() },
  // ---- view
  'opt-hex-bytes': { label: 'Flag and checksum bytes in hex', checked: () => hexBytes.value, run: () => setOption('hexBytes', !hexBytes.value) },
  'opt-zero-based': { label: 'Number blocks from 0', checked: () => zeroBased.value, run: () => setOption('zeroBased', !zeroBased.value) },
  'theme-light': { label: 'Theme: light', checked: () => theme.value === 'light', run: () => applyTheme('light') },
  'theme-dark': { label: 'Theme: dark', checked: () => theme.value === 'dark', run: () => applyTheme('dark') },
  'theme-system': { label: 'Theme: system', checked: () => theme.value === 'system', run: () => applyTheme('system') },
  // ---- help
  'about': { label: 'About Tapeti…', run: () => (dialog.value = { kind: 'about' }) },
} satisfies Record<string, Command>;

export type CommandId = keyof typeof COMMANDS;

function isCollapsedGroup(side: Side): boolean {
  const t = tape(side);
  const cur = t.blocks[t.cursor];
  return !!cur && groupRanges(t.blocks).has(t.cursor) && t.collapsed.has(cur.uid);
}

export function commandEnabled(id: CommandId, side: Side): boolean {
  const c: Command = COMMANDS[id];
  return c.enabled ? c.enabled(side) : true;
}

/** Run a command on `side` if it is enabled. Returns whether it ran. */
export function runCommand(id: CommandId, side: Side): boolean {
  if (!commandEnabled(id, side)) return false;
  COMMANDS[id].run(side);
  return true;
}

export function commandLabel(id: CommandId, side: Side): string {
  const l = (COMMANDS[id] as Command).label;
  return typeof l === 'function' ? l(side) : l;
}

export function commandKey(id: CommandId): string | undefined {
  return (COMMANDS[id] as Command).key;
}

/**
 * Keyboard shortcuts handled by the web view (App.tsx). `shift` left out means either. The desktop menu owns the accelerators
 * it declares in menu.rs, so those keydowns never arrive there; this table still lists them
 * for the browser build. Keys are `e.key` values; `mod` means ⌘ on macOS, Ctrl elsewhere.
 */
export const KEY_COMMANDS: { key: string; mod: boolean; shift?: boolean; id: CommandId }[] = [
  { key: 'c', mod: true, id: 'copy' },
  { key: 'x', mod: true, id: 'cut' },
  { key: 'v', mod: true, id: 'paste' },
  { key: 'd', mod: true, id: 'duplicate' },
  { key: 'a', mod: true, shift: false, id: 'select-all' },
  { key: 'a', mod: true, shift: true, id: 'select-program' },
  { key: 'e', mod: true, shift: true, id: 'extract' },
  { key: 'j', mod: true, id: 'programs' },
  { key: 'g', mod: true, id: 'group' },
  { key: 'f', mod: true, id: 'find-match' },
  { key: 'ArrowUp', mod: true, id: 'move-up' },
  { key: 'ArrowDown', mod: true, id: 'move-down' },
  { key: 'Delete', mod: false, id: 'delete' },
  { key: 'Backspace', mod: false, id: 'delete' },
  { key: 'Insert', mod: false, id: 'insert' },
  { key: 'Enter', mod: false, id: 'view-data' },
  { key: 'Tab', mod: false, id: 'switch-pane' },
];
