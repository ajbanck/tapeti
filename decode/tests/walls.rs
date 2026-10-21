// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The decoder's walls: for each defect of the cassette chain (`common::degrade`), the worst it
//! reads a tape whole through, and what it makes of the tape one step past that.
//!
//! Each wall is two constants, the last step of its sweep read whole and the first not, asserted
//! by a fast test together with what the loss is: which blocks are missing and how the decode
//! says so. Never a block whose checksum holds over bytes the tape does not have, unless the
//! decode says a problem of it: a block cut short, its last byte filled out with 0s, can check
//! out over the 0s. Beside each
//! fast test, ignored because it takes minutes unoptimised, is the sweep that found the wall: it
//! checks every step, whole up to the wall and no wrong bytes anywhere
//! (`cargo test --release -p tapeti-decode --test walls -- --ignored`). At every step of either
//! the ROM's loader runs over the same pulses, and a block it loads at every phase that framing
//! does not read is named beside the outcome. There is none up to any wall; past the 1 Hz wow and
//! the flutter on a slow tape, the ROM still loads what framing loses.

mod common;

use common::degrade::*;
use common::rom::unread;
use common::same_tape;
use std::fmt::Debug;
use std::sync::atomic::AtomicBool;
use tapeti_core::audio::{encode_wav, playback_order, render_tape, FlowOptions, RenderOptions};
use tapeti_core::types::{Block, Body};
use tapeti_decode::entry::{decode_file, decode_pulses, tape_band, Channels, Decoded, Finding};
use tapeti_decode::pulses::{of_blocks, Pulse};
use tapeti_decode::readers::Channel;
use tapeti_decode::waveform::edges::detect_pulses;
use tapeti_decode::waveform::filter::Filter;

const RATE: u32 = 44_100;

fn with_checksum(mut data: Vec<u8>) -> Vec<u8> {
    data.push(data.iter().fold(0, |x, &d| x ^ d));
    data
}

/// 256 bytes behind flag 0xFF: varied bytes, then a run of 0x00, whose short half-waves wear
/// first, and a run of 0xFF.
fn data_block() -> Block {
    let mut data = vec![0xFF];
    data.extend((0..224u32).map(|i| (i * 37 + 11) as u8));
    data.extend([0x00; 16]);
    data.extend([0xFF; 16]);
    Block::new(Body::Standard { pause: 1000, data: with_checksum(data) })
}

/// A header asking for the data block, and the data block.
fn tape() -> Vec<Block> {
    let mut h = vec![0x00, 0x03];
    h.extend(b"walls     ");
    h.extend(256u16.to_le_bytes());
    h.extend([0x00, 0x80, 0x00, 0x80]);
    vec![Block::new(Body::Standard { pause: 1000, data: with_checksum(h) }), data_block()]
}

/// The data block alone: what a loader's own headerless block is.
fn headerless() -> Vec<Block> {
    vec![data_block()]
}

/// The flag and the length behind it that a `LOAD` asks for, block by block.
fn asks(blocks: &[Block]) -> Vec<(u8, usize)> {
    blocks
        .iter()
        .map(|b| match &b.body {
            Body::Standard { data, .. } => (data[0], data.len() - 2),
            _ => unreachable!(),
        })
        .collect()
}

/// `blocks` as the Spectrum saves them, a square wave of ±0.5 at [`RATE`].
fn render(blocks: &[Block]) -> Vec<f32> {
    let order = playback_order(blocks, FlowOptions::default());
    render_tape(blocks, RenderOptions { sample_rate: RATE, mic: false, amplitude: 0.5 }, &order)
}

/// The sample where block `k` of `blocks` starts in [`render`].
fn block_at(blocks: &[Block], k: usize) -> usize {
    render(&blocks[..k]).len()
}

/// The sample where the bits of block `k` of `blocks` start: behind a data block's 3,223 pilot
/// pulses and the sync pair.
fn bits_at(blocks: &[Block], k: usize) -> usize {
    block_at(blocks, k) + ((3223.0 * 2168.0 + 667.0 + 735.0) * f64::from(RATE) / 3_500_000.0) as usize
}

/// `ms` milliseconds in samples at [`RATE`].
fn ms(ms: f64) -> usize {
    (ms * f64::from(RATE) / 1000.0).round() as usize
}

fn decode(wav: &[u8], channels: Channels) -> Decoded {
    let no_zlib = |_: &[u8]| -> Result<Vec<u8>, String> { Err("none".into()) };
    decode_file("t.wav", wav, channels, Filter::None, None, &no_zlib, &mut |_| {}, &AtomicBool::new(false))
        .unwrap()
}

/// What a decode reports about the blocks it read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Said {
    BadChecksum,
    Short,
    RawSignal,
    /// Pulses played as a pause.
    Collapsed,
    /// Pulses played as a pause, a pilot the ROM locks on among them.
    CollapsedPilot,
    /// Pulses played as a pause, or left out with the lead-in, that hold a block that did not
    /// read: a pilot with signal behind it, or the rest of a block that stopped short.
    Lost,
    /// A last byte filled out with 0s past the bits read, which make its checksum hold.
    FilledOut,
}

/// A decode against the tape.
#[derive(Debug, PartialEq)]
enum Outcome {
    /// Every block with its bytes and nothing else, every pause as long within 3%.
    Whole,
    /// Every block with its bytes and nothing else, a pause longer or shorter.
    Pause,
    /// The tape's blocks at these indices not read, and what the decode said, of [`Said`].
    Lost(Vec<usize>, Vec<Said>),
    /// A block whose checksum holds, with bytes the tape does not have, of which the decode says
    /// no problem.
    Wrong,
}

fn standard(b: &Block) -> Option<&Vec<u8>> {
    match &b.body {
        Body::Standard { data, .. } => Some(data),
        _ => None,
    }
}

/// `got` against `want` played at `speed`, the least and most it ran at.
///
/// A pause is measured to the next block's first edge, and a pilot rising out of silence through
/// worn treble, hiss or ripple shows its first half-waves a few milliseconds late: up to 26 ms
/// in these walls, which nothing that loads a tape notices; 3% is room for that.
fn outcome(got: &Decoded, want: &[Block], speed: (f64, f64)) -> Outcome {
    let wants: Vec<&Vec<u8>> = want.iter().filter_map(standard).collect();
    let checks = |d: &Vec<u8>| d.len() >= 2 && d.iter().fold(0, |x, &v| x ^ v) == 0;
    let said = |i: usize| got.findings.iter().any(|f| f.is_problem() && f.block() == Some(i));
    let wrong =
        |(i, b): (usize, &Block)| standard(b).is_some_and(|d| checks(d) && !wants.contains(&d) && !said(i));
    if got.blocks.iter().enumerate().any(wrong) {
        return Outcome::Wrong;
    }
    if same_tape(&got.blocks, want, 0.97 * speed.0..=1.03 * speed.1).is_ok() {
        return Outcome::Whole;
    }
    if same_tape(&got.blocks, want, 0.0..=f64::MAX).is_ok() {
        return Outcome::Pause;
    }
    let datas: Vec<&Vec<u8>> = got.blocks.iter().filter_map(standard).collect();
    let missing = (0..wants.len()).filter(|&k| !datas.contains(&wants[k])).collect();
    let mut said: Vec<Said> = got
        .findings
        .iter()
        .filter_map(|f| match f {
            Finding::BadChecksum { .. } => Some(Said::BadChecksum),
            Finding::Short { .. } => Some(Said::Short),
            Finding::RawSignal { .. } => Some(Said::RawSignal),
            Finding::Collapsed { pilot: false, .. } => Some(Said::Collapsed),
            Finding::Collapsed { pilot: true, .. } => Some(Said::CollapsedPilot),
            Finding::Lost { .. } => Some(Said::Lost),
            Finding::PartialFinalByte { completes: true, .. } => Some(Said::FilledOut),
            _ => None,
        })
        .collect();
    said.sort();
    said.dedup();
    Outcome::Lost(missing, said)
}

/// A decode's [`Outcome`], and the blocks the ROM's loader loads at every phase from the same
/// pulses that framing does not read from them ([`unread`]): none wherever a wall stands, and
/// past it a disagreement the wall records.
type Judged = (Outcome, Vec<usize>);

const WHOLE: Judged = (Outcome::Whole, Vec::new());

/// What the ROM's loader loads from `pulses`, whose starts count samples at `rate`, that framing
/// does not read.
fn rom_only(pulses: &[Pulse], rate: u32, want: &[Block]) -> Vec<usize> {
    unread(pulses, rate, &asks(want))
}

/// The decode of `x`, a mono recording at `rate` and `bits`, against `want` played at `speed`.
fn judge_at(x: &[f32], rate: u32, bits: u16, want: &[Block], speed: (f64, f64)) -> Judged {
    let heard = quantise(x, bits);
    let rom = rom_only(&detect_pulses(|| heard.iter().copied(), rate, Filter::None).unwrap(), rate, want);
    (outcome(&decode(&encode_wav(x, rate, bits), Channels::Only(Channel::Mix)), want, speed), rom)
}

/// [`judge_at`] at [`RATE`] and 16 bits.
fn judge(x: &[f32], want: &[Block], speed: (f64, f64)) -> Judged {
    judge_at(x, RATE, 16, want, speed)
}

/// A wall's fast test: whole at `whole`, `lost` at `loss`.
fn holds<T: Copy + Debug>(whole: T, loss: T, lost: Judged, at: impl Fn(T) -> Judged) {
    assert_eq!(at(whole), WHOLE, "at {whole:?}");
    assert_eq!(at(loss), lost, "at {loss:?}");
}

