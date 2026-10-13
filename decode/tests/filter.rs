// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The band-pass filters against their closed forms. With `Ω` the pre-warped frequency
//! `2 fs tan(π f / fs)` and `x = (Ω² - ω0²) / (B Ω)`, a Butterworth band-pass of order `n` has
//! the gain `1 / √(1 + x^2n)` and a Chebyshev one `1 / √(1 + ε² T_n(x)²)`, where `T_n` is the
//! Chebyshev polynomial and `ε² = 10^(ripple/10) - 1`.

use std::f64::consts::PI;
use tapeti_decode::waveform::filter::{Filter, MAX_ORDER};

const RATES: [u32; 4] = [11025, 22050, 44100, 48000];
const BANDS: [(f64, f64); 3] = [(600.0, 4100.0), (300.0, 3000.0), (1000.0, 1300.0)];

fn chebyshev_polynomial(n: u8, x: f64) -> f64 {
    let n = f64::from(n);
    if x.abs() <= 1.0 {
        (n * x.acos()).cos()
    } else {
        (n * x.abs().acosh()).cosh()
    }
}

/// The closed-form gain of `filter` at `hz` for a recording at `rate`.
fn closed_form(filter: Filter, rate: u32, hz: f64) -> f64 {
    let fs = f64::from(rate);
    let warp = |f: f64| 2.0 * fs * (PI * f / fs).tan();
    let (order, low, high, ripple) = match filter {
        Filter::Butterworth { order, low_hz, high_hz } => (order, low_hz, high_hz, None),
        Filter::Chebyshev { order, low_hz, high_hz, ripple_db } => (order, low_hz, high_hz, Some(ripple_db)),
        Filter::None => return 1.0,
    };
    let (lo, hi, w) = (warp(low), warp(high), warp(hz));
    let x = (w * w - lo * hi) / ((hi - lo) * w);
    match ripple {
        None => 1.0 / (1.0 + x.powi(2 * i32::from(order))).sqrt(),
        Some(r) => {
            let e2 = 10f64.powf(r / 10.0) - 1.0;
            1.0 / (1.0 + e2 * chebyshev_polynomial(order, x).powi(2)).sqrt()
        }
    }
}

fn filters() -> Vec<Filter> {
    let mut out = Vec::new();
    for order in 1..=MAX_ORDER {
        for (low_hz, high_hz) in BANDS {
            out.push(Filter::Butterworth { order, low_hz, high_hz });
            for ripple_db in [0.1, 0.5, 1.0, 3.0] {
                out.push(Filter::Chebyshev { order, low_hz, high_hz, ripple_db });
            }
        }
    }
    out
}

/// 400 frequencies from 20 Hz to just under half the rate, spaced evenly on a log scale.
fn frequencies(rate: u32) -> impl Iterator<Item = f64> {
    let top = f64::from(rate) / 2.0 * 0.995;
    (0..400).map(move |i| 20.0 * (top / 20.0).powf(f64::from(i) / 399.0))
}

#[test]
fn every_filter_has_the_textbook_response() {
    for filter in filters() {
        for rate in RATES {
            let cascade = filter.design(rate).unwrap();
            for hz in frequencies(rate) {
                let (got, want) = (cascade.response(hz), closed_form(filter, rate, hz));
                assert!(
                    (got - want).abs() <= 1e-9 * want.max(1e-6),
                    "{filter:?} at {rate} Hz: {got} at {hz} Hz, not {want}"
                );
            }
        }
    }
}

#[test]
fn a_butterworth_filter_is_3_db_down_at_its_edges_and_whole_at_its_centre() {
    for order in 1..=MAX_ORDER {
        for rate in RATES {
            for (low_hz, high_hz) in BANDS {
                let cascade = Filter::Butterworth { order, low_hz, high_hz }.design(rate).unwrap();
                for edge in [low_hz, high_hz] {
                    assert!(
                        (cascade.response(edge) - 0.5f64.sqrt()).abs() < 1e-9,
                        "order {order}, {edge} Hz"
                    );
                }
                // The centre is the geometric mean of the warped edges, warped back.
                let fs = f64::from(rate);
                let warp = |f: f64| 2.0 * fs * (PI * f / fs).tan();
                let centre = (warp(low_hz) * warp(high_hz)).sqrt().atan2(2.0 * fs) * fs / PI;
                assert!((cascade.response(centre) - 1.0).abs() < 1e-9, "order {order} at its centre");
            }
        }
    }
}

#[test]
fn a_butterworth_band_falls_away_from_its_centre_on_both_sides() {
    for order in 1..=MAX_ORDER {
        let cascade = Filter::Butterworth { order, low_hz: 600.0, high_hz: 4100.0 }.design(44100).unwrap();
        let gains: Vec<f64> = frequencies(44100).map(|hz| cascade.response(hz)).collect();
        let peak = gains.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        // The top is flat to within rounding, so a step may go down by 1e-15 there.
        let rounding = 1e-12;
        assert!(
            gains[..=peak].windows(2).all(|w| w[0] <= w[1] + rounding),
            "order {order} rises to its peak"
        );
        assert!(gains[peak..].windows(2).all(|w| w[0] + rounding >= w[1]), "order {order} falls after it");
        assert!(gains[peak] <= 1.0 + 1e-12);
    }
}

