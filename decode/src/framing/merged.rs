// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! A sync pair the recording merged into a neighbouring half-wave, or moved past the ROM's
//! window, read where the block's bits are.
//!
//! The sync pair is the highest tone a ROM block carries (2.6 and 2.4 kHz, against a 0 bit's 2
//! kHz and the pilot's 807 Hz), so it is the first thing a recording that has lost its treble
//! loses. When a lobe loses its edges, the half-waves either side of it become one pulse with
//! it, and which three they are sets where every later bit begins:
//! - backward: the pilot's last half-wave and the pair, about 3,570 T. The ROM takes it for
//!   more pilot (or, a little slower, times out on it), then the first 0 bit for the sync pair,
//!   and the block reads a bit late, with a checksum that often still holds;
//! - forward: the pair and the first data half-wave. The bit's other half survives, and the
//!   bits begin there, half a bit on: every pair after it straddles two bits;
//! - forward and hidden: the same with a 0 bit's half, 2,257 T, which passes for a pilot
//!   half-wave; the half that survives is then where the ROM finds its sync.
//!
//! Each form predicts the merged pulse from what has been measured: the leader's pilot half,
//! the saver's pair at the tape's speed, and for the forward forms the surviving half, which is
//! as long as the half it lost. Within [`DECIDE`] the length decides, unless the other form is
//! within [`ASK`] and its reading proves more ([`crate::proof`]); within [`ASK`] the length
//! only asks, and the readings answer, the one that proves most standing (`Seam::rank`).
//!
//! A pair can also be whole and still miss the ROM's window. A recording that moves one kind of
//! edge against the other moves where the pair splits, not its sum, and the ROM, which judges
//! the first pulse alone, takes a first pulse past 1,048 T for more pilot and the second for
//! its sync. So a pair is a sync pair by its sum too, within [`WIDTH`] of the saver's, widened
//! by the skew the pilot's own halves measure (`Seam::skew`). The ROM's reading then lies half
//! a bit out, and can still be a block, its checksum holding by chance; where the pair whole
//! reads a block that proves as much, the phase decides (`Seam::half_a_bit_out`).
//!
//! The ROM's own reading stays wherever a block stands behind it that no reading proves more
//! than, or displaces at the right phase, and wherever nothing answers.

use super::{read_bytes, Speed, Start, BOUNCE_MOST};
use crate::fallback::TONE_RUN;
use crate::proof::{self, Proof};
use crate::pulses::Pulse;
use std::cell::OnceCell;
use tapeti_core::types::RomTimings;

/// How near the merged pulse must come to a form's prediction for its length to decide.
///
/// From below, the form the length can tell where a tape's hiss is as loud as the band reads
/// through: a backward merge behind which the first bit is a 0, its crossings moved by a
/// quarter of a lobe each way, lies 6.7% short of its prediction, and must still be taken, or
/// the block behind it reads a bit late. From above, [`ASK`]: past it nothing is asked of the
/// bytes that could overrule the length. 10% is chosen between.
pub const DECIDE: f64 = 0.10;

/// How near the merged pulse must come to a form's prediction to be asked at all.
///
/// The length cannot decide here, only open the question, and the block each reading makes
/// answers it (`Seam::rank`). From below, a band-pass on a worn tape annihilates the second sync
/// pulse and moves the crossings around it: the merged pulse then misses its prediction by
/// 11.4% to 14.6%, and a header still reads past it up to 17.5%. From above nothing bounds it
/// but that test, since a wider window only puts more seams to the bytes: at 0.20 a real
/// program is lost, and at 0.30 nothing changes but a few chance blocks of five bytes. 0.25 is
/// chosen.
pub const ASK: f64 = 0.25;

