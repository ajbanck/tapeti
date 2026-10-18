// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The 48K ROM's loader (`LD-BYTES`, 0556 to 0604) run over a pulse list, read by read, at each
//! of the 59 phases its sampling loop can have against the signal: whether a Spectrum loads a
//! block at every phase, at some or at none.
//!
//! The loader reads the EAR bit every 59 T-states while waiting for an edge, seen at the first
//! read after it; the delay to the next read depends on where the loader is (such as the 358 T
//! of `LD-EDGE-1` and its surrounding instructions). Every decision is a read count held in `B`,
//! so the phase of the first read, 0 to 58 T, is the only freedom a pulse list leaves. `framing`'s
//! loop constant `c` is the two delays of a pair less a read each, checked in `tests/rom.rs`.

use std::sync::atomic::AtomicBool;
use tapeti_core::types::Body;
use tapeti_decode::entry::decode_pulses;
use tapeti_decode::pulses::Pulse;

/// The T-states of one pass of the sampling loop (`LD-SAMPLE`), one read of the EAR bit.
pub const READ: f64 = 59.0;

// From the read that saw an edge to the first read for the next one, by what the loader does in
// between.
/// The second edge of a pair, `LD-EDGE-2` calling `LD-EDGE-1` again.
const SECOND: f64 = 468.0;
/// The first edge of the first leader pair checked, behind the unchecked one (`LD-LEADER` entered
/// past its loop's own instructions).
const LEADER_FIRST: f64 = 511.0;
/// The first edge of every later leader pair.
const LEADER: f64 = 545.0;
/// The first pulse the sync loop reads, straight out of the leader.
const SYNC_FROM_LEADER: f64 = 523.0;
/// Every later pulse the sync loop reads.
const SYNC: f64 = 517.0;
/// The second sync pulse.
const SYNC2: f64 = 505.0;
/// The first edge of the flag's first bit, from the second sync pulse.
const BIT_FLAG: f64 = 550.0;
/// The first edge of the first data byte's first bit, behind the flag's check.
const BIT_FIRST_BYTE: f64 = 657.0;
/// The first edge of the first bit of every later byte, the checksum included.
const BIT_BYTE: f64 = 653.0;
/// The first edge of a bit inside a byte.
const BIT: f64 = 538.0;
/// The edge searched for after a restart: `CALL LD-EDGE-1`, its delay and the first read. Where
/// a restart comes from moves this by a few dozen T-states, which only moves the phase.
const START: f64 = 402.0;
/// From the edge `LD-START` sees to the first read of the unchecked pair: the edge routine's
/// exit, `JR NC`, `LD HL,0415h`, 1,045 passes of `DJNZ` over a `B` taken as 0 (256 turns, 3,349 T
/// a pass with `DEC HL … JR NZ`) and the calls into the next search. A `B` left higher shortens
/// the first pass by up to 3,328 T, under a pilot pulse and a half.
const WAIT: f64 = 3_500_214.0;

/// The leader loop's counter: 99 reads to a pair at most.
const LEADER_B: u8 = 0x9C;
/// A leader pair whose counter ends at this or lower, 42 reads or fewer, is too short.
const LEADER_FEW: u8 = 0xC6;
/// The sync loop's counter: 54 reads at most, for the first sync pulse and the second together.
const SYNC_B: u8 = 0xC9;
/// A pulse whose counter ends at this or higher, 11 reads or more, is pilot.
const SYNC_PILOT: u8 = 0xD4;
/// The bit loop's counter: 79 reads to a pair at most.
const BIT_B: u8 = 0xB0;
/// The counter of a byte's first bit but the flag's, two reads on.
const BYTE_B: u8 = 0xB2;
/// A pair whose counter ends above this, 28 reads or more from `BIT_B`, is a 1.
const ONE: u8 = 0xCB;

/// What one call of `LD-BYTES` did.
#[derive(Clone, Debug, PartialEq)]
pub struct Load {
    /// The pulse taken as the first sync pulse, if one was.
    pub sync: Option<usize>,
    /// The bytes read, flag and checksum included, whole bytes only.
    pub bytes: Vec<u8>,
    /// Whether it returned with the carry set: the flag asked for, every byte read, the checksum
    /// good.
    pub loaded: bool,
}

