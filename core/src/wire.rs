//! The binary format the wasm build hands parsed tapes to JavaScript in.
//!
//! `src/tzx/wire.ts` decodes it. Both ends are hand written, so the format is
//! deliberately dull: little-endian, fixed widths that match the block model,
//! no alignment and no compression. Fields appear in the order the TypeScript
//! object literals in `parser.ts` declare them, so the decoded objects come out
//! with the same key order as before.
//!
//! ```text
//! u8  WIRE_VERSION
//! u8  status        0 ok, 1 error
//! error:  str message
//! ok:     u8 major, u8 minor
//!         u32 count, then that many str   (warnings)
//!         u32 count, then that many block:
//!           u8 tag        the block ID, or UNKNOWN_TAG
//!           UNKNOWN_TAG:  u8 id, bytes raw
//!           otherwise:    the fields of that block type, in declaration order
//! ```
//!
//! `str` and `bytes` are both a u32 length and that many bytes; `str` holds the
//! Latin-1 bytes of the TZX text, which the decoder turns back into a JS string
//! one char per byte.

use crate::bytes::{ReadResult, Reader};
use crate::parser::ParsedTape;
use crate::types::{ArchiveEntry, Block, Body, HardwareEntry, PilotRun, SelectEntry, SymDef};

pub const WIRE_VERSION: u8 = 1;
const UNKNOWN_TAG: u8 = 0xff;

pub fn encode_tape(t: &ParsedTape) -> Vec<u8> {
    let mut w = Vec::with_capacity(4096);
    w.push(WIRE_VERSION);
    w.push(0); // ok
    w.push(t.major);
    w.push(t.minor);
    u32v(&mut w, t.warnings.len());
    for warning in &t.warnings {
        string(&mut w, warning);
    }
    encode_blocks_into(&mut w, &t.blocks);
    w
}

/// A bare block list, as the app sends one in for writing or measuring.
pub fn encode_blocks(blocks: &[Block]) -> Vec<u8> {
    let mut w = Vec::with_capacity(4096);
    encode_blocks_into(&mut w, blocks);
    w
}

fn encode_blocks_into(w: &mut Vec<u8>, blocks: &[Block]) {
    u32v(w, blocks.len());
    for b in blocks {
        block(w, b);
    }
}

/// The inverse of [`encode_blocks`], for payloads arriving from JavaScript.
pub fn decode_blocks(buf: &[u8]) -> ReadResult<Vec<Block>> {
    let mut r = Reader::new(buf);
    let count = r.u32()? as usize;
    let mut blocks = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        blocks.push(Block::new(read_block(&mut r)?));
    }
    Ok(blocks)
}

/// A byte string answer, such as a serialized tape.
pub fn encode_bytes(b: &[u8]) -> Vec<u8> {
    let mut w = Vec::with_capacity(b.len() + 8);
    w.push(WIRE_VERSION);
    w.push(0);
    bytes(&mut w, b);
    w
}

/// A TAP answer: the bytes plus the indices of the blocks left out.
pub fn encode_tap(b: &[u8], skipped: &[u32]) -> Vec<u8> {
    let mut w = encode_bytes(b);
    u32v(&mut w, skipped.len());
    for i in skipped {
        u32v(&mut w, *i as usize);
    }
    w
}

/// A TZX version answer.
pub fn encode_version(major: u8, minor: u8) -> Vec<u8> {
    vec![WIRE_VERSION, 0, major, minor]
}

