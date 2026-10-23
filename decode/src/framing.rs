// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Framing: standard blocks read from pulses as the 48K ROM's loader (`056B` to `0604`) reads
//! them, with its windows and nothing wider.
//!
//! The ROM counts how many times its 59 T-state sampling loop runs between two edges, giving
//! `c + 59 × reads` T-states, where `c` is the loop's other work: delay, edge search, calls,
//! returns and bookkeeping. An edge can fall anywhere in a 59 T band, so every limit here is the
//! band's low end: the counts are the ROM's, the T-states the plain sum. Past the leader, the
//! sync and bit windows scale with the tape's speed, measured from the leader's own pairs
//! ([`Speed`]). The windows are uncontended: on a 48K in the screen's lines a read can wait for
//! the ULA, widening every window by 2.6 to 4%, so these are the strictest a real machine applies.

mod merged;

pub use merged::{ASK, DECIDE, WIDTH};

use crate::fallback::TONE_RUN;
use crate::proof::{self, Proof};
use crate::pulses::Pulse;
use std::sync::atomic::{AtomicBool, Ordering};
use tapeti_core::types::RomTimings;

/// The T-states the sampling loop takes for each read.
const READ: f64 = 59.0;

/// The T-states of `reads` reads in a loop whose other work is `c`.
const fn after(c: f64, reads: f64) -> f64 {
    c + READ * reads
}

/// The leader loop (`LD-LEADER`, `0580`): a pair of pulses is pilot when it takes 43 to 99
/// reads. Its other work is 895 T; the first pair it checks comes in at `0580` and skips the
/// loop's `LD A,C6 … JR NZ`, 34 T less.
const LEADER_C: f64 = 895.0;
const LEADER_FIRST_C: f64 = 861.0;
/// A pair of this many reads or fewer is too short for pilot.
const LEADER_TOO_FEW: f64 = 42.0;
/// The counter runs out after this many reads.
const LEADER_MOST: f64 = 99.0;

/// How many pairs in a row the leader loop checks before it looks for the sync, behind the one
/// it takes unchecked.
pub const LEADER_PAIRS: usize = 256;

/// The fewest pairs a leader the ROM gives up on may hold and still be locked on.
///
/// The block behind such a leader stands only where it proves itself ([`next_block`]). The
/// count is a tone of `TONE_RUN` pulses, the run that makes a burst block-shaped. The ROM's 256
/// pairs, and the second it waits before them, are its guard against reading noise as a block;
/// a recording that starts inside a pilot, or a dropout that breaks one, leaves fewer pairs in
/// front of the sync, and the ROM loads nothing there. From above, the shortest pilot any known
/// loader writes is 65 half-waves. From below, a fragment of a dozen, which no loader loads, is
/// no tone.
pub const SHORT_LEADER_PAIRS: usize = TONE_RUN / 2;

/// How fast a block plays against the saver's own timing, from its leader: half the mean pilot
/// pair over the saver's 2,168 T pilot half, 1.1 for a tape playing 10% slow.
///
/// The ROM's windows are counts of its sampling loop, so a tape off speed moves every pair
/// towards one side of a fixed split, and a pair at the edge of a window falls out of it. Once
/// the leader has locked, the sync and bit windows are scaled by the speed, as they would be in
/// a Spectrum whose clock ran as far off: the pilot is the tape's longest and steadiest signal,
/// and its 256 pairs are measured before any of it is read. The leader's own window cannot
/// follow the speed it measures and stays the ROM's, which bounds the speed to 0.78 to 1.55;
/// so do the pilot pulses the sync loop takes and [`BOUNCE_MOST`]. The ROM itself reads only to
/// about 1.47 slow, where a 0 bit's pair reaches the split. Measured on worn renders, wow on a
/// tape 15% to 30% off speed, or jitter on one 40% slow, reads with the scaling and not without,
/// and no distortion tried reads worse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speed(pub f64);

impl Speed {
    /// The speed a leader of pilot pairs whose halves average `pilot_half` T-states measures.
    pub fn of_pilot(pilot_half: f64) -> Self {
        Speed(pilot_half / f64::from(RomTimings::PILOT))
    }

    /// `t` T-states of the saver's timing at this speed.
    pub fn of(self, t: f64) -> f64 {
        self.0 * t
    }
}

/// The sync loop (`LD-SYNC`): it reads one edge at a time. A pulse of 10 reads or fewer is the
/// first sync pulse; 11 to 54 is more pilot; at the 55th read the counter runs out and the ROM
/// starts again. Its other work is 458 T behind a pulse read in this loop, 464 T straight out of
/// the leader loop.
const SYNC_C: f64 = 458.0;
const SYNC_FROM_LEADER_C: f64 = 464.0;
const SYNC_MOST_READS: f64 = 10.0;
const SYNC_TIMEOUT_READS: f64 = 54.0;
/// The second sync pulse only has to arrive before the same counter runs out: 53 reads less
/// those the first took, with 505 T of other work.
const SYNC2_C: f64 = 505.0;
const SYNC2_SHARE: f64 = 53.0;

/// The longest pulse the sync loop still takes as pilot, on its usual path.
pub const PILOT_PULSE_MOST: f64 = after(SYNC_C, SYNC_TIMEOUT_READS);

/// Whether the sync loop, on its usual path, takes a pulse of `t` T-states as more pilot: longer
/// than a sync pulse, before its counter runs out.
pub fn pilot_pulse(t: f64) -> bool {
    t > after(SYNC_C, SYNC_MOST_READS) && t <= PILOT_PULSE_MOST
}

/// Where a bit's pair stops being a 0 and becomes a 1.
///
/// A pair of 28 reads or more is a 1, and the first bit of a byte runs more instructions than the
/// others, so the threshold lies by position: above 2,481 T mid-byte, 2,478 T at a data byte's
/// first bit, 2,482 T at the first data byte's and 2,493 T at the flag's. Every band lies inside
/// 2,478 to 2,551 T, and any split there reads a pair as the ROM does wherever the ROM's verdict
/// does not turn on the phase. The split is one for every position, inside the mid-byte bits'
/// own band (2,481 to 2,540 T), where all but one in eight bits fall. A block is read with it
/// at its own [`Speed`].
pub const ONE_ABOVE: f64 = 2510.0;

/// The bit loop (`LD-8-BITS`) runs out after 79 reads, and the load stops. Its other work by
/// position: 888 T mid-byte, 885 T at the first bit of a data byte from the second on, 889 T at
/// the first data byte's, 900 T at the flag's (whose counter starts at `B0h` straight from the
/// sync).
const BIT_MOST: f64 = 79.0;
const BIT_C: f64 = 888.0;
const BIT_C_BYTE: f64 = 885.0;
const BIT_C_FIRST_BYTE: f64 = 889.0;
const BIT_C_FLAG: f64 = 900.0;

/// The longest pair the ROM reads as the bit at `bit` (counted from the flag's first).
///
/// Past it, the load stops. A decoder that does not know the length takes that as the block's
/// end. A block is read with it at its own [`Speed`].
pub fn bit_timeout(bit: usize) -> f64 {
    let c = match (bit / 8, bit % 8) {
        (_, 1..) => BIT_C,
        (0, 0) => BIT_C_FLAG,
        (1, 0) => BIT_C_FIRST_BYTE,
        _ => BIT_C_BYTE,
    };
    after(c, BIT_MOST)
}

