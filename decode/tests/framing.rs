// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Standard blocks read from pulses: tapes played by `tapeti-core` and decoded back, clean and
//! distorted inside the ROM's windows, with the stretches no reader takes kept as raw signal or
//! played as pauses.

mod common;

use common::{clicks, jitter, lengths, pulses, speed, wobble};
use std::sync::atomic::AtomicBool;
use tapeti_core::audio::{block_duration, TSTATES_PER_SEC};
use tapeti_core::types::{Block, Body};
use tapeti_decode::entry::{decode_file, decode_pulses, Channels, DecodeError, Decoded, Finding};
use tapeti_decode::fallback::{sort, Part, TONE_RUN};
use tapeti_decode::framing::{bit_timeout, leader_pair, pilot_pulse, ONE_ABOVE, PILOT_PULSE_MOST};
use tapeti_decode::pulses::{of_blocks, Pulse};
use tapeti_decode::readers::Channel;
use tapeti_decode::waveform::filter::Filter;

fn decode(p: &[Pulse]) -> Decoded {
    decode_pulses(p, TSTATES_PER_SEC, &mut |_| {}, &AtomicBool::new(false)).unwrap()
}

fn standard(pause: u16, data: Vec<u8>) -> Block {
    Block::new(Body::Standard { pause, data })
}

/// A flag, the bytes and their checksum.
fn with_checksum(flag: u8, bytes: &[u8]) -> Vec<u8> {
    let mut data = vec![flag];
    data.extend(bytes);
    data.push(data.iter().fold(0, |x, &d| x ^ d));
    data
}

/// A CODE header for `len` bytes and the data block after it.
fn code_pair(name: &[u8; 10], len: usize, seed: u8) -> [Block; 2] {
    let mut h = vec![3];
    h.extend(name);
    h.extend((len as u16).to_le_bytes());
    h.extend([0x00, 0x80, 0x00, 0x80]);
    let body: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(37).wrapping_add(seed)).collect();
    [standard(1000, with_checksum(0x00, &h)), standard(2000, with_checksum(0xFF, &body))]
}

