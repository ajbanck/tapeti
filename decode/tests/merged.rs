// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! A sync pair merged into the half-wave beside it, in each of its three forms, on pulses: the
//! block is read where the merge left its bits, and a sync pair that is there is still read as
//! one.

use std::sync::atomic::AtomicBool;
use tapeti_core::audio::TSTATES_PER_SEC;
use tapeti_core::types::Body;
use tapeti_decode::entry::{decode_pulses, Decoded, Finding};
use tapeti_decode::framing::{ASK, DECIDE, WIDTH};
use tapeti_decode::pulses::Pulse;

const PILOT: f64 = 2168.0;
const SYNC1: f64 = 667.0;
const SYNC2: f64 = 735.0;
const ZERO: f64 = 855.0;
const ONE: f64 = 1710.0;
/// The pilot pulses in front of the sync pair: a data block's.
const PILOT_PULSES: usize = 3223;

/// The pulses of a ROM block holding `bytes`, closed by the saver's level and a second's pause.
fn block(bytes: &[u8]) -> Vec<f64> {
    let mut t = vec![PILOT; PILOT_PULSES];
    t.extend([SYNC1, SYNC2]);
    for byte in bytes {
        for k in (0..8).rev() {
            let half = if byte >> k & 1 == 1 { ONE } else { ZERO };
            t.extend([half, half]);
        }
    }
    t.push(3_500_000.0);
    t
}

fn placed(lengths: &[f64]) -> Vec<Pulse> {
    let mut start = 0.0;
    lengths
        .iter()
        .map(|&tstates| {
            let p = Pulse { tstates, start, quiet: false };
            start += tstates;
            p
        })
        .collect()
}

/// Pulses `i` to `i + 2` as one, as when the lobe between them loses both its edges.
fn merge_at(t: &[f64], i: usize) -> Vec<f64> {
    let mut out = t.to_vec();
    out[i] = t[i] + t[i + 1] + t[i + 2];
    out.drain(i + 1..i + 3);
    out
}

/// The crossing between pulses `i - 1` and `i` moved by `dt` T-states, later where positive: one
/// pulse grows by what the other loses, as noise moves a crossing.
fn move_crossing(t: &mut [f64], i: usize, dt: f64) {
    t[i - 1] += dt;
    t[i] -= dt;
}

fn decode(t: &[f64]) -> Decoded {
    decode_pulses(&placed(t), TSTATES_PER_SEC, &mut |_| {}, &AtomicBool::new(false)).unwrap()
}

fn read(t: &[f64]) -> Vec<u8> {
    match &decode(t).blocks.first().expect("a block").body {
        Body::Standard { data, .. } => data.clone(),
        other => panic!("{other:?}"),
    }
}

fn merged(t: &[f64]) -> bool {
    decode(t).findings.contains(&Finding::MergedSync { block: 0 })
}

/// A ROM header: a valid checksum, a type the ROM knows, a printable name.
fn header() -> Vec<u8> {
    let mut h = vec![0u8; 19];
    h[2..12].copy_from_slice(b"SYNCMERGE ");
    h[12] = 187;
    h[14] = 0x0A;
    h[16] = 0xBB;
    h[18] = h[..18].iter().fold(0, |a, b| a ^ b);
    h
}

/// A data block with a 1 first, its checksum good.
fn data() -> Vec<u8> {
    let mut d = vec![0xFF, 0x21, 0x9C, 0x5E, 0x00, 0x7F, 0xC3, 0x18, 0xA5, 0x42];
    d.push(d.iter().fold(0, |a, b| a ^ b));
    d
}

/// Bytes with bits that differ early and often, and no checksum: only the phase tells.
const HEADER_ISH: &[u8] = &[0x00, 0x00, b'S', b'A', b'B', b'R', b'E', b' ', 0x91, 0x01];
const DATA_ISH: &[u8] = &[0xFF, 0x21, 0x9C, 0x5E, 0x00, 0x7F, 0xC3, 0x18, 0xA5, 0x42];

