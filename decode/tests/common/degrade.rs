// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! What the cassette chain does to a tape's sound, at the level of samples: one function per
//! defect, acting on `tapeti-core`'s own render of a synthetic tape. The pulse-level distortions
//! beside them in this module's parent stay for framing.
//!
//! Every function is deterministic on every platform: randomness from the seeded [`Rng`], a
//! triangle wave wherever a slow swing is wanted, and no transcendental function (`sin`, `exp`,
//! `powf`, `atan`) anywhere a sample passes through, since their last bit differs between
//! libraries. A level in dB is a whole number, turned into a ratio by multiplication alone
//! ([`below_db`]).

use super::Rng;

/// The tape's speed past the head, 4.76 cm/s (1⅞ in/s), in µm a second.
pub const TAPE_UM_PER_S: f64 = 47_625.0;

impl Rng {
    /// A number from a normal distribution of mean 0 and deviation 1, near enough: the sum of
    /// twelve uniform numbers less six.
    pub fn gauss(&mut self) -> f64 {
        (0..12).map(|_| self.unit()).sum::<f64>() - 6.0
    }
}

/// A triangle wave of unit amplitude standing in for a sine: 0 at phase 0, 1 at a quarter, 0 at
/// a half, -1 at three quarters.
pub fn tri(phase: f64) -> f64 {
    let p = phase.rem_euclid(1.0);
    if p < 0.25 {
        4.0 * p
    } else if p < 0.75 {
        2.0 - 4.0 * p
    } else {
        4.0 * p - 4.0
    }
}

/// The amplitude ratio `db` decibels down, 10^(-db/20), by multiplication only.
pub fn below_db(db: u32) -> f64 {
    // 10^(-1/20).
    const ONE_DB: f64 = 0.891_250_938_133_745_6;
    (0..db).fold(1.0, |r, _| r * ONE_DB)
}

/// Where `x` crosses zero, in samples, between two samples of opposite sign by straight-line
/// interpolation. A sample of exactly 0 counts as positive.
pub fn crossings(x: &[f32]) -> Vec<f64> {
    x.windows(2)
        .enumerate()
        .filter(|(_, w)| (w[0] < 0.0) != (w[1] < 0.0))
        .map(|(i, w)| i as f64 + f64::from(w[0]) / f64::from(w[0] - w[1]))
        .collect()
}

/// A stereo PCM WAV file of `left` and `right` at 8 or 16 bits, quantised as `encode_wav` does a
/// mono one. The longer channel is cut to the shorter.
pub fn stereo_wav(left: &[f32], right: &[f32], rate: u32, bits: u16) -> Vec<u8> {
    let frames = left.len().min(right.len());
    let size = usize::from(bits / 8);
    let data = (frames * 2 * size) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend(b"RIFF");
    out.extend((36 + data).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * 2 * size as u32).to_le_bytes());
    out.extend((2 * size as u16).to_le_bytes());
    out.extend(bits.to_le_bytes());
    out.extend(b"data");
    out.extend(data.to_le_bytes());
    for i in 0..frames {
        for s in [left[i], right[i]] {
            let s = f64::from(s).clamp(-1.0, 1.0);
            if bits == 16 {
                out.extend(((s * 32767.0 + 0.5).floor() as i16).to_le_bytes());
            } else {
                out.push(((s + 1.0) * 127.5 + 0.5).floor() as u8);
            }
        }
    }
    out
}

