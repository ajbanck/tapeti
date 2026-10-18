// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The channel of a stereo recording, chosen by decoding each: the choice on constructed
//! decodes, then stereo recordings rendered by `tapeti-core`.

mod common;

use common::same_tape;
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
    assert_eq!(same_tape(&best.blocks, &tape(), 1.0..=1.0), Ok(()), "{:?}", best.findings);
    assert_eq!(best.findings[0], Finding::Channel { kept: Channel::Left });
}

#[test]
fn a_silent_channel_costs_nothing() {
    // The mix is the live channel at half the level, and every level the decoder reads by is
    // relative: all three decodes read the tape, and the tie goes to the mix.
    let s = render(&tape());
    let best = decode(&stereo_wav(&s, &vec![0.0; s.len()]), Channels::Best);
    assert_eq!(same_tape(&best.blocks, &tape(), 1.0..=1.0), Ok(()));
    assert_eq!(best.findings[0], Finding::Channel { kept: Channel::Mix });
}

#[test]
fn a_mono_recording_is_decoded_once() {
    let s = render(&tape());
    let mono = tapeti_core::audio::encode_wav(&s, 44100, 16);
    let best = decode(&mono, Channels::Best);
    assert_eq!(same_tape(&best.blocks, &tape(), 1.0..=1.0), Ok(()));
    assert!(!best.findings.iter().any(|f| matches!(f, Finding::Channel { .. })));
}

/// Deterministic full-scale noise over `samples[range]`.
fn trash(samples: &mut [f32], range: std::ops::Range<usize>, seed: u64) {
    let mut s = seed;
    for v in &mut samples[range] {
        s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        *v = (z as i32 as f32 / i32::MAX as f32) * 0.9;
    }
}

/// The samples of block `k` of `blocks` in its render, its pause left out: the renderer is
/// deterministic, so the renders of the blocks before it and up to it give the bounds.
fn span(blocks: &[Block], k: usize) -> std::ops::Range<usize> {
    let mut upto = blocks[..=k].to_vec();
    if let Body::Standard { pause, .. } = &mut upto[k].body {
        *pause = 0;
    }
    render(&blocks[..k]).len()..render(&upto).len()
}

#[test]
fn a_block_each_channel_loses_comes_from_the_other() {
    // The left channel loses the second program's data, the right the third's, and the right
    // head lies 11 samples behind, so that the mix loses the data band. No decode holds all
    // three programs; folded together they are whole.
    let blocks = [
        program("ONE", &(0..64u8).collect::<Vec<u8>>()),
        program("TWO", &(0..64u8).map(|k| k.wrapping_mul(3).wrapping_add(2)).collect::<Vec<u8>>()),
        program("THREE", &(0..64u8).map(|k| k.wrapping_mul(7).wrapping_add(3)).collect::<Vec<u8>>()),
    ]
    .concat();
    let s = render(&blocks);
    let mut left = s.clone();
    let mut right: Vec<f32> = std::iter::repeat_n(0.0, 11).chain(s.iter().copied()).collect();
    trash(&mut left, span(&blocks, 3), 1);
    trash(&mut right, span(&blocks, 5), 2);
    let wav = stereo_wav(&left, &right);
    for only in [Channel::Mix, Channel::Left, Channel::Right] {
        assert_ne!(datas(&decode(&wav, Channels::Only(only)).blocks), datas(&blocks), "{only:?} alone");
    }
    let best = decode(&wav, Channels::Best);
    assert_eq!(same_tape(&best.blocks, &blocks, 1.0..=1.0), Ok(()), "{:?}", best.findings);
    let from: Vec<&Finding> =
        best.findings.iter().filter(|f| matches!(f, Finding::FromChannel { .. })).collect();
    assert_eq!(from.len(), 1, "{:?}", best.findings);
}

