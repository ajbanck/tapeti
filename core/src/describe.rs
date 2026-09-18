//! Block descriptions and ROM headers, the port of `src/tzx/describe.ts`.

use crate::bytes::{latin1_to_string, string_to_latin1};
use crate::types::{block_name, Block, Body};
use crate::writer::serialize_block;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderInfo {
    /// 0 program, 1 number array, 2 char array, 3 bytes
    pub kind: u8,
    pub type_name: String,
    pub name: String,
    pub length: u16,
    pub param1: u16,
    pub param2: u16,
}

pub const HEADER_TYPE_NAMES: [&str; 4] = ["Program", "Number array", "Character array", "Bytes"];

/// Decode a standard ROM header if this block looks like one.
pub fn decode_header(data: &[u8]) -> Option<HeaderInfo> {
    if data.len() != 19 || data[0] != 0x00 {
        return None;
    }
    let kind = data[1];
    if kind > 3 {
        return None;
    }
    let word = |i: usize| u16::from(data[i]) | u16::from(data[i + 1]) << 8;
    Some(HeaderInfo {
        kind,
        type_name: HEADER_TYPE_NAMES[kind as usize].to_string(),
        name: latin1_to_string(&data[2..12]),
        length: word(12),
        param1: word(14),
        param2: word(16),
    })
}

pub fn encode_header(h: &HeaderInfo) -> Vec<u8> {
    let mut d = vec![0u8; 19];
    d[1] = h.kind;
    let mut name = string_to_latin1(&h.name);
    name.resize(10, b' ');
    d[2..12].copy_from_slice(&name[..10]);
    d[12..14].copy_from_slice(&h.length.to_le_bytes());
    d[14..16].copy_from_slice(&h.param1.to_le_bytes());
    d[16..18].copy_from_slice(&h.param2.to_le_bytes());
    d[18] = checksum(&d[..18]);
    d
}

/// XOR of the bytes, as the ROM loader computes it.
pub fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0, |c, b| c ^ b)
}

/// Numbers in the list follow the Dec/Hex switch.
pub fn fmt(n: u32, hex: bool, pad: usize) -> String {
    if hex {
        format!("{:0>pad$X}", n, pad = pad)
    } else {
        n.to_string()
    }
}

fn header_desc(prefix: &str, h: &HeaderInfo, hex: bool) -> String {
    let name = h.name.trim_end();
    let n = |v: u16| fmt(u32::from(v), hex, 0);
    match h.kind {
        0 => {
            let auto = if h.param1 >= 32768 { "none".to_string() } else { n(h.param1) };
            format!("{prefix} Prog: {name}; L: {}, P: {}, S: {auto}", n(h.length), n(h.param2))
        }
        3 => format!("{prefix} Bytes: {name}; S: {}, L: {}", n(h.param1), n(h.length)),
        1 => format!("{prefix} Num: {name}; L: {}, {}", n(h.length), array_var_name(h.param1)),
        2 => format!("{prefix} Char: {name}; L: {}, {}", n(h.length), array_var_name(h.param1)),
        _ => prefix.to_string(),
    }
}

/// The variable a saved array belongs to: bit 6 of the high byte says string.
fn array_var_name(p: u16) -> String {
    let high = p >> 8;
    let letter = ((high & 0x1f) as u8 + 0x60) as char;
    if high & 0x40 != 0 {
        format!("var {letter}$")
    } else {
        format!("var {letter}")
    }
}

fn speed_prefix(body: &Body) -> &'static str {
    match body {
        Body::Standard { .. } => "Std speed",
        Body::Turbo { .. } => "Turbo speed",
        Body::PureData { .. } => "Pure data",
        Body::Generalized { .. } => "Generalized",
        _ => "",
    }
}

/// The first line of a text field, which is all the list has room for.
fn first_line(text: &str) -> &str {
    let line = text.split('\n').next().unwrap_or("");
    line.strip_suffix('\r').unwrap_or(line)
}