/// Treble loss as spacing loss: what a gap of `microns` between tape and head does, the main
/// treble loss of a worn deck and of every generation of a copy.
///
/// Wallace's spacing loss is 54.6·d/λ dB at wavelength λ = v/f, a response of exp(-2π·f·d/v):
/// it grows exponentially with frequency where a filter's corner grows by a power. Its impulse
/// response is a Lorentzian of half-width `a = d/v` seconds, `h(t) ∝ a / (a² + t²)`, which is
/// what this convolves with: `A = a·rate` samples, the weights `1/(A² + k²)` for `k` from `-K`
/// to `K` with `K = ceil(64·A)`, normalised to sum to 1, the signal held at its first and last
/// sample beyond its ends. The kernel is symmetric, so an edge between two long levels keeps its
/// place. 0 µm is the signal itself.
///
/// In the samples: a half-wave of `L` seconds between two long ones of the other sign keeps a
/// crossing only while `L > 2a`; past that its two flanks are held on the far side of zero by
/// its neighbours' and the half-wave is gone, not shrunk. Sync 1 (667 T) vanishes past 4.5 µm,
/// a 0 bit's half (855 T) past 5.8 µm. In the pulses: a sync half merges into the pilot's last
/// half or into the first data half; the shortest zeros of the data vanish.
pub fn spacing_loss(x: &[f32], rate: u32, microns: f64) -> Vec<f32> {
    // Exact convolution with the Lorentzian kernel, rewritten over the signal's differences
    // (x[j] - x[j-1], zero almost everywhere on a piecewise-constant render) against the
    // kernel's own cumulative sum (its step response): y[n] = x[0] + Σ_j d[j]·H(n-j), with
    // H(m) the kernel's weight summed from -K up to m (0 below -K, 1 at and above K). A
    // nonzero diff only needs its local window of 2K samples plus a running carry for the
    // samples past it, where the kernel has already settled to its full weight.
    if microns <= 0.0 || x.len() < 2 {
        return x.to_vec();
    }
    let a = microns / TAPE_UM_PER_S * f64::from(rate);
    let k_max = (64.0 * a).ceil() as i64;
    let width = (2 * k_max + 1) as usize;

    let mut weights = vec![0.0f64; width];
    let mut total = 0.0;
    for (idx, w) in weights.iter_mut().enumerate() {
        let k = idx as i64 - k_max;
        *w = 1.0 / (a * a + (k * k) as f64);
        total += *w;
    }
    let mut cumw = vec![0.0f64; width];
    let mut acc = 0.0;
    for (idx, w) in weights.iter().enumerate() {
        acc += *w / total;
        cumw[idx] = acc;
    }

    let n = x.len();
    let len = n as i64;
    let mut y = vec![f64::from(x[0]); n];
    let mut carry_start = vec![0.0f64; n];

    for j in 1..n {
        let d = f64::from(x[j]) - f64::from(x[j - 1]);
        if d == 0.0 {
            continue;
        }
        let jf = j as i64;
        let lo = (jf - k_max).max(0);
        let hi = (jf + k_max - 1).min(len - 1);
        for out in lo..=hi {
            let idx = (out - jf + k_max) as usize;
            y[out as usize] += d * cumw[idx];
        }
        let start = jf + k_max;
        if start < len {
            carry_start[start as usize] += d;
        }
    }

    let mut carry = 0.0;
    for (n_i, c) in carry_start.into_iter().enumerate() {
        carry += c;
        y[n_i] += carry;
    }

    y.into_iter().map(|v| v as f32).collect()
}

/// Azimuth asymmetry: a one-pole low-pass whose time constant depends on the direction it moves
/// in, `rise_us` while the input lies above the output and `fall_us` otherwise.
///
/// `y[n] = y[n-1] + c·(x[n] - y[n-1])` with `c = dt / (τ + dt)`, `dt` a sample, `y[-1] = x[0]`:
/// rational, no `exp`. In the samples: rises and falls take different times. In the pulses:
/// one kind of edge crosses zero later than the other, by about the difference of the two time
/// constants; every pulse that starts on a rise lengthens and every one that starts on a fall
/// shortens by as much, and a pair of them keeps its sum.
pub fn azimuth(x: &[f32], rate: u32, rise_us: f64, fall_us: f64) -> Vec<f32> {
    let Some(&first) = x.first() else {
        return Vec::new();
    };
    let dt_us = 1_000_000.0 / f64::from(rate);
    let mut y = f64::from(first);
    x.iter()
        .map(|&xn| {
            let xn = f64::from(xn);
            let tau = if xn > y { rise_us } else { fall_us };
            y += dt_us / (tau + dt_us) * (xn - y);
            y as f32
        })
        .collect()
}

/// A slow fade: the level is kept up to sample `from`, falls in a straight line to `depth` of
/// itself at sample `to`, and stays there to the end.
///
/// In the samples: the envelope sinks. In the pulses: none, until the band loses the short
/// lobes, and then treble loss's signature. `from` and `to` past the end are the end.
pub fn fade(x: &[f32], from: usize, to: usize, depth: f32) -> Vec<f32> {
    let depth = f64::from(depth);
    let span = to.saturating_sub(from).max(1) as f64;
    x.iter()
        .enumerate()
        .map(|(n, &v)| {
            let gain = if n <= from {
                1.0
            } else if n >= to {
                depth
            } else {
                1.0 + (n - from) as f64 / span * (depth - 1.0)
            };
            (gain * f64::from(v)) as f32
        })
        .collect()
}