#[test]
fn a_clean_block_reads_as_the_rom_reads_it() {
    for bytes in [HEADER_ISH, DATA_ISH] {
        assert_eq!(read(&block(bytes)), bytes);
        assert!(!merged(&block(bytes)));
    }
}

#[test]
fn a_backward_merge_is_read_behind_the_merged_pulse() {
    // The pilot's last half and the pair, 3,570 T: the ROM takes it for more pilot and the first
    // 0 bit for the sync pair.
    let t = merge_at(&block(HEADER_ISH), PILOT_PULSES - 1);
    assert_eq!(read(&t), HEADER_ISH);
    assert!(merged(&t));
    // With a 1 first the ROM takes the 1 bits for more pilot too.
    let t = merge_at(&block(DATA_ISH), PILOT_PULSES - 1);
    assert_eq!(read(&t), DATA_ISH);
    // 3% slow the merged pulse runs the sync loop's counter out, and the ROM loses the block.
    let slow: Vec<f64> = merge_at(&block(DATA_ISH), PILOT_PULSES - 1).iter().map(|t| t * 1.03).collect();
    assert_eq!(read(&slow), DATA_ISH);
}

#[test]
fn a_forward_merge_is_read_from_the_half_it_left() {
    // The pair and a 1 bit's first half, 3,112 T: the bits begin with that bit's second half.
    let t = merge_at(&block(DATA_ISH), PILOT_PULSES);
    assert_eq!(read(&t), DATA_ISH);
    assert!(merged(&t));
}

#[test]
fn a_hidden_forward_merge_is_read_from_the_half_it_left() {
    // The pair and a 0 bit's first half, 2,257 T, passes for a pilot half-wave, and the half the
    // merge left is where the ROM finds its sync; only how the bits pair tells.
    let t = merge_at(&block(HEADER_ISH), PILOT_PULSES);
    assert!(((t[PILOT_PULSES] - PILOT) / PILOT).abs() < 0.05);
    assert_eq!(read(&t), HEADER_ISH);
    assert!(merged(&t));
}

#[test]
fn a_stretched_sync_pair_is_still_a_sync_pair() {
    // Stretched by wow to a 0 bit's pair, with a pilot pulse before it that fits the hidden
    // form's length: the bits pair better at the ROM's phase.
    let mut t = block(HEADER_ISH);
    t[PILOT_PULSES] = SYNC1 * 1.30;
    t[PILOT_PULSES + 1] = SYNC2 * 1.18;
    assert_eq!(read(&t), HEADER_ISH);
    assert!(!merged(&t));
}

#[test]
fn a_merge_whose_length_points_the_wrong_way_is_answered_by_its_bytes() {
    // A backward merge whose crossings noise moved, its first late by a quarter of a pilot lobe
    // over pi and its last early by as much of a 1 bit's: 0.4% off the forward prediction, 8.6%
    // off its own. Only the backward reading is a block.
    let pi4 = 4.0 * std::f64::consts::PI;
    let mut t = merge_at(&block(&data()), PILOT_PULSES - 1);
    move_crossing(&mut t, PILOT_PULSES - 1, PILOT / pi4);
    move_crossing(&mut t, PILOT_PULSES, -ONE / pi4);
    let (m, orphan) = (t[PILOT_PULSES - 1], t[PILOT_PULSES]);
    let (fwd, back) = (SYNC1 + SYNC2 + orphan, PILOT + SYNC1 + SYNC2);
    assert!((m - fwd).abs() / fwd < 0.005 && (m - back).abs() / back > 0.08);
    assert_eq!(read(&t), data());

    // And a forward merge that measures 4.2% off the backward prediction, its orphan 14.9% off.
    let mut t = merge_at(&block(&data()), PILOT_PULSES);
    move_crossing(&mut t, PILOT_PULSES, -PILOT / pi4);
    move_crossing(&mut t, PILOT_PULSES + 1, ONE / pi4);
    assert!((t[PILOT_PULSES] - back).abs() / back < 0.05);
    assert_eq!(read(&t), data());
}

