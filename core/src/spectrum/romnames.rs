//! Names the disassembler puts beside numbers: the 48K ROM's system variables,
//! the error reports behind `RST 08`, the calculator's literals behind `RST 28`,
//! and symbols of the user's own.
//!
//! The names are the ones "The Complete Spectrum ROM Disassembly" uses.

use std::collections::HashMap;

/// Address, name, length.
const SYSVARS: [(u16, &str, u16); 69] = [
    (0x5c00, "KSTATE", 8),
    (0x5c08, "LAST-K", 1),
    (0x5c09, "REPDEL", 1),
    (0x5c0a, "REPPER", 1),
    (0x5c0b, "DEFADD", 2),
    (0x5c0d, "K-DATA", 1),
    (0x5c0e, "TVDATA", 2),
    (0x5c10, "STRMS", 38),
    (0x5c36, "CHARS", 2),
    (0x5c38, "RASP", 1),
    (0x5c39, "PIP", 1),
    (0x5c3a, "ERR-NR", 1),
    (0x5c3b, "FLAGS", 1),
    (0x5c3c, "TV-FLAG", 1),
    (0x5c3d, "ERR-SP", 2),
    (0x5c3f, "LIST-SP", 2),
    (0x5c41, "MODE", 1),
    (0x5c42, "NEWPPC", 2),
    (0x5c44, "NSPPC", 1),
    (0x5c45, "PPC", 2),
    (0x5c47, "SUBPPC", 1),
    (0x5c48, "BORDCR", 1),
    (0x5c49, "E-PPC", 2),
    (0x5c4b, "VARS", 2),
    (0x5c4d, "DEST", 2),
    (0x5c4f, "CHANS", 2),
    (0x5c51, "CURCHL", 2),
    (0x5c53, "PROG", 2),
    (0x5c55, "NXTLIN", 2),
    (0x5c57, "DATADD", 2),
    (0x5c59, "E-LINE", 2),
    (0x5c5b, "K-CUR", 2),
    (0x5c5d, "CH-ADD", 2),
    (0x5c5f, "X-PTR", 2),
    (0x5c61, "WORKSP", 2),
    (0x5c63, "STKBOT", 2),
    (0x5c65, "STKEND", 2),
    (0x5c67, "BREG", 1),
    (0x5c68, "MEM", 2),
    (0x5c6a, "FLAGS2", 1),
    (0x5c6b, "DF-SZ", 1),
    (0x5c6c, "S-TOP", 2),
    (0x5c6e, "OLDPPC", 2),
    (0x5c70, "OSPCC", 1),
    (0x5c71, "FLAGX", 1),
    (0x5c72, "STRLEN", 2),
    (0x5c74, "T-ADDR", 2),
    (0x5c76, "SEED", 2),
    (0x5c78, "FRAMES", 3),
    (0x5c7b, "UDG", 2),
    (0x5c7d, "COORDS", 2),
    (0x5c7f, "P-POSN", 1),
    (0x5c80, "PR-CC", 2),
    (0x5c82, "ECHO-E", 2),
    (0x5c84, "DF-CC", 2),
    (0x5c86, "DF-CCL", 2),
    (0x5c88, "S-POSN", 2),
    (0x5c8a, "S-POSNL", 2),
    (0x5c8c, "SCR-CT", 1),
    (0x5c8d, "ATTR-P", 1),
    (0x5c8e, "MASK-P", 1),
    (0x5c8f, "ATTR-T", 1),
    (0x5c90, "MASK-T", 1),
    (0x5c91, "P-FLAG", 1),
    (0x5c92, "MEMBOT", 30),
    (0x5cb0, "NMIADD", 2),
    (0x5cb2, "RAMTOP", 2),
    (0x5cb4, "P-RAMT", 2),
    // The end of the table, so a look-up past it finds nothing.
    (0x5cb6, "", 0),
];

