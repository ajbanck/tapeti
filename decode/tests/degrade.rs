// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! What each of the cassette chain's defects in `common::degrade` must do to the samples, one test
//! for each, so that a wall measured with it is the defect's wall and not a weaker one's.

mod common;

use common::degrade::*;
use common::Rng;
use tapeti_core::audio::encode_wav;

/// A square wave of ±0.5 whose half-waves are `half` samples, `count` of them, the first high.
fn square(half: usize, count: usize) -> Vec<f32> {
    (0..half * count).map(|n| if (n / half).is_multiple_of(2) { 0.5 } else { -0.5 }).collect()
}

/// `n` numbers from -`span` to `span`, seeded.
fn noise(n: usize, span: f32, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..n).map(|_| span * (2.0 * rng.unit() as f32 - 1.0)).collect()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt()
}

/// The gaps between consecutive crossings of `x`.
fn gaps(x: &[f32]) -> Vec<f64> {
    crossings(x).windows(2).map(|w| w[1] - w[0]).collect()
}

/// A high half-wave of `n` samples between two long low ones.
fn lone_lobe(n: usize) -> Vec<f32> {
    let mut x = vec![-0.5f32; 6000];
    x.extend(vec![0.5f32; n]);
    x.extend(vec![-0.5f32; 6000]);
    x
}

/// The highest sample of the lobe [`lone_lobe`] put at 6000.
fn lobe_top(y: &[f32], n: usize) -> f32 {
    y[5990..6010 + n].iter().copied().fold(f32::MIN, f32::max)
}

#[test]
fn spacing_loss_removes_a_short_half_wave_rather_than_shrinking_it() {
    // A lobe of 10 samples at 44.1 kHz keeps a crossing while it is longer than 2a, twice the
    // Lorentzian's half-width: up to 10 / 88,200 s × 47,625 µm/s = 5.40 µm. Just inside, it is
    // a sliver over zero; just past, nothing crosses at all.
    let x = lone_lobe(10);
    let kept = spacing_loss(&x, 44_100, 5.0);
    let top = lobe_top(&kept, 10);
    assert!(top > 0.0 && top < 0.1, "at 5.0 µm the lobe tops at {top}");
    assert_eq!(crossings(&kept).len(), 2);
    let gone = spacing_loss(&x, 44_100, 5.8);
    assert!(lobe_top(&gone, 10) < 0.0, "at 5.8 µm the lobe tops at {}", lobe_top(&gone, 10));
    assert!(crossings(&gone).is_empty());
}

#[test]
fn spacing_loss_takes_sync_1_past_four_and_a_half_microns() {
    // 667 T is 67 samples at 352.8 kHz: 2a reaches it at 4.52 µm.
    let x = lone_lobe(67);
    assert_eq!(crossings(&spacing_loss(&x, 352_800, 4.3)).len(), 2);
    assert!(crossings(&spacing_loss(&x, 352_800, 4.8)).is_empty());
}

#[test]
fn spacing_loss_keeps_an_edge_in_place_and_a_level_as_it_is() {
    let step: Vec<f32> = (0..4000).map(|n| if n < 2000 { -0.5 } else { 0.5 }).collect();
    let y = spacing_loss(&step, 44_100, 3.0);
    // Symmetric about the step, so the crossing stays half-way between samples 1999 and 2000.
    assert!(y[1999] < 0.0 && y[2000] > 0.0 && (y[1999] + y[2000]).abs() < 1e-6, "{} {}", y[1999], y[2000]);
    assert!(y[2000] < 0.2, "the step is rounded: {}", y[2000]);
    // Beyond the kernel, which at 3 µm reaches 178 samples, each level is itself.
    assert!((y[0] + 0.5).abs() < 1e-6 && (y[3999] - 0.5).abs() < 1e-6);
    assert_eq!(spacing_loss(&step, 44_100, 0.0), step);
}

#[test]
fn azimuth_delays_one_kind_of_edge_and_keeps_each_pairs_sum() {
    // Half-waves of 28 samples at 44.1 kHz. A rise at 10 µs crosses within a sample; a fall at
    // 250 µs (c = 0.083 a sample) takes about 8 samples to reach zero.
    let x = square(28, 200);
    let g = gaps(&azimuth(&x, 44_100, 10.0, 250.0));
    // Skip the start, where the filter settles. The first crossing is a fall, so the gaps
    // alternate low, high from an even index.
    for k in (10..g.len() - 2).step_by(2) {
        let (low, high) = (g[k], g[k + 1]);
        assert!(high - low > 10.0 && high - low < 20.0, "gap {k}: low {low}, high {high}");
        assert!((low + high - 56.0).abs() < 0.5, "gap {k}: a pair of {}", low + high);
    }
    // The other way round, the other kind of pulse lengthens.
    let g = gaps(&azimuth(&x, 44_100, 250.0, 10.0));
    assert!(g[11] - g[10] < -10.0, "{} {}", g[10], g[11]);
    // Equal time constants move both kinds of edge alike.
    let g = gaps(&azimuth(&x, 44_100, 100.0, 100.0));
    assert!(g[10..g.len() - 2].iter().all(|&d| (d - 28.0).abs() < 0.1), "{:?}", &g[10..20]);
}

