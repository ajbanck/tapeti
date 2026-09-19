import { useState, useEffect, useMemo } from 'preact/hooks';
import { Side, tapes, replaceBlock, fmtNum, fmtByte, hex, locked, parseNum, showMessage, blockNo } from '../state/store';
import { convertBlock } from '../tzx/convert';
import {
  Block, CREATABLE_IDS, BLOCK_NAMES, isUnknown, ARCHIVE_TYPES, HARDWARE_TYPES, HARDWARE_INFO,
  SymDef, PilotRun, isDataBlock,
} from '../tzx/types';
import { decodeHeader, encodeHeader, checksum, HEADER_TYPE_NAMES, HeaderInfo } from '../tzx/describe';
import { NumInput, TextInput, Field } from './fields';
import { viewData } from '../state/actions';
import { decodePokes, encodePokes, pokesToText, textToPokes } from '../tzx/pokes';
import { TSTATES_PER_SEC, blockDuration } from '../tzx/audio';
import { detectContent, blockBody, ContentInfo } from '../tzx/content';
import { renderScreen } from '../spectrum/screen';
import { listBasic, basicToText } from '../spectrum/basic';
import { useRef } from 'preact/hooks';

function Preview({ info, data, compact }: { info: ContentInfo; data: Uint8Array; compact?: boolean }) {
  const body = useMemo(() => blockBody(data, info.skipFlag, info.skipChecksum), [data, info]);
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    if (info.kind !== 'screen' || !ref.current) return;
    const ctx = ref.current.getContext('2d')!;
    ctx.putImageData(new ImageData(renderScreen(body, 0, {}) as Uint8ClampedArray<ArrayBuffer>, 256, 192), 0, 0);
  }, [body, info.kind]);
  const listing = useMemo(() => {
    if (info.kind !== 'basic') return '';
    const lines = listBasic(body, 0, info.progLen ?? body.length, { showNumbers: false, basic128: false, speccyFormat: false });
    return basicToText(lines.slice(0, 40), { showNumbers: false, basic128: false, speccyFormat: false }) + (lines.length > 40 ? `\n… ${lines.length - 40} more line(s)` : '');
  }, [body, info]);
  const cls = 'preview' + (compact ? ' compact' : '') + (info.kind === 'screen' ? ' screen' : '');
  if (info.kind === 'screen') return <div class={cls}><canvas ref={ref} width={256} height={192} /></div>;
  if (info.kind === 'basic') return <div class={cls}><pre>{listing || '(no lines)'}</pre></div>;
  return null;
}

export function BlockEditor({ side, height }: { side: Side; height: number }) {
  const t = tapes[side].value;
  const block = t.blocks[t.cursor];
  if (!block) return <div class="editor" style={{ height }}><div class="empty">No block selected</div></div>;
  return <BlockForm key={block.uid} side={side} block={block} height={height} />;
}