/// Whether the `TONE_RUN` pulses from `i` are a pilot tone, by pairs.
///
/// Each pulse is one the sync loop takes for more pilot ([`pilot_pulse`]) at the speed
/// `pilot_half` measures, and pairs with a neighbour into a pilot pair, two of that half within
/// 10%. No byte of data holds such a run: a 1 bit's pair is 3,420 T against a 4,336 T pilot
/// pair, 21% under it, so a single pair cannot tell a pilot half from a plausible 1 bit's half,
/// but a run of them can. The pairs, not the halves, are the measure: a data half still pairs
/// with the pilot half behind it only 10.6% short, even on a recording that moves one kind of
/// edge against the other. A bounce, the two pulses hiss cuts into a half-wave, is folded into
/// it before pairing ([`BOUNCE_MOST`]), as the ROM, waiting after every edge, never sees it.
pub fn pilot_run_at(pulses: &[Pulse], i: usize, pilot_half: f64) -> bool {
    pilot_run_of(pulses, i, pilot_half, TONE_RUN)
}

/// Whether the `n` half-waves from pulse `i` are pilot, as [`pilot_run_at`] judges its run: each
/// one the sync loop takes for pilot, pairing with a neighbour into a pilot pair. Read a half at
/// a time, so that data, whose halves fail at once, costs a pulse or two.
fn pilot_run_of(pulses: &[Pulse], i: usize, pilot_half: f64, n: usize) -> bool {
    let speed = Speed::of_pilot(pilot_half);
    let mut halves = Bits::new(pulses, i, pilot_half);
    let pair = |a: f64, b: f64| (a + b - 2.0 * pilot_half).abs() <= 0.2 * pilot_half;
    // The half before the last read, and whether it paired with the one before it.
    let (mut prev, mut paired_back) = (None, false);
    for _ in 0..n {
        let Some(t) = halves.half(f64::INFINITY) else { return false };
        if !pilot_pulse(t / speed.0) {
            return false;
        }
        if let Some(p) = prev {
            let forward = pair(p, t);
            if !paired_back && !forward {
                return false;
            }
            paired_back = forward;
        }
        prev = Some(t);
    }
    paired_back
}

/// Whether the `TONE_RUN` half-waves from pulse `i` are a pilot tone by the bytes the bit loop
/// would read from them: each half one the sync loop takes for pilot, and each byte's 16 halves
/// together as long as that many pilot halves within 10%.
///
/// Summing a byte at a time survives a crossing moved within it, such as hum clipped on a
/// pilot's crests, which [`pilot_run_at`]'s pairing test would miss. No byte of data is that
/// long: 16 data halves lie 21% under 16 pilot halves, so the run is looked for at a byte
/// boundary or one pulse behind it (`Bits::pair`), holding a byte of data, or 15 of its halves
/// 20% under, if it starts a byte early. A sum tells a pilot from bits only at the speed the
/// bits were read at: a snag slows bits until bytes of them sum to a pilot's against the speed
/// of the pilot before them.
fn tone_at(pulses: &[Pulse], i: usize, pilot_half: f64) -> bool {
    let speed = Speed::of_pilot(pilot_half);
    let mut halves = Bits::new(pulses, i, pilot_half);
    let want = BYTE_PULSES as f64 * pilot_half;
    (0..TONE_RUN / BYTE_PULSES).all(|_| {
        let mut sum = 0.0;
        for _ in 0..BYTE_PULSES {
            match halves.half(f64::INFINITY) {
                Some(t) if pilot_pulse(t / speed.0) => sum += t,
                _ => return false,
            }
        }
        (sum - want).abs() <= 0.1 * want
    })
}

/// Whether the next block's lead-in starts at pulse `i`, behind a block read at `pilot_half`
/// whose bits so far run at `bit1_half` a 1 bit's half.
///
/// The pulse is longer than any 1 bit's half ([`SECOND_HALF_MOST`] at the block's speed), and
/// the [`TONE_RUN`] pulses from it are each inside the pilot window ([`WIDTH`]) and over the 1
/// bit's top at the larger of the block's speed and its bits', none quiet, and a tone: the
/// largest at most 1.1 / 0.9 times the smallest, as [`pilot_run_at`] pairs a tone. The saver
/// writes no half in data longer than a 1 bit's, so such a run holds no bit of this block's, at
/// whatever pace against its leader: a lead-in that wow or flutter has carried off it. A run
/// whose mean lies within 10% of the block's pilot is left out on purpose: asked at every bit,
/// the mean takes the pilot behind a sync that hiss made inside a pilot, stopping the reading
/// after a byte or two read as 1s, with two 0xFF bytes keeping the checksum.
fn lead_in_at(pulses: &[Pulse], i: usize, pilot_half: f64, bit1_half: f64) -> bool {
    let speed = Speed::of_pilot(pilot_half);
    let Some(run) = pulses.get(i..i + TONE_RUN) else { return false };
    let pilot = |t: f64| (t - pilot_half).abs() <= WIDTH * pilot_half;
    if run[0].tstates <= speed.of(SECOND_HALF_MOST) || !run.iter().all(|p| pilot(p.tstates)) {
        return false;
    }
    let top =
        SECOND_HALF_MOST / f64::from(RomTimings::ONE) * bit1_half.max(speed.of(f64::from(RomTimings::ONE)));
    let (lo, hi) =
        run.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), p| (lo.min(p.tstates), hi.max(p.tstates)));
    run.iter().all(|p| p.tstates > top && !p.quiet) && hi <= lo * 1.1 / 0.9
}

/// Whether a pilot as long as the ROM's leader, [`LEADER_PAIRS`] pairs, starts at pulse `i`: the
/// tone a Spectrum locks on the next block from, which a run of 1 bits slowed to a pilot's pace
/// is not.
fn leader_run_at(pulses: &[Pulse], i: usize, pilot_half: f64) -> bool {
    pilot_run_of(pulses, i, pilot_half, 2 * LEADER_PAIRS)
}

/// Whether a pilot run ([`pilot_run_at`]) starts at any of the half-waves of a byte from pulse
/// `at` on, or with `through_ones` further on while their pairs read as 1 bits: where bits read
/// from `at` would be the pilot they were taken in.
///
/// A ROM block holds a flag and a checksum at least, so the next block's pilot cannot start
/// inside its first byte, and a pilot run is no data. Bits that begin there begin at a glitch
/// inside a pilot, which spoils the half-waves behind it too, so the run is looked for at each
/// of the byte's. A pilot read as bits reads as 1s, and a glitch can spoil more than a byte;
/// but so does a block of nothing but 0xFF bytes saved with no pause, behind a sync pair that
/// says a block is there. So the run is looked for on through the 1s only behind a sync that
/// was not read but inferred ([`merged`]), whose reading must bear it out; and there, 1s to the
/// end of the recording are the pilot too, one that the recording ends inside.
fn pilot_runs_on(pulses: &[Pulse], at: usize, pilot_half: f64, through_ones: bool) -> bool {
    let speed = Speed::of_pilot(pilot_half);
    let one = |k: usize| {
        pulses.get(k..k + 2).is_some_and(|p| {
            let pair = p[0].tstates + p[1].tstates;
            pair > speed.of(ONE_ABOVE) && pair <= speed.of(bit_timeout(1))
        })
    };
    let mut k = 0;
    loop {
        if through_ones && at + k + TONE_RUN > pulses.len() {
            return (at..pulses.len().saturating_sub(1)).step_by(2).all(one);
        }
        if pilot_run_at(pulses, at + k, pilot_half) {
            return true;
        }
        k += 1;
        if k >= BYTE_PULSES && (!through_ones || k.is_multiple_of(2) && !one(at + k)) {
            return false;
        }
    }
}

