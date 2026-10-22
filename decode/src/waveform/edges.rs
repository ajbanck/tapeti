// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The edge detector: where a recording's signal changes sides, and so the pulses it holds.
//!
//! The samples lose their DC offset and go through the user's filter, then a comparator with a
//! band decides each crossing, timed at the zero line between samples. The band follows the
//! loudest sample within 10 ms (`ENV_HALF_WINDOW_MS`), never below a floor that keeps it off the
//! hiss in a pause; a weak lobe between two strong ones of the other sign counts, one with a
//! strong neighbour on one side only is noise. A block a dropout ended is cut again on a band
//! that follows the level down ([`Band::Fade`]). The samples are walked three times, never held
//! as floats: once for statistics over the whole recording, once for the edges and the
//! half-waves the band swallowed, and once to mark pulses that never rise over the noise line.

use super::filter::{Filter, FilterError};
use crate::pulses::Pulse;
use std::collections::VecDeque;
use std::ops::ControlFlow;
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

/// The block whose peak the fade band's median takes ([`Band::Fade`]).
///
/// From below, the median is the lobes' level only while more than half the blocks hold a
/// lobe's peak, about block / half-wave of them: over half the longest half-wave the ROM's
/// saver writes, the pilot's 2,168 T, 0.31 ms; under it the median sinks toward the lobes'
/// flanks and the band with it, until a pilot with noise a fifth of its level chatters (from
/// 0.15 ms). From above, the median follows a dropout only while more than half its window's
/// blocks lie inside it: a 12.4 ms dropout is not followed at 2 ms. 0.5 ms is chosen between.
const FADE_BLOCK_MS: f64 = 0.5;

/// The fade band's floor as a share of the ordinary one ([`FLOOR_FRAC`]).
///
/// The floor must come down with the envelope, or the band does not follow a dropout at all.
/// From above, it must stay under a dropout's lobes, a share under 1.2 (0.06 of the level over
/// 0.05); at 1.5 such a block is not read again. From below, the median lets go of a block at
/// once, so the floor meets the DC blocker's residue behind its last half-wave, 0.0124 of the
/// scale behind a ROM pilot: a share under 0.25 cuts that residue into an edge, bits out of
/// silence that can answer a header asking a byte too many with a 0x00 no checksum catches.
/// Half is chosen between, and it is not tied
/// to the recording's noise mark, since a dropout is local.
const FADE_FLOOR_SPAN: f32 = 0.5;

/// What the band the comparator arms at follows.
///
/// The band is a share of the loudest sample within `ENV_HALF_WINDOW_MS` either side; a
/// dropout narrower than that window cannot pull the maximum down, so no edge is cut there and
/// the block ends at the dropout instead. The fade band's median of `FADE_BLOCK_MS` block peaks
/// over the same window follows the dropout down, agreeing with the ordinary band on steady
/// signal. It is never a whole decode: a block a header asks for that the ordinary band cannot
/// read whole is read again on it, kept only at the length asked for with a good checksum
/// ([`crate::entry`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Band {
    /// The loudest sample in the window.
    #[default]
    Max,
    /// The median of the window's block peaks, with a floor `FADE_FLOOR_SPAN` of the ordinary.
    Fade,
}

/// The line the comparator measures the signal against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Axis {
    /// Zero, after the DC blocker.
    #[default]
    Zero,
    /// The signal's own local midline (`Midline`), which follows a centre that wanders.
    Midline,
}

/// The edges of a recording at `rate_hz` on the zero line. `samples` gives the samples from the
/// start each time it is called; it is called twice.
pub fn detect<I: Iterator<Item = f32>>(
    samples: impl Fn() -> I,
    rate_hz: u32,
    filter: Filter,
) -> Result<Vec<Edge>, FilterError> {
    detect_on(samples, rate_hz, filter, Axis::Zero)
}

/// The edges of a recording at `rate_hz` measured against `axis`.
pub fn detect_on<I: Iterator<Item = f32>>(
    samples: impl Fn() -> I,
    rate_hz: u32,
    filter: Filter,
    axis: Axis,
) -> Result<Vec<Edge>, FilterError> {
    let stats = measure(&samples, rate_hz, filter)?;
    edges_on(&samples, rate_hz, filter, &stats, axis, Band::Max, None)
}

/// The pulses of a recording at `rate_hz` on the zero line, each marked quiet where it never
/// rises over the noise.
///
/// They are cut from the edges as [`pulses`] cuts them, and the noise is the recording's noise
/// line (`noise_line`). `samples` is called three times.
pub fn detect_pulses<I: Iterator<Item = f32>>(
    samples: impl Fn() -> I,
    rate_hz: u32,
    filter: Filter,
) -> Result<Vec<Pulse>, FilterError> {
    let mut out = detect_pulses_each(samples, rate_hz, filter, &[Axis::Zero], |_, p| ControlFlow::Break(p))?;
    Ok(out.pop().expect("one axis asked for"))
}