function BlockForm({ side, block, height }: { side: Side; block: Block; height: number }) {
  const [draft, setDraft] = useState<Block>(block);
  const [errors, setErrors] = useState<string[]>([]);
  useEffect(() => { setDraft(block); setErrors([]); }, [block]);
  const dirty = draft !== block;
  const set = (p: Partial<any>) => setDraft({ ...(draft as any), ...p } as Block);
  const h = hex.value;
  const isLocked = locked.value;

  const commit = () => {
    if (errors.length) {
      showMessage('Cannot commit', errors);
      return;
    }
    replaceBlock(side, block.uid, draft);
  };

  const unknown = isUnknown(draft);
  const seconds = useMemo(() => (unknown ? 0 : blockDuration(draft) / TSTATES_PER_SEC), [draft, unknown]);
  return (
    <div class="editor" style={{ height }}>
      <div class="toprow">
        <label>Block type</label>
        <select
          value={unknown ? -1 : draft.id}
          disabled={unknown || isLocked}
          onChange={(e) => set(convertBlock(draft, Number((e.target as HTMLSelectElement).value)))}
        >
          {unknown && <option value={-1}>{BLOCK_NAMES[draft.id] ?? 'Unknown'} (ID {draft.id.toString(16).toUpperCase()})</option>}
          {CREATABLE_IDS.map((id) => <option key={id} value={id}>{BLOCK_NAMES[id]}</option>)}
        </select>
        {isDataBlock(draft) && <span class="note">Data blocks: edit the bytes with View data</span>}
      </div>
      <div class="body">
        <Fields draft={draft} set={set} side={side} setErrors={setErrors} h={h} disabled={isLocked} />
        {errors.length > 0 && <div class="error small-text">{errors.map((e, i) => <div key={i}>{e}</div>)}</div>}
      </div>
      <div class="footer">
        {'pause' in draft && (
          <>
            <label>Pause</label>
            <NumInput value={(draft as any).pause} max={0xffff} onChange={(v) => set({ pause: v })} width={70} disabled={isLocked} />
            <span>ms after this block</span>
          </>
        )}
        {seconds > 0 && (
          <span class="duration" title={'pause' in draft && (draft as any).pause > 0 ? 'Playing time of this block, including the pause' : 'Playing time of this block'}>
            Duration <b>{fmtDuration(seconds)}</b>
          </span>
        )}
        <span class="spacer" />
        <button class="primary" disabled={!dirty || isLocked} onClick={commit}>Commit</button>
        <button disabled={!dirty} onClick={() => { setDraft(block); setErrors([]); }}>Revert</button>
      </div>
    </div>
  );
}

/** "36.11 s", or "2:05.30" from a minute up. */
function fmtDuration(s: number): string {
  if (s < 60) return `${s.toFixed(2)} s`;
  const m = Math.floor(s / 60);
  return `${m}:${(s - m * 60).toFixed(2).padStart(5, '0')}`;
}

interface FP {
  draft: any;
  set: (p: any) => void;
  side: Side;
  setErrors: (e: string[]) => void;
  h: boolean;
  disabled: boolean;
}

function DataInfo({ draft, side }: FP) {
  const d: Uint8Array = draft.data;
  const t = tapes[side].value;
  const info = useMemo(() => detectContent(t.blocks.map((b) => (b.uid === draft.uid ? draft : b)), t.blocks.findIndex((b) => b.uid === draft.uid)), [t.blocks, draft]);
  const flag = d.length > 0 ? d[0] : null;
  const cs = d.length > 1 ? d[d.length - 1] : null;
  const expected = d.length > 1 ? checksum(d, 0, d.length - 1) : null;
  return (
    <div class="infocol">
      {info.skipFlag && info.skipChecksum && d.length >= 2 && [0x10, 0x11, 0x14, 0x19].includes(draft.id)
        ? <div title="As stored in the block; the header length counts only the data">Block length {fmtNum(d.length)} bytes: flag + {fmtNum(d.length - 2)} data + checksum</div>
        : <div>Data length {fmtNum(d.length)} bytes</div>}
      {flag !== null && <div>Flag byte {fmtByte(flag)}{flag === 0 ? ' (header)' : flag === 255 ? ' (data)' : ''}</div>}
      {cs !== null && <div>Checksum byte {fmtByte(cs)} {expected === cs ? <span class="chip ok">valid</span> : <span class="chip bad">expected {fmtByte(expected!)}</span>}</div>}
      {info.label && <div>Content <span class="chip">{info.label}</span> <span class="note">{info.source === 'header' ? 'from header' : info.source === 'heuristic' ? 'guessed' : ''}</span></div>}
      <div><button onClick={() => viewData(side)}>View data</button></div>
    </div>
  );
}

function PreviewFor(p: FP & { compact?: boolean }) {
  const t = tapes[p.side].value;
  const info = useMemo(() => detectContent(t.blocks.map((b) => (b.uid === p.draft.uid ? p.draft : b)), t.blocks.findIndex((b) => b.uid === p.draft.uid)), [t.blocks, p.draft]);
  return <Preview info={info} data={p.draft.data} compact={p.compact} />;
}

