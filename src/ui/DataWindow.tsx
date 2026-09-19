import { useState, useEffect, useMemo, useRef } from 'preact/hooks';
import { ComponentChildren } from 'preact';
import { dataWindow, tapes, Side, replaceBlock, locked, fmtNum, parseNum, setStatus, blockNo } from '../state/store';
import { downloadBytes, pickFile } from '../state/files';
import { Block } from '../tzx/types';
import { detectContent } from '../tzx/content';
import { decodeHeader, encodeHeader, HEADER_TYPE_NAMES, HeaderInfo } from '../tzx/describe';
import { BitData, joinBits, dropBits, addBits, shiftLeftBits, shiftRightBits, flipBytes, totalBits } from '../tzx/bits';
import { renderScreen, hasFlash, SCREEN_SIZE } from '../spectrum/screen';
import { listBasic, listVariables, basicToText, BasicOptions } from '../spectrum/basic';
import { disassemble, DisLine } from '../spectrum/z80dis';
import { zxChar, dumpChar } from '../spectrum/charset';
import { NumInput, TextInput, Check } from './fields';
import { Icon } from './icons';
import { Modal } from './Dialogs';

type ViewAs = 'dump' | 'header' | 'screen' | 'basic' | 'vars' | 'text' | 'dis';

export function DataWindow() {
  const req = dataWindow.value;
  if (!req) return null;
  const t = tapes[req.side].value;
  const blocks = req.uids.map((u) => t.blocks.find((b) => b.uid === u)).filter((b): b is Block => !!b);
  if (blocks.length === 0) {
    dataWindow.value = null;
    return null;
  }
  return <Inner key={req.uids.join(',')} side={req.side} blocks={blocks} />;
}

function bitDataOf(b: Block): BitData {
  const o: any = b;
  return { data: o.data ?? new Uint8Array(0), usedBits: typeof o.usedBits === 'number' ? o.usedBits : 8 };
}

