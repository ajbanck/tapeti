// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Recordings kept as raw signal: the bits and blocks, and tapes rendered by `tapeti-core`, kept
//! and played back to the pulses they started as.

use tapeti_core::audio::{encode_wav, playback_order, render_tape, FlowOptions, RenderOptions};
use tapeti_core::types::{Block, Body};
use tapeti_core::writer::serialize_tzx;
use tapeti_decode::capture::{csw_of_tstates, direct_recording, spacing};
use tapeti_decode::entry::{describe, keep_as_raw_signal, tape_band, ImportError, Kept};
use tapeti_decode::pulses::of_blocks;
use tapeti_decode::readers::Channel;
use tapeti_decode::waveform::filter::Filter;

fn no_zlib(_: &[u8]) -> Result<Vec<u8>, String> {
    Err("no zlib here".into())
}

fn data_block() -> Block {
    Block::new(Body::Standard { pause: 1000, data: vec![0xFF, 0x12, 0x34, 0xAA, 0x55, 0x00, 0xFF] })
}

/// `blocks` as a mono WAV: a square wave at `rate`, 16 bits.
fn wav_of(blocks: &[Block], rate: u32) -> Vec<u8> {
    let order = playback_order(blocks, FlowOptions::default());
    let samples =
        render_tape(blocks, RenderOptions { sample_rate: rate, mic: false, amplitude: 0.8 }, &order);
    encode_wav(&samples, rate, 16)
}

/// The lengths of the pulses `blocks` play.
fn pulse_lengths(blocks: &[Block]) -> Vec<f64> {
    of_blocks(blocks).unwrap().iter().map(|p| p.tstates).collect()
}

/// Checks that `got` plays the pulses of `want` between the first and the pause, each within
/// one sample of the recording plus `slack` of its length. The first pulse joins the silence
/// that leads in, and the pause the silence after.
fn plays_back(got: &[f64], want: &[f64], rate: u32, slack: f64) {
    let inner = 1..want.len() - 2;
    assert_eq!(got.len(), want.len(), "the same number of pulses");
    let sample = 3_500_000.0 / f64::from(rate);
    for i in inner {
        let (g, w) = (got[i], want[i]);
        assert!((g - w).abs() <= sample + slack * w, "pulse {i}: {g} T-states, not {w}");
    }
}

#[test]
fn the_spacing_is_the_recordings_rate_in_t_states() {
    assert_eq!([44100, 22050, 48000, 96000].map(spacing), [Some(79), Some(159), Some(73), Some(36)]);
    assert_eq!(spacing(54), Some(64815));
    assert_eq!(spacing(53), None, "66,038 T-states does not fit the block's 16 bits");
}

#[test]
fn levels_become_bits_from_the_top_of_each_byte() {
    let levels = [1, 1, 1, 0, 0, 0, 1, 1, 1, 0].map(|b| b == 1);
    let blocks = direct_recording(79, levels, 100);
    assert_eq!(blocks.len(), 1);
    assert_eq!(
        blocks[0].body,
        Body::Direct { tstates: 79, pause: 0, used_bits: 2, data: vec![0b1110_0011, 0b1000_0000] }
    );
}

#[test]
fn a_signal_longer_than_a_block_continues_in_the_next_without_a_seam() {
    let levels: Vec<bool> = (0..40).map(|i| (i / 3) % 2 == 0).collect();
    let split = direct_recording(79, levels.iter().copied(), 2);
    let used: Vec<u8> = split
        .iter()
        .map(|b| match &b.body {
            Body::Direct { used_bits, data, pause, .. } => {
                assert_eq!(*pause, 0);
                assert!(data.len() <= 2);
                *used_bits
            }
            _ => panic!("not a direct recording"),
        })
        .collect();
    assert_eq!(used, [8, 8, 8], "16, 16 and 8 bits");
    let whole = direct_recording(79, levels.iter().copied(), 100);
    assert_eq!(pulse_lengths(&split), pulse_lengths(&whole));
}

#[test]
fn a_rendered_tape_kept_as_raw_signal_plays_back_its_pulses() {
    let tape = [data_block()];
    for rate in [22050, 44100, 48000] {
        let blocks =
            keep_as_raw_signal("t.wav", &wav_of(&tape, rate), Channel::Mix, Filter::None, &no_zlib).unwrap();
        // The rounded spacing plays the tape up to 0.7% fast or slow.
        plays_back(&pulse_lengths(&blocks), &pulse_lengths(&tape), rate, 0.007);
    }
}

