// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The edge detector: where a recording's signal changes sides, and so the pulses it holds.
//!
//! The samples lose their DC offset and go through the user's filter, then a comparator with a
//! band decides each crossing and the zero line times it, between samples. The band follows the
//! signal: it is a share of the loudest sample within 10 ms (`ENV_HALF_WINDOW_MS`), never
//! below a floor that keeps it off the hiss in a pause. A weak lobe between two strong ones of
//! the other sign counts; one with a strong neighbour on one side only is noise.
//!
//! The samples are walked twice and never held as floats: once for the statistics the floor is
//! cut from, taken over the whole recording, and once for the edges, with the last second of
//! samples kept to time them.

use super::filter::{Filter, FilterError};
use crate::pulses::Pulse;
use std::collections::VecDeque;
use tapeti_core::audio::TSTATES_PER_SEC;

/// Where the signal crossed the zero line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// In samples from the start, between two of them.
    pub at: f64,
    pub rising: bool,
}

/// After this long under the band the detector is idle and takes the next crossing either way.
///
/// Silence brings the signal to zero without crossing the band, so waiting only for the other
/// side would miss the next pilot's first edge half the time. Two walls bound the choice: a
/// pilot pair at the ROM's load timeout, 6,710 T (1.92 ms), is the longest gap inside a block,
/// and a shorter wait would take the return from a dropout as a second edge; the shortest pause
/// between two blocks is about 975 ms. 20 ms is chosen between.
const IDLE_AFTER_MS: f64 = 20.0;

/// The shortest a lobe under the band lasts to be a half-wave, which keeps a run of weak lobes
/// alive.
///
/// From below, one sample out of line spans under two sample periods ([`min_half_samples`]).
/// From above, the shortest half-wave the ROM's saver writes is the first sync pulse, 667 T.
/// 426 T is chosen between, a fast loader's 0 bit; a loader with shorter half-waves gets no help
/// from this, a limit and never a wrong answer.
const MIN_HALF_TSTATES: f64 = 426.0;

/// [`MIN_HALF_TSTATES`] in samples, never under two: one sample out of line crosses any level
/// inside the periods either side of it. The two binds below 16.4 kHz.
fn min_half_samples(rate_hz: u32) -> f64 {
    (MIN_HALF_TSTATES * f64::from(rate_hz) / f64::from(TSTATES_PER_SEC)).max(2.0)
}

/// How far behind the band an edge's zero crossing may lie: half the widest half-wave the ROM
/// loads, the second sync pulse at 3,423 T with the rest of its block nominal. A half-sine
/// climbs from zero to any level under its peak within half its half-wave, so a longer stretch
/// under the floor is silence and not a climb.
const CROSSING_REACH_T: f64 = 3423.0 / 2.0;

/// The band follows the loudest sample within this many ms either side. Around a crossing the
/// window must reach the lobe's peak, or the band falls to the noise (a pilot with noise a fifth
/// of its level chatters at 0.1 ms, not at 0.2 ms); and a drop in level hides a lobe per
/// half-wave of the window under the old peak (a drop to a fifth costs 96 lobes at 60 ms, none
/// at 10 ms). 10 ms is chosen between.
const ENV_HALF_WINDOW_MS: f64 = 10.0;

/// The share of that envelope a lobe must reach to arm the comparator. Over a lobe's noise (a
/// pilot with noise of 0.15 on lobes of 0.5 chatters at 0.10, not at 0.25), under the weakest
/// lobe it must read (from 0.6 a weak first lobe out of silence is lost); chosen between.
const BAND_FRAC: f32 = 0.25;

/// The share at which a weak lobe of the other sign is noticed; it counts only once the lobe
/// after it reaches [`BAND_FRAC`].
///
/// From above, it lies under the weakest half-wave the band must read: through the default
/// band-pass, a zero's first half behind a one stands at 0.195 of the envelope at the reach
/// where the first sync pulse still crosses. From below, a crest that turns between the two
/// levels closes the bracket on nothing: from about 0.23 on a pilot at 44.1 kHz. A tenth is
/// chosen between.
const REV_FRAC: f32 = 0.10;