function Inner({ side, blocks }: { side: Side; blocks: Block[] }) {
  const single = blocks.length === 1;
  const block = blocks[0];
  const hasUsedBits = single && typeof (block as any).usedBits === 'number';
  const guess = useMemo(() => {
    const t = tapes[side].value;
    return detectContent(t.blocks, t.blocks.findIndex((b) => b.uid === block.uid));
  }, [block.uid]);
  const [work, setWork] = useState<BitData>(() => joinBits(blocks.map(bitDataOf)));
  const [base, setBase] = useState(single ? guess.base : 0x8000);
  const [viewAs, setViewAs] = useState<ViewAs>(single && guess.kind === 'header' ? 'header' : single && guess.kind === 'screen' ? 'screen' : single && guess.kind === 'basic' ? 'basic' : 'dump');
  const [flip, setFlip] = useState(false);
  const [reverse, setReverse] = useState(false);
  const [hideFlag, setHideFlag] = useState(single && guess.skipFlag);
  const [hideCs, setHideCs] = useState(single && guess.skipChecksum);
  const [baseBeforeReverse, setBaseBeforeReverse] = useState<number | null>(null);
  const [n, setN] = useState(1);
  const [dirty, setDirty] = useState(false);
  // This window's own Dec/Hex switch: the main window's says nothing about it,
  // and it starts at Dec every time a data window is opened.
  const [h, setH] = useState(false);
  const isLocked = locked.value;
  const modifiers = flip || reverse || hideFlag || hideCs;
  // Typing over a byte works through the modifiers: the index travels back
  // (see setByte). Drop/Add/Shift and the last-byte mask change the length and
  // the bit alignment of the raw stream, whose ends and bit order the modifiers
  // have moved, so those stay off while any modifier is on.
  const editable = single && !isLocked;
  const structural = editable && !modifiers;

  const view = useMemo(() => {
    let d = work.data;
    if (hideFlag && d.length > 0) d = d.subarray(1);
    if (hideCs && d.length > 0) d = d.subarray(0, d.length - 1);
    if (flip) d = flipBytes(d);
    if (reverse) d = d.slice().reverse();
    return d;
  }, [work, flip, reverse, hideFlag, hideCs]);
  const startAddr = reverse ? base - view.length + 1 : base;

  const setBits = (fn: (d: BitData) => BitData) => {
    setWork(fn(work));
    setDirty(true);
  };
  /**
   * Write back a byte the view shows. The view is the raw data with the
   * modifiers applied, so index and value travel the other way: reverse mirrors
   * the index, "hide flag byte" shifts it past byte 0 ("hide checksum byte"
   * only shortens the end, so it does not move anything), and flip is its own
   * inverse on the value.
   */
  const setByte = (i: number, v: number) => {
    const j = (reverse ? view.length - 1 - i : i) + (hideFlag && work.data.length > 0 ? 1 : 0);
    if (j < 0 || j >= work.data.length) return;
    const d = new Uint8Array(work.data);
    d[j] = flip ? flipBytes(new Uint8Array([v]))[0] : v;
    setWork({ ...work, data: d });
    setDirty(true);
  };

  /**
   * Reversed, a screen reads from its last byte down, so the base address is the
   * end of screen memory rather than the start. Put the old base back when the
   * tick comes off, or the picture sits above screen memory and the view goes
   * blank — but leave a base the user has set since alone.
   */
  const toggleReverse = (on: boolean) => {
    setReverse(on);
    if (viewAs !== 'screen') return;
    if (on) {
      setBaseBeforeReverse(base);
      setBase(0x5aff);
    } else if (base === 0x5aff && baseBeforeReverse !== null) {
      setBase(baseBeforeReverse);
      setBaseBeforeReverse(null);
    }
  };

  const close = () => (dataWindow.value = null);
  const ok = () => {
    if (dirty && single) {
      const patch: any = { data: work.data };
      if (hasUsedBits) patch.usedBits = work.usedBits;
      if (block.id === 0x19 && (block as any).dataSymbols.length <= 2) patch.totd = totalBits(work);
      replaceBlock(side, block.uid, { ...(block as any), ...patch });
    }
    close();
  };

  const appendFile = async (replace: boolean) => {
    const f = await pickFile();
    if (!f) return;
    const bytes = f.bytes;
    if (replace) setBits(() => ({ data: bytes, usedBits: 8 }));
    else {
      const cur = work.data;
      const out = new Uint8Array(cur.length + bytes.length);
      out.set(cur);
      out.set(bytes, cur.length);
      setBits(() => ({ data: out, usedBits: 8 }));
    }
    setStatus(`${replace ? 'Replaced with' : 'Appended'} ${bytes.length} bytes from ${f.name}`);
  };
  const saveFile = () => {
    downloadBytes(view, (tapes[side].value.name.replace(/\.(tzx|tap)$/i, '') || 'block') + `-block${blockNo(tapes[side].value.cursor)}.bin`);
  };

  const title = single ? `Data window — block #${blockNo(tapes[side].value.cursor)}` : `Data window — ${blocks.length} blocks viewed as one`;
  const mask = [];
  for (let i = 0; i < 8; i++) mask.push(i < work.usedBits);

  return (
    <Modal title={title} onClose={close} cls="datawin" footer={
      <>
        {/* This window's own Dec/Hex switch, in the corner the main window's status bar keeps its one. */}
        <div class={'basecell' + (h ? ' on' : '')} title="Number base in this window" onClick={() => setH(!h)}>
          <Icon name="hash" size={13} /><b>{h ? 'Hex' : 'Dec'}</b>
        </div>
        <span class="vsep" />
        <button onClick={() => appendFile(false)} disabled={!single || isLocked}>Append file</button>
        <button onClick={() => appendFile(true)} disabled={!single || isLocked}>Replace from file</button>
        <button onClick={saveFile}>Save to file</button>
        <span style={{ flex: 1 }} />
        <button class="primary" onClick={ok} disabled={!dirty}>OK</button>
        <button onClick={close}>{dirty ? 'Cancel' : 'Close'}</button>
      </>
    }>
      <div class="controls">
        <div class="c tabs" role="tablist">
          {([['dump', 'Dump'], ['header', 'Header'], ['screen', 'Screen'], ['basic', 'BASIC'], ['vars', 'Variables'], ['text', 'Text'], ['dis', 'Disassembly']] as [ViewAs, string][]).map(([k, label]) => (
            <button key={k} class={'tab' + (viewAs === k ? ' active' : '')} role="tab" data-view={k} onClick={() => setViewAs(k)}>{label}</button>
          ))}
        </div>
        <div class="c grow" />
        <div class="c"><label>Base address</label><NumInput value={base} max={0xffff} hex={h} onChange={setBase} /></div>
      </div>
      <div class="controls secondary">
        <div class="c stat">Raw length {fmtNum(work.data.length, h)} bytes{hasUsedBits && work.usedBits !== 8 ? ` (${fmtNum(work.usedBits, h)} bits used in last)` : ''}</div>
        <div class="c stat">Length {fmtNum(view.length, h)} bytes</div>
        {(!single || isLocked || modifiers) && <div class="c"><span class="chip">{!single ? 'read-only: multiple blocks' : isLocked ? 'locked' : 'Drop/Add/Shift need the modifiers off'}</span></div>}
      </div>
      <div class="controls secondary">
        <div class="c"><Check label="Flip bytes (RR L)" checked={flip} onChange={setFlip} /></div>
        <div class="c"><Check label="Reverse order (DEC IX)" checked={reverse} onChange={toggleReverse} /></div>
        <div class="c"><Check label="Hide flag byte" checked={hideFlag} onChange={setHideFlag} disabled={!single} /></div>
        <div class="c"><Check label="Hide checksum byte" checked={hideCs} onChange={setHideCs} disabled={!single} /></div>
      </div>

      {viewAs === 'dump' && <Dump data={view} startAddr={startAddr} editable={editable} setByte={setByte} h={h} />}
      {viewAs === 'header' && <HeaderView data={work.data} h={h} editable={editable} onChange={(d) => { setWork({ ...work, data: d }); setDirty(true); }} />}
      {viewAs === 'screen' && <Screen data={view} offset={16384 - startAddr} />}
      {viewAs === 'basic' && <Basic data={view} startAddr={startAddr} progLen={guess.progLen} vars={false} h={h} />}
      {viewAs === 'vars' && <Basic data={view} startAddr={startAddr} progLen={guess.progLen} vars={true} h={h} />}
      {viewAs === 'text' && <TextView data={view} />}
      {viewAs === 'dis' && <Dis data={view} startAddr={startAddr} h={h} />}

      <div class="editrow">
        <span class="label" />
        <button class="small" disabled={!structural} onClick={() => setBits((d) => dropBits(d, n))}>Drop</button>
        <button class="small" disabled={!structural} onClick={() => setBits((d) => addBits(d, n))}>Add</button>
        <button class="small" disabled={!structural} onClick={() => setBits((d) => shiftLeftBits(d, n))}>Shift left</button>
        <button class="small" disabled={!structural} onClick={() => setBits((d) => shiftRightBits(d, n))}>Shift right</button>
        <span>bit(s)</span>
        <span style={{ width: 20 }} />
        <span class="mask">
          <span>Last byte mask</span>
          {mask.map((on, i) => (
            <input key={i} type="checkbox" checked={on} disabled={!structural || !hasUsedBits} title={`bit ${7 - i}`} onChange={() => setBits((d) => ({ ...d, usedBits: i + 1 }))} />
          ))}
        </span>
      </div>
      <div class="editrow">
        <span class="label" />
        <button class="small" disabled={!structural} onClick={() => setBits((d) => dropBits(d, n * 8))}>Drop</button>
        <button class="small" disabled={!structural} onClick={() => setBits((d) => addBits(d, n * 8))}>Add</button>
        <button class="small" disabled={!structural} onClick={() => setBits((d) => shiftLeftBits(d, n * 8))}>Shift left</button>
        <button class="small" disabled={!structural} onClick={() => setBits((d) => shiftRightBits(d, n * 8))}>Shift right</button>
        <span>byte(s)</span>
        <span style={{ width: 20 }} />
        <label>N</label><NumInput value={n} min={1} max={0xffffff} width={70} hex={h} onChange={setN} />
        <span class="note">Drop/Add act on the end of the data; Shift left removes from the start, Shift right inserts zeros at the start.</span>
      </div>
    </Modal>
  );
}