/// The share either way of the leader's pilot half that a pulse may lie and still be pilot, and
/// of the saver's sync pair that a pair's sum may lie and still be a sync pair.
///
/// The ROM judges a single half-wave only once, the first sync pulse, and that behind a leader
/// of over 2,000 half-waves; everything else it judges by pairs. Hiss as loud as the band reads
/// through moves a half by up to 11.25%, and wow adds 0.42%. From below, the sum window holds the
/// longest first sync pulse the ROM takes beside a saver's second, 1,048 + 735 T, 27.2% over the
/// saver's 1,402; and the pilot window holds the fastest pilot the ROM loads, 0.831 of the
/// saver's, with a half moved by the hiss and the wow, 26.6% under it. From above, a run of 1
/// bits passes for pilot at 1,710 / 2,168 of the speed, and the 0 bit behind it, its pair short
/// by the hiss at its two outer crossings, must stay out of the sum window read at that speed:
/// under 45.9%. At the saver's split, a pair whose first pulse the ROM refuses, 1,049 T, sums to
/// 2,205 T, 57% over, out of the window. 0.35 is chosen between.
pub const WIDTH: f64 = 0.35;

/// How near a pulse past [`WIDTH`] must come to the leader's halves of its parity to be pilot
/// (`Seam::pilot`): the most hiss as loud as the band reads through moves a half, as
/// [`WIDTH`] has it.
const KIND_WIDTH: f64 = 0.1125;

/// How many bit pairs `Seam::fit` weighs: past a ROM header's leading run of equal bits, the
/// 0x00 flag and type, 16 bits that pair alike at either phase, and its name's first character,
/// whose first 1 bit comes by bit 19. Past that wall more pairs only average more noise out.
const FIT_BITS: usize = 96;

/// The block's bits where a merged or moved sync pair left them, where that decides against the
/// ROM's own reading; `None` where that stands.
///
/// `from` is the pulse behind the leader's pairs and `anchor` the pulse the ROM's sync
/// loop stopped at: its first sync pulse, or the pulse it timed out on. `rom` is where the ROM's
/// bits begin, if it read a sync pair. Each pulse behind the leader that is no pilot
/// ([`WIDTH`]) may be where the pilot ends, in order, up to `anchor`; the ROM's own sync ends
/// the search. A pulse with the pilot running on behind it, there or behind bits that read as
/// the pilot does, all 1s, is not where the pilot ends, but a glitch inside it
/// ([`super::pilot_runs_on`]), whatever form its length fits.
pub(super) fn bits_start(
    pulses: &[Pulse],
    from: usize,
    anchor: usize,
    rom: Option<usize>,
    pilot_half: f64,
    expected: Option<usize>,
) -> Option<Start> {
    // The leader's last halves, by their parity: a pilot whose two kinds of edge lie apart
    // alternates between two lengths.
    let (mut sum, mut n) = ([0.0f64; 2], [0usize; 2]);
    for (i, p) in pulses.iter().enumerate().take(from).skip(from.saturating_sub(TONE_RUN)) {
        sum[i % 2] += p.tstates;
        n[i % 2] += 1;
    }
    let kinds = [0, 1].map(|k| if n[k] > 0 { sum[k] / n[k] as f64 } else { pilot_half });
    let seam = Seam {
        pulses,
        pilot_half,
        kinds,
        speed: Speed::of_pilot(pilot_half),
        expected,
        rom,
        rom_rank: OnceCell::new(),
    };
    let to = anchor.min(pulses.len().saturating_sub(1));
    for b in from..=to {
        if b < anchor && seam.pilot(b) {
            continue;
        }
        if super::pilot_runs_on(pulses, b + 1, pilot_half, true) {
            continue;
        }
        if let Some(start) = seam.at(b) {
            return Some(start);
        }
        if rom == Some(b + 2) {
            return None;
        }
    }
    None
}

/// What every reading behind one leader shares.
struct Seam<'a> {
    pulses: &'a [Pulse],
    pilot_half: f64,
    /// The leader's last halves at even pulses and at odd ones, on average.
    kinds: [f64; 2],
    speed: Speed,
    expected: Option<usize>,
    /// Where the ROM's own bits begin, if it read a sync pair, and what they prove.
    rom: Option<usize>,
    rom_rank: OnceCell<Proof>,
}