#[test]
fn the_filter_keeps_the_signals_pulses_close_to_their_length() {
    let tape = [data_block()];
    let rate = 44100;
    let blocks =
        keep_as_raw_signal("t.wav", &wav_of(&tape, rate), Channel::Mix, tape_band(rate), &no_zlib).unwrap();
    let (got, want) = (pulse_lengths(&blocks), pulse_lengths(&tape));
    // The filter rings after each step; sliced at the average with no margin, the silence around
    // the block becomes slow crossings, one of which joins the first pilot pulse. The pulses
    // below are lined up on the first sync pulse: the first short one after a stretch of pilot.
    let sample = 3_500_000.0 / f64::from(rate);
    let pilot_like = |t: f64| (t - 2168.0).abs() <= sample + 0.03 * 2168.0;
    let pilot = (0..got.len()).find(|&i| got[i..i + 50].iter().all(|&t| pilot_like(t))).expect("a pilot");
    let sync1 = (pilot..got.len()).find(|&i| got[i] < 1200.0).expect("a sync pulse");
    let (offset, sync) = (sync1 - 3223, 3223..3225);
    for k in 100..=want.len() - 3 {
        let (g, w) = (got[offset + k], want[k]);
        // The filter shortens and lengthens the two short sync pulses the most.
        let slack = if sync.contains(&k) { 0.2 } else { 0.03 };
        assert!((g - w).abs() <= sample + slack * w, "pulse {k}: {g} T-states, not {w}");
    }
}

#[test]
fn a_stereo_recording_keeps_the_channel_asked_for() {
    let rate = 22050u32;
    let mono = wav_of(&[data_block()], rate);
    // The mono WAV's samples as the left channel, silence as the right.
    let samples = &mono[44..];
    let mut stereo = Vec::new();
    for s in samples.as_chunks::<2>().0 {
        stereo.extend_from_slice(s);
        stereo.extend_from_slice(&[0, 0]);
    }
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + stereo.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend([1, 0, 2, 0]);
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * 4).to_le_bytes());
    wav.extend([4, 0, 16, 0]);
    wav.extend(b"data");
    wav.extend((stereo.len() as u32).to_le_bytes());
    wav.extend(stereo);
    let left = keep_as_raw_signal("s.wav", &wav, Channel::Left, Filter::None, &no_zlib).unwrap();
    plays_back(&pulse_lengths(&left), &pulse_lengths(&[data_block()]), rate, 0.007);
    let right = keep_as_raw_signal("s.wav", &wav, Channel::Right, Filter::None, &no_zlib).unwrap();
    assert!(pulse_lengths(&right).len() <= 1, "the silent channel holds no pulses");
}

fn csw_v1(rate: u16, flags: u8, data: &[u8]) -> Vec<u8> {
    let mut out = b"Compressed Square Wave\x1a".to_vec();
    out.extend([1, 1]);
    out.extend(rate.to_le_bytes());
    out.extend([1, flags, 0, 0, 0]);
    out.extend(data);
    out
}

#[test]
fn a_csw_file_becomes_one_csw_block_as_it_is() {
    // At 35,000 Hz a sample is 100 T-states; a version 1 file's pulses are counted.
    let data = [3, 5, 0, 44, 1, 0, 0];
    let blocks =
        keep_as_raw_signal("t.csw", &csw_v1(35000, 1, &data), Channel::Mix, Filter::None, &no_zlib).unwrap();
    assert_eq!(blocks.len(), 1);
    let want =
        Body::Csw { pause: 0, sample_rate: 35000, compression: 1, pulse_count: 3, data: data.to_vec() };
    assert_eq!(blocks[0].body, want);
    assert_eq!(pulse_lengths(&blocks), [300.0, 500.0, 30000.0]);
}

fn csw_v2_z_rle(data: &[u8]) -> Vec<u8> {
    let mut csw = b"Compressed Square Wave\x1a".to_vec();
    csw.extend([2, 0]);
    csw.extend(35000u32.to_le_bytes());
    csw.extend(2u32.to_le_bytes());
    csw.extend([2, 0, 0]);
    csw.extend([0u8; 16]);
    csw.extend(data);
    csw
}

