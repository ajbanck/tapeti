// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Distortions for the decoder's tests: what a worn tape and a tired cassette deck do to a clean
//! signal. Each takes pulses and gives new ones placed on a T-state grid. Randomness comes from
//! a seeded generator and the wobble is a triangle wave, so a distorted tape is the same on
//! every run and platform. And [`same_tape`], what a decode of one is held to.

// Each test file uses its own share of these.
#![allow(dead_code)]

pub mod degrade;
pub mod rom;

use std::ops::RangeInclusive;
use tapeti_core::types::{Block, Body};
use tapeti_decode::pulses::{from_tstates, Pulse};

/// xorshift64*: small, seeded, the same everywhere.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    /// A number from 0.0 up to, not including, 1.0.
    pub fn unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn placed(lengths: impl IntoIterator<Item = f64>) -> Vec<Pulse> {
    let mut start = 0.0;
    lengths
        .into_iter()
        .map(|tstates| {
            let p = Pulse { tstates, start, quiet: false };
            start += tstates;
            p
        })
        .collect()
}

/// Every pulse `factor` times as long: 1.02 is a tape playing 2% slow.
pub fn speed(pulses: &[Pulse], factor: f64) -> Vec<Pulse> {
    placed(pulses.iter().map(|p| p.tstates * factor))
}

/// Wow and flutter: the speed swings by up to `depth` (0.03 is ±3%) and back, once every
/// `period` T-states, following a triangle wave.
pub fn wobble(pulses: &[Pulse], depth: f64, period: f64) -> Vec<Pulse> {
    let mut t = 0.0;
    placed(pulses.iter().map(|p| {
        let phase = (t % period) / period;
        let swing = 4.0 * (phase - 0.5).abs() - 1.0;
        t += p.tstates;
        p.tstates * (1.0 + depth * swing)
    }))
}

/// Each pulse longer or shorter by up to `amount` T-states, at random, never below one.
pub fn jitter(pulses: &[Pulse], amount: f64, seed: u64) -> Vec<Pulse> {
    let mut rng = Rng::new(seed);
    placed(pulses.iter().map(|p| (p.tstates + amount * (2.0 * rng.unit() - 1.0)).max(1.0)))
}

/// A click every `every` pulses: a spike of `width` T-states somewhere inside the pulse, which
/// splits it into three. A pulse no longer than the spike is left alone.
pub fn clicks(pulses: &[Pulse], every: usize, width: f64, seed: u64) -> Vec<Pulse> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(pulses.len() + 2 * pulses.len() / every.max(1));
    for (i, p) in pulses.iter().enumerate() {
        if (i + 1) % every.max(1) == 0 && p.tstates > width {
            let before = (p.tstates - width) * rng.unit();
            out.extend([before, width, p.tstates - width - before]);
        } else {
            out.push(p.tstates);
        }
    }
    placed(out)
}

/// The lengths of `pulses`, rounded to whole T-states, for comparing.
pub fn lengths(pulses: &[Pulse]) -> Vec<u64> {
    pulses.iter().map(|p| p.tstates.round() as u64).collect()
}

/// Pulses of the given lengths, for building a signal by hand.
pub fn pulses(lengths: &[u64]) -> Vec<Pulse> {
    from_tstates(lengths)
}

/// Whether `got` is the tape `want` as a decode gives it back: the same blocks in the same order
/// with the same bytes, and each pause as long within 2 ms once `speed` has stretched it (one
/// factor for a tape played off speed, a range for one that wobbles). A pause of 0 ms only where
/// `want` has one: a block that runs on into something that is not the next block's pilot has
/// lost its pause. The first difference, in words.
pub fn same_tape(got: &[Block], want: &[Block], speed: RangeInclusive<f64>) -> Result<(), String> {
    let pause = |b: &Block| match b.body {
        Body::Standard { pause, .. } | Body::Csw { pause, .. } | Body::Pause { pause } => pause,
        _ => 0,
    };
    let without_pause = |b: &Block| match &b.body {
        Body::Standard { data, .. } => Body::Standard { pause: 0, data: data.clone() },
        Body::Csw { sample_rate, compression, pulse_count, data, .. } => Body::Csw {
            pause: 0,
            sample_rate: *sample_rate,
            compression: *compression,
            pulse_count: *pulse_count,
            data: data.clone(),
        },
        Body::Pause { .. } => Body::Pause { pause: 0 },
        body => body.clone(),
    };
    let kinds =
        |blocks: &[Block]| blocks.iter().map(|b| format!("{:02x}", b.id())).collect::<Vec<_>>().join(" ");
    if got.len() != want.len() {
        return Err(format!(
            "{} blocks [{}] where the tape has {} [{}]",
            got.len(),
            kinds(got),
            want.len(),
            kinds(want)
        ));
    }
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        if without_pause(g) != without_pause(w) {
            return Err(format!("block {i} is {:?}, not {:?}", g.body, w.body));
        }
        let (g, w) = (f64::from(pause(g)), f64::from(pause(w)));
        let (lo, hi) = (w * speed.start() - 2.0, w * speed.end() + 2.0);
        if (g == 0.0) != (w == 0.0) || g < lo || g > hi {
            return Err(format!("block {i} has a pause of {g} ms, not {w} ms"));
        }
    }
    Ok(())
}
