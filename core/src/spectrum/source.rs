// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! BASIC as text that can be typed, saved and turned back into a program: the
//! other direction from the lister in `basic.rs`.
//!
//! The lister draws what is there, hidden numbers and all, and is free to lose
//! things. This text has to come back, so it is its own format: a line number,
//! keywords spelled out, and `{...}` for what a keyboard cannot type. After
//! TAPER's `.BAS` files, which it still reads:
//!
//! ```text
//! {1F}            a byte, two hex digits
//! {A} .. {U}      a user defined graphic
//! {-1} .. {+8}    a block graphic by its key, shifted with + (or the character itself)
//! {INK 5}         a colour control and its operand; PAPER FLASH BRIGHT INVERSE OVER
//! {AT 2,5}        likewise, and TAB, with two
//! {PRINT}         a keyword as its token where it would otherwise be letters: in a
//!                 string or after REM
//! {=1000}         the number the line really uses, after the digits it shows
//! {=}             digits with no number behind them at all
//! {(C)}           the copyright sign (or the character itself, as with the pound)
//! ```
//!
//! A number typed outside a string gets its five byte form behind it, as the
//! Spectrum's own editor does; so do the parameters of a DEF FN.
//!
//! **Lines that were not touched keep their bytes.** [`edit_basic`] tokenises only
//! the lines whose text differs from what [`basic_source`] wrote. That keeps editing
//! a protected loader safe: odd line lengths, numbers the ROM would round differently
//! and bytes no keyboard produces survive on every line left alone.

use super::basic::{decode_number, format_number};
use super::charset::{token_name, TOKENS};
use super::syntax::check_line;