#[test]
fn a_chebyshev_filter_ripples_within_its_band_and_is_down_by_the_ripple_at_its_edges() {
    for order in 1..=MAX_ORDER {
        for ripple_db in [0.1, 0.5, 1.0, 3.0] {
            let floor = 10f64.powf(-ripple_db / 20.0);
            let cascade =
                Filter::Chebyshev { order, low_hz: 600.0, high_hz: 4100.0, ripple_db }.design(44100).unwrap();
            for edge in [600.0, 4100.0] {
                assert!(
                    (cascade.response(edge) - floor).abs() < 1e-9,
                    "order {order}, {ripple_db} dB, {edge} Hz"
                );
            }
            for i in 0..=200 {
                let hz = 600.0 * (4100.0f64 / 600.0).powf(f64::from(i) / 200.0);
                let gain = cascade.response(hz);
                assert!(
                    gain >= floor - 1e-9 && gain <= 1.0 + 1e-9,
                    "order {order}, {ripple_db} dB: {gain} at {hz} Hz"
                );
            }
        }
    }
}

#[test]
fn every_pole_is_inside_the_unit_circle() {
    for filter in filters() {
        for rate in RATES {
            let cascade = filter.design(rate).unwrap();
            assert_eq!(
                cascade.sections().len(),
                usize::from(match filter {
                    Filter::Butterworth { order, .. } | Filter::Chebyshev { order, .. } => order,
                    Filter::None => 0,
                })
            );
            for s in cascade.sections() {
                // The roots of z² + a1 z + a2.
                let disc = s.a1 * s.a1 - 4.0 * s.a2;
                let largest = if disc < 0.0 { s.a2.sqrt() } else { (s.a1.abs() + disc.sqrt()) / 2.0 };
                assert!(largest < 1.0, "{filter:?} at {rate} Hz has a pole at radius {largest}");
            }
        }
    }
}

/// The amplitude of a sine at `hz` after `cascade`, once it has settled.
fn passed(filter: Filter, rate: u32, hz: f64) -> f64 {
    let cascade = filter.design(rate).unwrap();
    let n = rate as usize; // one second
    let sine = (0..n).map(|i| (2.0 * PI * hz * i as f64 / f64::from(rate)).sin() as f32);
    let out: Vec<f32> = cascade.apply(sine).collect();
    let tail = &out[n / 2..];
    (2.0 * tail.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / tail.len() as f64).sqrt()
}

#[test]
fn a_sine_comes_through_as_the_response_says() {
    let filter = Filter::Butterworth { order: 4, low_hz: 600.0, high_hz: 4100.0 };
    let cascade = filter.design(44100).unwrap();
    for hz in [100.0, 600.0, 1500.0, 4100.0, 10000.0] {
        let (got, want) = (passed(filter, 44100, hz), cascade.response(hz));
        assert!((got - want).abs() < 2e-3, "{hz} Hz: amplitude {got}, response {want}");
    }
}

#[test]
fn no_filter_passes_every_sample_unchanged() {
    let samples = [0.25f32, -1.0, 0.5, 0.0];
    let cascade = Filter::None.design(44100).unwrap();
    assert!(cascade.sections().is_empty());
    assert_eq!(cascade.response(1234.0), 1.0);
    assert_eq!(cascade.apply(samples).collect::<Vec<_>>(), samples);
}

#[test]
fn a_filter_that_cannot_be_built_says_why() {
    let cases = [
        (
            Filter::Butterworth { order: 0, low_hz: 600.0, high_hz: 4100.0 },
            "the order is 0; it must be 1 to 10",
        ),
        (
            Filter::Butterworth { order: 11, low_hz: 600.0, high_hz: 4100.0 },
            "the order is 11; it must be 1 to 10",
        ),
        (
            Filter::Butterworth { order: 2, low_hz: 4100.0, high_hz: 600.0 },
            "the band 4100-600 Hz must lie between 0 and 11025 Hz, half the rate",
        ),
        (
            Filter::Butterworth { order: 2, low_hz: 600.0, high_hz: 11025.0 },
            "the band 600-11025 Hz must lie between 0 and 11025 Hz, half the rate",
        ),
        (
            Filter::Butterworth { order: 2, low_hz: 0.0, high_hz: 4100.0 },
            "the band 0-4100 Hz must lie between 0 and 11025 Hz, half the rate",
        ),
        (
            Filter::Chebyshev { order: 2, low_hz: 600.0, high_hz: 4100.0, ripple_db: 0.0 },
            "the ripple must be more than 0 dB",
        ),
    ];
    for (filter, message) in cases {
        assert_eq!(filter.design(22050).unwrap_err().to_string(), message);
    }
}
