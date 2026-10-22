// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The channel of a stereo recording, chosen by decoding each: the choice on constructed
//! decodes, then stereo recordings rendered by `tapeti-core`.

mod common;

use common::same_tape;
use std::sync::atomic::AtomicBool;
use tapeti_core::audio::{playback_order, render_tape, FlowOptions, RenderOptions};
use tapeti_core::types::{Block, Body};
use tapeti_decode::entry::{decode_file, Channels, Decoded, Extent, Finding, Loss};
use tapeti_decode::fold::{delivered, fold, pick, pick_reading, shape, vote, Source};
use tapeti_decode::readers::Channel;
use tapeti_decode::waveform::edges::Axis;
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

/// `blocks` as a decode lays them out: block `i` from `at` plus 100,000 samples a block, its
/// signal 90,000 of them, then its pause.
fn laid(blocks: Vec<Block>, at: f64) -> Decoded {
    Decoded {
        extents: (0..blocks.len())
            .map(|i| {
                let start = at + i as f64 * 100_000.0;
                Extent { start, end: start + 100_000.0, signal_end: start + 90_000.0 }
            })
            .collect(),
        blocks,
        findings: Vec::new(),
        rate_hz: 44100,
        collapsed_ends: [0; 2],
    }
}

/// `blocks` with the bits of `bits` flipped in each byte `at` of block `k`.
fn flipped(blocks: &[Block], k: usize, at: &[usize], bits: u8) -> Vec<Block> {
    let mut blocks = blocks.to_vec();
    if let Body::Standard { data, .. } = &mut blocks[k].body {
        for &i in at {
            data[i] ^= bits;
        }
    }
    blocks
}

fn folded(base: &Decoded, donors: &[(Source, &Decoded)]) -> Decoded {
    let mut base = base.clone();
    fold(&mut base, donors);
    base
}

#[test]
fn a_block_is_taken_where_another_reading_proves_more_at_the_same_place() {
    let a = program("A", &[1, 2, 3, 4]);
    let b = program("BEE", &[5, 6, 7, 8, 9, 10]);
    let broken = flipped(&a, 1, &[2], 1);
    let right = Source::Channel(Channel::Right);
    // A failed block behind the header is replaced from the donor.
    let got = folded(&laid(broken.clone(), 0.0), &[(right, &laid(a.clone(), 0.0))]);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert_eq!(got.findings, [Finding::FromChannel { block: 1, channel: Channel::Right }]);
    // Not from a donor whose read fails there too.
    let got = folded(&laid(broken.clone(), 0.0), &[(right, &laid(broken.clone(), 0.0))]);
    assert_eq!(got.blocks, broken);
    // Nor from a donor that reads the same program elsewhere in the recording, where the base
    // reads another one whole: a block is met where it lies, a header against a header is one
    // rank, and one reading against one decides nothing.
    let base = laid([broken.clone(), b.clone()].concat(), 0.0);
    let got = folded(&base, &[(right, &laid(a.clone(), 200_000.0))]);
    assert_eq!(got.blocks, base.blocks);
}

#[test]
fn a_block_taken_over_a_reading_that_slipped_takes_the_place_of_its_run_on_too() {
    // The base read the data block to its length with its bits slipped and read on past it,
    // a run-on that starts inside the block's own signal: it goes with the failed read.
    let a = program("A", &(0..64u8).collect::<Vec<u8>>());
    let raw = Block::new(Body::Csw {
        pause: 0,
        sample_rate: 44100,
        compression: 1,
        pulse_count: 3,
        data: vec![1, 2, 3],
    });
    let mut base = laid(flipped(&a, 1, &[9], 0x40), 0.0);
    base.extents[1] = Extent { start: 100_000.0, signal_end: 189_500.0, end: 189_500.0 };
    base.blocks.push(raw);
    base.extents.push(Extent { start: 189_500.0, signal_end: 190_200.0, end: 200_000.0 });
    base.findings.push(Finding::RunOn { block: 2 });
    let band = Source::Band;
    let got = folded(&base, &[(band, &laid(a.clone(), 0.0))]);
    assert_eq!(got.blocks.len(), 2, "{:?}", got.blocks);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert_eq!(got.extents[1].end, 200_000.0);
    assert_eq!(got.findings, [Finding::BandPass { block: 1 }]);
}

