// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Pulses from T-state lengths, from CSW runs and from tapeti blocks, and the test distortions.

mod common;

use common::{clicks, jitter, lengths, pulses, speed, wobble};
use tapeti_core::types::{Block, Body};
use tapeti_decode::pulses::{from_runs, from_tstates, of_blocks, NotInflated, Pulse};

fn block(body: Body) -> Block {
    Block::new(body)
}

#[test]
fn t_state_lengths_are_placed_one_after_the_other() {
    let p = from_tstates(&[2168, 667, 735]);
    assert_eq!(
        p,
        [
            Pulse { tstates: 2168.0, start: 0.0 },
            Pulse { tstates: 667.0, start: 2168.0 },
            Pulse { tstates: 735.0, start: 2835.0 }
        ]
    );
}

#[test]
fn csw_runs_become_t_states_and_keep_their_place_in_samples() {
    // At 35,000 Hz a sample is exactly 100 T-states.
    let p = from_runs(&[3, 5, 300], 35000);
    assert_eq!(
        p,
        [
            Pulse { tstates: 300.0, start: 0.0 },
            Pulse { tstates: 500.0, start: 3.0 },
            Pulse { tstates: 30000.0, start: 8.0 }
        ]
    );
    let p = from_runs(&[1], 44100);
    assert!((p[0].tstates - 3_500_000.0 / 44100.0).abs() < 1e-9);
}

#[test]
fn a_standard_block_plays_its_pilot_sync_bits_and_pause() {
    let blocks = [block(Body::Standard { pause: 1000, data: vec![0x00, 0xFF] })];
    let p = lengths(&of_blocks(&blocks).unwrap());
    // A header's pilot (flag below 128) is 8,063 pulses. 8,097 pulses end on the high level,
    // so the pause first finishes that level with 1 ms, then holds low for the rest.
    let mut want = vec![2168; 8063];
    want.extend([667, 735]);
    want.extend([855; 16]);
    want.extend([1710; 16]);
    want.extend([3500, 3_496_500]);
    assert_eq!(p, want);
}

#[test]
fn a_direct_recording_plays_its_runs_and_a_pause_at_the_same_level_joins_the_last() {
    // Bits 111 000 111 0000000 at 79 T-states each, starting from the low level.
    let direct = Body::Direct { tstates: 79, pause: 0, used_bits: 8, data: vec![0b1110_0011, 0b1000_0000] };
    assert_eq!(lengths(&of_blocks(&[block(direct.clone())]).unwrap()), [237, 237, 237, 553]);
    let then_pause = [block(direct), block(Body::Pause { pause: 1 })];
    assert_eq!(lengths(&of_blocks(&then_pause).unwrap()), [237, 237, 237, 553 + 3500]);
}

#[test]
fn a_csw_block_plays_its_runs() {
    let csw = Body::Csw { pause: 0, sample_rate: 35000, compression: 1, pulse_count: 2, data: vec![3, 5] };
    assert_eq!(lengths(&of_blocks(&[block(csw)]).unwrap()), [300, 500]);
}

#[test]
fn a_z_rle_block_is_refused_until_it_is_inflated() {
    let zrle =
        Body::Csw { pause: 0, sample_rate: 44100, compression: 2, pulse_count: 2, data: vec![0x78, 0x9C] };
    let blocks = [block(Body::Pause { pause: 10 }), block(zrle)];
    let refused = of_blocks(&blocks).unwrap_err();
    assert_eq!(refused, NotInflated { block: 1 });
    assert_eq!(refused.to_string(), "block 1 is a Z-RLE CSW block whose data has not been inflated");
}

#[test]
fn a_pulse_of_no_length_is_two_edges_at_once_and_leaves_one_pulse() {
    let seq = block(Body::PulseSeq { pulses: vec![100, 0, 200] });
    assert_eq!(lengths(&of_blocks(&[seq]).unwrap()), [300]);
}

#[test]
fn a_speed_change_scales_every_pulse() {
    let p = speed(&pulses(&[1000, 2000]), 1.5);
    assert_eq!(p, [Pulse { tstates: 1500.0, start: 0.0 }, Pulse { tstates: 3000.0, start: 1500.0 }]);
}

#[test]
fn wobble_swings_the_speed_within_its_depth() {
    let clean = pulses(&[2168; 2000]);
    let wobbly = wobble(&clean, 0.03, 500_000.0);
    let ratios: Vec<f64> = wobbly.iter().map(|p| p.tstates / 2168.0).collect();
    assert!(ratios.iter().all(|r| (0.97 - 1e-12..=1.03 + 1e-12).contains(r)));
    assert!(ratios.iter().any(|r| *r > 1.029) && ratios.iter().any(|r| *r < 0.971));
    let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
    assert!((mean - 1.0).abs() < 0.002, "a wobble neither speeds up nor slows down the tape: {mean}");
}

#[test]
fn jitter_stays_within_its_amount_and_repeats_with_its_seed() {
    let clean = pulses(&[855; 1000]);
    let a = jitter(&clean, 50.0, 7);
    assert!(a.iter().all(|p| (805.0..=905.0).contains(&p.tstates)));
    assert_eq!(a, jitter(&clean, 50.0, 7));
    assert_ne!(a, jitter(&clean, 50.0, 8));
}

#[test]
fn a_click_splits_a_pulse_in_three_and_keeps_its_length() {
    let clean = pulses(&[1710; 10]);
    let clicked = clicks(&clean, 5, 40.0, 3);
    assert_eq!(clicked.len(), 14, "two of the ten pulses split into three");
    let total: f64 = clicked.iter().map(|p| p.tstates).sum();
    assert!((total - 17100.0).abs() < 1e-6);
    assert_eq!(clicked[5].tstates, 40.0);
    let last = clicked.last().unwrap();
    assert!((last.start + last.tstates - 17100.0).abs() < 1e-6, "the pulses stay end to end");
}