function HeaderEditor({ draft, set, h, disabled }: FP) {
  const hdr = decodeHeader(draft.data);
  if (!hdr) return null;
  const upd = (p: Partial<HeaderInfo>) => set({ data: encodeHeader({ ...hdr, ...p }) });
  // A header name is padded to 10 bytes; the padding is not the name. Editing
  // the padded string leaves no room under maxLength to type, so it comes off
  // here and encodeHeader puts it back.
  return (
    <div style={{ marginTop: 8 }}>
      <div class="note">Header</div>
      <div class="grid">
        <label>Type</label>
        <select value={hdr.type} disabled={disabled} onChange={(e) => upd({ type: Number((e.target as HTMLSelectElement).value) })}>
          {HEADER_TYPE_NAMES.map((n, i) => <option key={i} value={i}>{n}</option>)}
        </select>
        <label>Name</label>
        <TextInput value={hdr.name.replace(/ +$/, '')} maxLength={10} width={110} disabled={disabled} onChange={(s) => upd({ name: s })} />
        <label>Length</label>
        <NumInput value={hdr.length} max={0xffff} disabled={disabled} onChange={(v) => upd({ length: v })} />
        <label>{hdr.type === 0 ? 'Autostart line' : hdr.type === 3 ? 'Start address' : 'Variable name'}</label>
        <NumInput value={hdr.param1} max={0xffff} disabled={disabled} onChange={(v) => upd({ param1: v })} />
        <label>{hdr.type === 0 ? 'Program length' : 'Param 2'}</label>
        <NumInput value={hdr.param2} max={0xffff} disabled={disabled} onChange={(v) => upd({ param2: v })} />
        <span class="note">{hdr.type === 0 && hdr.param1 >= 32768 ? 'no autostart' : ''}</span>
      </div>
    </div>
  );
}

