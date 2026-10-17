// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

import { useState, useMemo, useEffect, useRef } from 'preact/hooks';
import { ComponentChildren } from 'preact';
import { Dialog, dialog, tapes, Side, insertBlocks, setCursor, audioMode, fmtNum, fmtTime, unitIndices, hex, blockNo, zeroBased } from '../state/store';
import { cannotWrite, downloadBytes, importSnapshot, insertDataFile, headerName, MAX_FILE_BYTES, pickFile } from '../state/files';
import {
  SNAPSHOT_SPEEDS,
  SNAPSHOT_SPEED_NAMES,
  SNAPSHOT_MACHINES,
  DEFAULT_SNAPSHOT_SPEED,
  SCREEN_SIZE,
  renderScreen,
  checkConsistency,
  tapeDuration,
  renderWav,
  playbackOrder,
  blockDuration,
  TSTATES_PER_SEC,
  requiredVersion,
  saveVersion,
  serializeTzx,
  describeBlock,
  encodeHeader,
  detectPrograms,
} from '../tzx/core';
import { NumInput } from './fields';
import { createBlock, CREATABLE_IDS, BLOCK_NAMES, Block, StandardBlock } from '../tzx/types';
import { jumpToProgram } from '../state/actions';
import { importRecording } from '../state/recordings';

export function Modal({ title, onClose, children, footer, width, cls }: { title: string; onClose: () => void; children: ComponentChildren; footer?: ComponentChildren; width?: number; cls?: string }) {
  return (
    <div class="overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div class={'modal ' + (cls ?? '')} style={width ? { width } : undefined} onKeyDown={(e) => { if (e.key === 'Escape') { e.stopPropagation(); onClose(); } }}>
        <div class="mtitle">
          <span>{title}</span>
          <button onClick={onClose}>✕</button>
        </div>
        <div class="mbody">{children}</div>
        {footer && <div class="mfooter">{footer}</div>}
      </div>
    </div>
  );
}

function close() {
  dialog.value = null;
}

export function Dialogs() {
  const d = dialog.value;
  if (!d) return null;
  switch (d.kind) {
    case 'message':
      return (
        <Modal title={d.title} onClose={close} footer={<button class="primary" onClick={close}>OK</button>}>
          {d.lines.map((l, i) => <div key={i} style={{ margin: '2px 0' }}>{l}</div>)}
        </Modal>
      );
    case 'confirm':
      return (
        <Modal title={d.title} onClose={close} footer={<><button class="primary" onClick={() => { close(); d.onOk(); }}>OK</button><button onClick={close}>Cancel</button></>}>
          {d.lines.map((l, i) => <div key={i}>{l}</div>)}
        </Modal>
      );
    case 'about':
      return (
        <Modal title="About Tapeti" onClose={close} footer={<button class="primary" onClick={close}>OK</button>}>
          <p>An editor for ZX Spectrum TZX and TAP tape images.</p>
          <p class="note">Copyright (C) 2026 AJ Banck. Tapeti is free software under the GNU General Public License, version 2 or (at your option) any later version, and comes with absolutely no warranty.</p>
          <p class="note">Parts derive from Taper, Copyleft (C) 1997-2001 ThunderWare Research Center, written by Martijn van der Heide.</p>
        </Modal>
      );
    case 'insert':
      return <InsertDialog side={d.side} />;
    case 'tapeinfo':
      return <TapeInfo side={d.side} />;
    case 'consistency':
      return <Consistency side={d.side} />;
    case 'wav':
      return <WavExport side={d.side} />;
    case 'programs':
      return <ProgramPicker side={d.side} />;
    case 'emulator':
      return <EmulatorDialog />;
    case 'snapshot':
      return <SnapshotImport d={d} />;
    case 'recording':
      return <RecordingImport d={d} />;
    case 'datafile':
      return <DataFileInsert d={d} />;
  }
  return null;
}

/** Not a block ID: the Insert dialog's entry for an empty BASIC program. */
const BASIC_PROGRAM = 0;

/**
 * A BASIC program with nothing in it yet: the Program header, with no autostart
 * line, and the data block behind it. The data window's BASIC view fills it in
 * and keeps the header's lengths right as it does. `empty_program` in the core.
 */
