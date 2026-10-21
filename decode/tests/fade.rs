// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! A dropout inside a block: the ordinary band, a share of the loudest sample within 10 ms, is
//! held up by the signal either side and cuts no edge across it, and the block ends there; the
//! fade band follows the level down, and the block a header asks for is read again on it.

mod common;

use common::Rng;
use std::sync::atomic::AtomicBool;
use tapeti_core::audio::{encode_wav, playback_order, render_tape, FlowOptions, RenderOptions};
use tapeti_core::types::{Block, Body};
use tapeti_decode::entry::{decode_file, decode_pulses, Channels, Decoded, Extent, Finding};
use tapeti_decode::fold::{fold, Source};
use tapeti_decode::readers::Channel;
use tapeti_decode::waveform::edges::{detect_pulses_band, Axis, Band};
use tapeti_decode::waveform::filter::Filter;

const RATE: u32 = 44_100;
/// The bytes the header asks for, flag and checksum aside: 1.76 s of bits.
const ASKED: usize = 300;

fn with_checksum(mut data: Vec<u8>) -> Vec<u8> {
    data.push(data.iter().fold(0, |x, &d| x ^ d));
    data
}

fn render(blocks: &[Block]) -> Vec<f32> {
    let order = playback_order(blocks, FlowOptions::default());
    render_tape(blocks, RenderOptions { sample_rate: RATE, mic: true, amplitude: 0.5 }, &order)
}

/// Gaussian, the sum of twelve uniforms less six.
fn gauss(rng: &mut Rng) -> f32 {
    ((0..12).map(|_| rng.unit()).sum::<f64>() - 6.0) as f32
}

/// A program whose header asks for [`ASKED`] bytes, behind 2 s of silence, its data block
/// faded for 12.4 ms to `depth` of its level 0.8 s into its bits, under hiss of 0.0008.
fn faded(depth: f32) -> (Vec<f32>, Vec<u8>) {
    let mut header = vec![0x00, 0x03];
    header.extend(b"fade      ");
    header.extend((ASKED as u16).to_le_bytes());
    header.extend([0x00, 0x80, 0x00, 0x80]);
    let mut data = vec![0xFF];
    data.extend((0..ASKED).map(|i| (i as u8).wrapping_mul(37).wrapping_add(11)));
    let (header, data) = (with_checksum(header), with_checksum(data));
    let head = vec![Block::new(Body::Standard { pause: 1000, data: header.clone() })];
    let data_at = render(&head).len();
    let tape = render(&[head[0].clone(), Block::new(Body::Standard { pause: 2000, data: data.clone() })]);
    let lead = 2 * RATE as usize;
    let mut x = vec![0.0f32; lead];
    x.extend(tape);
    // The data block's pilot is 3,223 half-waves of 2,168 T, 2.0 s; the dropout is 0.8 s behind.
    let from = lead + data_at + (2.8 * f64::from(RATE)) as usize;
    for s in &mut x[from..from + (0.0124 * f64::from(RATE)) as usize] {
        *s *= depth;
    }
    let mut rng = Rng::new(0xFADE);
    for s in &mut x {
        *s += 0.0008 * gauss(&mut rng);
    }
    (x, data)
}

fn decode(x: &[f32]) -> Decoded {
    let no_zlib = |_: &[u8]| -> Result<Vec<u8>, String> { Err("none".into()) };
    let wav = encode_wav(x, RATE, 16);
    let mix = Channels::Only(Channel::Mix);
    decode_file("t.wav", &wav, mix, Filter::None, None, &no_zlib, &mut |_| {}, &AtomicBool::new(false))
        .unwrap()
}

fn datas(d: &Decoded) -> Vec<Vec<u8>> {
    d.blocks
        .iter()
        .filter_map(|b| match &b.body {
            Body::Standard { data, .. } => Some(data.clone()),
            _ => None,
        })
        .collect()
}

/// What the ordinary band alone reads of `x`.
fn ordinary(x: &[f32]) -> Vec<Vec<u8>> {
    let pulses = detect_pulses_band(|| x.iter().copied(), RATE, Filter::None, Axis::Zero, Band::Max).unwrap();
    datas(&decode_pulses(&pulses, RATE, &mut |_| {}, &AtomicBool::new(false)).unwrap())
}

