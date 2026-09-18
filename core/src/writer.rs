//! TZX and TAP writing, the port of `src/tzx/writer.ts`.

use crate::bytes::{string_to_latin1, Writer};
use crate::parser::bits_per_symbol;
use crate::types::{Block, Body};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u8,
    pub minor: u8,
}

/// Lowest TZX version able to represent these blocks.
///
/// Must not look at block data: the app sends this one a payload with the byte
/// payloads left out, because only block types and entries matter here.
pub fn required_version(blocks: &[Block]) -> Version {
    let mut minor = 0u8;
    let mut has_unknown = false;
    for b in blocks {
        let mut need = 0u8;
        match &b.body {
            Body::Unknown { .. } => {
                has_unknown = true;
                continue;
            }
            Body::Custom { .. } => need = 1,
            Body::LoopStart { .. }
            | Body::LoopEnd
            | Body::Call { .. }
            | Body::Return
            | Body::Select { .. }
            | Body::Glue { .. } => need = 10,
            Body::Stop48 => need = 12,
            Body::Csw { .. } | Body::Generalized { .. } | Body::SignalLevel { .. } => need = 20,
            // Field types 04+ and multi-line entries need 1.10/1.12.
            Body::Archive { entries } => {
                for e in entries {
                    if (5..=8).contains(&e.kind) {
                        need = need.max(12);
                    } else if e.kind == 4 || e.text.contains('\n') || e.text.contains('\r') {
                        need = need.max(10);
                    }
                }
            }
            // Hardware ids were added to the spec in batches, so the floor a
            // block needs depends on both its type and its id.
            Body::Hardware { entries } => {
                for e in entries {
                    let added_in_120 = (e.kind == 1 && e.id >= 0x12)
                        || (e.kind == 2 && e.id >= 0x06)
                        || (e.kind == 3 && e.id >= 0x06)
                        || (e.kind == 6 && e.id >= 0x03)
                        || (e.kind == 0x0b && e.id >= 0x03)
                        || e.kind == 0x10;
                    if e.kind == 0 && (0x1e..=0x2d).contains(&e.id) {
                        need = need.max(20);
                    } else if e.kind == 0 && (e.id == 0x1c || e.id == 0x1d) {
                        need = need.max(13);
                    } else if e.kind == 0 && (e.id == 0x1a || e.id == 0x1b) {
                        need = need.max(12);
                    } else if e.kind == 0 && (0x15..=0x19).contains(&e.id) {
                        need = need.max(2);
                    } else if added_in_120 {
                        need = need.max(20);
                    }
                }
            }
            _ => {}
        }
        if need > minor {
            minor = need;
        }
    }
    // Unknown blocks: we cannot judge; assume the newest spec.
    if has_unknown {
        minor = minor.max(20);
    }
    Version { major: 1, minor }
}

/// Version to write in the header: the lowest one the blocks need, but never below the
/// version the tape was loaded with, so an unaltered file round-trips byte for byte.
pub fn save_version(blocks: &[Block], loaded: Option<Version>) -> Version {
    let v = required_version(blocks);
    match loaded {
        None => v,
        Some(l) => {
            let newer = l.major > v.major || (l.major == v.major && l.minor > v.minor);
            if newer {
                l
            } else {
                v
            }
        }
    }
}

/// Text fields carry a byte length, so anything longer is cut, as in the
/// TypeScript's `slice(0, 255)`.
fn short_str(s: &str) -> Vec<u8> {
    let mut b = string_to_latin1(s);
    b.truncate(255);
    b
}