/// The band's floor as a share of the recording's signal scale ([`PEAK_PCTILE`]), so the band
/// does not sink onto the noise in a pause. Over what the DC blocker leaves of a player's 1 ms
/// pause level once the envelope lets go of it, 0.021; under the weakest half-wave the ROM
/// writes, through the default band-pass, the second sync pulse at 0.42 (lost from 0.45). 0.05
/// is chosen between, near the lower wall.
const FLOOR_FRAC: f32 = 0.05;

/// The signal scale is this percentile of the envelope, not its maximum, which one click
/// sets. A click raises one envelope window, 20 ms, so the scale stands under clicks while they
/// fill under half a percent of the recording (one every 4 s) and on the loudest material
/// wherever that fills more (18 s of an hour).
const PEAK_PCTILE: f64 = 0.995;

/// The noise mark is this percentile of the envelope where dead air (tape with nothing on it)
/// fills at least that share of the recording; a tape with less goes to the count of its
/// quietest seconds ([`QUIET_SECONDS`]). Over a deck's own quieter noise past the tape of up
/// to a tenth of the recording (a fiftieth measures the deck), under the material beside the
/// dead air (a fifth lands in it).
const QUIET_PCTILE: f64 = 0.10;

/// A stretch is dead air when nine tenths of its envelope is: that leaves out the 10 ms of
/// material the envelope carries into each end and a few clicks (0.98 counts every such second
/// as material), and still asks for dead air through (0.5 counts a second half of which is a
/// gap).
const DEAD_AIR_PCTILE: f64 = 0.90;

/// The noise is measured over the median of this many quietest seconds, the fifth: the level
/// of dead air wherever the tape holds five seconds of it and four or fewer are quieter (the
/// deck before and past the tape). Thirteen lands in a quieter recording beside six seconds of
/// dead air; eight is the deck's level behind a tape.
const QUIET_SECONDS: usize = 10;

/// The unit the noise is counted in, and the floor's step. Its tenth must hold an envelope
/// window (20 ms), so 200 ms at least; and the gaps of a ROM tape, the ROM's own pause between
/// a header and its data, are just under a second, which a tenth of up to 533 ms fits however
/// it falls, so 5.33 s at most. One second is chosen between.
const SCALE_BLOCK_MS: f64 = 1000.0;

/// Where no whole second is dead air, the count runs again on seconds cut in this many: from
/// two (a block fits the ROM's pause wherever it falls) to fifty (a block still holds an
/// envelope window). A tenth is chosen between.
const DEAD_AIR_FINE: usize = 10;

/// How far either side, in seconds, the floor looks for the loudest material. A block leaves
/// the DC blocker's tail for about 1.4 s behind it, which must keep its recording's floor (none
/// cuts edges in it); a reach longer than the gap between two recordings lends the louder one's
/// floor to the quieter (eight does). Five is chosen between.
const SCALE_WINDOW_S: usize = 5;

/// A second is material when its scale clears this many times the noise mark. A second of
/// Gaussian hiss stands under 1.94 times its mark (1.5 cuts edges in it); the quietest material
/// the band reads stands 2.01 times over it (2.5 holds a quiet recording's floor up from a
/// louder one). Two is chosen between.
const MATERIAL_MULT: f32 = 2.0;

/// The floor follows a quiet recording down to the noise mark times this and no further: lower
/// cuts the hiss into pulses; higher leaves the lobes of quiet material uncut (from 1.1).
const NOISE_MULT: f32 = 1.0;

/// The DC blocker's corner. Two orders under the slowest tone a loader writes (a pilot at
/// 807 Hz), so a 3,423 T half-wave droops by under 7% across it; a slow drift of f Hz passes at
/// f / sqrt(f² + 10²).
const DC_CORNER_HZ: f64 = 10.0;