function emptyProgram(): Block[] {
  const header = encodeHeader({ type: 0, typeName: 'Program', name: 'program', length: 0, param1: 0x8000, param2: 0 });
  return [
    { ...(createBlock(0x10) as StandardBlock), data: header },
    { ...(createBlock(0x10) as StandardBlock), data: new Uint8Array([0xff, 0xff]) },
  ];
}

function InsertDialog({ side }: { side: Side }) {
  const [id, setId] = useState(0x10);
  const [where, setWhere] = useState<'before' | 'after' | 'end'>('after');
  const t = tapes[side].value;
  const doInsert = () => {
    const at = where === 'end' || t.cursor < 0 ? t.blocks.length : where === 'after' ? t.cursor + 1 : t.cursor;
    insertBlocks(side, at, id === BASIC_PROGRAM ? emptyProgram() : [createBlock(id)]);
    close();
  };
  return (
    <Modal title="Insert block" onClose={close} footer={<><button class="primary" onClick={doInsert}>Insert</button><button onClick={close}>Cancel</button></>}>
      <div class="typelist" onDblClick={doInsert}>
        {CREATABLE_IDS.map((i) => (
          <div key={i} class={'t' + (i === id ? ' sel' : '')} onClick={() => setId(i)}>
            <code>{i.toString(16).toUpperCase().padStart(2, '0')}</code> {BLOCK_NAMES[i]}
          </div>
        ))}
        {/* After the block types, the one thing here that is two blocks. */}
        <div class={'t' + (id === BASIC_PROGRAM ? ' sel' : '')} onClick={() => setId(BASIC_PROGRAM)}>
          <code>{'\u00a0\u00a0'}</code> BASIC program (header and data, to edit as text)
        </div>
      </div>
      <div class="row-flex" style={{ marginTop: 8 }}>
        <label><input type="radio" checked={where === 'before'} onChange={() => setWhere('before')} /> Before cursor</label>
        <label><input type="radio" checked={where === 'after'} onChange={() => setWhere('after')} /> After cursor</label>
        <label><input type="radio" checked={where === 'end'} onChange={() => setWhere('end')} /> At end</label>
      </div>
    </Modal>
  );
}

/** Type-ahead list of the programs (games) on the tape; picking one selects its blocks. */
function ProgramPicker({ side }: { side: Side }) {
  const t = tapes[side].value;
  const programs = useMemo(() => detectPrograms(t.blocks), [t.blocks]);
  const [filter, setFilter] = useState('');
  const [hi, setHi] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);
  const q = filter.trim().toLowerCase();
  const shown = q ? programs.filter((p) => p.name.toLowerCase().includes(q)) : programs;
  const pick = (i: number) => {
    const p = shown[i];
    if (!p) return;
    close();
    jumpToProgram(side, p);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); setHi(Math.min(hi + 1, shown.length - 1)); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); setHi(Math.max(hi - 1, 0)); }
    else if (e.key === 'Enter') { e.preventDefault(); pick(hi); }
  };
  const cur = Math.min(hi, Math.max(0, shown.length - 1));
  return (
    <Modal title="Programs" onClose={close} width={460} cls="programs" footer={<><button class="primary" disabled={!shown.length} onClick={() => pick(cur)}>Select</button><button onClick={close}>Cancel</button></>}>
      <input ref={input} type="text" class="filter" placeholder="Filter by name…" value={filter} onInput={(e) => { setFilter((e.target as HTMLInputElement).value); setHi(0); }} onKeyDown={onKey} />
      <div class="typelist proglist" onDblClick={() => pick(cur)}>
        {shown.map((p, i) => (
          <div key={p.start} class={'t' + (i === cur ? ' sel' : '')} onClick={() => setHi(i)} onDblClick={() => pick(i)} title={p.source === 'tape' ? 'No program boundaries found: the whole tape' : `Boundary from ${p.source === 'header' ? 'a BASIC Program header' : p.source === 'group' ? 'a group' : 'a Select block'}`}>
            <span class="name">{p.name}{p.source === 'tape' && <span class="note"> (whole tape)</span>}</span>
            <code>#{blockNo(p.start)}–#{blockNo(p.end)} · {p.end - p.start + 1} block{p.end - p.start ? 's' : ''}</code>
          </div>
        ))}
        {shown.length === 0 && <div class="empty">No program matches</div>}
      </div>
      <p class="note">Programs start at BASIC Program headers, at groups that contain one, and at Select block entries. Group blocks manually to fix a wrong split.</p>
    </Modal>
  );
}