/// What `each` makes of the pulses of a recording at `rate_hz` on each of `axes` in turn, until
/// it says to stop.
///
/// The pulses are cut as [`detect_pulses`] cuts them. The recording's statistics are measured
/// once, and one axis's pulses are let go before the next axis is walked. `samples` is called
/// once, then twice more for the zero line and once more for the midline.
pub fn detect_pulses_each<I: Iterator<Item = f32>, T>(
    samples: impl Fn() -> I,
    rate_hz: u32,
    filter: Filter,
    axes: &[Axis],
    mut each: impl FnMut(Axis, Vec<Pulse>) -> ControlFlow<T, T>,
) -> Result<Vec<T>, FilterError> {
    let stats = measure(&samples, rate_hz, filter)?;
    let mut out = Vec::with_capacity(axes.len());
    for &axis in axes {
        let mut about = (axis == Axis::Midline).then(Vec::new);
        let edges = edges_on(&samples, rate_hz, filter, &stats, axis, Band::Max, about.as_mut())?;
        let mut pulses = pulses(&edges, rate_hz, stats.samples);
        drop(edges);
        mark_quiet_on(&mut pulses, &samples, rate_hz, filter, &stats, about)?;
        match each(axis, pulses) {
            ControlFlow::Continue(t) => out.push(t),
            ControlFlow::Break(t) => {
                out.push(t);
                break;
            }
        }
    }
    Ok(out)
}

/// The pulses of a recording at `rate_hz` on `axis`, cut at `band`, each marked quiet as
/// [`detect_pulses`] marks them. `samples` is called three times, twice for the midline.
pub fn detect_pulses_band<I: Iterator<Item = f32>>(
    samples: impl Fn() -> I,
    rate_hz: u32,
    filter: Filter,
    axis: Axis,
    band: Band,
) -> Result<Vec<Pulse>, FilterError> {
    let stats = measure(&samples, rate_hz, filter)?;
    let mut about = (axis == Axis::Midline).then(Vec::new);
    let edges = edges_on(&samples, rate_hz, filter, &stats, axis, band, about.as_mut())?;
    let mut pulses = pulses(&edges, rate_hz, stats.samples);
    mark_quiet_on(&mut pulses, &samples, rate_hz, filter, &stats, about)?;
    Ok(pulses)
}

/// Marks quiet each of `pulses` that never rises over the noise line of the signal its axis
/// cuts ([`mark_quiet`]): the samples about the midline the edge pass walked (`about`, kept by
/// [`edges_on`]), or the zero line's, conditioned again, where there are none.
///
/// On the zero line that is the recording's noise line. The midline follows a wander the zero
/// line keeps, and a noise line measured about zero holds the wander: a dull tape's small
/// lobes, riding on it, fall under it, half of the midline's pulses and more under a wander of
/// 0.4 of the tape's level. So on the midline the line is measured over the envelope of the
/// signal about the midline instead, with the pulses held to it there; the scale and the
/// floors stay the recording's.
fn mark_quiet_on<I: Iterator<Item = f32>>(
    pulses: &mut [Pulse],
    samples: &impl Fn() -> I,
    rate_hz: u32,
    filter: Filter,
    stats: &Stats,
    about: Option<Vec<f32>>,
) -> Result<(), FilterError> {
    let (Some(peak), Some(zero_line)) = (stats.peak, stats.noise) else { return Ok(()) };
    match about {
        None => mark_quiet(pulses, condition(samples(), rate_hz, filter)?, rate_hz, zero_line),
        Some(about) => {
            let mut env = Vec::new();
            walk(about.iter().copied(), stats.half_win, None, 0, |i, e, _| {
                if i.is_multiple_of(stats.stride) {
                    env.push(e);
                }
            });
            if let Some(line) = noise_line(&env, &stats.moved, stats.per_sec, peak) {
                mark_quiet(pulses, about.into_iter(), rate_hz, line);
            }
        }
    }
    Ok(())
}

/// What the detector measures over the whole recording before it looks for edges.
struct Stats {
    /// The envelope's half window.
    half_win: usize,
    /// The signal scale, `None` where the recording is silent and holds no edge.
    peak: Option<f32>,
    floors: FloorProfile,
    /// The floors of the fade band ([`Band::Fade`]).
    fade_floors: FloorProfile,
    /// The recording's noise line, `None` where it has none.
    noise: Option<f32>,
    /// How many samples it holds.
    samples: usize,
    /// Every how many samples the envelope's statistics read it, how many such reads a second
    /// holds, and for each read whether the samples moved inside it and into it.
    stride: usize,
    per_sec: usize,
    moved: Vec<(bool, bool)>,
}