fn read_block(r: &mut Reader) -> ReadResult<Body> {
    let tag = r.u8()?;
    if tag == UNKNOWN_TAG {
        let id = r.u8()?;
        return Ok(Body::Unknown { id, raw: read_bytes(r)? });
    }
    Ok(match tag {
        0x10 => Body::Standard { pause: r.u16()?, data: read_bytes(r)? },
        0x11 => Body::Turbo {
            pilot: r.u16()?,
            sync1: r.u16()?,
            sync2: r.u16()?,
            zero: r.u16()?,
            one: r.u16()?,
            pilot_len: r.u16()?,
            used_bits: r.u8()?,
            pause: r.u16()?,
            data: read_bytes(r)?,
        },
        0x12 => Body::PureTone { pulse_len: r.u16()?, count: r.u16()? },
        0x13 => Body::PulseSeq { pulses: read_u16s(r)? },
        0x14 => Body::PureData {
            zero: r.u16()?,
            one: r.u16()?,
            used_bits: r.u8()?,
            pause: r.u16()?,
            data: read_bytes(r)?,
        },
        0x15 => Body::Direct { tstates: r.u16()?, pause: r.u16()?, used_bits: r.u8()?, data: read_bytes(r)? },
        0x18 => Body::Csw {
            pause: r.u16()?,
            sample_rate: r.u32()?,
            compression: r.u8()?,
            pulse_count: r.u32()?,
            data: read_bytes(r)?,
        },
        0x19 => {
            let pause = r.u16()?;
            let totp = r.u32()?;
            let npp = r.u8()?;
            let pilot_symbols = read_sym_defs(r)?;
            let mut pilot_stream = Vec::new();
            for _ in 0..r.u32()? {
                pilot_stream.push(PilotRun { symbol: r.u8()?, reps: r.u16()? });
            }
            Body::Generalized {
                pause,
                totp,
                npp,
                pilot_symbols,
                pilot_stream,
                totd: r.u32()?,
                npd: r.u8()?,
                data_symbols: read_sym_defs(r)?,
                data: read_bytes(r)?,
            }
        }
        0x20 => Body::Pause { pause: r.u16()? },
        0x21 => Body::GroupStart { name: read_str(r)? },
        0x22 => Body::GroupEnd,
        0x23 => Body::Jump { offset: r.i16()? },
        0x24 => Body::LoopStart { count: r.u16()? },
        0x25 => Body::LoopEnd,
        0x26 => {
            let mut offsets = Vec::new();
            for _ in 0..r.u32()? {
                offsets.push(r.i16()?);
            }
            Body::Call { offsets }
        }
        0x27 => Body::Return,
        0x28 => {
            let mut entries = Vec::new();
            for _ in 0..r.u32()? {
                entries.push(SelectEntry { offset: r.i16()?, text: read_str(r)? });
            }
            Body::Select { entries }
        }
        0x2a => Body::Stop48,
        0x2b => Body::SignalLevel { level: r.u8()? },
        0x30 => Body::Text { text: read_str(r)? },
        0x31 => Body::Message { time: r.u8()?, text: read_str(r)? },
        0x32 => {
            let mut entries = Vec::new();
            for _ in 0..r.u32()? {
                entries.push(ArchiveEntry { kind: r.u8()?, text: read_str(r)? });
            }
            Body::Archive { entries }
        }
        0x33 => {
            let mut entries = Vec::new();
            for _ in 0..r.u32()? {
                entries.push(HardwareEntry { kind: r.u8()?, id: r.u8()?, info: r.u8()? });
            }
            Body::Hardware { entries }
        }
        0x35 => Body::Custom { ident: read_str(r)?, data: read_bytes(r)? },
        0x5a => Body::Glue { raw: read_bytes(r)? },
        other => {
            return Err(crate::bytes::ReadError(format!(
                "Wire payload has block tag {other:02x}, which the core does not know"
            )))
        }
    })
}

fn read_bytes(r: &mut Reader) -> ReadResult<Vec<u8>> {
    let n = r.u32()? as usize;
    r.bytes(n)
}

fn read_str(r: &mut Reader) -> ReadResult<String> {
    let n = r.u32()? as usize;
    Ok(String::from_utf8_lossy(&r.bytes(n)?).into_owned())
}

fn read_u16s(r: &mut Reader) -> ReadResult<Vec<u16>> {
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        out.push(r.u16()?);
    }
    Ok(out)
}

fn read_sym_defs(r: &mut Reader) -> ReadResult<Vec<SymDef>> {
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        out.push(SymDef { flags: r.u8()?, pulses: read_u16s(r)? });
    }
    Ok(out)
}

pub fn encode_error(message: &str) -> Vec<u8> {
    let mut w = Vec::with_capacity(message.len() + 8);
    w.push(WIRE_VERSION);
    w.push(1); // error
    string(&mut w, message);
    w
}