/// Whether the leader loop takes `pair` T-states as a pilot pair; `first` for the first it checks.
pub fn leader_pair(pair: f64, first: bool) -> bool {
    let c = if first { LEADER_FIRST_C } else { LEADER_C };
    pair > after(c, LEADER_TOO_FEW) && pair <= after(c, LEADER_MOST)
}

/// A block read from pulses.
#[derive(Clone, Debug, PartialEq)]
pub struct RawBlock {
    /// The bytes, flag and checksum included, whole bytes only.
    pub data: Vec<u8>,
    /// The first pulse of the pilot tone.
    pub tone: usize,
    /// The first pulse of the tone the leader locked on: `tone`, or where `tone` reaches back
    /// across a break in the pilot (`tone_start`), the first pulse behind the last break.
    pub lock: usize,
    /// The pulse after the last whole byte.
    pub end: usize,
    /// Where the block's bits stop: past `end` where it runs on by whole bytes beyond the length
    /// it was asked for, at `end` otherwise.
    pub bits_end: usize,
    /// Half a pilot pair, the mean of those the leader loop checked.
    pub pilot_half: f64,
    /// Whether the bits ran out before `expected` bytes were read.
    pub short: bool,
    /// Whether a 0x00 byte behind a header, a tape copier's, was read and left out of `data`.
    pub stray_zero: bool,
    /// How many bits of the last byte were read, 1 to 8; the rest of it is 0s.
    pub used_bits: u8,
    /// Whether the sync pair had merged into a half-wave beside it, or lay past the ROM's window,
    /// and the bits were read where they are (`merged`).
    pub merged_sync: bool,
    /// Whether the leader in front held fewer pairs than the ROM's [`LEADER_PAIRS`], and the block
    /// stands on its own proof ([`SHORT_LEADER_PAIRS`]).
    pub short_leader: bool,
    /// How many of its bits were read from two half-waves that differ as no saver writes a bit's
    /// ([`UNLIKE_HALVES`]).
    pub unlike: usize,
    /// Whether a snag slowed the block past the pilot's speed and its bits were read at the
    /// speed of the bits before each ([`SPEED_BITS`]).
    pub snag: bool,
}

/// What stopped a lock.
enum Lost {
    /// The pulse at this index broke it; the ROM starts again from there.
    At(usize),
    /// A leader pair broke it, the pulse at this index, behind this many pairs that held.
    Leader(usize, usize),
    /// The pulses ran out.
    End,
}

/// The next block in `pulses` from `from` on, as the ROM finds and reads it.
///
/// `None` when there is none or `cancel` is set. `expected` is the length the ROM was asked for,
/// flag and checksum included, when a header said; without it the block ends where a pair times
/// out. Either way it ends at the next block's pilot ([`pilot_run_at`]).
///
/// A lock the ROM gives up on, or one that reads no whole byte, is searched past from where it
/// broke, as the ROM starts again: from the next edge, never from an earlier one. Where a leader
/// breaks short of the ROM's 256 pairs but past [`SHORT_LEADER_PAIRS`], the sync and the bits
/// behind it are read all the same, and the block stands where it proves itself beyond any
/// misreading: a ROM header in every field, or the block a header in front asked for, as long
/// and its checksum good ([`Proof::Answer`]).
pub fn next_block(
    pulses: &[Pulse],
    from: usize,
    expected: Option<usize>,
    cancel: &AtomicBool,
) -> Option<RawBlock> {
    let mut start = from;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        match lock(pulses, start, expected) {
            Ok(locked) => {
                let bits = locked.bits;
                if let Some(b) = block_from(pulses, from, locked, expected) {
                    return Some(b);
                }
                start = bits.pulse();
            }
            Err(Lost::Leader(i, pairs)) => {
                if pairs >= SHORT_LEADER_PAIRS {
                    let found = sync_behind(pulses, start + 2, pairs, expected)
                        .ok()
                        .and_then(|locked| block_from(pulses, from, locked, expected));
                    let proof = |b: &RawBlock| {
                        let p = match expected {
                            Some(n) => proof::answering(&b.data, n),
                            None => proof::of(&b.data),
                        };
                        if b.used_bits < 8 {
                            proof::filled_out(p)
                        } else {
                            p
                        }
                    };
                    if let Some(b) = found.filter(|b| proof(b) >= Proof::Answer) {
                        return Some(RawBlock { short_leader: true, ..b });
                    }
                }
                start = i.max(start + 1);
            }
            Err(Lost::At(i)) => start = i.max(start + 1),
            Err(Lost::End) => return None,
        }
    }
}

/// The block `locked` reads, the search having begun at `from`; `None` where it reads no byte.
fn block_from(pulses: &[Pulse], from: usize, locked: Locked, expected: Option<usize>) -> Option<RawBlock> {
    let Locked { first_checked, bits, pilot_half, merged_sync } = locked;
    let Read { mut bytes, mut end, short, gate, stray_zero, remainder, unlike, snag, pace, .. } =
        read_bytes(pulses, bits, expected, pilot_half);
    let mut used_bits = 8;
    // Bits past the last whole byte. Behind bytes that make a whole ROM block, whose checksum
    // proves where it ends, they are the seam: such as a recording's decay behind the saver's
    // last edge, whose pulses pair into a bit. Behind any other, nothing says where the block
    // ended, and they are the best read there is of a damaged block: kept as a last byte filled
    // with 0s.
    if let Some((last, n, after)) = remainder.filter(|_| !bytes.is_empty() && !complete(&bytes)) {
        bytes.push(last);
        end = after;
        used_bits = n;
    }
    if bytes.is_empty() {
        return None;
    }
    let (tone, lock) = tone_start(pulses, from, first_checked, bits.pulse(), pilot_half, bytes[0]);
    // Only a length can stop the ROM short of the bits; the bytes run out where they do
    // otherwise.
    let stopped = gate.is_some_and(|n| bytes.len() >= n);
    let bits_end = if stopped { bits_end(pulses, end, 8 * bytes.len(), pilot_half, pace) } else { end };
    Some(RawBlock {
        data: bytes,
        tone,
        lock,
        end,
        bits_end,
        pilot_half,
        short,
        stray_zero,
        used_bits,
        merged_sync,
        short_leader: false,
        unlike,
        snag,
    })
}

/// Where a block's bits begin.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Start {
    /// At this pulse.
    At(usize),
    /// With a first bit of which only one half-wave is left, this long, and the rest from the
    /// pulse.
    Lone(f64, usize),
}

impl Start {
    /// The first pulse read as a whole bit's.
    fn pulse(self) -> usize {
        match self {
            Start::At(i) | Start::Lone(_, i) => i,
        }
    }
}

