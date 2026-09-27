// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Comparing blocks and tapes.

use crate::describe::is_metadata;
use crate::types::{Block, Body};
use crate::writer::serialize_block;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockCompareMode {
    /// Only the bytes of data blocks.
    Data,
    DataTimings,
    DataTimingsPauses,
}

impl BlockCompareMode {
    pub fn from_name(s: &str) -> Self {
        match s {
            "data" => BlockCompareMode::Data,
            "data+timings+pauses" => BlockCompareMode::DataTimingsPauses,
            _ => BlockCompareMode::DataTimings,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapeCompareMode {
    DataBlocks,
    IgnoreMetadata,
    All,
}

impl TapeCompareMode {
    pub fn from_name(s: &str) -> Self {
        match s {
            "datablocks" => TapeCompareMode::DataBlocks,
            "all" => TapeCompareMode::All,
            _ => TapeCompareMode::IgnoreMetadata,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareResult {
    Same,
    Diff,
    Ignored,
}

impl CompareResult {
    pub fn name(self) -> &'static str {
        match self {
            CompareResult::Same => "same",
            CompareResult::Diff => "diff",
            CompareResult::Ignored => "ignored",
        }
    }
}

fn data_block_bytes(b: &Block) -> Option<&[u8]> {
    match &b.body {
        Body::Standard { data, .. }
        | Body::Turbo { data, .. }
        | Body::PureData { data, .. }
        | Body::Direct { data, .. }
        | Body::Generalized { data, .. } => Some(data),
        _ => None,
    }
}

/// A copy with its pause zeroed, so "data and timings" ignores pauses.
fn strip_pause(b: &Block) -> Block {
    let mut copy = b.clone();
    match &mut copy.body {
        Body::Standard { pause, .. }
        | Body::Turbo { pause, .. }
        | Body::PureData { pause, .. }
        | Body::Direct { pause, .. }
        | Body::Csw { pause, .. }
        | Body::Generalized { pause, .. }
        | Body::Pause { pause } => *pause = 0,
        _ => {}
    }
    copy
}

/// Compare two blocks according to the block-compare setting.
pub fn blocks_equal(a: &Block, b: &Block, mode: BlockCompareMode) -> bool {
    if mode == BlockCompareMode::Data {
        if let (Some(x), Some(y)) = (data_block_bytes(a), data_block_bytes(b)) {
            return x == y;
        }
    }
    if a.body.is_unknown() || b.body.is_unknown() {
        return match (&a.body, &b.body) {
            (Body::Unknown { id: ia, raw: ra }, Body::Unknown { id: ib, raw: rb }) => ia == ib && ra == rb,
            _ => false,
        };
    }
    if a.id() != b.id() {
        return false;
    }
    let (x, y) = if mode == BlockCompareMode::DataTimingsPauses {
        (a.clone(), b.clone())
    } else {
        (strip_pause(a), strip_pause(b))
    };
    serialize_block(&x) == serialize_block(&y)
}

fn considered(b: &Block, mode: TapeCompareMode) -> bool {
    match mode {
        TapeCompareMode::DataBlocks => data_block_bytes(b).is_some(),
        TapeCompareMode::IgnoreMetadata => !is_metadata(b),
        TapeCompareMode::All => true,
    }
}

/// Compare two tapes block by block.
///
/// Returns a result per block of each tape, plus whether the two tapes are identical overall.
pub fn compare_tapes(
    left: &[Block],
    right: &[Block],
    block_mode: BlockCompareMode,
    tape_mode: TapeCompareMode,
) -> (Vec<CompareResult>, Vec<CompareResult>, bool) {
    let li: Vec<usize> = (0..left.len()).filter(|i| considered(&left[*i], tape_mode)).collect();
    let ri: Vec<usize> = (0..right.len()).filter(|i| considered(&right[*i], tape_mode)).collect();
    let mut lres = vec![CompareResult::Ignored; left.len()];
    let mut rres = vec![CompareResult::Ignored; right.len()];
    let n = li.len().max(ri.len());
    let mut identical = li.len() == ri.len();
    for k in 0..n {
        match (li.get(k), ri.get(k)) {
            (None, Some(b)) => {
                rres[*b] = CompareResult::Diff;
                identical = false;
            }
            (Some(a), None) => {
                lres[*a] = CompareResult::Diff;
                identical = false;
            }
            (Some(a), Some(b)) => {
                let eq = blocks_equal(&left[*a], &right[*b], block_mode);
                let result = if eq { CompareResult::Same } else { CompareResult::Diff };
                lres[*a] = result;
                rres[*b] = result;
                if !eq {
                    identical = false;
                }
            }
            (None, None) => {}
        }
    }
    (lres, rres, identical)
}

/// Find all blocks in `haystack` matching `needle`.
///
/// `skip` is where the needle itself sits in the haystack, if it is in there: it is an
/// index because that is how a block is identified across the wire.
pub fn find_matches(
    needle: &Block,
    haystack: &[Block],
    mode: BlockCompareMode,
    skip: Option<usize>,
) -> Vec<u32> {
    haystack
        .iter()
        .enumerate()
        .filter(|(i, b)| Some(*i) != skip && blocks_equal(needle, b, mode))
        .map(|(i, _)| i as u32)
        .collect()
}
