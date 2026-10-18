// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Sampled audio through the edge detector: tapes rendered to WAV by `tapeti-core`, worn on
//! purpose, decoded back.

mod common;

use common::same_tape;
use std::ops::ControlFlow;
use std::sync::atomic::AtomicBool;
use tapeti_core::audio::{encode_wav, playback_order, render_tape, FlowOptions, RenderOptions};
use tapeti_core::types::{Block, Body};
use tapeti_decode::entry::{decode_file, decode_pulses, tape_band, Channels, Decoded, Finding};
use tapeti_decode::readers::Channel;
use tapeti_decode::waveform::edges::{detect, detect_on, detect_pulses, detect_pulses_each, pulses, Axis};
use tapeti_decode::waveform::filter::Filter;

fn standard(pause: u16, data: Vec<u8>) -> Block {
    Block::new(Body::Standard { pause, data })
}

fn with_checksum(flag: u8, bytes: &[u8]) -> Vec<u8> {
    let mut data = vec![flag];
    data.extend(bytes);
    data.push(data.iter().fold(0, |x, &d| x ^ d));
    data
}

fn tape() -> Vec<Block> {
    let body: Vec<u8> = (0..400u32).map(|i| (i * 37 + 11) as u8).collect();
    let mut h = vec![3];
    h.extend(b"edges     ");
    h.extend((body.len() as u16).to_le_bytes());
    h.extend([0x00, 0x80, 0x00, 0x80]);
    vec![
        standard(1000, with_checksum(0x00, &h)),
        standard(2000, with_checksum(0xFF, &body)),
        standard(1000, with_checksum(0xFF, &(0..=255).collect::<Vec<u8>>())),
    ]
}

fn render(blocks: &[Block], rate: u32, mic: bool) -> Vec<f32> {
    let order = playback_order(blocks, FlowOptions::default());
    render_tape(blocks, RenderOptions { sample_rate: rate, mic, amplitude: 0.5 }, &order)
}

fn decode_wav(wav: &[u8], filter: Filter) -> Decoded {
    let no_zlib = |_: &[u8]| -> Result<Vec<u8>, String> { Err("none".into()) };
    decode_file(
        "t.wav",
        wav,
        Channels::Only(Channel::Mix),
        filter,
        None,
        &no_zlib,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap()
}

fn datas(blocks: &[Block]) -> Vec<Vec<u8>> {
    blocks
        .iter()
        .filter_map(|b| match &b.body {
            Body::Standard { data, .. } => Some(data.clone()),
            _ => None,
        })
        .collect()
}

/// Checks that `tape()` comes back from `samples` at `rate`, block for block, pauses included.
fn reads(samples: &[f32], rate: u32, bits: u16, filter: Filter) {
    let got = decode_wav(&encode_wav(samples, rate, bits), filter);
    assert_eq!(same_tape(&got.blocks, &tape(), 1.0..=1.0), Ok(()), "{:?}", got.findings);
}

#[test]
fn the_spectrums_output_decodes_to_the_same_blocks() {
    for rate in [22050, 44100, 48000] {
        let got = decode_wav(&encode_wav(&render(&tape(), rate, true), rate, 16), Filter::None);
        assert_eq!(got.findings, [Finding::LeadIn { ms: 250 }], "{rate} Hz");
        assert_eq!(got.blocks, tape(), "{rate} Hz");
        assert_eq!(got.rate_hz, rate);
    }
}

#[test]
fn a_square_wave_holding_its_pause_level_decodes_to_the_same_blocks() {
    // A plain square wave holds a pause at the low level for a second, which no tape carries,
    // and the DC blocker drains it to zero, so the first pilot halves after it rise from zero.
    // The tape still comes back block for block, pauses included, with no raw signal.
    let got = decode_wav(&encode_wav(&render(&tape(), 44100, false), 44100, 16), Filter::None);
    assert_eq!(got.findings, [Finding::LeadIn { ms: 250 }]);
    assert_eq!(got.blocks, tape());
}

#[test]
fn the_spectrums_own_output_and_eight_bits_decode() {
    reads(&render(&tape(), 44100, true), 44100, 16, Filter::None);
    reads(&render(&tape(), 44100, false), 44100, 8, Filter::None);
    reads(&render(&tape(), 11025, false), 11025, 16, Filter::None);
}

#[test]
fn hiss_is_not_read_as_edges() {
    // Uniform noise of ±0.06 on a level of 0.5, everywhere, the silences too, from a seeded
    // xorshift so every run is the same.
    let mut state = 5u64;
    let mut unit = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let noisy: Vec<f32> = render(&tape(), 44100, false).iter().map(|&v| v + 0.12 * (unit() - 0.5)).collect();
    reads(&noisy, 44100, 16, Filter::None);
    reads(&noisy, 44100, 16, tape_band(44100));
}

#[test]
fn a_dull_treble_and_a_drifting_centre_line_still_decode() {
    // A one-pole low-pass at about 4 kHz rounds the square wave; then a 0.5 Hz drift of a fifth of
    // the level lifts the whole signal off zero and back.
    let rate = 44100.0;
    let a = (-2.0 * std::f32::consts::PI * 4000.0 / rate).exp();
    let mut y = 0.0f32;
    let shaped: Vec<f32> = render(&tape(), 44100, false)
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            y = a * y + (1.0 - a) * v;
            y + 0.1 * (2.0 * std::f32::consts::PI * 0.5 * i as f32 / rate).sin()
        })
        .collect();
    reads(&shaped, 44100, 16, Filter::None);
}