fn measure<I: Iterator<Item = f32>>(
    samples: &impl Fn() -> I,
    rate_hz: u32,
    filter: Filter,
) -> Result<Stats, FilterError> {
    let half_win = ((ENV_HALF_WINDOW_MS / 1000.0 * f64::from(rate_hz)) as usize).max(1);
    // The envelope's statistics read it every ~2 ms.
    let stride = (rate_hz as usize / 500).max(1);
    let per_sec = ((SCALE_BLOCK_MS / 1000.0 * f64::from(rate_hz)) as usize / stride).max(1);

    let mut moved = Moved::default();
    let mut env = Vec::new();
    let conditioned = condition(samples().inspect(|&v| moved.see(v, stride)), rate_hz, filter)?;
    walk(conditioned, half_win, None, 0, |i, e, _| {
        if i.is_multiple_of(stride) {
            env.push(e);
        }
    });
    let count = moved.n;
    let moved = moved.strides;
    let mut sorted = env.clone();
    sorted.sort_unstable_by(f32::total_cmp);
    let peak = at_pctile(&sorted, PEAK_PCTILE);
    if peak <= 0.0 {
        let floors = FloorProfile { blk: 1, v: vec![0.0] };
        let fade_floors = FloorProfile { blk: 1, v: vec![0.0] };
        return Ok(Stats {
            half_win,
            peak: None,
            floors,
            fade_floors,
            noise: None,
            samples: count,
            stride,
            per_sec,
            moved,
        });
    }
    let mark = quiet_mark(&sorted, &env, &moved, per_sec, peak);
    let floors = FloorProfile::local(&env, per_sec, stride, peak, mark, Band::Max);
    let fade_floors = FloorProfile::local(&env, per_sec, stride, peak, mark, Band::Fade);
    let noise = noise_line(&env, &moved, per_sec, peak);
    Ok(Stats {
        half_win,
        peak: Some(peak),
        floors,
        fade_floors,
        noise,
        samples: count,
        stride,
        per_sec,
        moved,
    })
}

/// The edges of the recording on `axis`, at `band`. On the midline `about`, if given, keeps the
/// samples about it as they are walked, so they are not made again to be marked quiet.
fn edges_on<I: Iterator<Item = f32>>(
    samples: &impl Fn() -> I,
    rate_hz: u32,
    filter: Filter,
    stats: &Stats,
    axis: Axis,
    band: Band,
    about: Option<&mut Vec<f32>>,
) -> Result<Vec<Edge>, FilterError> {
    let Some(peak) = stats.peak else { return Ok(Vec::new()) };
    let conditioned = condition(samples(), rate_hz, filter)?;
    let floors = match band {
        Band::Max => &stats.floors,
        Band::Fade => &stats.fade_floors,
    };
    Ok(match axis {
        Axis::Zero => find_edges(conditioned, rate_hz, filter, stats, band),
        Axis::Midline => {
            let midline = Midline::new(conditioned, rate_hz, peak, floors);
            match about {
                Some(kept) => {
                    kept.reserve(stats.samples);
                    find_edges(midline.inspect(|&x| kept.push(x)), rate_hz, filter, stats, band)
                }
                None => find_edges(midline, rate_hz, filter, stats, band),
            }
        }
    })
}

/// The edges in `xs`, the conditioned samples measured against the axis they are given on.
fn find_edges(
    xs: impl Iterator<Item = f32>,
    rate_hz: u32,
    filter: Filter,
    stats: &Stats,
    band: Band,
) -> Vec<Edge> {
    let half_win = stats.half_win;
    let (floors, fade_blk) = match band {
        Band::Max => (&stats.floors, None),
        Band::Fade => {
            (&stats.fade_floors, Some(((FADE_BLOCK_MS / 1000.0 * f64::from(rate_hz)) as usize).max(1)))
        }
    };
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
    let keep = rate_hz as usize + half_win + fade_blk.unwrap_or(0) + 2;
    // A filter removes the low tones a short half-wave rides on, so behind one the shape
    // `Notches` looks for is the filter's own ringing, and splitting it would cut real
    // half-waves in two.
    let mut notches = (filter == Filter::None).then(|| Notches::new(rate_hz));
    let mut envs = Ring { v: VecDeque::with_capacity(keep), first: 0, cap: keep };
    walk(xs, half_win, fade_blk, keep, |i, e, x| {
        envs.push(e);
        if i > 0 {
            d.step(x, i, e, floors.at(i));
            if let Some(n) = notches.as_mut() {
                n.catch_up(&d.edges, x, &envs, floors);
            }
        }
    });
    match notches {
        Some(n) => n.into_edges(d.edges),
        None => d.edges,
    }
}

/// How far either side the midline looks for the signal's highest and lowest, in ms.
///
/// From below, the window must hold the longest half-wave the ROM's saver writes, the pilot's
/// 2,168 T (0.62 ms): a window inside one lobe sees one side only, its midline follows the lobe
/// itself, and the lobe's noise crosses it past the band (a pilot with noise a fifth of its
/// level chatters at 0.30 ms). From above, the midline must follow the wander: a triangle wander
/// at half the pilot's level loses the lifted lobes from 0.55 ms. 0.4 ms is chosen between. A
/// tape the ROM loads slower than the window spans, its pilot over 1.29 times the saver's, is
/// left to the zero line.
const MIDLINE_HALF_WINDOW_MS: f64 = 0.4;

