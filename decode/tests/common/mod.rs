// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Distortions for the decoder's tests: what a worn tape and a tired cassette deck do to a clean
//! signal. Each takes pulses and gives new ones placed on a T-state grid. Randomness comes from
//! a seeded generator and the wobble is a triangle wave, so a distorted tape is the same on
//! every run and platform.

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