#[derive(Clone, Copy, Debug, Default)]
pub struct SourceOptions {
    /// `SPECTRUM` and `PLAY` are keywords rather than the graphics T and U.
    pub basic128: bool,
    /// Take `print` for `PRINT`. Off, a program can use lower case names that
    /// happen to spell a keyword, which is how the Spectrum itself tells them apart.
    pub any_case: bool,
    /// Hold a line that is tokenised to the 48K ROM's syntax as well as its
    /// spelling. Off in [`Default`], as the disassembler's extras are; both data
    /// windows turn it on.
    pub check_syntax: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLine {
    pub text: String,
    /// Where the line starts in the data, and how many bytes it takes there.
    pub offset: usize,
    pub len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceError {
    /// Line of the text, from 1.
    pub line: u32,
    pub message: String,
}

const COLOURS: [&str; 6] = ["INK", "PAPER", "FLASH", "BRIGHT", "INVERSE", "OVER"];
const BLOCKS: [char; 16] = [' ', '▝', '▘', '▀', '▗', '▐', '▚', '▜', '▖', '▞', '▌', '▛', '▄', '▟', '▙', '█'];
const REM: u8 = 0xea;
const BIN: u8 = 0xc4;
const DEF_FN: u8 = 0xce;

// ---- program to text -------------------------------------------------------

/// How many of these bytes are one number as it is typed: digits, a point, an
/// exponent — or, after BIN, noughts and ones. A point on its own is not one.
pub(super) fn number_len(b: &[u8], bin: bool) -> usize {
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    if bin {
        return b.iter().take_while(|c| matches!(c, b'0' | b'1')).count();
    }
    let mut n = digits(0);
    if b.get(n) == Some(&b'.') {
        let fraction = digits(n + 1);
        if n == 0 && fraction == 0 {
            return 0;
        }
        n += 1 + fraction;
    }
    if matches!(b.get(n), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(b.get(n + 1), Some(b'+' | b'-')));
        let exponent = digits((n + 1 + sign).min(b.len()));
        if exponent > 0 {
            n += 1 + sign + exponent;
        }
    }
    n
}

/// The longest tail of `s` that reads as a number.
fn trailing_number(s: &str) -> Option<f64> {
    (0..s.len()).filter(|i| s.is_char_boundary(*i)).find_map(|i| {
        let tail = &s[i..];
        (tail.starts_with(|c: char| c.is_ascii_digit() || c == '.'))
            .then(|| tail.parse::<f64>().ok())
            .flatten()
    })
}

/// The program between `start` and `end` of `data`, one entry per line.
pub fn basic_source(data: &[u8], start: usize, end: usize, opts: SourceOptions) -> Vec<SourceLine> {
    let end = end.min(data.len());
    let mut lines = Vec::new();
    let mut p = start;
    while p + 4 <= end {
        let number = usize::from(data[p]) << 8 | usize::from(data[p + 1]);
        if number >= 0x4000 {
            break;
        }
        let length = usize::from(data[p + 2]) | usize::from(data[p + 3]) << 8;
        let body_end = end.min(p + 4 + length);
        let text = format!("{number:>4} {}", line_source(&data[p + 4..body_end], opts));
        lines.push(SourceLine { text: text.trim_end().to_string(), offset: p, len: body_end - p });
        p = body_end;
        if length == 0 {
            break;
        }
    }
    lines
}

/// The same as one text.
pub fn basic_source_text(data: &[u8], start: usize, end: usize, opts: SourceOptions) -> String {
    let lines: Vec<String> = basic_source(data, start, end, opts).into_iter().map(|l| l.text).collect();
    lines.join("\n")
}

fn line_source(body: &[u8], opts: SourceOptions) -> String {
    let mut out = String::new();
    // Text since the last keyword, which is where a number's digits are.
    let mut run = String::new();
    let mut after_token = true;
    let mut in_string = false;
    let mut in_rem = false;
    // Between DEF FN and the end of its parameters every name has a number behind it.
    let mut in_def_fn = false;
    // After BIN the digits count in twos.
    let mut after_bin = false;
    // In a name, where digits are part of it; and what is left of a number being
    // written, with whether the program has its five bytes behind it.
    let mut in_name = false;
    let mut number: Option<(usize, bool)> = None;
    let mut q = 0;
    while q < body.len() {
        let c = body[q];
        q += 1;
        if c == 0x0d {
            break;
        }
        // A 0E with fewer than five bytes behind it is no number: it goes out as the byte it is.
        if c == 0x0e && !in_string && !in_rem && q + 5 <= body.len() {
            let v = decode_number(body, q);
            let shown = if after_bin {
                u16::from_str_radix(run.trim(), 2).ok().map(f64::from)
            } else {
                trailing_number(&run)
            };
            let same = shown.is_some_and(|t| (t - v).abs() <= 1e-6 * 1f64.max(v.abs()));
            if !same && !in_def_fn {
                out.push_str(&format!("{{={}}}", format_number(v)));
            }
            q += 5;
            run.clear();
            after_token = false;
            continue;
        }
        if let Some(name) = token_name(c, opts.basic128) {
            if in_string || in_rem {
                out.push_str(&format!("{{{name}}}"));
            } else {
                if !after_token {
                    out.push(' ');
                }
                out.push_str(name);
                out.push(' ');
                after_token = true;
                in_rem = c == REM;
                in_def_fn = c == DEF_FN;
                after_bin = c == BIN;
                in_name = false;
                run.clear();
            }
            continue;
        }
        after_token = false;
        match c {
            0x10..=0x15 if q < body.len() => {
                out.push_str(&format!("{{{} {}}}", COLOURS[usize::from(c) - 0x10], body[q]));
                q += 1;
            }
            0x16 | 0x17 if q + 1 < body.len() => {
                out.push_str(&format!(
                    "{{{} {},{}}}",
                    if c == 0x16 { "AT" } else { "TAB" },
                    body[q],
                    body[q + 1]
                ));
                q += 2;
            }
            0x60 => out.push('£'),
            0x7f => out.push('©'),
            b'"' => {
                in_string = !in_string && !in_rem;
                out.push('"');
            }
            // An opening brace starts an escape, so it cannot stand for itself.
            0x20..=0x7e if c != b'{' => {
                let plain = !in_string && !in_rem;
                if plain && number.is_none() && !in_name && (c.is_ascii_digit() || c == b'.') {
                    let len = number_len(&body[q - 1..], after_bin);
                    let hidden = body.get(q - 1 + len) == Some(&0x0e) && q + len + 4 <= body.len();
                    number = (len > 0).then_some((len, hidden));
                }
                out.push(char::from(c));
                run.push(char::from(c));
                if let Some((left, hidden)) = number {
                    number = (left > 1).then_some((left - 1, hidden));
                    if left == 1 && !hidden {
                        out.push_str("{=}");
                    }
                }
                if c == b')' {
                    in_def_fn = false;
                }
                in_name = plain && (c.is_ascii_alphabetic() || (in_name && c.is_ascii_digit()));
                continue;
            }
            // 80 is a blank square, which as text would be a space.
            0x81..=0x8f => out.push(BLOCKS[usize::from(c) - 0x80]),
            0x90..=0xa4 => out.push_str(&format!("{{{}}}", char::from(b'A' + c - 0x90))),
            _ => out.push_str(&format!("{{{c:02X}}}")),
        }
        run.clear();
        in_name = false;
    }
    // A space that ends the line would be trimmed off the text: spell it out.
    // The space a keyword brings along is the listing's, not the program's, and may go.
    if !after_token && out.ends_with(' ') {
        out.pop();
        out.push_str("{20}");
    }
    out
}

// ---- text to program -------------------------------------------------------

/// A number in the Spectrum's five bytes: the short form for whole numbers up to
/// 65535 either way, floating point otherwise. `None` when it is out of range.
pub fn encode_number(v: f64) -> Option<[u8; 5]> {
    if !v.is_finite() {
        return None;
    }
    if v.fract() == 0.0 && v.abs() <= 65535.0 {
        let n = (v as i32).rem_euclid(65536) as u16;
        return Some([0, if v < 0.0 { 0xff } else { 0 }, n as u8, (n >> 8) as u8, 0]);
    }
    let mut exp = v.abs().log2().floor() as i32 + 1;
    let mut mant = (v.abs() / 2f64.powi(exp) * 4294967296.0).round();
    if mant >= 4294967296.0 {
        mant /= 2.0;
        exp += 1;
    }
    if !(-127..=127).contains(&exp) {
        return None;
    }
    let m = mant as u32;
    let sign = if v < 0.0 { 0x80 } else { 0 };
    Some([(exp + 128) as u8, (m >> 24) as u8 & 0x7f | sign, (m >> 16) as u8, (m >> 8) as u8, m as u8])
}

/// Every keyword with its token, longest first so `GO SUB` wins over `GO TO`'s
/// start and `VAL$` over `VAL`. `GOTO` and `GOSUB` are what people type.
fn keywords(opts: SourceOptions) -> Vec<(String, u8)> {
    let mut all: Vec<(String, u8)> =
        TOKENS.iter().enumerate().map(|(n, name)| (name.to_string(), 0xa5 + n as u8)).collect();
    all.push(("GOTO".to_string(), 0xec));
    all.push(("GOSUB".to_string(), 0xed));
    if opts.basic128 {
        all.push(("SPECTRUM".to_string(), 0xa3));
        all.push(("PLAY".to_string(), 0xa4));
    }
    all.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
    all
}

struct Tokeniser<'a> {
    src: Vec<char>,
    at: usize,
    out: Vec<u8>,
    keywords: &'a [(String, u8)],
    any_case: bool,
    /// Nothing but keywords so far, or the last thing written was one.
    after_token: bool,
    /// The last byte is a space that was typed, and what came before it.
    typed_space: Option<bool>,
    in_string: bool,
    in_rem: bool,
    /// In a name, where digits are part of it rather than a number.
    in_name: bool,
    after_bin: bool,
    /// `DEF FN f(`: parameters get a placeholder number each.
    def_fn: DefFn,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DefFn {
    No,
    /// Seen the keyword, waiting for the bracket.
    Keyword,
    Parameters,
}

impl Tokeniser<'_> {
    fn keyword_here(&self) -> Option<(usize, u8)> {
        let rest = &self.src[self.at..];
        let prev_is_letter = self.at > 0 && self.src[self.at - 1].is_ascii_alphabetic();
        for (name, token) in self.keywords {
            let len = name.chars().count();
            if rest.len() < len {
                continue;
            }
            let matches =
                name.chars().zip(rest).all(
                    |(k, c)| {
                        if self.any_case {
                            k.eq_ignore_ascii_case(c)
                        } else {
                            k == *c
                        }
                    },
                );
            if !matches {
                continue;
            }
            // A word, not part of one: SCORE holds no OR, INTO no INT.
            let wordy_start = name.starts_with(|c: char| c.is_ascii_alphabetic());
            let wordy_end = name.ends_with(|c: char| c.is_ascii_alphabetic());
            if wordy_start && prev_is_letter {
                continue;
            }
            if wordy_end && rest.get(len).is_some_and(char::is_ascii_alphabetic) {
                continue;
            }
            return Some((len, *token));
        }
        None
    }