function Fields(p: FP) {
  const { draft, set, h, disabled } = p;
  if (isUnknown(draft)) {
    return (
      <div>
        <div>Raw body {fmtNum(draft.raw.length)} bytes. This block type is not editable; it will be written back unchanged.</div>
        <pre class="small-text">{Array.from(draft.raw.slice(0, 64)).map((b: number) => b.toString(16).padStart(2, '0')).join(' ')}{draft.raw.length > 64 ? ' …' : ''}</pre>
      </div>
    );
  }
  switch (draft.id) {
    case 0x10:
      return (
        <div>
          <div class="row-flex" style={{ alignItems: 'flex-start', gap: 30 }}>
            <div class="note" style={{ maxWidth: 150 }}>ROM timings<br />pilot 2168 × {draft.data[0] < 128 ? 8063 : 3223}<br />sync 667 / 735<br />bits 855 / 1710</div>
            <DataInfo {...p} />
            <PreviewFor {...p} />
          </div>
          <HeaderEditor {...p} />
        </div>
      );
    case 0x11:
      return (
        <div>
          <div class="row-flex" style={{ alignItems: 'flex-start', gap: 30 }}>
            <div class="grid">
              <Field label="Pilot pulse"><NumInput value={draft.pilot} max={0xffff} disabled={disabled} onChange={(v) => set({ pilot: v })} /></Field>
              <Field label="Pilot length"><NumInput value={draft.pilotLen} max={0xffff} disabled={disabled} onChange={(v) => set({ pilotLen: v })} /></Field>
              <Field label="Sync 1 pulse"><NumInput value={draft.sync1} max={0xffff} disabled={disabled} onChange={(v) => set({ sync1: v })} /></Field>
              <Field label="Sync 2 pulse"><NumInput value={draft.sync2} max={0xffff} disabled={disabled} onChange={(v) => set({ sync2: v })} /></Field>
              <Field label="Zero pulse"><NumInput value={draft.zero} max={0xffff} disabled={disabled} onChange={(v) => set({ zero: v })} /></Field>
              <Field label="One pulse"><NumInput value={draft.one} max={0xffff} disabled={disabled} onChange={(v) => set({ one: v })} /></Field>
              <Field label="Used bits (last byte)"><NumInput value={draft.usedBits} min={1} max={8} disabled={disabled} onChange={(v) => set({ usedBits: v })} /></Field>
              <div><button class="small" disabled={disabled} onClick={() => set({ pilot: 2168, sync1: 667, sync2: 735, zero: 855, one: 1710, pilotLen: draft.data[0] < 128 ? 8063 : 3223 })}>ROM timings</button></div>
            </div>
            <DataInfo {...p} />
            <PreviewFor {...p} />
          </div>
          <HeaderEditor {...p} />
        </div>
      );
    case 0x14:
      return (
        <div>
          <div class="row-flex" style={{ alignItems: 'flex-start', gap: 30 }}>
            <div class="grid">
              <Field label="Zero pulse"><NumInput value={draft.zero} max={0xffff} disabled={disabled} onChange={(v) => set({ zero: v })} /></Field>
              <Field label="One pulse"><NumInput value={draft.one} max={0xffff} disabled={disabled} onChange={(v) => set({ one: v })} /></Field>
              <Field label="Used bits (last byte)"><NumInput value={draft.usedBits} min={1} max={8} disabled={disabled} onChange={(v) => set({ usedBits: v })} /></Field>
            </div>
            <DataInfo {...p} />
            <PreviewFor {...p} />
          </div>
          <HeaderEditor {...p} />
        </div>
      );
    case 0x12:
      return (
        <div class="grid">
          <Field label="Pulse length"><NumInput value={draft.pulseLen} max={0xffff} disabled={disabled} onChange={(v) => set({ pulseLen: v })} /></Field>
          <Field label="Number of pulses"><NumInput value={draft.count} max={0xffff} disabled={disabled} onChange={(v) => set({ count: v })} /></Field>
          <span class="note" style={{ gridColumn: 'span 4' }}>Duration {((draft.pulseLen * draft.count) / TSTATES_PER_SEC * 1000).toFixed(1)} ms</span>
        </div>
      );
    case 0x13:
      return <NumberList label="Pulse lengths (max 255, separated by commas, semicolons or new lines)" values={draft.pulses} max={255} h={h} disabled={disabled} onChange={(v) => set({ pulses: v })} setErrors={p.setErrors} />;
    case 0x15:
      return (
        <div class="row-flex" style={{ alignItems: 'flex-start', gap: 30 }}>
          <div class="grid">
            <Field label="T-states per sample"><NumInput value={draft.tstates} min={1} max={0xffff} disabled={disabled} onChange={(v) => set({ tstates: v })} /></Field>
            <Field label="Sample rate"><SampleRate tstates={draft.tstates} disabled={disabled} onChange={(v) => set({ tstates: v })} /></Field>
            <Field label="Used bits (last byte)"><NumInput value={draft.usedBits} min={1} max={8} disabled={disabled} onChange={(v) => set({ usedBits: v })} /></Field>
            <span class="note" style={{ gridColumn: 'span 2' }}>{(((draft.data.length - 1) * 8 + draft.usedBits) * draft.tstates / TSTATES_PER_SEC).toFixed(2)} s</span>
          </div>
          <DataInfo {...p} />
          <PreviewFor {...p} />
        </div>
      );
    case 0x18:
      return (
        <div class="row-flex" style={{ alignItems: 'flex-start', gap: 30 }}>
          <div class="grid">
            <Field label="Sample rate"><NumInput value={draft.sampleRate} min={1} max={0xffffff} disabled={disabled} onChange={(v) => set({ sampleRate: v })} /></Field>
            <Field label="Compression">
              <select value={draft.compression} disabled={disabled} onChange={(e) => set({ compression: Number((e.target as HTMLSelectElement).value) })}>
                <option value={1}>RLE</option>
                <option value={2}>Z-RLE</option>
              </select>
            </Field>
            <Field label="Stored pulses"><NumInput value={draft.pulseCount} max={0xffffffff} disabled={disabled} onChange={(v) => set({ pulseCount: v })} /></Field>
          </div>
          <DataInfo {...p} />
        </div>
      );
    case 0x19:
      return <GeneralizedEditor {...p} />;
    case 0x20:
      return <div class="note">A pause of 0 ms means "stop the tape". Set the value in the Pause field below.</div>;
    case 0x21:
      return <div class="grid"><Field label="Group name"><TextInput value={draft.name} maxLength={255} width={300} disabled={disabled} onChange={(s) => set({ name: s })} /></Field></div>;
    case 0x22:
    case 0x25:
    case 0x27:
    case 0x2a:
      return <div class="note">This block has no parameters.</div>;
    case 0x23:
      return <OffsetField label="Relative offset" value={draft.offset} side={p.side} disabled={disabled} onChange={(v) => set({ offset: v })} />;
    case 0x24:
      return <div class="grid"><Field label="Repetitions"><NumInput value={draft.count} max={0xffff} disabled={disabled} onChange={(v) => set({ count: v })} /></Field></div>;
    case 0x26:
      return <NumberList label="Relative offsets of the called blocks, in order" values={draft.offsets} max={0xffff} min={-0x8000} h={h} disabled={disabled} onChange={(v) => set({ offsets: v })} setErrors={p.setErrors} side={p.side} />;
    case 0x28:
      return (
        <EntryList
          items={draft.entries}
          disabled={disabled}
          onChange={(entries) => set({ entries })}
          create={() => ({ offset: 1, text: '' })}
          render={(e: any, upd: (x: any) => void) => (
            <>
              <NumInput value={e.offset} min={-0x8000} max={0x7fff} width={60} disabled={disabled} onChange={(v) => upd({ ...e, offset: v })} />
              <TextInput value={e.text} maxLength={255} disabled={disabled} onChange={(s) => upd({ ...e, text: s })} />
            </>
          )}
        />
      );
    case 0x2b:
      return (
        <div class="grid">
          <Field label="Signal level">
            <select value={draft.level} disabled={disabled} onChange={(e) => set({ level: Number((e.target as HTMLSelectElement).value) })}>
              <option value={0}>Low</option>
              <option value={1}>High</option>
            </select>
          </Field>
        </div>
      );
    case 0x30:
      return <textarea class="wide" rows={5} maxLength={255} disabled={disabled} value={draft.text} onInput={(e) => set({ text: (e.target as HTMLTextAreaElement).value })} />;
    case 0x31:
      return (
        <div>
          <div class="grid"><Field label="Display time (s)"><NumInput value={draft.time} max={255} disabled={disabled} onChange={(v) => set({ time: v })} /></Field></div>
          <textarea class="wide" rows={4} maxLength={255} disabled={disabled} value={draft.text} onInput={(e) => set({ text: (e.target as HTMLTextAreaElement).value })} />
        </div>
      );
    case 0x32:
      return (
        <EntryList
          items={draft.entries}
          disabled={disabled}
          onChange={(entries) => set({ entries })}
          create={() => ({ type: 0xff, text: '' })}
          render={(e: any, upd: (x: any) => void) => (
            <>
              <select value={e.type} disabled={disabled} onChange={(ev) => upd({ ...e, type: Number((ev.target as HTMLSelectElement).value) })}>
                {Object.entries(ARCHIVE_TYPES).map(([k, v]) => <option key={k} value={Number(k)}>{v}</option>)}
              </select>
              <textarea rows={e.text.includes('\n') ? 3 : 1} maxLength={255} disabled={disabled} value={e.text} onInput={(ev) => upd({ ...e, text: (ev.target as HTMLTextAreaElement).value })} />
            </>
          )}
        />
      );
    case 0x33:
      return (
        <EntryList
          items={draft.entries}
          disabled={disabled}
          onChange={(entries) => set({ entries })}
          create={() => ({ type: 0, id: 1, info: 0 })}
          render={(e: any, upd: (x: any) => void) => (
            <>
              <select value={e.type} disabled={disabled} onChange={(ev) => upd({ ...e, type: Number((ev.target as HTMLSelectElement).value), id: 0 })}>
                {HARDWARE_TYPES.map((t, i) => <option key={i} value={i}>{t.name}</option>)}
              </select>
              <select value={e.id} disabled={disabled} onChange={(ev) => upd({ ...e, id: Number((ev.target as HTMLSelectElement).value) })}>
                {(HARDWARE_TYPES[e.type]?.ids ?? []).map((n, i) => <option key={i} value={i}>{n}</option>)}
                {!(HARDWARE_TYPES[e.type]?.ids ?? [])[e.id] && <option value={e.id}>Unknown ({e.id})</option>}
              </select>
              <select value={e.info} disabled={disabled} onChange={(ev) => upd({ ...e, info: Number((ev.target as HTMLSelectElement).value) })}>
                {HARDWARE_INFO.map((n, i) => <option key={i} value={i}>{n}</option>)}
              </select>
            </>
          )}
        />
      );
    case 0x35:
      return <CustomEditor {...p} />;
    case 0x5a:
      return (
        <div class="grid">
          <Field label="Major version"><NumInput value={draft.raw[7]} max={255} disabled={disabled} onChange={(v) => { const r = new Uint8Array(draft.raw); r[7] = v; set({ raw: r }); }} /></Field>
          <Field label="Minor version"><NumInput value={draft.raw[8]} max={255} disabled={disabled} onChange={(v) => { const r = new Uint8Array(draft.raw); r[8] = v; set({ raw: r }); }} /></Field>
        </div>
      );
  }
  return null;
}