#[test]
fn a_block_is_taken_only_behind_the_same_header_read_as_often() {
    use tapeti_decode::channels::combine;
    use tapeti_decode::entry::Extent;
    let decoded = |blocks: Vec<Block>| Decoded {
        extents: (0..blocks.len())
            .map(|i| Extent {
                start: i as f64 * 10.0,
                end: i as f64 * 10.0 + 9.0,
                signal_end: i as f64 * 10.0 + 9.0,
            })
            .collect(),
        blocks,
        findings: Vec::new(),
        rate_hz: 44100,
        collapsed_ends: [0; 2],
    };
    let a = program("A", &[1, 2, 3, 4]);
    let broken = |p: &[Block]| {
        let mut p = p.to_vec();
        if let Body::Standard { data, .. } = &mut p[1].body {
            data[2] ^= 1;
        }
        p
    };
    // A failed block behind the header is replaced from the donor.
    let mut base = decoded(broken(&a));
    combine(&mut base, &[(Channel::Right, &decoded(a.clone()))]);
    assert_eq!(datas(&base.blocks), datas(&a));
    assert_eq!(base.findings, [Finding::FromChannel { block: 1, channel: Channel::Right }]);
    // A header the base reads twice and the donor once anchors nothing.
    let mut base = decoded([broken(&a), a.clone()].concat());
    let before = base.blocks.clone();
    combine(&mut base, &[(Channel::Right, &decoded(a.clone()))]);
    assert_eq!(base.blocks, before);
    // Nor does a donor pair that is not whole, nor a header elsewhere in the recording.
    let mut base = decoded(broken(&a));
    combine(&mut base, &[(Channel::Right, &decoded(broken(&a)))]);
    assert_eq!(base.blocks, broken(&a));
    let mut base = decoded(broken(&a));
    let mut far = decoded(a.clone());
    for e in &mut far.extents {
        e.start += 1000.0;
        e.end += 1000.0;
    }
    combine(&mut base, &[(Channel::Right, &far)]);
    assert_eq!(base.blocks, broken(&a));
}

/// A turbo block at about twice the ROM's speed: signal shaped like a block that the ROM's
/// windows do not read, which a decode keeps as raw signal.
fn turbo() -> Block {
    let data: Vec<u8> = (0..200u32).map(|i| (i * 41 + 7) as u8).collect();
    Block::new(Body::Turbo {
        pilot: 1100,
        sync1: 330,
        sync2: 370,
        zero: 430,
        one: 860,
        pilot_len: 3000,
        used_bits: 8,
        pause: 1000,
        data,
    })
}

#[test]
fn a_stretch_one_channel_heard_as_silence_takes_the_others_raw_signal() {
    // The right channel holds a turbo block the left never recorded, and loses the second
    // program's data, so the left is kept: it reads more. The turbo block is no ROM block and
    // proves nothing, but the left heard nothing there, so it comes from the right.
    let blocks: Vec<Block> = [
        program("ONE", &(0..64u8).collect::<Vec<u8>>()),
        vec![turbo()],
        program("TWO", &(0..64u8).map(|k| k.wrapping_mul(5)).collect::<Vec<u8>>()),
    ]
    .concat();
    let s = render(&blocks);
    let mut left = s.clone();
    let mut right = s.clone();
    let quiet = span(&blocks, 2);
    left[quiet.start - 22050..quiet.end].iter_mut().for_each(|v| *v = 0.0);
    trash(&mut right, span(&blocks, 4), 3);
    let wav = stereo_wav(&left, &right);

    let got = decode(&wav, Channels::Best);
    assert_eq!(got.findings.first(), Some(&Finding::Channel { kept: Channel::Left }), "{:?}", got.findings);
    assert_eq!(datas(&got.blocks), datas(&blocks), "{:?}", got.findings);
    let raw: Vec<usize> =
        (0..got.blocks.len()).filter(|&i| matches!(got.blocks[i].body, Body::Csw { .. })).collect();
    assert_eq!(raw, [2], "the turbo block, as raw signal, in its place: {:?}", got.findings);
    assert!(
        got.findings.contains(&Finding::FromChannel { block: 2, channel: Channel::Mix }),
        "{:?}",
        got.findings
    );
    // The pause in front still runs to the stretch, about a second as the tape has it.
    let Body::Standard { pause, .. } = got.blocks[1].body else { unreachable!() };
    assert!(pause.abs_diff(1000) <= 600, "a pause of {pause} ms");

    // The left alone plays the stretch as the pause of the block in front.
    let left_only = decode(&wav, Channels::Only(Channel::Left));
    assert!(!left_only.blocks.iter().any(|b| matches!(b.body, Body::Csw { .. })));
}

