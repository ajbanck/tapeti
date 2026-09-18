//! "Check consistency": structure, useless blocks, infinite loops, cross
//! nesting. The port of `src/tzx/consistency.ts`.

use crate::content::{block_body, detect_content};
use crate::describe::checksum;
use crate::parser::bits_per_symbol;
use crate::types::{Block, Body};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// 0-based index, -1 for tape-wide.
    pub block: i32,
    pub severity: Severity,
    pub message: String,
}

fn issue(block: i32, severity: Severity, message: impl Into<String>) -> Issue {
    Issue { block, severity, message: message.into() }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Nesting {
    Group,
    Loop,
}

/// `base` is the number shown for the first block (1, or 0 with zero-based numbering).
pub fn check_consistency(blocks: &[Block], base: i32) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();
    let n = blocks.len();
    if n == 0 {
        return vec![issue(-1, Severity::Info, "Tape is empty")];
    }

    // Structural nesting of groups and loops (static)
    let mut stack: Vec<(Nesting, usize)> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let at = i as i32;
        match &b.body {
            Body::Unknown { id, .. } => {
                issues.push(issue(at, Severity::Warning, format!("Unknown or deprecated block ID {id:X}")));
            }
            Body::GroupStart { .. } => {
                if stack.iter().any(|(k, _)| *k == Nesting::Group) {
                    issues.push(issue(at, Severity::Error, "Nested group (groups cannot be nested)"));
                }
                stack.push((Nesting::Group, i));
            }
            Body::GroupEnd => match stack.last() {
                None => issues.push(issue(at, Severity::Error, "Group end without group start")),
                Some((Nesting::Loop, _)) => {
                    issues.push(issue(at, Severity::Error, "Group end crosses an open loop"))
                }
                Some(_) => {
                    stack.pop();
                }
            },
            Body::LoopStart { count } => {
                if *count == 0 {
                    issues.push(issue(at, Severity::Warning, "Loop with 0 repetitions is useless"));
                } else if *count == 1 {
                    issues.push(issue(at, Severity::Warning, "Loop with 1 repetition is useless"));
                }
                if stack.iter().any(|(k, _)| *k == Nesting::Loop) {
                    issues.push(issue(at, Severity::Error, "Nested loop (loops cannot be nested)"));
                }
                stack.push((Nesting::Loop, i));
            }
            Body::LoopEnd => match stack.last() {
                None => issues.push(issue(at, Severity::Error, "Loop end without loop start")),
                Some((Nesting::Group, _)) => {
                    issues.push(issue(at, Severity::Error, "Loop end crosses an open group"))
                }
                Some(_) => {
                    stack.pop();
                }
            },
            Body::Jump { offset } => {
                let t = at + i32::from(*offset);
                if *offset == 0 {
                    issues.push(issue(at, Severity::Error, "Jump to itself (infinite loop)"));
                } else if t < 0 || t > n as i32 {
                    issues.push(issue(
                        at,
                        Severity::Error,
                        format!("Jump target {} is outside the tape", t + base),
                    ));
                }
            }
            Body::Call { offsets } => {
                if offsets.is_empty() {
                    issues.push(issue(at, Severity::Warning, "Call sequence with no calls is useless"));
                }
                for (k, o) in offsets.iter().enumerate() {
                    let t = at + i32::from(*o);
                    if *o == 0 {
                        issues.push(issue(at, Severity::Error, format!("Call {} calls itself", k + 1)));
                    } else if t < 0 || t >= n as i32 {
                        issues.push(issue(
                            at,
                            Severity::Error,
                            format!("Call {} target {} is outside the tape", k + 1, t + base),
                        ));
                    }
                }
            }
            Body::Select { entries } => {
                for (k, e) in entries.iter().enumerate() {
                    let t = at + i32::from(e.offset);
                    if t < 0 || t >= n as i32 {
                        issues.push(issue(
                            at,
                            Severity::Error,
                            format!("Selection {} target {} is outside the tape", k + 1, t + base),
                        ));
                    }
                }
            }
            Body::Turbo { used_bits, data, .. } | Body::PureData { used_bits, data, .. } => {
                if *used_bits < 1 || *used_bits > 8 {
                    issues.push(issue(
                        at,
                        Severity::Error,
                        format!("Used bits in last byte is {used_bits}, must be 1-8"),
                    ));
                }
                if data.is_empty() {
                    issues.push(issue(at, Severity::Warning, "Data block is empty"));
                }
            }
            Body::Direct { used_bits, tstates, .. } => {
                if *used_bits < 1 || *used_bits > 8 {
                    issues.push(issue(
                        at,
                        Severity::Error,
                        format!("Used bits in last byte is {used_bits}, must be 1-8"),
                    ));
                }
                if *tstates == 0 {
                    issues.push(issue(at, Severity::Error, "T-states per sample is 0"));
                }
            }
            Body::Standard { data, .. } => {
                if data.is_empty() {
                    issues.push(issue(at, Severity::Warning, "Data block is empty"));
                } else if data.len() >= 2 && checksum(&data[..data.len() - 1]) != data[data.len() - 1] {
                    issues.push(issue(at, Severity::Warning, "Checksum byte does not match data"));
                }
            }
            Body::PureTone { pulse_len, count } => {
                if *count == 0 || *pulse_len == 0 {
                    issues.push(issue(at, Severity::Warning, "Pure tone with zero length is useless"));
                }
            }
            Body::PulseSeq { pulses } => {
                if pulses.is_empty() {
                    issues.push(issue(at, Severity::Warning, "Pulse sequence with no pulses is useless"));
                }
            }
            Body::Generalized {
                totp,
                npp,
                pilot_symbols,
                pilot_stream,
                totd,
                npd,
                data_symbols,
                data,
                ..
            } => {
                if *totp > 0 {
                    if pilot_symbols.is_empty() {
                        issues.push(issue(
                            at,
                            Severity::Error,
                            "Pilot stream present but no pilot symbols defined",
                        ));
                    }
                    if pilot_stream.len() as u32 != *totp {
                        issues.push(issue(
                            at,
                            Severity::Error,
                            format!("Pilot stream has {} runs but TOTP is {totp}", pilot_stream.len()),
                        ));
                    }
                    if let Some(r) =
                        pilot_stream.iter().find(|r| usize::from(r.symbol) >= pilot_symbols.len())
                    {
                        issues.push(issue(
                            at,
                            Severity::Error,
                            format!("Pilot stream references undefined symbol {}", r.symbol),
                        ));
                    }
                    if pilot_symbols.iter().any(|s| s.pulses.len() > usize::from(*npp)) {
                        issues.push(issue(
                            at,
                            Severity::Error,
                            "A pilot symbol has more pulses than NPP allows",
                        ));
                    }
                }
                if *totd > 0 {
                    if data_symbols.is_empty() {
                        issues.push(issue(
                            at,
                            Severity::Error,
                            "Data stream present but no data symbols defined",
                        ));
                    } else {
                        let need = (u64::from(bits_per_symbol(data_symbols.len() as u32)) * u64::from(*totd))
                            .div_ceil(8) as usize;
                        if data.len() < need {
                            issues.push(issue(
                                at,
                                Severity::Error,
                                format!(
                                    "Data stream too short: {} bytes, {need} needed for {totd} symbols",
                                    data.len()
                                ),
                            ));
                        }
                    }
                    if data_symbols.iter().any(|s| s.pulses.len() > usize::from(*npd)) {
                        issues.push(issue(
                            at,
                            Severity::Error,
                            "A data symbol has more pulses than NPD allows",
                        ));
                    }
                }
            }
            Body::Hardware { entries } if entries.is_empty() => {
                issues.push(issue(at, Severity::Warning, "Hardware block with no entries"));
            }
            Body::Archive { entries } if entries.is_empty() => {
                issues.push(issue(at, Severity::Warning, "Archive info block with no entries"));
            }
            _ => {}
        }
    }

    // Data blocks whose length differs from what the preceding ROM header announces.
    for (i, b) in blocks.iter().enumerate() {
        let data = match &b.body {
            Body::Standard { data, .. }
            | Body::Turbo { data, .. }
            | Body::PureData { data, .. }
            | Body::Direct { data, .. }
            | Body::Generalized { data, .. } => data,
            _ => continue,
        };
        let c = detect_content(blocks, i);
        let Some(expected) = c.expected_length else { continue };
        let len = block_body(data, c.skip_flag, c.skip_checksum).len();
        let expected = usize::from(expected);
        if len < expected {
            issues.push(issue(
                i as i32,
                Severity::Warning,
                format!(
                    "Data is {} bytes shorter than its header says ({len} of {expected})",
                    expected - len
                ),
            ));
        } else if len > expected {
            issues.push(issue(
                i as i32,
                Severity::Warning,
                format!(
                    "Data is {} bytes longer than its header says ({len}, header {expected})",
                    len - expected
                ),
            ));
        }
    }

    for (kind, at) in &stack {
        issues.push(issue(
            *at as i32,
            Severity::Error,
            match kind {
                Nesting::Group => "Group is never closed",
                Nesting::Loop => "Loop is never closed",
            },
        ));
    }

    // Dynamic flow: simulate with a visited-state set to detect infinite loops
    // and calls without a return.
    issues.extend(simulate_flow(blocks));
    issues.sort_by_key(|i| i.block);
    issues
}