const fmtVersion = (v: { major: number; minor: number }) => `${v.major}.${String(v.minor).padStart(2, '0')}`;

function TapeInfo({ side }: { side: Side }) {
  const t = tapes[side].value;
  const info = useMemo(() => {
    const issues = checkConsistency(t.blocks).filter((i) => i.severity === 'error');
    const dur = issues.length ? null : tapeDuration(t.blocks);
    const v = saveVersion(t.blocks, t.loadedVersion);
    const needed = requiredVersion(t.blocks);
    const problem = cannotWrite(t.blocks, 'tzx');
    const size = problem ? null : serializeTzx(t.blocks, v).length;
    const dataBytes = t.blocks.reduce((a, b) => a + ('data' in b ? (b as any).data.length : 0), 0);
    return { issues, dur, v, needed, size, problem, dataBytes };
  }, [t.blocks, t.loadedVersion]);
  const h = hex.value;
  return (
    <Modal title={`${side === 0 ? 'Left' : 'Right'} tape info`} onClose={close} footer={<button class="primary" onClick={close}>OK</button>}>
      <table>
        <tr><td>File</td><td>{t.name}</td></tr>
        <tr><td>Blocks</td><td>{fmtNum(t.blocks.length)}</td></tr>
        <tr><td>TZX version of the file</td><td>{t.loadedVersion ? fmtVersion(t.loadedVersion) : 'none (not loaded from a TZX file)'}</td></tr>
        <tr><td>TZX version the blocks need</td><td>{fmtVersion(info.needed)}</td></tr>
        <tr><td>TZX version when saved</td><td>{fmtVersion(info.v)}</td></tr>
        <tr><td>File size</td><td>{info.size === null ? `cannot be saved: ${info.problem}` : `${fmtNum(info.size)} bytes`}</td></tr>
        <tr><td>Data payload</td><td>{fmtNum(info.dataBytes)} bytes</td></tr>
        <tr><td>Estimated length</td><td>{info.dur ? `${fmtTime(info.dur.seconds)} (${info.dur.order.length} blocks played)` : 'n/a — fix consistency errors first'}</td></tr>
        {t.fileHashes ? <>
          <tr><td>CRC32</td><td><code>{t.fileHashes.crc32}</code></td></tr>
          <tr><td>MD5</td><td><code>{t.fileHashes.md5}</code></td></tr>
          <tr><td>SHA-1</td><td><code>{t.fileHashes.sha1}</code></td></tr>
        </> : <tr><td>Checksums</td><td>none (not read from a TZX or TAP file)</td></tr>}
      </table>
      {t.fileHashes && t.dirty && <p class="note">The checksums are of the file as opened or last saved; the tape has changed since.</p>}
      {info.issues.length > 0 && <p class="error">{info.issues.length} consistency error(s) — see Check consistency.</p>}
      <p class="note">Durations are computed from block timings and pauses at 3.5 MHz{h ? '' : ''}.</p>
    </Modal>
  );
}

function Consistency({ side }: { side: Side }) {
  const t = tapes[side].value;
  const issues = useMemo(() => checkConsistency(t.blocks, blockNo(0)), [t.blocks, zeroBased.value]);
  return (
    <Modal title={`Consistency check — ${side === 0 ? 'left' : 'right'} tape`} onClose={close} footer={<button class="primary" onClick={close}>OK</button>} width={560}>
      {issues.length === 0 ? <p>No problems found.</p> : (
        <div>
          {issues.map((i, k) => (
            <div key={k} class={'issue ' + i.severity} onClick={() => { if (i.block >= 0) setCursor(side, i.block); }}>
              {i.block >= 0 ? `#${blockNo(i.block)} ${describeBlock(t.blocks[i.block], hex.value)}: ` : ''}{i.message}
            </div>
          ))}
        </div>
      )}
    </Modal>
  );
}