    fn literal(&mut self, byte: u8) {
        self.out.push(byte);
        self.after_token = false;
        self.typed_space = None;
    }

    fn hidden(&mut self, v: f64) -> Result<(), String> {
        let bytes = encode_number(v).ok_or("Number too big")?;
        self.out.push(0x0e);
        self.out.extend_from_slice(&bytes);
        Ok(())
    }

    /// What stands between braces, from just past the opening one.
    fn escape(&mut self) -> Result<(), String> {
        let close = self.src[self.at..].iter().position(|c| *c == '}').ok_or("{ without }")?;
        let body: String = self.src[self.at..self.at + close].iter().collect();
        self.at += close + 1;
        let upper = body.trim().to_ascii_uppercase();
        let b = upper.as_bytes();
        if upper == "=" {
            return Ok(());
        }
        if let Some(number) = upper.strip_prefix('=') {
            let v: f64 = number.trim().parse().map_err(|_| format!("{{{body}}}: not a number"))?;
            return self.hidden(v);
        }
        if b.len() == 2 && b.iter().all(u8::is_ascii_hexdigit) {
            let byte = u8::from_str_radix(&upper, 16).unwrap();
            self.literal(byte);
            return Ok(());
        }
        if b.len() == 1 && (b'A'..=b'U').contains(&b[0]) {
            self.literal(0x90 + b[0] - b'A');
            return Ok(());
        }
        if b.len() == 2 && (b'1'..=b'8').contains(&b[1]) && (b[0] == b'-' || b[0] == b'+') {
            let key = (b[1] - b'0') % 8;
            self.literal(if b[0] == b'-' { 0x80 + key } else { 0x88 + (key ^ 7) });
            return Ok(());
        }
        if upper == "(C)" {
            self.literal(0x7f);
            return Ok(());
        }
        if let Some((name, operands)) = upper.split_once(' ') {
            let values: Result<Vec<u8>, _> = operands.split(',').map(|v| v.trim().parse::<u8>()).collect();
            let code = COLOURS.iter().position(|c| *c == name).map(|n| (0x10 + n as u8, 1)).or(match name {
                "AT" => Some((0x16, 2)),
                "TAB" => Some((0x17, 2)),
                _ => None,
            });
            if let (Some((code, count)), Ok(values)) = (code, values) {
                if values.len() != count {
                    return Err(format!("{{{body}}}: {name} takes {count} value(s)"));
                }
                self.literal(code);
                for v in values {
                    self.literal(v);
                }
                return Ok(());
            }
        }
        if let Some((_, token)) = self.keywords.iter().find(|(name, _)| *name == upper) {
            self.literal(*token);
            return Ok(());
        }
        Err(format!("{{{body}}} is not something a line can hold"))
    }