#[test]
fn a_damaged_block_behind_a_merge_the_length_tells_is_read_in_step() {
    // No reading is a block, so the length alone decides: a backward merge 6.7% short of its
    // prediction, the noise's reach, inside the deciding width.
    let mut h = header();
    h[18] ^= 0x01;
    let pi4 = 4.0 * std::f64::consts::PI;
    let mut t = merge_at(&block(&h), PILOT_PULSES - 1);
    move_crossing(&mut t, PILOT_PULSES - 1, PILOT / pi4);
    move_crossing(&mut t, PILOT_PULSES, -ZERO / pi4);
    let back = PILOT + SYNC1 + SYNC2;
    let off = (back - t[PILOT_PULSES - 1]) / back;
    assert!((0.067..0.068).contains(&off), "{off}");
    const { assert!(DECIDE > 0.068 && DECIDE < ASK) };
    assert_eq!(read(&t), h);
}

#[test]
fn a_header_behind_a_sync_pair_past_the_roms_window_is_read() {
    // A sync pair distorted far past the ROM's window, 2,950 and 1,000 T, the bits untouched:
    // the ROM takes the first for pilot and the second for its sync, but read as the pair it
    // is, the bits make a header.
    let h = header();
    let mut t = block(&h);
    t[PILOT_PULSES] = 2950.0;
    t[PILOT_PULSES + 1] = 1000.0;
    assert_eq!(read(&t), h);
}

#[test]
fn a_forward_merges_half_past_the_split_is_a_headers_flag_bit() {
    // The half the merge left reads as a 1, its crossing moved past the split, and the flag would
    // be 0x80; the header's flag bit is a 0.
    let h = header();
    let mut t = merge_at(&block(&h), PILOT_PULSES);
    t[PILOT_PULSES] = 2552.0;
    t[PILOT_PULSES + 1] = 1295.0;
    assert_eq!(read(&t), h);
}

#[test]
fn a_merge_with_the_bits_wandering_behind_it_is_read() {
    // Every bit's halves wander alternately by 200 T, so the pairs at the tape's phase are 400 T
    // off and the pairs straddling two equal bits exact; the merge left a 1,200 T half.
    let h = header();
    let mut t = vec![PILOT; PILOT_PULSES];
    t.extend([2552.0, 1200.0]);
    let mut k = 0;
    for byte in &h {
        for b in (0..8).rev() {
            let half = if byte >> b & 1 == 1 { ONE } else { ZERO };
            if k > 0 {
                let wander = if k % 2 == 1 { 200.0 } else { -200.0 };
                t.extend([half + wander, half + wander]);
            }
            k += 1;
        }
    }
    t.push(3_500_000.0);
    assert_eq!(read(&t), h);
}

/// A ROM block of `bytes` behind `pilot` and a sync pair of `sync1` and `sync2`.
fn behind(pilot: Vec<f64>, sync1: f64, sync2: f64, bytes: &[u8]) -> Vec<f64> {
    let mut t = pilot;
    t.extend([sync1, sync2]);
    t.extend(&block(bytes)[PILOT_PULSES + 2..]);
    t
}

/// Whether the block read from `t` is `bytes`.
fn reads(t: &[f64], bytes: &[u8]) -> bool {
    decode(t).blocks.iter().any(|b| matches!(&b.body, Body::Standard { data, .. } if data == bytes))
}

#[test]
fn a_sync_pair_whose_split_moved_reads_by_its_sum() {
    // One kind of edge 441 T late moves the split, 1,108 / 294 T, not the sum: the ROM takes the
    // first for more pilot and the second for its sync, a half-wave late.
    let h = header();
    let t = behind(vec![PILOT; PILOT_PULSES], SYNC1 + 441.0, SYNC2 - 441.0, &h);
    assert_eq!(read(&t), h);
    assert!(merged(&t));
    // By its sum alone, with no block behind to answer.
    let t = behind(vec![PILOT; PILOT_PULSES], SYNC1 + 441.0, SYNC2 - 441.0, HEADER_ISH);
    assert_eq!(read(&t), HEADER_ISH);
    // At the saver's split, a first pulse the ROM refuses, 1,049 T, sums to 2,205 T: no sync.
    let t = behind(vec![PILOT; PILOT_PULSES], 1049.0, 1049.0 * SYNC2 / SYNC1, &h);
    assert!(!reads(&t, &h));
    const { assert!((1.0 + WIDTH) * (SYNC1 + SYNC2) < 2205.0 && (1.0 + WIDTH) * (SYNC1 + SYNC2) > 1048.0 + SYNC2) };
}