/// Where the ROM keeps IY, which is how most of it reaches the variables.
pub const SYSVAR_IY: u32 = 0x5c3a;

/// The system variable at an address, `NAME+n` inside a longer one.
pub fn sysvar_name(addr: u32) -> Option<String> {
    let addr = u16::try_from(addr).ok()?;
    let (start, name, len) = SYSVARS.iter().rev().find(|(start, _, _)| *start <= addr)?;
    let into = addr - start;
    match into {
        _ if into >= *len => None,
        0 => Some(name.to_string()),
        n => Some(format!("{name}+{n}")),
    }
}

const REPORTS: [&str; 27] = [
    "1 NEXT without FOR",
    "2 Variable not found",
    "3 Subscript wrong",
    "4 Out of memory",
    "5 Out of screen",
    "6 Number too big",
    "7 RETURN without GOSUB",
    "8 End of file",
    "9 STOP statement",
    "A Invalid argument",
    "B Integer out of range",
    "C Nonsense in BASIC",
    "D BREAK - CONT repeats",
    "E Out of DATA",
    "F Invalid file name",
    "G No room for line",
    "H STOP in INPUT",
    "I FOR without NEXT",
    "J Invalid I/O device",
    "K Invalid colour",
    "L BREAK into program",
    "M RAMTOP no good",
    "N Statement lost",
    "O Invalid stream",
    "P FN without DEF",
    "Q Parameter error",
    "R Tape loading error",
];

/// The report for the byte that follows `RST 08`.
pub fn error_report(code: u8) -> Option<&'static str> {
    match code {
        0xff => Some("0 OK"),
        n => REPORTS.get(usize::from(n)).copied(),
    }
}

const LITERALS: [&str; 0x42] = [
    "jump-true",
    "exchange",
    "delete",
    "subtract",
    "multiply",
    "division",
    "to-power",
    "or",
    "no-&-no",
    "no-l-eql",
    "no-gr-eq",
    "nos-neql",
    "no-grtr",
    "no-less",
    "nos-eql",
    "addition",
    "str-&-no",
    "str-l-eql",
    "str-gr-eq",
    "strs-neql",
    "str-grtr",
    "str-less",
    "strs-eql",
    "strs-add",
    "val$",
    "usr-$",
    "read-in",
    "negate",
    "code",
    "val",
    "len",
    "sin",
    "cos",
    "tan",
    "asn",
    "acs",
    "atn",
    "ln",
    "exp",
    "int",
    "sqr",
    "sgn",
    "abs",
    "peek",
    "in",
    "usr-no",
    "str$",
    "chr$",
    "not",
    "duplicate",
    "n-mod-m",
    "jump",
    "stk-data",
    "dec-jr-nz",
    "less-0",
    "greater-0",
    "end-calc",
    "get-argt",
    "truncate",
    "fp-calc-2",
    "e-to-fp",
    "re-stack",
    "series-xx",
    "stk-const-xx",
    "st-mem-xx",
    "get-mem-xx",
];

pub const LIT_JUMP_TRUE: u8 = 0x00;
pub const LIT_JUMP: u8 = 0x33;
pub const LIT_STK_DATA: u8 = 0x34;
pub const LIT_DEC_JR_NZ: u8 = 0x35;
pub const LIT_END_CALC: u8 = 0x38;

/// The name of a calculator literal; the ones from 80 up carry a number.
pub fn literal_name(code: u8) -> String {
    let n = code & 0x1f;
    match code >> 5 {
        0b100 => format!("series-{n:02X}"),
        0b101 => match n {
            0 => "stk-zero".to_string(),
            1 => "stk-one".to_string(),
            2 => "stk-half".to_string(),
            3 => "stk-pi/2".to_string(),
            4 => "stk-ten".to_string(),
            _ => format!("stk-const-{n:02X}"),
        },
        0b110 => format!("st-mem-{n}"),
        0b111 => format!("get-mem-{n}"),
        _ => LITERALS.get(usize::from(code)).map_or_else(|| format!("literal {code:02X}"), |s| s.to_string()),
    }
}