fn simulate_flow(blocks: &[Block]) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();
    let n = blocks.len() as i32;
    let mut loop_stack: Vec<(i32, u16)> = Vec::new(); // start, remaining
    let mut call_stack: Vec<(i32, usize)> = Vec::new(); // block, next
    let mut seen: HashSet<String> = HashSet::new();
    let mut i: i32 = 0;
    let mut steps = 0u32;
    while i >= 0 && i < n {
        steps += 1;
        if steps > 200_000 {
            issues.push(issue(i, Severity::Error, "Playback does not terminate (too many steps)"));
            break;
        }
        let b = &blocks[i as usize];
        match &b.body {
            Body::Unknown { .. } => {
                i += 1;
                continue;
            }
            Body::Jump { offset } => {
                let calls: Vec<String> = call_stack.iter().map(|(b, n)| format!("{b}/{n}")).collect();
                let loops: Vec<String> = loop_stack.iter().map(|(s, r)| format!("{s}/{r}")).collect();
                let key = format!("{i}:{}:{}", calls.join(","), loops.join(","));
                if !seen.insert(key) {
                    issues.push(issue(
                        i,
                        Severity::Error,
                        "Infinite loop: this jump is reached again in the same state",
                    ));
                    break;
                }
                i += i32::from(*offset);
                continue;
            }
            Body::LoopStart { count } => loop_stack.push((i, *count)),
            Body::LoopEnd => {
                if let Some(l) = loop_stack.last_mut() {
                    l.1 = l.1.wrapping_sub(1);
                    if l.1 > 0 {
                        i = l.0 + 1;
                        continue;
                    }
                    loop_stack.pop();
                }
            }
            Body::Call { offsets } => {
                if call_stack.len() >= 16 {
                    issues.push(issue(i, Severity::Error, "Call nesting too deep (recursive calls?)"));
                    break;
                }
                if !offsets.is_empty() {
                    call_stack.push((i, 0));
                    i += i32::from(offsets[0]);
                    continue;
                }
            }
            Body::Return => match call_stack.last_mut() {
                None => {
                    issues.push(issue(i, Severity::Error, "Return reached without a pending call"));
                }
                Some(c) => {
                    c.1 += 1;
                    let (block, next) = (c.0, c.1);
                    let offsets = match &blocks[block as usize].body {
                        Body::Call { offsets } => offsets.clone(),
                        _ => Vec::new(),
                    };
                    if next < offsets.len() {
                        i = block + i32::from(offsets[next]);
                    } else {
                        call_stack.pop();
                        i = block + 1;
                    }
                    continue;
                }
            },
            Body::Pause { pause } if *pause == 0 => break,
            _ => {}
        }
        i += 1;
    }
    if let Some((block, _)) = call_stack.last() {
        issues.push(issue(
            *block,
            Severity::Error,
            "Call sequence never returns (end of tape reached inside a call)",
        ));
    }
    issues
}