#[test]
fn a_tape_that_fades_still_decodes() {
    let s = render(&tape(), 44100, false);
    let n = s.len() as f32;
    let faded: Vec<f32> = s.iter().enumerate().map(|(i, &v)| v * (1.0 - 0.8 * i as f32 / n)).collect();
    reads(&faded, 44100, 16, Filter::None);
}

#[test]
fn a_quiet_recording_between_loud_ones_decodes() {
    // Three copies of the tape, the middle one at a tenth of the level.
    let s = render(&tape(), 44100, false);
    let mut all = s.clone();
    all.extend(s.iter().map(|v| 0.1 * v));
    all.extend(&s);
    let got = decode_wav(&encode_wav(&all, 44100, 16), Filter::None);
    // Between two copies, the quarter second the next opens with.
    let mut want = [tape(), tape(), tape()].concat();
    for k in [2, 5] {
        if let Body::Standard { pause, .. } = &mut want[k].body {
            *pause += 250;
        }
    }
    assert_eq!(same_tape(&got.blocks, &want, 1.0..=1.0), Ok(()), "{:?}", got.findings);
}

#[test]
fn silence_has_no_edges_and_the_pulses_span_the_recording() {
    let silent = vec![0.0f32; 44100];
    assert!(detect(|| silent.iter().copied(), 44100, Filter::None).unwrap().is_empty());

    let s = render(&tape(), 44100, false);
    let edges = detect(|| s.iter().copied(), 44100, Filter::None).unwrap();
    assert!(edges.windows(2).all(|w| w[0].at < w[1].at && w[0].rising != w[1].rising));
    let p = pulses(&edges, 44100, s.len());
    let total: f64 = p.iter().map(|p| p.tstates).sum();
    assert!((total - s.len() as f64 * 3_500_000.0 / 44100.0).abs() < 1e-3);
}

#[test]
fn a_spike_in_a_pause_is_not_a_block() {
    // One sample out of line, a third of the level, in the middle of a pause: no edge pair the
    // ROM would read, so the blocks either side come back whole.
    let mut s = render(&tape(), 44100, false);
    let at = s.len() - 20000;
    s[at] += 0.17;
    reads(&s, 44100, 16, Filter::None);
}

/// Gaussian noise of `sigma` from a seeded generator (the sum of four uniform numbers), so every
/// run is the same.
fn hiss(n: usize, sigma: f32, seed: u64) -> Vec<f32> {
    let mut state = seed;
    let mut unit = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    // Four uniform numbers have a variance of 4/12 around 2.
    let scale = sigma / (4.0f32 / 12.0).sqrt();
    (0..n).map(|_| scale * ((0..4).map(|_| unit()).sum::<f32>() - 2.0)).collect()
}