/// The edges of a recording at `rate_hz`. `samples` gives the samples from the start each time
/// it is called; it is called twice.
pub fn detect<I: Iterator<Item = f32>>(
    samples: impl Fn() -> I,
    rate_hz: u32,
    filter: Filter,
) -> Result<Vec<Edge>, FilterError> {
    let half_win = ((ENV_HALF_WINDOW_MS / 1000.0 * f64::from(rate_hz)) as usize).max(1);
    // The envelope's statistics read it every ~2 ms.
    let stride = (rate_hz as usize / 500).max(1);
    let per_sec = ((SCALE_BLOCK_MS / 1000.0 * f64::from(rate_hz)) as usize / stride).max(1);

    let mut moved = Moved::default();
    let mut env = Vec::new();
    let conditioned = condition(samples().inspect(|&v| moved.see(v, stride)), rate_hz, filter)?;
    walk(conditioned, half_win, 0, |i, e, _| {
        if i.is_multiple_of(stride) {
            env.push(e);
        }
    });
    let moved = moved.strides;
    let mut sorted = env.clone();
    sorted.sort_unstable_by(f32::total_cmp);
    let peak = at_pctile(&sorted, PEAK_PCTILE);
    if peak <= 0.0 {
        return Ok(Vec::new());
    }
    let mark = quiet_mark(&sorted, &env, &moved, per_sec, peak);
    let floors = FloorProfile::local(&env, per_sec, stride, peak, mark);
    drop((env, sorted));

    let mut d = Detector {
        edges: Vec::new(),
        state: State::Idle,
        flat: 0,
        pending: Vec::with_capacity(2),
        idle_after: (IDLE_AFTER_MS / 1000.0 * f64::from(rate_hz)) as usize,
        min_half: min_half_samples(rate_hz),
        reach: (CROSSING_REACH_T * f64::from(rate_hz) / f64::from(TSTATES_PER_SEC)).ceil() as usize,
    };
    // A crossing is timed from the samples behind it, never more than a second back: any
    // half-wave is far shorter.
    let keep = rate_hz as usize + half_win + 2;
    walk(condition(samples(), rate_hz, filter)?, half_win, keep, |i, e, x| {
        if i > 0 {
            d.step(x, i, e, floors.at(i));
        }
    });
    Ok(d.edges)
}

/// The pulses between `edges`, with one from the start of the recording to the first edge and
/// one from the last to the end at `samples`, so the silence either side is kept.
pub fn pulses(edges: &[Edge], rate_hz: u32, samples: usize) -> Vec<Pulse> {
    let to_t = f64::from(TSTATES_PER_SEC) / f64::from(rate_hz);
    let mut at = Vec::with_capacity(edges.len() + 2);
    at.push(0.0);
    at.extend(edges.iter().map(|e| e.at));
    at.push(samples as f64);
    at.windows(2)
        .filter(|w| w[1] > w[0])
        .map(|w| Pulse { tstates: (w[1] - w[0]) * to_t, start: w[0] })
        .collect()
}

/// The DC blocker, a one-pole high-pass `y = x - x₋₁ + r·y₋₁`, then the user's filter.
fn condition(
    samples: impl Iterator<Item = f32>,
    rate_hz: u32,
    filter: Filter,
) -> Result<impl Iterator<Item = f32>, FilterError> {
    let r = (1.0 - 2.0 * std::f64::consts::PI * DC_CORNER_HZ / f64::from(rate_hz)) as f32;
    let (mut x1, mut y1) = (0.0f32, 0.0f32);
    let blocked = samples.map(move |x| {
        let y = x - x1 + r * y1;
        (x1, y1) = (x, y);
        y
    });
    Ok(filter.design(rate_hz)?.apply(blocked))
}

/// The last samples read, by their index from the start.
struct Ring {
    v: VecDeque<f32>,
    /// The index of `v[0]`.
    first: usize,
    cap: usize,
}