/// How many constants follow a literal, each in `stk-data`'s packed form.
pub fn series_count(code: u8) -> usize {
    if code >> 5 == 0b100 {
        usize::from(code & 0x1f)
    } else {
        0
    }
}

// ---- the user's own --------------------------------------------------------

/// Addresses with names, from text of the form TAPER's `.SYM` files have: an
/// address and a name per line. The address is decimal, or hexadecimal behind
/// `$` or `0x`; anything after `;` is a comment.
#[derive(Clone, Debug, Default)]
pub struct Symbols(HashMap<u32, String>);

impl Symbols {
    /// The table, and the lines (from 1) that could not be read.
    pub fn parse(text: &str) -> (Symbols, Vec<u32>) {
        let mut table = HashMap::new();
        let mut bad = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split(';').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let address = parts.next().and_then(parse_address);
            match (address, parts.next(), parts.next()) {
                (Some(a), Some(name), None) => {
                    table.insert(a, name.to_string());
                }
                _ => bad.push(n as u32 + 1),
            }
        }
        (Symbols(table), bad)
    }

    pub fn get(&self, addr: u32) -> Option<&str> {
        self.0.get(&addr).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn parse_address(s: &str) -> Option<u32> {
    let hex = s.strip_prefix('$').or_else(|| s.strip_prefix("0x")).or_else(|| s.strip_prefix("0X"));
    let v = match hex {
        Some(h) => u32::from_str_radix(h, 16).ok()?,
        None => s.parse().ok()?,
    };
    (v <= 0xffff).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_variables_by_address() {
        assert_eq!(sysvar_name(0x5c3a).as_deref(), Some("ERR-NR"));
        assert_eq!(sysvar_name(0x5c79).as_deref(), Some("FRAMES+1"));
        assert_eq!(sysvar_name(0x5cb5).as_deref(), Some("P-RAMT+1"));
        assert_eq!(sysvar_name(0x5cb6), None);
        assert_eq!(sysvar_name(0x5bff), None);
        // The table has no holes and no overlaps.
        for pair in SYSVARS.windows(2) {
            assert_eq!(pair[0].0 + pair[0].2, pair[1].0, "{}", pair[0].1);
        }
    }

    #[test]
    fn reports_and_literals() {
        assert_eq!(error_report(0x1a), Some("R Tape loading error"));
        assert_eq!(error_report(0xff), Some("0 OK"));
        assert_eq!(error_report(0x1b), None);
        assert_eq!(literal_name(0x38), "end-calc");
        assert_eq!(literal_name(0x0f), "addition");
        assert_eq!(literal_name(0xa1), "stk-one");
        assert_eq!(literal_name(0xc3), "st-mem-3");
        assert_eq!(literal_name(0xe0), "get-mem-0");
        assert_eq!((literal_name(0x86), series_count(0x86)), ("series-06".to_string(), 6));
        assert_eq!(LITERALS[usize::from(LIT_JUMP)], "jump");
        assert_eq!(LITERALS[usize::from(LIT_STK_DATA)], "stk-data");
        assert_eq!(LITERALS[usize::from(LIT_DEC_JR_NZ)], "dec-jr-nz");
    }

    #[test]
    fn symbols_from_text() {
        let (s, bad) = Symbols::parse(
            "$8000 START\n32768+1 x\n0xBF55\tLD_BLOCK ; the loader\n\n; note\n70000 far\n49152 TOP",
        );
        assert_eq!(bad, [2, 6]);
        assert_eq!(s.get(0x8000), Some("START"));
        assert_eq!(s.get(0xbf55), Some("LD_BLOCK"));
        assert_eq!(s.get(49152), Some("TOP"));
        assert_eq!(s.get(1), None);
    }
}