#[test]
fn dead_air_is_quiet_and_plays_as_the_pause() {
    // Three seconds of dead air, the tape with five seconds between its blocks, three seconds
    // more: the hiss of the tape alone, everywhere, a tenth of the level. The band cuts it into
    // pulses, which are quiet, and every stretch of it is the pause before the next block.
    let mut blocks = tape();
    for b in &mut blocks {
        if let Body::Standard { pause, .. } = &mut b.body {
            *pause = 5000;
        }
    }
    let mut s = vec![0.0f32; 3 * 44100];
    s.extend(render(&blocks, 44100, false));
    s.extend(vec![0.0f32; 3 * 44100]);
    let noise = hiss(s.len(), 0.015, 7);
    for (v, n) in s.iter_mut().zip(noise) {
        *v += n;
    }
    let p = detect_pulses(|| s.iter().copied(), 44100, Filter::None).unwrap();
    let quiet = p.iter().filter(|p| p.quiet).count();
    assert!(quiet > 10_000, "{quiet} of {} pulses quiet", p.len());

    let got = decode_wav(&encode_wav(&s, 44100, 16), Filter::None);
    assert_eq!(datas(&got.blocks), datas(&blocks), "{:?}", got.findings);
    assert_eq!(got.blocks.len(), blocks.len(), "nothing but the blocks: {:?}", got.findings);
    for (b, want) in got.blocks.iter().zip([5000, 5000, 8000]) {
        let Body::Standard { pause, .. } = b.body else { unreachable!() };
        assert!(pause.abs_diff(want) <= 5, "a pause of {pause} ms for {want}");
    }
    assert!(matches!(got.findings[0], Finding::LeadIn { ms: 3150..=3350 }), "{:?}", got.findings);
    let collapsed: Vec<usize> = got
        .findings
        .iter()
        .filter_map(|f| match *f {
            Finding::Collapsed { block, .. } => Some(block),
            _ => None,
        })
        .collect();
    assert_eq!(collapsed, [0, 1, 2], "{:?}", got.findings);
}

#[test]
fn a_recording_whose_pauses_hold_no_noise_has_no_quiet_pulse() {
    // Rendered from a file, the silence is samples that never change: nothing measures a noise
    // level there, and the signal itself must not be taken for one.
    let s = render(&tape(), 44100, false);
    let p = detect_pulses(|| s.iter().copied(), 44100, Filter::None).unwrap();
    assert!(p.iter().all(|p| !p.quiet));
}

/// Half-sines of `(amplitude, samples)` laid end to end, with a quarter of a second of silence
/// either side, and where each lobe starts.
fn lobes(list: &[(f64, f64)], rate: u32) -> (Vec<f32>, Vec<f64>) {
    let lead = rate as usize / 4;
    let mut x = vec![0.0f32; lead];
    let (mut t0, mut marks) = (lead as f64, Vec::new());
    for &(amp, len) in list {
        marks.push(t0);
        let end = t0 + len;
        while (x.len() as f64) < end {
            let t = x.len() as f64;
            x.push((amp * (std::f64::consts::PI * (t - t0) / len).sin()) as f32);
        }
        t0 = end;
    }
    x.extend(vec![0.0f32; lead]);
    (x, marks)
}

/// `t` T-states in samples at 44.1 kHz.
fn at_44k(t: f64) -> f64 {
    t * 44100.0 / 3_500_000.0
}

/// 400 pilot lobes at 0.5 and the given lobes behind them, then 40 of `tail` samples at 0.4, the
/// first of them low; the edges from the first of the given lobes to the middle of the third,
/// and where its second and third start.
fn after_a_pilot(given: [(f64, f64); 3], tail: f64) -> (Vec<tapeti_decode::waveform::edges::Edge>, f64, f64) {
    let mut list: Vec<(f64, f64)> =
        (0..400).map(|k| (if k % 2 == 0 { 0.5 } else { -0.5 }, at_44k(2168.0))).collect();
    list.extend(given);
    list.extend((0..40).map(|k| (if k % 2 == 0 { -0.4 } else { 0.4 }, tail)));
    let (x, marks) = lobes(&list, 44100);
    let (open, second, third) = (marks[400], marks[401], marks[402]);
    let edges = detect(|| x.iter().copied(), 44100, Filter::None).unwrap();
    let near = edges.into_iter().filter(|e| e.at > open - 2.0 && e.at < third + given[2].1 / 2.0).collect();
    (near, second, third)
}

