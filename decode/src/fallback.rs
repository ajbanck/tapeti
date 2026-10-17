// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The fallback: a stretch of pulses no reader took, sorted into what is signal and what is
//! the gap around it.
//!
//! Signal is a burst shaped like a block, a tone with a payload behind it, which whatever loader
//! made it can still read: it goes into a CSW block on the grid it came on, every pulse as it
//! is. The rest loads as a gap on every machine: dead air (pulses the edge detector found
//! quiet), stretches with no edge for longer than any block holds, and bursts with no tone and
//! payload, such as clicks, hum, an orphaned pilot tone or a block's wreckage. It plays as the
//! pause it lasts.

use crate::capture::csw_of_runs;
use crate::pulses::Pulse;
use std::ops::Range;
use tapeti_core::audio::TSTATES_PER_SEC;
use tapeti_core::types::Block;

/// A pulse longer than this is a gap, whatever it holds: 18.7 ms, the longest pulse a TZX block
/// stores. From below, the longest stretch without an edge inside a block the ROM loads is a
/// pilot pair at its timeout, 6,710 T. From above, the edge detector takes 20 ms under its band
/// for silence and goes idle (`IDLE_AFTER_MS`).
const LONGEST_SIGNAL_PULSE: f64 = 65535.0;

/// A burst is block-shaped when it holds a tone of this many pulses with as many faster ones
/// somewhere behind it.
///
/// From above, the shortest pilot any known loader writes in front of a
/// block is 65 half-waves, so every block they load qualifies. From below, a dropout can break a
/// pilot the tape held whole, and the run behind the break must still be a tone; a fragment of
/// a dozen, which no known loader loads, is not. 32 is chosen between.
pub const TONE_RUN: usize = 32;

/// The pulses of a tone lie within ±10% of some mean: the longest at most 1.1 / 0.9 of the
/// shortest. Under the ROM's pilot against its own 1 bits (2,168 T against 1,710 T, 1.27), so a
/// tone does not run on into its payload; the windows slide, so a tone that drifts slowly stays
/// one.
const TONE_SPREAD: f64 = 1.1 / 0.9;

/// A payload's pulses are under this share of the tone's mean. Every loader's data is well under
/// its pilot, the ROM's 1 bits closest at 0.79 of it; a wobbling orphan tone falls under 0.9
/// only for moments, never for [`TONE_RUN`] pulses in a row. Behind a ROM pilot, 32 pulses of
/// 1,900 T (under 1,951 T) are a payload and 32 of 2,000 T a second tone.
const PAYLOAD_SHARE: f64 = 0.9;

/// One part of a stretch, as a range of its pulses.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    /// Plays as a pause of its length.
    Gap(Range<usize>),
    /// Kept as it is.
    Signal(Range<usize>),
}

/// `stretch` in gaps and signal, in order, with no two parts of one kind side by side. The first
/// `keep` pulses are signal whatever they are.
pub fn sort(stretch: &[Pulse], keep: usize) -> Vec<Part> {
    let gap = |p: &Pulse| p.quiet || p.tstates > LONGEST_SIGNAL_PULSE;
    let mut parts = Vec::new();
    let mut push = |part: Part| match (parts.last_mut(), part) {
        (Some(Part::Gap(r)), Part::Gap(n)) | (Some(Part::Signal(r)), Part::Signal(n)) => r.end = n.end,
        (_, part) => parts.push(part),
    };
    let keep = keep.min(stretch.len());
    if keep > 0 {
        push(Part::Signal(0..keep));
    }
    let mut i = keep;
    while i < stretch.len() {
        let start = i;
        if gap(&stretch[i]) {
            while i < stretch.len() && gap(&stretch[i]) {
                i += 1;
            }
            push(Part::Gap(start..i));
            continue;
        }
        while i < stretch.len() && !gap(&stretch[i]) {
            i += 1;
        }
        push(if block_shaped(&stretch[start..i]) { Part::Signal(start..i) } else { Part::Gap(start..i) });
    }
    parts
}

/// Whether `burst` holds a tone of [`TONE_RUN`] pulses followed, anywhere after it, by as many
/// in a row under [`PAYLOAD_SHARE`] of its mean.
fn block_shaped(burst: &[Pulse]) -> bool {
    let w = TONE_RUN;
    if burst.len() < 2 * w {
        return false;
    }
    let tight = |win: &[Pulse]| {
        let (lo, hi) =
            win.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), p| (lo.min(p.tstates), hi.max(p.tstates)));
        hi <= lo * TONE_SPREAD
    };
    let mut k = 0;
    while k + w <= burst.len() {
        if !tight(&burst[k..k + w]) {
            k += 1;
            continue;
        }
        // The tone runs on while the window, slid a pulse at a time, stays tight.
        let mut q = k;
        while q + 1 + w <= burst.len() && tight(&burst[q + 1..q + 1 + w]) {
            q += 1;
        }
        let tone = &burst[k..q + w];
        let fast = PAYLOAD_SHARE * tone.iter().map(|p| p.tstates).sum::<f64>() / tone.len() as f64;
        let mut run = 0;
        for p in &burst[q + w..] {
            run = if p.tstates < fast { run + 1 } else { 0 };
            if run >= w {
                return true;
            }
        }
        k = q + 1;
    }
    false
}

/// `pulses` in a CSW block at `grid_hz`, the rate of the source they came from, or `None` when
/// they round to nothing on it.
pub fn raw_signal(pulses: &[Pulse], grid_hz: u32) -> Option<Block> {
    let scale = f64::from(grid_hz) / f64::from(TSTATES_PER_SEC);
    let runs: Vec<u64> = pulses.iter().map(|p| (p.tstates * scale).round().max(0.0) as u64).collect();
    runs.iter().any(|&n| n > 0).then(|| csw_of_runs(grid_hz, &runs))
}
