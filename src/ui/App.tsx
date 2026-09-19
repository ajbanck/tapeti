import { useEffect } from 'preact/hooks';
import { effect } from '@preact/signals';
import { MenuBar } from './MenuBar';
import { Panes } from './TapePane';
import { StatusBar } from './StatusBar';
import { Dialogs } from './Dialogs';
import { DataWindow } from './DataWindow';
import { active, tapes, dialog, dataWindow, setCursor, undo, redo, Side } from '../state/store';
import { runCommand, KEY_COMMANDS } from '../state/commands';
import { playTape } from '../state/actions';
import { playing, stopPlayback } from '../state/player';

export function App() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      const inField = /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName) || target.isContentEditable;
      const mod = e.metaKey || e.ctrlKey;
      if (e.key === 'Escape') {
        if (dialog.value) dialog.value = null;
        else if (dataWindow.value) dataWindow.value = null;
        return;
      }
      if (dataWindow.value || dialog.value) return; // modal handles its own keys
      const side: Side = active.value;
      const t = tapes[side].value;
      // Global shortcuts that work even inside fields
      if (mod && !e.altKey) {
        const k = e.key.toLowerCase();
        if (k === 'o') { e.preventDefault(); runCommand('open', side); return; }
        if (k === 's') { e.preventDefault(); runCommand(e.shiftKey ? 'save-as' : 'save', side); return; }
        if (k === 'z' && !inField) { e.preventDefault(); if (e.shiftKey) redo(side); else undo(side); return; }
        if (k === 'y' && !inField) { e.preventDefault(); redo(side); return; }
      }
      if (inField) return;
      const key = mod ? e.key.toLowerCase() : e.key;
      const bound = KEY_COMMANDS.find((kc) => kc.mod === mod && (kc.shift === undefined || kc.shift === e.shiftKey) && (kc.mod ? kc.key.toLowerCase() : kc.key) === key);
      if (bound) {
        e.preventDefault();
        runCommand(bound.id, side);
        return;
      }
      if (mod) return;
      // Cursor movement and play/stop are not menu commands: they need the modifier state.
      switch (e.key) {
        case 'ArrowUp':
          e.preventDefault();
          if (t.cursor > 0) setCursor(side, t.cursor - 1, e.shiftKey ? 'range' : 'single');
          break;
        case 'ArrowDown':
          e.preventDefault();
          if (t.cursor < t.blocks.length - 1) setCursor(side, t.cursor + 1, e.shiftKey ? 'range' : 'single');
          break;
        case 'Home':
          e.preventDefault();
          setCursor(side, 0);
          break;
        case 'End':
          e.preventDefault();
          setCursor(side, t.blocks.length - 1);
          break;
        case ' ':
          e.preventDefault();
          if (playing.value) stopPlayback();
          else playTape(side, true);
          break;
      }
    };
    window.addEventListener('keydown', onKey);
    const onBeforeUnload = (e: BeforeUnloadEvent) => {
      if (tapes[0].value.dirty || tapes[1].value.dirty) {
        e.preventDefault();
        e.returnValue = '';
      }
    };
    window.addEventListener('beforeunload', onBeforeUnload);
    // Window title mirrors the active tape
    const disposeTitle = effect(() => {
      const t = tapes[active.value].value;
      document.title = `${t.name}${t.dirty ? ' *' : ''} — Tapeti`;
    });
    return () => {
      disposeTitle();
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('beforeunload', onBeforeUnload);
    };
  }, []);

  return (
    <div class="app" onDragOver={(e) => e.preventDefault()} onDrop={(e) => e.preventDefault()}>
      <MenuBar />
      <Panes />
      <StatusBar />
      <DataWindow />
      <Dialogs />
    </div>
  );
}