/// A leader and a sync.
struct Locked {
    first_checked: usize,
    bits: Start,
    /// Half the mean pair the leader checked.
    pilot_half: f64,
    merged_sync: bool,
}

/// Where the sync loop stopped.
enum SyncLoop {
    /// At the first sync pulse, from `at`, with a second behind it from `second`, and the bits
    /// from `bits`.
    Sync { at: usize, second: usize, bits: usize },
    /// At this pulse, the counter run out.
    Timeout(usize),
    /// At the first sync pulse, from `at`, with a second too long behind it from `second`, and
    /// what follows that from `bits`.
    LongSecond { at: usize, second: usize, bits: usize },
}

/// The leader and the sync from the unchecked pair at `start`, as the ROM finds them, or as a
/// merged sync left them ([`merged`]). `expected` is the length a header in front asked for.
fn lock(pulses: &[Pulse], start: usize, expected: Option<usize>) -> Result<Locked, Lost> {
    let len = |i: usize| pulses.get(i).map(|p| p.tstates).ok_or(Lost::End);
    let first_checked = start + 2;
    for n in 0..LEADER_PAIRS {
        let i = first_checked + 2 * n;
        if !leader_pair(len(i)? + len(i + 1)?, n == 0) {
            return Err(Lost::Leader(i + 1, n));
        }
    }
    sync_behind(pulses, first_checked, LEADER_PAIRS, expected)
}

/// The sync behind the `pairs` leader pairs from `first_checked`, as the ROM finds it or as a
/// merged sync left it ([`merged`]), read at the speed the pairs measure.
fn sync_behind(
    pulses: &[Pulse],
    first_checked: usize,
    pairs: usize,
    expected: Option<usize>,
) -> Result<Locked, Lost> {
    let behind = first_checked + 2 * pairs;
    let sum: f64 = pulses[first_checked..behind].iter().map(|p| p.tstates).sum();
    let pilot_half = sum / (2 * pairs) as f64;
    // A pulse short enough for a first sync pulse with the pilot running on behind it is a glitch
    // inside the pilot, a click that split a half-wave: no block begins there ([`pilot_runs_on`]),
    // so the sync loop goes on from the pulse behind it. The ROM takes it for its sync and reads
    // the pilot as 1 bits; it loads the block only from a leader whose 256 pairs all lie behind
    // the glitch.
    let mut from = behind;
    let stop = loop {
        match sync_loop(pulses, from, Speed::of_pilot(pilot_half))? {
            SyncLoop::Sync { second, bits, .. } | SyncLoop::LongSecond { second, bits, .. }
                if pilot_runs_on(pulses, bits, pilot_half, false) =>
            {
                from = second;
            }
            stop => break stop,
        }
    };
    let (anchor, rom) = match stop {
        SyncLoop::Sync { at, bits, .. } => (at, Some(bits)),
        SyncLoop::Timeout(at) | SyncLoop::LongSecond { at, .. } => (at, None),
    };
    let locked = |bits, merged_sync| Locked { first_checked, bits, pilot_half, merged_sync };
    if let Some(bits) = merged::bits_start(pulses, behind, anchor, rom, pilot_half, expected) {
        return Ok(locked(bits, true));
    }
    match stop {
        SyncLoop::Sync { bits, .. } => Ok(locked(Start::At(bits), false)),
        SyncLoop::Timeout(i) => Err(Lost::At(i)),
        SyncLoop::LongSecond { second, .. } => Err(Lost::At(second)),
    }
}

/// The sync loop from pulse `i`, behind the leader, at the tape's `speed`, each pulse as the ROM
/// measures it ([`rom_pulse`]).
fn sync_loop(pulses: &[Pulse], mut i: usize, speed: Speed) -> Result<SyncLoop, Lost> {
    let mut c = SYNC_FROM_LEADER_C;
    loop {
        let (t, second) = rom_pulse(pulses, i).ok_or(Lost::End)?;
        if t <= speed.of(after(c, SYNC_MOST_READS)) {
            // The reads the first sync pulse took, at least one, at the tape's speed.
            let reads = ((t / speed.0 - c) / READ).ceil().max(1.0);
            let (sync2, bits) = rom_pulse(pulses, second).ok_or(Lost::End)?;
            if sync2 > speed.of(after(SYNC2_C, SYNC2_SHARE - reads)) {
                return Ok(SyncLoop::LongSecond { at: i, second, bits });
            }
            return Ok(SyncLoop::Sync { at: i, second, bits });
        }
        if t > after(c, SYNC_TIMEOUT_READS) {
            return Ok(SyncLoop::Timeout(i));
        }
        c = SYNC_C;
        i = second;
    }
}

/// The pulse from `i` as the ROM measures it, and the pulse after it; `None` past the last.
///
/// After every edge it finds, the ROM reads no level for a while: 358 T in `LD-DELAY` and its
/// callers' work, 468 T or more in all, 505 T behind the first sync pulse. Two edges inside that
/// wait, a comparator's bounce or a click, are none, and the time they take is the pulse's
/// after them: pairs of pulses lasting together under [`BOUNCE_MOST`], which the bit loop
/// folds alike ([`Bits::half`]), are added to the pulse behind them. The sync loop judges one
/// pulse at a time, so it is where this matters most; inside a block a bounce sits behind a
/// half-wave and is folded into it, which comes to the same pair.
fn rom_pulse(pulses: &[Pulse], mut i: usize) -> Option<(f64, usize)> {
    let mut lead = 0.0;
    while let (Some(a), Some(b), Some(_)) = (pulses.get(i), pulses.get(i + 1), pulses.get(i + 2)) {
        if lead + a.tstates + b.tstates >= BOUNCE_MOST {
            break;
        }
        lead += a.tstates + b.tstates;
        i += 2;
    }
    pulses.get(i).map(|p| (lead + p.tstates, i + 1))
}

/// The most two pulses inside a bit may last together and be a bounce rather than data.
///
/// A recording kept as samples of one bit, a CSW file or a Direct Recording, carries its
/// comparator's chatter as pulses: a bounce in a transition, one sample high and one low, turns
/// one bit into two, and every byte after it reads a bit out. Two pulses that together last less
/// than this, behind a half-wave and before one, are time the half before them took: they are
/// added to it, as a pair so the levels after them keep their order. It is their sum that is
/// bounded, not each pulse, because a genuine half skewed by its neighbour can be as short as
/// the longest bounce on the same tape; several pairs in a row fold while their sum stays under
/// it. Two walls. From above, a genuine pair: behind a half, the next two genuine halves last at
/// least a 0 bit's pair at the fastest speed the ROM loads, 2 × 855 × 0.831 = 1,421 T, and past
/// that a real 0 bit would fold. From below, a smaller bound folds fewer bounces and never
/// misreads: the chatter of a real one-bit recording needed 480 T to read whole. 485 T, half the
/// shortest 0 pair the ROM loads at every phase over random bytes, is chosen between.
pub const BOUNCE_MOST: f64 = 485.0;

/// The longest second half the last bit of a block of known length pairs with (`Bits::final_bit`).
///
/// A 1 bit's half, 1,710 T, and 35% over it, which leaves out a player's 1 ms pause level,
/// 3,500 T, standing in its place.
pub const SECOND_HALF_MOST: f64 = 2308.0;