// ---- virtual list -----------------------------------------------------------

function useVirtual(count: number, rowH: number) {
  const ref = useRef<HTMLDivElement>(null);
  const [scroll, setScroll] = useState(0);
  const [height, setHeight] = useState(400);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    setHeight(el.clientHeight);
    return () => ro.disconnect();
  }, []);
  const first = Math.max(0, Math.floor(scroll / rowH) - 2);
  const last = Math.min(count, first + Math.ceil(height / rowH) + 5);
  const ensureVisible = (row: number) => {
    const el = ref.current;
    if (!el) return;
    const top = row * rowH;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + rowH > el.scrollTop + el.clientHeight) el.scrollTop = top + rowH - el.clientHeight;
  };
  return { ref, first, last, onScroll: (e: Event) => setScroll((e.target as HTMLElement).scrollTop), ensureVisible, totalH: count * rowH, offsetH: first * rowH };
}

// ---- Dump ---------------------------------------------------------------------

function Dump({ data, startAddr, editable, setByte, h }: { data: Uint8Array; startAddr: number; editable: boolean; setByte: (i: number, v: number) => void; h: boolean }) {
  const rows = Math.ceil(data.length / 16);
  const v = useVirtual(rows, 18);
  const [cur, setCur] = useState(0);
  const [ascii, setAscii] = useState(false);
  const [nibble, setNibble] = useState(0);
  const [pattern, setPattern] = useState('');
  const [asciiPat, setAsciiPat] = useState('');
  const [hits, setHits] = useState<Set<number>>(new Set());
  const [searchMsg, setSearchMsg] = useState('');
  useEffect(() => { v.ensureVisible(cur >> 4); }, [cur]);
  useEffect(() => { v.ref.current?.focus(); }, []);
  useEffect(() => { if (cur >= data.length) setCur(Math.max(0, data.length - 1)); }, [data.length]);

  const move = (d: number) => {
    setCur(Math.max(0, Math.min(data.length - 1, cur + d)));
    setNibble(0);
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.metaKey || e.ctrlKey) return;
    switch (e.key) {
      case 'ArrowLeft': move(-1); break;
      case 'ArrowRight': move(1); break;
      case 'ArrowUp': move(-16); break;
      case 'ArrowDown': move(16); break;
      case 'PageUp': move(-16 * 16); break;
      case 'PageDown': move(16 * 16); break;
      case 'Home': setCur(0); setNibble(0); break;
      case 'End': setCur(Math.max(0, data.length - 1)); setNibble(0); break;
      case 'Tab': setAscii(!ascii); setNibble(0); break;
      case 'F3': findNext(); break;
      default: {
        if (!editable || data.length === 0) return;
        if (ascii) {
          if (e.key.length === 1) {
            setByte(cur, e.key.charCodeAt(0) & 0xff);
            move(1);
          } else return;
        } else {
          const d = parseInt(e.key, 16);
          if (Number.isNaN(d) || e.key.length !== 1) return;
          const old = data[cur];
          const nv = nibble === 0 ? ((d << 4) | (old & 0x0f)) : ((old & 0xf0) | d);
          setByte(cur, nv);
          if (nibble === 0) setNibble(1);
          else { setNibble(0); move(1); }
        }
      }
    }
    e.preventDefault();
  };

  const parsePattern = (): (number | null)[] | null => {
    const parts = pattern.split(/[\s,]+/).filter((x) => x !== '');
    const out: (number | null)[] = parts.map((p) => (p === '?' || p === '??' ? null : parseNum(p, h)));
    if (out.some((x) => x !== null && (Number.isNaN(x) || x < 0 || x > 255))) return null;
    for (const c of asciiPat) out.push(c.charCodeAt(0) & 0xff);
    return out;
  };

  const findNext = () => {
    const pat = parsePattern();
    if (!pat || pat.length === 0) { setSearchMsg('Enter a search pattern'); return; }
    const from = cur + 1;
    for (let i = from; i + pat.length <= data.length; i++) {
      let ok = true;
      for (let k = 0; k < pat.length; k++) if (pat[k] !== null && data[i + k] !== pat[k]) { ok = false; break; }
      if (ok) {
        setCur(i);
        setHits(new Set(Array.from({ length: pat.length }, (_, k) => i + k)));
        setSearchMsg(`Found at ${fmtNum(startAddr + i, h)}`);
        return;
      }
    }
    setHits(new Set());
    setSearchMsg('Not found (searching from the cursor onward)');
  };

  const lines = [];
  for (let r = v.first; r < v.last; r++) {
    const o = r * 16;
    const cells = [];
    const asc = [];
    for (let k = 0; k < 16; k++) {
      const i = o + k;
      const inRange = i < data.length;
      const isCur = i === cur;
      const cls = 'hexb' + (isCur ? ' cur' + (ascii ? ' ascii-mode' : '') : '') + (hits.has(i) ? ' hit' : '');
      cells.push(<span key={k} class={cls} onMouseDown={(e) => { e.preventDefault(); v.ref.current?.focus(); if (inRange) { setCur(i); setAscii(false); setNibble(0); } }}>{inRange ? data[i].toString(16).toUpperCase().padStart(2, '0') : '  '}</span>);
      if (k === 7) cells.push(<span key="g" class="gap" />);
      asc.push(<span key={k} class={(isCur ? 'cur' + (ascii ? ' ascii-mode' : '') : '') + (hits.has(i) ? ' hit' : '')} onMouseDown={(e) => { e.preventDefault(); v.ref.current?.focus(); if (inRange) { setCur(i); setAscii(true); } }}>{inRange ? dumpChar(data[i]) : ' '}</span>);
    }
    lines.push(
      <div class="line" key={r}>
        <span class="addr">{(startAddr + o).toString(16).toUpperCase().padStart(4, '0')}</span>
        {cells}
        <span class="asc">{asc}</span>
      </div>,
    );
  }
  return (
    <>
      <div class="view dump" ref={v.ref} tabIndex={0} onScroll={v.onScroll} onKeyDown={onKey}>
        <div style={{ height: v.totalH, position: 'relative' }}>
          <div style={{ position: 'absolute', top: v.offsetH, left: 0, right: 0 }}>{lines}</div>
        </div>
        {data.length === 0 && <div style={{ padding: 10, color: '#666' }}>No data. Use Add or Append file.</div>}
      </div>
      <div class="searchrow">
        <span>Cursor {fmtNum(startAddr + cur, h)} (offset {fmtNum(cur, h)}) {editable ? (ascii ? '— typing ASCII' : '— typing hex') : ''}</span>
        <span style={{ flex: 1 }} />
        <label>Search</label>
        <input type="text" placeholder={h ? 'CD ? 05' : '205 ? 5'} value={pattern} onInput={(e) => setPattern((e.target as HTMLInputElement).value)} onKeyDown={(e) => { if (e.key === 'Enter') findNext(); }} />
        <input type="text" placeholder="ASCII" value={asciiPat} onInput={(e) => setAsciiPat((e.target as HTMLInputElement).value)} onKeyDown={(e) => { if (e.key === 'Enter') findNext(); }} />
        <button class="small" onClick={findNext}>Find next</button>
        <span class="note">{searchMsg}</span>
      </div>
    </>
  );
}