fn block(w: &mut Vec<u8>, b: &Block) {
    if let Body::Unknown { id, raw } = &b.body {
        w.push(UNKNOWN_TAG);
        w.push(*id);
        bytes(w, raw);
        return;
    }
    w.push(b.id());
    match &b.body {
        Body::Standard { pause, data } => {
            u16v(w, *pause);
            bytes(w, data);
        }
        Body::Turbo { pilot, sync1, sync2, zero, one, pilot_len, used_bits, pause, data } => {
            for v in [*pilot, *sync1, *sync2, *zero, *one, *pilot_len] {
                u16v(w, v);
            }
            w.push(*used_bits);
            u16v(w, *pause);
            bytes(w, data);
        }
        Body::PureTone { pulse_len, count } => {
            u16v(w, *pulse_len);
            u16v(w, *count);
        }
        Body::PulseSeq { pulses } => u16s(w, pulses),
        Body::PureData { zero, one, used_bits, pause, data } => {
            u16v(w, *zero);
            u16v(w, *one);
            w.push(*used_bits);
            u16v(w, *pause);
            bytes(w, data);
        }
        Body::Direct { tstates, pause, used_bits, data } => {
            u16v(w, *tstates);
            u16v(w, *pause);
            w.push(*used_bits);
            bytes(w, data);
        }
        Body::Csw { pause, sample_rate, compression, pulse_count, data } => {
            u16v(w, *pause);
            u32v(w, *sample_rate as usize);
            w.push(*compression);
            u32v(w, *pulse_count as usize);
            bytes(w, data);
        }
        Body::Generalized {
            pause,
            totp,
            npp,
            pilot_symbols,
            pilot_stream,
            totd,
            npd,
            data_symbols,
            data,
        } => {
            u16v(w, *pause);
            u32v(w, *totp as usize);
            w.push(*npp);
            sym_defs(w, pilot_symbols);
            u32v(w, pilot_stream.len());
            for run in pilot_stream {
                w.push(run.symbol);
                u16v(w, run.reps);
            }
            u32v(w, *totd as usize);
            w.push(*npd);
            sym_defs(w, data_symbols);
            bytes(w, data);
        }
        Body::Pause { pause } => u16v(w, *pause),
        Body::GroupStart { name } => string(w, name),
        Body::GroupEnd => {}
        Body::Jump { offset } => u16v(w, *offset as u16),
        Body::LoopStart { count } => u16v(w, *count),
        Body::LoopEnd => {}
        Body::Call { offsets } => {
            u32v(w, offsets.len());
            for o in offsets {
                u16v(w, *o as u16);
            }
        }
        Body::Return => {}
        Body::Select { entries } => {
            u32v(w, entries.len());
            for e in entries {
                u16v(w, e.offset as u16);
                string(w, &e.text);
            }
        }
        Body::Stop48 => {}
        Body::SignalLevel { level } => w.push(*level),
        Body::Text { text } => string(w, text),
        Body::Message { time, text } => {
            w.push(*time);
            string(w, text);
        }
        Body::Archive { entries } => {
            u32v(w, entries.len());
            for e in entries {
                w.push(e.kind);
                string(w, &e.text);
            }
        }
        Body::Hardware { entries } => {
            u32v(w, entries.len());
            for e in entries {
                w.push(e.kind);
                w.push(e.id);
                w.push(e.info);
            }
        }
        Body::Custom { ident, data } => {
            string(w, ident);
            bytes(w, data);
        }
        Body::Glue { raw } => bytes(w, raw),
        Body::Unknown { .. } => unreachable!("handled above"),
    }
}

fn sym_defs(w: &mut Vec<u8>, defs: &[SymDef]) {
    u32v(w, defs.len());
    for d in defs {
        w.push(d.flags);
        u16s(w, &d.pulses);
    }
}

fn u16v(w: &mut Vec<u8>, v: u16) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn u32v(w: &mut Vec<u8>, v: usize) {
    w.extend_from_slice(&(v as u32).to_le_bytes());
}

fn u16s(w: &mut Vec<u8>, v: &[u16]) {
    u32v(w, v.len());
    for n in v {
        u16v(w, *n);
    }
}

fn bytes(w: &mut Vec<u8>, b: &[u8]) {
    u32v(w, b.len());
    w.extend_from_slice(b);
}

/// Strings travel as UTF-8: most of them are Latin-1 tape text, but the
/// character table is full of block glyphs, and one encoding for all of them is
/// one less thing to get wrong.
fn string(w: &mut Vec<u8>, s: &str) {
    bytes(w, s.as_bytes());
}

// ---- answers for the description, content, consistency and program calls ----