/// The half-wave the saver closes a block with, behind its last bit's (`SA-BYTES`' last edge).
const CLOSING_HALF: f64 = 945.0;

/// The longest pair a last 0 bit makes with the saver's closing half run into its second half:
/// 855 + 855 + 945 = 2,655 T, and 11.25% over it, the most hiss as loud as the band reads
/// through moves a half ([`WIDTH`]). A 1 bit's pair, 3,420 T, lies over it by as much under.
const CLOSED_ZERO_MOST: f64 = 2954.0;

/// The most the two halves of a bit may differ and the bit be one a saver wrote: 1.5 of a 0
/// bit's halves, 1,282.5 T, at a block's own [`Speed`].
///
/// The saver writes both halves of a bit through the same delay loop, alike within a few
/// T-states. A lobe the signal loses, such as a short one treble loss shrank or one a wander
/// lifted off the line, merges three half-waves into one and sums its pair to a 1's or more,
/// misreading two bits as one; the checksum then holds one time in 256. The sum alone cannot
/// tell such a pair, but its halves can, since the merge puts a 0 bit's pair, 1,710 T, between
/// them. The bound lies between how far a genuine bit's halves part from below, 957 T (a
/// wander-lifted lobe), and from above, 1,543 T (the 0 bit's pair less three crossings' worth of
/// hiss); a wider parting, up to 1,030 and 2,345 T on a recording whose one kind of edge comes
/// late, is still read by its sum, only counted ([`RawBlock::unlike`]): of two readings that both
/// check out, the one through fewer such pairs is the tape's (`crate::fold`).
pub const UNLIKE_HALVES: f64 = 1.5 * RomTimings::ZERO as f64;

/// How far hiss as loud as the band reads through moves a crossing, as a share of the half-wave
/// it closes: the crest the band leaves on a lobe's slope, √2 / 4π, 11.25%.
pub const HALF_CREST: f64 = std::f64::consts::SQRT_2 / (4.0 * std::f64::consts::PI);

/// The peak of a consumer transport's wow, 0.3% RMS, as a sinusoid's: 0.42%.
pub const WOW_PEAK: f64 = std::f64::consts::SQRT_2 * 0.003;

/// How many bits before a pair give the speed it is read at again where a snag slowed the
/// block (`Bits::bits_split`): their median.
///
/// A snag is none of the speed errors the pilot measures or the windows hold: the tape, held
/// back on its way to the capstan by a pack wound tight, a sticking pressure pad or a crease,
/// slips under the pinch roller for a fraction of a second, slows by tens of percent and is let
/// go. No transport bounds its depth, and past 1.47 of the pilot's speed a 0 bit's pair lies
/// past the split. From below, three are the fewest whose median holds the speed through one
/// disturbed bit, which moves a mean; from above, each more lags the snag by another bit.
pub const SPEED_BITS: usize = 3;

/// The bits of a block, read a pair of half-waves at a time as the bit loop reads them, with
/// bounces folded ([`BOUNCE_MOST`]).
struct Bits<'a> {
    pulses: &'a [Pulse],
    /// The next pulse to read.
    at: usize,
    pilot_half: f64,
    speed: Speed,
    /// The pairs read so far whose halves differ as no saver writes a bit's ([`UNLIKE_HALVES`]).
    unlike: usize,
    /// Whether each bit is read at the speed of the bits before it rather than the pilot's
    /// ([`Bits::one`]).
    at_bits: bool,
    /// The last [`SPEED_BITS`] whole bits, oldest first, each as a 0 bit's pair at its speed (a
    /// 1's halved), and how many have been taken. A pair counted unlike is kept out
    /// (`disturbed`): it is a merge's, no speed of the tape's.
    window: [f64; SPEED_BITS],
    taken: usize,
    disturbed: bool,
    /// Whether the first [`SPEED_BITS`] whole bits ran at the pilot's speed, inside the noise
    /// reach: a block a snag can slow.
    opened: bool,
    /// How far, as a share of the speed, the noise moves a bit's pair on this source: the crest
    /// on its two outer crossings, the wow's peak, and a step of the source's grid.
    reach: f64,
    /// Whether a bit read so far reads otherwise at the speed of the bits before it.
    departs: bool,
    /// The T-states of every whole pair read so far, and what the saver's timing gives the same
    /// bits: the block's speed as read ([`Bits::bit1_half`]).
    read: [f64; 2],
    /// Whether the block ended between a bit's halves at the next block's lead-in
    /// ([`lead_in_at`]): the bit was read from its first half, and no other follows.
    ended: bool,
}

impl<'a> Bits<'a> {
    fn new(pulses: &'a [Pulse], at: usize, pilot_half: f64) -> Self {
        let speed = Speed::of_pilot(pilot_half);
        // A pulse's T-states over its length on the source's grid: a sample, or a T-state.
        let grid = match (pulses.get(at), pulses.get(at + 1)) {
            (Some(p), Some(q)) if q.start > p.start => p.tstates / (q.start - p.start),
            _ => 1.0,
        };
        Bits {
            pulses,
            at,
            pilot_half,
            speed,
            unlike: 0,
            at_bits: false,
            window: [0.0; SPEED_BITS],
            taken: 0,
            disturbed: false,
            opened: false,
            reach: HALF_CREST / 2.0 + WOW_PEAK + grid / speed.of(f64::from(2 * RomTimings::ZERO)),
            departs: false,
            read: [0.0; 2],
            ended: false,
        }
    }

    /// The pair of halves `a` and `b`, counted where they differ as no saver writes a bit's
    /// ([`UNLIKE_HALVES`]) and the longer one is longer than a 1 bit's: a pair the ROM reads as
    /// a 0 with one half all but gone, 59 and 1,651 T, parts as far without being a merge.
    fn cell(&mut self, a: f64, b: f64) -> f64 {
        if (a - b).abs() > self.speed.of(UNLIKE_HALVES)
            && a.max(b) > self.speed.of(f64::from(RomTimings::ONE))
        {
            self.unlike += 1;
            self.disturbed = true;
        }
        a + b
    }

    /// The half-wave at the cursor, with the bounces before it ([`rom_pulse`]), which only a
    /// block's first half can have, and those behind it.
    fn half(&mut self, timeout: f64) -> Option<f64> {
        let (h, next) = rom_pulse(self.pulses, self.at)?;
        self.at = next;
        let (mut k, mut sum) = (self.at, 0.0);
        while let (Some(a), Some(b)) = (self.pulses.get(k), self.pulses.get(k + 1)) {
            if sum + a.tstates + b.tstates >= BOUNCE_MOST {
                break;
            }
            sum += a.tstates + b.tstates;
            k += 2;
        }
        // Only between two half-waves: chatter with no half-wave behind it is the gap's.
        let half_behind = self.pulses.get(k).is_some_and(|p| p.tstates >= BOUNCE_MOST && p.tstates < timeout);
        if k > self.at && half_behind {
            self.at = k;
            return Some(h + sum);
        }
        Some(h)
    }