impl Ring {
    fn push(&mut self, x: f32) {
        if self.cap == 0 {
            return;
        }
        if self.v.len() == self.cap {
            self.v.pop_front();
            self.first += 1;
        }
        self.v.push_back(x);
    }

    fn at(&self, i: usize) -> f32 {
        self.v[i - self.first]
    }
}

/// Calls `f` for every index in order with the envelope there, the loudest `|x|` within
/// `half_win` either side, and the last `keep` samples read, which reach `half_win` past it.
fn walk(
    mut xs: impl Iterator<Item = f32>,
    half_win: usize,
    keep: usize,
    mut f: impl FnMut(usize, f32, &Ring),
) {
    let mut ring = Ring { v: VecDeque::with_capacity(keep), first: 0, cap: keep };
    // Indices with their `|x|`, falling from front to back: the front is the window's loudest.
    let mut loud: VecDeque<(usize, f32)> = VecDeque::new();
    let (mut read, mut done) = (0usize, false);
    for i in 0.. {
        while !done && read <= i + half_win {
            match xs.next() {
                Some(x) => {
                    ring.push(x);
                    while loud.back().is_some_and(|&(_, a)| a <= x.abs()) {
                        loud.pop_back();
                    }
                    loud.push_back((read, x.abs()));
                    read += 1;
                }
                None => done = true,
            }
        }
        if i >= read {
            break;
        }
        while loud.front().is_some_and(|&(j, _)| j + half_win < i) {
            loud.pop_front();
        }
        f(i, loud.front().map_or(0.0, |&(_, a)| a), &ring);
    }
}

/// For each stride of the raw samples, whether they change inside it and whether its first
/// differs from the one before. A recording that runs on past its tape holds constant samples,
/// which say nothing about the tape's noise.
#[derive(Default)]
struct Moved {
    strides: Vec<(bool, bool)>,
    last: Option<f32>,
    n: usize,
}

impl Moved {
    fn see(&mut self, v: f32, stride: usize) {
        let changed = self.last.is_some_and(|l| l != v);
        if self.n.is_multiple_of(stride) {
            self.strides.push((false, changed));
        } else if changed {
            self.strides.last_mut().expect("a stride was pushed").0 = true;
        }
        self.last = Some(v);
        self.n += 1;
    }
}

/// Whether a block of strides holds tape: its samples differ anywhere among themselves.
fn holds_tape(moved: &[(bool, bool)]) -> bool {
    match moved.split_first() {
        Some((first, rest)) => first.0 || rest.iter().any(|&(inside, entry)| inside || entry),
        None => false,
    }
}

fn at_pctile(sorted: &[f32], p: f64) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p) as usize]
}

fn pctile(v: &mut [f32], p: f64) -> f32 {
    v.sort_unstable_by(f32::total_cmp);
    at_pctile(v, p)
}

/// Whether a dead-air estimate can be a noise level: material stands [`MATERIAL_MULT`] over the
/// noise, so an estimate within that of the scale measured the quietest program instead.
fn is_noise_level(est: f32, peak: f32) -> bool {
    MATERIAL_MULT * est < peak
}

/// The noise mark the floor stops at: [`QUIET_PCTILE`] of the envelope, or lower where the
/// tape's own dead air, counted inside the stretch that holds tape, is.
fn quiet_mark(sorted: &[f32], env: &[f32], moved: &[(bool, bool)], per_sec: usize, peak: f32) -> f32 {
    let pct = at_pctile(sorted, QUIET_PCTILE);
    let dead = tape_span(env, moved, per_sec).and_then(|span| {
        let (env, moved) = (&env[span.clone()], &moved[span]);
        let (est, _) = dead_air_at(env, moved, per_sec)?;
        if is_noise_level(est, peak) {
            return Some(est);
        }
        let (fine, _) = dead_air_at(env, moved, per_sec / DEAD_AIR_FINE)?;
        is_noise_level(fine, peak).then_some(fine)
    });
    dead.map_or(pct, |d| d.min(pct))
}

