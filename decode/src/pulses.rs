// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Pulses: the signal as the time between one edge and the next, the form every source takes
//! before framing reads it.
//!
//! A CSW file, an OUT trace and a tape's own blocks are pulses already; sampled audio becomes
//! pulses when its edges are found.

use std::fmt;
use tapeti_core::audio::{emit_block, RecordingSink, TSTATES_PER_SEC};
use tapeti_core::types::{Block, Body};

/// The time between two edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pulse {
    /// The length, in T-states of the Spectrum's 3.5 MHz clock.
    pub tstates: f64,
    /// Where the pulse starts, in samples of the source it came from: the recording's samples,
    /// or T-states for a source that has none.
    pub start: f64,
}

/// Pulses of exact lengths in T-states, as an OUT trace gives them. They are placed on a
/// T-state grid.
pub fn from_tstates(lengths: &[u64]) -> Vec<Pulse> {
    let mut start = 0.0;
    lengths
        .iter()
        .map(|&t| {
            let pulse = Pulse { tstates: t as f64, start };
            start += t as f64;
            pulse
        })
        .collect()
}

/// Pulses from lengths in samples at `rate_hz`, as a CSW file or block gives them.
pub fn from_runs(runs: &[u32], rate_hz: u32) -> Vec<Pulse> {
    let scale = f64::from(TSTATES_PER_SEC) / f64::from(rate_hz);
    let mut start = 0.0;
    runs.iter()
        .map(|&n| {
            let pulse = Pulse { tstates: f64::from(n) * scale, start };
            start += f64::from(n);
            pulse
        })
        .collect()
}

/// A block whose pulses cannot be played without inflating its data first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotInflated {
    /// The index of the block in the list.
    pub block: usize,
}

impl fmt::Display for NotInflated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "block {} is a Z-RLE CSW block whose data has not been inflated", self.block)
    }
}

impl std::error::Error for NotInflated {}

/// The pulses `blocks` play, one after the other, on a T-state grid.
///
/// Flow control is not followed, so a loop plays once. A Z-RLE CSW block must be inflated by the
/// caller, which has zlib, and is refused otherwise.
pub fn of_blocks(blocks: &[Block]) -> Result<Vec<Pulse>, NotInflated> {
    if let Some(block) = blocks.iter().position(|b| matches!(b.body, Body::Csw { compression: 2, .. })) {
        return Err(NotInflated { block });
    }
    let mut sink = RecordingSink::default();
    for b in blocks {
        emit_block(&mut sink, b);
    }
    // The sink records a stretch at one level in pieces: a pause after a pulse, a Direct
    // Recording's run of equal samples. A pulse is what lies between two edges, so pieces at
    // one level are joined. A piece of no length is not an edge and is left out.
    let mut lengths: Vec<u64> = Vec::new();
    let mut level = None;
    for (t, l) in sink.pulses {
        if t == 0 {
            continue;
        }
        match lengths.last_mut() {
            Some(last) if level == Some(l) => *last += t,
            _ => lengths.push(t),
        }
        level = Some(l);
    }
    Ok(from_tstates(&lengths))
}