#[test]
fn a_block_one_reading_reads_whole_answers_the_header_another_read() {
    // The midline reads the header and not the block, the zero line the block and not the
    // header: the block answers the midline's header, so it is the tape's.
    let a = program("A", &(0..200u8).collect::<Vec<u8>>());
    let midline = laid(flipped(&a, 1, &[7], 0x10), 0.0);
    let zero = laid(flipped(&a, 0, &[4], 0x02), 0.0);
    let got = folded(&midline, &[(Source::Axis(Axis::Zero), &zero)]);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert_eq!(got.findings, [Finding::FromAxis { block: 1, axis: Axis::Zero }]);
}

#[test]
fn a_block_another_reading_puts_where_a_pilot_was_heard_is_no_longer_lost() {
    // The midline heard the data block's pilot in the header's pause and read nothing of it;
    // the zero line reads it there.
    let a = program("A", &(0..200u8).collect::<Vec<u8>>());
    let lost = |block| Finding::Lost { block, pulses: 4000, what: Loss::Pilot };
    let mut midline = laid(a[..1].to_vec(), 0.0);
    midline.extents[0].end = 200_000.0;
    midline.findings = vec![lost(Some(0))];
    let zero = laid(a.clone(), 0.0);
    let got = folded(&midline, &[(Source::Axis(Axis::Zero), &zero)]);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert_eq!(got.findings, [Finding::FromAxis { block: 1, axis: Axis::Zero }]);
    // Before the first block, where the lead-in held it.
    let mut midline = laid(Vec::new(), 0.0);
    midline.findings = vec![Finding::LeadIn { ms: 4000, pulses: 4000 }, lost(None)];
    let got = folded(&midline, &[(Source::Axis(Axis::Zero), &zero)]);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert!(!got.findings.contains(&lost(None)), "{:?}", got.findings);
}

#[test]
fn a_block_no_reading_reads_whole_is_read_byte_by_byte_where_most_readings_agree() {
    let a = program("A", &(0..200u8).map(|k| k.wrapping_mul(13).wrapping_add(1)).collect::<Vec<u8>>());
    let wrong_at = |at: &[usize]| laid(flipped(&a, 1, at, 0x10), 0.0);
    let voted = |base: &Decoded, readings: &[&Decoded]| {
        let mut base = base.clone();
        vote(&mut base, readings);
        base
    };
    // Three readings, each wrong at a byte of its own: every byte has a majority.
    let base = wrong_at(&[7]);
    let (r1, r2, r3) = (wrong_at(&[7]), wrong_at(&[50]), wrong_at(&[120]));
    let got = voted(&base, &[&r1, &r2, &r3]);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert_eq!(got.findings, [Finding::Voted { block: 1, readings: 3 }]);
    // Two are not a vote.
    assert_eq!(voted(&base, &[&r1, &r2]).blocks, base.blocks);
    // Two readings wrong alike at a byte outvote the one right there, and the checksum then
    // fails: nothing is taken.
    assert_eq!(voted(&base, &[&r1, &wrong_at(&[7, 50]), &r3]).blocks, base.blocks);
    // A reading that slips out of step counts up to the slip: behind it three are left in step,
    // two of them right.
    let mut slipped = laid(a.clone(), 0.0);
    if let Body::Standard { data, .. } = &mut slipped.blocks[1].body {
        let tail: Vec<u8> = data[100..].iter().map(|&d| d.rotate_left(1) ^ 0x5A).collect();
        data[100..].copy_from_slice(&tail);
    }
    let r4 = wrong_at(&[60]);
    let got = voted(&base, &[&slipped, &r2, &r3, &r4]);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert_eq!(got.findings, [Finding::Voted { block: 1, readings: 4 }]);
    // Where the slip leaves two in step, a byte one of them misreads has no vote.
    assert_eq!(voted(&base, &[&slipped, &r2, &r3]).blocks, base.blocks);
}