/// A window that one flat level fills has no midline when the level spans less than this share
/// of it, and the level is at least [`PLATEAU_LEVEL`] of the signal scale.
///
/// A flat stretch longer than the window, such as the 1 ms level a player opens a pause with,
/// has its own level for a midpoint: taking it away would leave the DC blocker's droop, which
/// the band cuts into pulses shaped like bits. From below, that droop across the window, 4.85%
/// (the blocker's 15.9 ms against the window's 0.79 ms): at 4.5% a pause's level is cut in
/// pieces. Anything with a lobe in it spans the order of its level. 0.2 is chosen.
const PLATEAU_SPAN: f32 = 0.2;

/// The share of the signal scale a flat level must stand at to have no midline. From below, a
/// dropout's residue is flat too, near zero, and there the midline is what carries the read
/// across it. From above, what the DC blocker leaves of a level at the far end of the longest
/// the clause holds ([`PLATEAU_MOST_T`]), 0.92 of its start. 0.5 is chosen between.
const PLATEAU_LEVEL: f32 = 0.5;

/// The longest flat level, in T-states, kept on the zero line: a player's 1 ms opening level
/// with a quarter to spare, 4,375 T. From below, that level on the coarsest grid a recording
/// comes on, 437.5 T a sample at 8 kHz, spans one sample more where its edges fall between
/// samples, 3,937.5 T. From above, the DC blocker drains a held level at the scale under
/// [`PLATEAU_LEVEL`] after 11.0 ms (38,600 T), past which a longer level would be taken up and
/// let go inside. A longer level is cut only a window in from each end.
const PLATEAU_MOST_T: f64 = 3500.0 * 1.25;

/// The conditioned samples measured against their own local midline: the midpoint of the
/// highest and the lowest within [`MIDLINE_HALF_WINDOW_MS`] either side.
///
/// Zero is the signal's centre only where the tape put it there. A worn or copied tape loses
/// its treble, shrinking the short half-waves (the sync pair and the bits) but not the pilot,
/// while a slow wander can move the centre further than a shrunken lobe swings, so the lobe
/// never crosses zero and two bits merge into one. A filter cannot follow the wander, which
/// overlaps the signal's band (a high-pass at 300 Hz droops a pilot half-wave by 69%), so the
/// midline is tracked instead; on a clean recording both sides lie in every window and it is
/// about zero.
///
/// Two stretches keep the zero line instead: the samples behind silence, where a window's
/// midpoint would otherwise read the silence as a lobe of the other sign (a block's own end is
/// no such case, since its last lobe does end); and a flat level that fills the window
/// ([`PLATEAU_SPAN`]), up to [`PLATEAU_MOST_T`] long, whose samples are held back until the
/// level is known to be no longer.
struct Midline<'f, I: Iterator<Item = f32>> {
    input: I,
    done: bool,
    half_win: usize,
    /// The samples from the one being judged up to the last read.
    raw: VecDeque<f32>,
    /// How many samples have been read.
    read: usize,
    /// The window's highest and lowest, falling and rising from front to back, by index.
    high: VecDeque<(usize, f32)>,
    low: VecDeque<(usize, f32)>,
    /// The sample being judged.
    i: usize,
    floors: &'f FloorProfile,
    /// The floor at `i`, and the first sample past the block of the floor it is.
    floor: f32,
    floor_until: usize,
    peak: f32,
    /// The silent samples in a row up to `i`, and how many samples ahead still keep the zero
    /// line behind silence.
    silent: usize,
    idle: usize,
    reach: usize,
    /// A flat level in progress: each sample and its midpoint, held until the level is known.
    level: Vec<(f32, f32)>,
    level_most: usize,
    /// Whether the level in progress is longer than [`PLATEAU_MOST_T`].
    long: bool,
    out: VecDeque<f32>,
}

impl<'f, I: Iterator<Item = f32>> Midline<'f, I> {
    fn new(input: I, rate_hz: u32, peak: f32, floors: &'f FloorProfile) -> Self {
        let half_win = ((MIDLINE_HALF_WINDOW_MS / 1000.0 * f64::from(rate_hz)) as usize).max(1);
        let level_t = PLATEAU_MOST_T * f64::from(rate_hz) / f64::from(TSTATES_PER_SEC);
        Midline {
            input,
            done: false,
            half_win,
            raw: VecDeque::new(),
            read: 0,
            high: VecDeque::new(),
            low: VecDeque::new(),
            i: 0,
            floors,
            floor: floors.at(0),
            floor_until: floors.blk,
            peak,
            silent: 0,
            idle: (IDLE_AFTER_MS / 1000.0 * f64::from(rate_hz)) as usize,
            reach: 0,
            level: Vec::new(),
            level_most: (level_t as usize).saturating_sub(2 * half_win),
            long: false,
            out: VecDeque::new(),
        }
    }