/// One-line description used in the block list.
pub fn describe_block(b: &Block, hex: bool) -> String {
    let n = |v: u32| fmt(v, hex, 0);
    match &b.body {
        Body::Unknown { id, .. } => {
            format!("{} (ID {:02X})", block_name(*id).unwrap_or("Unknown block"), id)
        }
        Body::Standard { data, .. } | Body::Turbo { data, .. } | Body::PureData { data, .. } => {
            match decode_header(data) {
                Some(h) => header_desc(speed_prefix(&b.body), &h, hex),
                None => match &b.body {
                    Body::Standard { .. } => "Standard speed data".to_string(),
                    Body::Turbo { .. } => "Turbo loading data".to_string(),
                    _ => "Pure data".to_string(),
                },
            }
        }
        Body::Generalized { data, .. } => match decode_header(data) {
            Some(h) => header_desc("Generalized", &h, hex),
            None => "Generalized data".to_string(),
        },
        Body::PureTone { pulse_len, count } => {
            format!("Pure tone {} x {}", n(u32::from(*pulse_len)), n(u32::from(*count)))
        }
        Body::PulseSeq { pulses } => format!("Pulse sequence ({})", n(pulses.len() as u32)),
        Body::Direct { tstates, .. } => {
            format!("Direct recording {} T/sample", n(u32::from(*tstates)))
        }
        Body::Csw { sample_rate, .. } => format!("CSW recording {} Hz", n(*sample_rate)),
        Body::Pause { pause } => {
            if *pause == 0 {
                "Stop the tape".to_string()
            } else {
                format!("Pause {} ms", n(u32::from(*pause)))
            }
        }
        Body::GroupStart { name } => format!("Group {name}"),
        Body::GroupEnd => "Group end".to_string(),
        Body::Jump { offset } => {
            let sign = if *offset < 0 { '-' } else { '+' };
            format!("Jump {sign}{}", n(i32::from(*offset).unsigned_abs()))
        }
        Body::LoopStart { count } => format!("Loop {} times", n(u32::from(*count))),
        Body::LoopEnd => "Loop end".to_string(),
        Body::Call { offsets } => format!("Call sequence ({})", n(offsets.len() as u32)),
        Body::Return => "Return from sequence".to_string(),
        Body::Select { entries } => format!("Select block ({})", n(entries.len() as u32)),
        Body::Stop48 => "Stop the tape if in 48K mode".to_string(),
        Body::SignalLevel { level } => {
            format!("Set signal level {}", if *level != 0 { "high" } else { "low" })
        }
        Body::Text { text } => format!("Text: {}", first_line(text)),
        Body::Message { text, .. } => format!("Message: {}", first_line(text)),
        Body::Archive { .. } => "Archive info".to_string(),
        Body::Hardware { .. } => "Hardware type".to_string(),
        Body::Custom { ident, .. } => format!("Custom info {}", ident.trim()),
        Body::Glue { .. } => "Glue block".to_string(),
    }
}

/// Right-hand column in the block list: data length for data blocks, else
/// serialized size.
pub fn block_length(b: &Block) -> u32 {
    match &b.body {
        Body::Unknown { raw, .. } => raw.len() as u32,
        Body::Standard { data, .. }
        | Body::Turbo { data, .. }
        | Body::PureData { data, .. }
        | Body::Direct { data, .. }
        | Body::Csw { data, .. }
        | Body::Generalized { data, .. }
        | Body::Custom { data, .. } => data.len() as u32,
        Body::PureTone { count, .. } => u32::from(*count),
        Body::PulseSeq { pulses } => pulses.len() as u32,
        _ => serialize_block(b).len() as u32 - 1,
    }
}

/// Metadata blocks are not part of the tape signal. The TypeScript keeps its
/// own copy of this predicate for the UI (`isMetadata` in `describe.ts`), and a
/// differential test pins the two together for every block ID.
pub fn is_metadata(b: &Block) -> bool {
    match &b.body {
        Body::GroupStart { .. }
        | Body::GroupEnd
        | Body::Text { .. }
        | Body::Message { .. }
        | Body::Archive { .. }
        | Body::Hardware { .. }
        | Body::Custom { .. }
        | Body::Glue { .. } => true,
        Body::Unknown { id, .. } => *id != 0x16 && *id != 0x17,
        _ => false,
    }
}