    /// The pair of bit `bit` (counted from the flag's first), or `None` where the bit loop
    /// stops: the pair times out, the pulses run out, or at a byte boundary the next block's
    /// pilot starts, there or behind one pulse. The cursor is left where it was then. `asked`
    /// where a length the block has not reached yet was asked for.
    ///
    /// A pilot run cannot be data ([`pilot_run_at`]), so neither can a byte whose first
    /// half-wave is followed by one: that pulse is what the gap left, such as a pause level cut
    /// in two. But a run of 1 bits that a snag or the wow slows to a pilot's pace is pilot to
    /// [`pilot_run_at`] for as long as it lasts; where a length was asked for, the ROM stops
    /// only there, reads such a run as bits and loads the block. So in a block that opened at
    /// the pilot's speed and whose bits before the boundary now run slower than it past the
    /// noise reach ([`Bits::bits_split`]), only a pilot as long as the ROM's leader ends it short
    /// of that length ([`leader_run_at`]): the next block, saved with no pause behind one
    /// shorter than its header asked. Anywhere else a run ends it: a reading begun inside a
    /// pilot reads the rest of it as 1 bits at the pilot's pace, and hiss breaks a pilot short
    /// of a leader's length.
    ///
    /// The next block's lead-in ([`lead_in_at`]) is also looked for at every bit and between a
    /// bit's halves, wherever a run is: a lead-in that wow or flutter has carried off the
    /// block's leader starts inside a byte, and the bit loop would read on into it as 1 bits.
    /// Where it starts between a bit's halves, the bit is read from its first half.
    ///
    /// Where no length was asked for, nothing but the next pilot says where the block ends, and
    /// once [`SPEED_BITS`] bits are read it is also looked for byte by byte at the speed of the
    /// bits before the boundary ([`tone_at`]). Against the block's own pilot, measured before its
    /// bits, wow can carry the next pilot out of the 10% a run allows, and hum clipped on its
    /// crests leaves one half of each crest paired with neither neighbour; the bit loop then
    /// reads on through that pilot as 1 bits, and an even number of its bytes, each 0xFF, keeps
    /// the checksum.
    fn pair(&mut self, bit: usize, asked: bool) -> Option<f64> {
        if self.ended {
            return None;
        }
        let slowed = self.opened && self.bits_speed() > 1.0 + self.reach;
        let lead_in = |bits: &Self| {
            !(asked && slowed) && lead_in_at(bits.pulses, bits.at, bits.pilot_half, bits.bit1_half())
        };
        if bit > 0 && lead_in(self) {
            return None;
        }
        if bit > 0 && bit.is_multiple_of(8) {
            let pilot = if asked && slowed {
                let leader = |at: usize| leader_run_at(self.pulses, at, self.pilot_half);
                leader(self.at) || leader(self.at + 1)
            } else {
                let bits = (!asked && self.taken >= SPEED_BITS).then(|| self.pilot_half * self.bits_speed());
                let run = |at: usize| {
                    pilot_run_at(self.pulses, at, self.pilot_half)
                        || bits.is_some_and(|half| tone_at(self.pulses, at, half))
                };
                run(self.at) || run(self.at + 1)
            };
            if pilot {
                return None;
            }
        }
        let start = self.at;
        let timeout = self.speed.of(bit_timeout(bit));
        let first = self.half(timeout);
        if let Some(a) = first.filter(|&a| a <= timeout && lead_in(self)) {
            self.ended = true;
            return Some(2.0 * a);
        }
        match first.zip(self.half(timeout)) {
            Some((a, b)) if a + b <= timeout => Some(self.cell(a, b)),
            _ => {
                self.at = start;
                None
            }
        }
    }

    /// A 1 bit's half at the speed this block's bits have been read at: every pair read so far
    /// against what the saver's timing gives the same bits; at the pilot's speed before any.
    fn bit1_half(&self) -> f64 {
        let [read, nominal] = self.read;
        if nominal > 0.0 {
            f64::from(RomTimings::ONE) * read / nominal
        } else {
            self.speed.of(f64::from(RomTimings::ONE))
        }
    }

    /// Bit `bit` read from its first half alone, where its second is the gap after the block:
    /// `None` where it is not.
    ///
    /// The saver writes both halves of a bit through the same delay loops (`SA-BIT-1`, and
    /// `SA-SET` for a 1, at 0511 to 0525), so they differ by a few instructions only, and one
    /// half alone tells the bit. No half it writes in data is longer than a 1 bit's, so a second
    /// half past [`SECOND_HALF_MOST`] is no data. It is the gap when what follows it is no data
    /// either: the next block's pilot, silence past the bit loop's timeout, or the end of the
    /// recording. That is a recording whose last half-wave settles into silence with no edge to
    /// close it, or a pause level that never crosses zero, run together with it.
    fn last_before_gap(&mut self, bit: usize) -> Option<bool> {
        let half_wave = |t: f64| (self.speed.of(BOUNCE_MOST)..=self.speed.of(SECOND_HALF_MOST)).contains(&t);
        let first = self.pulses.get(self.at)?.tstates;
        if !half_wave(first) {
            return None;
        }
        let gap = match self.pulses.get(self.at + 1) {
            None => true,
            Some(second) => {
                second.tstates > self.speed.of(SECOND_HALF_MOST)
                    && (self.at + 2 >= self.pulses.len()
                        || second.tstates > self.speed.of(bit_timeout(bit))
                        || pilot_run_at(self.pulses, self.at + 2, self.pilot_half))
            }
        };
        if !gap {
            return None;
        }
        self.at += 1;
        Some(self.one(2.0 * first))
    }

    /// Bit `bit` as the last of a block whose length is known, or `None` where the bit loop stops.
    ///
    /// No length behind it catches a misread, and a gap too short for a pause can stand where
    /// its second half was: the signal settles into silence with no edge to close the half, or a
    /// player's 1 ms pause level follows it. So the second half must prove itself: a data
    /// half-wave, between the shortest half of a 0 bit's pair the ROM reads ([`BOUNCE_MOST`])
    /// and [`SECOND_HALF_MOST`], with no pilot run starting at it. Proven, the pair decides as
    /// every other bit's does; otherwise the first half decides alone, and the pulse after it is
    /// left to the gap.
    ///
    /// A pair can prove its second half and still not be the bit's: the saver closes the block
    /// with a half-wave of its own ([`CLOSING_HALF`]), and where that lobe fades into the
    /// silence a last 0's second half runs into it, 1,800 T, a half its pair proves, and pairs over
    /// the split to a 1, as the ROM reads it. Its halves are no bit's, which the saver writes alike,
    /// and the pair lies between the split and [`CLOSED_ZERO_MOST`], which no whole bit's does;
    /// there, with the second half the longer by over half the closing half, the first decides.
    fn final_bit(&mut self, bit: usize) -> Option<bool> {
        let start = self.at;
        let timeout = self.speed.of(bit_timeout(bit));
        let first = self.half(timeout).filter(|&h| h <= timeout);
        let Some(first) = first else {
            self.at = start;
            return None;
        };
        let second = self.pulses.get(self.at).map(|p| p.tstates);
        let half_wave = |t: f64| (self.speed.of(BOUNCE_MOST)..=self.speed.of(SECOND_HALF_MOST)).contains(&t);
        if second.is_some_and(half_wave) && !pilot_run_at(self.pulses, self.at, self.pilot_half) {
            let at = self.at;
            if let Some(pair) =
                self.half(timeout).filter(|&h| first + h <= timeout).map(|h| self.cell(first, h))
            {
                let closed = pair > self.split()
                    && pair <= self.speed.of(CLOSED_ZERO_MOST)
                    && pair - 2.0 * first > self.speed.of(CLOSING_HALF / 2.0);
                if closed {
                    self.disturbed = false;
                    return Some(self.one(2.0 * first));
                }
                return Some(self.whole(pair));
            }
            self.at = at;
        }
        Some(self.one(2.0 * first))
    }

