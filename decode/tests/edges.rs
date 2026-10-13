// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Sampled audio through the edge detector: tapes rendered to WAV by `tapeti-core`, worn on
//! purpose, decoded back.

use std::sync::atomic::AtomicBool;
use tapeti_core::audio::{encode_wav, playback_order, render_tape, FlowOptions, RenderOptions};
use tapeti_core::types::{Block, Body};
use tapeti_decode::entry::{decode_file, tape_band, Decoded};
use tapeti_decode::readers::Channel;
use tapeti_decode::waveform::edges::{detect, pulses};
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
    decode_file("t.wav", wav, Channel::Mix, filter, None, &no_zlib, &mut |_| {}, &AtomicBool::new(false))
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

/// Checks that every block of `tape()` comes back from `samples` at `rate`.
fn reads(samples: &[f32], rate: u32, bits: u16, filter: Filter) {
    let got = decode_wav(&encode_wav(samples, rate, bits), filter);
    assert_eq!(datas(&got.blocks), datas(&tape()), "{:?}", got.findings);
}

#[test]
fn the_spectrums_output_decodes_to_the_same_blocks() {
    for rate in [22050, 44100, 48000] {
        let got = decode_wav(&encode_wav(&render(&tape(), rate, true), rate, 16), Filter::None);
        // The render opens and closes with half a second of silence.
        let mut want = vec![Block::new(Body::Pause { pause: 500 })];
        want.extend(tape());
        if let Some(Body::Standard { pause, .. }) = want.last_mut().map(|b| &mut b.body) {
            *pause += 500;
        }
        assert_eq!(got.blocks, want, "{rate} Hz");
        assert_eq!(got.rate_hz, rate);
    }
}

#[test]
fn a_square_wave_holding_its_pause_level_costs_the_first_pilot_pulses() {
    // A plain square wave holds a pause at the low level for a second, which no tape carries,
    // and the DC blocker drains it to zero, so the first pilot halves after it rise from zero
    // and their low halves do not cross back for a few ms. That leaves a few pulses in front of
    // each pilot, kept as raw signal: every byte reads, and the pause comes back as a pause and
    // a short CSW block.
    let got = decode_wav(&encode_wav(&render(&tape(), 44100, false), 44100, 16), Filter::None);
    assert_eq!(datas(&got.blocks), datas(&tape()));
    assert!(got.blocks.iter().all(|b| matches!(b.body, Body::Standard { .. } | Body::Csw { .. })));
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
    assert_eq!(datas(&got.blocks), [datas(&tape()), datas(&tape()), datas(&tape())].concat());
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