#[test]
fn fade_keeps_the_level_falls_in_a_line_and_holds_at_its_depth() {
    let x = noise(1000, 0.5, 3);
    let y = fade(&x, 100, 300, 0.12);
    assert_eq!(y.len(), x.len());
    assert_eq!(y[..100], x[..100]);
    for n in 300..1000 {
        assert!((y[n] - 0.12 * x[n]).abs() < 1e-6, "sample {n}");
    }
    assert!((y[200] - 0.56 * x[200]).abs() < 0.01 * x[200].abs() + 1e-6, "{} {}", y[200], x[200]);
    assert!(x.iter().zip(&y).all(|(a, b)| (*a < 0.0) == (*b < 0.0)));
    assert_eq!(fade(&x, 2000, 3000, 0.1), x);
}

#[test]
fn dropout_takes_its_stretch_down_and_nothing_else() {
    let x = noise(1000, 0.5, 4);
    let y = dropout(&x, 100, 50, 0.06);
    assert_eq!(y[..100], x[..100]);
    assert_eq!(y[150..], x[150..]);
    assert!((100..150).all(|n| (y[n] - 0.06 * x[n]).abs() < 1e-7));
    let end = dropout(&x, 990, 50, 0.0);
    assert_eq!(end.len(), 1000);
    assert!(end[990..].iter().all(|&v| v == 0.0) && end[..990] == x[..990]);
}

#[test]
fn dc_ramp_adds_a_straight_line() {
    let y = dc_ramp(&[0.0; 101], -0.2, 0.2);
    assert!((y[0] + 0.2).abs() < 1e-6 && y[50].abs() < 1e-6 && (y[100] - 0.2).abs() < 1e-6, "{y:?}");
    let x = noise(101, 0.5, 5);
    let z = dc_ramp(&x, -0.2, 0.2);
    assert!((0..101).all(|n| (z[n] - x[n] - y[n]).abs() < 1e-6));
}

#[test]
fn hum_adds_a_triangle_of_its_frequency_and_amplitude() {
    let y = hum(&vec![0.0; 44_100], 44_100, 50.0, 0.1);
    assert_eq!(y[0], 0.0);
    let (lo, hi) = y.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    // The peak falls between samples 220 and 221, at 0.0998 of 0.1.
    assert!((0.0997..=0.1 + 1e-6).contains(&hi) && (-0.1 - 1e-6..=-0.0997).contains(&lo), "{lo} {hi}");
    // A period is 882 samples, and fifty of them average out.
    assert!((0..44_100 - 882).all(|n| (y[n] - y[n + 882]).abs() < 1e-5));
    assert!(y.iter().map(|&v| f64::from(v)).sum::<f64>().abs() < 1e-3);
    assert!(y[220] > 0.099 && y[661] < -0.099, "{} {}", y[220], y[661]);
}

#[test]
fn hiss_lies_its_decibels_under_the_whole_signals_level() {
    let x = square(28, 2000);
    let added = |y: &[f32], x: &[f32]| y.iter().zip(x).map(|(a, b)| a - b).collect::<Vec<f32>>();
    let y = hiss(&x, 20, 7);
    let n = added(&y, &x);
    assert!((rms(&n) / 0.05 - 1.0).abs() < 0.03, "deviation {}", rms(&n));
    assert!(n.iter().map(|&v| f64::from(v)).sum::<f64>().abs() / (n.len() as f64) < 0.002);
    assert_eq!(hiss(&x, 20, 7), y);
    assert_ne!(hiss(&x, 20, 8), y);
    // The level is the whole signal's, silence included: half of it silent is 3 dB down.
    let mut half = x.clone();
    half.extend(vec![0.0f32; x.len()]);
    let n = added(&hiss(&half, 20, 7), &half);
    assert!((rms(&n) / (0.05 / 2f64.sqrt()) - 1.0).abs() < 0.03, "deviation {}", rms(&n));
    let n = added(&hiss(&x, 0, 7), &x);
    assert!((rms(&n) / 0.5 - 1.0).abs() < 0.03, "deviation {}", rms(&n));
}

#[test]
fn below_db_is_ten_to_the_minus_db_over_twenty() {
    assert_eq!(below_db(0), 1.0);
    assert!((below_db(20) - 0.1).abs() < 1e-12);
    assert!((below_db(6) - 0.501_187_233_627).abs() < 1e-9);
}