    /// Digits, a point, an exponent: `start` is where they begin.
    fn number(&mut self) -> Result<(), String> {
        let start = self.at;
        // The same measure the writer takes, so the two agree on what a number is.
        let ascii: Vec<u8> =
            self.src[start..].iter().take(80).map(|c| if c.is_ascii() { *c as u8 } else { 0 }).collect();
        let len = number_len(&ascii, self.after_bin);
        if len == 0 {
            // A point on its own, or BIN with something other than its digits behind it.
            self.at += 1;
            self.after_bin = false;
            self.literal(ascii[0]);
            return Ok(());
        }
        self.at += len;
        let text: String = self.src[start..self.at].iter().collect();
        let value = if self.after_bin {
            if len > 16 {
                return Err("BIN takes 16 digits at most".to_string());
            }
            f64::from(u16::from_str_radix(&text, 2).unwrap_or(0))
        } else {
            text.parse::<f64>().map_err(|_| format!("{text} is not a number"))?
        };
        for i in start..self.at {
            let c = self.src[i] as u8;
            self.literal(c);
        }
        self.after_bin = false;
        // {=...} right behind the digits says what the number really is.
        if self.src.get(self.at) == Some(&'{') && self.src.get(self.at + 1) == Some(&'=') {
            return Ok(());
        }
        self.hidden(value)
    }