function SampleRate({ tstates, onChange, disabled }: { tstates: number; onChange: (t: number) => void; disabled: boolean }) {
  // The only field that is always decimal, whatever the Dec/Hex switch says.
  const rate = Math.round(TSTATES_PER_SEC / Math.max(1, tstates));
  return (
    <input
      type="text" value={rate} style={{ width: 70 }} disabled={disabled}
      onChange={(e) => {
        const v = parseInt((e.target as HTMLInputElement).value, 10);
        if (v > 0) onChange(Math.max(1, Math.round(TSTATES_PER_SEC / v)));
      }}
    />
  );
}

function OffsetField({ label, value, side, onChange, disabled }: { label: string; value: number; side: Side; onChange: (v: number) => void; disabled: boolean }) {
  const t = tapes[side].value;
  const target = t.cursor + value;
  return (
    <div class="grid">
      <Field label={label}><NumInput value={value} min={-0x8000} max={0x7fff} disabled={disabled} onChange={onChange} /></Field>
      <span class="note" style={{ gridColumn: 'span 2' }}>{target >= 0 && target < t.blocks.length ? `→ block #${blockNo(target)}` : target === t.blocks.length ? '→ end of tape' : 'outside the tape!'}</span>
    </div>
  );
}

function NumberList({ label, values, min = 0, max, h, onChange, setErrors, disabled, side }: { label: string; values: number[]; min?: number; max: number; h: boolean; onChange: (v: number[]) => void; setErrors: (e: string[]) => void; disabled: boolean; side?: Side }) {
  const [text, setText] = useState(values.map((v) => fmtNum(v)).join(', '));
  useEffect(() => setText(values.map((v) => fmtNum(v)).join(', ')), [values, h]);
  const t = side !== undefined ? tapes[side].value : null;
  return (
    <div>
      <div class="note">{label}</div>
      <textarea
        class="wide" rows={5} value={text} disabled={disabled}
        onInput={(e) => {
          const s = (e.target as HTMLTextAreaElement).value;
          setText(s);
          const parts = s.split(/[\s,;]+/).filter((x) => x !== '');
          const nums = parts.map((x) => parseNum(x));
          const bad = parts.filter((p, i) => Number.isNaN(nums[i]) || nums[i] < min || nums[i] > max);
          if (bad.length) setErrors([`Invalid values: ${bad.join(', ')}`]);
          else {
            setErrors([]);
            onChange(nums);
          }
        }}
      />
      <div class="note">{values.length} value(s){t ? ' → ' + values.map((o) => (t.cursor + o >= 0 && t.cursor + o < t.blocks.length ? `#${blockNo(t.cursor + o)}` : '?')).join(', ') : ''}</div>
    </div>
  );
}