fn tape() -> Vec<Block> {
    let mut blocks = Vec::new();
    blocks.extend(code_pair(b"first     ", 300, 1));
    blocks.extend(code_pair(b"second    ", 1, 0xFF));
    blocks.push(standard(1500, with_checksum(0xFF, &[0x00; 40])));
    blocks.push(standard(1000, with_checksum(0xFF, &[0xFF; 40])));
    blocks
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

#[test]
fn a_tape_played_to_pulses_decodes_to_the_same_blocks() {
    let blocks = tape();
    let got = decode(&of_blocks(&blocks).unwrap());
    assert_eq!(got.blocks, blocks);
    assert!(got.findings.is_empty(), "{:?}", got.findings);
    assert_eq!(got.extents.len(), blocks.len());
    assert!(got.extents.windows(2).all(|w| w[0].end <= w[1].start + 1e-9));
}

#[test]
fn every_byte_value_comes_back() {
    let blocks = vec![standard(1000, with_checksum(0xFF, &(0..=255).collect::<Vec<u8>>()))];
    assert_eq!(decode(&of_blocks(&blocks).unwrap()).blocks, blocks);
}

#[test]
fn a_tape_off_speed_inside_the_windows_still_reads_in_full() {
    let blocks = tape();
    let clean = of_blocks(&blocks).unwrap();
    for factor in [0.82, 0.9, 1.1, 1.3, 1.4] {
        let got = decode(&speed(&clean, factor));
        assert_eq!(datas(&got.blocks), datas(&blocks), "speed {factor}");
    }
}

#[test]
fn wobble_and_jitter_inside_the_windows_still_read_in_full() {
    let blocks = tape();
    let clean = of_blocks(&blocks).unwrap();
    let got = decode(&wobble(&clean, 0.15, 700_000.0));
    assert_eq!(datas(&got.blocks), datas(&blocks));
    // A zero's pair can grow by 400 T and a one's shrink by as much before they meet the split.
    let got = decode(&jitter(&clean, 190.0, 7));
    assert_eq!(datas(&got.blocks), datas(&blocks));
}

#[test]
fn a_tape_too_fast_for_the_rom_is_kept_as_raw_signal_of_the_same_length() {
    // At 70% speed a pilot pair (3,035 T) is too short for the leader loop. Each block is still a
    // tone with a payload, kept as raw signal, and the silence between two is its pause.
    let blocks = tape();
    let fast = speed(&of_blocks(&blocks).unwrap(), 0.7);
    let got = decode(&fast);
    assert!(datas(&got.blocks).is_empty());
    assert_eq!(got.blocks.len(), blocks.len());
    assert!(got.blocks.iter().all(|b| matches!(b.body, Body::Csw { pause: 1.., .. })));
    let total: f64 = fast.iter().map(|p| p.tstates).sum();
    // On a T-state grid each pulse is rounded to a whole T-state, each pause to a millisecond.
    let kept: u64 = got.blocks.iter().map(block_duration).sum();
    let most = 0.5 * fast.len() as f64 + 1750.0 * blocks.len() as f64;
    assert!((kept as f64 - total).abs() <= most, "{kept} against {total}");
    assert_eq!(got.findings, (0..blocks.len()).map(|block| Finding::RawSignal { block }).collect::<Vec<_>>());
}

#[test]
fn a_stretch_between_blocks_with_no_blocks_shape_is_part_of_the_pause() {
    let junk = [300u16, 5000, 777, 12000, 50];
    let [h, d] = code_pair(b"between   ", 20, 9);
    let mut blocks = vec![h, Block::new(Body::PulseSeq { pulses: junk.to_vec() }), d];
    let got = decode(&of_blocks(&blocks).unwrap());
    // The header's second and the junk's 18,127 T (5 ms).
    if let Body::Standard { pause, .. } = &mut blocks[0].body {
        *pause += 5;
    }
    blocks.remove(1);
    assert_eq!(got.blocks, blocks);
    assert!(matches!(got.findings[..], [Finding::Collapsed { block: 0, pulses: 5.. }]), "{:?}", got.findings);
}

#[test]
fn a_block_shaped_stretch_between_blocks_is_kept_as_it_is() {
    // A turbo block the ROM cannot read: a tone with a payload.
    let turbo = Block::new(Body::Turbo {
        pilot: 1500,
        sync1: 400,
        sync2: 450,
        zero: 500,
        one: 1000,
        pilot_len: 600,
        used_bits: 8,
        pause: 1000,
        data: vec![0x5A, 0x00, 0xC3, 0xFF],
    });
    let [h, d] = code_pair(b"between   ", 20, 9);
    let got = decode(&of_blocks(&[h.clone(), turbo.clone(), d.clone()]).unwrap());
    assert_eq!(got.blocks.len(), 3, "{:?}", got.blocks);
    assert_eq!(got.blocks[0], h);
    assert_eq!(got.blocks[2], d);
    // Every pulse as it was, but the first pilot pulse, which the silence before it ends, and the
    // pause; and the whole as long to a millisecond.
    let played = lengths(&of_blocks(&got.blocks[1..2]).unwrap());
    let want = lengths(&of_blocks(std::slice::from_ref(&turbo)).unwrap());
    let inner = &want[1..want.len() - 1];
    assert!(played.windows(inner.len()).any(|w| w == inner));
    let (kept, was) = (block_duration(&got.blocks[1]), block_duration(&turbo));
    assert!(kept.abs_diff(was) <= 3500, "{kept} against {was}");
    assert_eq!(got.findings, vec![Finding::RawSignal { block: 1 }]);
}

/// A second of silence, 200 pilot half-waves, `tail`, and a second of silence.
fn tone_then(tail: &[u64]) -> Vec<Pulse> {
    let mut l = vec![3_500_000];
    l.extend([2168; 200]);
    l.extend(tail);
    l.push(3_500_000);
    pulses(&l)
}

#[test]
fn a_burst_is_kept_only_block_shaped() {
    let n = |t: u64, n: usize| vec![t; n];
    let gap = |p: &[Pulse]| vec![Part::Gap(0..p.len())];
    let kept =
        |p: &[Pulse]| vec![Part::Gap(0..1), Part::Signal(1..p.len() - 1), Part::Gap(p.len() - 1..p.len())];
    // A tone with nothing behind it is an orphan, which no loader loads.
    let p = tone_then(&[]);
    assert_eq!(sort(&p, 0), gap(&p));
    // With TONE_RUN faster pulses behind it, it has a payload; with one fewer, it has none.
    let p = tone_then(&n(1710, TONE_RUN));
    assert_eq!(sort(&p, 0), kept(&p));
    let p = tone_then(&n(1710, TONE_RUN - 1));
    assert_eq!(sort(&p, 0), gap(&p));
    // Behind a pair that ends the tone, pulses under nine tenths of it (1,951 T) are a payload, and
    // pulses over it a second tone.
    let p = tone_then(&[[1200, 1200].as_slice(), &n(1900, TONE_RUN)].concat());
    assert_eq!(sort(&p, 0), kept(&p));
    let p = tone_then(&[[1200, 1200].as_slice(), &n(2000, TONE_RUN)].concat());
    assert_eq!(sort(&p, 0), gap(&p));
    // A tone is within ±10%: the ROM's pilot does not run on into its 1 bits.
    let p = tone_then(&[n(1710, TONE_RUN), n(2168, TONE_RUN), n(855, TONE_RUN)].concat());
    assert_eq!(sort(&p, 0), kept(&p));
}

#[test]
fn quiet_pulses_and_long_ones_are_gaps_and_kept_pulses_are_signal() {
    let mut p = tone_then(&[1710; 2 * TONE_RUN]);
    // A quiet stretch between the tone and its payload: neither side is block-shaped.
    for q in &mut p[195..201] {
        q.quiet = true;
    }
    assert_eq!(sort(&p, 0), vec![Part::Gap(0..p.len())]);
    // What the caller keeps is signal whatever it is.
    assert_eq!(sort(&p, 3), vec![Part::Signal(0..3), Part::Gap(3..p.len())]);
    // Past 65,535 T a pulse is a gap even inside a burst.
    let p = pulses(&[[2168; 100].as_slice(), &[65536], &[2168; 100]].concat());
    assert_eq!(sort(&p, 0), vec![Part::Gap(0..p.len())]);
}

#[test]
fn a_click_in_the_data_loses_that_block_and_no_other() {
    let blocks = tape();
    let mut p = of_blocks(&blocks).unwrap();
    // Into the second block's data: past the first block, whose pause joins the next pilot's
    // first pulse, and the second's pilot and sync.
    let at = of_blocks(&blocks[..1]).unwrap().len() - 1 + 3223 + 2 + 101;
    let cut = clicks(&p[at..at + 1], 1, 60.0, 3);
    p.splice(at..at + 1, cut);
    let got = decode(&p);
    let mut want = datas(&blocks);
    let read = datas(&got.blocks);
    assert!(!read.contains(&want[1]));
    want.remove(1);
    assert!(want.iter().all(|d| read.contains(d)));
    assert!(got
        .findings
        .iter()
        .any(|f| matches!(f, Finding::BadChecksum { .. } | Finding::RawSignal { .. })));
}

#[test]
fn a_bad_checksum_is_read_and_reported() {
    let mut data = with_checksum(0xFF, &[1, 2, 3]);
    *data.last_mut().unwrap() ^= 0x10;
    let blocks = vec![standard(1000, data)];
    let got = decode(&of_blocks(&blocks).unwrap());
    assert_eq!(got.blocks, blocks);
    assert_eq!(got.findings, vec![Finding::BadChecksum { block: 0 }]);
}

#[test]
fn the_block_after_a_header_has_the_headers_length() {
    // The header asks for 10 bytes and the block holds 30: the ROM reads ten, and the rest stays
    // raw signal.
    let [h, _] = code_pair(b"longer    ", 10, 3);
    let long = standard(1000, with_checksum(0xFF, &[0x5A; 28]));
    let got = decode(&of_blocks(&[h.clone(), long]).unwrap());
    assert_eq!(got.blocks[0], h);
    assert!(matches!(&got.blocks[1].body, Body::Standard { data, .. } if data.len() == 12));
    assert!(matches!(got.blocks[2].body, Body::Csw { .. }));

    // Asked for 300 and holding 20: short.
    let [h, _] = code_pair(b"shorter   ", 300, 3);
    let short = standard(1000, with_checksum(0xFF, &[0xA5; 18]));
    let got = decode(&of_blocks(&[h, short.clone()]).unwrap());
    assert_eq!(got.blocks[1], short);
    assert_eq!(got.findings, vec![Finding::Short { block: 1, expected: 302, read: 20 }]);
}

#[test]
fn a_block_without_a_pause_ends_where_the_header_says() {
    let [h, d] = code_pair(b"no pause  ", 50, 5);
    let d0 = match &d.body {
        Body::Standard { data, .. } => standard(0, data.clone()),
        _ => unreachable!(),
    };
    let next = standard(1000, with_checksum(0xFF, &[7; 5]));
    let blocks = vec![h, d0, next];
    assert_eq!(decode(&of_blocks(&blocks).unwrap()).blocks, blocks);
}

#[test]
fn a_long_silence_is_a_pause_and_pause_blocks() {
    let blocks = vec![standard(1000, with_checksum(0xFF, &[1])), Block::new(Body::Pause { pause: 30000 })];
    let mut p = of_blocks(&blocks).unwrap();
    // One silence of 100 s after the block.
    let tail = p.pop().unwrap();
    let start = tail.start;
    p.push(Pulse { tstates: 100.0 * f64::from(TSTATES_PER_SEC) - 3500.0, start, quiet: false });
    let got = decode(&p);
    assert!(matches!(got.blocks[0].body, Body::Standard { pause: 65535, .. }));
    assert_eq!(got.blocks[1].body, Body::Pause { pause: 34465 });
}

#[test]
fn the_rom_windows_from_its_loops() {
    // Leader: 43 to 99 reads a pair.
    assert!(!leader_pair(895.0 + 59.0 * 42.0, false));
    assert!(leader_pair(895.0 + 59.0 * 42.0 + 1.0, false));
    assert!(leader_pair(6736.0, false) && !leader_pair(6737.0, false));
    assert!(leader_pair(3340.0, true) && !leader_pair(3339.0, true) && !leader_pair(6703.0, true));
    // A standard pilot pair, and a one's pair which locks the leader too.
    assert!(leader_pair(2.0 * 2168.0, false) && leader_pair(2.0 * 1710.0, false));
    // The sync loop: more pilot from 11 reads to 54.
    assert!(!pilot_pulse(1048.0) && pilot_pulse(1049.0) && pilot_pulse(2168.0));
    assert_eq!(PILOT_PULSE_MOST, 3644.0);
    // Bits: the split inside every position's band, the end by position.
    assert!((2481.0..=2540.0).contains(&ONE_ABOVE));
    const { assert!(2.0 * 855.0 < ONE_ABOVE && 2.0 * 1710.0 > ONE_ABOVE) };
    assert_eq!(bit_timeout(0), 5561.0);
    assert_eq!(bit_timeout(1), 5549.0);
    assert_eq!(bit_timeout(8), 5550.0);
    assert_eq!(bit_timeout(16), 5546.0);
    assert_eq!(bit_timeout(17), 5549.0);
}

/// A pilot of `n` pairs at 2,168 T, then sync and one byte with its checksum.
fn pilot_then_byte(pairs: usize, sync1: u64, sync2: u64) -> Vec<Pulse> {
    let mut l = vec![2168u64; 2 * pairs];
    l.extend([sync1, sync2]);
    for byte in [0xFFu8, 0xFF] {
        for bit in (0..8).rev() {
            let t = if byte >> bit & 1 == 1 { 1710 } else { 855 };
            l.extend([t, t]);
        }
    }
    l.push(1_000_000);
    pulses(&l)
}

#[test]
fn the_leader_needs_its_pairs_and_the_sync_its_window() {
    let read = |p: &[Pulse]| datas(&decode(p).blocks) == [vec![0xFF, 0xFF]];
    // One pair unchecked, 256 checked.
    assert!(read(&pilot_then_byte(257, 667, 735)));
    assert!(!read(&pilot_then_byte(256, 667, 735)));
    // Straight out of the leader the first sync pulse may be 1,054 T, after more pilot 1,048 T.
    // A longer one is more pilot, and the second sync pulse is then taken for the first.
    assert!(read(&pilot_then_byte(257, 1054, 735)));
    assert!(read(&pilot_then_byte(400, 1048, 735)));
    assert!(!read(&pilot_then_byte(400, 1049, 735)));
    // The second only has to come before the counter runs out: 3,573 T behind a one-read sync,
    // 3,042 T behind a ten-read one.
    assert!(read(&pilot_then_byte(400, 400, 3573)));
    assert!(!read(&pilot_then_byte(400, 400, 3574)));
    assert!(read(&pilot_then_byte(400, 1048, 3042)));
    assert!(!read(&pilot_then_byte(400, 1048, 3043)));
}

#[test]
fn a_set_cancel_flag_stops_the_decode() {
    let p = of_blocks(&tape()).unwrap();
    let got = decode_pulses(&p, TSTATES_PER_SEC, &mut |_| {}, &AtomicBool::new(true));
    assert_eq!(got, Err(DecodeError::Cancelled));
}

/// A cancel set while the edges of a long recording are being found ends the decode then: the
/// samples stop, so no pass over them runs to the end. With the flag set from the start, ten
/// minutes of tone come back cancelled at once, where reading them takes seconds.
#[test]
fn a_set_cancel_flag_stops_the_edge_detector() {
    use tapeti_core::audio::encode_wav;
    let rate = 11025;
    let tone: Vec<f32> = (0..rate * 600).map(|i| if i / 7 % 2 == 0 { 0.5 } else { -0.5 }).collect();
    let wav = encode_wav(&tone, rate as u32, 8);
    let no_zlib = |_: &[u8]| -> Result<Vec<u8>, String> { Err("none".into()) };
    let started = std::time::Instant::now();
    let got = decode_file(
        "t.wav",
        &wav,
        Channels::Only(Channel::Mix),
        Filter::None,
        None,
        &no_zlib,
        &mut |_| {},
        &AtomicBool::new(true),
    );
    assert_eq!(got, Err(DecodeError::Cancelled));
    assert!(started.elapsed().as_millis() < 500, "took {:?}", started.elapsed());
}

#[test]
fn progress_comes_after_every_block_and_at_the_end() {
    let p = of_blocks(&tape()).unwrap();
    let mut seen = Vec::new();
    decode_pulses(&p, TSTATES_PER_SEC, &mut |g| seen.push((g.blocks, g.done)), &AtomicBool::new(false))
        .unwrap();
    let blocks: Vec<usize> = seen.iter().map(|s| s.0).collect();
    assert_eq!(blocks, [1, 2, 3, 4, 5, 6, 6]);
    assert!(seen.windows(2).all(|w| w[0].1 <= w[1].1), "{seen:?}");
    assert_eq!(seen.last().unwrap().1, 1.0);
}

/// Sampled audio reports how far the edge detector has come before any block is read, and a
/// stereo recording read for the best channel counts its three decodes as one.
#[test]
fn progress_counts_the_edge_detector_and_every_channel() {
    use tapeti_core::audio::{encode_wav, playback_order, render_tape, FlowOptions, RenderOptions};
    let blocks = tape();
    let order = playback_order(&blocks, FlowOptions::default());
    let mono = render_tape(&blocks, RenderOptions { sample_rate: 44100, mic: true, amplitude: 0.5 }, &order);
    let no_zlib = |_: &[u8]| -> Result<Vec<u8>, String> { Err("none".into()) };
    let run = |wav: &[u8]| {
        let mut seen = Vec::new();
        let mut progress = |g: tapeti_decode::entry::Progress| seen.push((g.blocks, g.done));
        decode_file(
            "t.wav",
            wav,
            Channels::Best,
            Filter::None,
            None,
            &no_zlib,
            &mut progress,
            &AtomicBool::new(false),
        )
        .unwrap();
        seen
    };

    let seen = run(&encode_wav(&mono, 44100, 16));
    assert!(seen.windows(2).all(|w| w[0].1 <= w[1].1), "never goes back: {seen:?}");
    assert!(seen[0].0 == 0 && seen[0].1 > 0.0, "the edge detector reports first: {seen:?}");
    assert_eq!(*seen.last().unwrap(), (6, 1.0));

    let stereo: Vec<f32> = mono.iter().flat_map(|&x| [x, x]).collect();
    let mut wav = encode_wav(&stereo, 44100, 16);
    wav[22] = 2; // two channels: the same samples, read as pairs
    wav[28..32].copy_from_slice(&(44100u32 * 4).to_le_bytes());
    wav[32] = 4;
    let seen = run(&wav);
    assert!(seen.windows(2).all(|w| w[0].1 <= w[1].1 && w[0].0 <= w[1].0), "never goes back: {seen:?}");
    let third = seen.iter().position(|s| s.1 > 2.0 / 3.0).expect("the third decode reports");
    assert_eq!(seen[third - 1], (6, 2.0 / 3.0), "each decode is a third, ending on its blocks");
    assert_eq!(*seen.last().unwrap(), (6, 1.0));
}

#[test]
fn a_wav_decodes_through_the_file_entry_point() {
    use tapeti_core::audio::{encode_wav, playback_order, render_tape, FlowOptions, RenderOptions};
    let blocks = tape();
    let order = playback_order(&blocks, FlowOptions::default());
    let samples =
        render_tape(&blocks, RenderOptions { sample_rate: 44100, mic: false, amplitude: 0.8 }, &order);
    let wav = encode_wav(&samples, 44100, 16);
    let no_zlib = |_: &[u8]| -> Result<Vec<u8>, String> { Err("none".into()) };
    let got = decode_file(
        "t.wav",
        &wav,
        Channels::Only(Channel::Mix),
        Filter::None,
        None,
        &no_zlib,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(got.rate_hz, 44100);
    assert_eq!(datas(&got.blocks), datas(&blocks));
}