/// The envelope entries from the first to the last second louder than the quietest dead air
/// by [`MATERIAL_MULT`]: the tape, without the deck's own quieter noise before and past it.
fn tape_span(env: &[f32], moved: &[(bool, bool)], per_sec: usize) -> Option<std::ops::Range<usize>> {
    let (quietest, _) = dead_air_at(env, moved, per_sec)?;
    let per_sec = per_sec.max(1);
    let mut buf = Vec::with_capacity(per_sec);
    let louder: Vec<bool> = env
        .chunks_exact(per_sec)
        .map(|e| {
            buf.clear();
            buf.extend_from_slice(e);
            pctile(&mut buf, PEAK_PCTILE) > MATERIAL_MULT * quietest
        })
        .collect();
    let first = louder.iter().position(|&m| m)?;
    let last = louder.iter().rposition(|&m| m)?;
    Some(first * per_sec..(last + 1) * per_sec)
}

/// The dead air's level in blocks of `per_block` envelope entries: the median of the quietest
/// [`QUIET_SECONDS`] blocks (or a tenth of them) that hold tape, each read at
/// [`DEAD_AIR_PCTILE`]; and whether dead air fills less than [`QUIET_PCTILE`] of them.
fn dead_air_at(env: &[f32], moved: &[(bool, bool)], per_block: usize) -> Option<(f32, bool)> {
    let per_block = per_block.max(1);
    let mut buf = Vec::with_capacity(per_block);
    let mut level = Vec::with_capacity(env.len() / per_block);
    for (e, m) in env.chunks_exact(per_block).zip(moved.chunks_exact(per_block)) {
        if holds_tape(m) {
            buf.clear();
            buf.extend_from_slice(e);
            level.push(pctile(&mut buf, DEAD_AIR_PCTILE));
        }
    }
    if level.is_empty() {
        return None;
    }
    level.sort_unstable_by(f32::total_cmp);
    let n = QUIET_SECONDS.min((level.len() as f64 * QUIET_PCTILE) as usize).max(1);
    let est = at_pctile(&level[..n], 0.5);
    if est <= 0.0 {
        return None;
    }
    let dead = level.partition_point(|&l| l <= MATERIAL_MULT * est);
    Some((est, (dead as f64) < QUIET_PCTILE * level.len() as f64))
}

/// The band's floor, one value a second.
///
/// A second's scale is [`PEAK_PCTILE`] of its own envelope. Where the loudest material second
/// within [`SCALE_WINDOW_S`] is under half the recording's scale, the floor follows it down, so
/// a quiet recording between loud ones is read; a second with no material in reach keeps the
/// nearest material's. It never falls under the noise mark and never rises over the
/// recording's own floor. Over half the scale it stands, because there the arm and reversal
/// levels decide wherever there is material ([`FLOOR_FRAC`] / [`REV_FRAC`]) and a lower floor
/// could only admit hiss in the pauses.
struct FloorProfile {
    /// Samples a value.
    blk: usize,
    v: Vec<f32>,
}