impl Seam<'_> {
    /// The bits as the pulse at `b` ends the pilot, where that decides against the ROM.
    fn at(&self, b: usize) -> Option<Start> {
        let len = |k: usize| self.pulses.get(k).map(|p| p.tstates);
        let breaker = len(b)?;
        let syncs = self.speed.of(f64::from(RomTimings::SYNC1) + f64::from(RomTimings::SYNC2));
        let off = |pred: f64| (breaker - pred).abs() / pred;
        let back_off = off(self.pilot_half + syncs);
        let orphan = len(b + 1).filter(|&o| self.data_half(o));
        let fwd_off = orphan.map_or(f64::INFINITY, |o| off(syncs + o));
        let back = Start::At(b + 1);
        let fwd = |o: f64| self.orphan(o, b + 2);

        // The length decides, unless the other form is open and its reading proves more.
        if let Some(o) = orphan.filter(|_| fwd_off <= DECIDE && fwd_off < back_off) {
            if back_off <= ASK && self.rank(back) > self.rank(fwd(o)) {
                return Some(back);
            }
            return Some(fwd(o));
        }
        if back_off <= DECIDE {
            if let Some(o) = orphan.filter(|_| fwd_off <= ASK) {
                if self.rank(fwd(o)) > self.rank(back) {
                    return Some(fwd(o));
                }
            }
            return Some(back);
        }
        if let Some(start) = self.hidden(b, syncs) {
            return Some(start);
        }
        if self.rom == Some(b + 2) {
            return None;
        }
        // The pair whole, by its sum, unless the ROM's reading proves more. Where the ROM took
        // the pair's second pulse for its sync, its reading lies half a bit out and a block
        // there is one by chance; between two that prove as much, the phase decides (`half_a_bit_out`).
        let (kind, last) = self.skew(b);
        let pair = len(b + 1).map(|t| breaker + t);
        let (floor, ceiling) =
            ((1.0 - WIDTH) * syncs - kind.max(0.0), (1.0 + WIDTH) * syncs + (-kind).max(0.0));
        let whole = Start::At(b + 2);
        if pair.is_some_and(|p| (floor..=ceiling).contains(&p)) {
            let (sum, rom) = (self.rank(whole), self.rom_rank());
            if sum > rom || rom == Proof::None || sum == rom && self.half_a_bit_out(b + 2) {
                return Some(whole);
            }
        }
        // Only asked: the reading that proves most stands, the pair whole before either form and
        // one form before the other only where both read the same bytes. The window opens by
        // where the pilot's last edge lies: one half-wave is as far off on hiss as on a pilot.
        let back_open = back_off <= ASK;
        let fwd_open = orphan.filter(|_| fwd_off <= ASK);
        let last_open = pair.is_some_and(|p| p >= floor - (-last).max(0.0) && p <= ceiling + last.max(0.0));
        if !back_open && fwd_open.is_none() && !last_open {
            return None;
        }
        let sum = self.rank(whole);
        let fwd_read = fwd_open.map(|o| (fwd(o), self.reading(fwd(o))));
        let back_read = back_open.then(|| self.reading(back));
        let ask = |r: &Option<(Start, (Proof, Vec<u8>))>| r.as_ref().map_or(Proof::None, |(_, (p, _))| *p);
        let top = sum.max(ask(&fwd_read)).max(back_read.as_ref().map_or(Proof::None, |(p, _)| *p));
        if top < Proof::Checksum || self.rom_rank() >= top {
            return None;
        }
        if sum == top {
            return Some(whole);
        }
        match (fwd_read, back_read) {
            (Some((f, (fp, fwd_bytes))), Some((bp, back_bytes))) if fp == top && bp == top => {
                (fwd_bytes == back_bytes).then_some(if fwd_off < back_off { f } else { back })
            }
            (Some((f, (fp, _))), _) if fp == top => Some(f),
            _ => Some(back),
        }
    }

    /// What the ROM's own reading proves ([`rank`]): nothing where it read no sync pair.
    fn rom_rank(&self) -> Proof {
        *self.rom_rank.get_or_init(|| self.rom.map_or(Proof::None, |at| self.rank(Start::At(at))))
    }

    /// Whether the ROM's reading lies half a bit behind the bits read from pulse `at`: it begins
    /// one pulse later, and the reading from `at` is a block whose pairs lie nearer the bits'
    /// own lengths ([`fit`]).
    ///
    /// Half a bit out, a pair straddles two bits; where they differ it lies between a 0's pair
    /// and a 1's, and decides by which side of the split the recording moved it. A recording
    /// that lengthens the second half of a 1 bit and the first of the 0 after it (two channels
    /// a few samples apart, mixed) puts a 1 then a 0 over the split and a 0 then a 1 under it,
    /// so most bits read right and a byte goes wrong only where a pair lands just past the
    /// split. The checksum is the XOR of every byte, so two such bytes wrong in the same bit
    /// leave it holding.
    fn half_a_bit_out(&self, at: usize) -> bool {
        self.rom == Some(at + 1)
            && self.rank(Start::At(at)) >= Proof::Checksum
            && self.fit(at) < self.fit(at + 1)
    }

    /// Whether the pulse at `b` is pilot: within [`WIDTH`] of the leader's half, or within
    /// [`KIND_WIDTH`] of the leader's halves of its parity.
    ///
    /// Falls slow by a millisecond and more part the pilot's halves by more than [`WIDTH`]
    /// either way, the pairs held, and each long one is then as long as a backward merge: read
    /// from each, the rest of the pilot is 1 bits, 0xFF bytes that leave the checksum as it
    /// was, and of a thousand such readings one holds by chance. Every half of a kind is as
    /// long as the others of its kind, and no merge is: a backward merge is longer by the sync
    /// pair, and the half a forward merge leaves is a bit's. Pairs cannot tell the two apart:
    /// a merged pulse and the half behind it can sum to a pilot pair.
    fn pilot(&self, b: usize) -> bool {
        let t = self.pulses[b].tstates;
        (t - self.pilot_half).abs() <= WIDTH * self.pilot_half
            || (t - self.kinds[b % 2]).abs() <= KIND_WIDTH * self.pilot_half
    }

    /// How far the pilot's edges lie off their transitions, from its halves up to pulse `b`: how
    /// much later its last kind of edge lies than the other kind, and how much earlier its last
    /// edge lies than the others of its kind. Both are 0 where fewer than [`TONE_RUN`] halves are
    /// pilot.
    ///
    /// A pilot's halves are equal on the tape, and a half that starts on one kind of edge and ends
    /// on the other measures its length plus the first edge's displacement less the second's, so
    /// the halves of the two kinds differ by twice the displacement between the kinds. The sync
    /// pair runs from the pilot's last edge to the next edge of that kind, so a skew common to one
    /// kind cancels in its sum, and what is left is how far each of its edges lies off, which
    /// grows with the lobe the edge closes; the pilot's lobes are the longest a block holds, so
    /// their skew bounds the pair's, on the side it names. The pilot's last half starts on an
    /// edge of the other kind like the others of its kind, so it falls short of them by as much
    /// as its own last edge lies early.
    fn skew(&self, b: usize) -> (f64, f64) {
        let (mut sum, mut n) = ([0.0f64; 2], [0usize; 2]);
        let mut last_half = 0.0;
        for (k, i) in (0..b).rev().enumerate() {
            if !self.pilot(i) {
                break;
            }
            let t = self.pulses[i].tstates;
            if k == 0 {
                last_half = t;
            }
            sum[k % 2] += t;
            n[k % 2] += 1;
        }
        if n[0] + n[1] < TONE_RUN || n[1] == 0 {
            return (0.0, 0.0);
        }
        let kind = (sum[0] / n[0] as f64 - sum[1] / n[1] as f64) / 2.0;
        let last = (sum[0] - last_half) / (n[0] - 1) as f64 - last_half;
        (kind, last)
    }

    /// Whether a half-wave of `t` T-states can be half a bit: the bit loop reads it doubled as
    /// one, and it is no bounce.
    fn data_half(&self, t: f64) -> bool {
        t >= BOUNCE_MOST && 2.0 * t <= self.speed.of(super::bit_timeout(0))
    }

    /// The bits of a forward merge: a first bit of which only `orphan` is left, the rest from
    /// pulse `at`. A ROM header's first bit, the flag's top bit, is a 0, so where the orphan
    /// reads as a 1 and the reading with a 0 there is a header, the bit is the header's: only a
    /// crossing the recording moved past the split read it otherwise.
    fn orphan(&self, orphan: f64, at: usize) -> Start {
        let start = Start::Lone(orphan, at);
        if 2.0 * orphan <= self.speed.of(super::ONE_ABOVE) {
            return start;
        }
        let zero = Start::Lone(self.speed.of(f64::from(RomTimings::ZERO)), at);
        match self.reading(zero) {
            (Proof::Header, _) => zero,
            _ => start,
        }
    }

    /// The bytes read from `start` as the block a header in front asks for, or without one,
    /// and what they prove ([`proof::of`]): nothing where they do not end at a byte, or run
    /// short of what the header asked.
    fn reading(&self, start: Start) -> (Proof, Vec<u8>) {
        let read = read_bytes(self.pulses, start, self.expected, self.pilot_half);
        if read.short || !read.whole {
            return (Proof::None, read.bytes);
        }
        (proof::of(&read.bytes), read.bytes)
    }

    /// What the bytes read from `start` prove, as a block: nothing short of a checksum that holds
    /// over more than a misread leaves ([`Proof::Checksum`]; a header must be one in every
    /// field, which one read a bit out is not).
    fn rank(&self, start: Start) -> Proof {
        match self.reading(start).0 {
            p if p >= Proof::Checksum => p,
            _ => Proof::None,
        }
    }

    /// The bits of a hidden forward merge at `b`, where the pulse before it is the merged one and
    /// passed for pilot.
    ///
    /// `b` must be half a bit, and with the pulse behind it be nearer a bit's pair than the
    /// sync pair's sum `syncs` (a real sync pair misses a 0 bit's pair by 18%), and the pulse
    /// before it must lie within [`DECIDE`] of `syncs` and `b`. Neither length can decide: a
    /// merge with a 0 bit's half is 4% off a pilot half-wave, and a sync pair stretched by wow
    /// passes both tests. The readings do: the merge stands where its reading proves more than
    /// the ROM's, and where the two prove as much, where its bits pair nearer the bits' own
    /// lengths than the ROM's ([`fit`]).
    fn hidden(&self, b: usize, syncs: f64) -> Option<Start> {
        let len = |k: usize| self.pulses.get(k).map(|p| p.tstates);
        let (before, orphan, next) = (len(b.checked_sub(1)?)?, len(b)?, len(b + 1)?);
        if !self.data_half(orphan) {
            return None;
        }
        let pair = orphan + next;
        let off = |nominal: f64| (pair - nominal).abs() / nominal;
        let (zero, one) = (
            self.speed.of(2.0 * f64::from(RomTimings::ZERO)),
            self.speed.of(2.0 * f64::from(RomTimings::ONE)),
        );
        if off(zero).min(off(one)) >= off(syncs) {
            return None;
        }
        let pred = syncs + orphan;
        if (before - pred).abs() / pred > DECIDE {
            return None;
        }
        let start = self.orphan(orphan, b + 1);
        let (merge, rom) = (self.rank(start), self.rom_rank());
        let decided = self.fit(b + 1) < self.fit(b + 2);
        (merge > rom || merge == rom && decided).then_some(start)
    }

    /// How far the pairs from pulse `from` lie from the bits' own lengths, on average, as a share:
    /// [`FIT_BITS`] of them at most, as far as the block goes.
    ///
    /// Half a bit out, each pair straddles two bits, and is as long as either wherever the two
    /// are equal; only where they differ does it lie between a 0's pair and a 1's, as far from
    /// both as a pair can. So the phase shows over many pairs, and in none alone.
    fn fit(&self, from: usize) -> f64 {
        let (zero, one) = (
            self.speed.of(2.0 * f64::from(RomTimings::ZERO)),
            self.speed.of(2.0 * f64::from(RomTimings::ONE)),
        );
        let (mut total, mut n) = (0.0, 0usize);
        for k in 0..FIT_BITS {
            let i = from + 2 * k;
            let (Some(a), Some(b)) = (self.pulses.get(i), self.pulses.get(i + 1)) else { break };
            let pair = a.tstates + b.tstates;
            if pair > self.speed.of(super::bit_timeout(1))
                || super::pilot_run_at(self.pulses, i, self.pilot_half)
                || super::pilot_run_at(self.pulses, i + 1, self.pilot_half)
            {
                break;
            }
            let off = |nominal: f64| (pair - nominal).abs() / nominal;
            total += off(zero).min(off(one));
            n += 1;
        }
        if n == 0 {
            f64::INFINITY
        } else {
            total / n as f64
        }
    }
}
