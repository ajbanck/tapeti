// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Framing: standard blocks read from pulses the way the 48K ROM's loader (`056B` to `0604`)
//! reads them, with its windows and nothing wider.
//!
//! The ROM never measures a pulse. It counts how many times its 59 T-state sampling loop runs
//! between two edges (one edge in the sync loop, two in the others), and a count of `reads`
//! comes to `c + 59 × reads` T-states, where `c` is what the loop does besides sampling: the
//! 358 T delay before each edge search, the edge routine's exit, calls, returns and the loop's
//! own bookkeeping. An edge can fall anywhere between two reads, so every limit is a band 59 T
//! wide in which the ROM's verdict turns on the sampling phase, which pulses do not carry. Every
//! figure here is the band's low end, as the limits below are written: the counts are the ROM's,
//! the T-states the plain sum. The windows are fixed: a tape recorded off speed reads only as
//! far as the ROM itself would load it.
//!
//! The figures are uncontended. On a 48K in the screen's lines a read can wait for the ULA,
//! which widens every window by 2.6 to 4%, so the uncontended windows are the strictest a real
//! machine applies.

use crate::pulses::Pulse;
use std::sync::atomic::{AtomicBool, Ordering};

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
/// own band (2,481 to 2,540 T), where all but one in eight bits fall.
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

/// The longest pair the ROM reads as the bit at `bit` (counted from the flag's first): past it,
/// the load stops. A decoder that does not know the length takes that as the block's end.
pub fn bit_timeout(bit: usize) -> f64 {
    let c = match (bit / 8, bit % 8) {
        (_, 1..) => BIT_C,
        (0, 0) => BIT_C_FLAG,
        (1, 0) => BIT_C_FIRST_BYTE,
        _ => BIT_C_BYTE,
    };
    after(c, BIT_MOST)
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
    /// The pulse after the last whole byte.
    pub end: usize,
    /// Where the block's bits stop: past `end` where it runs on beyond the length it was asked
    /// for, at `end` otherwise.
    pub bits_end: usize,
    /// Half a pilot pair, the mean of those the leader loop checked.
    pub pilot_half: f64,
    /// Whether the bits ran out before `expected` bytes were read.
    pub short: bool,
}

/// What stopped a lock.
enum Lost {
    /// The pulse at this index broke it; the ROM starts again from there.
    At(usize),
    /// The pulses ran out.
    End,
}

/// The next block in `pulses` from `from` on, as the ROM finds and reads it.
///
/// `None` when there is none or `cancel` is set. `expected` is the length the ROM was asked for, flag and checksum
/// included, when a header said; without it the block ends where a pair times out.
///
/// A lock the ROM gives up on, or one that reads no whole byte, is searched past from where it
/// broke, as the ROM starts again: from the next edge, never from an earlier one.
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
        match lock(pulses, start) {
            Ok((first_checked, data, pilot_half)) => {
                let (bytes, end, short) = read_bytes(pulses, data, expected);
                if !bytes.is_empty() {
                    let tone = tone_start(pulses, from, first_checked);
                    // Only a length can stop the ROM short of the bits; the bytes run out where
                    // they do otherwise.
                    let bits_end = match expected {
                        Some(_) if !short => data + 2 * bits(pulses, data, (end - data) / 2),
                        _ => end,
                    };
                    return Some(RawBlock { data: bytes, tone, end, bits_end, pilot_half, short });
                }
                start = data;
            }
            Err(Lost::At(i)) => start = i.max(start + 1),
            Err(Lost::End) => return None,
        }
    }
}

/// The leader and the sync from the unchecked pair at `start`: the first checked pair, the first
/// data pulse and half the mean checked pair.
fn lock(pulses: &[Pulse], start: usize) -> Result<(usize, usize, f64), Lost> {
    let len = |i: usize| pulses.get(i).map(|p| p.tstates).ok_or(Lost::End);
    let first_checked = start + 2;
    let mut sum = 0.0;
    for n in 0..LEADER_PAIRS {
        let i = first_checked + 2 * n;
        let pair = len(i)? + len(i + 1)?;
        if !leader_pair(pair, n == 0) {
            return Err(Lost::At(i + 1));
        }
        sum += pair;
    }
    let pilot_half = sum / (2 * LEADER_PAIRS) as f64;
    let mut i = first_checked + 2 * LEADER_PAIRS;
    let mut c = SYNC_FROM_LEADER_C;
    loop {
        let t = len(i)?;
        if t <= after(c, SYNC_MOST_READS) {
            // The reads the first sync pulse took, at least one.
            let reads = ((t - c) / READ).ceil().max(1.0);
            let sync2 = len(i + 1)?;
            if sync2 > after(SYNC2_C, SYNC2_SHARE - reads) {
                return Err(Lost::At(i + 1));
            }
            return Ok((first_checked, i + 2, pilot_half));
        }
        if t > after(c, SYNC_TIMEOUT_READS) {
            return Err(Lost::At(i));
        }
        c = SYNC_C;
        i += 1;
    }
}

/// Bytes from the pair at `from` until a pair times out, the pulses run out or `expected` bytes
/// are read: the bytes, the pulse after the last whole one, and whether `expected` was not met.
fn read_bytes(pulses: &[Pulse], from: usize, expected: Option<usize>) -> (Vec<u8>, usize, bool) {
    let mut bytes = Vec::new();
    let mut byte = 0u8;
    let mut bit = 0usize;
    while expected.is_none_or(|n| bytes.len() < n) {
        let i = from + 2 * bit;
        let Some(pair) = pulses.get(i).zip(pulses.get(i + 1)).map(|(a, b)| a.tstates + b.tstates) else {
            break;
        };
        if pair > bit_timeout(bit) {
            break;
        }
        byte = byte << 1 | u8::from(pair > ONE_ABOVE);
        bit += 1;
        if bit.is_multiple_of(8) {
            bytes.push(byte);
            byte = 0;
        }
    }
    let short = expected.is_some_and(|n| bytes.len() < n);
    let end = from + 16 * bytes.len();
    (bytes, end, short)
}

/// How many pairs from the pair at `from` the ROM reads as bits before one times out, counting on
/// from bit `read`, which the caller has read already.
fn bits(pulses: &[Pulse], from: usize, read: usize) -> usize {
    let mut bit = read;
    while let Some(pair) = pulses.get(from + 2 * bit).zip(pulses.get(from + 2 * bit + 1)) {
        if pair.0.tstates + pair.1.tstates > bit_timeout(bit) {
            break;
        }
        bit += 1;
    }
    bit
}

/// Where the tone holding the pair at `first_checked` begins: back over every pulse before it that
/// the sync loop would take as more pilot ([`pilot_pulse`]), no further than `floor`.
fn tone_start(pulses: &[Pulse], floor: usize, first_checked: usize) -> usize {
    let mut i = first_checked;
    while i > floor && pilot_pulse(pulses[i - 1].tstates) {
        i -= 1;
    }
    i
}