#[test]
fn a_block_a_dropout_ended_is_read_again_on_the_fade_band() {
    let (x, data) = faded(1.0);
    let got = decode(&x);
    assert!(datas(&got).contains(&data), "test construction: it reads without the dropout");
    assert!(!got.findings.iter().any(|f| matches!(f, Finding::FadeBand { .. })));
    let (x, data) = faded(0.06);
    assert!(!ordinary(&x).contains(&data), "test construction: the dropout ends the ordinary read");
    let got = decode(&x);
    assert!(datas(&got).contains(&data), "{:?}", got.findings);
    assert!(got.findings.contains(&Finding::FadeBand { block: 1 }), "{:?}", got.findings);
}

/// 800 half-sine lobes of a ROM pilot at 0.5, behind and before silence, with uniform noise of
/// `noise` on them.
fn pilot(noise: f32) -> Vec<f32> {
    let half = 2168.0 * f64::from(RATE) / 3_500_000.0;
    let mut rng = Rng::new(0xFADE + 1);
    let mut x = vec![0.0f32; 2205];
    for i in 0..(800.0 * half) as usize {
        let phase = i as f64 / half;
        let a = if (phase.floor() as usize).is_multiple_of(2) { 0.5 } else { -0.5 };
        x.push(
            (a * (std::f64::consts::PI * phase.fract()).sin()) as f32
                + noise * (2.0 * rng.unit() as f32 - 1.0),
        );
    }
    x.extend(vec![0.0f32; 2205]);
    x
}

/// How many edges the fade band cuts in `x`: one fewer than its pulses, which run from the start
/// to the end.
fn fade_edges(x: &[f32]) -> usize {
    detect_pulses_band(|| x.iter().copied(), RATE, Filter::None, Axis::Zero, Band::Fade).unwrap().len() - 1
}

#[test]
fn the_fade_block_holds_a_lobes_peak() {
    // A block under half a pilot half-wave lets the median sink to the lobes' flanks, and the
    // noise on the lobes cuts edges; at half a millisecond each block holds a lobe's peak.
    assert_eq!(fade_edges(&pilot(0.1)), 800);
}

#[test]
fn the_fade_floor_lies_over_a_blocks_residue() {
    // The DC blocker leaves a residue behind the pilot's last lobe that a floor at a quarter of
    // the ordinary one cuts into an edge; at half it is none.
    assert_eq!(fade_edges(&pilot(0.0)), 800);
}

#[test]
fn a_fade_band_read_gives_way_to_another_channels_ordinary_read() {
    let block = |data: &[u8]| Block::new(Body::Standard { pause: 1000, data: data.to_vec() });
    let decoded = |data: &[u8], findings: Vec<Finding>| Decoded {
        blocks: vec![block(data)],
        extents: vec![Extent { start: 0.0, end: 100.0, signal_end: 100.0 }],
        findings,
        rate_hz: RATE,
        collapsed_ends: [0; 2],
    };
    // Two bytes wrong in the same bit: the checksum holds.
    let right = with_checksum(vec![0xFF, 0x20, 0x20, 0x41]);
    let wrong = with_checksum(vec![0xFF, 0x60, 0x60, 0x41]);
    let mut base = decoded(&wrong, vec![Finding::FadeBand { block: 0 }]);
    fold(&mut base, &[(Source::Channel(Channel::Left), &decoded(&right, Vec::new()))]);
    assert_eq!(datas(&base), vec![right.clone()]);
    assert_eq!(base.findings, [Finding::FromChannel { block: 0, channel: Channel::Left }]);
    // Not to another fade band read, nor where the block is the ordinary band's own.
    let mut base = decoded(&wrong, vec![Finding::FadeBand { block: 0 }]);
    fold(
        &mut base,
        &[(Source::Channel(Channel::Left), &decoded(&right, vec![Finding::FadeBand { block: 0 }]))],
    );
    assert_eq!(datas(&base), vec![wrong.clone()]);
    let mut base = decoded(&wrong, Vec::new());
    fold(&mut base, &[(Source::Channel(Channel::Left), &decoded(&right, Vec::new()))]);
    assert_eq!(datas(&base), [wrong]);
}