#[test]
fn a_shallow_trough_behind_a_weak_lobe_is_a_half_wave() {
    // A weak lobe (0.10: over the reversal level 0.05, under the arm band 0.125) lasting a 0 bit's
    // half-wave, a trough to -0.04, then a strong lobe: the comparator reads the trough as its own
    // half-wave, timing its rise from the strong lobe's start, bounded by the weak lobe's swing.
    let bit0 = at_44k(855.0);
    let (on, trough, strong) = after_a_pilot([(0.10, bit0), (-0.04, 0.65 * bit0), (0.4, bit0)], bit0);
    assert_eq!(on.len(), 3, "{on:?}");
    assert!(on[0].rising && !on[1].rising && on[2].rising, "{on:?}");
    assert!(on[1].at > trough - bit0 / 2.0 && on[1].at < trough + 1.0, "{on:?}");
    assert!((on[2].at - strong).abs() < 1.0, "the rise at {}, the lobe at {strong}", on[2].at);

    // A trough that stays on its side of the zero line is ripple, not a half-wave.
    let (ripple, ..) = after_a_pilot([(0.10, bit0), (0.04, 0.65 * bit0), (0.4, bit0)], bit0);
    assert_eq!(ripple.len(), 1, "{ripple:?}");
}

#[test]
fn a_wiggle_opening_a_pulse_is_not_a_half_wave() {
    // The same shape with a weak lobe of four samples (317 T), under the shortest half-wave.
    let bit0 = at_44k(855.0);
    let (on, ..) = after_a_pilot([(0.10, 4.0), (-0.04, 0.65 * bit0), (0.4, bit0)], bit0);
    assert_eq!(on.len(), 1, "{on:?}");
}

#[test]
fn a_trough_leaves_no_sliver_of_its_pulse_behind_it() {
    // A pilot lobe holding a trough to -0.04 a 0 bit's half-wave long and a flank at 0.4 behind
    // it: recovered with a 0 bit's half-wave of flank, not with four samples, which would leave
    // a sliver no saver writes.
    let (pilot, bit0) = (at_44k(2168.0), at_44k(855.0));
    let (on, ..) = after_a_pilot([(0.5, pilot), (-0.04, bit0), (0.4, bit0)], pilot);
    assert_eq!(on.len(), 3, "{on:?}");
    let (on, ..) = after_a_pilot([(0.5, pilot), (-0.04, bit0), (0.4, 4.0)], pilot);
    assert_eq!(on.len(), 1, "{on:?}");
}

/// A block as lobes: a pilot at 0.5, the sync pair as a recording that lost its treble leaves
/// it (a weak first half and a trough that barely crosses the zero line), the bits at 0.4 and
/// the saver's closing level, then a second of silence.
fn dull_sync(data: &[u8], pilot_pulses: usize) -> Vec<(f64, f64)> {
    let mut halves: Vec<(f64, f64)> = Vec::new();
    let mut sign = 1.0;
    let mut push = |amp: f64, t: f64, halves: &mut Vec<(f64, f64)>| {
        halves.push((sign * amp, at_44k(t)));
        sign = -sign;
    };
    for _ in 0..pilot_pulses {
        push(0.5, 2168.0, &mut halves);
    }
    push(0.10, 667.0, &mut halves);
    push(0.04, 735.0, &mut halves);
    for &byte in data {
        for bit in (0..8).rev() {
            let t = if byte >> bit & 1 == 1 { 1710.0 } else { 855.0 };
            push(0.4, t, &mut halves);
            push(0.4, t, &mut halves);
        }
    }
    push(0.4, 945.0, &mut halves);
    halves.push((0.0, 44100.0));
    halves
}