function EntryList<T>({ items, onChange, create, render, disabled }: { items: T[]; onChange: (v: T[]) => void; create: () => T; render: (e: T, upd: (x: T) => void) => any; disabled: boolean }) {
  return (
    <div class="entries">
      {items.map((e, i) => (
        <div class="entry" key={i}>
          {render(e, (x) => onChange(items.map((y, k) => (k === i ? x : y))))}
          <button class="small" title="Remove" disabled={disabled} onClick={() => onChange(items.filter((_, k) => k !== i))}>−</button>
          <button class="small" title="Move up" disabled={disabled || i === 0} onClick={() => { const a = items.slice(); [a[i - 1], a[i]] = [a[i], a[i - 1]]; onChange(a); }}>↑</button>
        </div>
      ))}
      <div><button class="small" disabled={disabled || items.length >= 255} onClick={() => onChange([...items, create()])}>+ Add</button></div>
    </div>
  );
}

// ---- Generalized data block -------------------------------------------------

function symbolsToText(syms: SymDef[], h: boolean): string {
  return syms.map((s, i) => `${fmtNum(i)}: ${fmtNum(s.flags)}; ${s.pulses.map((v) => fmtNum(v)).join(', ')}`).join('\n');
}
function textToSymbols(text: string): SymDef[] {
  const out: SymDef[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    const body = line.includes(':') ? line.slice(line.indexOf(':') + 1) : line;
    const parts = body.split(/[,;]/).map((x) => x.trim()).filter((x) => x !== '');
    if (parts.length === 0) throw new Error(`Symbol line "${raw}" has no flags`);
    const nums = parts.map((x) => parseNum(x));
    if (nums.some(Number.isNaN)) throw new Error(`Symbol line "${raw}" contains a bad number`);
    if (nums[0] < 0 || nums[0] > 3) throw new Error(`Symbol flags must be 0-3 in "${raw}"`);
    out.push({ flags: nums[0], pulses: nums.slice(1) });
  }
  if (out.length > 256) throw new Error('At most 256 symbols are allowed');
  return out;
}
function streamToText(runs: PilotRun[], h: boolean): string {
  return runs.map((r) => `${fmtNum(r.symbol)}, ${fmtNum(r.reps)}`).join('; ');
}
function textToStream(text: string): PilotRun[] {
  const nums = text.split(/[\s,;]+/).filter((x) => x !== '').map((x) => parseNum(x));
  if (nums.some(Number.isNaN)) throw new Error('Pilot stream contains a bad number');
  if (nums.length % 2) throw new Error('Pilot stream must be pairs of symbol, repetitions');
  const out: PilotRun[] = [];
  for (let i = 0; i < nums.length; i += 2) out.push({ symbol: nums[i], reps: nums[i + 1] });
  return out;
}