    /// Whether the pair `pair` is a 1 bit, read at the split in force ([`Bits::split`]).
    fn one(&mut self, pair: f64) -> bool {
        let at_bits = self.bits_split();
        self.departs |= (pair > self.speed.of(ONE_ABOVE)) != (pair > at_bits);
        pair > self.split()
    }

    /// Whether the whole pair `pair` is a 1 bit ([`Bits::one`]), the pair then taken into the
    /// window of the bits before the next.
    fn whole(&mut self, pair: f64) -> bool {
        let one = self.one(pair);
        self.read[0] += pair;
        self.read[1] += f64::from(2 * if one { RomTimings::ONE } else { RomTimings::ZERO });
        if !std::mem::take(&mut self.disturbed) {
            self.window.rotate_left(1);
            self.window[SPEED_BITS - 1] = if one { pair / 2.0 } else { pair };
            self.taken += 1;
            if self.taken == SPEED_BITS {
                self.opened = (self.bits_speed() - 1.0).abs() <= self.reach;
            }
        }
        one
    }

    /// The split a pair is read at: the pilot's speed's, or where the block is read at the speed
    /// of the bits before each pair, theirs ([`Bits::bits_split`]).
    fn split(&self) -> f64 {
        if self.at_bits {
            self.bits_split()
        } else {
            self.speed.of(ONE_ABOVE)
        }
    }

    /// The speed of the last [`SPEED_BITS`] whole bits against the pilot's: their median.
    fn bits_speed(&self) -> f64 {
        let mut w = self.window;
        w.sort_by(f64::total_cmp);
        w[SPEED_BITS / 2] / self.speed.of(f64::from(2 * RomTimings::ZERO))
    }

    /// The split at the speed of the bits before the pair: the pilot's, moved by as far as they
    /// run slower than it past the noise reach.
    ///
    /// Every bit of a tape a snag has not slowed lies inside the reach, so their median does,
    /// and the block reads at the pilot's speed. A snag holds back a tape that ran at its
    /// pilot's speed, and never runs it faster than the capstan the pilot measured: so the
    /// bits' speed counts only in a block whose first bits ran at the pilot's, and only where
    /// they run slower. A block whose bits open off it is no block a snag slowed, but another
    /// saver's behind this pilot, which the ROM's split reads.
    fn bits_split(&self) -> f64 {
        let speed = self.bits_speed();
        let pilots = self.speed.of(ONE_ABOVE);
        if self.opened && speed > 1.0 + self.reach {
            pilots * speed / (1.0 + self.reach)
        } else {
            pilots
        }
    }
}

/// Bytes read from pulses.
struct Read {
    bytes: Vec<u8>,
    /// The pulse after the last whole byte.
    end: usize,
    /// Whether the bits ran out before the bytes asked for were read.
    short: bool,
    /// Whether the bits stopped at the end of a byte.
    whole: bool,
    /// The length the read stopped at, if any: `expected`, or a header's 19.
    gate: Option<usize>,
    /// Whether a 0x00 behind a header's 19 bytes was read and left out.
    stray_zero: bool,
    /// The 1 to 7 bits read past the last whole byte: as a byte from its top bit, filled with
    /// 0s, how many bits there are, and the pulse after them.
    remainder: Option<(u8, u8, usize)>,
    /// The pairs read whose halves differ as no saver writes a bit's ([`UNLIKE_HALVES`]).
    unlike: usize,
    /// Whether a bit reads otherwise at the speed of the bits before it ([`Bits::bits_split`]).
    departs: bool,
    /// Whether the bytes were read at the speed of the bits before each pair ([`read_bytes`]).
    snag: bool,
    /// The last [`SPEED_BITS`] whole bits read and how many were taken, and the block's pairs
    /// against the saver's timing, as `Bits` keeps them: the speed the block ended at.
    pace: Pace,
}

/// How fast a block's bits ran, as [`Bits`] keeps it: its window of the last [`SPEED_BITS`]
/// whole bits, how many were taken, and its pairs against the saver's timing.
type Pace = ([f64; SPEED_BITS], usize, [f64; 2]);

/// Whether `data` is a whole ROM block: a flag and at least a checksum, which holds.
fn complete(data: &[u8]) -> bool {
    proof::of(data) >= Proof::Trivial
}

/// A ROM header's length, flag and checksum included.
const HEADER_LEN: usize = proof::HEADER_LEN;

/// The half-waves of a byte: eight bits of two each.
pub(crate) const BYTE_PULSES: usize = 16;

/// Bytes from `start` until the bit loop stops ([`Bits::pair`]) or `expected` bytes are read.
///
/// Every bit is read at the speed the pilot measured. Where a snag slowed the block past it
/// ([`SPEED_BITS`]), the checksum fails, and some pair lies between the split at the pilot's
/// speed and the split at the speed of the bits before it, the block is read again once at the
/// bits' speed. That reading stands where it is the same bits, to the same pulse, and its
/// checksum holds: a window lags a snag let go at once, reads the 1 bits past it as 0s, and the
/// checksum is what refuses that reading.
fn read_bytes(pulses: &[Pulse], start: Start, expected: Option<usize>, pilot_half: f64) -> Read {
    let read = read_at(pulses, start, expected, pilot_half, false);
    if read.departs && !proof::xor_ok(&read.bytes) {
        let again = read_at(pulses, start, expected, pilot_half, true);
        let same = again.end == read.end
            && again.bytes.len() == read.bytes.len()
            && again.remainder.map(|(_, n, at)| (n, at)) == read.remainder.map(|(_, n, at)| (n, at));
        if same && complete(&again.bytes) {
            return Read { snag: true, ..again };
        }
    }
    read
}

/// [`read_bytes`] at the speed of the pilot, or `at_bits` of the bits before each pair.
///
/// A block whose flag reads 0x00 is a ROM header and stops at its 19 bytes, whatever was
/// expected: the ROM asks for a header by its flag, and loading data it fails at a header's
/// first byte, reading none of it. Where the 19 ask for nothing ([`proof::asks`]), a data
/// block's first byte read 0x00 by chance, and the block is read again as it would be without
/// that stop.
fn read_at(pulses: &[Pulse], start: Start, expected: Option<usize>, pilot_half: f64, at_bits: bool) -> Read {
    let read = read_gated(pulses, start, expected, pilot_half, true, at_bits);
    if read.bytes.len() == HEADER_LEN && read.bytes[0] == 0x00 && proof::asks(&read.bytes).is_none() {
        return read_gated(pulses, start, expected, pilot_half, false, at_bits);
    }
    read
}