#[test]
fn a_sync_pair_swallowed_by_a_dull_recording_is_read_back() {
    // Merged, the sync pair and the first data half are one pulse the ROM takes for more pilot,
    // and the block reads half a bit or more out of step.
    let blocks = &tape()[..2];
    let mut list = Vec::new();
    for (b, pilot) in blocks.iter().zip([8063, 3223]) {
        let Body::Standard { data, .. } = &b.body else { unreachable!() };
        list.extend(dull_sync(data, pilot));
    }
    let (x, _) = lobes(&list, 44100);
    let got = decode_wav(&encode_wav(&x, 44100, 16), Filter::None);
    assert_eq!(datas(&got.blocks), datas(blocks), "{:?}", got.findings);
}

/// A triangle wave of unit amplitude: 0 at 0, 1 at a quarter, -1 at three quarters.
fn tri(phase: f64) -> f64 {
    let p = phase.rem_euclid(1.0);
    if p < 0.25 {
        4.0 * p
    } else if p < 0.75 {
        2.0 - 4.0 * p
    } else {
        4.0 * p - 4.0
    }
}

/// A header and its data block as half-sines at 44.1 kHz, the pilot at 0.5 and every shorter
/// half-wave at 0.2, as a recording that lost its treble leaves them, under a triangle wander of
/// `wander` with a period of 10 ms; a second of silence after each.
fn worn(wander: f32) -> (Vec<f32>, Vec<Vec<u8>>) {
    let mut h = vec![3];
    h.extend(b"wander    ");
    h.extend(200u16.to_le_bytes());
    h.extend([0x00, 0x80, 0x00, 0x80]);
    let body: Vec<u8> = (0..200u32).map(|i| (i * 37 + 1) as u8).collect();
    let datas = vec![with_checksum(0x00, &h), with_checksum(0xFF, &body)];
    let mut x = vec![0.0f32; 22050];
    for (data, pilot) in datas.iter().zip([8063, 3223]) {
        let mut halves: Vec<(f64, f64)> = vec![(0.5, at_44k(2168.0)); pilot];
        halves.extend([(0.2, at_44k(667.0)), (0.2, at_44k(735.0))]);
        for &byte in data {
            for k in (0..8).rev() {
                let t = at_44k(if byte >> k & 1 == 1 { 1710.0 } else { 855.0 });
                halves.extend([(0.2, t), (0.2, t)]);
            }
        }
        halves.push((0.2, at_44k(945.0)));
        let list: Vec<(f64, f64)> =
            halves.iter().enumerate().map(|(k, &(a, l))| (if k % 2 == 0 { a } else { -a }, l)).collect();
        let (mut block, _) = lobes(&list, 44100);
        // The wander from a period into the lobes to their end, in whole periods.
        let (from, to) = (11025 + 441, block.len() - 11025);
        let periods = ((to - from) as f64 / 441.0).round();
        for (i, s) in block[from..to].iter_mut().enumerate() {
            *s += wander * tri(i as f64 * periods / (to - from) as f64) as f32;
        }
        x.extend(block);
        x.extend(vec![0.0f32; 44100]);
    }
    (x, datas)
}

#[test]
fn a_wandering_centre_line_is_read_about_the_signals_own_midline() {
    // Lifted by a wander of a quarter of the pilot's level, the shrunken lobes of the other
    // sign never cross zero; about the midline each is its own excursion.
    let (x, want) = worn(0.25);
    let on_zero = detect_pulses_each(
        || x.iter().copied(),
        44100,
        Filter::None,
        &[Axis::Zero],
        |_, p| {
            ControlFlow::Break(datas(
                &decode_pulses(&p, 44100, &mut |_| {}, &AtomicBool::new(false)).unwrap().blocks,
            ))
        },
    )
    .unwrap();
    assert!(!on_zero[0].contains(&want[0]) && !on_zero[0].contains(&want[1]), "{:?}", on_zero);
    let got = decode_wav(&encode_wav(&x, 44100, 16), Filter::None);
    assert_eq!(datas(&got.blocks), want, "{:?}", got.findings);
    assert_eq!(got.findings[0], Finding::Midline);
    // Where both lines read the blocks the zero line's decode is kept.
    let (x, want) = worn(0.15);
    let got = decode_wav(&encode_wav(&x, 44100, 16), Filter::None);
    assert_eq!(datas(&got.blocks), want);
    assert!(!got.findings.contains(&Finding::Midline));
}