function WavExport({ side }: { side: Side }) {
  const t = tapes[side].value;
  const [rate, setRate] = useState(44100);
  const [bits, setBits] = useState<8 | 16>(16);
  const [what, setWhat] = useState<'tape' | 'selection'>('tape');
  const [busy, setBusy] = useState(false);
  const sel = unitIndices(t, t.cursor);
  const order = what === 'tape' ? playbackOrder(t.blocks) : sel;
  const secs = order.reduce((a, i) => a + blockDuration(t.blocks[i]), 0) / TSTATES_PER_SEC + 1;
  const go = () => {
    setBusy(true);
    setTimeout(() => {
      // One call: the samples of a long tape stay inside the core instead of
      // being copied out only to be sent back for encoding.
      const wav = renderWav(t.blocks, { sampleRate: rate, mode: audioMode.value }, bits, order);
      downloadBytes(wav, t.name.replace(/\.(tzx|tap)$/i, '') + '.wav', 'audio/wav');
      setBusy(false);
      close();
    }, 20);
  };
  return (
    <Modal title="Export WAV" onClose={close} footer={<><button class="primary" onClick={go} disabled={busy || order.length === 0}>{busy ? 'Rendering…' : 'Export'}</button><button onClick={close}>Cancel</button></>}>
      <div class="grid" style={{ gridTemplateColumns: 'auto auto' }}>
        <label>Sample rate</label>
        <select value={rate} onChange={(e) => setRate(Number((e.target as HTMLSelectElement).value))}>
          {[48000, 44100, 22050, 11025, 8000].map((r) => <option key={r} value={r}>{r} Hz</option>)}
        </select>
        <label>Resolution</label>
        <select value={bits} onChange={(e) => setBits(Number((e.target as HTMLSelectElement).value) as 8 | 16)}>
          <option value={16}>16 bit</option>
          <option value={8}>8 bit</option>
        </select>
        <label>Waveform</label>
        <select value={audioMode.value} onChange={(e) => (audioMode.value = (e.target as HTMLSelectElement).value as 'mic' | 'square')}>
          <option value="mic">MIC emulation</option>
          <option value="square">Square wave</option>
        </select>
        <label>Content</label>
        <div>
          <label><input type="radio" checked={what === 'tape'} onChange={() => setWhat('tape')} /> Whole tape (following loops/jumps)</label><br />
          <label><input type="radio" checked={what === 'selection'} onChange={() => setWhat('selection')} disabled={sel.length === 0} /> Selection ({sel.length} block(s))</label>
        </div>
      </div>
      <p class="note">About {fmtTime(secs)} of audio, {fmtNum(Math.round(secs * rate * bits / 8 / 1024))} KB.</p>
    </Modal>
  );
}

/**
 * A file that is no tape was inserted: it becomes a data block, with the header
 * SAVE "name" CODE would have put in front.
 */
function DataFileInsert({ d }: { d: Extract<Dialog, { kind: 'datafile' }> }) {
  const [name, setName] = useState(headerName(d.name));
  const [address, setAddress] = useState(d.bytes.length === 6912 ? 16384 : 32768);
  const [withHeader, setWithHeader] = useState(true);
  const tooBig = d.bytes.length > MAX_FILE_BYTES;
  const go = () => {
    close();
    insertDataFile(d.side, name, d.bytes, address, withHeader);
  };
  return (
    <Modal title="Insert file as data" onClose={close} width={440} cls="datafile" footer={<><button class="primary" disabled={tooBig} onClick={go}>Insert</button><button onClick={close}>Cancel</button></>}>
      <div class="grid" style={{ gridTemplateColumns: 'auto 1fr' }}>
        <label>File</label>
        <div>{d.name} ({fmtNum(d.bytes.length)} bytes)</div>
        <label></label>
        <label><input type="checkbox" checked={withHeader} onChange={(e) => setWithHeader((e.target as HTMLInputElement).checked)} /> With a header in front</label>
        <label>Name</label>
        <input type="text" maxLength={10} value={name} disabled={!withHeader} style={{ width: 130 }} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
        <label>Start address</label>
        <NumInput value={address} max={0xffff} width={90} disabled={!withHeader} onChange={setAddress} />
      </div>
      {tooBig
        ? <p class="error">A data block holds {fmtNum(MAX_FILE_BYTES)} bytes at most.</p>
        : <p class="note">Goes in at the cursor as a Bytes header and a standard speed data block: flag FF, the file, checksum.</p>}
    </Modal>
  );
}