/// A wall's sweep: whole at every step of `grid` up to `whole`, `lost` at the next, `loss`, and
/// no wrong bytes at any step past it.
fn sweep<T: Copy + Debug + PartialEq>(grid: &[T], whole: T, loss: T, lost: Judged, at: impl Fn(T) -> Judged) {
    let w = grid.iter().position(|&v| v == whole).unwrap();
    assert_eq!(grid.get(w + 1), Some(&loss));
    for (k, &v) in grid.iter().enumerate() {
        let got = at(v);
        println!("{v:?}: {got:?}");
        match k {
            _ if k <= w => assert_eq!(got, WHOLE, "at {v:?}"),
            _ if k == w + 1 => assert_eq!(got, lost, "at {v:?}"),
            _ => assert_ne!(got.0, Outcome::Wrong, "at {v:?}"),
        }
    }
}

fn same(_: impl Copy) -> (f64, f64) {
    (1.0, 1.0)
}

// Treble loss.

/// The tape through `microns` of spacing between tape and head.
fn treble(microns: f64) -> Judged {
    judge(&spacing_loss(&render(&tape()), RATE, microns), &tape(), same(microns))
}

/// Spacing loss takes the short half-waves first; from 11 µm the header reads only on the fade
/// band, and so does the data block it then asks for. At 12 µm the header reads on neither
/// band, its pilot and bits left in the lead-in, counted and not said lost, and the data block
/// is kept as raw signal. A good deck keeps about 1 µm, a worn one a few, a copy of a copy about
/// 12.
const TREBLE_WHOLE_UM: f64 = 11.5;
const TREBLE_LOSS_UM: f64 = 12.0;
const TREBLE_GRID_UM: [f64; 16] =
    [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 10.5, 11.0, 11.5, 12.0, 12.5, 13.0];

fn treble_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::RawSignal]), Vec::new())
}

#[test]
fn treble_loss_reads_whole_to_eleven_and_a_half_microns() {
    holds(TREBLE_WHOLE_UM, TREBLE_LOSS_UM, treble_lost(), treble);
}

#[test]
#[ignore]
fn treble_loss_sweep() {
    sweep(&TREBLE_GRID_UM, TREBLE_WHOLE_UM, TREBLE_LOSS_UM, treble_lost(), treble);
}

// Treble loss through the band-pass.

/// The decode of `x` through the band-pass the apps offer ([`tape_band`]), against `want`
/// played at `speed`, and what the ROM's loader loads at every phase from the same filtered
/// pulses that framing does not read.
fn judge_filtered(x: &[f32], want: &[Block], speed: (f64, f64)) -> Judged {
    let no_zlib = |_: &[u8]| -> Result<Vec<u8>, String> { Err("none".into()) };
    let rom = rom_only(&detect_pulses(|| x.iter().copied(), RATE, tape_band(RATE)).unwrap(), RATE, want);
    let wav = encode_wav(x, RATE, 16);
    let got = decode_file(
        "t.wav",
        &wav,
        Channels::Only(Channel::Mix),
        tape_band(RATE),
        None,
        &no_zlib,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    (outcome(&got, want, speed), rom)
}

/// The tape through `microns` of spacing, decoded through the band-pass.
fn filtered_treble(microns: f64) -> Judged {
    judge_filtered(&spacing_loss(&render(&tape()), RATE, microns), &tape(), same(microns))
}

/// The band-pass rings, merging the sync pair and the header's first 0 half into one pulse
/// under the pilot window's top (`DECIDE`). It reads whole to 11 µm; at 11.5 the data block's
/// run of 0x00 bytes merges, its read runs short, and the header is read on the fade band.
const FILTERED_TREBLE_WHOLE_UM: f64 = 11.0;
const FILTERED_TREBLE_LOSS_UM: f64 = 11.5;

fn filtered_treble_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Short, Said::Collapsed]), Vec::new())
}

#[test]
fn treble_loss_through_the_band_pass_reads_whole_to_eleven_microns() {
    holds(FILTERED_TREBLE_WHOLE_UM, FILTERED_TREBLE_LOSS_UM, filtered_treble_lost(), filtered_treble);
}

#[test]
#[ignore]
fn filtered_treble_sweep() {
    sweep(
        &TREBLE_GRID_UM,
        FILTERED_TREBLE_WHOLE_UM,
        FILTERED_TREBLE_LOSS_UM,
        filtered_treble_lost(),
        filtered_treble,
    );
}

// Hiss.

fn hissed(db: u32) -> Judged {
    judge(&hiss(&render(&tape()), db, 0x5EED), &tape(), same(db))
}

/// Hiss moves every crossing and cuts false edges into weak lobes; the band rides over it down to
/// 10 dB under the signal (the tape's level over its pauses and all), the false edges in a pilot
/// passed over as glitches. At 9 dB the data block comes back with a bad checksum. Dead air
/// either side leaves the wall within a dB (`dead_air_takes_the_noise_line_out_of_the_signal`).
/// A clean recording lies 34 dB or more over its hiss; a noisy one 14 to 26.
const HISS_WHOLE_DB: u32 = 10;
const HISS_LOSS_DB: u32 = 9;

fn hiss_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), Vec::new())
}

#[test]
fn hiss_reads_whole_to_ten_db() {
    holds(HISS_WHOLE_DB, HISS_LOSS_DB, hiss_lost(), hissed);
}

#[test]
#[ignore]
fn hiss_sweep() {
    let grid: Vec<u32> = (0..=30).rev().collect();
    sweep(&grid, HISS_WHOLE_DB, HISS_LOSS_DB, hiss_lost(), hissed);
}

// Short dropouts.

/// The tape `blocks` with `len` ms at nothing, half a second into the bits of its last block.
fn dropped(blocks: &[Block], len: f64) -> Judged {
    let at = bits_at(blocks, blocks.len() - 1) + ms(500.0);
    judge(&dropout(&render(blocks), at, ms(len), 0.0), blocks, same(len))
}

/// A dropout under the envelope's ±10 ms lets no edge through, and one long pulse ends the
/// block. Behind a header, the midline still finds the half-waves either side of a hole up to
/// 1.25 ms; from 1.5 ms the block stops there, short of the length asked for with a bad checksum,
/// and the rest of its bits play as its pause, said as lost. A hole is 2.1 ms for every 0.1 mm of tape a
/// speck of oxide or a crease takes away.
const DROPOUT_WHOLE_MS: f64 = 1.25;
const DROPOUT_LOSS_MS: f64 = 1.5;
/// Without a header to say how long the block is, a hole of 1 ms already ends it.
const HEADERLESS_DROPOUT_WHOLE_MS: f64 = 0.75;
const HEADERLESS_DROPOUT_LOSS_MS: f64 = 1.0;
const DROPOUT_GRID_MS: [f64; 12] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 5.0, 10.0, 20.0, 40.0];

fn dropout_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Short, Said::Lost]), Vec::new())
}

fn headerless_dropout_lost() -> Judged {
    (Outcome::Lost(vec![0], vec![Said::BadChecksum]), Vec::new())
}

#[test]
fn a_dropout_behind_a_header_reads_whole_to_a_millisecond_and_a_quarter() {
    holds(DROPOUT_WHOLE_MS, DROPOUT_LOSS_MS, dropout_lost(), |len| dropped(&tape(), len));
}

#[test]
fn a_dropout_in_a_headerless_block_reads_whole_to_three_quarters_of_a_millisecond() {
    holds(HEADERLESS_DROPOUT_WHOLE_MS, HEADERLESS_DROPOUT_LOSS_MS, headerless_dropout_lost(), |len| {
        dropped(&headerless(), len)
    });
}

#[test]
#[ignore]
fn dropout_sweeps() {
    sweep(&DROPOUT_GRID_MS, DROPOUT_WHOLE_MS, DROPOUT_LOSS_MS, dropout_lost(), |len| dropped(&tape(), len));
    sweep(
        &DROPOUT_GRID_MS,
        HEADERLESS_DROPOUT_WHOLE_MS,
        HEADERLESS_DROPOUT_LOSS_MS,
        headerless_dropout_lost(),
        |len| dropped(&headerless(), len),
    );
}

/// The sample where the header's bits start: behind its 8,063 pilot pulses and the sync pair.
fn header_bits_at() -> usize {
    block_at(&tape(), 0) + ((8063.0 * 2168.0 + 667.0 + 735.0) * f64::from(RATE) / 3_500_000.0) as usize
}

/// The tape with its signal sunk to `depth` of its level for `len` ms from sample `at`.
fn sunk(at: usize, len: f64, depth: f32) -> Judged {
    judge(&dropout(&render(&tape()), at, ms(len), depth), &tape(), same(len))
}

/// A dropout that sinks the signal to a tenth for 20 ms rather than taking it away: the ordinary
/// band, held up for 10 ms by the signal in front, cuts no edge in the first of it, and the
/// block stops there; the fade band follows the level down. A header is read again on it as the
/// block behind one is, since the ROM asks for a header by its flag, 19 bytes. From 2 to 10 ms
/// neither band reads it, the in-block dip no envelope window follows (a known limit).
#[test]
fn a_header_sunk_in_a_dropout_is_read_again_on_the_fade_band_as_the_block_behind_it_is() {
    for (k, at) in [header_bits_at() + ms(20.0), bits_at(&tape(), 1) + ms(500.0)].into_iter().enumerate() {
        assert_eq!(sunk(at, 20.0, 0.1), WHOLE, "block {k}");
        assert!(matches!(sunk(at, 5.0, 0.1).0, Outcome::Lost(ref lost, _) if *lost == [k]), "block {k}");
    }
}

/// A dropout that sinks the data block's sync or its flag byte for 20 ms: the ordinary band
/// reads no block there at all, and the header, which nothing then answers, is owed its block
/// from where it ends, read on the fade band.
#[test]
fn a_block_sunk_at_its_sync_is_read_on_the_fade_band_behind_its_header() {
    let bits = bits_at(&tape(), 1);
    for at in [bits - ms((667.0 + 735.0) / 3500.0), bits + ms(3.0)] {
        assert_eq!(sunk(at - ms(10.0), 20.0, 0.1), WHOLE);
    }
}