/// `[u32 count]` then that many strings.
pub fn encode_strings(items: &[String]) -> Vec<u8> {
    let mut w = header();
    u32v(&mut w, items.len());
    for s in items {
        string(&mut w, s);
    }
    w
}

/// An optional string: `[u8 present]` and, when present, the string.
pub fn encode_opt_string(value: Option<&str>) -> Vec<u8> {
    let mut w = header();
    match value {
        Some(s) => {
            w.push(1);
            string(&mut w, s);
        }
        None => w.push(0),
    }
    w
}

/// A block's list description and its length column.
pub fn encode_described(description: &str, length: u32) -> Vec<u8> {
    let mut w = header();
    string(&mut w, description);
    u32v(&mut w, length as usize);
    w
}

/// A single byte answer, such as a checksum.
pub fn encode_u8(v: u8) -> Vec<u8> {
    vec![WIRE_VERSION, 0, v]
}

/// A double, such as the BASIC score.
pub fn encode_f64(v: f64) -> Vec<u8> {
    let mut w = header();
    w.extend_from_slice(&v.to_le_bytes());
    w
}

/// `[u32 count]` then `[u32 start][u32 end]` per range.
pub fn encode_ranges(ranges: &[(u32, u32)]) -> Vec<u8> {
    let mut w = header();
    u32v(&mut w, ranges.len());
    for (start, end) in ranges {
        u32v(&mut w, *start as usize);
        u32v(&mut w, *end as usize);
    }
    w
}

pub fn encode_content(c: &crate::content::ContentInfo) -> Vec<u8> {
    let mut w = header();
    string(&mut w, c.kind.name());
    string(&mut w, &c.label);
    u16v(&mut w, c.base);
    w.push(c.skip_flag as u8);
    w.push(c.skip_checksum as u8);
    opt_u16(&mut w, c.prog_len);
    match &c.header {
        Some(h) => {
            w.push(1);
            put_header(&mut w, h);
        }
        None => w.push(0),
    }
    string(&mut w, c.source.name());
    opt_u16(&mut w, c.expected_length);
    w
}

/// An optional ROM header, as `decodeHeader` returns one.
pub fn encode_header_info(h: Option<&crate::describe::HeaderInfo>) -> Vec<u8> {
    let mut w = header();
    match h {
        Some(h) => {
            w.push(1);
            put_header(&mut w, h);
        }
        None => w.push(0),
    }
    w
}

/// `[u32 count]` then `[i32 block][str severity][str message]` per issue.
pub fn encode_issues(issues: &[crate::consistency::Issue]) -> Vec<u8> {
    let mut w = header();
    u32v(&mut w, issues.len());
    for i in issues {
        w.extend_from_slice(&i.block.to_le_bytes());
        string(&mut w, i.severity.name());
        string(&mut w, &i.message);
    }
    w
}

/// `[u32 count]` then `[str name][u32 start][u32 end][str source]` per program.
pub fn encode_programs(programs: &[crate::programs::Program]) -> Vec<u8> {
    let mut w = header();
    u32v(&mut w, programs.len());
    for p in programs {
        string(&mut w, &p.name);
        u32v(&mut w, p.start as usize);
        u32v(&mut w, p.end as usize);
        string(&mut w, p.source.name());
    }
    w
}

/// A ROM header arriving from JavaScript, for `encode_header`.
pub fn decode_header_info(buf: &[u8]) -> ReadResult<crate::describe::HeaderInfo> {
    let mut r = Reader::new(buf);
    let kind = r.u8()?;
    let name = read_str(&mut r)?;
    Ok(crate::describe::HeaderInfo {
        kind,
        type_name: crate::describe::HEADER_TYPE_NAMES.get(kind as usize).unwrap_or(&"").to_string(),
        name,
        length: r.u16()?,
        param1: r.u16()?,
        param2: r.u16()?,
    })
}

fn header() -> Vec<u8> {
    vec![WIRE_VERSION, 0]
}

fn put_header(w: &mut Vec<u8>, h: &crate::describe::HeaderInfo) {
    w.push(h.kind);
    string(w, &h.type_name);
    string(w, &h.name);
    u16v(w, h.length);
    u16v(w, h.param1);
    u16v(w, h.param2);
}

fn opt_u32(w: &mut Vec<u8>, v: Option<u32>) {
    match v {
        Some(n) => {
            w.push(1);
            u32v(w, n as usize);
        }
        None => {
            w.push(0);
            u32v(w, 0);
        }
    }
}