/// The loader at some point of a signal.
pub struct Loader {
    /// Where each pulse ends: the edges, in T-states. Pulse `k` ends at edge `k`; the last ends the
    /// signal and is none.
    edges: Vec<f64>,
    /// The read that saw the last edge, or where the last search ran out.
    now: f64,
    /// How many edges lie at or before `now`: their parity is the level the loader holds.
    seen: usize,
}

impl Loader {
    /// The loader on `pulses`, whose first read for an edge comes `phase` T-states into them.
    pub fn new(pulses: &[Pulse], phase: f64) -> Self {
        let mut t = 0.0;
        let mut edges: Vec<f64> = pulses
            .iter()
            .map(|p| {
                t += p.tstates;
                t
            })
            .collect();
        edges.pop();
        Loader { edges, now: phase - START, seen: 0 }
    }

    /// How many edges lie at or before `t`, from `seen` on.
    fn passed(&self, t: f64) -> usize {
        self.seen + self.edges[self.seen..].partition_point(|&e| e <= t)
    }

    /// One edge, its first read `first` T-states after `now` and one every [`READ`] after it, each
    /// counted in `b`: whether one was seen before `b` ran out. A level that changed and changed
    /// back between two reads is no edge.
    fn edge(&mut self, first: f64, b: &mut u8) -> bool {
        let mut t = self.now + first;
        loop {
            // `INC B; RET Z` comes before each read.
            if *b == 255 {
                *b = 0;
                self.now = t;
                return false;
            }
            *b += 1;
            let seen = self.passed(t);
            if (seen - self.seen) % 2 == 1 {
                self.seen = seen;
                self.now = t;
                return true;
            }
            // Nothing changes before the next edge: on to the first read at or after it.
            let reads = self.edges.get(seen).map_or(f64::INFINITY, |&e| ((e - t) / READ).ceil());
            let left = f64::from(255 - *b);
            if reads > left {
                *b = 0;
                self.now = t + READ * (left + 1.0);
                return false;
            }
            *b += (reads - 1.0) as u8;
            t += READ * reads;
        }
    }

    /// The first edge after a restart, however long it takes; `false` at the end of the signal.
    fn any_edge(&mut self) -> bool {
        let mut first = START;
        while self.edges.last().is_some_and(|&e| e > self.now) {
            if self.edge(first, &mut 0) {
                return true;
            }
            first = READ;
        }
        false
    }

    /// A pair of edges, the first `first` T-states after `now`, counted in `b` from `start`: the
    /// counter at the end, or `None` where it ran out.
    fn pair(&mut self, first: f64, start: u8) -> Option<u8> {
        let mut b = start;
        (self.edge(first, &mut b) && self.edge(SECOND, &mut b)).then_some(b)
    }

    /// One call of `LD-BYTES` for a block of flag `flag` and `len` bytes behind it, checksum aside,
    /// from where the loader is. It looks for a leader until the signal ends, and leaves the loader
    /// where it returned.
    pub fn load(&mut self, flag: u8, len: usize) -> Load {
        'start: loop {
            let failed = |sync, bytes| Load { sync, bytes, loaded: false };
            if !self.any_edge() {
                return failed(None, Vec::new());
            }
            // The unchecked pair, behind the second's wait, with `B` as the wait left it.
            if self.pair(WAIT, 0).is_none() {
                continue;
            }
            for n in 0..256 {
                let first = if n == 0 { LEADER_FIRST } else { LEADER };
                if self.pair(first, LEADER_B).is_none_or(|b| b <= LEADER_FEW) {
                    continue 'start;
                }
            }
            let mut first = SYNC_FROM_LEADER;
            let sync = loop {
                let mut b = SYNC_B;
                if !self.edge(first, &mut b) {
                    continue 'start;
                }
                if b < SYNC_PILOT {
                    let sync = self.seen - 1;
                    if !self.edge(SYNC2, &mut b) {
                        return failed(Some(sync), Vec::new());
                    }
                    break Some(sync);
                }
                first = SYNC;
            };
            let (mut bytes, mut parity) = (Vec::with_capacity(len + 2), 0u8);
            for k in 0..len + 2 {
                let mut byte = 0u8;
                for bit in 0..8 {
                    let (first, start) = match (k, bit) {
                        (0, 0) => (BIT_FLAG, BIT_B),
                        (1, 0) => (BIT_FIRST_BYTE, BYTE_B),
                        (_, 0) => (BIT_BYTE, BYTE_B),
                        _ => (BIT, BIT_B),
                    };
                    let Some(b) = self.pair(first, start) else {
                        return failed(sync, bytes);
                    };
                    byte = byte << 1 | u8::from(b > ONE);
                }
                bytes.push(byte);
                parity ^= byte;
                if k == 0 && byte != flag {
                    return failed(sync, bytes);
                }
            }
            return Load { sync, bytes, loaded: parity == 0 };
        }
    }
}