/// [`read_at`], with `header_gate` whether a 0x00 flag stops the block at 19 bytes.
fn read_gated(
    pulses: &[Pulse],
    start: Start,
    expected: Option<usize>,
    pilot_half: f64,
    header_gate: bool,
    at_bits: bool,
) -> Read {
    let mut bits = Bits::new(pulses, start.pulse(), pilot_half);
    bits.at_bits = at_bits;
    let (mut bytes, mut end) = (Vec::new(), start.pulse());
    // A pilot running on inside the first byte is the pilot the sync was taken in, whose pairs
    // the bit loop would read as 1 bits ([`pilot_runs_on`]).
    if pilot_runs_on(pulses, start.pulse(), pilot_half, false) {
        let short = expected.is_some();
        return Read {
            bytes,
            end,
            short,
            whole: true,
            gate: expected,
            stray_zero: false,
            remainder: None,
            unlike: 0,
            departs: false,
            snag: false,
            pace: ([0.0; SPEED_BITS], 0, [0.0; 2]),
        };
    }
    let mut byte = 0u8;
    let mut bit = 0usize;
    if let Start::Lone(half, _) = start {
        byte = u8::from(bits.one(2.0 * half));
        bit = 1;
    }
    let mut gate = expected;
    loop {
        if header_gate && bytes.len() == 1 && bytes[0] == 0x00 {
            gate = Some(HEADER_LEN);
        }
        if gate.is_some_and(|n| bytes.len() >= n) {
            break;
        }
        let last = gate.is_some_and(|n| bytes.len() + 1 == n) && bit % 8 == 7;
        let before_gap = bits.last_before_gap(bit);
        let Some(one) = before_gap.or_else(|| {
            if last {
                bits.final_bit(bit)
            } else {
                bits.pair(bit, gate.is_some()).map(|p| bits.whole(p))
            }
        }) else {
            break;
        };
        byte = byte << 1 | u8::from(one);
        bit += 1;
        if bit.is_multiple_of(8) {
            bytes.push(byte);
            byte = 0;
            end = bits.at;
        }
        if before_gap.is_some() {
            break;
        }
    }
    // A tape copier that saves a header whole, its checksum included, has the ROM's saver add a
    // checksum of its own: the bytes already add up to zero, so it is a 0x00. The ROM stops at
    // the 19 bytes, and the byte behind them is read here so that its pulses stay in the
    // block that owns them and the pause is measured from the true gap.
    // Its last bit is the block's last, behind which the saver's closing half-wave lies
    // ([`Bits::final_bit`]). A byte that is no stray zero is not the header's: what the probe
    // read of it, the speed window above all (`pace`, which [`bits_end`] reads on from), is
    // put back as the 19 bytes left it.
    let (mut stray_zero, mut unlike) = (false, bits.unlike);
    if gate == Some(HEADER_LEN) && proof::asks(&bytes).is_some() {
        let kept = (bits.window, bits.taken, bits.read, bits.departs, bits.at, bits.ended);
        let mut extra = 0u8;
        let got = (bit..bit + 8).all(|b| {
            let one =
                if b == bit + 7 { bits.final_bit(b) } else { bits.pair(b, false).map(|p| bits.whole(p)) };
            one.map(|one| extra = extra << 1 | u8::from(one)).is_some()
        });
        if got && extra == 0x00 {
            end = bits.at;
            stray_zero = true;
            unlike = bits.unlike;
        } else {
            (bits.window, bits.taken, bits.read, bits.departs, bits.at, bits.ended) = kept;
        }
    }
    // Short of a length a header asked for; a header's own 19 are no request.
    let short = expected.is_some_and(|n| bytes.len() < n) && gate == expected;
    let rest = bit % 8;
    let remainder = (rest > 0).then(|| (byte << (8 - rest), rest as u8, bits.at));
    Read {
        bytes,
        end,
        short,
        whole: rest == 0,
        gate,
        stray_zero,
        remainder,
        unlike,
        departs: bits.departs,
        snag: false,
        pace: (bits.window, bits.taken, bits.read),
    }
}

/// Where the last whole byte ends that the bit loop reads from pulse `end` on, counting on from
/// bit `read`, which the caller has read already.
///
/// Whole bytes only, as a block with no length ends: 1 to 7 bits behind the last are the seam,
/// such as a recording's decay behind the saver's last edge, whose pulses pair into a bit.
///
/// They end, as such a block does, where the next block's pilot starts, at the speed of the
/// block's own last bits, `pace` ([`Read::pace`]), too: a header saved with no pause runs into
/// its block's pilot, and flutter can carry that pilot out of a run measured against the
/// header's leader, seconds before. The pairs read past the end move no speed: the pilot, read
/// as bits, would move it to its own.
fn bits_end(pulses: &[Pulse], end: usize, read: usize, pilot_half: f64, pace: Pace) -> usize {
    let mut bits = Bits::new(pulses, end, pilot_half);
    (bits.window, bits.taken, bits.read) = pace;
    let (mut bit, mut whole) = (read, end);
    while bits.pair(bit, false).is_some() {
        bit += 1;
        if bit.is_multiple_of(8) {
            whole = bits.at;
        }
    }
    whole
}

/// Where the tone holding the pair at `first_checked` begins, and where the run of it the leader
/// locked on does: back over every pulse before it that the sync loop would take as more pilot
/// ([`pilot_pulse`]), no further than `floor`.
///
/// The ROM saves its pilot as one unbroken tone, as many half-waves as the `flag` asks for. So a
/// dropout or a click that breaks the tone does not end it: pilot heard in front of the break, a
/// run of it ([`pilot_run_at`] at the block's `pilot_half`), is the same pilot's front, as long
/// as it lies inside that many half-waves, counted back from the bits at `bits` past the sync.
/// Counted as pause, it would lengthen the pause in front by its own length, and be said to be a
/// pilot the pause holds. Noise with no pilot run in it moves nothing, and neither does a pilot
/// heard whole: it fills the count by itself.
fn tone_start(
    pulses: &[Pulse],
    floor: usize,
    first_checked: usize,
    bits: usize,
    pilot_half: f64,
    flag: u8,
) -> (usize, usize) {
    let back = |mut i: usize| {
        while i > floor && pilot_pulse(pulses[i - 1].tstates) {
            i -= 1;
        }
        i
    };
    let lock = back(first_checked);
    let mut tone = lock;
    let halves = if flag < 0x80 { RomTimings::PILOT_HEADER } else { RomTimings::PILOT_DATA };
    let speed = Speed::of_pilot(pilot_half);
    let most = f64::from(halves) * pilot_half
        + speed.of(f64::from(RomTimings::SYNC1) + f64::from(RomTimings::SYNC2));
    let mut held: f64 = pulses[tone..bits].iter().map(|p| p.tstates).sum();
    let mut k = tone;
    while k > floor {
        k -= 1;
        held += pulses[k].tstates;
        if held > most {
            break;
        }
        if pilot_run_at(pulses, k, pilot_half) {
            // The run's front, as far as the count reaches.
            while k > floor && pilot_pulse(pulses[k - 1].tstates) && held + pulses[k - 1].tstates <= most {
                k -= 1;
                held += pulses[k].tstates;
            }
            tone = k;
        }
    }
    (tone, lock)
}