impl FloorProfile {
    fn local(env: &[f32], per_block: usize, stride: usize, peak: f32, mark: f32) -> Self {
        let nb = env.len().div_ceil(per_block).max(1);
        let mut buf = Vec::with_capacity(per_block);
        let block: Vec<f32> = (0..nb)
            .map(|k| {
                let hi = ((k + 1) * per_block).min(env.len());
                buf.clear();
                buf.extend_from_slice(&env[(k * per_block).min(hi)..hi]);
                if buf.is_empty() {
                    0.0
                } else {
                    pctile(&mut buf, PEAK_PCTILE)
                }
            })
            .collect();
        let gate = MATERIAL_MULT * mark;
        let mut scale: Vec<f32> = (0..nb)
            .map(|k| {
                let (lo, hi) = (k.saturating_sub(SCALE_WINDOW_S), (k + SCALE_WINDOW_S).min(nb - 1));
                block[lo..=hi].iter().filter(|&&b| b > gate).fold(0.0f32, |m, &b| m.max(b))
            })
            .collect();
        // Hold from the nearest material second, the earlier on a tie; none at all keeps the
        // recording's floor.
        let material: Vec<usize> = (0..nb).filter(|&k| scale[k] > 0.0).collect();
        for k in 0..nb {
            if scale[k] <= 0.0 {
                let after = material.partition_point(|&m| m < k);
                let near = match (after.checked_sub(1).map(|a| material[a]), material.get(after)) {
                    (Some(a), Some(&b)) => Some(if k - a <= b - k { a } else { b }),
                    (a, b) => a.or(b.copied()),
                };
                scale[k] = near.map_or(peak, |m| scale[m]);
            }
        }
        let lowest = (NOISE_MULT * mark / FLOOR_FRAC).min(peak);
        let bind = FLOOR_FRAC / REV_FRAC * peak;
        let v = scale
            .into_iter()
            .map(|s| FLOOR_FRAC * if s < bind { s.clamp(lowest, peak) } else { peak })
            .collect();
        FloorProfile { blk: per_block * stride, v }
    }

    fn at(&self, i: usize) -> f32 {
        self.v[(i / self.blk).min(self.v.len() - 1)]
    }
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    High,
    Low,
}

struct Detector {
    edges: Vec<Edge>,
    state: State,
    /// Samples in a row inside the arm band.
    flat: usize,
    /// Edges of weak lobes awaiting a strong one: an odd count is inside a weak lobe of the
    /// other sign, an even one back on the confirmed side.
    pending: Vec<Edge>,
    idle_after: usize,
    min_half: f64,
    reach: usize,
}

impl Detector {
    fn step(&mut self, x: &Ring, i: usize, env: f32, floor: f32) {
        let cur = x.at(i);
        let arm = (BAND_FRAC * env).max(floor);
        let rev = (REV_FRAC * env).max(floor);
        let reach = self.reach;
        let edge = |rising, thr| zero_edge(x, floor, i, rising, thr, reach);
        let side = |away: bool| if away { State::High } else { State::Low };
        match self.state {
            State::Idle => {
                // Only a full excursion starts a train out of silence.
                if cur.abs() > arm {
                    let rising = cur > arm;
                    self.edges.push(edge(rising, arm));
                    self.state = side(rising);
                    self.flat = 0;
                    return;
                }
            }
            State::High | State::Low => {
                // The sample as seen from the confirmed side: below zero is away from it.
                let y = if self.state == State::High { cur } else { -cur };
                let away = self.state == State::Low;
                match self.pending.len() {
                    0 => {
                        if y < -rev {
                            let e = edge(away, rev);
                            if y <= -arm {
                                self.edges.push(e);
                                self.state = side(away);
                                self.flat = 0;
                                return;
                            }
                            self.pending.push(e);
                        }
                    }
                    n if n % 2 == 1 => {
                        if y <= -arm {
                            // The weak lobe deepened to the band after all.
                            self.edges.append(&mut self.pending);
                            self.state = side(away);
                            self.flat = 0;
                            return;
                        } else if y > rev {
                            let e = edge(!away, rev);
                            if y >= arm {
                                self.edges.append(&mut self.pending);
                                self.edges.push(e);
                                self.flat = 0;
                                return;
                            }
                            self.pending.push(e);
                        }
                    }
                    _ => {
                        if y >= arm {
                            // A strong lobe on the confirmed side confirms the weak run.
                            self.edges.append(&mut self.pending);
                            self.flat = 0;
                            return;
                        } else if y < -rev {
                            // A further weak lobe extends the run, unless the last one was too
                            // short to be a half-wave: then the run was a wiggle.
                            let e = edge(away, rev);
                            if e.at - self.pending[self.pending.len() - 1].at < self.min_half {
                                self.pending.clear();
                            }
                            if y <= -arm {
                                self.edges.append(&mut self.pending);
                                self.edges.push(e);
                                self.state = side(away);
                                self.flat = 0;
                                return;
                            }
                            self.pending.push(e);
                        }
                    }
                }
            }
        }
        // A long stretch inside the band is a pause: idle, and the weak run goes with it.
        if cur.abs() < arm {
            self.flat += 1;
            if self.flat >= self.idle_after {
                self.state = State::Idle;
                self.pending.clear();
            }
        } else {
            self.flat = 0;
        }
    }
}

