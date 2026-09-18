//! What a data block contains: a ROM header, a BASIC program, a screen, machine
//! code, an array or plain data. The port of `src/tzx/content.ts`: it uses the
//! preceding header when there is one, otherwise heuristics on the bytes.

use crate::describe::{decode_header, HeaderInfo};
use crate::types::{Block, Body};

pub const SCREEN_SIZE: usize = 6912;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentKind {
    Header,
    Basic,
    Screen,
    Code,
    Array,
    Data,
    Empty,
}

impl ContentKind {
    pub fn name(self) -> &'static str {
        match self {
            ContentKind::Header => "header",
            ContentKind::Basic => "basic",
            ContentKind::Screen => "screen",
            ContentKind::Code => "code",
            ContentKind::Array => "array",
            ContentKind::Data => "data",
            ContentKind::Empty => "empty",
        }
    }
}

/// Whether the detection came from a preceding header or from the bytes themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Header,
    Heuristic,
    None,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Header => "header",
            Source::Heuristic => "heuristic",
            Source::None => "none",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentInfo {
    pub kind: ContentKind,
    /// Short label for the block list, e.g. "BASIC", "SCREEN", "CODE 32768".
    pub label: String,
    /// Load address of the body in Spectrum memory (best guess).
    pub base: u16,
    /// Bytes to skip at the start (flag byte) and drop at the end (checksum).
    pub skip_flag: bool,
    pub skip_checksum: bool,
    /// Program length from the header, when known (BASIC: where VARS start).
    pub prog_len: Option<u16>,
    /// Decoded header when kind is `Header`.
    pub header: Option<HeaderInfo>,
    pub source: Source,
    /// Body length announced by the preceding header, when that header was used.
    pub expected_length: Option<u16>,
}

impl Default for ContentInfo {
    fn default() -> Self {
        ContentInfo {
            kind: ContentKind::Data,
            label: String::new(),
            base: 0x8000,
            skip_flag: false,
            skip_checksum: false,
            prog_len: None,
            header: None,
            source: Source::None,
            expected_length: None,
        }
    }
}

/// Body of a data block after removing flag/checksum bytes. The TypeScript keeps
/// its own copy of this slice for the editor; a differential test pins them.
pub fn block_body(data: &[u8], skip_flag: bool, skip_checksum: bool) -> &[u8] {
    let mut d = data;
    if skip_flag && !d.is_empty() {
        d = &d[1..];
    }
    if skip_checksum && !d.is_empty() {
        d = &d[..d.len() - 1];
    }
    d
}

/// Does the byte stream look like a BASIC program area? Returns the fraction of
/// bytes that parse as lines.
pub fn basic_score(d: &[u8]) -> f64 {
    let mut p = 0usize;
    let mut lines = 0u32;
    let mut last_no: i32 = -1;
    while p + 4 <= d.len() {
        let no = (i32::from(d[p]) << 8) | i32::from(d[p + 1]);
        let len = usize::from(d[p + 2]) | usize::from(d[p + 3]) << 8;
        if no > 9999 || no <= last_no || len == 0 || p + 4 + len > d.len() {
            break;
        }
        if d[p + 4 + len - 1] != 0x0d {
            break;
        }
        last_no = no;
        lines += 1;
        p += 4 + len;
    }
    if lines == 0 {
        return 0.0;
    }
    // A program is usually followed by the variables area; count the parsed part.
    p as f64 / d.len() as f64
}

fn looks_like_screen(len: usize) -> bool {
    len == SCREEN_SIZE
}

/// The data of a block that carries some, or `None` for the block types that
/// content detection ignores.
fn data_of(b: &Block) -> Option<&[u8]> {
    match &b.body {
        Body::Standard { data, .. }
        | Body::Turbo { data, .. }
        | Body::PureData { data, .. }
        | Body::Direct { data, .. }
        | Body::Generalized { data, .. } => Some(data),
        _ => None,
    }
}

/// Header-carrying block types: standard, turbo and generalized.
fn may_hold_header(b: &Block) -> bool {
    matches!(b.body, Body::Standard { .. } | Body::Turbo { .. } | Body::Generalized { .. })
}