/// A dropout that sinks the header's sync or its flag byte: the ordinary band reads no header,
/// and the data block behind it is data no header asks for. The ROM's pilot in front of it, with
/// bits behind it, is owed a header, read on the fade band and taken because the data block
/// answers it. At 5 ms the sink is the in-block dip neither band follows (a known limit), and a
/// hole at nothing takes the bits away at any band: the header is kept as raw signal, or said
/// lost.
#[test]
fn a_header_sunk_at_its_sync_is_read_on_the_fade_band_in_front_of_its_block() {
    let sync = header_bits_at() - ms((667.0 + 735.0) / 3500.0);
    for (at, len) in [(sync, 20.0), (header_bits_at() + ms(3.0), 20.0), (sync, 10.0), (sync, 30.0)] {
        assert_eq!(sunk(at - ms(len / 2.0), len, 0.1), WHOLE, "{at}, {len} ms");
    }
    let raw = (Outcome::Lost(vec![0], vec![Said::RawSignal]), Vec::new());
    assert_eq!(sunk(sync - ms(2.5), 5.0, 0.1), raw);
    assert_eq!(sunk(sync - ms(10.0), 20.0, 0.0), (Outcome::Lost(vec![0], vec![Said::Lost]), Vec::new()));
}

// Fade.

/// The tape with hiss 50 dB under it and its data block fading over its first second to `depth`
/// of the level, and staying there.
fn faded(depth: f32) -> Judged {
    let clean = render(&tape());
    let noise: Vec<f32> = hiss(&clean, 50, 0xF00D).iter().zip(&clean).map(|(a, b)| a - b).collect();
    let from = block_at(&tape(), 1);
    let x: Vec<f32> =
        fade(&clean, from, from + ms(1000.0), depth).iter().zip(&noise).map(|(a, b)| a + b).collect();
    judge(&x, &tape(), same(depth))
}

/// The edge detector's floor is a share of the loudest material within seconds, so a block that
/// falls under it so soon after a loud one is taken for dead air. The fade band, which follows
/// the level down, reads it to 0.04; at 0.03, 20 dB over the hiss, it plays as the header's
/// pause, its pilot said among the pause's pulses but not that a block was lost, since its bits
/// leave no edges. A quiet program on a collection tape can measure 0.07 of the loud ones around
/// it.
const FADE_WHOLE: f32 = 0.04;
const FADE_LOSS: f32 = 0.03;
const FADE_GRID: [f32; 14] = [0.5, 0.3, 0.2, 0.15, 0.12, 0.1, 0.09, 0.08, 0.07, 0.06, 0.05, 0.04, 0.03, 0.02];

fn fade_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::CollapsedPilot]), Vec::new())
}

#[test]
fn a_block_fading_after_a_loud_one_reads_whole_to_four_hundredths() {
    holds(FADE_WHOLE, FADE_LOSS, fade_lost(), faded);
}

#[test]
#[ignore]
fn fade_sweep() {
    sweep(&FADE_GRID, FADE_WHOLE, FADE_LOSS, fade_lost(), faded);
}

// Wow and flutter.

fn wowed(hz: f64, depth: f64) -> Judged {
    judge(&wow(&render(&tape()), RATE, depth, hz), &tape(), (1.0 - depth, 1.0 + depth))
}

/// The speed swinging within a block moves its pairs towards the split, which one speed measured
/// on the leader does not follow; a 1 bit's pair sits 36% over the split and a 0's 32% under,
/// and the block is lost past a swing of ±22% at 1 Hz and ±26% at 10 Hz, where swings that take
/// the leader's pairs out of its window leave runs of 16 pairs between them to lock on. A
/// capstan's wow is 0.1 to 0.3%, a pinch roller's at 1 to 2 Hz about as much: the walls stand far
/// over any deck.
const WOW_1HZ_WHOLE: f64 = 0.22;
const WOW_1HZ_LOSS: f64 = 0.23;
const WOW_10HZ_WHOLE: f64 = 0.26;
const WOW_10HZ_LOSS: f64 = 0.27;
const WOW_GRID: [f64; 15] =
    [0.01, 0.02, 0.04, 0.06, 0.08, 0.1, 0.12, 0.15, 0.2, 0.21, 0.22, 0.23, 0.25, 0.26, 0.27];

/// The ROM's own windows, fixed, load the data block at 23%: its windows measured on the leader
/// are what loses it. The leader, at its moment in the swing, measures the tape 6.4% slow, and at
/// the swing's fast end a 1 bit's pair is 2,633 T: over the ROM's split, 2,510 T, and under the
/// split at the leader's speed, 2,671 T.
fn wow_1hz_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), vec![1])
}

fn wow_10hz_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::RawSignal, Said::Collapsed]), Vec::new())
}

#[test]
fn wow_reads_whole_to_twenty_two_percent_at_1_hz() {
    holds(WOW_1HZ_WHOLE, WOW_1HZ_LOSS, wow_1hz_lost(), |d| wowed(1.0, d));
}

#[test]
fn flutter_reads_whole_to_twenty_six_percent_at_10_hz() {
    holds(WOW_10HZ_WHOLE, WOW_10HZ_LOSS, wow_10hz_lost(), |d| wowed(10.0, d));
}

#[test]
#[ignore]
fn wow_sweeps() {
    sweep(&WOW_GRID, WOW_1HZ_WHOLE, WOW_1HZ_LOSS, wow_1hz_lost(), |d| wowed(1.0, d));
    sweep(&WOW_GRID, WOW_10HZ_WHOLE, WOW_10HZ_LOSS, wow_10hz_lost(), |d| wowed(10.0, d));
}

// Snags.

/// The tape with a snag in its data block, as the one measured on a recording had it: from
/// 400 ms into the block's bits the tape slows over 70 ms to `depth` of its speed, holds there
/// for 15 ms and is let go over 15.
fn snagged(depth: f64) -> Judged {
    let at = bits_at(&tape(), 1) + ms(400.0);
    judge(&snag(&render(&tape()), at, ms(70.0), ms(15.0), ms(15.0), depth), &tape(), same(depth))
}

/// Past 1.47 of the pilot's speed a 0 bit's pair lies past the split the pilot gives, and read
/// at that speed the block was lost from 1.45 (its slowest bits on the sample grid). Read again
/// at the speed of the bits before each pair, it reads to 1.58: at 1.59 a 1 bit whose two
/// halves both fall long on the sample grid pairs to 5,552 T, past the bit loop's timeout,
/// where the ROM stops too. The snag measured on a recording reached 1.55.
const SNAG_WHOLE: f64 = 1.58;
const SNAG_LOSS: f64 = 1.59;
const SNAG_GRID: [f64; 11] = [1.1, 1.2, 1.3, 1.4, 1.45, 1.5, 1.55, 1.58, 1.59, 1.65, 1.8];

fn snag_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Short, Said::Lost]), Vec::new())
}

#[test]
fn a_snag_reads_whole_to_158_percent_of_the_speed() {
    holds(SNAG_WHOLE, SNAG_LOSS, snag_lost(), snagged);
}

#[test]
#[ignore]
fn snag_sweep() {
    sweep(&SNAG_GRID, SNAG_WHOLE, SNAG_LOSS, snag_lost(), snagged);
}

// Clicks.

/// Where the signal changes sign in `x[from..to]`: the first sample of each new sign.
fn edges_in(x: &[f32], from: usize, to: usize) -> Vec<usize> {
    (from + 1..to).filter(|&i| (x[i] < 0.0) != (x[i - 1] < 0.0)).collect()
}

/// The places in the data block's bits clicks go: every 400th edge from the 100th, eight.
fn click_edges(clean: &[f32]) -> Vec<usize> {
    let at = bits_at(&tape(), 1);
    edges_in(clean, at, at + ms(1400.0)).into_iter().skip(100).step_by(400).take(8).collect()
}

/// Eight clicks of `width` samples back to the level before, a sample behind an edge in the data
/// block: where a bounce sits.
fn clicked_at_edges(width: usize) -> Judged {
    let clean = render(&tape());
    let x = click_edges(&clean).iter().fold(clean.clone(), |x, &e| spike(&x, e + 1, width, 0.5));
    judge(&x, &tape(), same(width))
}

/// A click a sample behind an edge leaves two short pulses between two half-waves, which fold
/// into a half-wave beside them, as the ROM, waiting 468 to 653 T after an edge before its first
/// read, never sees them, while the sample before the click and the click last together under
/// 485 T: to 5 samples, 476 T. At 6, 555 T, the block is lost, and the ROM loses it at some
/// phases too. A click is one or two samples of a crackle or a bad splice.
const CLICK_WHOLE_SAMPLES: usize = 5;
const CLICK_LOSS_SAMPLES: usize = 6;

fn click_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::RawSignal]), Vec::new())
}

/// Eight clicks of one sample in the middle of a half-wave of the data block, moving it `height`
/// twice over towards the other side.
fn clicked_inside(height: f32) -> Judged {
    let clean = render(&tape());
    let edges = edges_in(&clean, bits_at(&tape(), 1), clean.len());
    let mut x = clean.clone();
    for k in 0..8 {
        let i = (100 + 400 * k..).find(|&i| edges[i + 1] - edges[i] >= 20).unwrap();
        let mid = (edges[i] + edges[i + 1]) / 2;
        x[mid] -= x[mid].signum() * 2.0 * height;
    }
    judge(&x, &tape(), same(height))
}