#[test]
fn a_z_rle_csw_file_is_kept_compressed_and_inflated_only_to_describe_it() {
    let csw = csw_v2_z_rle(b"compressed");
    let inflate = |data: &[u8]| -> Result<Vec<u8>, String> {
        assert_eq!(data, b"compressed");
        Ok(vec![4, 6])
    };
    let s = describe("t.csw", &csw, &inflate).unwrap();
    assert_eq!((s.format, s.samples, s.kept, s.blocks), ("CSW", 10, Kept::Csw, 1));
    let blocks = keep_as_raw_signal("t.csw", &csw, Channel::Mix, Filter::None, &no_zlib).unwrap();
    let want = Body::Csw {
        pause: 0,
        sample_rate: 35000,
        compression: 2,
        pulse_count: 2,
        data: b"compressed".to_vec(),
    };
    assert_eq!(blocks[0].body, want);
    let refused = describe("t.csw", &csw, &no_zlib).unwrap_err();
    assert_eq!(refused.to_string(), "CSW: the Z-RLE data: no zlib here");
}

fn out_trace(edges: &[u16]) -> Vec<u8> {
    edges
        .iter()
        .flat_map(|&t| [t.to_le_bytes(), 0x00FEu16.to_le_bytes()].concat().into_iter().chain([0]))
        .collect()
}

#[test]
fn an_out_trace_becomes_a_csw_block_of_exact_t_states() {
    let trace = out_trace(&[2168, 4336, 5003, 5738, 6593, 8303]);
    let s = describe("t.out", &trace, &no_zlib).unwrap();
    assert_eq!((s.format, s.rate_hz, s.samples, s.kept), ("OUT", 3_500_000, 8303, Kept::Csw));
    let blocks = keep_as_raw_signal("t.out", &trace, Channel::Mix, Filter::None, &no_zlib).unwrap();
    assert_eq!(pulse_lengths(&blocks), [2168.0, 2168.0, 667.0, 735.0, 855.0, 1710.0]);
    assert_eq!(s.tzx_size, serialize_tzx(&blocks, None).unwrap().len());
}

#[test]
fn pulses_over_255_t_states_take_the_long_form_and_empty_ones_join_their_neighbours() {
    let block = csw_of_tstates(&[100, 300, 0, 50, 0, 0, 20]);
    // 300 and 50 lie either side of an empty pulse, so they join; two empty ones in a row cancel,
    // which leaves 50 and 20 apart.
    let Body::Csw { data, pulse_count, sample_rate, .. } = &block.body else { panic!("not a CSW block") };
    assert_eq!((*pulse_count, *sample_rate), (3, 3_500_000));
    assert_eq!(data, &[100, 0, 94, 1, 0, 0, 20]);
    assert_eq!(pulse_lengths(&[block]), [100.0, 350.0, 20.0]);
}

#[test]
fn a_summary_says_what_the_recording_holds_and_what_keeping_it_makes() {
    let tape = [data_block()];
    let wav = wav_of(&tape, 22050);
    let s = describe("t.wav", &wav, &no_zlib).unwrap();
    assert_eq!((s.format, s.rate_hz, s.channels, s.samples), ("WAV", 22050, 1, (wav.len() - 44) / 2));
    assert_eq!(s.kept, Kept::DirectRecording);
    let kept = keep_as_raw_signal("t.wav", &wav, Channel::Mix, Filter::None, &no_zlib).unwrap();
    assert_eq!(s.blocks, kept.len());
    assert_eq!(s.tzx_size, serialize_tzx(&kept, None).unwrap().len());
    assert!((s.seconds() - s.samples as f64 / 22050.0).abs() < 1e-12);
}

#[test]
fn what_cannot_be_kept_says_why() {
    let message = |e: ImportError| e.to_string();
    let slow = encode_wav(&[0.0; 10], 50, 16);
    assert_eq!(message(describe("t.wav", &slow, &no_zlib).unwrap_err()), "a sample rate of 50 Hz is too low");
    let fast = encode_wav(&[0.0; 10], 7_000_001, 16);
    let refused = message(describe("t.wav", &fast, &no_zlib).unwrap_err());
    assert_eq!(refused, "a sample rate of 7000001 Hz is too high");
    let empty = encode_wav(&[], 44100, 16);
    assert_eq!(message(describe("t.wav", &empty, &no_zlib).unwrap_err()), "the recording holds no sound");
    let mut fast = csw_v2_z_rle(&[]);
    fast[0x19..0x1D].copy_from_slice(&20_000_000u32.to_le_bytes());
    let refused = message(describe("t.csw", &fast, &no_zlib).unwrap_err());
    assert_eq!(refused, "a CSW rate of 20000000 Hz is more than a TZX CSW block holds");
    let bad = Filter::Butterworth { order: 2, low_hz: 300.0, high_hz: 30000.0 };
    let refused = keep_as_raw_signal("t.wav", &wav_of(&[data_block()], 22050), Channel::Mix, bad, &no_zlib);
    assert!(matches!(refused, Err(ImportError::Filter(_))));
}
