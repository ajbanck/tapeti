//! POKEs text syntax <-> the standardized 'POKEs' custom info block, the port
//! of `src/tzx/pokes.ts`.
//!
//!   Each POKE on its own line:  [POKE] [page:]adr,val[/orgval]
//!   'val' may be '?' meaning "ask the user".
//!   Lines starting with ';' are trainer descriptions; the first ';' lines before any
//!   trainer form the general description.
//!
//! The TypeScript parses a line with one regular expression; this reads the
//! same grammar by hand, because the crate has no dependencies.

use crate::bytes::{latin1_to_string, string_to_latin1, ReadResult, Reader, Writer};

/// The numbers are as wide as the text can spell them, not as wide as the block
/// stores them: the editor round-trips text through here, and truncating early
/// would silently rewrite what the user typed. Writing truncates instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Poke {
    pub page: Option<u32>,
    pub addr: u32,
    /// None = the user inserts the value.
    pub value: Option<u32>,
    pub original: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Trainer {
    pub description: String,
    pub pokes: Vec<Poke>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PokesInfo {
    pub description: String,
    pub trainers: Vec<Trainer>,
}

/// The block stores line breaks as CR; the editor works in LF.
fn cr_to_lf(s: &str) -> String {
    s.replace('\r', "\n")
}

fn lf_to_cr(s: &str) -> String {
    s.replace('\n', "\r")
}

/// A block that runs out of bytes fails the way the TypeScript reader did.
pub fn decode_pokes(data: &[u8]) -> ReadResult<PokesInfo> {
    let mut r = Reader::new(data);
    let dl = r.u8()? as usize;
    let description = cr_to_lf(&r.str(dl)?);
    let n = r.u8()?;
    let mut trainers = Vec::new();
    for _ in 0..n {
        let tl = r.u8()? as usize;
        let desc = cr_to_lf(&r.str(tl)?);
        let np = r.u8()?;
        let mut pokes = Vec::new();
        for _ in 0..np {
            let kind = r.u8()?;
            let addr = r.u16()?;
            let value = r.u8()?;
            let original = r.u8()?;
            pokes.push(Poke {
                page: if kind & 8 != 0 { None } else { Some(u32::from(kind & 7)) },
                addr: u32::from(addr),
                value: if kind & 0x10 != 0 { None } else { Some(u32::from(value)) },
                original: if kind & 0x20 != 0 { None } else { Some(u32::from(original)) },
            });
        }
        trainers.push(Trainer { description: desc, pokes });
    }
    Ok(PokesInfo { description, trainers })
}

pub fn encode_pokes(info: &PokesInfo) -> Vec<u8> {
    let mut w = Writer::new();
    let mut d = string_to_latin1(&lf_to_cr(&info.description));
    d.truncate(255);
    w.u8(d.len() as u8);
    w.bytes(&d);
    w.u8(info.trainers.len() as u8);
    for t in &info.trainers {
        let mut td = string_to_latin1(&lf_to_cr(&t.description));
        td.truncate(255);
        w.u8(td.len() as u8);
        w.bytes(&td);
        w.u8(t.pokes.len() as u8);
        for p in &t.pokes {
            let mut kind = 0u8;
            match p.page {
                None => kind |= 8,
                Some(page) => kind |= (page & 7) as u8,
            }
            if p.value.is_none() {
                kind |= 0x10;
            }
            if p.original.is_none() {
                kind |= 0x20;
            }
            w.u8(kind);
            w.u16(p.addr as u16);
            w.u8(p.value.unwrap_or(0) as u8);
            w.u8(p.original.unwrap_or(0) as u8);
        }
    }
    w.into_vec()
}

pub fn pokes_to_text(info: &PokesInfo, hex: bool) -> String {
    let f = |n: u32| if hex { format!("{n:X}") } else { n.to_string() };
    let mut lines: Vec<String> = Vec::new();
    for l in info.description.split('\n') {
        lines.push(format!("; {l}"));
    }
    for t in &info.trainers {
        lines.push(String::new());
        lines.push(format!("[{}]", t.description.replace('\n', " | ")));
        for p in &t.pokes {
            let mut s = String::from("POKE ");
            if let Some(page) = p.page {
                s.push_str(&f(page));
                s.push(':');
            }
            s.push_str(&f(p.addr));
            s.push(',');
            match p.value {
                None => s.push('?'),
                Some(v) => s.push_str(&f(v)),
            }
            if let Some(o) = p.original {
                s.push('/');
                s.push_str(&f(o));
            }
            lines.push(s);
        }
    }
    lines.join("\n")
}

/// The characters a number may be written with, as the TypeScript's
/// `[0-9a-fx$#]` with the case-insensitive flag.
fn is_number_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c.to_ascii_lowercase(), 'a'..='f' | 'x') || c == '$' || c == '#'
}