    /// Judges sample `i`, which the window has read past: what it becomes, unless it is held
    /// back with a flat level, or the samples held back go out with it.
    fn judge(&mut self) -> Option<f32> {
        let i = self.i;
        while self.high.front().is_some_and(|&(j, _)| j + self.half_win < i) {
            self.high.pop_front();
        }
        while self.low.front().is_some_and(|&(j, _)| j + self.half_win < i) {
            self.low.pop_front();
        }
        let (hi, lo) = (self.high.front().map_or(0.0, |h| h.1), self.low.front().map_or(0.0, |l| l.1));
        let mid = 0.5 * (hi + lo);
        let x = self.raw.pop_front().expect("the sample judged has been read");
        if i >= self.floor_until {
            self.floor = self.floors.at(i);
            self.floor_until += self.floors.blk;
        }
        self.silent = if x.abs() <= self.floor { self.silent + 1 } else { 0 };
        let onset = if self.silent > self.idle {
            self.reach = self.half_win;
            true
        } else if self.reach > 0 {
            self.reach -= 1;
            x * (x - mid) < 0.0
        } else {
            false
        };
        let flat = mid.abs() >= PLATEAU_LEVEL * self.peak && hi - lo < PLATEAU_SPAN * mid.abs();
        self.i += 1;
        if onset || !flat {
            self.long = false;
            let y = if onset { x } else { x - mid };
            if self.level.is_empty() {
                return Some(y);
            }
            self.out.extend(self.level.drain(..).map(|(x, _)| x));
            self.out.push_back(y);
            None
        } else if self.long {
            Some(x - mid)
        } else {
            self.level.push((x, mid));
            if self.level.len() > self.level_most {
                self.out.extend(self.level.drain(..).map(|(x, m)| x - m));
                self.long = true;
            }
            None
        }
    }
}

impl<I: Iterator<Item = f32>> Iterator for Midline<'_, I> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        loop {
            if let Some(v) = self.out.pop_front() {
                return Some(v);
            }
            while !self.done && self.read <= self.i + self.half_win {
                match self.input.next() {
                    Some(x) => {
                        while self.high.back().is_some_and(|&(_, h)| h <= x) {
                            self.high.pop_back();
                        }
                        while self.low.back().is_some_and(|&(_, l)| l >= x) {
                            self.low.pop_back();
                        }
                        self.high.push_back((self.read, x));
                        self.low.push_back((self.read, x));
                        self.raw.push_back(x);
                        self.read += 1;
                    }
                    None => self.done = true,
                }
            }
            if self.i >= self.read {
                // A level the samples end in keeps the zero line.
                self.out.extend(self.level.drain(..).map(|(x, _)| x));
                return self.out.pop_front();
            }
            if let Some(y) = self.judge() {
                return Some(y);
            }
        }
    }
}

/// One turn of the signal inside a pulse: where, how far it got (seen from the side the pulse
/// opened on), and the envelope there.
#[derive(Clone, Copy)]
struct Pivot {
    i: usize,
    v: f32,
    peak: bool,
    env: f32,
}

/// The turns of one pulse's signal, found as it is walked: a turn counts once the signal has
/// come back from it by more than the reversal level.
struct Zigzag {
    /// +1 where the pulse opened on a rising edge, -1 on a falling one.
    side: f32,
    pivots: Vec<Pivot>,
    hi: Pivot,
    lo: Pivot,
    ext: Pivot,
    dir: i8,
}

impl Zigzag {
    /// From sample `a`; `pivots` is a buffer to reuse, emptied here.
    fn new(a: usize, side: f32, v: f32, env: f32, mut pivots: Vec<Pivot>) -> Self {
        pivots.clear();
        let p = Pivot { i: a, v: v * side, peak: true, env };
        Zigzag { side, pivots, hi: p, lo: Pivot { peak: false, ..p }, ext: p, dir: 0 }
    }

    fn step(&mut self, i: usize, v: f32, env: f32, rev: f32) {
        let v = v * self.side;
        let at = |peak| Pivot { i, v, peak, env };
        match self.dir {
            1 if v >= self.ext.v => self.ext = at(true),
            1 if self.ext.v - v > rev => {
                self.pivots.push(self.ext);
                (self.dir, self.ext) = (-1, at(false));
            }
            -1 if v <= self.ext.v => self.ext = at(false),
            -1 if v - self.ext.v > rev => {
                self.pivots.push(self.ext);
                (self.dir, self.ext) = (1, at(true));
            }
            0 => {
                if v > self.hi.v {
                    self.hi = at(true);
                }
                if v < self.lo.v {
                    self.lo = at(false);
                }
                if v - self.lo.v > rev {
                    self.pivots.push(self.lo);
                    (self.dir, self.ext) = (1, at(true));
                } else if self.hi.v - v > rev {
                    self.pivots.push(self.hi);
                    (self.dir, self.ext) = (-1, at(false));
                }
            }
            _ => {}
        }
    }

    fn finish(mut self) -> (Vec<Pivot>, f32) {
        self.pivots.push(Pivot { peak: self.dir >= 0, ..self.ext });
        (self.pivots, self.side)
    }
}

/// Where the samples `x` gives first cross `level` after `from`, between two samples, searching
/// to `to`; `to` where they do not.
fn mid_cross(x: impl Fn(usize) -> f32, from: usize, to: usize, level: f32) -> f64 {
    for i in from + 1..=to {
        let (a, b) = (x(i - 1), x(i));
        if (a - level) * (b - level) <= 0.0 && a != b {
            return (i - 1) as f64 + f64::from((level - a) / (b - a));
        }
    }
    to as f64
}

