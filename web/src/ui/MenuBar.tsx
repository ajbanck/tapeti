// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

import { useState, useEffect } from 'preact/hooks';
import { Side, active, theme, themePinned, cycleTheme } from '../state/store';
import { COMMANDS, Command, CommandId, commandEnabled, commandKey, commandLabel, desktopOnly, runCommand } from '../state/commands';
import { IconBtn } from './icons';

export interface MenuItem {
  label?: string;
  key?: string;
  action?: () => void;
  disabled?: boolean;
  checked?: boolean;
  sep?: boolean;
}

export function MenuItems({ items, onDone }: { items: MenuItem[]; onDone: () => void }) {
  return (
    <>
      {items.map((it, i) =>
        it.sep ? (
          <div class="sep" key={i} />
        ) : (
          <div
            key={i}
            class={'item' + (it.disabled ? ' disabled' : '') + (it.checked ? ' checked' : '')}
            onClick={(e) => {
              e.stopPropagation();
              if (it.disabled) return;
              onDone();
              it.action?.();
            }}
          >
            <span>{it.label}</span>
            {it.key && <span class="key">{it.key}</span>}
          </div>
        ),
      )}
    </>
  );
}

function Menu({ title, items, open, onOpen, onClose }: { title: string; items: MenuItem[]; open: boolean; onOpen: () => void; onClose: () => void }) {
  return (
    <div class={'menu' + (open ? ' open' : '')}>
      <div class="title" onClick={() => (open ? onClose() : onOpen())} onMouseEnter={() => { /* hover switching handled by parent */ }}>
        {title}
      </div>
      {open && (
        <div class="dropdown">
          <MenuItems items={items} onDone={onClose} />
        </div>
      )}
    </div>
  );
}

// A separator, a command id, or a command with a label override.
type Entry = CommandId | { id: CommandId; label?: string } | 'sep';

const entryId = (e: Entry): CommandId | null => (e === 'sep' ? null : typeof e === 'string' ? e : e.id);

/** `entries` without the given ids, and without the separators that leaves at an end or doubled. */
function without(entries: Entry[], drop: (id: CommandId) => boolean): Entry[] {
  const kept = entries.filter((e) => {
    const id = entryId(e);
    return id === null || !drop(id);
  });
  return kept.filter((e, i, a) => !(e === 'sep' && (i === 0 || i === a.length - 1 || a[i - 1] === 'sep')));
}

const isDesktopOnly = (id: CommandId) => (COMMANDS[id] as Command).enabled === desktopOnly;

function items(side: Side, entries: Entry[]): MenuItem[] {
  return without(entries, isDesktopOnly).map((e) => {
    if (e === 'sep') return { sep: true };
    const id = typeof e === 'string' ? e : e.id;
    const c: Command = COMMANDS[id];
    return {
      label: typeof e === 'string' ? commandLabel(id, side) : e.label ?? commandLabel(id, side),
      key: commandKey(id),
      disabled: !commandEnabled(id, side),
      checked: c.checked?.(),
      action: () => runCommand(id, side),
    };
  });
}

/**
 * The menu bar's groups, all of them run on the active pane. The desktop app's `MENUS` in
 * `desktop/src/menutable.rs` lists the same ids under the same titles; `desktop/tests/menu.rs`
 * reads this table and fails if they drift.
 */
export const MENUS: [string, Entry[]][] = [
  ['File', [
    'new', 'open', 'insert-file', 'sep',
    'save', 'save-as', 'save-tap', 'export-wav',
  ]],
  ['Edit', [
    'undo', 'redo', 'sep',
    'cut', 'copy', 'paste', 'duplicate', 'delete', 'sep',
    'select-all', 'invert-selection', 'select-program',
  ]],
  ['Block', [
    'insert', 'view-data', 'view-as-one', 'sep',
    'move-up', 'move-down', 'sep',
    'group', 'loop', 'toggle-collapse', 'collapse-all', 'expand-all', 'sep',
    'extract', 'set-timings',
  ]],
  ['Tape', [
    'programs', 'tape-info', 'consistency', 'sep',
    'redecode', 'redecode-stretch', 'decode-signal', 'sep',
    'compare', 'find-match', 'clear-compare', 'sep',
    'toggle-lock',
  ]],
  ['Play', [
    'play', 'play-cursor', 'play-selection', 'stop', 'sep',
    'step-next', 'step-reset', 'sep',
    'emu-tape', 'emu-cursor', 'emu-selection', 'sep',
    'emu-settings',
  ]],
  ['View', [
    'opt-zero-based', 'opt-hex-bytes', 'opt-backup', 'sep',
    'theme-light', 'theme-dark', 'theme-system', 'sep',
    'switch-pane',
  ]],
  ['Help', ['about']],
];

/** The pane header's overflow menu: what is per tape and has no button of its own there. */
export function paneMenu(side: Side): MenuItem[] {
  return items(side, [
    'new', 'insert-file', 'sep',
    'save-as', 'save-tap', 'export-wav', 'sep',
    'play', 'play-selection', 'sep',
    'consistency', 'redecode',
  ]);
}

/** The right-click menu of a block: the short list, not the whole Block menu. */
export function contextMenu(side: Side): MenuItem[] {
  return items(side, [
    'view-data', 'sep',
    'cut', 'copy', 'paste', 'duplicate', 'delete', 'sep',
    'insert', 'group', 'toggle-collapse', 'sep',
    'select-program', 'extract', 'sep',
    { id: 'play-cursor', label: 'Play from here' },
    { id: 'emu-cursor', label: commandLabel('emu-cursor', side).replace('from cursor', 'from here') }, 'sep',
    'decode-signal', 'redecode-stretch',
  ]);
}

const THEME_IDS: Entry[] = ['theme-light', 'theme-dark', 'theme-system'];

/** The menus as shown: a pinned theme (`?theme=`) is the embedding page's to switch, so its items go. */
const SHOWN_MENUS: [string, Entry[]][] = !themePinned ? MENUS : MENUS.map(([title, entries]) => [
  title,
  without(entries, (id) => THEME_IDS.includes(id)),
]);

export function MenuBar() {
  const [open, setOpen] = useState<string | null>(null);
  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(null);
    document.addEventListener('click', close);
    return () => document.removeEventListener('click', close);
  }, [open]);
  const menus: [string, MenuItem[]][] = SHOWN_MENUS.map(([title, entries]) => [title, items(active.value, entries)]);
  return (
    <div class="menubar" onClick={(e) => e.stopPropagation()}>
      {menus.map(([title, items]) => (
        <div key={title} onMouseEnter={() => open && open !== title && setOpen(title)}>
          <Menu title={title} items={items} open={open === title} onOpen={() => setOpen(title)} onClose={() => setOpen(null)} />
        </div>
      ))}
      <span class="spacer" />
      {!themePinned && (
        <IconBtn
          name={theme.value === 'dark' ? 'moon' : theme.value === 'light' ? 'sun' : 'monitor'}
          title={`Theme: ${theme.value} (click to change)`}
          onClick={cycleTheme}
        />
      )}
    </div>
  );
}