/// Analyse block `index`; the previous block is consulted for a ROM header.
pub fn detect_content(blocks: &[Block], index: usize) -> ContentInfo {
    let Some(b) = blocks.get(index) else { return ContentInfo::default() };
    let Some(data) = data_of(b) else { return ContentInfo::default() };
    if data.is_empty() {
        return ContentInfo { kind: ContentKind::Empty, ..ContentInfo::default() };
    }

    // ROM header block?
    if let Some(own) = decode_header(data) {
        if may_hold_header(b) {
            return ContentInfo {
                kind: ContentKind::Header,
                label: "header".to_string(),
                header: Some(own),
                skip_flag: true,
                skip_checksum: true,
                source: Source::Header,
                ..ContentInfo::default()
            };
        }
    }

    // Flag/checksum convention: standard/turbo blocks always have them; other block types only
    // when the first byte is a ROM-style flag.
    let has_flag =
        matches!(b.body, Body::Standard { .. } | Body::Turbo { .. }) || data[0] == 0xff || data[0] == 0x00;
    let body = block_body(data, has_flag, has_flag);

    // Preceding header tells us what this is. A length mismatch (truncated or
    // over-long block) still uses the header, marked in the label.
    let hdr = index
        .checked_sub(1)
        .and_then(|i| blocks.get(i))
        .filter(|p| may_hold_header(p))
        .and_then(|p| data_of(p))
        .and_then(decode_header);
    if let Some(hdr) = hdr {
        if usize::from(hdr.length) == body.len() || !looks_like_screen(body.len()) {
            let suffix = if body.len() < usize::from(hdr.length) {
                " (short)"
            } else if body.len() > usize::from(hdr.length) {
                " (long)"
            } else {
                ""
            };
            let from_hdr = ContentInfo {
                skip_flag: has_flag,
                skip_checksum: has_flag,
                source: Source::Header,
                expected_length: Some(hdr.length),
                ..ContentInfo::default()
            };
            match hdr.kind {
                0 => {
                    let limit = usize::from(hdr.length).min(body.len());
                    return ContentInfo {
                        kind: ContentKind::Basic,
                        label: format!("BASIC{suffix}"),
                        base: 23755,
                        prog_len: if usize::from(hdr.param2) <= limit { Some(hdr.param2) } else { None },
                        ..from_hdr
                    };
                }
                3 => {
                    // A 6912-byte CODE block is a screen even when loaded elsewhere (loaders often
                    // load it out of sight and copy it to 16384); view it at the screen address
                    // either way.
                    if looks_like_screen(usize::from(hdr.length)) {
                        let label = if hdr.param1 == 16384 {
                            format!("SCREEN{suffix}")
                        } else {
                            format!("SCREEN {}{suffix}", hdr.param1)
                        };
                        return ContentInfo { kind: ContentKind::Screen, label, base: 16384, ..from_hdr };
                    }
                    return ContentInfo {
                        kind: ContentKind::Code,
                        label: format!("CODE {}{suffix}", hdr.param1),
                        base: hdr.param1,
                        ..from_hdr
                    };
                }
                1 | 2 => {
                    let what = if hdr.kind == 1 { "NUM ARRAY" } else { "CHAR ARRAY" };
                    return ContentInfo {
                        kind: ContentKind::Array,
                        label: format!("{what}{suffix}"),
                        base: 0x8000,
                        ..from_hdr
                    };
                }
                _ => {}
            }
        }
    }

    // Heuristics on the bytes. Loaders differ in whether a checksum follows the screen,
    // so try the plausible flag/checksum combinations.
    for (sf, sc) in [(has_flag, has_flag), (has_flag, false), (false, false)] {
        if looks_like_screen(block_body(data, sf, sc).len()) {
            return ContentInfo {
                kind: ContentKind::Screen,
                label: "SCREEN?".to_string(),
                base: 16384,
                skip_flag: sf,
                skip_checksum: sc,
                source: Source::Heuristic,
                ..ContentInfo::default()
            };
        }
    }
    let score = basic_score(body);
    if score > 0.5 || (score > 0.0 && body.len() < 64) {
        return ContentInfo {
            kind: ContentKind::Basic,
            label: "BASIC?".to_string(),
            base: 23755,
            skip_flag: has_flag,
            skip_checksum: has_flag,
            source: Source::Heuristic,
            ..ContentInfo::default()
        };
    }
    ContentInfo {
        kind: ContentKind::Data,
        base: 0x8000,
        skip_flag: has_flag,
        skip_checksum: has_flag,
        source: Source::None,
        ..ContentInfo::default()
    }
}

/// The list labels for a whole tape, in one call: the UI asks for all of them at
/// once rather than crossing the wasm boundary per row.
pub fn content_labels(blocks: &[Block]) -> Vec<String> {
    (0..blocks.len())
        .map(|i| if data_of(&blocks[i]).is_some() { detect_content(blocks, i).label } else { String::new() })
        .collect()
}