// ---- Screen -------------------------------------------------------------------

/** Screen bytes (0..6912) that the block supplies when the screen starts at `offset` in `data`. */
function screenOverlap(len: number, offset: number): number {
  return Math.max(0, Math.min(len, offset + SCREEN_SIZE) - Math.max(0, offset));
}

function Screen({ data, offset: atBase }: { data: Uint8Array; offset: number }) {
  // When the base address puts the whole block outside screen memory, show it from its first
  // byte instead of an all-black picture.
  const fromStart = screenOverlap(data.length, atBase) === 0;
  const offset = fromStart ? 0 : atBase;
  const ref = useRef<HTMLCanvasElement>(null);
  const [hideAttr, setHideAttr] = useState(false);
  const [animate, setAnimate] = useState(true);
  const [phase, setPhase] = useState(false);
  const flashing = useMemo(() => hasFlash(data, offset), [data, offset]);
  useEffect(() => {
    if (!flashing || !animate) return;
    const id = setInterval(() => setPhase((p) => !p), 320);
    return () => clearInterval(id);
  }, [flashing, animate]);
  useEffect(() => {
    const c = ref.current;
    if (!c) return;
    const ctx = c.getContext('2d')!;
    const img = new ImageData(renderScreen(data, offset, { hideAttributes: hideAttr, flashPhase: phase && animate }) as Uint8ClampedArray<ArrayBuffer>, 256, 192);
    ctx.putImageData(img, 0, 0);
  }, [data, offset, hideAttr, phase, animate]);
  const avail = screenOverlap(data.length, offset);
  return (
    <div class="view">
      <div class="screenwrap">
        <canvas ref={ref} width={256} height={192} style={{ width: 512, height: 384 }} />
        <div class="row-flex">
          <Check label="Hide attributes" checked={hideAttr} onChange={setHideAttr} />
          <Check label="Animate FLASH" checked={animate} onChange={setAnimate} disabled={!flashing} />
          <button onClick={() => downloadBytes(data.slice(Math.max(0, offset), Math.max(0, offset) + SCREEN_SIZE), 'screen.scr')}>Save to SCR</button>
          <button onClick={() => ref.current?.toBlob(async (b) => { if (b) downloadBytes(new Uint8Array(await b.arrayBuffer()), 'screen.png', 'image/png'); })}>Save PNG</button>
        </div>
        <div class="note">
          {fromStart && 'No screen bytes at this base address (16384–23295); showing the block from its first byte. '}
          {avail < SCREEN_SIZE ? `Only ${avail} of 6912 screen bytes are present${fromStart ? '' : ' at this base address'}; missing pixels are blank and missing attributes use black ink on white paper.` : 'Full screen present.'}
        </div>
      </div>
    </div>
  );
}

