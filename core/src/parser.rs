//! TZX and TAP parsing, the port of `src/tzx/parser.ts`.

use crate::bytes::{ReadError, ReadResult, Reader};
use crate::types::{ArchiveEntry, Block, Body, HardwareEntry, PilotRun, SelectEntry, SymDef};

#[derive(Clone, Debug, Default)]
pub struct ParsedTape {
    pub blocks: Vec<Block>,
    pub major: u8,
    pub minor: u8,
    pub warnings: Vec<String>,
}

const TZX_SIGNATURE: &[u8; 8] = b"ZXTape!\x1a";

pub fn is_tzx(buf: &[u8]) -> bool {
    buf.len() >= 10 && &buf[..8] == TZX_SIGNATURE
}

pub fn parse_tzx(buf: &[u8]) -> Result<ParsedTape, String> {
    if !is_tzx(buf) {
        return Err("Not a TZX file (missing ZXTape! signature)".to_string());
    }
    let mut r = Reader::new(buf);
    r.pos = 8;
    // The signature check guarantees these two.
    let major = r.u8().unwrap();
    let minor = r.u8().unwrap();
    let mut blocks: Vec<Block> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    while !r.eof() {
        let start = r.pos;
        let id = r.u8().unwrap();
        match parse_block(id, &mut r) {
            Ok(body) => blocks.push(Block::new(body)),
            Err(e) => {
                warnings.push(format!(
                    "Block {} (ID {:02x}) at offset {}: {}",
                    blocks.len() + 1,
                    id,
                    start,
                    e
                ));
                // Keep whatever is left as an unknown block so nothing is silently lost.
                r.pos = start + 1;
                let raw = r.bytes(r.remaining()).unwrap();
                blocks.push(Block::new(Body::Unknown { id, raw }));
                break;
            }
        }
    }
    Ok(ParsedTape { blocks, major, minor, warnings })
}

fn read_sym_defs(r: &mut Reader, count: usize, max_pulses: u8) -> ReadResult<Vec<SymDef>> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let flags = r.u8()?;
        let mut pulses = Vec::with_capacity(max_pulses as usize);
        for _ in 0..max_pulses {
            pulses.push(r.u16()?);
        }
        // trim trailing zero pulses
        while pulses.last() == Some(&0) {
            pulses.pop();
        }
        out.push(SymDef { flags, pulses });
    }
    Ok(out)
}

pub fn bits_per_symbol(alphabet_size: u32) -> u32 {
    if alphabet_size <= 1 {
        return 1;
    }
    u32::BITS - (alphabet_size - 1).leading_zeros()
}