#[test]
fn a_header_one_reading_reads_whole_goes_in_front_of_the_block_another_read() {
    // The left channel reads the block and not the header, the mix the header and not the
    // block: the header goes in front, and the block behind it answers it.
    let a = program("A", &(0..200u8).collect::<Vec<u8>>());
    let left = laid(flipped(&a, 0, &[4], 0x02), 0.0);
    let mix = laid(flipped(&a, 1, &[7], 0x10), 0.0);
    let got = folded(&left, &[(Source::Channel(Channel::Mix), &mix)]);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert_eq!(got.findings, [Finding::FromChannel { block: 0, channel: Channel::Mix }]);
    // A header whose name is colour codes proves itself only by the block behind it, and does
    // where it would stand.
    let coded = program("\u{10}\u{2}MINER", &(0..200u8).collect::<Vec<u8>>());
    let left = laid(flipped(&coded, 0, &[4], 0x02), 0.0);
    let got = folded(&left, &[(Source::Channel(Channel::Mix), &laid(coded.clone(), 0.0))]);
    assert_eq!(datas(&got.blocks), datas(&coded));
}

#[test]
fn at_one_rank_a_block_read_through_the_roms_sync_outranks_one_read_out_of_step_through_a_form() {
    // Both channels read the block through a merged sync, out of step: the same bit wrong in
    // every byte, so the checksum holds. The mix reads it through the sync pair; all three
    // answer the header, and the two alike are outranked by how the third was read.
    let a = program("A", &(0..112u8).collect::<Vec<u8>>());
    let out_of_step = flipped(&a, 1, &(1..=112).collect::<Vec<usize>>(), 0x08);
    let merged = |blocks: &[Block]| {
        let mut d = laid(blocks.to_vec(), 0.0);
        d.findings.push(Finding::MergedSync { block: 1 });
        d
    };
    let (left, right) = (merged(&out_of_step), merged(&out_of_step));
    let mix = laid(a.clone(), 0.0);
    let donors = [(Source::Channel(Channel::Mix), &mix), (Source::Channel(Channel::Right), &right)];
    let got = folded(&left, &donors);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert_eq!(got.findings, [Finding::FromChannel { block: 1, channel: Channel::Mix }]);
    // The other way about, the reading through the sync pair stands.
    let got = folded(&laid(a.clone(), 0.0), &[(Source::Channel(Channel::Left), &left)]);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert!(got.findings.is_empty(), "{:?}", got.findings);
}

#[test]
fn a_reading_through_the_sync_pair_with_two_bytes_wrong_does_not_outrank_one_through_a_form() {
    // A screen read through a merged sync, whole, and through the sync pair with bit 3 wrong in
    // two attributes, the checksum holding: the same start, so the sync decides nothing; the
    // two readings alike stand against the one.
    let a = program("SCREEN", &(0..6912u32).map(|k| (k * 7 + 3) as u8).collect::<Vec<u8>>());
    let mut whole = laid(a.clone(), 0.0);
    whole.findings.push(Finding::MergedSync { block: 1 });
    let damaged = laid(flipped(&a, 1, &[6623, 6728], 0x08), 0.0);
    let got = folded(&whole, &[(Source::Channel(Channel::Right), &damaged)]);
    assert_eq!(datas(&got.blocks), datas(&a));
    let other = whole.clone();
    let got = folded(
        &whole,
        &[(Source::Channel(Channel::Left), &other), (Source::Channel(Channel::Right), &damaged)],
    );
    assert_eq!(datas(&got.blocks), datas(&a));
    // Two alike against it, the damaged reading kept gives way to them.
    let got = folded(
        &damaged,
        &[(Source::Channel(Channel::Left), &whole), (Source::Channel(Channel::Right), &other)],
    );
    assert_eq!(datas(&got.blocks), datas(&a));
    assert_eq!(got.findings[1..], [Finding::BothChannels { block: 1 }]);
}

