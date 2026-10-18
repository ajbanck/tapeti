// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The ROM's loader run at every phase (`common::rom`) against the windows framing reads with:
//! each window's edge is a band 59 T wide, refused at every phase at its low end on one side and
//! taken at every phase 59 T on, and framing's figure is that low end. Then whole tapes, and
//! the decoder held to the loader wherever the loader does not depend on the phase.

mod common;

use common::rom::{agrees, loads, Loader};
use common::{jitter, pulses, speed, wobble};
use tapeti_core::types::{Block, Body};
use tapeti_decode::framing::{bit_timeout, leader_pair, pilot_pulse, ONE_ABOVE, PILOT_PULSE_MOST};
use tapeti_decode::pulses::of_blocks;

/// How many of the 59 phases something holds at.
#[derive(Debug, PartialEq)]
enum At {
    Every,
    Some,
    None,
}

fn at(phases: usize) -> At {
    match phases {
        59 => At::Every,
        0 => At::None,
        _ => At::Some,
    }
}

/// The pilot pulse of the saver.
const PILOT: u64 = 2168;

/// A ROM block's pulses: 0.1 s of silence, `pilot` pulses of pilot, the sync pair, the bits of
/// `data`, the saver's closing 945 T and a second of silence. The silence is pulse 0 and the
/// pilot pulses 1 to `pilot`.
///
/// `LD-START` sees the pilot's first edge; the unchecked pair behind the wait ends with pulse
/// 1,616 at every phase, so the leader's pairs are pulses 1,617 and 1,618 to 2,127 and 2,128.
fn block(pilot: usize, sync: [u64; 2], data: &[u8]) -> Vec<u64> {
    let mut v = vec![350_000];
    v.extend(std::iter::repeat_n(PILOT, pilot));
    v.extend(sync);
    for &byte in data {
        for k in (0..8).rev() {
            let half = if byte >> k & 1 == 1 { 1710 } else { 855 };
            v.extend([half, half]);
        }
    }
    v.extend([945, 3_500_000]);
    v
}

/// The pulses `at` and `at + 1` made a pair of `pair` T-states, split as evenly as it goes.
fn probe(mut v: Vec<u64>, at: usize, pair: u64) -> Vec<u64> {
    v[at] = pair / 2;
    v[at + 1] = pair - pair / 2;
    v
}

/// At how many phases `lengths` load as `data` with the sync at pulse `sync`.
fn loaded(lengths: &[u64], data: &[u8], sync: usize) -> At {
    let p = pulses(lengths);
    at((0..59)
        .filter(|&phase| {
            let got = Loader::new(&p, f64::from(phase)).load(data[0], data.len() - 2);
            got.loaded && got.bytes == data && got.sync == Some(sync)
        })
        .count())
}

/// A data block of four bytes.
const DATA: [u8; 6] = [0xFF, 0x12, 0x34, 0x56, 0x78, 0xFF ^ 0x12 ^ 0x34 ^ 0x56 ^ 0x78];

#[test]
fn a_leader_pair_is_43_to_99_reads() {
    // (pair, framing's other work, the first pair checked)
    for (pair, c, first) in [(0, 861, true), (100, 895, false)] {
        let at_pulse = 1617 + 2 * pair;
        let with = |len| loaded(&probe(block(2400, [667, 735], &DATA), at_pulse, len), &DATA, 2401);
        let (few, most) = (c + 59 * 42, c + 59 * 99);
        assert_eq!(
            [with(few), with(few + 1), with(few + 59)],
            [At::None, At::Some, At::Every],
            "pair {pair}"
        );
        assert_eq!(
            [with(most), with(most + 1), with(most + 59)],
            [At::Every, At::Some, At::None],
            "pair {pair}"
        );
        assert!(!leader_pair(few as f64, first) && leader_pair(few as f64 + 1.0, first));
        assert!(leader_pair(most as f64, first) && !leader_pair(most as f64 + 1.0, first));
    }
    // The boundary T-state counts `leader_pair` must agree with.
    assert_eq!([861 + 59 * 42, 861 + 59 * 99, 895 + 59 * 42, 895 + 59 * 99], [3339, 6702, 3373, 6736]);
}

#[test]
fn the_first_sync_pulse_is_ten_reads_or_fewer() {
    // On the usual path, behind a pulse the sync loop read: 1,048 T. Straight out of the
    // leader: 1,054 T.
    for (pilot, most) in [(2400, 1048), (2128, 1054)] {
        let with = |len| loaded(&block(pilot, [len, 735], &DATA), &DATA, pilot + 1);
        assert_eq!([with(most), with(most + 1), with(most + 59)], [At::Every, At::Some, At::None], "{pilot}");
    }
    assert!(!pilot_pulse(1048.0) && pilot_pulse(1049.0));
}

#[test]
fn a_pulse_in_the_sync_loop_is_pilot_up_to_54_reads() {
    // Past the counter, at 3,644 T, the loader starts again, and 270 pulses of pilot are not
    // enough for another leader.
    let with = |len| {
        let mut v = block(2400, [667, 735], &DATA);
        v[2200] = len;
        loaded(&v, &DATA, 2401)
    };
    assert_eq!([with(3644), with(3645), with(3703)], [At::Every, At::Some, At::None]);
    assert_eq!(PILOT_PULSE_MOST, 3644.0);
}

#[test]
fn the_second_sync_pulse_has_what_the_first_left_of_the_counter() {
    // A first sync pulse of 1,048 T takes ten reads at every phase, one of 517 T one read: the
    // second then has 44 or 53 reads, up to 3,042 or 3,573 T.
    for (sync1, most) in [(1048, 3042), (517, 3573)] {
        let with = |len| loaded(&block(2400, [sync1, len], &DATA), &DATA, 2401);
        assert_eq!([with(most), with(most + 1), with(most + 59)], [At::Every, At::Some, At::None], "{sync1}");
    }
}