/// A square pilot of 400 half-waves at ±0.5 at `rate`, with `noise` (uniform, seeded) on every
/// sample and the half-wave at `level_at` replaced by a flat level `level` samples long, between
/// 50 ms of silence; with the level's first and one-past-last sample.
fn square_pilot(rate: u32, noise: f32, level_at: Option<(usize, usize)>) -> (Vec<f32>, (usize, usize)) {
    let gap = rate as usize / 20;
    let half = 2168.0 * f64::from(rate) / 3_500_000.0;
    let mut state = 1u64;
    let mut unit = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((state >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    };
    let mut x = vec![0.0f32; gap];
    let (mut sign, mut span, mut t) = (0.5f32, (0, 0), 0.0f64);
    for k in 0..400 {
        let len = match level_at {
            Some((at, level)) if k == at => {
                span = (x.len(), x.len() + level);
                level
            }
            _ => {
                let from = t.round() as usize;
                t += half;
                t.round() as usize - from
            }
        };
        x.extend((0..len).map(|_| sign + noise * unit()));
        sign = -sign;
    }
    x.extend(vec![0.0f32; gap]);
    (x, span)
}

#[test]
fn the_midline_window_holds_a_pilot_half_wave() {
    // Noise of a fifth of the pilot's level makes no edge about the midline: its window spans a
    // whole pilot half-wave, so it never sits inside one lobe.
    let edges =
        |x: &[f32]| detect_on(|| x.iter().copied(), 44100, Filter::None, Axis::Midline).unwrap().len();
    let (clean, _) = square_pilot(44100, 0.0, None);
    assert_eq!(detect(|| clean.iter().copied(), 44100, Filter::None).unwrap().len(), 400);
    assert_eq!(edges(&square_pilot(44100, 0.1, None).0), edges(&clean));
}

#[test]
fn a_pause_level_about_the_midline_is_one_level() {
    // A player's 1 ms level fills the window flat and has no midline of its own; it keeps the
    // zero line, at 44.1 kHz and on an 8 kHz grid, where it spans one sample more.
    for (rate, level) in [(44100, 44), (8000, 9)] {
        let (x, (from, to)) = square_pilot(rate, 0.0, Some((200, level)));
        let edges = detect_on(|| x.iter().copied(), rate, Filter::None, Axis::Midline).unwrap();
        let inside = edges.iter().filter(|e| e.at > from as f64 + 1.0 && e.at < to as f64 - 1.0).count();
        assert_eq!(inside, 0, "{rate} Hz: {edges:?}");
    }
}

#[test]
fn a_block_out_of_silence_starts_on_its_first_lobe_about_the_midline() {
    // Out of silence a window holds the silence and the first lobe; the midpoint between them
    // would cut the lobe's rise in two, so the zero line stays there. About the midline a block
    // has one edge more than on the zero line, where its last lobe falls to silence.
    let mut list: Vec<(f64, f64)> =
        (0..400).map(|k| (if k % 2 == 0 { 0.5 } else { -0.5 }, at_44k(2168.0))).collect();
    list.extend(
        (0..320)
            .map(|k| (if k % 2 == 0 { 0.4 } else { -0.4 }, at_44k(if k % 4 < 2 { 855.0 } else { 1710.0 }))),
    );
    let (x, _) = lobes(&list, 44100);
    let zero = detect(|| x.iter().copied(), 44100, Filter::None).unwrap().len();
    let midline = detect_on(|| x.iter().copied(), 44100, Filter::None, Axis::Midline).unwrap().len();
    assert_eq!((zero, midline), (720, 721));
}