/// A dropout: `len` samples from `at` at `depth` of their level (0.0 is nothing at all), the rest
/// untouched; the part past the end is left out.
///
/// Shorter than the edge detector's envelope window (±10 ms) it is a short dropout: the band
/// held up by the signal either side cuts no edge across the hole, which becomes one long pulse
/// that ends the block. Longer, the detector goes idle and the hole is a pause inside a block.
pub fn dropout(x: &[f32], at: usize, len: usize, depth: f32) -> Vec<f32> {
    let end = (at + len).min(x.len());
    let mut out = x.to_vec();
    for v in &mut out[at.min(end)..end] {
        *v *= depth;
    }
    out
}

/// DC drift as a straight ramp: `start` added at the first sample, `end` at the last, in a
/// straight line between.
///
/// In the samples: the centre line moves. In the pulses: none on a strong signal; with treble
/// loss, the short lobes of one sign stop crossing zero.
pub fn dc_ramp(x: &[f32], start: f32, end: f32) -> Vec<f32> {
    let (start, end) = (f64::from(start), f64::from(end));
    let last = x.len().saturating_sub(1).max(1) as f64;
    x.iter()
        .enumerate()
        .map(|(i, &s)| (f64::from(s) + start + (end - start) * i as f64 / last) as f32)
        .collect()
}

/// Hum, or a wandering centre line: a triangle wave ([`tri`]) of `amplitude` and `hz` added,
/// phase 0 at the first sample. 50 Hz is mains hum; a few Hz to a hundred is wander.
pub fn hum(x: &[f32], rate: u32, hz: f64, amplitude: f32) -> Vec<f32> {
    x.iter().enumerate().map(|(n, &s)| s + amplitude * tri(hz * n as f64 / f64::from(rate)) as f32).collect()
}

/// Hiss `snr_db` below the signal: noise of deviation `rms(x) · 10^(-snr_db/20)` added to every
/// sample, from [`Rng::gauss`] seeded with `seed`, `rms(x)` over the whole of `x` (pauses
/// included). Nothing is clamped.
///
/// In the samples: the floor. In the pulses: chatter in the pauses, crossings moved, false
/// edges on weak lobes.
pub fn hiss(x: &[f32], snr_db: u32, seed: u64) -> Vec<f32> {
    hiss_under(x, rms(x), snr_db, seed)
}

/// The root mean square of `x`, 0 for none.
pub fn rms(x: &[f32]) -> f64 {
    let n = x.len().max(1) as f64;
    (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / n).sqrt()
}

/// [`hiss`] `snr_db` below a level of `level` RMS rather than `x`'s own: dead air added around
/// a tape lowers its RMS, and hiss measured on it would be quieter against the signal.
pub fn hiss_under(x: &[f32], level: f64, snr_db: u32, seed: u64) -> Vec<f32> {
    let dev = level * below_db(snr_db);
    let mut rng = Rng::new(seed);
    x.iter().map(|&v| v + (dev * rng.gauss()) as f32).collect()
}

/// Clipping: the signal `drive` times as loud, cut at full scale (±1.0).
///
/// In the samples: flat tops. In the pulses: nothing, since no sample changes sign.
pub fn clip(x: &[f32], drive: f32) -> Vec<f32> {
    x.iter().map(|&v| (v * drive).clamp(-1.0, 1.0)).collect()
}

/// A tape played off speed: every stretch of it `factor` times as long, 1.02 for a tape playing
/// 2% slow, as `common::speed` has it for pulses.
///
/// Output sample `n` is the input at position `n / factor`, interpolated in a straight line
/// between the two samples either side; the output runs while that position lies inside the
/// input, so it is `floor((len - 1) · factor) + 1` samples long.
pub fn speed(x: &[f32], factor: f64) -> Vec<f32> {
    if x.len() < 2 {
        return x.to_vec();
    }
    let last = x.len() - 1;
    let len = (last as f64 * factor).floor() as usize + 1;
    (0..len)
        .map(|n| {
            let pos = n as f64 / factor;
            let i = (pos.floor() as usize).min(last - 1);
            let frac = pos - i as f64;
            (f64::from(x[i]) * (1.0 - frac) + f64::from(x[i + 1]) * frac) as f32
        })
        .collect()
}