/// Inside a half-wave a click splits it in three wherever it crosses zero, one sample wide or
/// more, and no fold can tell its pulses from a bit's: the block is lost. One that does not reach
/// the other side, up to 0.2 on a level of 0.5, is nothing.
const CLICK_INSIDE_WHOLE: f32 = 0.2;
const CLICK_INSIDE_LOSS: f32 = 0.3;

fn click_inside_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::RawSignal]), Vec::new())
}

#[test]
fn a_click_behind_an_edge_reads_whole_to_five_samples() {
    holds(CLICK_WHOLE_SAMPLES, CLICK_LOSS_SAMPLES, click_lost(), clicked_at_edges);
}

#[test]
fn a_click_inside_a_half_wave_reads_whole_while_it_stays_on_its_side() {
    holds(CLICK_INSIDE_WHOLE, CLICK_INSIDE_LOSS, click_inside_lost(), clicked_inside);
}

#[test]
#[ignore]
fn click_sweeps() {
    let grid: Vec<usize> = (1..=10).collect();
    sweep(&grid, CLICK_WHOLE_SAMPLES, CLICK_LOSS_SAMPLES, click_lost(), clicked_at_edges);
    let grid = [0.05, 0.1, 0.15, CLICK_INSIDE_WHOLE, CLICK_INSIDE_LOSS, 0.5];
    sweep(&grid, CLICK_INSIDE_WHOLE, CLICK_INSIDE_LOSS, click_inside_lost(), clicked_inside);
}

// Sample rate and depth.

/// The tape recorded at `rate`, each sample the mean over its time, at `bits`.
fn at_rate(rate: u32, bits: u16) -> Judged {
    judge_at(&downsample(&render(&tape()), RATE, rate), rate, bits, &tape(), (1.0, 1.0))
}

#[test]
fn eight_kilohertz_and_eight_bits_read_whole() {
    // A sync 1 is 1.5 samples at 8 kHz, and an 8-bit grid adds noise of 0.0023; an edge is
    // still placed between samples by how far each lies from the line.
    for rate in [22_050, 11_025, 8000] {
        for bits in [16, 8] {
            assert_eq!(at_rate(rate, bits), WHOLE, "{rate} Hz, {bits} bits");
        }
    }
}

#[test]
fn eight_bits_under_hiss_read_as_sixteen_do() {
    // The 8-bit grid's noise, 0.0023, lies 46 dB under the tape's level: the hiss wall is the
    // same, whole at 10 dB and the data block bad at 9.
    let at = |db: u32| judge_at(&hiss(&render(&tape()), db, 0x5EED), RATE, 8, &tape(), (1.0, 1.0));
    holds(HISS_WHOLE_DB, HISS_WHOLE_DB - 1, hiss_lost(), at);
}

/// Under a sample to a sync 1 and two to a 0 bit's half the half-waves still read down to
/// 4,410 Hz; at 4,000 Hz both blocks come back three bytes long with bad checksums, the rest of
/// each said as lost. Sound cards record at 8 kHz at the least, and 11 kHz is the sensible floor
/// for a tape.
const RATE_WHOLE_HZ: u32 = 4410;
const RATE_LOSS_HZ: u32 = 4000;

fn rate_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::BadChecksum, Said::Lost]), Vec::new())
}

#[test]
fn the_sample_rate_reads_whole_down_to_4410_hz() {
    holds(RATE_WHOLE_HZ, RATE_LOSS_HZ, rate_lost(), |rate| at_rate(rate, 16));
}

#[test]
#[ignore]
fn rate_sweep() {
    let grid = [8000, 7000, 6000, 5512, 5000, RATE_WHOLE_HZ, RATE_LOSS_HZ];
    sweep(&grid, RATE_WHOLE_HZ, RATE_LOSS_HZ, rate_lost(), |rate| at_rate(rate, 16));
}

/// The tape resampled to `rate` by a careful resampler, cut sharply at 0.45 of it
/// (`resample_sharp`), at `bits`.
fn at_sharp_rate(rate: u32, bits: u16) -> Judged {
    judge_at(&resample_sharp(&render(&tape()), RATE, rate), rate, bits, &tape(), (1.0, 1.0))
}

/// A careful resampler cuts at 0.45 of its rate, and a 0 bit's half-waves, 2,047 Hz a cycle,
/// pass while the cut lies over them: to 5,000 Hz, a cut at 2,250 Hz. At 4,410 Hz the cut,
/// 1,985 Hz, takes them, and both blocks come back with bad checksums, the rest of each said
/// lost, at 8 bits as at 16. The mean over each sample's time reads at 4,410 Hz, since it rolls
/// off slowly past its cut.
const SHARP_RATE_WHOLE_HZ: u32 = 5000;
const SHARP_RATE_LOSS_HZ: u32 = 4410;

#[test]
fn a_sharp_resampler_reads_whole_down_to_5000_hz() {
    holds(SHARP_RATE_WHOLE_HZ, SHARP_RATE_LOSS_HZ, rate_lost(), |rate| at_sharp_rate(rate, 16));
    holds(SHARP_RATE_WHOLE_HZ, SHARP_RATE_LOSS_HZ, rate_lost(), |rate| at_sharp_rate(rate, 8));
}

#[test]
#[ignore]
fn sharp_rate_sweep() {
    let grid = [22_050, 11_025, 8000, 6000, 5512, SHARP_RATE_WHOLE_HZ, SHARP_RATE_LOSS_HZ, 4000, 3500];
    sweep(&grid, SHARP_RATE_WHOLE_HZ, SHARP_RATE_LOSS_HZ, rate_lost(), |rate| at_sharp_rate(rate, 16));
}

// Confirmations: defects the design reads through, and two it does not.

#[test]
fn clipping_moves_no_crossing() {
    // Rounded by 3 µm of spacing first, so that there is a top to cut; at 16 times the level
    // every half-wave is a flat top and still crosses where it did.
    let dull = spacing_loss(&render(&tape()), RATE, 3.0);
    assert_eq!(judge(&clip(&dull, 16.0), &tape(), (1.0, 1.0)), WHOLE);
}

#[test]
fn a_recording_upside_down_reads_whole() {
    // A reader of pairs does not see the polarity; the first edge out of silence and the 1 ms
    // level a block's pause opens with are the other way up, and the pauses still measure.
    assert_eq!(judge(&invert(&render(&tape())), &tape(), (1.0, 1.0)), WHOLE);
    let order = playback_order(&tape(), FlowOptions::default());
    let mic = render_tape(&tape(), RenderOptions { sample_rate: RATE, mic: true, amplitude: 0.5 }, &order);
    assert_eq!(judge(&invert(&mic), &tape(), (1.0, 1.0)), WHOLE);
}

/// Ripple on the tops never crosses zero and the band's hysteresis ignores it, until the tops
/// dip to 0.05 of the level, the floor's own share of the peak: at a depth of 0.95 the tape is
/// raw signal.
const RIPPLE_WHOLE: f32 = 0.9;
const RIPPLE_LOSS: f32 = 0.95;

fn rippled(depth: f32) -> Judged {
    judge(&ripple(&render(&tape()), RATE, depth, 2400.0), &tape(), same(depth))
}

fn ripple_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::RawSignal, Said::Collapsed]), Vec::new())
}

#[test]
fn ripple_on_the_peaks_reads_whole_to_nine_tenths() {
    holds(RIPPLE_WHOLE, RIPPLE_LOSS, ripple_lost(), rippled);
}

#[test]
#[ignore]
fn ripple_sweep() {
    sweep(&[0.3, 0.5, 0.7, RIPPLE_WHOLE, RIPPLE_LOSS], RIPPLE_WHOLE, RIPPLE_LOSS, ripple_lost(), rippled);
}

/// The pulses of the tape as a 1-bit capture has them, with a bounce of `width` T-states, two
/// pulses, at pulse `at` from the data block's first sync pulse: at the start of that pulse, or
/// at its end with `end`.
fn bounced(at: usize, end: bool, width: u64) -> Judged {
    let p = of_blocks(&tape()).unwrap();
    let mut lens: Vec<u64> = p.iter().map(|q| q.tstates.round() as u64).collect();
    let sync = (1..lens.len()).filter(|&i| lens[i] == 667 && lens[i - 1] == 2168).nth(1).unwrap() + at;
    let h = lens[sync];
    let parts = if end { [h - 2 * width, width, width] } else { [width, width, h - 2 * width] };
    lens.splice(sync..=sync, parts);
    let p = common::pulses(&lens);
    let got = decode_pulses(&p, 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    (outcome(&got, &tape(), (1.0, 1.0)), rom_only(&p, 3_500_000, &tape()))
}

#[test]
fn a_bounce_in_the_sync_reads_whole() {
    // The ROM waits 505 T after sync 1 before its first read for sync 2, and 468 T or more after
    // every other edge, and two edges inside that wait are none: the sync loop and the first
    // bit's first half, which have no half-wave before them to take a bounce, take it into the
    // pulse after it. A bounce at either sync pulse, at its start or its end, or at the first bit,
    // up to two pulses of 240 T.
    for (at, end) in [(0, false), (0, true), (1, false), (1, true), (2, false)] {
        for width in [20, 79, 240] {
            assert_eq!(bounced(at, end, width), WHOLE, "{width} T at pulse {at}");
        }
    }
}

#[test]
fn a_click_in_the_sync_reads_whole() {
    // One to four samples anywhere inside sync 1 or sync 2 of the data block, which the ROM's
    // waits after an edge hide as they hide a bounce.
    let clean = render(&tape());
    let sync = bits_at(&tape(), 1) - ((667.0 + 735.0) * f64::from(RATE) / 3_500_000.0) as usize;
    for at in [0, 3, 8, 12, 16] {
        for width in [1, 3, 4] {
            assert_eq!(
                judge(&spike(&clean, sync + at, width, 0.5), &tape(), (1.0, 1.0)),
                WHOLE,
                "{width} samples at {at}"
            );
        }
    }
}

// Compounds.

/// The tape `factor` times as long, with flutter of `depth` at 10 Hz on top.
fn off_speed_fluttering(factor: f64, depth: f64) -> Judged {
    let x = wow(&speed(&render(&tape()), factor), RATE, depth, 10.0);
    judge(&x, &tape(), (factor * (1.0 - depth), factor * (1.0 + depth)))
}

/// A tape 20% fast puts the pilot's pairs at 3,613 T, and the leader's window, which stays the
/// ROM's whatever the speed, ends at 3,373 T: flutter of 7% takes pairs out of it at every
/// swing, and no run of the ROM's 256 is left. A run of 16 between two swings locks where the
/// block behind it proves itself, to 21%; at 22% no block locks, the whole tape raw signal. A
/// worn belt runs 5% off.
const FAST_FLUTTER_WHOLE: f64 = 0.21;
const FAST_FLUTTER_LOSS: f64 = 0.22;
const FAST: f64 = 1.0 / 1.2;
const SLOW: f64 = 1.0 / 0.85;

fn fast_flutter_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::RawSignal]), Vec::new())
}

