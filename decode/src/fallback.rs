// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The fallback: a stretch of pulses no reader took, kept as it is.
//!
//! Nothing is dropped and nothing collapsed: every pulse goes into a CSW block on the grid it
//! came on, so whatever loader made the stretch can still read it, and it lasts as long as it
//! did.

use crate::capture::csw_of_runs;
use crate::pulses::Pulse;
use tapeti_core::audio::TSTATES_PER_SEC;
use tapeti_core::types::Block;

/// `pulses` in a CSW block at `grid_hz`, the rate of the source they came from, or `None` when
/// they round to nothing on it.
pub fn raw_signal(pulses: &[Pulse], grid_hz: u32) -> Option<Block> {
    let scale = f64::from(grid_hz) / f64::from(TSTATES_PER_SEC);
    let runs: Vec<u64> = pulses.iter().map(|p| (p.tstates * scale).round().max(0.0) as u64).collect();
    runs.iter().any(|&n| n > 0).then(|| csw_of_runs(grid_hz, &runs))
}