// ---- BASIC / variables -----------------------------------------------------------

function Basic({ data, startAddr, progLen, vars, h }: { data: Uint8Array; startAddr: number; progLen: number | null; vars: boolean; h: boolean }) {
  const [prog, setProg] = useState(startAddr);
  const [varsAddr, setVarsAddr] = useState(progLen !== null ? startAddr + progLen : -1);
  const [opts, setOpts] = useState<BasicOptions>({ showNumbers: false, basic128: false, speccyFormat: false });
  useEffect(() => setProg(startAddr), [startAddr]);
  const progOff = prog - startAddr;
  const lines = useMemo(() => listBasic(data, Math.max(0, progOff), data.length, opts), [data, progOff, opts]);
  const autoVars = useMemo(() => {
    if (varsAddr >= 0) return varsAddr;
    const last = lines[lines.length - 1];
    return last ? startAddr + last.offset + 4 + last.length : prog;
  }, [lines, varsAddr, startAddr, prog]);
  const variables = useMemo(() => (vars ? listVariables(data, autoVars - startAddr, data.length) : []), [data, autoVars, startAddr, vars]);
  const text = useMemo(() => basicToText(lines.filter((l) => l.offset + 4 + l.length <= autoVars - startAddr || varsAddr < 0), opts), [lines, opts, autoVars]);
  return (
    <>
      <div class="row-flex">
        <label>PROG</label><NumInput value={prog} max={0xffff} hex={h} onChange={setProg} width={70} />
        <label>VARS</label><NumInput value={autoVars} max={0xffff} hex={h} onChange={setVarsAddr} width={70} />
        {!vars && <>
          <Check label="Show numbers" checked={opts.showNumbers} onChange={(v) => setOpts({ ...opts, showNumbers: v })} />
          <Check label="Speccy formatting" checked={opts.speccyFormat} onChange={(v) => setOpts({ ...opts, speccyFormat: v })} />
          <Check label="128k BASIC" checked={opts.basic128} onChange={(v) => setOpts({ ...opts, basic128: v })} />
        </>}
        <span class="note">{vars ? `${variables.length} variable(s)` : `${lines.length} line(s)`}</span>
      </div>
      <div class="view">
        {vars ? (
          <table style={{ margin: 6, fontFamily: 'inherit' }}>
            <tr><th style={{ textAlign: 'left' }}>Name</th><th style={{ textAlign: 'left' }}>Type</th><th style={{ textAlign: 'left' }}>Value</th></tr>
            {variables.map((vv, i) => <tr key={i}><td>{vv.name}</td><td>{vv.type}</td><td style={{ userSelect: 'text' }}>{vv.value}</td></tr>)}
          </table>
        ) : opts.speccyFormat ? (
          <pre>{text}</pre>
        ) : (
          <pre>
            {lines.map((l, i) => (
              <div key={i}>
                <span>{l.number.toString().padStart(4, ' ')} </span>
                {l.tokens.map((tk, k) => <span key={k} class={tk.kind}>{tk.text === '\t' ? '    ' : tk.text === '\b' ? '' : tk.text}</span>)}
                {l.error && <span class="ctrl"> [{l.error}]</span>}
              </div>
            ))}
          </pre>
        )}
      </div>
    </>
  );
}