fn opt_u16(w: &mut Vec<u8>, v: Option<u16>) {
    match v {
        Some(n) => {
            w.push(1);
            u16v(w, n);
        }
        None => w.push(0),
    }
}

/// A block list as an answer, for the calls that hand blocks back.
pub fn encode_blocks_answer(blocks: &[Block]) -> Vec<u8> {
    let mut w = header();
    encode_blocks_into(&mut w, blocks);
    w
}

/// `[u32 count]` then that many `u32`s.
pub fn encode_u32s(values: &[u32]) -> Vec<u8> {
    let mut w = header();
    u32v(&mut w, values.len());
    for v in values {
        u32v(&mut w, *v as usize);
    }
    w
}

/// Two result lists and whether the tapes matched.
pub fn encode_comparison(
    left: &[crate::compare::CompareResult],
    right: &[crate::compare::CompareResult],
    identical: bool,
) -> Vec<u8> {
    let mut w = header();
    for side in [left, right] {
        u32v(&mut w, side.len());
        for r in side {
            w.push(match r {
                crate::compare::CompareResult::Same => 0,
                crate::compare::CompareResult::Diff => 1,
                crate::compare::CompareResult::Ignored => 2,
            });
        }
    }
    w.push(identical as u8);
    w
}

/// A bit stream: its bytes and how many bits of the last one are used.
pub fn encode_bit_data(d: &crate::bits::BitData) -> Vec<u8> {
    let mut w = header();
    bytes(&mut w, &d.data);
    w.push(d.used_bits);
    w
}

/// One or more bit streams arriving from JavaScript.
pub fn decode_bit_data(buf: &[u8]) -> ReadResult<Vec<crate::bits::BitData>> {
    let mut r = Reader::new(buf);
    let count = r.u32()? as usize;
    let mut out = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let data = read_bytes(&mut r)?;
        out.push(crate::bits::BitData { data, used_bits: r.u8()? });
    }
    Ok(out)
}

pub fn encode_pokes_info(info: &crate::pokes::PokesInfo) -> Vec<u8> {
    let mut w = header();
    string(&mut w, &info.description);
    u32v(&mut w, info.trainers.len());
    for t in &info.trainers {
        string(&mut w, &t.description);
        u32v(&mut w, t.pokes.len());
        for p in &t.pokes {
            opt_u32(&mut w, p.page);
            u32v(&mut w, p.addr as usize);
            opt_u32(&mut w, p.value);
            opt_u32(&mut w, p.original);
        }
    }
    w
}

pub fn decode_pokes_info(buf: &[u8]) -> ReadResult<crate::pokes::PokesInfo> {
    let mut r = Reader::new(buf);
    let description = read_str(&mut r)?;
    let mut trainers = Vec::new();
    for _ in 0..r.u32()? {
        let description = read_str(&mut r)?;
        let mut pokes = Vec::new();
        for _ in 0..r.u32()? {
            let opt = |r: &mut Reader| -> ReadResult<Option<u32>> {
                let present = r.u8()? == 1;
                let value = r.u32()?;
                Ok(if present { Some(value) } else { None })
            };
            let page = opt(&mut r)?;
            let addr = r.u32()?;
            let value = opt(&mut r)?;
            let original = opt(&mut r)?;
            pokes.push(crate::pokes::Poke { page, addr, value, original });
        }
        trainers.push(crate::pokes::Trainer { description, pokes });
    }
    Ok(crate::pokes::PokesInfo { description, trainers })
}

// ---- the Spectrum side: BASIC listings, variables and disassembly ----------

/// `[u32 count]` then `[u32 number][u32 length][u32 offset][u8 hasError][str error]`
/// and the line's tokens, each `[str text][str kind]`.
pub fn encode_basic_lines(lines: &[crate::spectrum::basic::BasicLine]) -> Vec<u8> {
    let mut w = header();
    u32v(&mut w, lines.len());
    for l in lines {
        u32v(&mut w, l.number as usize);
        u32v(&mut w, l.length as usize);
        u32v(&mut w, l.offset as usize);
        match &l.error {
            Some(e) => {
                w.push(1);
                string(&mut w, e);
            }
            None => w.push(0),
        }
        u32v(&mut w, l.tokens.len());
        for t in &l.tokens {
            string(&mut w, &t.text);
            string(&mut w, t.kind.name());
        }
    }
    w
}