/// At how many phases bit `bit` of byte `byte` of a block of 0x00 bytes reads as a 1, and at how
/// many the load stops there, with that bit's pair `pair` T-states long.
fn bit_with(byte: usize, bit: usize, pair: u64) -> (At, At) {
    let data = [0u8; 6];
    let at_pulse = 1 + 2400 + 2 + 16 * byte + 2 * bit;
    let p = pulses(&probe(block(2400, [667, 735], &data), at_pulse, pair));
    let runs: Vec<Vec<u8>> =
        (0..59).map(|phase| Loader::new(&p, f64::from(phase)).load(0x00, 4).bytes).collect();
    let ones = runs.iter().filter(|b| b.get(byte).is_some_and(|v| v >> (7 - bit) & 1 == 1)).count();
    let stopped = runs.iter().filter(|b| b.len() <= byte).count();
    (at(ones), at(stopped))
}

#[test]
fn a_bit_is_a_one_from_28_reads_and_runs_out_at_80_by_its_position() {
    // (byte, bit, framing's other work for that bit, its index from the flag's first)
    for (byte, bit, c, index) in
        [(0, 0, 900, 0), (1, 0, 889, 8), (2, 4, 888, 20), (3, 0, 885, 24), (5, 0, 885, 40)]
    {
        let (one, end) = (c + 59 * 27, c + 59 * 79);
        let ones = |len| bit_with(byte, bit, len).0;
        assert_eq!(
            [ones(one), ones(one + 1), ones(one + 59)],
            [At::None, At::Some, At::Every],
            "byte {byte}"
        );
        let stops = |len| bit_with(byte, bit, len).1;
        assert_eq!(
            [stops(end), stops(end + 1), stops(end + 59)],
            [At::None, At::Some, At::Every],
            "byte {byte}"
        );
        assert_eq!(bit_timeout(index), end as f64);
        assert!((2478.0..=2551.0).contains(&ONE_ABOVE));
    }
    // The four one-thresholds and three load-stop counts `framing`'s doc comments cite.
    assert_eq!([900, 889, 888, 885].map(|c| c + 59 * 27), [2493, 2482, 2481, 2478]);
    assert_eq!([900, 888, 885].map(|c| c + 59 * 79), [5561, 5549, 5546]);
    // The split framing takes lies in the mid-byte bits' own band.
    assert!((2481.0..=2540.0).contains(&ONE_ABOVE));
}

fn standard(data: Vec<u8>) -> Block {
    Block::new(Body::Standard { pause: 1000, data })
}

/// A header asking for 200 bytes and the block of them, as (flag, length) pairs and blocks.
fn program() -> (Vec<(u8, usize)>, Vec<Block>) {
    let mut header = vec![0x00, 0x03];
    header.extend(b"rom       ");
    header.extend([200, 0, 0x00, 0x80, 0x00, 0x80]);
    let mut data = vec![0xFF];
    data.extend((0..200u32).map(|i| (i * 37 + 11) as u8));
    let mut blocks = Vec::new();
    for mut d in [header, data] {
        d.push(d.iter().fold(0, |x, &v| x ^ v));
        blocks.push(standard(d));
    }
    (vec![(0x00, 17), (0xFF, 200)], blocks)
}

#[test]
fn a_saved_program_loads_at_every_phase() {
    let (asks, blocks) = program();
    let p = of_blocks(&blocks).unwrap();
    for (verdict, block) in loads(&p, &asks).iter().zip(&blocks) {
        let Body::Standard { data, .. } = &block.body else { unreachable!() };
        assert_eq!(verdict.bytes.as_ref(), Some(data));
    }
}

#[test]
fn a_tape_off_speed_loads_while_its_pairs_stay_in_the_windows() {
    // Fast, a pilot pair leaves the leader's window under 0.778 of the speed; slow, a 0 bit's
    // pair reaches a 1's at about 1.47.
    let (asks, blocks) = program();
    let p = of_blocks(&blocks).unwrap();
    let phases = |f: f64| loads(&speed(&p, f), &asks).iter().map(|v| at(v.phases)).collect::<Vec<_>>();
    assert_eq!(phases(0.80), [At::Every, At::Every]);
    assert_eq!(phases(0.76), [At::None, At::None]);
    assert_eq!(phases(1.44), [At::Every, At::Every]);
    // Past it every bit is a 1: the header's flag is wrong, and the data block's 202 bytes of
    // 0xFF, an even number of one value, fold to a checksum of 0. The ROM loads garbage.
    assert_eq!(phases(1.50), [At::None, At::Every]);
    assert_eq!(loads(&speed(&p, 1.5), &asks)[1].bytes, Some(vec![0xFF; 202]));
}

#[test]
fn where_the_rom_loads_at_every_phase_the_decoder_reads_the_same_bytes() {
    let (asks, blocks) = program();
    let p = of_blocks(&blocks).unwrap();
    let grid = 3_500_000;
    for f in [0.76, 0.8, 0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.44, 1.47, 1.5] {
        assert_eq!(agrees(&speed(&p, f), grid, &asks), Ok(()), "speed {f}");
    }
    for depth in [0.05, 0.1, 0.15, 0.2, 0.25] {
        assert_eq!(agrees(&wobble(&p, depth, 700_000.0), grid, &asks), Ok(()), "wobble {depth}");
    }
    for amount in [100.0, 200.0, 300.0, 400.0, 500.0] {
        assert_eq!(agrees(&jitter(&p, amount, 3), grid, &asks), Ok(()), "jitter {amount}");
    }
}
