// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Band-pass filters for a recording: Butterworth or Chebyshev (type I), designed the textbook
//! way.
//!
//! The design starts from the analog low-pass prototype's poles, moves them to a band-pass with
//! `s -> (s² + ω0²) / (B s)`, and maps them to digital poles with the bilinear transform. The
//! band edges are pre-warped (`2 fs tan(π f / fs)`), so the digital filter's response at a
//! frequency is exactly the analog one at the warped frequency, and the edges land where asked.
//! Each pair of poles becomes a second-order section with zeros at 0 Hz and at half the rate.

use std::f64::consts::PI;
use std::fmt;

/// The most prototype order a filter may have. A band-pass filter has twice as many poles.
pub const MAX_ORDER: u8 = 10;

/// A band-pass filter, or none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Filter {
    #[default]
    None,
    /// Maximally flat in the band, 3 dB down at its edges.
    Butterworth { order: u8, low_hz: f64, high_hz: f64 },
    /// Ripples by `ripple_db` in the band and falls off faster outside it; `ripple_db` down at
    /// its edges.
    Chebyshev { order: u8, low_hz: f64, high_hz: f64, ripple_db: f64 },
}

/// A filter that cannot be designed at a given sample rate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterError(pub String);

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FilterError {}

impl Filter {
    /// The filter for a recording at `rate_hz`. [`Filter::None`] gives a cascade with no
    /// sections, which passes samples through unchanged.
    pub fn design(&self, rate_hz: u32) -> Result<Cascade, FilterError> {
        let (order, low, high, ripple_db) = match *self {
            Filter::None => return Ok(Cascade::new(Vec::new(), f64::from(rate_hz))),
            Filter::Butterworth { order, low_hz, high_hz } => (order, low_hz, high_hz, None),
            Filter::Chebyshev { order, low_hz, high_hz, ripple_db } => {
                (order, low_hz, high_hz, Some(ripple_db))
            }
        };
        let fs = f64::from(rate_hz);
        let bad = |what: String| Err(FilterError(what));
        if !(1..=MAX_ORDER).contains(&order) {
            return bad(format!("the order is {order}; it must be 1 to {MAX_ORDER}"));
        }
        if !(low > 0.0 && low < high && high < fs / 2.0) {
            return bad(format!(
                "the band {low}-{high} Hz must lie between 0 and {} Hz, half the rate",
                fs / 2.0
            ));
        }
        if ripple_db.is_some_and(|r| !(r > 0.0 && r.is_finite())) {
            return bad("the ripple must be more than 0 dB".to_string());
        }

        let warp = |f: f64| 2.0 * fs * (PI * f / fs).tan();
        let (lo, hi) = (warp(low), warp(high));
        let w0 = (lo * hi).sqrt();
        let width = hi - lo;
        let n = usize::from(order);
        // Chebyshev poles lie on an ellipse that `mu` sets; ε is the ripple's depth.
        let epsilon = ripple_db.map(|r| (10f64.powf(r / 10.0) - 1.0).sqrt());
        let mu = epsilon.map(|e| (1.0 / e).asinh() / n as f64);
        let prototype = |k: usize| {
            let theta = PI * (2 * k + 1) as f64 / (2 * n) as f64;
            match mu {
                None => Complex::new(-theta.sin(), theta.cos()),
                Some(mu) => Complex::new(-mu.sinh() * theta.sin(), mu.cosh() * theta.cos()),
            }
        };
        let to_band = |p: Complex| {
            let half = p.scale(width / 2.0);
            let root = (half * half - Complex::new(w0 * w0, 0.0)).sqrt();
            (half + root, half - root)
        };
        let bilinear = |s: Complex| (Complex::new(2.0 * fs, 0.0) + s) / (Complex::new(2.0 * fs, 0.0) - s);

        // The prototype's poles above the real axis each give two digital poles, whose mirror
        // images come from the pole below; a real pole (odd order) gives one pair.
        let mut denominators = Vec::with_capacity(n);
        for k in 0..n / 2 {
            let (s1, s2) = to_band(prototype(k));
            for z in [bilinear(s1), bilinear(s2)] {
                denominators.push([-2.0 * z.re, z.re * z.re + z.im * z.im]);
            }
        }
        if n % 2 == 1 {
            let (s1, s2) = to_band(prototype(n / 2));
            let (z1, z2) = (bilinear(s1), bilinear(s2));
            denominators.push([-(z1 + z2).re, (z1 * z2).re]);
        }

        // The gain at the band's centre is the prototype's at 0 Hz: 1, or 1/√(1+ε²) for a
        // Chebyshev filter of even order. It is shared evenly between the sections.
        let unity: Vec<Biquad> =
            denominators.iter().map(|&[a1, a2]| Biquad { b: [1.0, 0.0, -1.0], a1, a2 }).collect();
        let centre = 2.0 * (w0 / (2.0 * fs)).atan() * fs / (2.0 * PI);
        let target = match epsilon {
            Some(e) if n % 2 == 0 => 1.0 / (1.0 + e * e).sqrt(),
            _ => 1.0,
        };
        let gain = (target / Cascade::new(unity.clone(), fs).response(centre)).powf(1.0 / n as f64);
        let sections = unity.into_iter().map(|s| Biquad { b: s.b.map(|b| b * gain), ..s }).collect();
        Ok(Cascade::new(sections, fs))
    }
}

