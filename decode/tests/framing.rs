// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Standard blocks read from pulses: tapes played by `tapeti-core` and decoded back, clean and
//! distorted inside the ROM's windows, with the stretches no reader takes kept as raw signal or
//! played as pauses.

mod common;

use common::{clicks, jitter, lengths, pulses, same_tape, speed, wobble};
use std::sync::atomic::AtomicBool;
use tapeti_core::audio::{block_duration, TSTATES_PER_SEC};
use tapeti_core::types::{Block, Body};
use tapeti_decode::entry::{decode_file, decode_pulses, Channels, DecodeError, Decoded, Finding, Loss};
use tapeti_decode::fallback::{sort, Part, TONE_RUN};
use tapeti_decode::framing::{
    bit_timeout, leader_pair, pilot_pulse, pilot_run_at, BOUNCE_MOST, ONE_ABOVE, PILOT_PULSE_MOST,
};
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
        assert_eq!(same_tape(&got.blocks, &blocks, factor..=factor), Ok(()), "speed {factor}");
    }
}

/// Blocks saved with no pause run straight into the next block's pilot, which the bit loop reads
/// as 1 bits. With no header, nothing but that pilot marks where the first block ends, at any
/// speed the windows take; a block of all 1 bits is not cut short by its own data.
#[test]
fn a_block_with_no_pause_behind_it_ends_at_the_next_blocks_pilot() {
    let blocks = vec![
        standard(0, with_checksum(0xFF, &[0xFF; 60])),
        standard(0, with_checksum(0x00, &[0x5A; 30])),
        standard(1000, with_checksum(0xFF, &(0..=255).collect::<Vec<u8>>())),
    ];
    let clean = of_blocks(&blocks).unwrap();
    for factor in [0.82, 0.88, 1.0, 1.15, 1.4] {
        let got = decode(&speed(&clean, factor));
        assert_eq!(same_tape(&got.blocks, &blocks, factor..=factor), Ok(()), "speed {factor}");
        let broken = |f: &&Finding| matches!(f, Finding::BadChecksum { .. } | Finding::Short { .. });
        assert!(!got.findings.iter().any(|f| broken(&f)), "speed {factor}: {:?}", got.findings);
    }
    // Behind a header the length is known as well, and the run ends the block at the same place.
    let [header, data] = code_pair(b"nopause   ", 100, 3);
    let mut header_first = vec![header, data];
    if let Body::Standard { pause, .. } = &mut header_first[1].body {
        *pause = 0;
    }
    header_first.push(standard(1000, with_checksum(0xFF, &[1, 2, 3])));
    let got = decode(&of_blocks(&header_first).unwrap());
    assert_eq!(same_tape(&got.blocks, &header_first, 1.0..=1.0), Ok(()));
}

/// The run's walls: `TONE_RUN` (32) pulses, each one the sync loop takes for pilot and pairing
/// with a neighbour within 10% of a pair of the block's pilot halves. A 1 bit's pair, 3,420 T,
/// lies 21% under a 4,336 T pilot pair.
#[test]
fn a_pilot_run_is_32_pulses_within_a_tenth_of_the_pilot() {
    let run = |t: f64, n: usize| pulses(&vec![t as u64; n]);
    assert!(pilot_run_at(&run(2168.0, 32), 0, 2168.0));
    assert!(!pilot_run_at(&run(2168.0, 31), 0, 2168.0), "31 are too few");
    assert!(pilot_run_at(&run(1952.0, 32), 0, 2168.0));
    assert!(!pilot_run_at(&run(1950.0, 32), 0, 2168.0), "under 0.9 of the pilot");
    assert!(pilot_run_at(&run(2384.0, 32), 0, 2168.0));
    assert!(!pilot_run_at(&run(2386.0, 32), 0, 2168.0), "over 1.1 of the pilot");
    assert!(!pilot_run_at(&run(1710.0, 64), 0, 2168.0), "a run of 1 bits");
    // Measured against the block's own pilot: on a tape playing 15% slow, so is the next pilot.
    assert!(pilot_run_at(&run(2493.0, 32), 0, 2493.0));
    assert!(!pilot_run_at(&run(1967.0, 32), 0, 2493.0));
    // A pilot whose halves a skew between the two kinds of edge moves 17% either way holds its
    // pairs, and so does one whose skew changes sides, two long halves and later two short.
    let skewed: Vec<u64> = (0..33).map(|k| if k % 2 == 0 { 2540 } else { 1796 }).collect();
    assert!(pilot_run_at(&pulses(&skewed), 0, 2168.0));
    assert!(pilot_run_at(&pulses(&skewed), 1, 2168.0));
    let flips: Vec<u64> = (0..40).map(|k| if (k + k / 12) % 2 == 0 { 2540 } else { 1796 }).collect();
    assert!(pilot_run_at(&pulses(&flips), 0, 2168.0));
    // Falls slow by 1.5 ms move them 42% either way, and the sync loop still takes every half
    // for pilot: over 1,048 T and to 3,644 T. Past that window a pulse is no pilot, whatever
    // it pairs into.
    let alternate = |long: u64, short: u64| {
        pulses(&(0..32).map(|k| if k % 2 == 0 { long } else { short }).collect::<Vec<u64>>())
    };
    assert!(pilot_run_at(&alternate(3100, 1250), 0, 2168.0));
    assert!(pilot_run_at(&alternate(3288, 1049), 0, 2168.0));
    assert!(!pilot_run_at(&alternate(3290, 1048), 0, 2168.0), "a first sync pulse");
    assert!(pilot_run_at(&alternate(3644, 1100), 0, 2168.0));
    assert!(!pilot_run_at(&alternate(3645, 1100), 0, 2168.0), "the sync loop's counter runs out");
    // Nor does a run start a pulse early, on the last 1 bit's half in front of a pilot.
    let behind_ones: Vec<u64> = [1710].into_iter().chain([2168; 32]).collect();
    assert!(!pilot_run_at(&pulses(&behind_ones), 0, 2168.0));
    assert!(pilot_run_at(&pulses(&behind_ones), 1, 2168.0));
}