/// Times a crossing the band decided at sample `i`: back to where the excursion left the zero
/// line, between the two samples either side of it.
///
/// Timed where it crossed the band, a small lobe would cross late and leave early, and an edge
/// would move with the loudness of the tape elsewhere. The scan goes back through the floor to
/// the sign change if that lies within `reach` of where the signal entered the band; a climb off
/// a stretch on the same side with no sign change of its own is timed where its line meets the
/// axis ([`climb_meets_axis`]), or else at its foot. With no sign change behind it in the samples
/// kept, the crossing of `thr` is the time.
fn zero_edge(x: &Ring, floor: f32, i: usize, rising: bool, thr: f32, reach: usize) -> Edge {
    let side = |v: f32, level: f32| if rising { v > level } else { v < -level };
    let lo = x.first;
    let mut j = i;
    while j > lo && side(x.at(j - 1), floor) {
        j -= 1;
    }
    if j > lo && side(x.at(j - 1), 0.0) {
        let mut k = j - 1;
        while k > lo && j - 1 - k < reach && side(x.at(k - 1), 0.0) {
            k -= 1;
        }
        if k > lo && !side(x.at(k - 1), 0.0) {
            j = k;
        } else if let Some(at) = climb_meets_axis(x, floor, j, i, rising, thr, reach) {
            return Edge { at, rising };
        }
    }
    let at = if j > lo {
        let (a, b) = (f64::from(x.at(j - 1)), f64::from(x.at(j)));
        (j - 1) as f64 + (-a / (b - a)).clamp(0.0, 1.0)
    } else {
        let (prev, cur) = (x.at(i - 1), x.at(i));
        let level = if rising { thr } else { -thr };
        let frac = if (cur - prev).abs() > f32::EPSILON {
            ((level - prev) / (cur - prev)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (i - 1) as f64 + f64::from(frac)
    };
    Edge { at, rising }
}

/// Where a climb off a same-side stretch under the floor would have met the axis: the line
/// through its crossings of the floor (between `j - 1` and `j`) and of `thr` (between `i - 1`
/// and `i`), extended back. A half-sine is straight about its crossing (1% off its tangent at a
/// quarter of its height). `None` where `thr` is the floor itself, with no second point to draw
/// the line through, or where the line meets the axis further back than `reach`.
fn climb_meets_axis(
    x: &Ring,
    floor: f32,
    j: usize,
    i: usize,
    rising: bool,
    thr: f32,
    reach: usize,
) -> Option<f64> {
    if thr <= floor {
        return None;
    }
    let v = |k: usize| f64::from(if rising { x.at(k) } else { -x.at(k) });
    let cross = |k: usize, level: f64| {
        let rise = v(k) - v(k - 1);
        (k - 1) as f64 + if rise > 0.0 { ((level - v(k - 1)) / rise).clamp(0.0, 1.0) } else { 1.0 }
    };
    let (lo, hi) = (f64::from(floor), f64::from(thr));
    let (t_lo, t_hi) = (cross(j, lo), cross(i, hi));
    if t_hi <= t_lo {
        return None;
    }
    let t = t_lo - lo * (t_hi - t_lo) / (hi - lo);
    (t >= t_hi - reach as f64).then_some(t)
}