/// One second-order section: `(b0 + b1 z⁻¹ + b2 z⁻²) / (1 + a1 z⁻¹ + a2 z⁻²)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biquad {
    pub b: [f64; 3],
    pub a1: f64,
    pub a2: f64,
}

/// A designed filter, ready to run: its sections in series, and their state.
#[derive(Clone, Debug)]
pub struct Cascade {
    sections: Vec<Biquad>,
    rate_hz: f64,
    state: Vec<[f64; 2]>,
}

impl Cascade {
    fn new(sections: Vec<Biquad>, rate_hz: f64) -> Self {
        let state = vec![[0.0; 2]; sections.len()];
        Cascade { sections, rate_hz, state }
    }

    pub fn sections(&self) -> &[Biquad] {
        &self.sections
    }

    /// The gain at `hz`, as a ratio (1.0 passes it unchanged).
    pub fn response(&self, hz: f64) -> f64 {
        let w = 2.0 * PI * hz / self.rate_hz;
        let (z1, z2) = (Complex::new(w.cos(), -w.sin()), Complex::new((2.0 * w).cos(), -(2.0 * w).sin()));
        self.sections
            .iter()
            .map(|s| {
                let num = Complex::new(s.b[0], 0.0) + z1.scale(s.b[1]) + z2.scale(s.b[2]);
                let den = Complex::new(1.0, 0.0) + z1.scale(s.a1) + z2.scale(s.a2);
                (num / den).abs()
            })
            .product()
    }

    /// The next output sample (transposed direct form II, in f64).
    pub fn process(&mut self, x: f32) -> f32 {
        let mut v = f64::from(x);
        for (s, st) in self.sections.iter().zip(&mut self.state) {
            let y = s.b[0] * v + st[0];
            st[0] = s.b[1] * v - s.a1 * y + st[1];
            st[1] = s.b[2] * v - s.a2 * y;
            v = y;
        }
        v as f32
    }

    /// The filtered samples, from a fresh state.
    pub fn apply<I: IntoIterator<Item = f32>>(mut self, samples: I) -> impl Iterator<Item = f32> {
        self.state.iter_mut().for_each(|st| *st = [0.0; 2]);
        samples.into_iter().map(move |x| self.process(x))
    }
}

#[derive(Clone, Copy, Debug)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    fn new(re: f64, im: f64) -> Self {
        Complex { re, im }
    }

    fn scale(self, k: f64) -> Self {
        Complex::new(self.re * k, self.im * k)
    }

    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// The principal square root.
    fn sqrt(self) -> Self {
        let r = self.abs();
        let re = ((r + self.re) / 2.0).sqrt();
        let im = ((r - self.re) / 2.0).sqrt();
        Complex::new(re, if self.im < 0.0 { -im } else { im })
    }
}

impl std::ops::Add for Complex {
    type Output = Complex;
    fn add(self, o: Complex) -> Complex {
        Complex::new(self.re + o.re, self.im + o.im)
    }
}

impl std::ops::Sub for Complex {
    type Output = Complex;
    fn sub(self, o: Complex) -> Complex {
        Complex::new(self.re - o.re, self.im - o.im)
    }
}

impl std::ops::Mul for Complex {
    type Output = Complex;
    fn mul(self, o: Complex) -> Complex {
        Complex::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
}

impl std::ops::Div for Complex {
    type Output = Complex;
    fn div(self, o: Complex) -> Complex {
        let d = o.re * o.re + o.im * o.im;
        Complex::new((self.re * o.re + self.im * o.im) / d, (self.im * o.re - self.re * o.im) / d)
    }
}