/// 17.6% slow, flutter is nothing to 19%. At 20% the data block's run of 0xFF bytes, whose
/// pairs lie 21% under the pilot's, swings into the pilot's window and ends the block there,
/// short of its length; the ROM, which stops only at the length, loads it. Whether that happens
/// turns on where the flutter's swing falls (25% reads whole). A deck's flutter is a few tenths
/// of a percent.
const SLOW_FLUTTER_WHOLE: f64 = 0.19;
const SLOW_FLUTTER_LOSS: f64 = 0.2;

fn slow_flutter_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Short, Said::Collapsed, Said::Lost]), vec![1])
}

#[test]
fn flutter_on_a_tape_twenty_percent_fast_reads_whole_to_twenty_one_percent() {
    holds(FAST_FLUTTER_WHOLE, FAST_FLUTTER_LOSS, fast_flutter_lost(), |d| off_speed_fluttering(FAST, d));
}

#[test]
fn flutter_on_a_tape_seventeen_percent_slow_reads_whole_to_nineteen_percent() {
    holds(SLOW_FLUTTER_WHOLE, SLOW_FLUTTER_LOSS, slow_flutter_lost(), |d| off_speed_fluttering(SLOW, d));
}

#[test]
#[ignore]
fn speed_and_flutter_sweeps() {
    let grid = [0.0, 0.04, 0.07, 0.12, 0.16, 0.2, FAST_FLUTTER_WHOLE, FAST_FLUTTER_LOSS, 0.3];
    sweep(&grid, FAST_FLUTTER_WHOLE, FAST_FLUTTER_LOSS, fast_flutter_lost(), |d| {
        off_speed_fluttering(FAST, d)
    });
    let grid = [0.0, 0.04, 0.08, 0.12, 0.16, 0.18, SLOW_FLUTTER_WHOLE, SLOW_FLUTTER_LOSS];
    sweep(&grid, SLOW_FLUTTER_WHOLE, SLOW_FLUTTER_LOSS, slow_flutter_lost(), |d| {
        off_speed_fluttering(SLOW, d)
    });
}

/// The tape `factor` times as long, its falls slowed by a time constant of `fall_us`.
fn off_speed_skewed(factor: f64, fall_us: f64) -> Judged {
    let x = azimuth(&speed(&render(&tape()), factor), RATE, 10.0, fall_us);
    judge(&x, &tape(), (factor, factor))
}

/// A fall slowed by a time constant `τ` crosses zero late, and every pair keeps its sum but the
/// sync's first pulse and a block's last bit, which are read alone. The pilot's halves part with
/// it, 42% either way at 1.5 ms on the midline, which the sync loop and the leader loop take for
/// pilot as they are; so does the decoder, by their pairs (`pilot_run_at`) and each against the
/// others of its kind (`a_pilot_whose_halves_part_by_more_than_a_third_reads_whole`). From 3.5 ms
/// a run of 0 bits, the header's spaces, loses its short lobes: the header reads on the fade
/// band, and so does the data block it then asks for. At 4 ms the data block stops three bytes
/// in with a bad checksum on either band, the rest of it played as its pause. Played 17.6% slow,
/// every half-wave is longer against the same delay, and from 2 ms the pilot's long halves pass
/// 3,644 T, where the ROM's sync loop runs out: the header stops a byte short, and the fade band
/// reads it and then the data block. At 5 ms both read so, but the data block's pilot rises out
/// of the header's pause late, which runs 3.5% long. A rise and a fall that differ by a quarter
/// of a millisecond are what one track of a worn head gives.
const AZIMUTH_WHOLE_US: f64 = 3750.0;
const AZIMUTH_LOSS_US: f64 = 4000.0;
const SLOW_AZIMUTH_WHOLE_US: f64 = 4750.0;
const SLOW_AZIMUTH_LOSS_US: f64 = 5000.0;

fn azimuth_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Short, Said::Collapsed]), Vec::new())
}

fn slow_azimuth_lost() -> Judged {
    (Outcome::Pause, Vec::new())
}

#[test]
fn a_late_kind_of_edge_reads_whole_to_three_and_three_quarter_milliseconds() {
    holds(AZIMUTH_WHOLE_US, AZIMUTH_LOSS_US, azimuth_lost(), |t| off_speed_skewed(1.0, t));
}

#[test]
fn a_late_kind_of_edge_on_a_slow_tape_reads_whole_to_4750_us() {
    holds(SLOW_AZIMUTH_WHOLE_US, SLOW_AZIMUTH_LOSS_US, slow_azimuth_lost(), |t| off_speed_skewed(SLOW, t));
}

#[test]
#[ignore]
fn azimuth_sweeps() {
    let grid = [100.0, 500.0, 1000.0, 2000.0, 3000.0, 3500.0, AZIMUTH_WHOLE_US, AZIMUTH_LOSS_US, 5000.0];
    sweep(&grid, AZIMUTH_WHOLE_US, AZIMUTH_LOSS_US, azimuth_lost(), |t| off_speed_skewed(1.0, t));
    let grid =
        [25.0, 250.0, 1000.0, 2000.0, 3000.0, 4000.0, SLOW_AZIMUTH_WHOLE_US, SLOW_AZIMUTH_LOSS_US, 8000.0];
    sweep(&grid, SLOW_AZIMUTH_WHOLE_US, SLOW_AZIMUTH_LOSS_US, slow_azimuth_lost(), |t| {
        off_speed_skewed(SLOW, t)
    });
}

fn fast_hissed(db: u32) -> Judged {
    let f = 1.0 / 1.1;
    judge(&hiss(&speed(&render(&tape()), f), db, 0x5EED), &tape(), (f, f))
}

/// Played 10% fast, every half-wave is shorter against the same hiss, and the wall stays where
/// it is without the speed: whole at 10 dB, the data block's checksum bad at 9.
#[test]
fn hiss_on_a_tape_ten_percent_fast_reads_whole_to_ten_db() {
    holds(HISS_WHOLE_DB, HISS_LOSS_DB, hiss_lost(), fast_hissed);
}

#[test]
#[ignore]
fn speed_and_hiss_sweep() {
    sweep(
        &[14, 13, 12, 11, HISS_WHOLE_DB, HISS_LOSS_DB, 8, 7],
        HISS_WHOLE_DB,
        HISS_LOSS_DB,
        hiss_lost(),
        fast_hissed,
    );
}

/// The tape through `microns` of spacing, with a wandering centre line: a triangle of `wander`
/// at 120 Hz.
fn dull_wandering(microns: f64, wander: f32) -> Judged {
    judge(&hum(&spacing_loss(&render(&tape()), RATE, microns), RATE, 120.0, wander), &tape(), (1.0, 1.0))
}

/// Treble loss shrinks the short lobes and a wander lifts them off the zero line; the midline
/// reads them about the signal's own centre. A wander of 0.025, a twentieth of the level, leaves
/// the treble wall where it is; past it both blocks are lost, said here of the lead-in, where
/// the header's pilot with bits behind it lies. At 5 µm the midline follows
/// a wander up to 0.9, nearly twice the level, where the zero line, on which the short lobes no
/// longer cross, has read no block since 0.4. At 1.0 the midline's data block stops five bits
/// into its last byte but one, short, and the 0s that fill that byte out complete its checksum,
/// which the decode says proves nothing. The treble wall is this tape's, which has little
/// dead air (`dead_air_takes_the_noise_line_out_of_the_signal`). A worn deck's low end carries
/// 10% of the level under 300 Hz on a tape that reads, 23% on one that does not.
const WANDER: f32 = 0.025;
const WANDER_AT_UM: f64 = 5.0;
const WANDER_WHOLE: f32 = 0.9;
const WANDER_LOSS: f32 = 1.0;

fn wander_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::Short, Said::Collapsed, Said::FilledOut]), Vec::new())
}

fn nothing_left() -> Judged {
    (Outcome::Lost(vec![0, 1], Vec::new()), Vec::new())
}

fn treble_wander_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::Lost]), Vec::new())
}

#[test]
fn treble_loss_with_a_wandering_centre_line_reads_whole_to_eleven_and_a_half_microns() {
    holds(TREBLE_WHOLE_UM, TREBLE_LOSS_UM, treble_wander_lost(), |d| dull_wandering(d, WANDER));
}

#[test]
fn a_wandering_centre_line_on_a_dull_tape_reads_whole_to_nine_tenths() {
    holds(WANDER_WHOLE, WANDER_LOSS, wander_lost(), |w| dull_wandering(WANDER_AT_UM, w));
}

/// The tape with hum at mains rate, a triangle of `amplitude` at 50 Hz.
fn hummed(amplitude: f32) -> Judged {
    judge(&hum(&render(&tape()), RATE, 50.0, amplitude), &tape(), (1.0, 1.0))
}