function GeneralizedEditor(p: FP) {
  const { draft, set, h, disabled, setErrors } = p;
  const [pt, setPt] = useState(symbolsToText(draft.pilotSymbols, h));
  const [ps, setPs] = useState(streamToText(draft.pilotStream, h));
  const [dt, setDt] = useState(symbolsToText(draft.dataSymbols, h));
  useEffect(() => { setPt(symbolsToText(draft.pilotSymbols, h)); setPs(streamToText(draft.pilotStream, h)); setDt(symbolsToText(draft.dataSymbols, h)); }, [h, draft.uid]);
  const apply = (fn: () => any) => {
    try {
      set(fn());
      setErrors([]);
    } catch (e) {
      setErrors([(e as Error).message]);
    }
  };
  const npp = Math.max(1, ...draft.pilotSymbols.map((s: SymDef) => s.pulses.length));
  const npd = Math.max(1, ...draft.dataSymbols.map((s: SymDef) => s.pulses.length));
  return (
    <div class="gen-grid">
      <div style={{ minWidth: 0 }}>
        <div class="note">Pilot/sync symbol table — [code]: flags; pulse1, pulse2, …</div>
        <textarea class="wide" rows={4} value={pt} disabled={disabled} onInput={(e) => { const s = (e.target as HTMLTextAreaElement).value; setPt(s); apply(() => { const syms = textToSymbols(s); return { pilotSymbols: syms, npp: Math.max(1, ...syms.map((x) => x.pulses.length)) }; }); }} />
        <div class="note">Pilot/sync stream — code, repetitions; …</div>
        <textarea class="wide" rows={2} value={ps} disabled={disabled} onInput={(e) => { const s = (e.target as HTMLTextAreaElement).value; setPs(s); apply(() => { const runs = textToStream(s); return { pilotStream: runs, totp: runs.length }; }); }} />
        <div class="note">Data symbol table — [code]: flags; pulse1, pulse2, …</div>
        <textarea class="wide" rows={4} value={dt} disabled={disabled} onInput={(e) => { const s = (e.target as HTMLTextAreaElement).value; setDt(s); apply(() => { const syms = textToSymbols(s); return { dataSymbols: syms, npd: Math.max(1, ...syms.map((x) => x.pulses.length)) }; }); }} />
      </div>
      <div class="infocol">
        <div>NPP {fmtNum(npp)}, ASP {fmtNum(draft.pilotSymbols.length)}, TOTP {fmtNum(draft.totp)}</div>
        <div>NPD {fmtNum(npd)}, ASD {fmtNum(draft.dataSymbols.length)}</div>
        <div class="grid" style={{ gridTemplateColumns: 'auto auto' }}>
          <Field label="TOTD (symbols)"><NumInput value={draft.totd} max={0xffffffff} disabled={disabled} onChange={(v) => set({ totd: v })} /></Field>
        </div>
        <DataInfo {...p} />
        <div class="note">Flags: 0 = edge, 1 = no edge, 2 = force low, 3 = force high</div>
      </div>
      <PreviewFor {...p} compact />
    </div>
  );
}