pub fn parse_block(id: u8, r: &mut Reader) -> ReadResult<Body> {
    Ok(match id {
        0x10 => {
            let pause = r.u16()?;
            let len = r.u16()? as usize;
            Body::Standard { pause, data: r.bytes(len)? }
        }
        0x11 => {
            let pilot = r.u16()?;
            let sync1 = r.u16()?;
            let sync2 = r.u16()?;
            let zero = r.u16()?;
            let one = r.u16()?;
            let pilot_len = r.u16()?;
            let used_bits = r.u8()?;
            let pause = r.u16()?;
            let len = r.u24()? as usize;
            Body::Turbo { pilot, sync1, sync2, zero, one, pilot_len, used_bits, pause, data: r.bytes(len)? }
        }
        0x12 => Body::PureTone { pulse_len: r.u16()?, count: r.u16()? },
        0x13 => {
            let n = r.u8()?;
            let mut pulses = Vec::with_capacity(n as usize);
            for _ in 0..n {
                pulses.push(r.u16()?);
            }
            Body::PulseSeq { pulses }
        }
        0x14 => {
            let zero = r.u16()?;
            let one = r.u16()?;
            let used_bits = r.u8()?;
            let pause = r.u16()?;
            let len = r.u24()? as usize;
            Body::PureData { zero, one, used_bits, pause, data: r.bytes(len)? }
        }
        0x15 => {
            let tstates = r.u16()?;
            let pause = r.u16()?;
            let used_bits = r.u8()?;
            let len = r.u24()? as usize;
            Body::Direct { tstates, pause, used_bits, data: r.bytes(len)? }
        }
        0x18 => {
            let len = r.u32()? as usize;
            // A length under the 10-byte header is corrupt; reading len - 10 bytes
            // would move the read position backwards.
            if len < 10 {
                return Err(ReadError(format!(
                    "CSW block length {} is shorter than its 10-byte header",
                    len
                )));
            }
            let pause = r.u16()?;
            let sample_rate = r.u24()?;
            let compression = r.u8()?;
            let pulse_count = r.u32()?;
            Body::Csw { pause, sample_rate, compression, pulse_count, data: r.bytes(len - 10)? }
        }
        0x19 => {
            let len = r.u32()? as usize;
            let end = r.pos + len;
            let pause = r.u16()?;
            let totp = r.u32()?;
            let npp = r.u8()?;
            let mut asp = r.u8()? as u32;
            let totd = r.u32()?;
            let npd = r.u8()?;
            let mut asd = r.u8()? as u32;
            if asp == 0 {
                asp = 256;
            }
            if asd == 0 {
                asd = 256;
            }
            let mut pilot_symbols: Vec<SymDef> = Vec::new();
            let mut pilot_stream: Vec<PilotRun> = Vec::new();
            if totp > 0 {
                pilot_symbols = read_sym_defs(r, asp as usize, npp)?;
                for _ in 0..totp {
                    let symbol = r.u8()?;
                    let reps = r.u16()?;
                    pilot_stream.push(PilotRun { symbol, reps });
                }
            }
            let mut data_symbols: Vec<SymDef> = Vec::new();
            let mut data: Vec<u8> = Vec::new();
            if totd > 0 {
                data_symbols = read_sym_defs(r, asd as usize, npd)?;
                let bits = u64::from(bits_per_symbol(asd)) * u64::from(totd);
                data = r.bytes(bits.div_ceil(8) as usize)?;
            }
            // Some files are sloppy; trust the declared block length.
            r.pos = end;
            Body::Generalized { pause, totp, npp, pilot_symbols, pilot_stream, totd, npd, data_symbols, data }
        }
        0x20 => Body::Pause { pause: r.u16()? },
        0x21 => {
            let n = r.u8()? as usize;
            Body::GroupStart { name: r.str(n)? }
        }
        0x22 => Body::GroupEnd,
        0x23 => Body::Jump { offset: r.i16()? },
        0x24 => Body::LoopStart { count: r.u16()? },
        0x25 => Body::LoopEnd,
        0x26 => {
            let n = r.u16()?;
            let mut offsets = Vec::with_capacity(n as usize);
            for _ in 0..n {
                offsets.push(r.i16()?);
            }
            Body::Call { offsets }
        }
        0x27 => Body::Return,
        0x28 => {
            let len = r.u16()? as usize;
            let end = r.pos + len;
            let n = r.u8()?;
            let mut entries = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let offset = r.i16()?;
                let l = r.u8()? as usize;
                entries.push(SelectEntry { offset, text: r.str(l)? });
            }
            r.pos = end;
            Body::Select { entries }
        }
        0x2a => {
            r.u32()?; // length, always 0
            Body::Stop48
        }
        0x2b => {
            r.u32()?; // length, always 1
            Body::SignalLevel { level: r.u8()? }
        }
        0x30 => {
            let n = r.u8()? as usize;
            Body::Text { text: r.str(n)? }
        }
        0x31 => {
            let time = r.u8()?;
            let n = r.u8()? as usize;
            Body::Message { time, text: r.str(n)? }
        }
        0x32 => {
            let len = r.u16()? as usize;
            let end = r.pos + len;
            let n = r.u8()?;
            let mut entries = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let kind = r.u8()?;
                let l = r.u8()? as usize;
                entries.push(ArchiveEntry { kind, text: r.str(l)? });
            }
            r.pos = end;
            Body::Archive { entries }
        }
        0x33 => {
            let n = r.u8()?;
            let mut entries = Vec::with_capacity(n as usize);
            for _ in 0..n {
                entries.push(HardwareEntry { kind: r.u8()?, id: r.u8()?, info: r.u8()? });
            }
            Body::Hardware { entries }
        }
        0x35 => {
            let ident = r.str(16)?;
            let len = r.u32()? as usize;
            Body::Custom { ident, data: r.bytes(len)? }
        }
        0x5a => Body::Glue { raw: r.bytes(9)? },
        // Deprecated blocks: keep raw so they round-trip.
        0x16 | 0x17 => {
            let start = r.pos;
            let len = r.u32()? as usize;
            r.pos = start;
            Body::Unknown { id, raw: r.bytes(4 + len)? }
        }
        0x34 => Body::Unknown { id, raw: r.bytes(8)? },
        0x40 => {
            let start = r.pos;
            r.u8()?;
            let len = r.u24()? as usize;
            r.pos = start;
            Body::Unknown { id, raw: r.bytes(4 + len)? }
        }
        // General extension rule: any unknown block has a DWORD length.
        _ => {
            let start = r.pos;
            let len = r.u32()? as usize;
            r.pos = start;
            Body::Unknown { id, raw: r.bytes(4 + len)? }
        }
    })
}

/// Parse a TAP file into standard speed blocks.
pub fn parse_tap(buf: &[u8]) -> ParsedTape {
    let mut r = Reader::new(buf);
    let mut blocks: Vec<Block> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    while !r.eof() {
        if r.remaining() < 2 {
            warnings.push("Trailing byte ignored at end of TAP file".to_string());
            break;
        }
        let len = r.u16().unwrap() as usize;
        if len > r.remaining() {
            warnings.push(format!(
                "Truncated TAP block {}: declared {} bytes, {} available",
                blocks.len() + 1,
                len,
                r.remaining()
            ));
            let rest = r.remaining();
            blocks.push(Block::new(Body::Standard { pause: 1000, data: r.bytes(rest).unwrap() }));
            break;
        }
        blocks.push(Block::new(Body::Standard { pause: 1000, data: r.bytes(len).unwrap() }));
    }
    ParsedTape { blocks, major: 1, minor: 20, warnings }
}

/// Auto-detect TZX vs TAP by signature.
pub fn parse_tape(buf: &[u8]) -> Result<ParsedTape, String> {
    if is_tzx(buf) {
        parse_tzx(buf)
    } else {
        Ok(parse_tap(buf))
    }
}