/// The same shape without the answer header, as it arrives from JavaScript.
pub fn decode_basic_lines(buf: &[u8]) -> ReadResult<Vec<crate::spectrum::basic::BasicLine>> {
    use crate::spectrum::basic::{BasicLine, BasicToken, TokenKind};
    let mut r = Reader::new(buf);
    let count = r.u32()? as usize;
    let mut lines = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        let number = r.u32()?;
        let length = r.u32()?;
        let offset = r.u32()?;
        let error = if r.u8()? == 1 { Some(read_str(&mut r)?) } else { None };
        let mut tokens = Vec::new();
        for _ in 0..r.u32()? {
            let text = read_str(&mut r)?;
            tokens.push(BasicToken { text, kind: TokenKind::from_name(&read_str(&mut r)?) });
        }
        lines.push(BasicLine { number, length, offset, tokens, error });
    }
    Ok(lines)
}

/// `[u32 count]` then `[str name][str type][str value][u32 offset][u32 size]`.
pub fn encode_variables(vars: &[crate::spectrum::basic::VariableEntry]) -> Vec<u8> {
    let mut w = header();
    u32v(&mut w, vars.len());
    for v in vars {
        string(&mut w, &v.name);
        string(&mut w, &v.kind);
        string(&mut w, &v.value);
        u32v(&mut w, v.offset as usize);
        u32v(&mut w, v.size as usize);
    }
    w
}

/// `[u32 count]` then `[u32 addr][bytes][str text][u8 hasTarget][u32 target]`.
pub fn encode_dis_lines(lines: &[crate::spectrum::z80dis::DisLine]) -> Vec<u8> {
    let mut w = header();
    u32v(&mut w, lines.len());
    for l in lines {
        u32v(&mut w, l.addr as usize);
        bytes(&mut w, &l.bytes);
        string(&mut w, &l.text);
        match l.target {
            Some(t) => {
                w.push(1);
                u32v(&mut w, t as usize);
            }
            None => w.push(0),
        }
    }
    w
}

// ---- audio ----------------------------------------------------------------

/// A block list followed by a playback order, as the render calls send one.
pub fn decode_blocks_and_order(buf: &[u8]) -> ReadResult<(Vec<Block>, Vec<u32>)> {
    let mut r = Reader::new(buf);
    let count = r.u32()? as usize;
    let mut blocks = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        blocks.push(Block::new(read_block(&mut r)?));
    }
    let n = r.u32()? as usize;
    let mut order = Vec::with_capacity(n.min(1 << 20));
    for _ in 0..n {
        order.push(r.u32()?);
    }
    Ok((blocks, order))
}

/// `[u32 count]` then that many doubles. T-state totals outgrow 32 bits on a
/// long tape, and JavaScript counts in doubles anyway.
pub fn encode_f64s(values: &[f64]) -> Vec<u8> {
    let mut w = header();
    u32v(&mut w, values.len());
    for v in values {
        w.extend_from_slice(&v.to_le_bytes());
    }
    w
}

/// The playback timeline: every start, then the total.
pub fn encode_timeline(t: &crate::audio::Timeline) -> Vec<u8> {
    let starts: Vec<f64> = t.starts.iter().map(|s| *s as f64).collect();
    let mut w = encode_f64s(&starts);
    w.extend_from_slice(&(t.total as f64).to_le_bytes());
    w
}

/// A tape duration: the seconds, then the playback order it followed.
pub fn encode_duration(seconds: f64, order: &[u32]) -> Vec<u8> {
    let mut w = header();
    w.extend_from_slice(&seconds.to_le_bytes());
    u32v(&mut w, order.len());
    for i in order {
        u32v(&mut w, *i as usize);
    }
    w
}

/// `[u32 count]` then `[f64 tstates][u8 level]` per pulse.
pub fn encode_pulses(pulses: &[(u64, u8)]) -> Vec<u8> {
    let mut w = header();
    u32v(&mut w, pulses.len());
    for (t, level) in pulses {
        w.extend_from_slice(&(*t as f64).to_le_bytes());
        w.push(*level);
    }
    w
}

/// Rendered samples, as little-endian `f32`.
pub fn encode_samples(samples: &[f32]) -> Vec<u8> {
    let mut w = header();
    u32v(&mut w, samples.len() * 4);
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}