/// The half-waves the comparator swallowed, found as its edges come: a short half-wave riding
/// on a long one that never reached the reversal level.
///
/// Such a half-wave crosses the zero line by a hair and stays inside the pulse it rides on: on
/// a recording that has lost its treble, the sync pair on the tail of the pilot's last
/// half-wave, and the block reads a bit late with a checksum that still passes. The shape
/// decides instead: inside a pulse, a peak, a trough and a peak whose two swings each pass the
/// reversal level are a half-wave, guarded against ripple:
/// - the trough crosses the zero line, as a half-wave of the other sign does and a ripple does
///   not (without this, a pilot's noise splits one pulse in a hundred);
/// - it stays short of the reversal level, which the comparator has already ruled on;
/// - the peak after it reaches the arm band, and so does the one before unless that is the
///   pulse's own opening lobe, confirmed instead by the flank after the trough; such a lobe must
///   also last as long as a half-wave;
/// - the half-wave and the rest of the pulse behind it last at least [`MIN_HALF_TSTATES`].
///
/// Each edge is timed at the level half-way between the turns it separates: the swallowed
/// lobe's own zero line is weakened and lifted, so it would time the edge far off, and behind a
/// weak opening lobe that level is taken against the lower of the two peaks instead.
///
/// A pulse is looked at once the edge that closes it is found, while its samples and envelope
/// are still kept; one longer than that is passed over as silence, not a riding half-wave.
struct Notches {
    /// The pulses behind the edges before this one have been looked at.
    done: usize,
    /// Each half-wave found, as its two edges, and the edge it follows.
    found: Vec<(usize, [Edge; 2])>,
    /// The turns of the pulse being looked at, a buffer kept from one to the next.
    pivots: Vec<Pivot>,
    min_half: f64,
    /// The most samples a pulse looked at spans.
    longest: usize,
}

impl Notches {
    fn new(rate_hz: u32) -> Self {
        Notches {
            done: 0,
            found: Vec::new(),
            pivots: Vec::new(),
            min_half: min_half_samples(rate_hz),
            longest: rate_hz as usize,
        }
    }

    /// Looks at each pulse between two of `edges` not looked at yet; `x` and `env` hold the
    /// samples and the envelope up to its end.
    fn catch_up(&mut self, edges: &[Edge], x: &Ring, env: &Ring, floors: &FloorProfile) {
        while self.done + 1 < edges.len() {
            let p = self.done;
            self.done += 1;
            let (a, b) = (edges[p].at.ceil() as usize, edges[p + 1].at.floor() as usize);
            if b <= a + 3 || b - a > self.longest || a < x.first || a < env.first {
                continue;
            }
            let side = if edges[p].rising { 1.0 } else { -1.0 };
            let mut z = Zigzag::new(a, side, x.at(a), env.at(a), std::mem::take(&mut self.pivots));
            for j in a + 1..=b {
                let e = env.at(j);
                z.step(j, x.at(j), e, (REV_FRAC * e).max(floors.at(j)));
            }
            let found = &mut self.found;
            self.pivots =
                swallowed(z, &edges[p..p + 2], x, floors, self.min_half, |pair| found.push((p, pair)));
        }
    }

    /// `edges` with the half-waves found put back.
    fn into_edges(self, edges: Vec<Edge>) -> Vec<Edge> {
        if self.found.is_empty() {
            return edges;
        }
        let mut out = Vec::with_capacity(edges.len() + 2 * self.found.len());
        let mut found = self.found.into_iter().peekable();
        for (k, edge) in edges.into_iter().enumerate() {
            out.push(edge);
            while let Some((_, pair)) = found.next_if(|&(after, _)| after == k) {
                out.extend(pair);
            }
        }
        out
    }
}

/// Gives `found` each half-wave the turns of the pulse between the two `around` edges hold, as
/// its two edges; the turns' buffer back.
fn swallowed(
    z: Zigzag,
    around: &[Edge],
    x: &Ring,
    floors: &FloorProfile,
    min_half: f64,
    mut found: impl FnMut([Edge; 2]),
) -> Vec<Pivot> {
    let (e0, e1) = (around[0].at, around[1].at);
    let (piv, side) = z.finish();
    let y = |i: usize| x.at(i) * side;
    let rev = |p: &Pivot| (REV_FRAC * p.env).max(floors.at(p.i));
    let arm = |p: &Pivot| (BAND_FRAC * p.env).max(floors.at(p.i));
    let mut last = e0;
    for k in 1..piv.len().saturating_sub(1) {
        let (before, trough, after) = (piv[k - 1], piv[k], piv[k + 1]);
        if trough.peak || !before.peak || !after.peak {
            continue;
        }
        if trough.v > 0.0 || trough.v <= -rev(&trough) {
            continue;
        }
        let weak = before.v < arm(&before);
        let opening = piv[..k - 1].iter().all(|p| !p.peak) && before.v >= rev(&before);
        if (weak && !opening) || after.v < arm(&after) {
            continue;
        }
        let c1 = mid_cross(y, before.i, trough.i, 0.5 * (before.v + trough.v));
        let ceiling = if weak { before.v.min(after.v) } else { after.v };
        let c2 = mid_cross(y, trough.i, after.i, 0.5 * (trough.v + ceiling));
        if weak && c1 - e0 < min_half {
            continue;
        }
        // Strictly inside the pulse and in order: a lobe flush against an edge is that edge.
        if c1 <= last || c2 <= c1 || c2 >= e1 {
            continue;
        }
        if c2 - c1 < min_half || e1 - c2 < min_half {
            continue;
        }
        let rising = side < 0.0;
        found([Edge { at: c1, rising }, Edge { at: c2, rising: !rising }]);
        last = c2;
    }
    piv
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
        .map(|w| Pulse { tstates: (w[1] - w[0]) * to_t, start: w[0], quiet: false })
        .collect()
}