#[test]
fn clip_flattens_the_tops_and_moves_no_crossing() {
    let x: Vec<f32> = (0..1000).map(|n| 0.5 * tri(n as f64 / 250.0) as f32).collect();
    let y = clip(&x, 4.0);
    assert!(y.iter().all(|v| v.abs() <= 1.0));
    assert_eq!(y.iter().copied().fold(f32::MIN, f32::max), 1.0);
    assert!(x.iter().zip(&y).all(|(a, b)| (*a < 0.0) == (*b < 0.0) && (*a == 0.0) == (*b == 0.0)));
    assert!(x.iter().zip(&y).filter(|(a, _)| a.abs() <= 0.25).all(|(a, b)| (4.0 * a - b).abs() < 1e-6));
}

#[test]
fn speed_stretches_every_half_wave_alike() {
    let x = square(100, 20);
    let y = speed(&x, 1.1);
    assert_eq!(y.len(), 2199);
    let g = gaps(&y);
    assert_eq!(g.len(), 18);
    assert!(g.iter().all(|&d| (d - 110.0).abs() < 0.5), "{g:?}");
    let y = speed(&x, 0.8);
    assert!(gaps(&y).iter().all(|&d| (d - 80.0).abs() < 0.5));
}

#[test]
fn wow_swings_the_half_waves_with_its_triangle() {
    // A period of 8,000 output samples at ±5%: the half-waves are longest a quarter of the way
    // in, shortest at three quarters.
    let x = square(100, 400);
    let y = wow(&x, 44_100, 0.05, 44_100.0 / 8000.0);
    let c = crossings(&y);
    let at = |t: f64| {
        let k = c.windows(2).position(|w| w[0] <= t && t < w[1]).unwrap();
        c[k + 1] - c[k]
    };
    assert!(at(2000.0) > 103.0 && at(2000.0) < 106.0, "{}", at(2000.0));
    assert!(at(6000.0) > 94.0 && at(6000.0) < 97.0, "{}", at(6000.0));
    assert!(at(10_000.0) > 103.0 && (at(2000.0) - at(10_000.0)).abs() < 1.0);
    assert!(gaps(&y).iter().all(|&d| d > 94.0 && d < 106.0));
}

#[test]
fn a_snag_stretches_its_stretch_of_tape_and_nothing_else() {
    // Half-waves of 100 samples: from input sample 4,000 stretched over 2,000 samples up to 1.5
    // times as long, held for 2,000 and let go over 1,000.
    let x = square(100, 120);
    let y = snag(&x, 4000, 2000, 2000, 1000, 1.5);
    let c = crossings(&y);
    let at = |t: f64| {
        let k = c.windows(2).position(|w| w[0] <= t && t < w[1]).unwrap();
        c[k + 1] - c[k]
    };
    assert!((at(2000.0) - 100.0).abs() < 0.5, "{}", at(2000.0));
    // The hold starts at output sample 4,000 + 2,000 · 1.5 / ln 1.5 ≈ 6,466 and lasts 3,000.
    assert!((at(8000.0) - 150.0).abs() < 0.5, "{}", at(8000.0));
    assert!((at(y.len() as f64 - 300.0) - 100.0).abs() < 0.5);
    assert!(gaps(&y).iter().all(|&d| d > 99.5 && d < 150.5));
    assert_eq!(snag(&x, 4000, 2000, 2000, 1000, 1.0), x);
}

#[test]
fn invert_turns_the_signal_upside_down() {
    let x = noise(500, 0.7, 6);
    assert!(invert(&x).iter().zip(&x).all(|(a, b)| *a == -*b));
}

#[test]
fn spike_replaces_its_samples_on_the_other_side_of_zero() {
    let mut x = vec![0.5f32; 100];
    x.extend(vec![-0.5f32; 100]);
    let y = spike(&x, 40, 3, 0.8);
    assert_eq!(y[40..43], [-0.8, -0.8, -0.8]);
    assert_eq!(y[..40], x[..40]);
    assert_eq!(y[43..], x[43..]);
    assert_eq!(spike(&x, 150, 2, 0.8)[150..152], [0.8, 0.8]);
    assert_eq!(spike(&[0.0; 10], 4, 1, 0.3)[4], 0.3);
    let end = spike(&x, 199, 5, 0.8);
    assert_eq!(end.len(), 200);
    assert_eq!(end[199], 0.8);
}

#[test]
fn clicks_put_one_spike_in_every_stretch_where_the_seed_says() {
    let x = vec![0.5f32; 10_500];
    let y = clicks(&x, 1000, 3, 0.8, 9);
    let mut rng = Rng::new(9);
    let mut want = x.clone();
    for k in 0..10 {
        let at = k * 1000 + (rng.unit() * 998.0).floor() as usize;
        want[at..at + 3].fill(-0.8);
    }
    assert_eq!(y, want);
}