/// The DC blocker passes 50 Hz at 0.98, and the midline follows hum up to 1.3, over twice the
/// tape's level, where the zero line has read no block since 0.6; at 1.4 neither reads one, and
/// nothing is left. Mains hum on a deck is a few percent of the level.
const HUM_WHOLE: f32 = 1.3;
const HUM_LOSS: f32 = 1.4;

#[test]
fn hum_at_mains_rate_reads_whole_to_over_twice_the_tapes_level() {
    holds(HUM_WHOLE, HUM_LOSS, nothing_left(), hummed);
}

#[test]
#[ignore]
fn hum_sweep() {
    sweep(
        &[0.05, 0.1, 0.3, 0.5, 0.8, 1.0, HUM_WHOLE, HUM_LOSS, 2.0],
        HUM_WHOLE,
        HUM_LOSS,
        nothing_left(),
        hummed,
    );
}

/// The tape with `secs` of dead air either side, and the sample where the tape starts.
fn padded(secs: f64) -> (Vec<f32>, usize) {
    let mut x = vec![0.0; ms(secs * 1000.0)];
    let at = x.len();
    x.extend(render(&tape()));
    x.extend(vec![0.0; ms(secs * 1000.0)]);
    (x, at)
}

/// The tape with `secs` of dead air either side, then `defect`.
fn in_dead_air(secs: f64, defect: impl Fn(&[f32]) -> Vec<f32>) -> Judged {
    judge(&defect(&padded(secs).0), &tape(), (1.0, 1.0))
}

/// Every wall's tape holds 2.25 s of silence in 10.8 s, and the edge detector measures the
/// recording's noise and its floor over the quietest seconds, the fifth of ten (`QUIET_SECONDS`):
/// on these tapes those seconds are mostly signal. Treble loss, alone and with a wander, keeps its
/// wall in any dead air: whole at 11.5 µm with two seconds of it or ten, lost at 12. (It once
/// read whole a step past its wall with ten seconds and not with two or five, before a block a
/// header read on the fade band asks for was read there too.) The hiss wall does not move out: measured against the tape, not over
/// the dead air too (which would lower it 4.6 dB), the hiss reads whole to 11 dB and the data
/// block is lost at 10, a dB short of the bare tape. A wander on a dull tape keeps its wall, 0.9,
/// since it lies over the dead air too. Nothing is changed here, and the walls keep their
/// tape. Whole, the last block's pause holds the dead air behind
/// it.
#[test]
fn dead_air_takes_the_noise_line_out_of_the_signal() {
    let treble_wander = |x: &[f32]| hum(&spacing_loss(x, RATE, TREBLE_WHOLE_UM), RATE, 120.0, WANDER);
    assert_eq!(in_dead_air(2.0, treble_wander), (Outcome::Pause, Vec::new()));
    assert_eq!(in_dead_air(10.0, |x| spacing_loss(x, RATE, TREBLE_WHOLE_UM)), (Outcome::Pause, Vec::new()));
    assert_eq!(
        in_dead_air(10.0, |x| spacing_loss(x, RATE, TREBLE_LOSS_UM)),
        (Outcome::Lost(vec![0, 1], vec![Said::RawSignal, Said::Collapsed, Said::Lost]), Vec::new())
    );
    assert_eq!(in_dead_air(10.0, |x| hiss_as_bare(x, 11)), (Outcome::Pause, Vec::new()));
    assert_eq!(
        in_dead_air(10.0, |x| hiss_as_bare(x, 10)),
        (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), Vec::new())
    );
    let wandering = |w: f32| move |x: &[f32]| hum(&spacing_loss(x, RATE, WANDER_AT_UM), RATE, 120.0, w);
    assert_eq!(in_dead_air(10.0, wandering(WANDER_WHOLE)), (Outcome::Pause, Vec::new()));
    assert_eq!(
        in_dead_air(10.0, wandering(WANDER_LOSS)),
        (
            Outcome::Lost(
                vec![1],
                vec![Said::Short, Said::RawSignal, Said::Collapsed, Said::Lost, Said::FilledOut]
            ),
            Vec::new()
        )
    );
}

/// A defect of the walls applied to the padded tape: the samples, where the tape starts in them
/// and the step of the sweep, to the samples and the rate they are at.
type Padded = fn(&[f32], usize, f64) -> (Vec<f32>, u32);

/// Hiss `db` under the bare tape's level, whatever dead air is around it.
fn hiss_as_bare(x: &[f32], db: u32) -> Vec<f32> {
    hiss_under(x, rms(&render(&tape())), db, 0x5EED)
}

/// Every wall's defect, over its grid, on the tape in dead air.
fn padded_walls() -> Vec<(&'static str, Vec<f64>, Padded)> {
    let f = |g: &[f32]| g.iter().map(|&v| f64::from(v)).collect::<Vec<f64>>();
    vec![
        ("treble", TREBLE_GRID_UM.to_vec(), |x, _, v| (spacing_loss(x, RATE, v), RATE)),
        ("hiss", (0..=30).rev().map(f64::from).collect(), |x, _, v| (hiss_as_bare(x, v as u32), RATE)),
        // Measured over the dead air too, as a recording's own level would be: between the
        // steps of the other, 1.4 dB quieter at 2 s, 4.6 dB at 10.
        ("hiss over all", (0..=30).rev().map(f64::from).collect(), |x, _, v| {
            (hiss(x, v as u32, 0x5EED), RATE)
        }),
        ("dropout", DROPOUT_GRID_MS.to_vec(), |x, at, v| {
            (dropout(x, at + bits_at(&tape(), 1) + ms(500.0), ms(v), 0.0), RATE)
        }),
        ("fade", f(&FADE_GRID), |x, at, v| {
            let from = at + block_at(&tape(), 1);
            (hiss_as_bare(&fade(x, from, from + ms(1000.0), v as f32), 50), RATE)
        }),
        ("wow 1 Hz", WOW_GRID.to_vec(), |x, _, v| (wow(x, RATE, v, 1.0), RATE)),
        ("flutter 10 Hz", WOW_GRID.to_vec(), |x, _, v| (wow(x, RATE, v, 10.0), RATE)),
        ("click at edge", (1..=10).map(f64::from).collect(), |x, at, v| {
            let edges = click_edges(&x[at..]);
            (edges.iter().fold(x.to_vec(), |y, &e| spike(&y, at + e + 1, v as usize, 0.5)), RATE)
        }),
        ("click inside", vec![0.05, 0.1, 0.15, 0.2, 0.3, 0.5], |x, at, v| {
            let edges = edges_in(x, at + bits_at(&tape(), 1), x.len());
            let mut y = x.to_vec();
            for k in 0..8 {
                let i = (100 + 400 * k..).find(|&i| edges[i + 1] - edges[i] >= 20).unwrap();
                let mid = (edges[i] + edges[i + 1]) / 2;
                y[mid] -= y[mid].signum() * 2.0 * v as f32;
            }
            (y, RATE)
        }),
        ("rate", vec![8000.0, 7000.0, 6000.0, 5512.0, 5000.0, 4410.0, 4000.0, 3500.0], |x, _, v| {
            (downsample(x, RATE, v as u32), v as u32)
        }),
        ("ripple", vec![0.3, 0.5, 0.7, 0.9, 0.95, 1.0], |x, _, v| (ripple(x, RATE, v as f32, 2400.0), RATE)),
        ("fast flutter", vec![0.0, 0.04, 0.07, 0.12, 0.16, 0.2, 0.21, 0.22, 0.3], |x, _, v| {
            (wow(&speed(x, FAST), RATE, v, 10.0), RATE)
        }),
        ("slow flutter", vec![0.0, 0.04, 0.08, 0.12, 0.16, 0.18, 0.19, 0.2, 0.25], |x, _, v| {
            (wow(&speed(x, SLOW), RATE, v, 10.0), RATE)
        }),
        (
            "azimuth",
            vec![500.0, 1000.0, 1500.0, 2000.0, 3000.0, 3500.0, 3750.0, 4000.0, 5000.0],
            |x, _, v| (azimuth(x, RATE, 10.0, v), RATE),
        ),
        ("slow azimuth", vec![500.0, 1000.0, 2000.0, 3000.0, 4000.0, 4750.0, 5000.0, 8000.0], |x, _, v| {
            (azimuth(&speed(x, SLOW), RATE, 10.0, v), RATE)
        }),
        ("fast hiss", (0..=14).rev().map(f64::from).collect(), |x, _, v| {
            (hiss_as_bare(&speed(x, 1.0 / 1.1), v as u32), RATE)
        }),
        ("treble, wander", TREBLE_GRID_UM.to_vec(), |x, _, v| {
            (hum(&spacing_loss(x, RATE, v), RATE, 120.0, WANDER), RATE)
        }),
        ("wander at 5 um", vec![0.05, 0.2, 0.4, 0.6, 0.8, 0.9, 1.0, 1.25], |x, _, v| {
            (hum(&spacing_loss(x, RATE, WANDER_AT_UM), RATE, 120.0, v as f32), RATE)
        }),
        ("hum", vec![0.1, 0.5, 0.8, 1.0, 1.3, 1.4, 2.0], |x, _, v| (hum(x, RATE, 50.0, v as f32), RATE)),
        ("dip, hiss", vec![30.0, 25.0, 20.0, 19.0, 17.0, 14.0, 11.0, 8.0, 5.0], |x, at, v| {
            let mut y = x.to_vec();
            let from = at + bits_at(&tape(), 1) + ms(500.0);
            for s in &mut y[from..from + ms(150.0)] {
                *s *= DIP_DEPTH;
            }
            (hiss_as_bare(&y, v as u32), RATE)
        }),
    ]
}

