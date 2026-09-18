//! A canonical text rendering of a parsed tape, used to compare this crate
//! against the TypeScript implementation byte for byte.
//!
//! `scripts/dump-blocks.mjs` prints the same format from `src/tzx/parser.ts`,
//! so a fixture generated there is a differential test of the two parsers.
//! The format is:
//!
//! ```text
//! tape <major>.<minor>
//! warn <message>                       (one line per warning, in order)
//! <index> <id:02x> <field>=<value> ... (one line per block, uid excluded)
//! ```
//!
//! Field names are the TypeScript property names, in declaration order.
//! Values: numbers in decimal; byte arrays in lowercase hex; strings quoted
//! with `\xNN` for anything outside printable ASCII; list and record elements
//! separated by `;`, with `/` between a record's fields, except a pilot run,
//! which is `symbol*reps`.
//!
//! The fixture files carry one extra `file <path>` line at the top, naming the
//! tape they were generated from; `dump_tape` does not print it.

use crate::parser::ParsedTape;
use crate::types::{Block, Body, SymDef};

pub fn dump_tape(t: &ParsedTape) -> String {
    let mut out = format!("tape {}.{}\n", t.major, t.minor);
    for w in &t.warnings {
        out.push_str(&format!("warn {}\n", w));
    }
    for (i, b) in t.blocks.iter().enumerate() {
        out.push_str(&dump_block(i, b));
        out.push('\n');
    }
    out
}

pub fn dump_block(index: usize, b: &Block) -> String {
    let mut s = format!("{} {:02x}", index, b.id());
    let mut f = |name: &str, value: String| s.push_str(&format!(" {}={}", name, value));
    match &b.body {
        Body::Standard { pause, data } => {
            f("pause", pause.to_string());
            f("data", hex(data));
        }
        Body::Turbo { pilot, sync1, sync2, zero, one, pilot_len, used_bits, pause, data } => {
            f("pilot", pilot.to_string());
            f("sync1", sync1.to_string());
            f("sync2", sync2.to_string());
            f("zero", zero.to_string());
            f("one", one.to_string());
            f("pilotLen", pilot_len.to_string());
            f("usedBits", used_bits.to_string());
            f("pause", pause.to_string());
            f("data", hex(data));
        }
        Body::PureTone { pulse_len, count } => {
            f("pulseLen", pulse_len.to_string());
            f("count", count.to_string());
        }
        Body::PulseSeq { pulses } => f("pulses", nums(pulses)),
        Body::PureData { zero, one, used_bits, pause, data } => {
            f("zero", zero.to_string());
            f("one", one.to_string());
            f("usedBits", used_bits.to_string());
            f("pause", pause.to_string());
            f("data", hex(data));
        }
        Body::Direct { tstates, pause, used_bits, data } => {
            f("tstates", tstates.to_string());
            f("pause", pause.to_string());
            f("usedBits", used_bits.to_string());
            f("data", hex(data));
        }
        Body::Csw { pause, sample_rate, compression, pulse_count, data } => {
            f("pause", pause.to_string());
            f("sampleRate", sample_rate.to_string());
            f("compression", compression.to_string());
            f("pulseCount", pulse_count.to_string());
            f("data", hex(data));
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
            f("pause", pause.to_string());
            f("totp", totp.to_string());
            f("npp", npp.to_string());
            f("pilotSymbols", syms(pilot_symbols));
            f("pilotStream", join(pilot_stream.iter().map(|p| format!("{}*{}", p.symbol, p.reps))));
            f("totd", totd.to_string());
            f("npd", npd.to_string());
            f("dataSymbols", syms(data_symbols));
            f("data", hex(data));
        }
        Body::Pause { pause } => f("pause", pause.to_string()),
        Body::GroupStart { name } => f("name", quote(name)),
        Body::GroupEnd => {}
        Body::Jump { offset } => f("offset", offset.to_string()),
        Body::LoopStart { count } => f("count", count.to_string()),
        Body::LoopEnd => {}
        Body::Call { offsets } => f("offsets", nums(offsets)),
        Body::Return => {}
        Body::Select { entries } => {
            f("entries", join(entries.iter().map(|e| format!("{}/{}", e.offset, quote(&e.text)))))
        }
        Body::Stop48 => {}
        Body::SignalLevel { level } => f("level", level.to_string()),
        Body::Text { text } => f("text", quote(text)),
        Body::Message { time, text } => {
            f("time", time.to_string());
            f("text", quote(text));
        }
        Body::Archive { entries } => {
            f("entries", join(entries.iter().map(|e| format!("{}/{}", e.kind, quote(&e.text)))))
        }
        Body::Hardware { entries } => {
            f("entries", join(entries.iter().map(|e| format!("{}/{}/{}", e.kind, e.id, e.info))))
        }
        Body::Custom { ident, data } => {
            f("ident", quote(ident));
            f("data", hex(data));
        }
        Body::Glue { raw } => f("raw", hex(raw)),
        Body::Unknown { raw, .. } => {
            f("unknown", "true".to_string());
            f("raw", hex(raw));
        }
    }
    s
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for v in b {
        s.push_str(&format!("{:02x}", v));
    }
    s
}

fn nums<T: std::fmt::Display>(v: &[T]) -> String {
    join(v.iter().map(|n| n.to_string()))
}

fn join<I: Iterator<Item = String>>(parts: I) -> String {
    let v: Vec<String> = parts.collect();
    v.join(";")
}

fn syms(v: &[SymDef]) -> String {
    join(v.iter().map(|s| format!("{}/{}", s.flags, nums(&s.pulses))))
}

/// Latin-1 text with everything but printable ASCII escaped, so the dump stays
/// one line per block and compares exactly.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        let v = c as u32;
        if c == '"' || c == '\\' || !(0x20..=0x7e).contains(&v) {
            out.push_str(&format!("\\x{:02x}", v));
        } else {
            out.push(c);
        }
    }
    out.push('"');
    out
}