/// A low-pass of `poles` one-pole sections, each `y[n] = y[n-1] + a·(x[n] - y[n-1])` with
/// `a = 1 - exp(-2π·corner_hz / rate)`, run in turn from rest: the RC roll-off a deck's
/// amplifier and a resampler stack up, each section 3 dB down at the corner.
///
/// In the samples: every crossing is late by the group delay, which is longer for a long lobe
/// than for a short one, so the half-waves change their widths by what they hold; a short lobe
/// between two long ones of the other sign shrinks towards the line. In the pulses: a 0 bit's
/// halves and the sync's narrow, a 1's widen, and past a corner near the pilot's tone the sync
/// pair no longer validates.
pub fn low_pass(x: &[f32], rate: u32, corner_hz: f64, poles: usize) -> Vec<f32> {
    let a = 1.0 - (-2.0 * std::f64::consts::PI * corner_hz / f64::from(rate)).exp();
    let mut y: Vec<f64> = x.iter().map(|&v| f64::from(v)).collect();
    for _ in 0..poles {
        let mut state = 0.0;
        for v in &mut y {
            state += a * (*v - state);
            *v = state;
        }
    }
    y.into_iter().map(|v| v as f32).collect()
}

/// Wow and flutter: the speed swings with a triangle wave of `hz`, every stretch of the tape
/// lasting `1 + depth · tri(hz · t)` times as long as it should at output time `t` (0.03 is ±3%),
/// as `common::wobble` has it for pulses. A few Hz is wow, tens of Hz flutter.
///
/// The input position advances by `1 / (1 + depth · tri(hz · n / rate))` from output sample `n`
/// to the next, starting at 0, and the input is read there in a straight line between the two
/// samples either side; the output stops when the position passes the last sample.
pub fn wow(x: &[f32], rate: u32, depth: f64, hz: f64) -> Vec<f32> {
    if x.is_empty() {
        return Vec::new();
    }
    let last = (x.len() - 1) as f64;
    let mut out = Vec::new();
    let mut pos = 0.0_f64;
    let mut n = 0_u64;
    while pos <= last {
        let i = pos.floor();
        let idx = i as usize;
        let frac = pos - i;
        let s0 = f64::from(x[idx]);
        let s1 = f64::from(x[(idx + 1).min(x.len() - 1)]);
        out.push((s0 + frac * (s1 - s0)) as f32);
        let speed = 1.0 + depth * tri(hz * n as f64 / f64::from(rate));
        pos += 1.0 / speed;
        n += 1;
    }
    out
}

/// A snag: the tape held back on its way to the capstan (a pack wound tight, a sticking pressure
/// pad, a crease) and let go. From input sample `from` every stretch of the tape lasts longer,
/// in a straight line over `slow` input samples up to `depth` times as long, holds there over
/// `hold`, and comes back in a straight line over `release`; before and after it the tape plays
/// at its speed.
///
/// The input position advances by `1 / s` from one output sample to the next, `s` the stretch
/// at that position, starting at 0, and the input is read there in a straight line between the
/// two samples either side; the output stops when the position passes the last sample.
pub fn snag(x: &[f32], from: usize, slow: usize, hold: usize, release: usize, depth: f64) -> Vec<f32> {
    if x.is_empty() {
        return Vec::new();
    }
    let (held, free) = ((from + slow) as f64, (from + slow + hold) as f64);
    let stretch = |pos: f64| match pos {
        p if p < from as f64 => 1.0,
        p if p < held => 1.0 + (depth - 1.0) * (p - from as f64) / slow as f64,
        p if p < free => depth,
        p if p < free + release as f64 => depth - (depth - 1.0) * (p - free) / release as f64,
        _ => 1.0,
    };
    let last = (x.len() - 1) as f64;
    let mut out = Vec::new();
    let mut pos = 0.0_f64;
    while pos <= last {
        let i = pos.floor();
        let idx = i as usize;
        let frac = pos - i;
        let s0 = f64::from(x[idx]);
        let s1 = f64::from(x[(idx + 1).min(x.len() - 1)]);
        out.push((s0 + frac * (s1 - s0)) as f32);
        pos += 1.0 / stretch(pos);
    }
    out
}

