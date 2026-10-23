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
use crate::framing::{
    bit_timeout, leader_pair, Speed, BOUNCE_MOST, BYTE_PULSES, LEADER_PAIRS, SHORT_LEADER_PAIRS,
};
use crate::pulses::Pulse;
use std::ops::Range;
use tapeti_core::audio::TSTATES_PER_SEC;
use tapeti_core::types::{Block, RomTimings};

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
    let gap = |p: &Pulse| !signal(p);
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

/// Whether a pulse can be signal: not dead air, and no longer than a block holds.
pub(crate) fn signal(p: &Pulse) -> bool {
    !p.quiet && p.tstates <= LONGEST_SIGNAL_PULSE
}

/// The first run in `gap` of pilot the ROM's leader loop locks on, [`LEADER_PAIRS`] pairs in a
/// row and as many more as follow at their pace ([`PILOT_PACE`]), as a range of its pulses. The
/// leader loop's window is wider than a pilot: it takes a 1 bit's pair too, and the pairs of many
/// a block's bits after treble loss has merged them.
pub(crate) fn rom_pilot(gap: &[Pulse]) -> Option<Range<usize>> {
    let pair = |k: usize| gap[k].tstates + gap[k + 1].tstates;
    (0..2).find_map(|phase| {
        let mut from = phase;
        let mut k = phase;
        while k + 1 < gap.len() {
            if !leader_pair(pair(k), false) {
                k += 2;
                from = k;
                continue;
            }
            k += 2;
            if k - from >= 2 * LEADER_PAIRS {
                let pace = gap[from..k].iter().map(|p| p.tstates).sum::<f64>() / (k - from) as f64 * 2.0;
                while k + 1 < gap.len() && (pair(k) - pace).abs() <= PILOT_PACE * pace {
                    k += 2;
                }
                return Some(from..k);
            }
        }
        None
    })
}

/// Whether `gap` holds a pilot tone broken into pieces the ROM's leader loop never locks on: runs
/// of [`SHORT_LEADER_PAIRS`] leader pairs or more in a row, at one pace ([`PILOT_PACE`]), that
/// between them hold its [`LEADER_PAIRS`] pairs inside the time of the shortest pilot the saver
/// writes, a data block's; a run reaching past that time counts the part inside it. A run of
/// [`rom_pilot`]'s is one.
///
/// Hiss as loud as the tape breaks a pilot every few dozen pairs, splitting a half into two or
/// merging two, and the leader loop starts again at each break: the tape's pilot is heard, and
/// no Spectrum locks on it. A run is counted once over both phases of the pairs, since a break of
/// an odd number of pulses carries the pilot to the other phase. Hiss alone makes no such run:
/// its crossings come far closer than a pilot's, and the leader loop's window, 43 reads and
/// more a pair, holds none of its pairs sixteen times in a row.
pub(crate) fn pilot_in_pieces(gap: &[Pulse]) -> bool {
    let pair = |k: usize| gap[k].tstates + gap[k + 1].tstates;
    // Each run at either phase, its pulses and its mean pair.
    let mut runs: Vec<(Range<usize>, f64)> = Vec::new();
    for phase in 0..2 {
        let mut k = phase;
        while k + 1 < gap.len() {
            let from = k;
            while k + 1 < gap.len() && leader_pair(pair(k), false) {
                k += 2;
            }
            if k - from >= 2 * SHORT_LEADER_PAIRS {
                let pace = gap[from..k].iter().map(|p| p.tstates).sum::<f64>() / (k - from) as f64 * 2.0;
                runs.push((from..k, pace));
            }
            k = k.max(from + 2);
        }
    }
    runs.sort_by_key(|(r, _)| r.start);
    // The T-states from the gap's start to each pulse.
    let at: Vec<f64> = std::iter::once(0.0)
        .chain(gap.iter().scan(0.0, |t, p| {
            *t += p.tstates;
            Some(*t)
        }))
        .collect();
    runs.iter().enumerate().any(|(i, (first, pace))| {
        let span = f64::from(RomTimings::PILOT_DATA) * pace / 2.0;
        // The first pulse that starts past the span.
        let limit = at.partition_point(|&t| t - at[first.start] <= span);
        let (mut covered, mut reach) = (0, first.start);
        for (r, p) in &runs[i..] {
            if r.start >= limit {
                break;
            }
            if (p - pace).abs() > PILOT_PACE * pace {
                continue;
            }
            covered += r.end.min(limit).saturating_sub(r.start.max(reach));
            reach = reach.max(r.end);
        }
        covered >= 2 * LEADER_PAIRS
    })
}

/// How far a pilot's pair may lie from the mean of its pairs: 15%, between a pilot's last pairs
/// as it fades into a recording's end (seen to stray 10% to 20%) and a 1 bit's pair, 21% under a
/// pilot pair.
const PILOT_PACE: f64 = 0.15;

/// Whether behind `pilot`, a run of [`rom_pilot`] in `gap`, lies the signal of a block that did
/// not read: at least a flag and a checksum of pairs the bit loop reads as bits, [`BYTE_PULSES`]
/// pairs in a row, none of them a gap.
///
/// A pilot alone says no block was lost: a recording that starts inside one, or ends inside one
/// as the next program's pilot fades, or a block cut by a hole in its pilot and read from what
/// follows the hole, leaves a pilot and at most fragments of it behind in the gap. A block that
/// fades under the edge detector's floor leaves the same, and so does a pilot with no block
/// behind it on the tape; the pulses cannot tell them apart ([`crate::entry::Finding::Collapsed`]
/// names the pilot). A bit is a pair the bit loop takes before it runs out ([`bit_timeout`]) at
/// the pilot's speed, and longer than [`BOUNCE_MOST`] twice, half the shortest 0 bit's pair the
/// ROM loads: treble loss that merges a block's halves moves its edges but keeps its pairs about
/// a bit long, where the tone of a motor or a deck's noise behind a pilot that a recording ends
/// inside keeps pairs longer, and hiss shorter, than any bit. A pair at the pilot's pace
/// ([`PILOT_PACE`]), such as the rest of a pilot behind a hole or its last pairs as it fades,
/// neither counts nor breaks the run. Unlike [`block_shaped`], which keeps signal, this keeps
/// nothing: the gap plays as it did, and the decode says what is in it.
pub(crate) fn burst_behind(gap: &[Pulse], pilot: Range<usize>) -> bool {
    let half = gap[pilot.clone()].iter().map(|p| p.tstates).sum::<f64>() / pilot.len() as f64;
    let speed = Speed::of_pilot(half);
    let bit = |pair: f64| pair > speed.of(2.0 * BOUNCE_MOST) && pair <= speed.of(bit_timeout(1));
    (0..2).any(|phase| {
        let mut run = 0;
        for k in (pilot.end + phase..gap.len().saturating_sub(1)).step_by(2) {
            let (a, b) = (&gap[k], &gap[k + 1]);
            let pair = a.tstates + b.tstates;
            if !signal(a) || !signal(b) || !bit(pair) {
                run = 0;
            } else if (pair - 2.0 * half).abs() > PILOT_PACE * 2.0 * half {
                run += 1;
                if run >= BYTE_PULSES {
                    return true;
                }
            }
        }
        false
    })
}

/// Whether `burst` holds a tone of [`TONE_RUN`] pulses followed, anywhere after it, by as many
/// in a row under [`PAYLOAD_SHARE`] of its mean.
pub(crate) fn block_shaped(burst: &[Pulse]) -> bool {
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
