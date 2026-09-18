//! ZX Spectrum character set to Unicode, the port of `src/spectrum/charset.ts`.

const BLOCKS: [&str; 16] = [" ", "▝", "▘", "▀", "▗", "▐", "▚", "▜", "▖", "▞", "▌", "▛", "▄", "▟", "▙", "█"];

pub const TOKENS: [&str; 91] = [
    "RND",
    "INKEY$",
    "PI",
    "FN",
    "POINT",
    "SCREEN$",
    "ATTR",
    "AT",
    "TAB",
    "VAL$",
    "CODE",
    "VAL",
    "LEN",
    "SIN",
    "COS",
    "TAN",
    "ASN",
    "ACS",
    "ATN",
    "LN",
    "EXP",
    "INT",
    "SQR",
    "SGN",
    "ABS",
    "PEEK",
    "IN",
    "USR",
    "STR$",
    "CHR$",
    "NOT",
    "BIN",
    "OR",
    "AND",
    "<=",
    ">=",
    "<>",
    "LINE",
    "THEN",
    "TO",
    "STEP",
    "DEF FN",
    "CAT",
    "FORMAT",
    "MOVE",
    "ERASE",
    "OPEN #",
    "CLOSE #",
    "MERGE",
    "VERIFY",
    "BEEP",
    "CIRCLE",
    "INK",
    "PAPER",
    "FLASH",
    "BRIGHT",
    "INVERSE",
    "OVER",
    "OUT",
    "LPRINT",
    "LLIST",
    "STOP",
    "READ",
    "DATA",
    "RESTORE",
    "NEW",
    "BORDER",
    "CONTINUE",
    "DIM",
    "REM",
    "FOR",
    "GO TO",
    "GO SUB",
    "INPUT",
    "LOAD",
    "LIST",
    "LET",
    "PAUSE",
    "NEXT",
    "POKE",
    "PRINT",
    "PLOT",
    "RUN",
    "SAVE",
    "RANDOMIZE",
    "IF",
    "CLS",
    "DRAW",
    "CLEAR",
    "RETURN",
    "COPY",
];

/// Name of a token byte (0xA5-0xFF), or 128k tokens for 0xA3/0xA4 when enabled.
pub fn token_name(code: u8, basic128: bool) -> Option<&'static str> {
    if code >= 0xa5 {
        return TOKENS.get(code as usize - 0xa5).copied();
    }
    if basic128 && code == 0xa3 {
        return Some("SPECTRUM");
    }
    if basic128 && code == 0xa4 {
        return Some("PLAY");
    }
    None
}

/// Printable form of a single ZX character, for the dump / text views.
pub fn zx_char(code: u8, expand_tokens: bool) -> String {
    match code {
        0x60 => "£".to_string(),
        0x7f => "©".to_string(),
        0x20..=0x7e => (code as char).to_string(),
        0x80..=0x8f => BLOCKS[code as usize - 0x80].to_string(),
        // circled letters for UDGs
        0x90..=0xa4 => char::from_u32(0x2460 + u32::from(code - 0x90)).unwrap().to_string(),
        _ if expand_tokens && code >= 0xa5 => TOKENS[code as usize - 0xa5].to_string(),
        _ => ".".to_string(),
    }
}

/// Single-cell character for the hex dump ASCII column.
pub fn dump_char(code: u8) -> String {
    match code {
        0x60 => "£".to_string(),
        0x7f => "©".to_string(),
        0x20..=0x7e => (code as char).to_string(),
        0x80..=0x8f => BLOCKS[code as usize - 0x80].to_string(),
        _ => "·".to_string(),
    }
}

/// All 256 characters at once. The hex dump asks per byte, which is no way to
/// cross into wasm, so the app builds the table once and indexes it.
pub fn char_table(kind: u32) -> Vec<String> {
    (0..=255u8)
        .map(|c| match kind {
            0 => zx_char(c, true),
            1 => zx_char(c, false),
            _ => dump_char(c),
        })
        .collect()
}
