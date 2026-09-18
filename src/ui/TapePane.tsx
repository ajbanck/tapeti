import { useState, useEffect, useRef } from 'preact/hooks';
import { Side, tapes, active, setCursor, unitIndices, moveBlocks, toggleCollapse, hex, blockNo, zeroBased, dialog } from '../state/store';
import { openFiles, pickAndOpen, saveTzx, confirmDiscard } from '../state/files';
import { checkConsistency, Issue } from '../tzx/consistency';
import { groupRanges } from '../tzx/programs';
import { describeBlock, blockLength, isMetadata } from '../tzx/describe';
import { isDataBlock, isUnknown as isUnknownBlock, BLOCK_NAMES } from '../tzx/types';
import { fmtNum } from '../state/store';
import { BlockEditor } from './BlockEditor';
import { MenuItems, blockMenu } from './MenuBar';
import { viewData, playTape, openInsertDialog, openProgramPicker } from '../state/actions';
import { IconBtn } from './icons';
import { playing, playingSide, playingBlock, stopPlayback } from '../state/player';
import { requiredVersion } from '../tzx/writer';
import { contentLabels } from '../tzx/content';
import { useMemo } from 'preact/hooks';

const DRAG_TYPE = 'application/x-tapeti-blocks';
const EDITOR_MIN = 140;
const EDITOR_DEFAULT = 340;

function loadEditorHeight(): number {
  try {
    const v = Number(localStorage.getItem('tapeti.editorHeight'));
    if (v >= EDITOR_MIN) return v;
  } catch { /* ignore */ }
  return EDITOR_DEFAULT;
}

function category(id: number, unknown: boolean): string {
  if (unknown) return 'cat-unknown';
  if ([0x10, 0x11, 0x14, 0x15, 0x18, 0x19].includes(id)) return 'cat-data';
  if (id === 0x12 || id === 0x13 || id === 0x2b) return 'cat-signal';
  if (id === 0x21 || id === 0x22) return 'cat-struct';
  if (id === 0x20 || (id >= 0x23 && id <= 0x2a)) return 'cat-flow';
  return 'cat-info';
}

const SPLIT_KEY = 'tapeti.paneSplit';
const PANE_MIN = 300; // px
const SPLITTER_W = 10; // px, matches .vsplitter
const PANES_PAD = 10; // px, matches .panes padding

function loadSplit(): number {
  try {
    const v = Number(localStorage.getItem(SPLIT_KEY));
    if (v > 0 && v < 1) return v;
  } catch { /* ignore */ }
  return 0.5;
}

