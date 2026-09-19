import { useState, useEffect } from 'preact/hooks';
import { Side, active, theme, cycleTheme } from '../state/store';
import { COMMANDS, Command, CommandId, commandEnabled, commandKey, commandLabel, runCommand } from '../state/commands';
import { stopPlayback, playing, playPos } from '../state/player';
import { fmtTime } from '../state/store';
import { Icon, IconBtn } from './icons';

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

/** A separator, a command id, or a command with a label override. */
type Entry = CommandId | { id: CommandId; label?: string } | 'sep';

function items(side: Side, entries: Entry[]): MenuItem[] {
  return entries.map((e) => {
    if (e === 'sep') return { sep: true };
    const id = typeof e === 'string' ? e : e.id;
    const c: Command = COMMANDS[id];
    return {
      label: typeof e === 'string' ? commandLabel(id, side) : e.label ?? commandLabel(id, side),
      key: commandKey(id, side),
      disabled: !commandEnabled(id, side),
      checked: c.checked?.(),
      action: () => runCommand(id, side),
    };
  });
}

export function tapeMenu(side: Side): MenuItem[] {
  return items(side, [
    'new', 'open', 'insert-file', 'sep',
    'save-as', 'save-tap', 'export-wav', 'sep',
    'play', 'play-cursor', 'play-selection', 'stop', 'emu-tape', 'sep',
    'programs', 'tape-info', 'consistency', 'compare', 'clear-compare', 'sep',
    'undo', 'redo', 'select-all',
  ]);
}

export function blockMenu(side: Side): MenuItem[] {
  return items(side, [
    'insert', 'view-data', 'view-as-one', 'sep',
    'cut', 'copy', 'paste', 'duplicate', 'delete', 'sep',
    'move-up', 'move-down', 'group', 'toggle-collapse', 'collapse-all', 'expand-all', 'sep',
    'select-program', 'extract', 'sep',
    'emu-cursor', 'emu-selection', 'sep',
    'find-match', 'set-timings',
  ]);
}

export function optionsMenu(): MenuItem[] {
  return items(active.value, ['opt-hex-bytes', 'opt-zero-based']);
}

export function MenuBar() {
  const [open, setOpen] = useState<string | null>(null);
  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(null);
    document.addEventListener('click', close);
    return () => document.removeEventListener('click', close);
  }, [open]);
  const menus: [string, MenuItem[]][] = [
    ['Left', tapeMenu(0)],
    ['Right', tapeMenu(1)],
    ['Block', blockMenu(active.value)],
    ['Options', optionsMenu()],
    ['Help', items(active.value, ['about'])],
  ];
  return (
    <div class="menubar" onClick={(e) => e.stopPropagation()}>
      <div class="brand"><span class="logo"><Icon name="cassette" size={15} /></span>Tapeti</div>
      {menus.map(([title, items]) => (
        <div key={title} onMouseEnter={() => open && open !== title && setOpen(title)}>
          <Menu title={title} items={items} open={open === title} onOpen={() => setOpen(title)} onClose={() => setOpen(null)} />
        </div>
      ))}
      <span class="spacer" />
      {playing.value && (
        <span class="playing-pill" onClick={() => stopPlayback()} title="Click to stop"><span class="dot" />Playing <span class="time">{fmtTime(playPos.value.elapsed)}</span></span>
      )}
      <IconBtn
        name={theme.value === 'dark' ? 'moon' : theme.value === 'light' ? 'sun' : 'monitor'}
        title={`Theme: ${theme.value} (click to change)`}
        onClick={cycleTheme}
      />
    </div>
  );
}