/// How the loader takes one block at all 59 phases.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    /// At how many phases it loads.
    pub phases: usize,
    /// The bytes, flag and checksum included, where it loads at every phase the same bytes.
    pub bytes: Option<Vec<u8>>,
}

impl Verdict {
    /// Whether the block loads at every phase.
    pub fn every(&self) -> bool {
        self.bytes.is_some()
    }
}

/// `LD-BYTES` called for each of `blocks` (flag, length behind it) in turn over `pulses`, each
/// call where the last returned, at each phase: how each block loads.
pub fn loads(pulses: &[Pulse], blocks: &[(u8, usize)]) -> Vec<Verdict> {
    let runs: Vec<Vec<Load>> = (0..59)
        .map(|phase| {
            let mut loader = Loader::new(pulses, f64::from(phase));
            blocks.iter().map(|&(flag, len)| loader.load(flag, len)).collect()
        })
        .collect();
    (0..blocks.len())
        .map(|k| {
            let loaded: Vec<&Load> = runs.iter().map(|r| &r[k]).filter(|l| l.loaded).collect();
            let same = loaded.len() == 59 && loaded.iter().all(|l| l.bytes == loaded[0].bytes);
            Verdict { phases: loaded.len(), bytes: same.then(|| loaded[0].bytes.clone()) }
        })
        .collect()
}

/// Which of `blocks` the loader loads at every phase over `pulses`, whose starts count samples at
/// `grid_hz`, and [`decode_pulses`] does not read from them, with those bytes and in that order.
///
/// Bytes of one value are left out: an even count of them checks out whatever the value, so a
/// tape too slow for the loader still loads them, every bit read as 1, where the decoder reads
/// the tape at the speed its leader measures.
pub fn unread(pulses: &[Pulse], grid_hz: u32, blocks: &[(u8, usize)]) -> Vec<usize> {
    let decoded = decode_pulses(pulses, grid_hz, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let datas: Vec<&Vec<u8>> = decoded
        .blocks
        .iter()
        .filter_map(|b| match &b.body {
            Body::Standard { data, .. } => Some(data),
            _ => None,
        })
        .collect();
    let mut from = 0;
    let mut unread = Vec::new();
    for (k, verdict) in loads(pulses, blocks).iter().enumerate() {
        let Some(bytes) = verdict.bytes.as_ref().filter(|b| b.iter().any(|&v| v != b[0])) else {
            continue;
        };
        match datas[from..].iter().position(|d| *d == bytes) {
            Some(i) => from += i + 1,
            None => unread.push(k),
        }
    }
    unread
}

/// Whether [`decode_pulses`] reads every block [`unread`] asks about; the first it does not, in
/// words.
pub fn agrees(pulses: &[Pulse], grid_hz: u32, blocks: &[(u8, usize)]) -> Result<(), String> {
    match unread(pulses, grid_hz, blocks).first() {
        None => Ok(()),
        Some(k) => Err(format!("block {k} loads at every phase, and the decoder reads no such block")),
    }
}