/// The steps of [`dead_air_sweeps`] that read a wrong block: each a misread past its wall whose
/// checksum holds by chance, one in 256 (`proof`). Of the sweep's 750 steps, 106 hold a block
/// read wrong, each read on two lines or more, so about one such step is to be expected. No
/// reading of either, on either line or band, holds the tape's bytes, so no rule could read it
/// right. The second's block runs on past the length its header asked, which the saver never
/// writes; no recording has shown a header's block running on to judge a rule against, and
/// its two run-ons are pulses in the pause of a header read right.
const DEAD_AIR_CHANCE: [&str; 2] = [
    // A 245-byte headerless misread behind a header read wrong.
    "treble 12 in 5 s",
    // Both lines slip in the dip by two and four bytes, and the midline's checks out.
    "dip, hiss 17 in 2 s",
];

/// A recording has dead air around its tape, which moves the edge detector's noise line
/// (`dead_air_takes_the_noise_line_out_of_the_signal`): every wall's sweep again with 2, 5 and
/// 10 s of it either side. No step may read a wrong block but those whose checksum holds by
/// chance ([`DEAD_AIR_CHANCE`]).
#[test]
#[ignore]
fn dead_air_sweeps() {
    let jobs: Vec<(f64, &'static str, f64, Padded)> = padded_walls()
        .into_iter()
        .flat_map(|(name, grid, defect)| {
            [2.0, 5.0, 10.0]
                .into_iter()
                .flat_map(move |secs| grid.clone().into_iter().map(move |v| (secs, name, v, defect)))
        })
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut got: Vec<(usize, Judged)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&(secs, _, v, defect)) = jobs.get(k) else { break mine };
                        let (x, at) = padded(secs);
                        let (y, rate) = defect(&x, at, v);
                        mine.push((k, judge_at(&y, rate, 16, &tape(), (0.0, f64::MAX))));
                    }
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap()).collect()
    });
    got.sort_by_key(|g| g.0);
    let mut wrong = Vec::new();
    for (k, judged) in got {
        let (secs, name, v, _) = jobs[k];
        println!("{name} {v} in {secs} s: {judged:?}");
        if judged.0 == Outcome::Wrong {
            wrong.push(format!("{name} {v} in {secs} s"));
        }
    }
    wrong.sort();
    let mut open: Vec<String> = DEAD_AIR_CHANCE.iter().map(|s| s.to_string()).collect();
    open.sort();
    assert_eq!(wrong, open, "the steps that read a wrong block");
}

#[test]
#[ignore]
fn treble_and_wander_sweeps() {
    sweep(&TREBLE_GRID_UM[..14], TREBLE_WHOLE_UM, TREBLE_LOSS_UM, treble_wander_lost(), |d| {
        dull_wandering(d, WANDER)
    });
    let grid = [0.01, 0.05, 0.1, 0.2, 0.4, 0.6, 0.8, WANDER_WHOLE, WANDER_LOSS, 1.25, 1.5];
    sweep(&grid, WANDER_WHOLE, WANDER_LOSS, wander_lost(), |w| dull_wandering(WANDER_AT_UM, w));
}

/// The tape on a stereo head whose right track lags `lag` samples, with a dropout of `len` ms in
/// both tracks half a second into the data block's bits.
fn skewed_dropout(lag: f64, len: f64) -> Judged {
    let x = dropout(&render(&tape()), bits_at(&tape(), 1) + ms(500.0), ms(len), 0.0);
    let (left, right) = head_skew(&x, lag);
    let mix: Vec<f32> = left.iter().zip(&right).map(|(a, b)| (a + b) / 2.0).collect();
    let rom = rom_only(&detect_pulses(|| mix.iter().copied(), RATE, Filter::None).unwrap(), RATE, &tape());
    (outcome(&decode(&stereo_wav(&left, &right, RATE, 16), Channels::Best), &tape(), (1.0, 1.0)), rom)
}

/// Two heads 0.9 mm apart lag 2.4 samples for every 10′ of azimuth, and +2 to +7 has been
/// measured; the mix then cancels the short half-waves, and a channel alone is read instead.
/// With a dropout in both tracks that the mono wall reads, a lag of 20 samples still reads; the
/// dropout's own wall is where it is in mono.
const SKEW_DROPOUT_MS: f64 = 1.25;
const SKEW_MOST: f64 = 20.0;
const SKEW_LAG: f64 = 7.0;

#[test]
fn head_skew_with_a_dropout_reads_whole_to_twenty_samples() {
    assert_eq!(skewed_dropout(SKEW_MOST, SKEW_DROPOUT_MS), WHOLE);
    holds(DROPOUT_WHOLE_MS, DROPOUT_LOSS_MS, dropout_lost(), |len| skewed_dropout(SKEW_LAG, len));
}

#[test]
#[ignore]
fn head_skew_and_dropout_sweeps() {
    for lag in [0.0, 1.0, 2.0, 3.0, 5.0, 7.0, 10.0, 12.0, 16.0, SKEW_MOST] {
        assert_eq!(skewed_dropout(lag, SKEW_DROPOUT_MS), WHOLE, "lag {lag}");
    }
    sweep(&DROPOUT_GRID_MS[..8], DROPOUT_WHOLE_MS, DROPOUT_LOSS_MS, dropout_lost(), |len| {
        skewed_dropout(SKEW_LAG, len)
    });
}

// Each case below reproduces a signal defect a generator can model, at the severity observed.
// The stereo arbitration losses (four of eight) have no generator and are cells of the fold
// instead.

/// The tape's pulses with one pulse of `hole` T-states, a hole in the pilot, `pairs` pilot pairs
/// before the data block's sync.
fn holed(pairs: usize, hole: u64) -> Vec<Pulse> {
    let p = of_blocks(&tape()).unwrap();
    let mut lens: Vec<u64> = p.iter().map(|q| q.tstates.round() as u64).collect();
    let sync = (1..lens.len()).filter(|&i| lens[i] == 667 && lens[i - 1] == 2168).nth(1).unwrap();
    lens[sync - 2 * pairs - 1] = hole;
    common::pulses(&lens)
}

fn pilot_hole_decode(pairs: usize, hole: u64) -> Decoded {
    decode_pulses(&holed(pairs, hole), 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap()
}

fn pilot_hole(pairs: usize, hole: u64) -> Judged {
    let got = pilot_hole_decode(pairs, hole);
    (outcome(&got, &tape(), (1.0, 1.0)), rom_only(&holed(pairs, hole), 3_500_000, &tape()))
}

/// A hole in the pilot restarts the lock. The ROM needs 256 pairs behind it and loses the block
/// with fewer, at every phase; the decoder locks on 16 (`SHORT_LEADER_PAIRS`) behind the one
/// the leader takes unchecked, and keeps the block where it answers its header, so the block is
/// read whole while 17 pairs follow the hole. Two recordings were this cell: a
/// 25 ms hole 37 pairs before the sync, and a recording cut 200 pairs before it. Under 16 pairs
/// no tone is left that makes the stretch block-shaped, and it plays as the header's pause, said
/// as a block lost there: the pilot before the hole, which the ROM locks on, and the signal
/// behind it.
const PILOT_HOLE_WHOLE_PAIRS: usize = 17;
const PILOT_HOLE_LOSS_PAIRS: usize = 16;
/// 25 ms, as measured.
const PILOT_HOLE_T: u64 = 88_290;

/// The data block raw signal, and the pilot pulses before the hole played as the header's
/// pause, which is then longer.
fn pilot_hole_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::RawSignal, Said::CollapsedPilot]), Vec::new())
}

#[test]
fn a_hole_in_the_pilot_loses_the_block_unless_17_pairs_follow_it() {
    // The pilot pulses before the hole play as the header's pause, which is then longer; the
    // block they lead into is read, and nothing is said lost.
    for n in [257, 37, PILOT_HOLE_WHOLE_PAIRS] {
        assert_eq!(pilot_hole(n, PILOT_HOLE_T), (Outcome::Pause, Vec::new()), "{n}");
        let got = pilot_hole_decode(n, PILOT_HOLE_T);
        assert!(!got.findings.iter().any(|f| matches!(f, Finding::Lost { .. })), "{n}: {:?}", got.findings);
    }
    assert_eq!(pilot_hole(PILOT_HOLE_LOSS_PAIRS, PILOT_HOLE_T), pilot_hole_lost());
    assert_eq!(
        pilot_hole(PILOT_HOLE_LOSS_PAIRS - 1, PILOT_HOLE_T),
        (Outcome::Lost(vec![1], vec![Said::Lost]), Vec::new())
    );
}

#[test]
#[ignore]
fn pilot_hole_sweep() {
    for n in [400, 300, 257, 256, 200, 100, 37, 20, PILOT_HOLE_WHOLE_PAIRS] {
        assert_eq!(pilot_hole(n, PILOT_HOLE_T), (Outcome::Pause, Vec::new()), "{n}");
    }
    assert_eq!(pilot_hole(PILOT_HOLE_LOSS_PAIRS, PILOT_HOLE_T), pilot_hole_lost());
    for n in [15, 10, 4] {
        assert_eq!(
            pilot_hole(n, PILOT_HOLE_T),
            (Outcome::Lost(vec![1], vec![Said::Lost]), Vec::new()),
            "{n}"
        );
    }
}