#[test]
fn a_pilot_that_alternates_widens_the_pairs_window_by_its_skew() {
    // A pilot alternating 1,725 / 2,659 T, its last half the short one: its last edge lies 467 T
    // early against the other kind, and the pair behind it, 1,326 / 707 T (2,033 T), lies past
    // the window by less than that. Behind a steady pilot of the same mean it is no sync.
    let h = header();
    let alternating: Vec<f64> = (0..PILOT_PULSES)
        .map(|k| if (PILOT_PULSES - 1 - k).is_multiple_of(2) { 1725.0 } else { 2659.0 })
        .collect();
    assert!(reads(&behind(alternating, 1326.0, 707.0, &h), &h));
    assert!(!reads(&behind(vec![2192.0; PILOT_PULSES], 1326.0, 707.0, &h), &h));
}

#[test]
fn the_pilots_last_edge_opens_the_pairs_window_and_the_block_answers() {
    // A steady pilot whose last half is 1,768 T, its last edge 400 T early, and a pair of
    // 1,067 / 835 T, 1,902 T, just past the window: one half-wave only asks, and the header
    // behind answers. Behind a pilot whose last half is a pilot's, or with a header that does
    // not check out, the pair stays no sync.
    let h = header();
    let ending = |last: f64| {
        let mut pilot = vec![PILOT; PILOT_PULSES];
        pilot[PILOT_PULSES - 1] = last;
        pilot
    };
    assert!(reads(&behind(ending(1768.0), 1067.0, 835.0, &h), &h));
    assert!(!reads(&behind(ending(PILOT), 1067.0, 835.0, &h), &h));
    let mut damaged = h.clone();
    damaged[18] ^= 0x01;
    assert!(!reads(&behind(ending(1768.0), 1067.0, 835.0, &damaged), &damaged));
}

#[test]
fn a_header_that_checks_out_half_a_bit_out_gives_way_to_the_pair_read_whole() {
    // Two channels mixed a few samples apart move the split a pair straddles, so most bytes
    // read right half a bit out. The sync pair's split moved, 1,100 / 302 T; two 0 bits before
    // a 1 straddle it, both bit 6 of a space turned into a '`', and the XOR checksum holds.
    let mut h = vec![0u8; 19];
    h[1] = 3;
    h[2..12].copy_from_slice(b"HALF  BIT ");
    h[12] = 0x0B;
    h[14] = 0xD0;
    h[16] = 0x80;
    h[18] = h[..18].iter().fold(0, |a, b| a ^ b);
    let bits: Vec<bool> = h.iter().flat_map(|&byte| (0..8).rev().map(move |k| byte >> k & 1 == 1)).collect();
    let late = |n: usize| (n / 8 == 6 || n / 8 == 7) && n % 8 == 1;
    let mut halves = Vec::new();
    for (n, &one) in bits.iter().enumerate() {
        halves.extend(match (one, n > 0 && bits[n - 1]) {
            (true, _) => [1600.0, 1820.0],
            (false, true) => [1100.0, 700.0],
            (false, false) if late(n) => [800.0, 1000.0],
            (false, false) => [860.0, 860.0],
        });
    }
    // Half a bit out, the last bit read from its first half alone, the bytes check out, and two
    // of them are wrong.
    let mut shifted: Vec<f64> = halves[1..].to_vec();
    shifted.push(halves[halves.len() - 1]);
    let out: Vec<u8> = shifted
        .chunks(16)
        .map(|byte| byte.chunks(2).fold(0, |b, pair| b << 1 | u8::from(pair[0] + pair[1] > 2510.0)))
        .collect();
    assert_eq!(&out[6..8], b"``");
    assert_eq!(out.iter().fold(0, |a, b| a ^ b), 0);
    let mut t = vec![PILOT; PILOT_PULSES];
    t.extend([1100.0, 302.0]);
    t.extend(&halves);
    t.push(3_500_000.0);
    assert_eq!(read(&t), h);
    assert!(merged(&t));
}