#[test]
fn wobble_and_jitter_inside_the_windows_still_read_in_full() {
    let blocks = tape();
    let clean = of_blocks(&blocks).unwrap();
    // The ROM takes a swing of 15% too, but a run of 1 bits 14% slower than the block's own pilot
    // is in the band of the next block's pilot ([`pilot_run_at`]), and the block ends there.
    let got = decode(&wobble(&clean, 0.12, 700_000.0));
    assert_eq!(same_tape(&got.blocks, &blocks, 0.88..=1.12), Ok(()));
    // A zero's pair can grow by 400 T and a one's shrink by as much before they meet the split.
    let got = decode(&jitter(&clean, 190.0, 7));
    assert_eq!(same_tape(&got.blocks, &blocks, 1.0..=1.0), Ok(()));
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
fn a_pilot_whose_block_did_not_read_is_said_lost() {
    // A pilot with its sync gone, and behind it pairs as long as bits whose halves are all too
    // long for a sync pulse, as treble loss leaves a block, loads nothing and is no block-shaped
    // burst to keep as raw signal: a block lost. A pilot fragment that a gap ends, with the block
    // saved again behind it, loses nothing.
    let [h, d] = code_pair(b"again     ", 20, 9);
    let pilot = Block::new(Body::PureTone { pulse_len: 2168, count: 1200 });
    let merged = Block::new(Body::PulseSeq { pulses: [1100, 2200, 1400, 1300].repeat(16) });
    let gap = Block::new(Body::Pause { pause: 500 });
    let lost = |got: &Decoded, at: Option<usize>| {
        got.findings
            .iter()
            .any(|f| matches!(*f, Finding::Lost { block, pulses: 1264.., what: Loss::Pilot } if block == at))
    };
    let got =
        decode(&of_blocks(&[h.clone(), pilot.clone(), merged.clone(), gap.clone(), d.clone()]).unwrap());
    assert_eq!(datas(&got.blocks), datas(&[h.clone(), d.clone()]));
    assert!(lost(&got, Some(0)), "{:?}", got.findings);
    let got = decode(&of_blocks(&[pilot.clone(), merged, gap.clone(), h.clone(), d.clone()]).unwrap());
    assert!(lost(&got, None), "{:?}", got.findings);
    let got = decode(&of_blocks(&[pilot, gap, h.clone(), d.clone()]).unwrap());
    assert_eq!(datas(&got.blocks), datas(&[h, d]));
    assert!(!got.findings.iter().any(|f| matches!(f, Finding::Lost { .. })), "{:?}", got.findings);
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
    assert!(
        matches!(got.findings[..], [Finding::Collapsed { block: 0, pulses: 5.., pilot: false }]),
        "{:?}",
        got.findings
    );
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
    // first pulse, and the second's pilot and sync. The click is wider than a bounce, which
    // would fold into the half-wave before it.
    let at = of_blocks(&blocks[..1]).unwrap().len() - 1 + 3223 + 2 + 101;
    let cut = clicks(&p[at..at + 1], 1, 600.0, 3);
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

    // Behind the length, a recording's decay: two pulses that pair into a 1 bit, short of a byte.
    // They are the pause's, with the silence after them (its 1 ms opening and the rest).
    let [h, d] = code_pair(b"decay     ", 10, 3);
    let Body::Standard { data, .. } = &d.body else { unreachable!() };
    let tail = Block::new(Body::PulseSeq { pulses: vec![873, 2778] });
    let blocks = [h.clone(), standard(0, data.clone()), tail, Block::new(Body::Pause { pause: 1000 })];
    let got = decode(&of_blocks(&blocks).unwrap());
    assert_eq!(got.blocks, vec![h, standard(1001, data.clone())]);
    assert_eq!(got.findings, vec![Finding::Collapsed { block: 1, pulses: 4, pilot: false }]);

    // A run-on that ends in 1 bits right before the next block's pilot: their halves are as long
    // as the sync loop takes for pilot, and the pilot does not start among them.
    let [h, d] = code_pair(b"runs on   ", 10, 3);
    let Body::Standard { data, .. } = &d.body else { unreachable!() };
    let next = standard(1000, with_checksum(0xFF, &[1, 2, 3]));
    for tail in [vec![0x0F], vec![0x00, 0x3F], vec![0xFF]] {
        let long = standard(0, [data.clone(), tail.clone()].concat());
        let got = decode(&of_blocks(&[h.clone(), long, next.clone()]).unwrap());
        assert_eq!(got.blocks[1], standard(0, data.clone()), "{tail:x?}");
        assert_eq!(&got.blocks[3], &next, "{tail:x?}");
        assert!(matches!(got.blocks[2].body, Body::Csw { pause: 0, .. }), "{tail:x?}: {:?}", got.blocks[2]);
    }

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
    // A longer one is more pilot to the ROM, which then takes the second for the first; the
    // pair still reads by its sum, unless it lies past the window, as at the saver's own split.
    assert!(read(&pilot_then_byte(257, 1054, 735)));
    assert!(read(&pilot_then_byte(400, 1048, 735)));
    assert!(read(&pilot_then_byte(400, 1049, 735)));
    assert!(!read(&pilot_then_byte(400, 1049, 1156)));
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
    assert!(seen.last().is_some_and(|&(blocks, done)| blocks >= 6 && done == 1.0), "{seen:?}");

    let stereo: Vec<f32> = mono.iter().flat_map(|&x| [x, x]).collect();
    let mut wav = encode_wav(&stereo, 44100, 16);
    wav[22] = 2; // two channels: the same samples, read as pairs
    wav[28..32].copy_from_slice(&(44100u32 * 4).to_le_bytes());
    wav[32] = 4;
    let seen = run(&wav);
    assert!(seen.windows(2).all(|w| w[0].1 <= w[1].1 && w[0].0 <= w[1].0), "never goes back: {seen:?}");
    // The count is the most any decode has read so far, which a decode not kept can pass.
    let third = seen.iter().position(|s| s.1 > 2.0 / 3.0).expect("the third decode reports");
    assert!(seen[third - 1].0 >= 6 && seen[third - 1].1 == 2.0 / 3.0, "each decode is a third: {seen:?}");
    assert!(seen.last().is_some_and(|&(blocks, done)| blocks >= 6 && done == 1.0), "{seen:?}");
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

/// Whether a pulse of the clean render is a data half-wave: the bits are the only pulses of
/// 855 or 1,710 T.
fn is_data(t: f64) -> bool {
    t == 855.0 || t == 1710.0
}

/// `pulses` with a burst of `burst` (T-states) put into every `every`-th transition between two
/// data half-waves, taken from the halves either side so that every pair keeps its length; and
/// how many went in.
fn bounced(pulses: &[Pulse], every: usize, burst: &[f64]) -> (Vec<Pulse>, usize) {
    let half = burst.len() / 2;
    let (before, after): (f64, f64) = (burst[..half].iter().sum(), burst[half..].iter().sum());
    let mut lengths: Vec<u64> = Vec::new();
    let (mut seen, mut put) = (0, 0);
    let mut carry = 0.0;
    for w in pulses.windows(2) {
        let (x, y) = (w[0].tstates, w[1].tstates);
        let mut x = x - carry;
        carry = 0.0;
        if is_data(w[0].tstates) && is_data(y) {
            seen += 1;
            if seen % every == 0 {
                x -= before;
                lengths.push(x as u64);
                lengths.extend(burst.iter().map(|&t| t as u64));
                carry = after;
                put += 1;
                continue;
            }
        }
        lengths.push(x as u64);
    }
    lengths.push((pulses[pulses.len() - 1].tstates - carry) as u64);
    (common::pulses(&lengths), put)
}

#[test]
fn bounces_inside_the_bits_fold_into_the_half_before_them() {
    // The shapes of a one-bit recording's chatter at 79 T a sample: one and one, one and two,
    // two and one, one and four, three and three; all under the bound.
    let blocks = tape();
    let clean = of_blocks(&blocks).unwrap();
    for (k, burst) in
        [[79.0, 79.0], [79.0, 158.0], [158.0, 79.0], [79.0, 316.0], [237.0, 237.0]].iter().enumerate()
    {
        let (p, put) = bounced(&clean, 7 + k, burst);
        assert!(put > 100, "{put} bounces");
        let got = decode(&p);
        assert_eq!(same_tape(&got.blocks, &blocks, 1.0..=1.0), Ok(()), "{burst:?}: {:?}", got.findings);
    }
    // In every transition, back to back: each fold leaves the next bounce behind a half-wave.
    let (p, _) = bounced(&clean, 1, &[79.0, 79.0]);
    assert_eq!(same_tape(&decode(&p).blocks, &blocks, 1.0..=1.0), Ok(()));
}

#[test]
fn two_pulses_past_the_bound_are_data() {
    // Four and four samples, 632 T: not a bounce, read as the pulses say, which breaks the block.
    let blocks = tape();
    let (p, _) = bounced(&of_blocks(&blocks).unwrap(), 50, &[316.0, 316.0]);
    let got = decode(&p);
    assert!(!datas(&got.blocks).contains(&datas(&blocks)[0]));
}

#[test]
fn a_burst_of_bounces_folds_whole_while_it_stays_under_the_bound() {
    // Two pairs in one transition, 154 77 77 77 T: pair by pair, the pulse behind the first is
    // the second's, no half-wave, and the burst would read as a bit of its own.
    let blocks = tape();
    let clean = of_blocks(&blocks).unwrap();
    let (p, _) = bounced(&clean, 9, &[154.0, 77.0, 77.0, 77.0]);
    assert_eq!(same_tape(&decode(&p).blocks, &blocks, 1.0..=1.0), Ok(()));
    // Three, two, two and one samples: 632 T, past the bound, and read as the pulses say.
    let (p, _) = bounced(&clean, 50, &[237.0, 158.0, 158.0, 79.0]);
    assert!(!datas(&decode(&p).blocks).contains(&datas(&blocks)[0]));
}

#[test]
fn the_bound_lies_under_a_zero_pair_at_the_fastest_speed_the_rom_loads() {
    // Behind a half, the two genuine halves after it last a 0 bit's pair at least; at the
    // fastest speed the ROM loads, 0.831 of the saver's timing, that is 1,421 T.
    const { assert!(BOUNCE_MOST < 2.0 * 855.0 * 0.831) };
    let blocks = tape();
    let (p, _) = bounced(&of_blocks(&blocks).unwrap(), 7, &[158.0, 158.0]);
    assert_eq!(same_tape(&decode(&speed(&p, 0.831)).blocks, &blocks, 0.831..=0.831), Ok(()));
}

#[test]
fn a_tape_off_speed_is_read_at_the_speed_its_leader_measures() {
    // From 1.48 slow a 0 bit's pair passes the ROM's split, and nearer 1.4 a little jitter or
    // wow takes it there; read at the leader's speed, the split moves with the tape. Past 1.55
    // the leader's own window, which measures the speed, no longer takes the pilot.
    let blocks = tape();
    let clean = of_blocks(&blocks).unwrap();
    for factor in [1.48, 1.5, 1.53] {
        let got = decode(&speed(&clean, factor));
        assert_eq!(same_tape(&got.blocks, &blocks, factor..=factor), Ok(()), "speed {factor}");
    }
    let got = decode(&jitter(&speed(&clean, 1.4), 150.0, 3));
    assert_eq!(same_tape(&got.blocks, &blocks, 1.4..=1.4), Ok(()));
    let got = decode(&wobble(&speed(&clean, 1.3), 0.14, 300_000.0));
    assert_eq!(same_tape(&got.blocks, &blocks, 1.3 * 0.86..=1.3 * 1.14), Ok(()));
    assert!(datas(&decode(&speed(&clean, 1.56)).blocks).is_empty());
}

#[test]
fn a_stray_zero_behind_a_header_is_left_out() {
    // A copier saved the header with its checksum, and the saver added its own: a 0x00.
    let [header, data] = code_pair(b"copied    ", 50, 9);
    let Body::Standard { data: h, .. } = &header.body else { unreachable!() };
    let mut copied = h.clone();
    copied.push(0x00);
    let tape = vec![standard(1000, copied), data.clone()];
    let got = decode(&of_blocks(&tape).unwrap());
    assert_eq!(got.blocks, vec![header.clone(), data.clone()]);
    assert_eq!(got.findings, vec![Finding::StrayZero { block: 0 }]);
    // Its last bit is the block's, run into the saver's closing half-wave, as a block's last 0
    // can be (`a_last_0_run_into_the_savers_closing_half_is_read_from_its_first_half`).
    let mut lens = lengths(&of_blocks(&tape).unwrap());
    let level = lens.iter().position(|&t| t == 3500).unwrap();
    lens[level - 1] += 945;
    let got = decode(&pulses(&lens));
    assert_eq!(datas(&got.blocks), datas(&[header.clone(), data.clone()]));
    assert_eq!(got.findings, vec![Finding::StrayZero { block: 0 }]);

    // Only behind a header whose checksum holds; anything else stays the block's.
    let mut broken = h.clone();
    broken[5] ^= 1;
    for kept in [[broken, vec![0x00]].concat(), [with_checksum(0xFF, &[0x00; 17]), vec![0x00]].concat()] {
        let got = decode(&of_blocks(&[standard(1000, kept.clone())]).unwrap());
        assert_eq!(datas(&got.blocks), vec![kept.clone()], "{kept:?}");
        assert!(!got.findings.contains(&Finding::StrayZero { block: 0 }));
    }
    // And only a single 0x00: behind a header the ROM stops at its 19 bytes, and what follows
    // them in the same block is raw signal.
    for (tail, stray) in [(vec![0x01], false), (vec![0x00, 0x00], true)] {
        let got = decode(&of_blocks(&[standard(1000, [h.clone(), tail.clone()].concat())]).unwrap());
        assert_eq!(datas(&got.blocks), vec![h.clone()], "{tail:?}");
        assert!(got.findings.contains(&Finding::RawSignal { block: 1 }), "{tail:?}: {:?}", got.findings);
        assert_eq!(got.findings.contains(&Finding::StrayZero { block: 0 }), stray, "{tail:?}");
    }
}

#[test]
fn a_worn_tape_reads_no_merged_sync_where_there_is_none() {
    let blocks = tape();
    let clean = of_blocks(&blocks).unwrap();
    for (p, played) in [
        (speed(&clean, 0.85), 0.85..=0.85),
        (speed(&clean, 1.3), 1.3..=1.3),
        (wobble(&clean, 0.12, 700_000.0), 0.88..=1.12),
        (jitter(&clean, 190.0, 7), 1.0..=1.0),
        (jitter(&speed(&clean, 1.4), 150.0, 3), 1.4..=1.4),
    ] {
        let got = decode(&p);
        assert_eq!(same_tape(&got.blocks, &blocks, played), Ok(()));
        assert!(!got.findings.iter().any(|f| matches!(f, Finding::MergedSync { .. })), "{:?}", got.findings);
    }
}

#[test]
fn a_recording_that_moves_one_kind_of_edge_still_reads() {
    // Every edge of one kind 500 T late: each pulse alternately longer and shorter by as much.
    // Pairs keep their sums; the sync pair's first pulse alone would pass the ROM's window.
    let blocks = tape();
    let clean = of_blocks(&blocks).unwrap();
    for d in [-500.0, 300.0, 500.0] {
        let lengths: Vec<u64> = clean
            .iter()
            .enumerate()
            .map(|(i, p)| (p.tstates + if i % 2 == 0 { d } else { -d }).round() as u64)
            .collect();
        let got = decode(&pulses(&lengths));
        assert_eq!(same_tape(&got.blocks, &blocks, 1.0..=1.0), Ok(()), "skew {d}: {:?}", got.findings);
    }
}

#[test]
fn a_header_behind_a_header_is_read_whole() {
    // The ROM asks for a header by its flag: loading the data a header declared, it fails at
    // the next header's first byte and reads none of it. So a header behind a header is not cut
    // to the length the first declared.
    let [h, d] = code_pair(b"twice     ", 10, 3);
    for blocks in [vec![h.clone(), h.clone()], vec![h.clone(), h.clone(), h.clone(), d.clone()]] {
        let got = decode(&of_blocks(&blocks).unwrap());
        assert_eq!(got.blocks, blocks);
        assert!(got.findings.is_empty(), "{:?}", got.findings);
    }
}

#[test]
fn a_block_whose_flag_reads_zero_is_a_header_only_where_its_19_bytes_prove_one() {
    // A data block whose first byte is 0x00 runs past 19 bytes; so does one whose 19 are a flag
    // and 18 copies of one value, which fold clean whatever they hold. Nineteen zeros declare no
    // length either, so the block behind them is read to its own end.
    let long = standard(1000, with_checksum(0x00, &(0..40).collect::<Vec<u8>>()));
    let zeros = standard(1000, vec![0x00; 19]);
    let behind = standard(1000, with_checksum(0xFF, &[0x55; 30]));
    for blocks in [vec![long], vec![zeros, behind]] {
        assert_eq!(decode(&of_blocks(&blocks).unwrap()).blocks, blocks);
    }
}

/// The pulses of `blocks` with the last half-wave of block `n`'s data run into what follows it,
/// `join` pulses in all: the edge that closes it is missing, as on a recording whose last
/// half-wave settles into silence, or a player's pause level that never crosses zero.
fn without_the_final_edge(blocks: &[Block], n: usize, join: usize) -> Vec<Pulse> {
    let mut p = of_blocks(blocks).unwrap();
    let levels: Vec<usize> = (0..p.len()).filter(|&i| p[i].tstates == 3500.0).collect();
    let last = levels[n] - 1;
    let joined: f64 = p[last..last + join].iter().map(|q| q.tstates).sum();
    p[last].tstates = joined;
    p.drain(last + 1..last + join);
    p
}

#[test]
fn the_last_bit_of_a_block_of_known_length_is_read_from_its_first_half_when_the_second_proves_nothing() {
    // Data blocks whose last bit is a 0 and a 1, each with its last half-wave run into the
    // silence behind it: the header says where the block ends, and the half before decides the
    // bit. Without that the pair times out and the block is a byte short.
    let mut last_bits = Vec::new();
    for len in [10, 11] {
        let [h, d] = code_pair(b"settles   ", len, 3);
        let Body::Standard { data, .. } = &d.body else { unreachable!() };
        let blocks = [h, d.clone()];
        let got = decode(&without_the_final_edge(&blocks, 1, 3));
        assert_eq!(datas(&got.blocks), datas(&blocks), "last bit {}", data[data.len() - 1] & 1);
        assert!(got.findings.is_empty(), "{:?}", got.findings);
        last_bits.push(data[data.len() - 1] & 1);
    }
    assert_eq!(last_bits, [0, 1]);
}

#[test]
fn a_last_0_run_into_the_savers_closing_half_is_read_from_its_first_half() {
    // The ROM's saver closes a block with a half-wave of 945 T behind its last bit's. Where that
    // lobe fades into the silence, the edge between it and the bit's second half goes, and a
    // last 0 pairs to 855 + 1,800 T, which the ROM reads as a 1. A last 1 so run on pairs to
    // 1,710 + 2,655 T, whose second half proves nothing already.
    let mut last_bits = Vec::new();
    for len in [10, 11] {
        let [h, d] = code_pair(b"closing   ", len, 3);
        let Body::Standard { data, .. } = &d.body else { unreachable!() };
        let blocks = [h, d.clone()];
        let mut lens = lengths(&of_blocks(&blocks).unwrap());
        let level = lens.iter().rposition(|&t| t == 3500).unwrap();
        lens[level - 1] += 945;
        let got = decode(&pulses(&lens));
        assert_eq!(datas(&got.blocks), datas(&blocks), "last bit {}", data[data.len() - 1] & 1);
        last_bits.push(data[data.len() - 1] & 1);
    }
    assert_eq!(last_bits, [0, 1]);
}

#[test]
fn a_one_ms_pause_level_standing_in_a_headers_last_half_is_not_paired() {
    // A header with a 1 ms pause, its last half-wave and the pause's level one pulse: as a
    // second half that pulse would make the bit a 1. The pilot behind it is not the header's.
    let mut last_bits = Vec::new();
    for name in [b"one ms    ", b"one ms 3  "] {
        let [h, d] = code_pair(name, 10, 3);
        let Body::Standard { data, .. } = &h.body else { unreachable!() };
        let h1 = standard(1, data.clone());
        let blocks = [h1, d];
        let got = decode(&without_the_final_edge(&blocks, 0, 2));
        assert_eq!(datas(&got.blocks), datas(&blocks), "last bit {}", data[18] & 1);
        last_bits.push(data[18] & 1);
    }
    assert_eq!(last_bits, [1, 0]);
}

#[test]
fn a_last_half_wave_run_into_the_gap_is_read_from_its_first_half() {
    // A block with no header in front, its last half-wave run into the silence behind it, or into
    // a 1 ms pause level that never crossed zero with the next pilot behind it: the second half
    // is no data, and the first tells the bit, a 0 or a 1.
    let mut bad = with_checksum(0xFF, &[1, 2, 3, 4, 5]);
    bad[3] ^= 0x10;
    for last in [0xB6, 0xB7] {
        let mut data = bad.clone();
        *data.last_mut().unwrap() = last;
        let silence = [standard(1000, data.clone())];
        let got = decode(&without_the_final_edge(&silence, 0, 3));
        assert_eq!(datas(&got.blocks), datas(&silence), "into silence, last byte {last:02x}");
        assert!(
            !got.findings.iter().any(|f| matches!(f, Finding::PartialFinalByte { .. })),
            "{:?}",
            got.findings
        );
        let pilot = [standard(1, data.clone()), standard(1000, with_checksum(0xFF, &[9; 20]))];
        let got = decode(&without_the_final_edge(&pilot, 0, 2));
        assert_eq!(datas(&got.blocks), datas(&pilot), "into a pause level, last byte {last:02x}");
    }
}

#[test]
fn bits_past_the_last_whole_byte_are_kept_only_behind_a_block_that_does_not_prove_its_end() {
    // A dropout five bits into a damaged block's last byte, with the tape going on behind it:
    // the five are kept as a last byte filled with 0s, said on the block.
    let mut bad = with_checksum(0xFF, &[1, 2, 3, 4, 5]);
    bad[3] ^= 0x10;
    let mut p = of_blocks(&[standard(1000, bad.clone())]).unwrap();
    let level = p.iter().position(|q| q.tstates == 3500.0).unwrap();
    p[level - 6].tstates = 6000.0;
    let got = decode(&p);
    let mut want = bad.clone();
    *want.last_mut().unwrap() &= 0xF8;
    assert_eq!(datas(&got.blocks)[0], want);
    assert!(
        got.findings.contains(&Finding::PartialFinalByte { block: 0, used_bits: 5 }),
        "{:?}",
        got.findings
    );

    // Behind a block whose checksum holds, bits past it are the seam, and go.
    let good = with_checksum(0xFF, &[1, 2, 3, 4, 5]);
    let mut p = of_blocks(&[standard(1000, good.clone())]).unwrap();
    let level = p.iter().position(|q| q.tstates == 3500.0).unwrap();
    for _ in 0..2 {
        p.insert(level, Pulse { tstates: 855.0, ..p[level] });
    }
    let got = decode(&p);
    assert_eq!(datas(&got.blocks), vec![good]);
    assert!(
        !got.findings.iter().any(|f| matches!(f, Finding::PartialFinalByte { .. })),
        "{:?}",
        got.findings
    );
}

#[test]
fn a_pilot_one_pulse_into_a_byte_ends_the_block_in_front() {
    // A block with no pause, and a pulse left between it and the next block's pilot, as a
    // pause level cut in two leaves: read on, that pulse and the pilot behind it would make one
    // more byte of 1 bits.
    let a = standard(0, with_checksum(0xFF, &[1, 2, 3]));
    let b = standard(1000, with_checksum(0xFF, &[4, 5, 6]));
    let at = of_blocks(std::slice::from_ref(&a)).unwrap().len();
    let mut p = of_blocks(&[a.clone(), b.clone()]).unwrap();
    assert!(pilot_run_at(&p, at, 2168.0), "the next pilot starts at {at}");
    p.insert(at, Pulse { tstates: 1300.0, ..p[at] });
    let got = decode(&p);
    assert_eq!(datas(&got.blocks), datas(&[a, b]), "{:?}", got.findings);
}

/// The pulse index of the first sync pulse of block `k`: behind the `k`th pilot.
fn sync_of(lens: &[u64], k: usize) -> usize {
    (1..lens.len()).filter(|&i| lens[i] == 667 && lens[i - 1] == 2168).nth(k).unwrap()
}

#[test]
fn a_block_behind_a_pilot_shorter_than_the_roms_stands_on_its_own_proof() {
    // A recording that starts 40 pulses before a header's sync, and a 25 ms dropout 40 pulses
    // before the data block's: the ROM, which needs 256 pairs, loads neither. The header proves
    // itself in every field and the data block answers it, so both are read.
    let [header, data] = code_pair(b"cut       ", 100, 3);
    let lens = lengths(&of_blocks(&[header.clone(), data.clone()]).unwrap());
    let (first, second) = (sync_of(&lens, 0), sync_of(&lens, 1));
    let mut cut = lens[first - 40..].to_vec();
    cut[second - first - 1] = 88_290;
    let got = decode(&pulses(&cut));
    assert_eq!(datas(&got.blocks), datas(&[header, data.clone()]), "{:?}", got.findings);
    for block in [0, 1] {
        assert!(got.findings.contains(&Finding::ShortLeader { block }), "{:?}", got.findings);
    }
    // A block no header asks for proves no more than its checksum, which a misread can hold, and
    // behind so short a pilot it stays raw signal. So does one under 16 pairs of pilot.
    let lens = lengths(&of_blocks(std::slice::from_ref(&data)).unwrap());
    let got = decode(&pulses(&lens[sync_of(&lens, 0) - 40..]));
    assert!(!got.blocks.iter().any(|b| matches!(b.body, Body::Standard { .. })), "{:?}", got.blocks);
    assert!(got.findings.contains(&Finding::RawSignal { block: 0 }), "{:?}", got.findings);
    let [header, data] = code_pair(b"cut       ", 100, 3);
    let lens = lengths(&of_blocks(&[header, data]).unwrap());
    let got = decode(&pulses(&lens[sync_of(&lens, 0) - 30..]));
    assert_eq!(got.blocks.iter().filter(|b| matches!(b.body, Body::Standard { .. })).count(), 1);
}

#[test]
fn a_recording_that_starts_inside_a_block_leaves_its_rest_in_the_lead_in() {
    // Cut a byte, fifty or the whole block in: no pilot, no sync, no flag, nothing a Spectrum
    // loads, and nothing to say where a byte begins. The rest is left out with the lead-in, its
    // pulses counted, and the program behind it reads whole.
    let [_, cut] = code_pair(b"first     ", 300, 1);
    let next = code_pair(b"second    ", 100, 5);
    let lens = lengths(&of_blocks(&[cut, next[0].clone(), next[1].clone()]).unwrap());
    for (bytes_in, held) in [(1, 4818), (50, 4034), (301, 18)] {
        let got = decode(&pulses(&lens[sync_of(&lens, 0) + 2 + 16 * bytes_in..]));
        assert_eq!(datas(&got.blocks), datas(&next), "{bytes_in} bytes in");
        assert!(
            matches!(got.findings[..], [Finding::LeadIn { pulses, .. }] if pulses == held),
            "{bytes_in} bytes in: {:?}",
            got.findings
        );
    }
}

/// A CODE header whose checksum's lowest bit, the block's last bit, is `last`, and its pulses with
/// every edge of one kind `skew` T-states late, the last half-wave run into silence.
fn skewed_last_bit(last: u8, skew: i64) -> (Vec<u8>, Vec<Pulse>) {
    let mut fields = vec![3];
    fields.extend(b"skewed    ");
    fields.extend([100, 0, 0x00, 0x80, 0x00, 0x00]);
    let mut data = with_checksum(0x00, &fields);
    while data[18] & 1 != last {
        fields[16] += 1;
        data = with_checksum(0x00, &fields);
    }
    let lens = lengths(&of_blocks(&[standard(1000, data.clone())]).unwrap());
    let end = sync_of(&lens, 0) + 2 + 19 * 16;
    let mut skewed: Vec<u64> = lens[..end]
        .iter()
        .enumerate()
        .map(|(k, &t)| (t as i64 + if k % 2 == 0 { skew } else { -skew }) as u64)
        .collect();
    *skewed.last_mut().unwrap() += 3_500_000;
    (data, pulses(&skewed))
}

/// The wall of a last bit read from its first half alone: the half stands 427.5 T from the split
/// between a 0's and a 1's, and a skew between the two kinds of edge moves a lone half whole,
/// where a pair cancels it. Read right to 400 T either way, as far as a worn head has been
/// measured to skew one kind of edge; past it a 0 reads as a 1 at 430 T and a 1 as a 0 at 500,
/// and the checksum, whose lowest bit it is, fails.
#[test]
fn a_last_bit_read_from_its_first_half_reads_right_to_a_skew_of_400_t() {
    let read = |last: u8, skew: i64| {
        let (data, p) = skewed_last_bit(last, skew);
        decode(&p).blocks.iter().any(|b| matches!(&b.body, Body::Standard { data: d, .. } if *d == data))
    };
    for last in [0, 1] {
        for skew in [-400, -200, 0, 200, 400] {
            assert!(read(last, skew), "last bit {last}, skew {skew}");
        }
    }
    assert!(!read(0, -430));
    assert!(!read(1, 500));
}