#[test]
fn raw_signal_goes_only_where_the_kept_decode_heard_silence_and_read_nothing_proved() {
    use tapeti_decode::channels::combine;
    use tapeti_decode::entry::Extent;
    let ext = |start: f64, signal_end: f64, end: f64| Extent { start, end, signal_end };
    let raw = Block::new(Body::Csw {
        pause: 1500,
        sample_rate: 44100,
        compression: 1,
        pulse_count: 3,
        data: vec![10, 20, 30],
    });
    let bad = standard(vec![0x80, 0x01, 0x02, 0x04]);
    let [h1, d1] = <[Block; 2]>::try_from(program("LOADER", &[1, 2, 3])).unwrap();
    let [h2, _] = <[Block; 2]>::try_from(program("NEXT", &[4, 5, 6])).unwrap();
    let tape = |middle: Vec<(Block, Extent)>, next_at: f64| {
        let mut rows =
            vec![(h1.clone(), ext(0.0, 9_000.0, 9_500.0)), (d1.clone(), ext(10_000.0, 40_000.0, 200_000.0))];
        rows.extend(middle);
        rows.push((h2.clone(), ext(next_at, next_at + 9_000.0, next_at + 9_500.0)));
        let (blocks, extents) = rows.into_iter().unzip();
        Decoded { blocks, extents, findings: Vec::new(), rate_hz: 44100, collapsed_ends: [0; 2] }
    };
    let donor = |end: f64| tape(vec![(raw.clone(), ext(41_000.0, end, 199_000.0))], 200_000.0);
    let folded = |base: &Decoded, donor: &Decoded| {
        let mut b = base.clone();
        combine(&mut b, &[(Channel::Right, donor)]);
        b
    };

    // The data block's pause runs over the stretch: the recording goes in, and the pause
    // in front of it ends where it starts.
    let silent = tape(Vec::new(), 200_000.0);
    let out = folded(&silent, &donor(190_000.0));
    assert_eq!(out.blocks.len(), 4, "{:?}", out.blocks);
    assert_eq!(out.blocks[2], raw);
    assert_eq!(out.extents[2], ext(41_000.0, 190_000.0, 199_000.0));
    assert_eq!(out.findings, [Finding::FromChannel { block: 2, channel: Channel::Right }]);
    let Body::Standard { pause, .. } = out.blocks[1].body else { unreachable!() };
    assert_eq!(pause, 23, "1,000 samples at 44.1 kHz");
    // A bad read and pause blocks in the stretch give way to it too.
    let pieces = tape(
        vec![
            (bad.clone(), ext(40_500.0, 52_000.0, 52_000.0)),
            (Block::new(Body::Pause { pause: 3000 }), ext(52_000.0, 52_000.0, 190_000.0)),
        ],
        200_000.0,
    );
    let out = folded(&pieces, &donor(190_000.0));
    assert_eq!(out.blocks.len(), 4, "{:?}", out.blocks);
    assert_eq!(out.blocks[2], raw);

    // Nothing moves for a recording past the next block's start, where the stretch held a
    // block whose checksum holds, or where it heard signal and played no pause.
    assert_eq!(folded(&silent, &donor(205_000.0)).blocks, silent.blocks, "past the next block");
    let proved = tape(
        vec![(standard(checksummed(vec![0xFF, 9, 9, 8])), ext(40_500.0, 52_000.0, 190_000.0))],
        200_000.0,
    );
    assert_eq!(folded(&proved, &donor(190_000.0)).blocks, proved.blocks, "a block that proves itself");
    let mut heard = tape(vec![(bad.clone(), ext(40_500.0, 190_000.0, 190_000.0))], 200_000.0);
    heard.extents[1].end = 40_000.0;
    if let Body::Standard { pause, .. } = &mut heard.blocks[1].body {
        *pause = 0;
    }
    assert_eq!(folded(&heard, &donor(190_000.0)).blocks, heard.blocks, "no silence");
}