#[test]
fn at_one_rank_a_block_read_through_fewer_unlike_halves_outranks_the_other() {
    // A reading a bit out from a lost lobe on, whose checksum holds by chance, the same bit
    // wrong in an even number of bytes, against the tape's: both answer the header, and the
    // pair whose halves no saver writes tells them apart, whichever was kept.
    let a = program("A", &(0..112u8).collect::<Vec<u8>>());
    let out = flipped(&a, 1, &(40..=111).collect::<Vec<usize>>(), 0x10);
    let through = |blocks: &[Block], bits: usize, more: Option<Finding>| {
        let mut d = laid(blocks.to_vec(), 0.0);
        if bits > 0 {
            d.findings.push(Finding::UnlikeHalves { block: 1, bits });
        }
        d.findings.extend(more);
        d
    };
    let got = folded(&through(&out, 1, None), &[(Source::Channel(Channel::Left), &through(&a, 0, None))]);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert_eq!(got.findings, [Finding::FromChannel { block: 1, channel: Channel::Left }]);
    let got = folded(&through(&a, 0, None), &[(Source::Channel(Channel::Left), &through(&out, 1, None))]);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert!(got.findings.is_empty(), "{:?}", got.findings);
    // Fewer, not none; and over a reading on the ordinary band.
    let got = folded(&through(&out, 2, None), &[(Source::Channel(Channel::Left), &through(&a, 1, None))]);
    assert_eq!(datas(&got.blocks), datas(&a));
    let faded = Some(Finding::FadeBand { block: 1 });
    let got = folded(&through(&out, 1, None), &[(Source::Axis(Axis::Midline), &through(&a, 0, faded))]);
    assert_eq!(datas(&got.blocks), datas(&a));
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

/// The one-track case above with the right track `lag` samples behind the left: what the decode
/// keeps, as block lengths (0 for raw signal).
fn one_track_lagging(lag: usize) -> Vec<usize> {
    let blocks: Vec<Block> = [
        program("ONE", &(0..64u8).collect::<Vec<u8>>()),
        vec![turbo()],
        program("TWO", &(0..64u8).map(|k| k.wrapping_mul(5)).collect::<Vec<u8>>()),
    ]
    .concat();
    let s = render(&blocks);
    let quiet = span(&blocks, 2);
    let mut left = s.clone();
    let mut right: Vec<f32> = std::iter::repeat_n(0.0, lag).chain(s.iter().copied()).take(s.len()).collect();
    left[quiet.start - 22050..quiet.end].iter_mut().for_each(|v| *v = 0.0);
    let trashed = span(&blocks, 4);
    trash(&mut right, trashed.start + lag..trashed.end + lag, 3);
    let got = decode(&stereo_wav(&left, &right), Channels::Best);
    got.blocks
        .iter()
        .map(|b| match &b.body {
            Body::Standard { data, .. } => data.len(),
            _ => 0,
        })
        .collect()
}

#[test]
fn a_stretch_one_track_heard_comes_in_to_a_lag_of_4000_samples() {
    // The raw-signal rung takes another reading's stretch matched to the blocks either side
    // within `POSITION_TOL`, 1,000 samples; past that, an extra header can enter the pause in
    // front of it as a block too many. Two heads 0.9 mm apart lag 2.4 samples per 10' of azimuth.
    assert_eq!(one_track_lagging(4000), [19, 66, 0, 19, 66]);
    assert_eq!(one_track_lagging(5000), [19, 19, 66, 0, 19, 19, 66]);
}

#[test]
fn raw_signal_goes_only_where_the_kept_decode_heard_silence_and_read_nothing_proved() {
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
        fold(&mut b, &[(Source::Channel(Channel::Right), donor)]);
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
    // A recording whose pause runs on past where the next block starts ends its pause there, so
    // that nothing after it moves.
    let mut late = tape(vec![(raw.clone(), ext(41_000.0, 60_000.0, 260_000.0))], 200_000.0);
    late.blocks.pop();
    late.extents.pop();
    let out = folded(&silent, &late);
    assert_eq!(out.blocks.len(), 4, "{:?}", out.blocks);
    assert_eq!(out.extents[2], ext(41_000.0, 60_000.0, 200_000.0));
    let Body::Csw { pause, .. } = out.blocks[2].body else { unreachable!() };
    assert_eq!(pause, 3175, "140,000 samples at 44.1 kHz");

    // A run-on is the donor's reading of the tail of the block in front, which the kept reading
    // read to its own end: it comes only behind that block taken from the donor.
    let mut run_on = donor(190_000.0);
    run_on.findings.push(Finding::RunOn { block: 2 });
    assert_eq!(folded(&silent, &run_on).blocks, silent.blocks, "a run-on");

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
    let channels = |left: &Decoded, right: &Decoded| {
        [(Source::Channel(Channel::Left), left.clone()), (Source::Channel(Channel::Right), right.clone())]
    };
    let vote = |base: &Decoded, left: &Decoded, right: &Decoded| {
        let donors = channels(left, right);
        folded(base, &donors.iter().map(|(s, d)| (*s, d)).collect::<Vec<_>>())
    };
    // The mix reads bit 6 of two spaces in the header's name as a 1: "`", the checksum holding.
    let tape = program("MACHINE", &[1, 2, 3, 4]);
    let misread = flipped(&tape, 0, &[10, 11], 0x40);
    let got = vote(&laid(misread.clone(), 0.0), &laid(tape.clone(), 2.0), &laid(tape.clone(), -2.0));
    assert_eq!(datas(&got.blocks), datas(&tape));
    assert_eq!(got.findings, [Finding::BothChannels { block: 0 }]);
    // Not where the channels differ from each other, or one reads nothing whole there.
    let other = flipped(&tape, 0, &[3, 4], 0x01);
    let broken = flipped(&tape, 0, &[3], 0x01);
    for right in [other, broken] {
        let got = vote(&laid(misread.clone(), 0.0), &laid(tape.clone(), 0.0), &laid(right, 0.0));
        assert_eq!(datas(&got.blocks), datas(&misread));
        assert!(got.findings.is_empty());
    }
    // A block is met at its own place: the pause of the block before it, running 20 samples
    // into it in one channel, does not make that block the one to compare.
    let leg = flipped(&tape, 1, &[2, 3], 0x04);
    let mut right = laid(tape.clone(), 0.0);
    right.extents[0].end += 20.0;
    let got = vote(&laid(leg, 0.0), &laid(tape.clone(), 0.0), &right);
    assert_eq!(datas(&got.blocks), datas(&tape));
    assert_eq!(got.findings, [Finding::BothChannels { block: 1 }]);
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

#[test]
fn a_block_enters_over_a_failed_reading_of_it_only_where_the_checksum_tells_the_two_apart() {
    // The zero line reads a long block in step with the midline but fails its checksum, whether
    // they part at one byte, a burst of them, two places whose differences do not cancel, or
    // from one byte on as the failed reading slips: the checksum holds over the midline's reading
    // and fails over the zero line's, so the midline's is the tape's.
    let a = program("LONG", &(0..2000u32).map(|k| (k * 13 + 1) as u8).collect::<Vec<u8>>());
    let midline = Source::Axis(Axis::Midline);
    for failed in [
        flipped(&a, 1, &[700], 0x01),
        flipped(&a, 1, &[700, 701, 702], 0x01),
        flipped(&flipped(&a, 1, &[300], 0x20), 1, &[1200], 0x04),
        flipped(&a, 1, &(1500..1991).collect::<Vec<usize>>(), 0x55),
    ] {
        let got = folded(&laid(failed, 0.0), &[(midline, &laid(a.clone(), 0.0))]);
        assert_eq!(datas(&got.blocks), datas(&a));
    }
    // Where they part at two places by the same bit, the midline's checksum may hold despite being
    // wrong twice, here at 300 and 1200, where the failed reading has them right; with the failed
    // reading also slipping at 1900, the checksum cannot tell the two apart and the failure stands.
    let damaged = flipped(&a, 1, &[300, 1200], 0x20);
    let failed = laid(flipped(&a, 1, &(1900..1991).collect::<Vec<usize>>(), 0x55), 0.0);
    let got = folded(&failed, &[(midline, &laid(damaged, 0.0))]);
    assert_eq!(got.blocks, failed.blocks);
}

#[test]
fn a_reading_that_checks_out_against_a_failed_one_parting_twice_is_not_kept_for_it() {
    // As the fold above: the midline holds by bit 5 wrong at 300 and 1200, the zero line fails
    // and slips at 1900, and both read every other block alike. Pick keeps the zero line here,
    // and the midline only where the checksum tells the two apart.
    let a = program("LONG", &(0..2000u32).map(|k| (k * 13 + 1) as u8).collect::<Vec<u8>>());
    let tape = [a.clone(), program("NEXT", &[1, 2, 3])].concat();
    let damaged = laid(flipped(&tape, 1, &[300, 1200], 0x20), 0.0);
    let failed = laid(flipped(&tape, 1, &(1900..1991).collect::<Vec<usize>>(), 0x55), 0.0);
    assert_eq!(pick(&[&failed.blocks[..], &damaged.blocks[..]]), 1);
    assert_eq!(pick_reading(&[Some(&failed), Some(&damaged)]), 0);
    let once = laid(flipped(&tape, 1, &[700], 0x01), 0.0);
    assert_eq!(pick_reading(&[Some(&once), Some(&laid(tape.clone(), 0.0))]), 1);
    let twice = laid(flipped(&flipped(&tape, 1, &[300], 0x20), 1, &[1200], 0x04), 0.0);
    assert_eq!(pick_reading(&[Some(&twice), Some(&laid(tape.clone(), 0.0))]), 1);
    assert_eq!(pick_reading(&[None, Some(&damaged)]), 1);
}

/// A reading of no block at all, `pilot` where it heard a pilot the ROM locks on.
fn nothing_read(pilot: bool) -> Decoded {
    Decoded { findings: vec![Finding::NoBlock { ms: 9000, pulses: 9000, pilot }], ..laid(Vec::new(), 0.0) }
}

#[test]
fn a_reading_of_no_block_at_all_is_not_kept_over_one_that_read_a_block_however_it_failed() {
    // A worn tape: the zero line, whose short lobes no longer cross, reads no block at all, and
    // the midline reads the header with its checksum wrong. Kept, the zero line would drop the
    // header and all that is said of it, since the fold takes nothing a donor failed to read.
    let failed = laid(flipped(&program("A", &[1, 2, 3, 4]), 0, &[3], 0x01)[..1].to_vec(), 0.0);
    assert_eq!(pick_reading(&[Some(&nothing_read(true)), Some(&failed)]), 1);
    // Where none read one, the first that did not fail.
    assert_eq!(pick_reading(&[None, Some(&nothing_read(false)), Some(&nothing_read(true))]), 1);
    assert_eq!(pick_reading(&[None, None]), 0);
}

#[test]
fn a_recording_no_reading_reads_a_block_of_holds_a_pilot_any_of_them_heard() {
    let got = folded(&nothing_read(false), &[(Source::Axis(Axis::Midline), &nothing_read(true))]);
    assert_eq!(got.findings, [Finding::NoBlock { ms: 9000, pulses: 9000, pilot: true }]);
    // A block put in leaves the stretch before it a lead-in.
    let a = program("A", &[1, 2, 3, 4]);
    let got = folded(&nothing_read(false), &[(Source::Band, &laid(a.clone(), 44_100.0))]);
    assert_eq!(datas(&got.blocks), datas(&a));
    assert!(got.findings.contains(&Finding::LeadIn { ms: 1000, pulses: 9000 }), "{:?}", got.findings);
    assert!(!got.findings.iter().any(|f| matches!(f, Finding::NoBlock { .. })), "{:?}", got.findings);
}

#[test]
fn a_block_whose_last_byte_was_filled_out_proves_nothing_where_the_0s_complete_its_checksum() {
    // A reading stops five bits into the checksum byte of a block it read one bit wrong, and the
    // 0s the decode fills the byte out with make up for that bit: the checksum holds, by bits
    // that were never read.
    let a = program("A", &(0..200u8).map(|k| k.wrapping_mul(7).wrapping_add(3)).collect::<Vec<u8>>());
    let tape = [a.clone(), program("NEXT", &[1, 2, 3])].concat();
    let Body::Standard { data, .. } = &tape[1].body else { unreachable!() };
    let unread = data[data.len() - 1] & 0x07;
    assert_ne!(unread, 0, "the 0s must differ from the bits they stand for");
    let mut filled = laid(flipped(&tape, 1, &[7], unread), 0.0);
    if let Body::Standard { data, .. } = &mut filled.blocks[1].body {
        *data.last_mut().unwrap() &= 0xF8;
        assert!(data.iter().fold(0, |x, &d| x ^ d) == 0, "the checksum holds by the 0s");
    }
    filled.findings = vec![Finding::PartialFinalByte { block: 1, used_bits: 5, completes: true }];
    let failed = laid(flipped(&tape, 1, &[50], 0x01), 0.0);

    // Not put over a reading that fails there.
    let got = folded(&failed, &[(Source::Axis(Axis::Midline), &filled)]);
    assert_eq!(got.blocks, failed.blocks);
    // Not kept for it: the two weigh alike, and the first stands.
    assert_eq!(pick_reading(&[Some(&failed), Some(&filled)]), 0);
    // Read again byte by byte where it was kept, its filled-out byte no vote.
    let wrong_at = |at: &[usize]| laid(flipped(&tape, 1, at, 0x10), 0.0);
    let mut base = filled.clone();
    vote(&mut base, &[&filled, &wrong_at(&[60]), &wrong_at(&[90]), &wrong_at(&[120])]);
    assert_eq!(datas(&base.blocks), datas(&tape));
    assert_eq!(base.findings, [Finding::Voted { block: 1, readings: 4 }]);
}