    fn run(&mut self) -> Result<(), String> {
        while self.at < self.src.len() {
            let c = self.src[self.at];
            if c == '{' {
                self.at += 1;
                self.escape()?;
                continue;
            }
            let plain = !self.in_string && !self.in_rem;
            if plain {
                if let Some((len, token)) = self.keyword_here() {
                    // The space a listing puts before a keyword is not in the program.
                    if self.typed_space == Some(false) {
                        self.out.pop();
                    }
                    self.at += len;
                    self.out.push(token);
                    if self.src.get(self.at) == Some(&' ') {
                        self.at += 1;
                    }
                    self.after_token = true;
                    self.typed_space = None;
                    self.in_name = false;
                    self.in_rem = token == REM;
                    self.after_bin = token == BIN;
                    if token == DEF_FN {
                        self.def_fn = DefFn::Keyword;
                    }
                    continue;
                }
                if (c.is_ascii_digit() || c == '.') && !self.in_name {
                    self.number()?;
                    continue;
                }
            }
            self.at += 1;
            let byte = match c {
                '£' | '`' => 0x60,
                '©' => 0x7f,
                '\t' => b' ',
                ' '..='~' => c as u8,
                // The lister's circled letters for the graphics A to U.
                '\u{2460}'..='\u{2474}' => 0x90 + (c as u32 - 0x2460) as u8,
                _ => match BLOCKS.iter().position(|b| *b == c) {
                    Some(n) => 0x80 + n as u8,
                    None => return Err(format!("'{c}' is not a Spectrum character")),
                },
            };
            if plain && self.def_fn != DefFn::No {
                match (self.def_fn, c) {
                    (DefFn::Keyword, '(') => self.def_fn = DefFn::Parameters,
                    (DefFn::Keyword, '=') => self.def_fn = DefFn::No,
                    // The end of a name: a comma, or the bracket that closes the list.
                    (DefFn::Parameters, ',' | ')') => {
                        if self.out.last().is_some_and(|b| b.is_ascii_alphabetic() || *b == b'$') {
                            self.hidden(0.0)?;
                        }
                        if c == ')' {
                            self.def_fn = DefFn::No;
                        }
                    }
                    _ => {}
                }
            }
            let before = self.after_token;
            self.literal(byte);
            if c == ' ' {
                self.typed_space = Some(before);
            }
            if c == '"' && !self.in_rem {
                self.in_string = !self.in_string;
            }
            self.in_name = plain && (c.is_ascii_alphabetic() || (self.in_name && c.is_ascii_digit()));
        }
        Ok(())
    }
}

/// One line of text as the bytes of a program line: number, length, body, ENTER.
pub fn tokenise_line(text: &str, opts: SourceOptions) -> Result<Vec<u8>, String> {
    tokenise_with(text, &keywords(opts), opts)
}

fn tokenise_with(text: &str, keywords: &[(String, u8)], opts: SourceOptions) -> Result<Vec<u8>, String> {
    let trimmed = text.trim_start();
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return Err("A line starts with its number".to_string());
    }
    let number: u32 = trimmed[..digits].parse().map_err(|_| "Line numbers go up to 9999".to_string())?;
    if number > 9999 {
        return Err("Line numbers go up to 9999".to_string());
    }
    let src: Vec<char> = trimmed[digits..].trim_start().trim_end().chars().collect();
    let mut t = Tokeniser {
        src,
        at: 0,
        out: Vec::new(),
        keywords,
        any_case: opts.any_case,
        after_token: true,
        typed_space: None,
        in_string: false,
        in_rem: false,
        in_name: false,
        after_bin: false,
        def_fn: DefFn::No,
    };
    t.run()?;
    if t.in_string {
        return Err("A string is not closed".to_string());
    }
    let mut body = t.out;
    body.push(0x0d);
    if body.len() > 0xffff {
        return Err("The line is too long".to_string());
    }
    if opts.check_syntax {
        check_line(&body, opts.basic128)?;
    }
    let mut line = vec![(number >> 8) as u8, number as u8, body.len() as u8, (body.len() >> 8) as u8];
    line.extend_from_slice(&body);
    Ok(line)
}

