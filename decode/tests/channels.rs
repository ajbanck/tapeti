// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The channel of a stereo recording, chosen by decoding each: the choice on constructed
//! decodes, then stereo recordings rendered by `tapeti-core`.

use std::sync::atomic::AtomicBool;
use tapeti_core::audio::{playback_order, render_tape, FlowOptions, RenderOptions};
use tapeti_core::types::{Block, Body};
use tapeti_decode::channels::{delivered, pick, shape};
use tapeti_decode::entry::{decode_file, Channels, Decoded, Finding};
use tapeti_decode::readers::Channel;
use tapeti_decode::waveform::filter::Filter;

fn standard(data: Vec<u8>) -> Block {
    Block::new(Body::Standard { pause: 1000, data })
}

fn checksummed(mut data: Vec<u8>) -> Vec<u8> {
    data.push(data.iter().fold(0, |x, &d| x ^ d));
    data
}

/// A header declaring `payload.len()` bytes and the data block that answers it.
fn program(name: &str, payload: &[u8]) -> Vec<Block> {
    let mut h = vec![0x00, 0x03];
    h.extend(format!("{name:<10}").as_bytes());
    h.extend((payload.len() as u16).to_le_bytes());
    h.extend([0x00, 0x80, 0x00, 0x80]);
    let mut d = vec![0xFF];
    d.extend(payload);
    vec![standard(checksummed(h)), standard(checksummed(d))]
}

/// A data block of `n` bytes that no header answers.
fn lone_block(n: usize) -> Block {
    let mut d = vec![0xFF];
    d.extend((0..n).map(|i| (i as u8).wrapping_mul(29).wrapping_add(5)));
    standard(checksummed(d))
}

/// Three bytes under flag 0x07 whose checksum happens to be good: a block to count, which proves
/// nothing.
fn junk() -> Block {
    standard(vec![0x07, 0x01, 0x06])
}

fn chosen(mix: &[Block], left: &[Block], right: &[Block]) -> usize {
    pick(&[mix, left, right])
}

#[test]
fn a_channel_wins_by_delivering_all_the_incumbent_does_and_more_verified_bytes() {
    let a = program("A", &[1, 2, 3, 4]);
    let b = program("B", &[5, 6, 7, 8]);
    let c = program("C", &(0..200u8).collect::<Vec<u8>>());
    let two: Vec<Block> = [a.clone(), b.clone()].concat();
    assert_eq!((delivered(&two), delivered(&c)), ((2, 12), (1, 202)));
    // Alike: the mix.
    assert_eq!(chosen(&a, &a, &a), 0);
    // More bytes, but not the mix's two programs: the mix is kept.
    assert_eq!(chosen(&two, &c, &[]), 0);
    // All the mix's and more: the left.
    assert_eq!(chosen(&two, &[two.clone(), c.clone()].concat(), &[]), 1);
    // The mix counts five chance blocks more, so its shape names it the incumbent. The left
    // adds a third program of three bytes, the right a hundred bytes no header answers: both
    // deliver all the mix does, and the right wins on bytes, a name being no loaded material.
    let counted = [two.clone(), vec![junk(); 5]].concat();
    let named = [two.clone(), program("D", &[9, 10, 11])].concat();
    let bytes = [two.clone(), vec![lone_block(99)]].concat();
    assert!(shape(&counted) > shape(&named));
    assert_eq!((delivered(&named), delivered(&bytes)), ((3, 17), (2, 113)));
    assert_eq!(chosen(&counted, &named, &bytes), 2);
}

#[test]
fn a_copy_and_chance_blocks_deliver_nothing() {
    let once = program("ONCE", &[1, 2, 3, 4, 5]);
    let twice = [once.clone(), once.clone()].concat();
    assert_eq!(delivered(&twice), delivered(&once));
    let junked = [once.clone(), vec![junk(); 10]].concat();
    assert_eq!(delivered(&junked), delivered(&once));
    // One byte over and over checks out at an even length and proves nothing.
    assert_eq!(delivered(&[standard(vec![0xFF, 0xFF, 0xFF, 0xFF])]), (0, 0));
}

fn tape() -> Vec<Block> {
    let payload: Vec<u8> = (0..300u32).map(|i| (i * 37 + 11) as u8).collect();
    [program("first", &payload), program("second", &payload[..200]), vec![lone_block(120)]].concat()
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

fn render(blocks: &[Block]) -> Vec<f32> {
    let order = playback_order(blocks, FlowOptions::default());
    render_tape(blocks, RenderOptions { sample_rate: 44100, mic: false, amplitude: 0.5 }, &order)
}

/// A 16-bit stereo WAV at 44.1 kHz of `left` and `right`.
fn stereo_wav(left: &[f32], right: &[f32]) -> Vec<u8> {
    let n = left.len().max(right.len());
    let at = |s: &[f32], i: usize| (s.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0) * 32767.0) as i16;
    let data: Vec<u8> = (0..n).flat_map(|i| [at(left, i), at(right, i)]).flat_map(i16::to_le_bytes).collect();
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + data.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(44100u32.to_le_bytes());
    wav.extend((44100u32 * 4).to_le_bytes());
    wav.extend(4u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((data.len() as u32).to_le_bytes());
    wav.extend(data);
    wav
}

fn decode(wav: &[u8], channels: Channels) -> Decoded {
    let no_zlib = |_: &[u8]| -> Result<Vec<u8>, String> { Err("none".into()) };
    decode_file("t.wav", wav, channels, Filter::None, None, &no_zlib, &mut |_| {}, &AtomicBool::new(false))
        .unwrap()
}

#[test]
fn the_best_channel_rescues_a_skewed_stereo_recording() {
    // The right head 11 samples behind the left: mixed, the two cancel at 44100 / 22 Hz, 2 kHz,
    // the pitch of a 0 bit's pair, and the data band is lost. Real heads have been measured
    // drifting from 2 to 7 samples apart; 11 is past that, so the loss is plain to see.
    let s = render(&tape());
    let late: Vec<f32> = std::iter::repeat_n(0.0, 11).chain(s.iter().copied()).collect();
    let wav = stereo_wav(&s, &late);
    let mixed = decode(&wav, Channels::Only(Channel::Mix));
    assert_ne!(datas(&mixed.blocks), datas(&tape()), "the mix still reads every block");

    let best = decode(&wav, Channels::Best);
    assert_eq!(datas(&best.blocks), datas(&tape()), "{:?}", best.findings);
    assert_eq!(best.findings[0], Finding::Channel { kept: Channel::Left });
}

#[test]
fn a_silent_channel_costs_nothing() {
    // The mix is the live channel at half the level, and every level the decoder reads by is
    // relative: all three decodes read the tape, and the tie goes to the mix.
    let s = render(&tape());
    let best = decode(&stereo_wav(&s, &vec![0.0; s.len()]), Channels::Best);
    assert_eq!(datas(&best.blocks), datas(&tape()));
    assert_eq!(best.findings[0], Finding::Channel { kept: Channel::Mix });
}

#[test]
fn a_mono_recording_is_decoded_once() {
    let s = render(&tape());
    let mono = tapeti_core::audio::encode_wav(&s, 44100, 16);
    let best = decode(&mono, Channels::Best);
    assert_eq!(datas(&best.blocks), datas(&tape()));
    assert!(!best.findings.iter().any(|f| matches!(f, Finding::Channel { .. })));
}