#[test]
fn head_skew_delays_the_right_track() {
    let x = noise(300, 0.5, 7);
    let (l, r) = head_skew(&x, 3.0);
    assert_eq!(l, x);
    assert_eq!(r.len(), x.len());
    assert!(r[..3].iter().all(|&v| v == x[0]));
    assert!((3..300).all(|n| (r[n] - x[n - 3]).abs() < 1e-7));
    let (_, r) = head_skew(&x, 0.5);
    assert!((1..300).all(|n| (r[n] - 0.5 * (x[n] + x[n - 1])).abs() < 1e-6));
}

#[test]
fn one_track_silences_its_stretch() {
    let x = noise(300, 0.5, 8);
    let y = one_track(&x, 100, 200);
    assert!(y[100..200].iter().all(|&v| v == 0.0));
    assert!(y[..100] == x[..100] && y[200..] == x[200..]);
    assert_eq!(one_track(&x, 250, 400)[250..], [0.0; 50]);
}

#[test]
fn downsample_takes_the_mean_over_each_output_samples_time() {
    let flat = downsample(&vec![0.3f32; 1000], 44_100, 8000);
    assert_eq!(flat.len(), 181);
    assert!(flat.iter().all(|v| (v - 0.3).abs() < 1e-6));
    // A step at input sample 1000: output sample 181 covers 997.76 to 1003.28, 59.4% of it high.
    let step: Vec<f32> = (0..2000).map(|n| if n < 1000 { -0.5 } else { 0.5 }).collect();
    let y = downsample(&step, 44_100, 8000);
    assert!((y[180] + 0.5).abs() < 1e-6 && (y[182] - 0.5).abs() < 1e-6);
    assert!((y[181] - 0.094_104).abs() < 1e-4, "{}", y[181]);
    let x = noise(1000, 0.5, 9);
    assert_eq!(downsample(&x, 44_100, 44_100), x);
    let half = downsample(&x, 44_100, 22_050);
    assert!((0..500).all(|n| (half[n] - 0.5 * (x[2 * n] + x[2 * n + 1])).abs() < 1e-6));
}

#[test]
fn quantise_lands_on_the_levels_a_wav_file_holds() {
    let x = noise(2000, 1.2, 10);
    for bits in [8u16, 16] {
        let y = quantise(&x, bits);
        assert_eq!(encode_wav(&y, 8000, bits), encode_wav(&x, 8000, bits), "{bits} bits");
        assert_eq!(quantise(&y, bits), y, "{bits} bits");
    }
    let y = quantise(&x, 8);
    assert!(y.iter().all(|&v| {
        let k = (f64::from(v) + 1.0) * 127.5;
        (k - k.round()).abs() < 1e-4
    }));
}

#[test]
fn ripple_moves_the_tops_and_never_the_sign() {
    let x = square(28, 200);
    let y = ripple(&x, 44_100, 0.9, 2400.0);
    let ratio: Vec<f32> = x.iter().zip(&y).map(|(a, b)| b / a).collect();
    assert!(ratio.iter().all(|&r| r > 0.1 - 1e-6 && r < 1.9 + 1e-6));
    assert!(ratio.iter().copied().fold(f32::MAX, f32::min) < 0.15);
    assert!(ratio.iter().copied().fold(f32::MIN, f32::max) > 1.85);
}

#[test]
fn stereo_wav_interleaves_the_channels() {
    let wav = stereo_wav(&[0.5, -1.0], &[0.0, 1.0], 8000, 16);
    assert_eq!(wav.len(), 44 + 8);
    assert_eq!(wav[44..], [0x00, 0x40, 0x00, 0x00, 0x01, 0x80, 0xFF, 0x7F]);
}

#[test]
fn a_sharp_resampler_passes_a_level_and_a_low_tone_and_rings_at_an_edge() {
    // A level stays where it is, away from the ends.
    let level = resample_sharp(&vec![0.25; 4410], 44_100, 8000);
    assert_eq!(level.len(), 800);
    assert!(level[100..700].iter().all(|&v| (v - 0.25).abs() < 1e-6));
    // A square wave of 21 samples a half, 1,050 Hz, keeps its crossings within a sample at
    // 8 kHz, and its top rings past the level.
    let sq = square(21, 40);
    let y = resample_sharp(&sq, 44_100, 8000);
    let want: Vec<f64> = gaps(&sq).iter().map(|g| g * 8000.0 / 44_100.0).collect();
    let got = gaps(&y);
    assert_eq!(got.len(), want.len());
    assert!(got.iter().zip(&want).skip(2).all(|(a, b)| (a - b).abs() < 1.0), "{got:?}");
    assert!(y.iter().any(|&v| v > 0.52));
    assert_eq!(resample_sharp(&sq, 44_100, 8000), y);
}