/// Marks quiet each of `pulses` over which no sample of `conditioned` stands above `line`,
/// counting the samples either side of both its edges, so a pulse shorter than a sample is
/// measured too.
fn mark_quiet(pulses: &mut [Pulse], conditioned: impl Iterator<Item = f32>, rate_hz: u32, line: f32) {
    let to_samples = f64::from(rate_hz) / f64::from(TSTATES_PER_SEC);
    let span = |p: &Pulse| (p.start.floor() as usize, (p.start + p.tstates * to_samples).ceil() as usize);
    let mut loud = vec![false; pulses.len()];
    // The first pulse whose span has not ended; spans overlap only at their ends.
    let mut first = 0;
    for (j, x) in conditioned.enumerate() {
        while first < pulses.len() && span(&pulses[first]).1 < j {
            first += 1;
        }
        if first == pulses.len() {
            break;
        }
        if x.abs() > line {
            for (k, p) in pulses.iter().enumerate().skip(first) {
                if span(p).0 > j {
                    break;
                }
                loud[k] = true;
            }
        }
    }
    for (p, loud) in pulses.iter_mut().zip(loud) {
        p.quiet = !loud;
    }
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

/// Calls `f` for every index in order with the envelope there and the last `keep` samples read,
/// which reach past it as far as the envelope looks.
///
/// The envelope is the loudest `|x|` within `half_win` either side, or with `fade_blk` the fade
/// band's ([`Band::Fade`]): the median of the peaks of the `fade_blk`-sample blocks within as
/// many blocks either side as `half_win` holds, the same for every index of a block.
fn walk(
    mut xs: impl Iterator<Item = f32>,
    half_win: usize,
    fade_blk: Option<usize>,
    keep: usize,
    mut f: impl FnMut(usize, f32, &Ring),
) {
    let mut ring = Ring { v: VecDeque::with_capacity(keep), first: 0, cap: keep };
    // Indices with their `|x|`, falling from front to back: the front is the window's loudest.
    let mut loud: VecDeque<(usize, f32)> = VecDeque::new();
    let mut fade = fade_blk.map(|blk| FadeScan::new(blk, half_win));
    let ahead = half_win + fade_blk.unwrap_or(0);
    let (mut read, mut done) = (0usize, false);
    for i in 0.. {
        while !done && read <= i + ahead {
            match xs.next() {
                Some(x) => {
                    ring.push(x);
                    match fade.as_mut() {
                        Some(fade) => fade.see(x),
                        None => {
                            while loud.back().is_some_and(|&(_, a)| a <= x.abs()) {
                                loud.pop_back();
                            }
                            loud.push_back((read, x.abs()));
                        }
                    }
                    read += 1;
                }
                None => {
                    done = true;
                    if let Some(fade) = fade.as_mut() {
                        fade.close();
                    }
                }
            }
        }
        if i >= read {
            break;
        }
        let env = match fade.as_mut() {
            Some(fade) => fade.at(i),
            None => {
                while loud.front().is_some_and(|&(j, _)| j + half_win < i) {
                    loud.pop_front();
                }
                loud.front().map_or(0.0, |&(_, a)| a)
            }
        };
        f(i, env, &ring);
    }
}

/// The fade band's envelope, walked: block peaks as the samples are read, and the median of
/// those within `reach` blocks either side of the block being judged, kept sorted as the window
/// moves on by a block.
struct FadeScan {
    blk: usize,
    reach: usize,
    /// The peaks of the blocks read whole, from block `first`.
    peaks: VecDeque<f32>,
    first: usize,
    /// The peak of the block being read so far, and how many of its samples are read.
    open: f32,
    open_n: usize,
    /// The peaks of blocks `lo` up to, not including, `hi`, sorted.
    window: Vec<f32>,
    lo: usize,
    hi: usize,
    /// The block judged last and its median.
    k: Option<usize>,
    median: f32,
}

impl FadeScan {
    fn new(blk: usize, half_win: usize) -> Self {
        FadeScan {
            blk,
            reach: (half_win / blk).max(1),
            peaks: VecDeque::new(),
            first: 0,
            open: 0.0,
            open_n: 0,
            window: Vec::new(),
            lo: 0,
            hi: 0,
            k: None,
            median: 0.0,
        }
    }

    fn see(&mut self, x: f32) {
        self.open = self.open.max(x.abs());
        self.open_n += 1;
        if self.open_n == self.blk {
            self.peaks.push_back(self.open);
            (self.open, self.open_n) = (0.0, 0);
        }
    }

    /// The samples have run out: a block read in part is a block.
    fn close(&mut self) {
        if self.open_n > 0 {
            self.peaks.push_back(self.open);
            (self.open, self.open_n) = (0.0, 0);
        }
    }

    fn peak(&self, block: usize) -> f32 {
        self.peaks[block - self.first]
    }

    /// The envelope at sample `i`, which the samples read reach `reach + 1` blocks past, or
    /// past which they have run out.
    fn at(&mut self, i: usize) -> f32 {
        let k = i / self.blk;
        if self.k == Some(k) {
            return self.median;
        }
        let (lo, hi) =
            (k.saturating_sub(self.reach), (k + self.reach + 1).min(self.first + self.peaks.len()));
        while self.hi < hi {
            let v = self.peak(self.hi);
            let at = self.window.partition_point(|a| a.total_cmp(&v).is_lt());
            self.window.insert(at, v);
            self.hi += 1;
        }
        while self.lo < lo {
            let v = self.peak(self.lo);
            let at = self.window.partition_point(|a| a.total_cmp(&v).is_lt());
            self.window.remove(at);
            self.lo += 1;
        }
        while self.first < self.lo {
            self.peaks.pop_front();
            self.first += 1;
        }
        self.k = Some(k);
        self.median = self.window.get(self.window.len().saturating_sub(1) / 2).copied().unwrap_or(0.0);
        self.median
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

/// The noise line a pulse must stay under to be quiet, as a share of the loudest the recording
/// gets where it holds tape: [`QUIET_PCTILE`] of the envelope is a noise level only under this.
/// Dead air alone measures its own ratio, at most 0.64 (a sliding max over hiss against its max,
/// which material only lowers), so at 0.6 a recording of dead air has no line and none of its
/// hiss is quiet. Material of one level measures over 0.81 (its level over its level plus the
/// loudest hiss it stands [`MATERIAL_MULT`] over), so at 0.9 a recording without dead air has a
/// line, in its own signal. 0.7 is chosen between.
const QUIET_GUARD: f32 = 0.7;

/// The recording's noise line: [`QUIET_PCTILE`] of the envelope of the seconds that hold tape,
/// lowered to the tape's own dead air where that percentile's share of dead air is not there.
/// `None` where the recording has no quiet to measure: the percentile is not under
/// [`QUIET_GUARD`] of the loudest, or the only quiet is samples that never change (a tape
/// rendered from a file, whose pauses hold no noise).
fn noise_line(env: &[f32], moved: &[(bool, bool)], per_sec: usize, peak: f32) -> Option<f32> {
    let dead = dead_air_mark(env, moved, per_sec, peak);
    let per_sec = per_sec.max(1);
    let silent = moved.chunks(per_sec).any(|m| !holds_tape(m));
    let mut tape: Vec<f32> = env
        .chunks(per_sec)
        .zip(moved.chunks(per_sec))
        .filter(|(_, m)| holds_tape(m))
        .flat_map(|(e, _)| e.iter().copied())
        .collect();
    if tape.is_empty() {
        return None;
    }
    let pct = pctile(&mut tape, QUIET_PCTILE);
    if silent && dead.is_none() && !is_noise_level(pct, peak) {
        return None;
    }
    let top = tape[tape.len() - 1];
    (pct <= QUIET_GUARD * top).then(|| dead.map_or(pct, |d| d.min(pct)))
}

/// The tape's dead air, where [`QUIET_PCTILE`] cannot measure it because dead air fills less
/// than that share: counted a second at a time, or a tenth of a second where the quietest
/// seconds are material.
fn dead_air_mark(env: &[f32], moved: &[(bool, bool)], per_sec: usize, peak: f32) -> Option<f32> {
    match dead_air_at(env, moved, per_sec)? {
        (est, _) if !is_noise_level(est, peak) => match dead_air_at(env, moved, per_sec / DEAD_AIR_FINE)? {
            (fine, true) if is_noise_level(fine, peak) => Some(fine),
            _ => None,
        },
        (est, short) => short.then_some(est),
    }
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
    /// The floors for `band`: the fade band's are [`FADE_FLOOR_SPAN`] of the ordinary share.
    fn local(env: &[f32], per_block: usize, stride: usize, peak: f32, mark: f32, band: Band) -> Self {
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
        // The fade band's envelope follows a dropout down, so inside one its levels are shares
        // of the dropout's level, and the floor decides in signal at any scale.
        let (share, bind) = match band {
            Band::Max => (FLOOR_FRAC, FLOOR_FRAC / REV_FRAC * peak),
            Band::Fade => (FADE_FLOOR_SPAN * FLOOR_FRAC, f32::INFINITY),
        };
        let v =
            scale.into_iter().map(|s| share * if s < bind { s.clamp(lowest, peak) } else { peak }).collect();
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