/// The signal upside down.
///
/// In the pulses: nothing for a reader of pairs; the first edge out of silence and a player's
/// 1 ms pause level are the other way up.
pub fn invert(x: &[f32]) -> Vec<f32> {
    x.iter().map(|&s| -s).collect()
}

/// A click: `width` samples from `at` replaced by `height` on the other side of zero from the
/// sample at `at` (on the positive side where that sample is 0), so that a click inside a
/// half-wave splits it in three. The part past the end is left out.
pub fn spike(x: &[f32], at: usize, width: usize, height: f32) -> Vec<f32> {
    let mut y = x.to_vec();
    if at >= y.len() {
        return y;
    }
    let sign = if x[at] > 0.0 { -1.0 } else { 1.0 };
    let end = (at + width).min(y.len());
    for v in &mut y[at..end] {
        *v = sign * height;
    }
    y
}

/// Clicks all through: in every stretch of `every` samples from the first (a last stretch
/// shorter than `every` has none), one [`spike`] of `width` and `height`, starting
/// `floor(u · (every - width + 1))` samples into it for a number `u` drawn from [`Rng::unit`]
/// seeded with `seed`, one draw per stretch.
pub fn clicks(x: &[f32], every: usize, width: usize, height: f32, seed: u64) -> Vec<f32> {
    if every == 0 {
        return x.to_vec();
    }
    let mut rng = Rng::new(seed);
    let mut out = x.to_vec();
    for k in 0..x.len() / every {
        let u = rng.unit();
        if width <= every {
            let start = k * every;
            let span = (every - width + 1) as f64;
            out = spike(&out, start + (u * span).floor() as usize, width, height);
        }
    }
    out
}

/// Stereo head skew: the two tracks of a stereo head read the same signal, one `lag` samples
/// behind the other. Returns `(left, right)`: the left is `x`, the right `x` at position
/// `n - lag`, interpolated in a straight line between the two samples either side for a
/// fractional lag, and `x[0]` before the start; both as long as `x`. `lag` is 0 or more.
///
/// In the mix: a comb, which cancels the short half-waves while the pilot keeps most of its
/// level. Each channel alone is clean.
pub fn head_skew(x: &[f32], lag: f64) -> (Vec<f32>, Vec<f32>) {
    let right = (0..x.len())
        .map(|n| {
            let p = n as f64 - lag;
            if p <= 0.0 || x.is_empty() {
                x.first().copied().unwrap_or(0.0)
            } else {
                let i = p.floor() as usize;
                let frac = p - i as f64;
                let a = f64::from(x[i.min(x.len() - 1)]);
                let b = f64::from(x[(i + 1).min(x.len() - 1)]);
                (a + frac * (b - a)) as f32
            }
        })
        .collect();
    (x.to_vec(), right)
}

/// A one-track stretch: one channel of a stereo recording silent from sample `from` up to `to`,
/// as a head that lost contact on one track leaves it; [`dropout`] at depth 0 on that channel.
/// Returns the channel with the stretch silent.
pub fn one_track(channel: &[f32], from: usize, to: usize) -> Vec<f32> {
    dropout(channel, from, to.saturating_sub(from), 0.0)
}

/// A recording made at a lower rate: `x` at `from_rate` brought to `to_rate`, each output sample
/// the mean of the input over the time it covers, as an integrating converter takes it.
///
/// Input sample `i` is a constant level over `[i, i + 1)`; output sample `n` covers
/// `[n·r, (n + 1)·r)` input samples with `r = from_rate / to_rate`, and is the mean of the input
/// over that interval, a sample cut by its ends counting by the share inside. The output has
/// `floor(len / r)` samples. The same rate is the signal itself.
///
/// In the samples: few of them to a half-wave (a sync 1 is 1.5 at 8 kHz) and edges smeared
/// over one. In the pulses: lengths on a coarse grid; a short lobe may sit in one sample.
pub fn downsample(x: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if x.is_empty() || to_rate == 0 || from_rate == 0 {
        return Vec::new();
    }
    let r = f64::from(from_rate) / f64::from(to_rate);
    let len = x.len();
    let out_len = (len as f64 / r).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        let start = n as f64 * r;
        let end = (start + r).min(len as f64);
        let i0 = start.floor() as usize;
        let i1 = (end.ceil() as usize).min(len);
        let mut sum = 0.0f64;
        for (i, &v) in (i0..i1).zip(&x[i0..i1]) {
            let lo = start.max(i as f64);
            let hi = end.min((i + 1) as f64);
            sum += f64::from(v) * (hi - lo).max(0.0);
        }
        out.push((sum / r) as f32);
    }
    out
}