/// The program area that `text` stands for, given the one it was made from:
/// `data[start..end]`.
///
/// Lines still as [`basic_source`] wrote them keep their bytes; the rest is
/// tokenised. Lines stay in the order they are written in.
pub fn edit_basic(
    data: &[u8],
    start: usize,
    end: usize,
    text: &str,
    opts: SourceOptions,
) -> Result<Vec<u8>, Vec<SourceError>> {
    let original = basic_source(data, start, end, opts);
    let keywords = keywords(opts);
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if let Some(kept) = original.iter().find(|o| o.text == line) {
            out.extend_from_slice(&data[kept.offset..kept.offset + kept.len]);
            continue;
        }
        match tokenise_with(line, &keywords, opts) {
            Ok(bytes) => out.extend_from_slice(&bytes),
            Err(message) => errors.push(SourceError { line: n as u32 + 1, message }),
        }
    }
    if out.len() > 0xffff {
        errors.push(SourceError {
            line: 0,
            message: "The program is longer than a tape block can say".to_string(),
        });
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTS: SourceOptions = SourceOptions { basic128: false, any_case: false, check_syntax: false };

    fn body(text: &str) -> Vec<u8> {
        let line = tokenise_line(text, OPTS).unwrap();
        line[4..].to_vec()
    }

    fn small(n: u16) -> Vec<u8> {
        vec![0x0e, 0, 0, n as u8, (n >> 8) as u8, 0]
    }

    #[test]
    fn keywords_lose_the_spaces_a_listing_gives_them() {
        let mut want = vec![0xf5, b'a', 0xc6, b'b', 0x0d]; // PRINT a AND b
        assert_eq!(body("10 PRINT a AND b"), want);
        // two typed spaces: one is the listing's, one is the program's
        want.insert(2, b' ');
        assert_eq!(body("10 PRINT a  AND b"), want);
        assert_eq!(body("10 GOTO 5")[0], 0xec);
        assert_eq!(body("10 GO SUB 5")[0], 0xed);
    }

    #[test]
    fn names_that_hold_a_keyword_stay_names() {
        assert_eq!(body("10 LET SCORE=INTO")[..7], [0xf1, b'S', b'C', b'O', b'R', b'E', b'=']);
        assert_eq!(&body("10 LET SCORE=INTO")[7..11], b"INTO");
        // lower case is never a keyword unless asked for
        assert_eq!(body("10 print"), b"print\x0d");
        let any = SourceOptions { any_case: true, ..OPTS };
        assert_eq!(tokenise_line("10 print", any).unwrap()[4..], [0xf5, 0x0d]);
    }

    #[test]
    fn numbers_get_their_five_bytes() {
        let mut want = vec![0xec, b'1', b'0'];
        want.extend(small(10));
        want.push(0x0d);
        assert_eq!(body("20 GO TO 10"), want);
        // a digit in a name is not a number
        assert_eq!(body("20 LET a1=b"), [&[0xf1][..], b"a1=b\x0d"].concat());
        // BIN counts in twos
        let bin = body("20 LET a=BIN 101");
        assert_eq!(&bin[bin.len() - 7..bin.len() - 1], &small(5)[..]);
        // and {=} overrides what the digits say
        let hidden = body("20 GO TO 10{=1000}");
        assert_eq!(&hidden[3..9], &small(1000)[..]);
    }

    #[test]
    fn floating_point_matches_the_rom_for_exact_values() {
        assert_eq!(encode_number(0.5), Some([0x80, 0, 0, 0, 0]));
        assert_eq!(encode_number(-0.5), Some([0x80, 0x80, 0, 0, 0]));
        assert_eq!(encode_number(65536.0), Some([0x91, 0, 0, 0, 0]));
        assert_eq!(encode_number(-3.0), Some([0, 0xff, 0xfd, 0xff, 0]));
        assert_eq!(encode_number(1e40), None);
        for v in [0.1, 7.25, 1e10, 123456.789, 1e-20] {
            let back = decode_number(&encode_number(v).unwrap(), 0);
            assert!((back - v).abs() <= v.abs() * 1e-9, "{v} came back as {back}");
        }
    }

    #[test]
    fn strings_and_rem_are_left_alone() {
        assert_eq!(body("10 PRINT \"GO TO 10\""), [&[0xf5][..], b"\"GO TO 10\"\x0d"].concat());
        assert_eq!(body("10 REM PRINT 10"), [&[0xea][..], b"PRINT 10\x0d"].concat());
        assert_eq!(body("10 REM {PRINT}{1F}"), [0xea, 0xf5, 0x1f, 0x0d]);
        assert!(tokenise_line("10 PRINT \"open", OPTS).is_err());
    }

    #[test]
    fn escapes() {
        assert_eq!(
            body("10 REM {INK 5}{AT 2,5}{A}{u}{-1}{+1}{(C)}£©▛{7B}"),
            [0xea, 0x10, 5, 0x16, 2, 5, 0x90, 0xa4, 0x81, 0x8e, 0x7f, 0x60, 0x7f, 0x8b, 0x7b, 0x0d]
        );
        for bad in ["10 REM {", "10 REM {nope}", "10 REM {AT 1}", "10 REM é", "PRINT", "10000 CLS"] {
            assert!(tokenise_line(bad, OPTS).is_err(), "{bad}");
        }
    }

    #[test]
    fn def_fn_parameters_get_placeholders() {
        let line = body("10 DEF FN f(x,y$)=x");
        let zero = [0x0e, 0, 0, 0, 0, 0];
        let want = [&[0xce][..], b"f(x", &zero, b",y$", &zero, b")=x\x0d"].concat();
        assert_eq!(line, want);
    }

    /// Whatever a program holds, its text comes back as the same bytes — through
    /// the tokeniser, not the kept lines, which is the harder way round.
    #[test]
    fn a_program_survives_being_text() {
        let lines = [
            "10 REM {INK 5}hello{7B}",
            "20 LET a=10: LET b$=\"x{PRINT}y\"+CHR$ 65",
            "30 IF a<=10 AND b$<>\"\" THEN GO TO 100",
            "40 DEF FN s(a,b)=a+b*2.5",
            "50 PRINT AT 1,2;\"▛{A}£\";TAB 5;BIN 1100",
            "60 RANDOMIZE USR (PEEK 23627+256*PEEK 23628)",
            "70 GO TO 10{=1000}",
        ];
        let program: Vec<u8> = lines.iter().flat_map(|l| tokenise_line(l, OPTS).unwrap()).collect();
        let text = basic_source_text(&program, 0, program.len(), OPTS);
        let again: Vec<u8> = text.lines().flat_map(|l| tokenise_line(l, OPTS).unwrap()).collect();
        assert_eq!(again, program, "{text}");
        // and the text settles: written twice, it reads the same
        assert_eq!(basic_source_text(&again, 0, again.len(), OPTS), text);
    }

    #[test]
    fn untouched_lines_keep_bytes_no_keyboard_makes() {
        // A number the ROM rounded its own way, and a line length that lies: it goes
        // last, because a length like that takes in everything after it.
        let mut odd = vec![0, 30, 0xff, 0xff, 0xf5, b'1'];
        odd.extend_from_slice(&[0x0e, 0x7d, 0x4c, 0xcc, 0xcc, 0xcc, 0x0d]);
        let plain = tokenise_line("20 CLS", OPTS).unwrap();
        let program = [plain, odd.clone()].concat();
        let text = basic_source_text(&program, 0, program.len(), OPTS);

        // Nothing changed: nothing changes.
        assert_eq!(edit_basic(&program, 0, program.len(), &text, OPTS).unwrap(), program);

        // One line is changed and one added; the odd one keeps its bytes.
        let edited = text.replace("20 CLS", "20 BORDER 1\n\n  25 CLS");
        let out = edit_basic(&program, 0, program.len(), &edited, OPTS).unwrap();
        let fresh =
            [tokenise_line("20 BORDER 1", OPTS).unwrap(), tokenise_line("25 CLS", OPTS).unwrap()].concat();
        assert_eq!(out, [fresh, odd].concat());

        let errors = edit_basic(&program, 0, program.len(), "10 CLS\nCLS\n20 REM {x", OPTS).unwrap_err();
        assert_eq!(errors.iter().map(|e| e.line).collect::<Vec<_>>(), [2, 3]);
    }
}