const BORDER_NAMES = ['Black', 'Blue', 'Red', 'Magenta', 'Green', 'Cyan', 'Yellow', 'White'];

/** A snapshot was opened: ask how the tape that loads it should be made. */
function SnapshotImport({ d }: { d: Extract<Dialog, { kind: 'snapshot' }> }) {
  const [speed, setSpeed] = useState(DEFAULT_SNAPSHOT_SPEED);
  const [border, setBorder] = useState(d.info.border);
  const [compressAll, setCompressAll] = useState(false);
  const [screen, setScreen] = useState<{ name: string; bytes: Uint8Array } | null>(null);
  const [error, setError] = useState('');
  const canvas = useRef<HTMLCanvasElement>(null);
  const shown = screen?.bytes ?? d.info.screen;
  useEffect(() => {
    const ctx = canvas.current?.getContext('2d');
    if (!ctx || shown.length < SCREEN_SIZE) return;
    ctx.putImageData(new ImageData(renderScreen(shown, 0) as Uint8ClampedArray<ArrayBuffer>, 256, 192), 0, 0);
  }, [shown]);
  const pickScreen = async () => {
    const f = await pickFile();
    if (!f) return;
    if (f.bytes.length !== SCREEN_SIZE) {
      setError(`A loading screen is ${fmtNum(SCREEN_SIZE)} bytes; ${f.name} has ${fmtNum(f.bytes.length)}.`);
      return;
    }
    setError('');
    setScreen(f);
  };
  const go = () => {
    close();
    importSnapshot(d.side, d.name, d.bytes, d.format, { speed, border, compressAll, screen: screen?.bytes ?? null }, d.insertAtCursor);
  };
  return (
    <Modal title="Import snapshot" onClose={close} width={520} cls="snapshot" footer={<><button class="primary" onClick={go}>Import</button><button onClick={close}>Cancel</button></>}>
      <div class="grid" style={{ gridTemplateColumns: 'auto 1fr' }}>
        <label>Snapshot</label>
        <div>{d.name} ({SNAPSHOT_MACHINES[d.info.machine]})</div>
        <label>Loading speed</label>
        <select value={speed} onChange={(e) => setSpeed(Number((e.target as HTMLSelectElement).value))}>
          {SNAPSHOT_SPEEDS.map((bps, i) => <option key={i} value={i}>{SNAPSHOT_SPEED_NAMES[i]} ({bps} bps)</option>)}
        </select>
        <label>Border</label>
        <select value={border} onChange={(e) => setBorder(Number((e.target as HTMLSelectElement).value))}>
          {BORDER_NAMES.map((n, i) => <option key={i} value={i}>{i} {n}</option>)}
        </select>
        <label>Method</label>
        <select value={compressAll ? 1 : 0} onChange={(e) => setCompressAll((e.target as HTMLSelectElement).value === '1')}>
          <option value={0}>Cleanest: nothing but the picture on screen while loading</option>
          <option value={1}>Fastest: pack every block</option>
        </select>
        <label>Loading screen</label>
        <div class="row-flex">
          <span>{screen ? screen.name : "The snapshot's own"}</span>
          <button onClick={pickScreen}>Choose…</button>
          {screen && <button onClick={() => setScreen(null)}>Reset</button>}
        </div>
        <label></label>
        <canvas ref={canvas} width={256} height={192} style={{ imageRendering: 'pixelated', border: '1px solid var(--border)' }} />
      </div>
      {error && <p class="error">{error}</p>}
      <p class="note">
        The snapshot becomes a BASIC loader followed by its memory as packed blocks{speed === 0 ? ' at ROM speed, which still need the custom loader' : ''}.
        The loader runs at BE00 and finishes in the bottom three pixel lines of the screen, so those and the bottom attribute row are lost.
        {speed === 3 && ' 6000 bps is for emulators and direct playback, not for recording to tape.'}
      </p>
    </Modal>
  );
}