pub fn write_block(w: &mut Writer, b: &Block) {
    w.u8(b.id());
    match &b.body {
        Body::Unknown { raw, .. } => w.bytes(raw),
        Body::Standard { pause, data } => {
            w.u16(*pause);
            w.u16(data.len() as u16);
            w.bytes(data);
        }
        Body::Turbo { pilot, sync1, sync2, zero, one, pilot_len, used_bits, pause, data } => {
            w.u16(*pilot);
            w.u16(*sync1);
            w.u16(*sync2);
            w.u16(*zero);
            w.u16(*one);
            w.u16(*pilot_len);
            w.u8(*used_bits);
            w.u16(*pause);
            w.u24(data.len() as u32);
            w.bytes(data);
        }
        Body::PureTone { pulse_len, count } => {
            w.u16(*pulse_len);
            w.u16(*count);
        }
        Body::PulseSeq { pulses } => {
            w.u8(pulses.len() as u8);
            for p in pulses {
                w.u16(*p);
            }
        }
        Body::PureData { zero, one, used_bits, pause, data } => {
            w.u16(*zero);
            w.u16(*one);
            w.u8(*used_bits);
            w.u16(*pause);
            w.u24(data.len() as u32);
            w.bytes(data);
        }
        Body::Direct { tstates, pause, used_bits, data } => {
            w.u16(*tstates);
            w.u16(*pause);
            w.u8(*used_bits);
            w.u24(data.len() as u32);
            w.bytes(data);
        }
        Body::Csw { pause, sample_rate, compression, pulse_count, data } => {
            w.u32(10 + data.len() as u32);
            w.u16(*pause);
            w.u24(*sample_rate);
            w.u8(*compression);
            w.u32(*pulse_count);
            w.bytes(data);
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
            let mut body = Writer::new();
            body.u16(*pause);
            let asp = pilot_symbols.len();
            let asd = data_symbols.len();
            let totp_out = if *totp > 0 && asp > 0 { pilot_stream.len() as u32 } else { 0 };
            let totd_out = if *totd > 0 && asd > 0 { *totd } else { 0 };
            body.u32(totp_out);
            body.u8(*npp);
            body.u8(if asp >= 256 { 0 } else { asp as u8 });
            body.u32(totd_out);
            body.u8(*npd);
            body.u8(if asd >= 256 { 0 } else { asd as u8 });
            if totp_out > 0 {
                for s in pilot_symbols {
                    body.u8(s.flags);
                    for i in 0..*npp as usize {
                        body.u16(s.pulses.get(i).copied().unwrap_or(0));
                    }
                }
                for run in pilot_stream {
                    body.u8(run.symbol);
                    body.u16(run.reps);
                }
            }
            if totd_out > 0 {
                for s in data_symbols {
                    body.u8(s.flags);
                    for i in 0..*npd as usize {
                        body.u16(s.pulses.get(i).copied().unwrap_or(0));
                    }
                }
                let ds = (u64::from(bits_per_symbol(asd as u32)) * u64::from(totd_out)).div_ceil(8) as usize;
                let mut out = vec![0u8; ds];
                let take = ds.min(data.len());
                out[..take].copy_from_slice(&data[..take]);
                body.bytes(&out);
            }
            w.u32(body.len() as u32);
            w.bytes(&body.into_vec());
        }
        Body::Pause { pause } => w.u16(*pause),
        Body::GroupStart { name } => {
            let n = short_str(name);
            w.u8(n.len() as u8);
            w.bytes(&n);
        }
        Body::GroupEnd => {}
        Body::Jump { offset } => w.i16(*offset),
        Body::LoopStart { count } => w.u16(*count),
        Body::LoopEnd => {}
        Body::Call { offsets } => {
            w.u16(offsets.len() as u16);
            for o in offsets {
                w.i16(*o);
            }
        }
        Body::Return => {}
        Body::Select { entries } => {
            let mut body = Writer::new();
            body.u8(entries.len() as u8);
            for e in entries {
                body.i16(e.offset);
                let t = short_str(&e.text);
                body.u8(t.len() as u8);
                body.bytes(&t);
            }
            w.u16(body.len() as u16);
            w.bytes(&body.into_vec());
        }
        Body::Stop48 => w.u32(0),
        Body::SignalLevel { level } => {
            w.u32(1);
            w.u8(*level);
        }
        Body::Text { text } => {
            let t = short_str(text);
            w.u8(t.len() as u8);
            w.bytes(&t);
        }
        Body::Message { time, text } => {
            let t = short_str(text);
            w.u8(*time);
            w.u8(t.len() as u8);
            w.bytes(&t);
        }
        Body::Archive { entries } => {
            let mut body = Writer::new();
            body.u8(entries.len() as u8);
            for e in entries {
                let t = short_str(&e.text);
                body.u8(e.kind);
                body.u8(t.len() as u8);
                body.bytes(&t);
            }
            w.u16(body.len() as u16);
            w.bytes(&body.into_vec());
        }
        Body::Hardware { entries } => {
            w.u8(entries.len() as u8);
            for e in entries {
                w.u8(e.kind);
                w.u8(e.id);
                w.u8(e.info);
            }
        }
        Body::Custom { ident, data } => {
            let mut id = string_to_latin1(ident);
            id.resize(16, b' ');
            w.bytes(&id);
            w.u32(data.len() as u32);
            w.bytes(data);
        }
        Body::Glue { raw } => {
            if raw.len() == 9 {
                w.bytes(raw);
            } else {
                w.bytes(&[0x58, 0x54, 0x61, 0x70, 0x65, 0x21, 0x1a, 1, 20]);
            }
        }
    }
}

pub fn serialize_tzx(blocks: &[Block], version: Option<Version>) -> Vec<u8> {
    let v = version.unwrap_or_else(|| required_version(blocks));
    let mut w = Writer::new();
    w.str("ZXTape!\u{1a}");
    w.u8(v.major);
    w.u8(v.minor);
    for b in blocks {
        write_block(&mut w, b);
    }
    w.into_vec()
}

/// Serialize a single block to bytes (ID + body); used for comparison and size display.
pub fn serialize_block(b: &Block) -> Vec<u8> {
    let mut w = Writer::new();
    write_block(&mut w, b);
    w.into_vec()
}

/// TAP export: only standard-speed blocks can be represented. Returns the
/// indices of the blocks that had to be left out.
pub fn serialize_tap(blocks: &[Block]) -> (Vec<u8>, Vec<u32>) {
    let mut w = Writer::new();
    let mut skipped: Vec<u32> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        match &b.body {
            Body::Unknown { .. } => skipped.push(i as u32),
            Body::Standard { data, .. } | Body::Turbo { data, .. } | Body::PureData { data, .. } => {
                w.u16(data.len() as u16);
                w.bytes(data);
            }
            Body::Generalized { data, .. } if !data.is_empty() => {
                w.u16(data.len() as u16);
                w.bytes(data);
            }
            // metadata: silently dropped
            Body::Pause { .. }
            | Body::GroupStart { .. }
            | Body::GroupEnd
            | Body::Text { .. }
            | Body::Message { .. }
            | Body::Archive { .. }
            | Body::Hardware { .. }
            | Body::Custom { .. }
            | Body::Stop48
            | Body::Glue { .. } => {}
            _ => skipped.push(i as u32),
        }
    }
    (w.into_vec(), skipped)
}