// ---- Text ----------------------------------------------------------------------

function TextView({ data }: { data: Uint8Array }) {
  const [cols, setCols] = useState(32);
  const [tokens, setTokens] = useState(true);
  const text = useMemo(() => {
    const out: string[] = [];
    let line = '';
    let width = 0;
    for (let i = 0; i < data.length; i++) {
      const c = data[i];
      if (c === 0x0d) { out.push(line); line = ''; width = 0; continue; }
      const s = c < 0x20 ? '·' : zxChar(c, tokens);
      line += s;
      width += 1;
      if (width >= cols) { out.push(line); line = ''; width = 0; }
    }
    if (line) out.push(line);
    return out.join('\n');
  }, [data, cols, tokens]);
  return (
    <>
      <div class="row-flex">
        <label>Columns</label>
        <select value={cols} onChange={(e) => setCols(Number((e.target as HTMLSelectElement).value))}>{[32, 64, 128].map((c) => <option key={c} value={c}>{c}</option>)}</select>
        <Check label="Expand tokens" checked={tokens} onChange={setTokens} />
      </div>
      <div class="view"><pre>{text}</pre></div>
    </>
  );
}

// ---- Header --------------------------------------------------------------------

/**
 * The 17 bytes of a standard ROM header, in the fields they stand for, so a
 * header opens on something better than its own hex dump. The same form as the
 * block editor's: this is the data window's copy of it, and it writes the same
 * re-encoded 19 bytes back, checksum and all.
 *
 * It reads the block's own bytes rather than the modified view: a header is the
 * flag, 17 bytes and the checksum, and "hide flag byte" is on by default for
 * exactly this content.
 */