/// A sine by multiplication and addition alone, the same to the last bit everywhere: reduced to
/// a quarter turn either side of zero, then its Taylor series to the 17th power, under 1e-12 off.
fn sine(x: f64) -> f64 {
    use std::f64::consts::{FRAC_PI_2, PI};
    let mut r = x - (x / (2.0 * PI)).round() * 2.0 * PI;
    if r > FRAC_PI_2 {
        r = PI - r;
    } else if r < -FRAC_PI_2 {
        r = -PI - r;
    }
    let (mut term, mut sum) = (r, r);
    for k in 1..=8 {
        term *= -r * r / f64::from(2 * k * (2 * k + 1));
        sum += term;
    }
    sum
}

/// `sin(πx) / (πx)`, 1 at 0.
fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        sine(std::f64::consts::PI * x) / (std::f64::consts::PI * x)
    }
}

/// `x` at `from_rate` resampled to `to_rate` as a careful resampler does it: a low-pass sharp at
/// 0.45 of the new rate, a sinc eight zero crossings either side under a Lanczos window,
/// normalised to pass a level as it is, read at each new sample's time. The output has
/// `floor(len · to_rate / from_rate)` samples.
///
/// In the samples: every harmonic over the cut gone, a square wave's edges rounded and ringing
/// on either side. In the pulses: the sync's 2.6 kHz near the cut at 6 kHz.
pub fn resample_sharp(x: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if x.is_empty() || to_rate == 0 || from_rate == 0 {
        return Vec::new();
    }
    let r = f64::from(from_rate) / f64::from(to_rate);
    let cut = 0.45 * f64::from(to_rate.min(from_rate)) / f64::from(from_rate);
    const LOBES: f64 = 8.0;
    let width = LOBES / (2.0 * cut);
    let out_len = (x.len() as f64 / r).floor() as usize;
    (0..out_len)
        .map(|n| {
            let p = n as f64 * r;
            let from = (p - width).ceil().max(0.0) as usize;
            let to = ((p + width).floor() as usize).min(x.len() - 1);
            let (mut sum, mut weight) = (0.0f64, 0.0f64);
            for (k, &v) in x.iter().enumerate().take(to + 1).skip(from) {
                let d = p - k as f64;
                let h = sinc(2.0 * cut * d) * sinc(d / width);
                sum += f64::from(v) * h;
                weight += h;
            }
            (sum / weight) as f32
        })
        .collect()
}

/// A recording at `bits` bits (8 or 16): each sample, clamped to ±1.0, moved to the nearest level
/// a WAV file of that depth holds, as `encode_wav` rounds it: `floor((s + 1) · 127.5 + 0.5)`
/// levels of 1/127.5 above -1 for 8 bits, `floor(s · 32767 + 0.5)` levels of 1/32767 for 16.
///
/// In the samples: a step of 0.0078 at 8 bits, noise of a uniform 0.0023 deviation on a busy
/// signal.
pub fn quantise(x: &[f32], bits: u16) -> Vec<f32> {
    x.iter()
        .map(|&v| {
            let s = f64::from(v).clamp(-1.0, 1.0);
            if bits == 16 {
                let level = (s * 32767.0 + 0.5).floor() as i32 as i16;
                (f64::from(level) / 32767.0) as f32
            } else {
                let level = ((s + 1.0) * 127.5 + 0.5).floor() as i32 as u8;
                (f64::from(level) / 127.5 - 1.0) as f32
            }
        })
        .collect()
}

/// Rippled peaks: the signal times `1 + depth · tri(hz · t)` (`depth` under 1), a ripple that
/// moves the tops of the half-waves and never their sign, as a double-humped peak does.
///
/// In the pulses: nothing for a comparator whose band the ripple never crosses.
pub fn ripple(x: &[f32], rate: u32, depth: f32, hz: f64) -> Vec<f32> {
    x.iter()
        .enumerate()
        .map(|(n, &s)| {
            let t = n as f64 / f64::from(rate);
            let factor = 1.0 + f64::from(depth) * tri(hz * t);
            (f64::from(s) * factor) as f32
        })
        .collect()
}