#[test]
fn a_glitch_inside_a_pilot_is_no_sync() {
    // A pilot cut off by the end of the recording, with a glitch inside it: one pulse as long as a
    // pilot half and a sync pair (3,570 T), which a backward merge predicts, then a half too long
    // and one just outside the pilot's 10%. The pilot runs on behind them, and its pairs would
    // read as 1 bits, a byte of 0xFF; a block cannot start inside a pilot.
    let mut t = vec![PILOT; PILOT_PULSES];
    t.extend([PILOT + SYNC1 + SYNC2, 2630.0, 2268.0, 0.89 * PILOT]);
    t.extend(vec![PILOT; 400]);
    t.push(3_500_000.0);
    let got = decode(&t);
    assert!(!got.blocks.iter().any(|b| matches!(b.body, Body::Standard { .. })), "{:?}", got.findings);
    // Behind a real block, the same pilot is the next block's, and the block stands.
    let h = header();
    let mut t = block(&h);
    t.pop();
    t.extend(vec![PILOT; 400]);
    t.push(3_500_000.0);
    assert_eq!(read(&t), h);
}

/// A backward merge 15% off its prediction, past DECIDE and within ASK, so the readings answer.
/// The pair read whole starts half a bit on, straddling the header's bits into a 20-byte
/// reading (a CODE header's type reads 7, and a tape copier's 0x00 behind the header makes it
/// 20 bytes, no header's shape), its XOR checksum held by some choice of parameters; the backward
/// reading, a header in every field, outranks it.
#[test]
fn a_header_outranks_a_reading_half_a_bit_out_whose_checksum_holds() {
    let straddled_xor = |h: &[u8]| {
        let bits: Vec<u8> =
            h.iter().chain(&[0u8]).flat_map(|&b| (0..8).rev().map(move |k| b >> k & 1)).collect();
        let g: Vec<u8> = (0..160).map(|k| bits[k] | bits.get(k + 1).copied().unwrap_or(0)).collect();
        g.chunks(8).map(|byte| byte.iter().fold(0, |b, &bit| b << 1 | bit)).fold(0u8, |x, b| x ^ b)
    };
    let mut h = header();
    h[1] = 3;
    let mut found = false;
    'search: for p1 in 0..=255u8 {
        for p3 in 0..=255u8 {
            h[14] = p1;
            h[16] = p3;
            h[18] = h[..18].iter().fold(0, |a, b| a ^ b);
            if straddled_xor(&h) == 0 {
                found = true;
                break 'search;
            }
        }
    }
    assert!(found, "no parameters keep the straddled checksum");
    let mut t = vec![PILOT; PILOT_PULSES];
    t.push(0.85 * (PILOT + SYNC1 + SYNC2));
    for byte in h.iter().chain(&[0u8]) {
        for k in (0..8).rev() {
            let half = if byte >> k & 1 == 1 { ONE } else { ZERO };
            t.extend([half, half]);
        }
    }
    t.push(3_500_000.0);
    let got = decode(&t);
    let datas: Vec<&Vec<u8>> = got
        .blocks
        .iter()
        .filter_map(|b| match &b.body {
            Body::Standard { data, .. } => Some(data),
            _ => None,
        })
        .collect();
    assert_eq!(datas, [&h], "{:?}", got.findings);
    assert!(got.findings.contains(&Finding::StrayZero { block: 0 }), "{:?}", got.findings);
    assert!(merged(&t));
}