function HeaderView({ data, h, editable, onChange }: { data: Uint8Array; h: boolean; editable: boolean; onChange: (d: Uint8Array) => void }) {
  const hdr = useMemo(() => decodeHeader(data), [data]);
  if (!hdr) {
    return <div class="view"><div class="hdrview note">Not a standard header: that is 19 bytes beginning with flag 0.</div></div>;
  }
  const upd = (p: Partial<HeaderInfo>) => onChange(encodeHeader({ ...hdr, ...p }));
  // The name is padded to 10 bytes and the padding is not the name: it comes off
  // for editing (no room to type under maxLength otherwise) and encodeHeader
  // puts it back.
  const rows: [string, ComponentChildren][] = [
    ['Type', <select value={hdr.type} disabled={!editable} onChange={(e) => upd({ type: Number((e.target as HTMLSelectElement).value) })}>
      {HEADER_TYPE_NAMES.map((n, i) => <option key={i} value={i}>{n}</option>)}
    </select>],
    ['Name', <TextInput value={hdr.name.replace(/ +$/, '')} maxLength={10} width={110} disabled={!editable} onChange={(v) => upd({ name: v })} />],
    ['Length', <><NumInput value={hdr.length} max={0xffff} width={90} hex={h} disabled={!editable} onChange={(v) => upd({ length: v })} /> <span class="note">bytes</span></>],
    [hdr.type === 0 ? 'Autostart line' : hdr.type === 3 ? 'Start address' : 'Variable name',
      <><NumInput value={hdr.param1} max={0xffff} width={90} hex={h} disabled={!editable} onChange={(v) => upd({ param1: v })} />
        {hdr.type === 0 && hdr.param1 >= 32768 ? <span class="note"> no autostart</span> : null}</>],
    [hdr.type === 0 ? 'Program length' : 'Param 2',
      <NumInput value={hdr.param2} max={0xffff} width={90} hex={h} disabled={!editable} onChange={(v) => upd({ param2: v })} />],
  ];
  return (
    <div class="view">
      <div class="hdrview">
        {rows.map(([k, v]) => <div class="row" key={k}><span class="k">{k}</span><span class="v">{v}</span></div>)}
      </div>
    </div>
  );
}

