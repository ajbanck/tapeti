//! The BASIC lister and the variables area, the port of `src/spectrum/basic.ts`.
//!
//! The number formatting follows JavaScript's, because that is what the listing
//! has always shown: `Number.prototype.toPrecision(8)` and its trailing-zero
//! trim, reimplemented here rather than approximated.

use super::charset::{token_name, zx_char};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Text,
    Token,
    Number,
    Ctrl,
    Hidden,
}

impl TokenKind {
    pub fn name(self) -> &'static str {
        match self {
            TokenKind::Text => "text",
            TokenKind::Token => "token",
            TokenKind::Number => "number",
            TokenKind::Ctrl => "ctrl",
            TokenKind::Hidden => "hidden",
        }
    }

    pub fn from_name(s: &str) -> Self {
        match s {
            "token" => TokenKind::Token,
            "number" => TokenKind::Number,
            "ctrl" => TokenKind::Ctrl,
            "hidden" => TokenKind::Hidden,
            _ => TokenKind::Text,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasicToken {
    pub text: String,
    pub kind: TokenKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasicLine {
    pub number: u32,
    pub length: u32,
    pub offset: u32,
    pub tokens: Vec<BasicToken>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BasicOptions {
    /// Show the real 5-byte value after the textual number.
    pub show_numbers: bool,
    pub basic128: bool,
    /// 32 columns, control codes interpreted.
    pub speccy_format: bool,
}

/// Decode a 5-byte Sinclair floating point number.
pub fn decode_number(d: &[u8], o: usize) -> f64 {
    if o + 5 > d.len() {
        return f64::NAN;
    }
    let (b0, b1, b2, b3, b4) = (d[o], d[o + 1], d[o + 2], d[o + 3], d[o + 4]);
    if b0 == 0 {
        // "small integer" form; the real interpreter ignores byte 4
        let v = f64::from(u16::from(b2) | u16::from(b3) << 8);
        return if b1 == 0xff { v - 65536.0 } else { v };
    }
    let exp = i32::from(b0) - 128;
    let sign = if b1 & 0x80 != 0 { -1.0 } else { 1.0 };
    let mant = (f64::from(b1 | 0x80) * 16777216.0
        + f64::from(u32::from(b2) << 16)
        + f64::from(u32::from(b3) << 8)
        + f64::from(b4))
        / 4294967296.0;
    sign * mant * 2f64.powi(exp)
}

/// JavaScript's `Number.prototype.toPrecision(8)`.
///
/// Rust rounds a tie to the even digit and JavaScript rounds it up, so the
/// digits are rounded here by hand, from a 17-digit form that identifies the
/// value exactly.
fn to_precision8(v: f64) -> String {
    const P: usize = 8;
    if v == 0.0 {
        return "0.0000000".to_string();
    }
    let sign = if v < 0.0 { "-" } else { "" };
    let sci = format!("{:.16e}", v.abs());
    let (mantissa, exp) = sci.split_once('e').expect("exponential form");
    let mut e: i32 = exp.parse().expect("exponent");
    let mut digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    digits.resize(17, b'0');

    // Round to P significant digits, half away from zero: the first dropped
    // digit decides, and a five rounds up whatever follows it.
    let round_up = digits[P] >= b'5';
    let mut kept: Vec<u8> = digits[..P].to_vec();
    if round_up {
        let mut i = P;
        loop {
            if i == 0 {
                kept.insert(0, b'1');
                kept.truncate(P);
                e += 1;
                break;
            }
            i -= 1;
            if kept[i] == b'9' {
                kept[i] = b'0';
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    let d = String::from_utf8(kept).expect("digits");

    if !(-6..P as i32).contains(&e) {
        let esign = if e < 0 { '-' } else { '+' };
        return format!("{sign}{}.{}e{esign}{}", &d[..1], &d[1..], e.abs());
    }
    if e < 0 {
        return format!("{sign}0.{}{d}", "0".repeat((-e - 1) as usize));
    }
    let point = (e + 1) as usize;
    if point >= P {
        return format!("{sign}{d}");
    }
    format!("{sign}{}.{}", &d[..point], &d[point..])
}

/// The listing's number format: integers plain, everything else to eight
/// significant digits with the trailing zeros trimmed off.
pub fn format_number(v: f64) -> String {
    if v.is_nan() {
        return "?".to_string();
    }
    // `toPrecision` spells these out, and the trailing-zero trim leaves them be.
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if v.fract() == 0.0 && v.is_finite() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = to_precision8(v);
    if s.contains('e') {
        return s;
    }
    // `s.replace(/\.?0+$/, '')`, quirks included: a trailing run of zeros goes,
    // even when there is no decimal point in front of it.
    let trimmed = s.trim_end_matches('0');
    if trimmed.len() == s.len() {
        return s;
    }
    trimmed.strip_suffix('.').unwrap_or(trimmed).to_string()
}

/// Does the whole string parse as the number literal the lister looks for?
fn is_number_literal(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && b[i] == b'-' {
        i += 1;
    }
    let before = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - before;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - start;
        // `[0-9]*\.?[0-9]+` needs at least one digit after the point.
        if frac_digits == 0 {
            return false;
        }
    } else if int_digits == 0 {
        return false;
    }
    if int_digits == 0 && frac_digits == 0 {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == b.len()
}

/// The number the text ends with, as the lister's regular expression finds it:
/// the longest suffix that is a number literal.
fn trailing_number(text: &str) -> Option<&str> {
    let b = text.as_bytes();
    for i in 0..b.len() {
        if !text.is_char_boundary(i) {
            continue;
        }
        let tail = &text[i..];
        if is_number_literal(tail) {
            return Some(tail);
        }
    }
    None
}

/// List a BASIC program area. `data` is the raw bytes starting at PROG.
pub fn list_basic(data: &[u8], start: usize, end: usize, opts: BasicOptions) -> Vec<BasicLine> {
    let end = end.min(data.len().max(end)); // the TypeScript reads past the end as undefined
    let mut lines: Vec<BasicLine> = Vec::new();
    let mut p = start;
    while p + 4 <= end {
        let at = |i: usize| -> u8 { data.get(i).copied().unwrap_or(0) };
        let number = u32::from(at(p)) << 8 | u32::from(at(p + 1));
        if number >= 0x4000 {
            break; // reached the variables area or garbage
        }
        let length = usize::from(at(p + 2)) | usize::from(at(p + 3)) << 8;
        let line_start = p + 4;
        let line_end = end.min(line_start + length);
        let mut line =
            BasicLine { number, length: length as u32, offset: p as u32, tokens: Vec::new(), error: None };
        let mut q = line_start;
        let mut text = String::new();
        let mut last_was_space = true;
        while q < line_end {
            let c = at(q);
            q += 1;
            if c == 0x0d {
                break;
            }
            if c == 0x0e {
                // hidden 5-byte number follows
                let v = decode_number(data, q);
                // Compare with the textual form to spot protected/altered numbers.
                let textual = trailing_number(&text).and_then(|s| s.parse::<f64>().ok());
                if !text.is_empty() {
                    line.tokens.push(BasicToken { text: std::mem::take(&mut text), kind: TokenKind::Text });
                }
                text.clear();
                let differs = textual.is_some_and(|t| (t - v).abs() > 1e-6 * 1f64.max(v.abs()));
                if opts.show_numbers || differs {
                    line.tokens.push(BasicToken {
                        text: format!("{{{}}}", format_number(v)),
                        kind: TokenKind::Number,
                    });
                }
                q += 5;
                continue;
            }
            if let Some(tok) = token_name(c, opts.basic128) {
                if !text.is_empty() {
                    line.tokens.push(BasicToken { text: std::mem::take(&mut text), kind: TokenKind::Text });
                }
                // Sinclair ROM prints a leading space unless preceded by a space, and a trailing space
                let s = if last_was_space { tok.to_string() } else { format!(" {tok}") };
                line.tokens.push(BasicToken { text: format!("{s} "), kind: TokenKind::Token });
                last_was_space = true;
                continue;
            }
            if c < 0x20 {
                if !text.is_empty() {
                    line.tokens.push(BasicToken { text: std::mem::take(&mut text), kind: TokenKind::Text });
                }
                if (0x10..=0x15).contains(&c) && q < line_end {
                    const NAMES: [&str; 6] = ["INK", "PAPER", "FLASH", "BRIGHT", "INVERSE", "OVER"];
                    let t = if opts.speccy_format {
                        String::new()
                    } else {
                        format!("[{} {}]", NAMES[usize::from(c) - 0x10], at(q))
                    };
                    line.tokens.push(BasicToken { text: t, kind: TokenKind::Ctrl });
                    q += 1;
                } else if (c == 0x16 || c == 0x17) && q + 1 < line_end {
                    let t = if opts.speccy_format {
                        String::new()
                    } else {
                        format!("[{} {},{}]", if c == 0x16 { "AT" } else { "TAB" }, at(q), at(q + 1))
                    };
                    line.tokens.push(BasicToken { text: t, kind: TokenKind::Ctrl });
                    q += 2;
                } else if c == 0x06 {
                    let t = if opts.speccy_format { "\t" } else { "[,]" };
                    line.tokens.push(BasicToken { text: t.to_string(), kind: TokenKind::Ctrl });
                } else if c == 0x08 {
                    // cursor back: approximate by deleting the last character
                    let t = if opts.speccy_format { "\u{8}" } else { "[<]" };
                    line.tokens.push(BasicToken { text: t.to_string(), kind: TokenKind::Ctrl });
                } else {
                    line.tokens.push(BasicToken { text: format!("[{c:02x}]"), kind: TokenKind::Ctrl });
                }
                last_was_space = false;
                continue;
            }
            let ch = zx_char(c, false);
            last_was_space = ch == " ";
            text.push_str(&ch);
        }
        if !text.is_empty() {
            line.tokens.push(BasicToken { text: text.clone(), kind: TokenKind::Text });
        }
        p = line_start + length;
        if length == 0 {
            line.error = Some("zero length line".to_string());
            p += 1;
        }
        lines.push(line);
    }
    lines
}

/// Render a listing as plain text.
pub fn basic_to_text(lines: &[BasicLine], opts: BasicOptions) -> String {
    let rendered: Vec<String> = lines
        .iter()
        .map(|l| {
            let mut s = format!("{:>4} ", l.number);
            for t in &l.tokens {
                if t.text == "\u{8}" {
                    let mut chars: Vec<char> = s.chars().collect();
                    chars.pop();
                    s = chars.into_iter().collect();
                } else if t.text == "\t" {
                    let pad = 16 - (s.chars().count() % 16);
                    s.push_str(&" ".repeat(pad));
                } else {
                    s.push_str(&t.text);
                }
            }
            if opts.speccy_format {
                // wrap at 32 characters like the ROM's LIST
                let chars: Vec<char> = s.chars().collect();
                let mut out: Vec<String> = Vec::new();
                let mut i = 0;
                while i < chars.len() {
                    let end = (i + 32).min(chars.len());
                    out.push(chars[i..end].iter().collect());
                    i = end;
                }
                out.join("\n")
            } else {
                s.trim_end().to_string()
            }
        })
        .collect();
    rendered.join("\n")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariableEntry {
    pub name: String,
    pub kind: String,
    pub value: String,
    pub offset: u32,
    pub size: u32,
}

/// JavaScript's `JSON.stringify` of a string, which is how the previews are quoted.
fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// List the variables area (starting at VARS) until the 0x80 end marker.
pub fn list_variables(data: &[u8], start: usize, end: usize) -> Vec<VariableEntry> {
    let mut out: Vec<VariableEntry> = Vec::new();
    let at = |i: usize| -> u8 { data.get(i).copied().unwrap_or(0) };
    let word = |i: usize| -> usize { usize::from(at(i)) | usize::from(at(i + 1)) << 8 };
    let mut p = start;
    let mut guard = 0;
    while p < end && guard < 10000 {
        guard += 1;
        let h = at(p);
        if h == 0x80 {
            break;
        }
        let kind = h >> 5;
        let letter = char::from((h & 0x1f) + 0x60);
        let at_p = p as u32;
        match kind {
            0b011 => {
                let len = word(p + 1);
                let mut s = String::new();
                for i in 0..len {
                    if p + 3 + i >= end {
                        break;
                    }
                    s.push_str(&zx_char(at(p + 3 + i), false));
                }
                out.push(VariableEntry {
                    name: format!("{letter}$"),
                    kind: "string".to_string(),
                    value: json_string(&s),
                    offset: at_p,
                    size: (3 + len) as u32,
                });
                p += 3 + len;
            }
            0b010 | 0b110 => {
                let len = word(p + 1);
                let dims = usize::from(at(p + 3));
                if dims == 0 {
                    out.push(VariableEntry {
                        name: "?".to_string(),
                        kind: "error".to_string(),
                        value: "zero dimensions".to_string(),
                        offset: at_p,
                        size: 0,
                    });
                    return out;
                }
                let dim_list: Vec<usize> = (0..dims).map(|i| word(p + 4 + i * 2)).collect();
                let is_char = kind == 0b110;
                let elem_start = p + 4 + dims * 2;
                let total: usize = dim_list.iter().product();
                let preview = if is_char {
                    let mut s = String::new();
                    for i in 0..total.min(64) {
                        s.push_str(&zx_char(at(elem_start + i), false));
                    }
                    if total > 64 {
                        s.push('…');
                    }
                    json_string(&s)
                } else {
                    let vals: Vec<String> = (0..total.min(16))
                        .map(|i| format_number(decode_number(data, elem_start + i * 5)))
                        .collect();
                    format!("{}{}", vals.join(", "), if total > 16 { ", …" } else { "" })
                };
                let names: Vec<String> = dim_list.iter().map(|d| d.to_string()).collect();
                out.push(VariableEntry {
                    name: format!("{letter}{}({})", if is_char { "$" } else { "" }, names.join(",")),
                    kind: if is_char { "char array" } else { "number array" }.to_string(),
                    value: preview,
                    offset: at_p,
                    size: (3 + len) as u32,
                });
                p += 3 + len;
            }
            0b100 => {
                out.push(VariableEntry {
                    name: letter.to_string(),
                    kind: "number".to_string(),
                    value: format_number(decode_number(data, p + 1)),
                    offset: at_p,
                    size: 6,
                });
                p += 6;
            }
            0b101 => {
                let mut name = letter.to_string();
                let mut q = p + 1;
                while q < end {
                    let c = at(q);
                    q += 1;
                    let code = c & 0x7f;
                    name.push(char::from(if code == 0 { 0x3f } else { code }));
                    if c & 0x80 != 0 {
                        break;
                    }
                }
                out.push(VariableEntry {
                    name,
                    kind: "number".to_string(),
                    value: format_number(decode_number(data, q)),
                    offset: at_p,
                    size: (q + 5 - p) as u32,
                });
                p = q + 5;
            }
            0b111 => {
                let v = decode_number(data, p + 1);
                let lim = decode_number(data, p + 6);
                let step = decode_number(data, p + 11);
                let line = word(p + 16);
                let stmt = at(p + 18);
                out.push(VariableEntry {
                    name: letter.to_string(),
                    kind: "FOR control".to_string(),
                    value: format!(
                        "value {}, limit {}, step {}, loop line {line}:{stmt}",
                        format_number(v),
                        format_number(lim),
                        format_number(step)
                    ),
                    offset: at_p,
                    size: 19,
                });
                p += 19;
            }
            _ => {
                out.push(VariableEntry {
                    name: "?".to_string(),
                    kind: "garbage".to_string(),
                    value: format!("byte {:x} at {at_p}", h),
                    offset: at_p,
                    size: 1,
                });
                return out;
            }
        }
    }
    out
}
