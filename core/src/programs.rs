//! Tape structure: group/loop ranges and the programs (games) a collection tape
//! holds. The port of `src/tzx/programs.ts`.

use crate::describe::{decode_header, HeaderInfo};
use crate::types::{Block, Body};
use std::collections::HashMap;

/// Pairs of start/end indices for groups and loops (nested ones included), in
/// the order they close, which is the order the TypeScript `Map` keeps.
pub fn group_ranges(blocks: &[Block]) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let mut stack: Vec<(u8, usize)> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let id = b.id();
        if id == 0x21 || id == 0x24 {
            stack.push((id, i));
        } else if id == 0x22 || id == 0x25 {
            let want = if id == 0x22 { 0x21 } else { 0x24 };
            if let Some(k) = stack.iter().rposition(|(kind, _)| *kind == want) {
                ranges.push((stack[k].1 as u32, i as u32));
                stack.truncate(k);
            }
        }
    }
    ranges
}

/// What decided a program boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramSource {
    Header,
    Group,
    Select,
    Tape,
}

impl ProgramSource {
    pub fn name(self) -> &'static str {
        match self {
            ProgramSource::Header => "header",
            ProgramSource::Group => "group",
            ProgramSource::Select => "select",
            ProgramSource::Tape => "tape",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub name: String,
    /// First block index.
    pub start: u32,
    /// Last block index, inclusive.
    pub end: u32,
    pub source: ProgramSource,
}

/// Blocks that introduce the program that follows them (text, tones, loader
/// loops), as opposed to pauses and stops, which close the program before them.
fn is_leading(b: &Block) -> bool {
    matches!(
        b.body,
        Body::PureTone { .. }
            | Body::PulseSeq { .. }
            | Body::LoopStart { .. }
            | Body::LoopEnd
            | Body::SignalLevel { .. }
            | Body::Text { .. }
            | Body::Message { .. }
            | Body::Archive { .. }
            | Body::Hardware { .. }
            | Body::Custom { .. }
            | Body::Glue { .. }
    )
}

/// The BASIC Program header a block carries, if it is one.
fn program_header(b: &Block) -> Option<HeaderInfo> {
    let data = match &b.body {
        Body::Standard { data, .. } | Body::Turbo { data, .. } | Body::Generalized { data, .. } => data,
        _ => return None,
    };
    decode_header(data).filter(|h| h.kind == 0)
}

fn has_program_header(blocks: &[Block], from: usize, to: usize) -> bool {
    (from..=to).any(|i| blocks.get(i).and_then(program_header).is_some())
}

/// Title from an Archive info block, if the tape has one.
pub fn tape_title(blocks: &[Block]) -> Option<String> {
    for b in blocks {
        if let Body::Archive { entries } = &b.body {
            if let Some(t) = entries.iter().find(|e| e.kind == 0).map(|e| e.text.trim()) {
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
    }
    None
}

/// Split a tape into programs. A program starts at every BASIC Program header (the loader of a
/// game), at every top-level group that contains one (grouped collections keep their names),
/// and at every Select block target. Anything else, including headerless custom loaders and
/// "stop the tape" blocks between the parts of a multi-load game, stays with the program it
/// follows. Metadata right before a boundary (text, tones, loader loops) belongs to the program
/// it introduces. A tape without any boundary is one program named after its archive info.
/// The result always covers every block.
pub fn detect_programs(blocks: &[Block]) -> Vec<Program> {
    let n = blocks.len();
    if n == 0 {
        return Vec::new();
    }
    let mut starts: HashMap<usize, (String, ProgramSource)> = HashMap::new();
    let ranges: HashMap<u32, u32> = group_ranges(blocks).into_iter().collect();
    let mut i = 0usize;
    while i < n {
        let b = &blocks[i];
        if let Body::GroupStart { name } = &b.body {
            if let Some(end) = ranges.get(&(i as u32)).copied() {
                let end = end as usize;
                if end > i + 1 && has_program_header(blocks, i + 1, end - 1) {
                    let name = name.trim();
                    let name = if name.is_empty() { "Group".to_string() } else { name.to_string() };
                    starts.insert(i, (name, ProgramSource::Group));
                }
                i = end + 1;
                continue;
            }
        }
        if let Some(h) = program_header(b) {
            let name = h.name.trim();
            let name = if name.is_empty() { "Untitled".to_string() } else { name.to_string() };
            starts.insert(i, (name, ProgramSource::Header));
        }
        i += 1;
    }
    for (i, b) in blocks.iter().enumerate() {
        let Body::Select { entries } = &b.body else { continue };
        for e in entries {
            let t = i as i32 + i32::from(e.offset);
            if t > 0 && (t as usize) < n {
                let text = e.text.trim();
                let name = if text.is_empty() { format!("Block {}", t + 1) } else { text.to_string() };
                starts.insert(t as usize, (name, ProgramSource::Select));
            }
        }
    }
    if starts.is_empty() {
        return vec![Program {
            name: tape_title(blocks).unwrap_or_else(|| "Untitled".to_string()),
            start: 0,
            end: (n - 1) as u32,
            source: ProgramSource::Tape,
        }];
    }

    let mut sorted: Vec<usize> = starts.keys().copied().collect();
    sorted.sort_unstable();
    let mut programs: Vec<Program> = sorted
        .iter()
        .enumerate()
        .map(|(k, at)| {
            let (name, source) = starts[at].clone();
            let mut start = *at;
            if k == 0 {
                start = 0;
            } else if source != ProgramSource::Select {
                let floor = sorted[k - 1] + 1;
                while start > floor && is_leading(&blocks[start - 1]) {
                    start -= 1;
                }
            }
            Program { name, start: start as u32, end: (n - 1) as u32, source }
        })
        .collect();
    for k in 0..programs.len().saturating_sub(1) {
        programs[k].end = programs[k + 1].start - 1;
    }
    programs
}