// ---- Disassembly ---------------------------------------------------------------

function Dis({ data, startAddr, h }: { data: Uint8Array; startAddr: number; h: boolean }) {
  const [from, setFrom] = useState(startAddr);
  const [labels, setLabels] = useState(true);
  useEffect(() => setFrom(startAddr), [startAddr]);
  const lines: DisLine[] = useMemo(() => disassemble(data, Math.max(0, from - startAddr), from, 1e6, { hex: h, romLabels: labels }), [data, from, startAddr, h, labels]);
  const v = useVirtual(lines.length, 18);
  const rows = [];
  for (let r = v.first; r < v.last; r++) {
    const l = lines[r];
    const [ins, lbl] = l.text.split('  ; ');
    rows.push(
      <div class="line" key={r}>
        <span class="addr">{l.addr.toString(16).toUpperCase().padStart(4, '0')}</span>
        <span class="bytes">{l.bytes.map((b) => b.toString(16).toUpperCase().padStart(2, '0')).join(' ')}</span>
        <span>{ins}</span>
        {lbl && <span class="lbl">  ; {lbl}</span>}
      </div>,
    );
  }
  return (
    <>
      <div class="row-flex">
        <label>From address</label><NumInput value={from} max={0xffff} hex={h} onChange={setFrom} width={70} />
        <Check label="ROM labels" checked={labels} onChange={setLabels} />
        <span class="note">{lines.length} instruction(s)</span>
      </div>
      <div class="view dis" ref={v.ref} onScroll={v.onScroll}>
        <div style={{ height: v.totalH, position: 'relative' }}>
          <div style={{ position: 'absolute', top: v.offsetH, left: 0, right: 0 }}>{rows}</div>
        </div>
      </div>
    </>
  );
}