/// JavaScript's `parseInt`: the longest prefix that is a number in this radix,
/// and nothing at all if there is no such prefix.
fn parse_int_prefix(s: &str, radix: u32) -> Option<u32> {
    let body = if radix == 16 {
        let lower = s.to_ascii_lowercase();
        if lower.starts_with("0x") {
            &s[2..]
        } else {
            s
        }
    } else {
        s
    };
    let mut value: u64 = 0;
    let mut any = false;
    for c in body.chars() {
        match c.to_digit(radix) {
            Some(d) => {
                value = value.saturating_mul(u64::from(radix)).saturating_add(u64::from(d));
                any = true;
            }
            None => break,
        }
    }
    if any {
        Some(value.min(u64::from(u32::MAX)) as u32)
    } else {
        None
    }
}

/// `$` and `0x` mean hex whatever the Dec/Hex switch says, `#` means decimal.
fn parse_number(s: &str, hex: bool) -> Result<u32, String> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if lower.starts_with('$') {
        return parse_int_prefix(&s[1..], 16).ok_or_else(|| bad_number(s));
    }
    if lower.starts_with("0x") {
        return parse_int_prefix(&s[2..], 16).ok_or_else(|| bad_number(s));
    }
    if let Some(rest) = s.strip_prefix('#') {
        return parse_int_prefix(rest, 10).ok_or_else(|| bad_number(s));
    }
    parse_int_prefix(s, if hex { 16 } else { 10 }).ok_or_else(|| bad_number(s))
}

fn bad_number(s: &str) -> String {
    format!("Bad number \"{s}\"")
}

/// One `[page:]addr,value[/original]` line, with an optional `POKE` in front.
fn parse_poke_line(line: &str, hex: bool) -> Option<Result<Poke, String>> {
    let rest = strip_poke(line);
    let (first, rest) = take_number(rest)?;
    let (page, addr, rest) = if let Some(after) = rest.strip_prefix(':') {
        let (second, rest) = take_number(after)?;
        (Some(first), second, rest)
    } else {
        (None, first, rest)
    };
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(',')?.trim_start();
    let (value, rest) = if let Some(after) = rest.strip_prefix('?') {
        (None, after)
    } else {
        let (v, rest) = take_number(rest)?;
        (Some(v), rest)
    };
    let trimmed = rest.trim_start();
    let (original, rest) = match trimmed.strip_prefix('/') {
        Some(after) => {
            let (o, rest) = take_number(after.trim_start())?;
            (Some(o), rest)
        }
        None => (None, rest),
    };
    if !rest.is_empty() {
        return None;
    }
    Some((|| {
        Ok(Poke {
            page: match page {
                Some(p) => Some(parse_number(p, hex)?),
                None => None,
            },
            addr: parse_number(addr, hex)?,
            value: match value {
                Some(v) => Some(parse_number(v, hex)?),
                None => None,
            },
            original: match original {
                Some(o) => Some(parse_number(o, hex)?),
                None => None,
            },
        })
    })())
}

fn strip_poke(line: &str) -> &str {
    if line.len() >= 4 && line[..4].eq_ignore_ascii_case("poke") {
        let after = &line[4..];
        let trimmed = after.trim_start();
        if trimmed.len() < after.len() {
            return trimmed;
        }
    }
    line
}

/// A run of number characters, and what follows it; `None` when there is none.
fn take_number(s: &str) -> Option<(&str, &str)> {
    let end = s.find(|c: char| !is_number_char(c)).unwrap_or(s.len());
    if end == 0 {
        None
    } else {
        Some((&s[..end], &s[end..]))
    }
}

pub fn text_to_pokes(text: &str, hex: bool) -> Result<PokesInfo, String> {
    let mut info = PokesInfo::default();
    let mut desc: Vec<String> = Vec::new();
    for (ln, raw) in text.split('\n').enumerate() {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix(';') {
            let text = rest.trim().to_string();
            match info.trainers.last_mut() {
                Some(cur) => {
                    if !cur.description.is_empty() {
                        cur.description.push('\n');
                    }
                    cur.description.push_str(&text);
                }
                None => desc.push(text),
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let name = rest.strip_suffix(']').unwrap_or(rest);
            info.trainers.push(Trainer { description: name.replace(" | ", "\n"), pokes: Vec::new() });
            continue;
        }
        let poke = parse_poke_line(line, hex)
            .ok_or_else(|| format!("Line {}: cannot parse \"{raw}\"", ln + 1))??;
        if info.trainers.is_empty() {
            info.trainers.push(Trainer::default());
        }
        info.trainers.last_mut().unwrap().pokes.push(poke);
    }
    info.description = desc.join("\n");
    Ok(info)
}

/// Latin-1 helpers the TypeScript re-exported for its tests.
pub fn text_bytes(s: &str) -> Vec<u8> {
    string_to_latin1(s)
}

pub fn bytes_text(b: &[u8]) -> String {
    latin1_to_string(b)
}