/// The tape's pulses with `glitch` in place of as many pilot pulses, `back` pulses before the
/// data block's sync.
fn pilot_glitch(glitch: &[u64], back: usize) -> Judged {
    let p = of_blocks(&tape()).unwrap();
    let mut lens: Vec<u64> = p.iter().map(|q| q.tstates.round() as u64).collect();
    let sync = (1..lens.len()).filter(|&i| lens[i] == 667 && lens[i - 1] == 2168).nth(1).unwrap();
    let at = sync - back;
    lens.splice(at..at + glitch.len(), glitch.iter().copied());
    let p = common::pulses(&lens);
    let got = decode_pulses(&p, 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    (outcome(&got, &tape(), (1.0, 1.0)), rom_only(&p, 3_500_000, &tape()))
}

#[test]
fn a_glitch_in_the_pilot_reads_whole() {
    // Three pulses of 1,300, 3,097 and 1,543 T in the pilot, 147 pairs before its sync. The
    // ROM's sync loop asks each pulse only to be longer than
    // a sync pulse and counts on, and loads the block at every phase; the 3,097 T pulse is as
    // long as a forward merge of the sync pair, but the pilot runs on behind it, so no block
    // begins there.
    const GLITCH: [u64; 3] = [1300, 3097, 1543];
    for back in [100, 400, 1000] {
        assert_eq!(pilot_glitch(&GLITCH, back), WHOLE, "{back}");
    }
}

#[test]
fn a_click_that_splits_a_pilot_half_wave_reads_whole() {
    // A pilot half split in three by a click, its middle pulse short enough for a first sync
    // pulse: the ROM takes it for its sync, reads the pilot as 1 bits and fails, and loads the
    // block only where its 256 pairs all lie behind the click. The pilot runs on behind it, so
    // it is no sync, and the sync loop goes on.
    for glitch in [[1000, 168, 1000], [700, 300, 1168]] {
        for back in [100, 400] {
            let (outcome, _) = pilot_glitch(&glitch, back);
            assert_eq!(outcome, Outcome::Whole, "{glitch:?} {back}");
        }
    }
}

#[test]
fn a_glitch_that_spoils_more_than_a_byte_of_the_pilot_reads_whole() {
    // A pulse as long as a forward merge of the sync pair, then two bytes' worth of pulses that
    // pair into 1 bits and are no pilot: the pilot runs on only behind them. Read from the
    // glitch, they are two bytes of 0xFF, which is what a pilot read as bits gives.
    let glitch: Vec<u64> = [3097, 1543].into_iter().chain([1300; 32]).collect();
    for back in [100, 400] {
        assert_eq!(pilot_glitch(&glitch, back), WHOLE, "{back}");
    }
}

#[test]
fn a_glitch_in_the_pilot_with_a_bounce_behind_it_reads_whole() {
    // Hiss under 6 dB, on a recording with dead air, split a half of the header's pilot as
    // 657 + 29 + 1,454 T, the first short enough for a first sync pulse, and 31 pulses on another
    // as 1,930 + 22 + 203 T. The pilot runs on behind the first, so it is a glitch; the second
    // lies inside every run of 32 that starts within a byte of it, unless its two short pulses,
    // which the ROM's wait after an edge hides as it hides a bounce, are folded into the half in
    // front of them, as the bit loop folds them. Read from the glitch, the pilot is two bytes of
    // 0xFF, and the checksum holds.
    let p = of_blocks(&tape()).unwrap();
    let lens: Vec<u64> = p.iter().map(|q| q.tstates.round() as u64).collect();
    // Pulses into the header's pilot of 8,063, past the leader's 256 pairs.
    for at in [2000, 5000] {
        let mut glitched = lens.clone();
        glitched.splice(at + 31..at + 32, [1943, 22, 203]);
        glitched.splice(at..at + 1, [657, 29, 1482]);
        let p = common::pulses(&glitched);
        let got = decode_pulses(&p, 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(outcome(&got, &tape(), (1.0, 1.0)), Outcome::Whole, "{at}");
    }
}

#[test]
fn a_sync_pair_split_by_440_t_reads_whole() {
    // A sync whose first pulse is 1,106 T and second 295 T, the sum kept, as a worn recording
    // has it: read whole, and the ROM's loader, which judges the first pulse alone, loads it
    // too.
    let p = of_blocks(&tape()).unwrap();
    let mut lens: Vec<u64> = p.iter().map(|q| q.tstates.round() as u64).collect();
    let sync = (1..lens.len()).filter(|&i| lens[i] == 667 && lens[i - 1] == 2168).nth(1).unwrap();
    lens[sync] = 1106;
    lens[sync + 1] = 295;
    let p = common::pulses(&lens);
    let got = decode_pulses(&p, 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!((outcome(&got, &tape(), (1.0, 1.0)), rom_only(&p, 3_500_000, &tape())), WHOLE);
}

#[test]
fn a_stretch_of_other_timings_behind_the_tape_leaves_a_short_bad_block() {
    // A pilot, a sync and 600 pulses that are no ROM bits (a turbo loader's, or a scramble of
    // ROM halves) at the recording's end: the merged-sync forms read a short block from it
    // with a bad checksum rather than leaving raw signal, which is the two-byte 0xFF block such
    // a recording can end in. A bare pilot plays as the last block's pause.
    let p = of_blocks(&tape()).unwrap();
    let base: Vec<u64> = p.iter().map(|q| q.tstates.round() as u64).collect();
    fn turbo(j: usize) -> u64 {
        if j.is_multiple_of(3) {
            2000
        } else {
            1000
        }
    }
    fn scramble(j: usize) -> u64 {
        if (j / 7).is_multiple_of(2) {
            1710
        } else {
            855
        }
    }
    for bits in [turbo as fn(usize) -> u64, scramble] {
        let mut lens = base.clone();
        lens.extend([2168; 1000]);
        lens.extend([667, 735]);
        lens.extend((0..600).map(bits));
        let got =
            decode_pulses(&common::pulses(&lens), 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(outcome(&got, &tape(), (1.0, 1.0)), Outcome::Lost(vec![], vec![Said::BadChecksum]));
        assert_eq!(got.blocks.len(), 3);
    }
    let mut lens = base.clone();
    lens.extend([2168; 1000]);
    let got = decode_pulses(&common::pulses(&lens), 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(outcome(&got, &tape(), (1.0, 1.0)), Outcome::Pause);
    // So does one whose halves part at its end, as a recording that stops inside a pilot can
    // have them, the pairs holding: read from a merge there, the pilot is 1 bits to the end,
    // which is no block either.
    lens.extend([3500, 836, 3500, 836]);
    lens.extend([2168; 14]);
    let got = decode_pulses(&common::pulses(&lens), 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(outcome(&got, &tape(), (1.0, 1.0)), Outcome::Pause);
}

/// The tape's pulses with the data block's pilot halves moved `skew` T-states apart either way,
/// their pairs kept, the first and last short, and its sync pair split as `sync`: one kind of
/// edge late, as a recording's slow falls leave it, the first sync pulse lengthened.
fn skewed_pilot(skew: u64, sync: [u64; 2]) -> Judged {
    let p = of_blocks(&tape()).unwrap();
    let mut lens: Vec<u64> = p.iter().map(|q| q.tstates.round() as u64).collect();
    let at = (1..lens.len()).filter(|&i| lens[i] == 667 && lens[i - 1] == 2168).nth(1).unwrap();
    for (k, i) in (at - 3223..at).enumerate() {
        lens[i] = if k % 2 == 0 { lens[i] - skew } else { lens[i] + skew };
    }
    lens[at..at + 2].copy_from_slice(&sync);
    let p = common::pulses(&lens);
    let got = decode_pulses(&p, 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    (outcome(&got, &tape(), (1.0, 1.0)), rom_only(&p, 3_500_000, &tape()))
}

#[test]
fn a_pilot_whose_halves_part_by_more_than_a_third_reads_whole() {
    // A fall 1.5 ms slow on the midline parts the pilot's halves to 3,100 and 1,250 T, 42%
    // either side, the pairs held, and splits the sync pair 1,328 / 101 T, the first past the
    // ROM's window. The sync loop takes every one of those halves for pilot and the leader loop
    // every pair, so none is where the pilot ends: read from one, the rest of the pilot is 1 bits,
    // 0xFF bytes that leave the checksum as it was, and of a thousand such readings one holds by
    // chance. The ROM takes the 101 T pulse for its sync, reads the block half a bit out and
    // loads it at no phase; the pair is a sync pair by its sum.
    for skew in [600, 925, 1000] {
        assert_eq!(skewed_pilot(skew, [1328, 101]), WHOLE, "{skew}");
    }
}

/// The tape with hiss `db` under it and the data block's level cut to `depth` for 150 ms, half
/// a second into its bits.
fn hissed_dip(db: u32, depth: f32) -> Judged {
    let mut x = render(&tape());
    let at = bits_at(&tape(), 1) + ms(500.0);
    for v in &mut x[at..at + ms(150.0)] {
        *v *= depth;
    }
    judge(&hiss(&x, db, 0x5EED), &tape(), same(db))
}

/// Material in a dip to a quarter of the level stands 12 dB nearer the hiss, and the wall moves
/// out by about as much: whole at 20 dB, a bad checksum at 19. A recording at
/// 11.5 dB with two such dips loses a byte in each, and no channel of it reads whole.
const DIP_HISS_WHOLE_DB: u32 = 20;
const DIP_HISS_LOSS_DB: u32 = 19;
const DIP_DEPTH: f32 = 0.25;

fn dip_hiss_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), Vec::new())
}

#[test]
fn a_dip_to_a_quarter_under_hiss_reads_whole_to_twenty_db() {
    holds(DIP_HISS_WHOLE_DB, DIP_HISS_LOSS_DB, dip_hiss_lost(), |db| hissed_dip(db, DIP_DEPTH));
}

#[test]
#[ignore]
fn dip_and_hiss_sweep() {
    sweep(
        &[30, 25, DIP_HISS_WHOLE_DB, DIP_HISS_LOSS_DB, 17, 14, 11],
        DIP_HISS_WHOLE_DB,
        DIP_HISS_LOSS_DB,
        dip_hiss_lost(),
        |db| hissed_dip(db, DIP_DEPTH),
    );
}