#[test]
fn a_block_both_channels_read_alike_outvotes_the_mixs_other_reading() {
    use tapeti_decode::channels::outvote;
    use tapeti_decode::entry::Extent;
    let decoded = |blocks: Vec<Block>, shift: f64| Decoded {
        extents: (0..blocks.len())
            .map(|i| Extent {
                start: i as f64 * 10.0 + shift,
                end: i as f64 * 10.0 + 9.0 + shift,
                signal_end: i as f64 * 10.0 + 9.0 + shift,
            })
            .collect(),
        blocks,
        findings: Vec::new(),
        rate_hz: 44100,
        collapsed_ends: [0; 2],
    };
    // The mix reads bit 6 of two spaces in the header's name as a 1: "`", the checksum holding.
    let tape = program("MACHINE", &[1, 2, 3, 4]);
    let mut misread = tape.clone();
    if let Body::Standard { data, .. } = &mut misread[0].body {
        data[10] ^= 0x40;
        data[11] ^= 0x40;
    }
    let mut base = decoded(misread.clone(), 0.0);
    outvote(&mut base, &decoded(tape.clone(), 2.0), &decoded(tape.clone(), -2.0));
    assert_eq!(datas(&base.blocks), datas(&tape));
    assert_eq!(base.findings, [Finding::BothChannels { block: 0 }]);
    // Not where the channels differ from each other, or one reads nothing whole there.
    let mut other = tape.clone();
    if let Body::Standard { data, .. } = &mut other[0].body {
        data[3] ^= 0x01;
        data[4] ^= 0x01;
    }
    let mut broken = tape.clone();
    if let Body::Standard { data, .. } = &mut broken[0].body {
        data[3] ^= 0x01;
    }
    for right in [other, broken] {
        let mut base = decoded(misread.clone(), 0.0);
        outvote(&mut base, &decoded(tape.clone(), 0.0), &decoded(right, 0.0));
        assert_eq!(datas(&base.blocks), datas(&misread));
        assert!(base.findings.is_empty());
    }
    // Nor where the channels read it elsewhere in the recording.
    let mut base = decoded(misread.clone(), 0.0);
    outvote(&mut base, &decoded(tape.clone(), 100.0), &decoded(tape.clone(), 100.0));
    assert_eq!(datas(&base.blocks), datas(&misread));
}

#[test]
fn runs_of_one_byte_that_check_out_name_no_incumbent() {
    // The left channel reads the pilots and both headers as runs of 0xFF, cut into blocks of an
    // even length, which check out, and the code block a bit out, its checksum holding by
    // chance. The mix reads both programs whole.
    let whole = [program("3D", &[1, 2, 3, 4]), program("TANX", &(0..200u8).collect::<Vec<u8>>())].concat();
    let slipped = standard(checksummed((0..201u8).map(|k| k.wrapping_mul(13) | 0x80).collect()));
    let scraps = [vec![standard(vec![0xFF; 6]); 7], vec![slipped]].concat();
    assert_eq!(chosen(&whole, &scraps, &whole), 0);
    assert!(shape(&scraps) < shape(&whole));
}