/** The two tape panes with a draggable vertical splitter between them. */
export function Panes() {
  const [split, setSplit] = useState(loadSplit);
  const [dragging, setDragging] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const [, setWidth] = useState(0);
  useEffect(() => {
    // Re-clamp once the container has a size, and whenever the window size changes.
    const onResize = () => setWidth(ref.current?.clientWidth ?? 0);
    onResize();
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);
  const avail = () => (ref.current?.clientWidth ?? 0) - 2 * PANES_PAD - SPLITTER_W;
  const save = (v: number) => { try { localStorage.setItem(SPLIT_KEY, String(Math.round(v * 1000) / 1000)); } catch { /* ignore */ } };
  // Fraction of the width for the left pane, keeping both panes at least PANE_MIN wide.
  // Too narrow for two minimum-width panes: split equally.
  const clamp = (v: number) => {
    const w = avail();
    const min = w > 2 * PANE_MIN ? PANE_MIN / w : 0.5;
    return Math.min(1 - min, Math.max(min, v));
  };
  const onDown = (e: PointerEvent) => {
    e.preventDefault();
    const box = ref.current!.getBoundingClientRect();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    setDragging(true);
    let last = split;
    const move = (ev: PointerEvent) => {
      last = clamp((ev.clientX - box.left - PANES_PAD - SPLITTER_W / 2) / avail());
      setSplit(last);
    };
    const up = () => {
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      el.removeEventListener('pointercancel', up);
      setDragging(false);
      save(last);
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    e.preventDefault();
    e.stopPropagation();
    const v = clamp(split + (e.key === 'ArrowLeft' ? -0.02 : 0.02));
    setSplit(v);
    save(v);
  };
  const reset = () => { setSplit(0.5); save(0.5); };
  return (
    <div class="panes" ref={ref}>
      <TapePane side={0} grow={clamp(split)} />
      <div
        class={'vsplitter' + (dragging ? ' dragging' : '')}
        role="separator"
        aria-orientation="vertical"
        tabIndex={0}
        title="Drag to resize the tapes (double-click: equal widths)"
        onPointerDown={onDown}
        onDblClick={reset}
        onKeyDown={onKey}
      />
      <TapePane side={1} grow={1 - clamp(split)} />
    </div>
  );
}

export function TapePane({ side, grow = 1 }: { side: Side; grow?: number }) {
  const t = tapes[side].value;
  const isActive = active.value === side;
  const [ctx, setCtx] = useState<{ x: number; y: number } | null>(null);
  const [drop, setDrop] = useState<{ index: number; after: boolean } | null>(null);
  const [fileOver, setFileOver] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const paneRef = useRef<HTMLDivElement>(null);
  const [editorH, setEditorH] = useState(loadEditorHeight);
  const [dragging, setDragging] = useState(false);
  const h = hex.value;

  const onSplitterDown = (e: PointerEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = editorH;
    const maxH = Math.max(EDITOR_MIN, (paneRef.current?.clientHeight ?? 800) - 160);
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    setDragging(true);
    let last = startH;
    const move = (ev: PointerEvent) => {
      last = Math.min(maxH, Math.max(EDITOR_MIN, startH + (startY - ev.clientY)));
      setEditorH(last);
    };
    const up = () => {
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      el.removeEventListener('pointercancel', up);
      setDragging(false);
      try { localStorage.setItem('tapeti.editorHeight', String(Math.round(last))); } catch { /* ignore */ }
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  };

  // the list background: the list element itself or the "No tape loaded" placeholder inside it
  const isBackground = (target: EventTarget | null) =>
    target === listRef.current || (target instanceof Element && !!target.closest('.empty'));

  useEffect(() => {
    if (!ctx) return;
    const close = () => setCtx(null);
    document.addEventListener('click', close);
    document.addEventListener('contextmenu', close);
    return () => {
      document.removeEventListener('click', close);
      document.removeEventListener('contextmenu', close);
    };
  }, [ctx]);

  // keep the cursor row visible
  useEffect(() => {
    const el = listRef.current?.querySelector('.row.cursor') as HTMLElement | null;
    if (listRef.current && el) scrollRowIntoView(listRef.current, el);
  }, [t.cursor, side]);

  // One call for the whole list: asking the core per row would send the tape across for each.
  const kinds = useMemo(() => contentLabels(t.blocks), [t.blocks]);
  const issues = useMemo(() => {
    const by = new Map<number, Issue[]>();
    for (const is of checkConsistency(t.blocks, blockNo(0))) {
      if (is.block < 0 || is.severity === 'info') continue;
      by.set(is.block, [...(by.get(is.block) ?? []), is]);
    }
    return by;
  }, [t.blocks, zeroBased.value]);
  const ranges = groupRanges(t.blocks);
  const depth: number[] = [];
  const hidden: boolean[] = [];
  {
    let d = 0;
    let hideUntil = -1;
    for (let i = 0; i < t.blocks.length; i++) {
      const b = t.blocks[i];
      if (b.id === 0x22 || b.id === 0x25) d = Math.max(0, d - 1);
      hidden[i] = i <= hideUntil;
      depth[i] = d;
      const end = ranges.get(i);
      if (end !== undefined) {
        if (t.collapsed.has(b.uid) && end > hideUntil) hideUntil = end;
        d++;
      }
    }
  }

  // The row to mark as playing: a block hidden inside a collapsed range marks the range header.
  let playingRow = playingSide.value === side ? playingBlock.value : -1;
  while (playingRow > 0 && hidden[playingRow]) playingRow--;
  useEffect(() => {
    if (playingRow < 0) return;
    const row = listRef.current?.querySelector(`.row[data-index="${playingRow}"]`) as HTMLElement | null;
    if (listRef.current && row) scrollRowIntoView(listRef.current, row);
  }, [playingRow]);

  const onRowClick = (e: MouseEvent, i: number) => {
    e.preventDefault();
    const mode = e.shiftKey ? 'range' : e.metaKey || e.ctrlKey ? 'toggle' : 'single';
    setCursor(side, i, mode);
    listRef.current?.focus();
  };

  const onContext = (e: MouseEvent, i: number) => {
    e.preventDefault();
    e.stopPropagation();
    // Right-clicking inside the selection keeps it, so the menu acts on the whole selection.
    setCursor(side, i, t.selected.has(t.blocks[i].uid) ? 'keep' : 'single');
    setCtx({ x: e.clientX, y: e.clientY });
  };

  const onDragStart = (e: DragEvent, i: number) => {
    if (!t.selected.has(t.blocks[i].uid)) setCursor(side, i, 'single');
    const cur = tapes[side].value;
    const idx = unitIndices(cur, i);
    e.dataTransfer!.setData(DRAG_TYPE, JSON.stringify({ side, indices: idx }));
    e.dataTransfer!.effectAllowed = 'copyMove';
  };

  const dropTarget = (e: DragEvent): { index: number; after: boolean } => {
    const rows = Array.from(listRef.current!.querySelectorAll('.row:not(.hidden)')) as HTMLElement[];
    for (const r of rows) {
      const rect = r.getBoundingClientRect();
      if (e.clientY < rect.top + rect.height / 2) return { index: Number(r.dataset.index), after: false };
      if (e.clientY < rect.bottom) return { index: Number(r.dataset.index), after: true };
    }
    return { index: t.blocks.length - 1, after: true };
  };

  const onDragOver = (e: DragEvent) => {
    const types = Array.from(e.dataTransfer?.types ?? []);
    if (types.includes(DRAG_TYPE)) {
      e.preventDefault();
      e.dataTransfer!.dropEffect = e.altKey || e.ctrlKey ? 'copy' : 'move';
      setDrop(dropTarget(e));
      autoScroll(e);
    } else if (types.includes('Files')) {
      e.preventDefault();
      setFileOver(true);
    }
  };

  const autoScroll = (e: DragEvent) => {
    const el = listRef.current!;
    const rect = el.getBoundingClientRect();
    const edge = 30;
    if (e.clientY < rect.top + edge) el.scrollTop -= 8 + (rect.top + edge - e.clientY);
    else if (e.clientY > rect.bottom - edge) el.scrollTop += 8 + (e.clientY - (rect.bottom - edge));
  };

  const onDrop = async (e: DragEvent) => {
    e.preventDefault();
    setDrop(null);
    setFileOver(false);
    const payload = e.dataTransfer?.getData(DRAG_TYPE);
    if (payload) {
      const { side: from, indices } = JSON.parse(payload) as { side: Side; indices: number[] };
      const tgt = dropTarget(e);
      let to = tgt.index < 0 ? 0 : tgt.after ? tgt.index + 1 : tgt.index;
      // dropping after a collapsed group start lands after the whole group
      if (tgt.after) {
        const end = ranges.get(tgt.index);
        if (end !== undefined && t.collapsed.has(t.blocks[tgt.index].uid)) to = end + 1;
      }
      const copy = e.altKey || e.ctrlKey;
      if (from === side && !copy && indices.includes(to) && indices.includes(to - 1)) return; // dropped on itself
      moveBlocks(from, indices, side, to, copy);
      return;
    }
    if (e.dataTransfer?.files.length) {
      active.value = side;
      await openFiles(side, e.dataTransfer.files, e.shiftKey);
    }
  };

  return (
    <div
      class={'pane' + (isActive ? ' active' : '')}
      style={{ flexGrow: grow }}
      ref={paneRef}
      onMouseDown={() => (active.value = side)}
    >
      <div class="pane-head">
        <div class="pane-title filename" title={t.name}>
          <span class="side-tag">{side === 0 ? 'L' : 'R'}</span>
          <span class="fname">{t.name}</span>
          {t.dirty && <span class="dirty-dot" title="Unsaved changes" />}
          {t.blocks.length > 0 && (() => { const v = requiredVersion(t.blocks); return <span class="ver" title="TZX version this tape will be saved as">TZX {v.major}.{String(v.minor).padStart(2, '0')}</span>; })()}
        </div>
        <div class="toolbar">
          <IconBtn name="folder" title="Open tape…" onClick={() => confirmDiscard(side, () => pickAndOpen(side))} />
          <IconBtn name="save" title="Save as TZX" disabled={t.blocks.length === 0} onClick={() => saveTzx(side)} />
          <span class="vsep" />
          <IconBtn name="plus" title="Insert block…" onClick={() => openInsertDialog(side)} />
          <IconBtn name={playing.value ? 'stop' : 'play'} title={playing.value ? 'Stop playback' : 'Play from cursor'} disabled={t.blocks.length === 0} onClick={() => (playing.value ? stopPlayback() : playTape(side, true))} />
          <IconBtn name="list" title="Programs…" disabled={t.blocks.length === 0} onClick={() => openProgramPicker(side)} />
          <IconBtn name="info" title="Tape info…" disabled={t.blocks.length === 0} onClick={() => (dialog.value = { kind: 'tapeinfo', side })} />
        </div>
      </div>
      <div
        class={'blocklist' + (fileOver ? ' dragover' : '')}
        tabIndex={0}
        ref={listRef}
        onDragOver={onDragOver}
        onDragLeave={() => { setDrop(null); setFileOver(false); }}
        onDrop={onDrop}
        onClick={(e) => { if (isBackground(e.target)) setCursor(side, -1); }}
        onContextMenu={(e) => { if (isBackground(e.target)) { e.preventDefault(); active.value = side; setCtx({ x: e.clientX, y: e.clientY }); } }}
      >
        {t.blocks.length === 0 && (
          <div class="empty">
            <div class="big">No tape loaded</div>
            Drop a TZX or TAP file here, or use the folder button above.
          </div>
        )}
        {t.blocks.map((b, i) => {
          const sel = t.selected.has(b.uid);
          const cmp = t.compare.get(b.uid);
          const isRange = ranges.has(i);
          const collapsed = isRange && t.collapsed.has(b.uid);
          const cls = ['row'];
          if (i === t.cursor) cls.push('cursor');
          if (sel) cls.push('selected');
          if (i === playingRow) cls.push('playing');
          if (!isDataBlock(b) && (isMetadata(b) || b.id === 0x20 || b.id === 0x23 || b.id === 0x24 || b.id === 0x25 || b.id === 0x26 || b.id === 0x27 || b.id === 0x28 || b.id === 0x2a || b.id === 0x2b)) cls.push('info');
          if (cmp && cmp !== 'none' && cmp !== 'same') cls.push('cmp-' + cmp);
          if (hidden[i]) cls.push('hidden');
          if (drop && drop.index === i) cls.push(drop.after ? 'drop-after' : 'drop-before');
          return (
            <div
              key={b.uid}
              class={cls.join(' ')}
              data-index={i}
              draggable
              onClick={(e) => onRowClick(e, i)}
              onContextMenu={(e) => onContext(e, i)}
              onDblClick={(e) => { e.preventDefault(); if (isRange) toggleCollapse(side, b.uid); else viewData(side); }}
              onDragStart={(e) => onDragStart(e, i)}
              onDragEnd={() => setDrop(null)}
            >
              <span class="num">{blockNo(i)}</span>
              <span class={'badge ' + category(b.id, isUnknownBlock(b))} title={BLOCK_NAMES[b.id] ?? 'Unknown block'}>{b.id.toString(16).toUpperCase().padStart(2, '0')}</span>
              <span class="desc" style={{ paddingLeft: depth[i] * 12 }}>
                {isRange ? (
                  <span class="tog" onClick={(e) => { e.stopPropagation(); toggleCollapse(side, b.uid); }}>{collapsed ? '▸' : '▾'}</span>
                ) : (
                  <span class="tog" />
                )}
                {describeBlock(b, h)}
                {collapsed && <span class="note"> … {ranges.get(i)! - i - 1} block(s)</span>}
              </span>
              <IssueMark issues={collapsed ? rangeIssues(issues, i, ranges.get(i)!) : issues.get(i)} />
              <span class="kindcol">{kinds[i] && <span class="kind" title="Detected content">{kinds[i]}</span>}</span>
              <span class="len">{fmtNum(blockLength(b))}</span>
            </div>
          );
        })}
      </div>
      <div
        class={'splitter' + (dragging ? ' dragging' : '')}
        title="Drag to resize the editor"
        onPointerDown={onSplitterDown}
        onDblClick={() => { setEditorH(EDITOR_DEFAULT); try { localStorage.setItem('tapeti.editorHeight', String(EDITOR_DEFAULT)); } catch { /* ignore */ } }}
      />
      <BlockEditor side={side} height={editorH} />
      {ctx && (
        <div class="ctxmenu" style={{ left: Math.min(ctx.x, window.innerWidth - 240), top: Math.min(ctx.y, window.innerHeight - 420) }} onClick={(e) => e.stopPropagation()}>
          <MenuItems items={blockMenu(side)} onDone={() => setCtx(null)} />
        </div>
      )}
    </div>
  );
}

/** Issues of blocks start..end, so a collapsed group or loop shows what it hides. */
// Scrolls only the list. Element.scrollIntoView also scrolls overflow:hidden ancestors
// (Safari 14 does so on every call), which shifted the whole window during playback.
function scrollRowIntoView(list: HTMLElement, row: HTMLElement) {
  const l = list.getBoundingClientRect();
  const r = row.getBoundingClientRect();
  if (r.top < l.top) list.scrollTop -= l.top - r.top;
  else if (r.bottom > l.top + list.clientHeight) list.scrollTop += r.bottom - (l.top + list.clientHeight);
}

function rangeIssues(by: Map<number, Issue[]>, start: number, end: number): Issue[] {
  const out: Issue[] = [];
  for (let i = start; i <= end; i++) {
    for (const is of by.get(i) ?? []) out.push(i === start ? is : { ...is, message: `#${blockNo(i)}: ${is.message}` });
  }
  return out;
}

/** Consistency marker in the block list: red for errors (invalid), amber for warnings (e.g. bad checksum). */
function IssueMark({ issues }: { issues?: Issue[] }) {
  if (!issues || issues.length === 0) return null;
  const error = issues.some((i) => i.severity === 'error');
  return <span class={'issue ' + (error ? 'error' : 'warning')} title={issues.map((i) => i.message).join('\n')}>!</span>;
}