// ---- Custom info -------------------------------------------------------------

function CustomEditor(p: FP) {
  const { draft, set, h, disabled, setErrors } = p;
  const isPokes = /^POKEs/.test(draft.ident);
  const pokesText = useMemo(() => {
    if (!isPokes) return '';
    try {
      return pokesToText(decodePokes(draft.data), h);
    } catch {
      return null;
    }
  }, [draft.uid, h, isPokes]);
  const [text, setText] = useState(pokesText ?? '');
  useEffect(() => setText(pokesText ?? ''), [pokesText]);
  return (
    <div>
      <div class="grid" style={{ gridTemplateColumns: 'auto auto auto' }}>
        <label>Identification</label>
        <TextInput value={draft.ident.replace(/\s+$/, '')} maxLength={16} width={160} disabled={disabled} onChange={(s) => set({ ident: (s + '                ').slice(0, 16) })} />
        <select value="" disabled={disabled} onChange={(e) => { const v = (e.target as HTMLSelectElement).value; if (v) set({ ident: (v + '                ').slice(0, 16) }); }}>
          <option value="">standard…</option>
          {['POKEs', 'Instructions', 'Screen', 'ZX-Edit document', 'Picture', 'Custom'].map((n) => <option key={n} value={n}>{n}</option>)}
        </select>
      </div>
      <div class="row-flex"><span>Data length {fmtNum(draft.data.length)} bytes</span><button onClick={() => viewData(p.side)}>View data</button></div>
      {isPokes && (
        <div>
          <div class="note">POKEs — one per line: [POKE] [page:]adr,val[/orgval]; use ? as value to ask the user. Lines starting with ; are descriptions, [name] starts a trainer.</div>
          {pokesText === null && <div class="error">Existing POKEs data could not be decoded; editing will replace it.</div>}
          <textarea class="wide" rows={6} value={text} disabled={disabled} onInput={(e) => {
            const s = (e.target as HTMLTextAreaElement).value;
            setText(s);
            try {
              set({ data: encodePokes(textToPokes(s, h)) });
              setErrors([]);
            } catch (err) {
              setErrors([(err as Error).message]);
            }
          }} />
        </div>
      )}
      {!isPokes && /^(Instructions|Screen|ZX-Edit|Picture)/.test(draft.ident) && <div class="note">Standardized custom block; edit the bytes with View data.</div>}
    </div>
  );
}