/** A recording was opened: decode it into blocks or keep it as raw signal, which channel, and
 *  whether to filter it. The desktop app's "Import recording" dialog. */
function RecordingImport({ d }: { d: Extract<Dialog, { kind: 'recording' }> }) {
  const s = d.summary;
  const audio = !s.keptCsw;
  const [decode, setDecode] = useState(true);
  // 3 is whichever decodes best; raw signal keeps one reading, and the best of it is the mix.
  const [channel, setChannel] = useState(3);
  const shownChannel = !decode && channel === 3 ? 0 : channel;
  const [filter, setFilter] = useState(false);
  const go = () => {
    close();
    importRecording(d.side, d.name, d.request, { decode, channel: shownChannel, filter: audio && filter }, d.insertAtCursor);
  };
  const rate = s.format === 'OUT' ? 'T-states at 3.5 MHz' : `${fmtNum(s.rateHz)} Hz`;
  const highKhz = Math.min(4000, 0.45 * s.rateHz) / 1000;
  const kept = s.keptCsw ? 'a CSW block' : s.blocks === 1 ? 'a Direct Recording block' : `${s.blocks} Direct Recording blocks`;
  return (
    <Modal title="Import recording" onClose={close} width={500} cls="recording" footer={<><button class="primary" onClick={go}>Import</button><button onClick={close}>Cancel</button></>}>
      <div class="grid" style={{ gridTemplateColumns: 'auto 1fr' }}>
        <label>Recording</label>
        <div>{d.name}</div>
        <label>Format</label>
        <div>{s.format}, {rate}, {s.channels === 2 ? 'stereo' : 'mono'}</div>
        <label>Length</label>
        <div>{fmtTime(s.samples / s.rateHz)}</div>
        <label>Open as</label>
        <select value={decode ? 1 : 0} onChange={(e) => setDecode((e.target as HTMLSelectElement).value === '1')}>
          <option value={1}>Blocks, decoded</option>
          <option value={0}>Raw signal</option>
        </select>
        {audio && s.channels === 2 && <>
          <label>Channel</label>
          <select value={shownChannel} onChange={(e) => setChannel(Number((e.target as HTMLSelectElement).value))}>
            {decode && <option value={3}>Whichever reads best</option>}
            <option value={0}>Both, mixed</option>
            <option value={1}>Left</option>
            <option value={2}>Right</option>
          </select>
        </>}
        {audio && <>
          <label>Filter</label>
          <label><input type="checkbox" checked={filter} onChange={(e) => setFilter((e.target as HTMLInputElement).checked)} /> Band-pass, 300 Hz to {highKhz.toFixed(1)} kHz</label>
        </>}
        {!decode && <>
          <label>Kept as</label>
          <div>{kept}, a TZX file of {fmtNum(s.tzxSize)} bytes</div>
        </>}
      </div>
      {s.missingBytes > 0 && <p class="error">The file is {fmtNum(s.missingBytes)} bytes shorter than its header says; what it holds is kept.</p>}
      {s.skipped.length > 0 && <p class="note">Passed over: {s.skipped.join(', ')}.</p>}
      <p class="note">
        {decode
          ? "Standard blocks are read as the Spectrum's ROM reads them, in the background. A turbo or protected loader's blocks are kept as raw signal, which loads in an emulator as it is; hiss and noise between blocks become their pauses."
          : audio
            ? 'The recording is kept as it is, one bit per sample. It loads in an emulator whatever loader made it, but shows no headers or programs until it is decoded. Hiss can become stray pulses; the filter cleans it, and changes what is kept.'
            : 'The pulses are kept exactly as they are. The tape loads in an emulator whatever loader made it, but shows no headers or programs until it is decoded.'}
      </p>
    </Modal>
  );
}

function blockNumberList(blocks: Block[], indices: number[]) {
  return indices.map((i) => `#${blockNo(i)} ${describeBlock(blocks[i], false)}`);
}

function EmulatorDialog() {
  return (
    <Modal title="Emulator" onClose={close} width={520} footer={<button class="primary" onClick={close}>Close</button>}>
      <p class="note">
        A web page cannot start another program, so "Download … for emulator" in the Tape menu writes a
        TZX file for you to open in your emulator. The desktop app starts one itself.
      </p>
    </Modal>
  );
}
