// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The decoder's walls: for each defect of the cassette chain (`common::degrade`), the worst it
//! reads a tape whole through, and what it makes of the tape one step past that.
//!
//! Each wall is two constants, the last step of its sweep read whole and the first not, asserted
//! by a fast test together with what the loss is: which blocks are missing and how the decode
//! says so. Never a block whose checksum holds over bytes the tape does not have. Beside each
//! fast test, ignored because it takes minutes unoptimised, is the sweep that found the wall: it
//! checks every step, whole up to the wall and no wrong bytes anywhere
//! (`cargo test --release -p tapeti-decode --test walls -- --ignored`). At every step of either
//! the ROM's loader runs over the same pulses, and a block it loads at every phase that framing
//! does not read is named beside the outcome. There is none up to any wall; past the 1 Hz wow and
//! the click behind an edge, and with a bounce or a click in the sync, the ROM still loads what
//! framing loses.

mod common;

use common::degrade::*;
use common::rom::unread;
use common::same_tape;
use std::fmt::Debug;
use std::sync::atomic::AtomicBool;
use tapeti_core::audio::{encode_wav, playback_order, render_tape, FlowOptions, RenderOptions};
use tapeti_core::types::{Block, Body};
use tapeti_decode::entry::{decode_file, decode_pulses, Channels, Decoded, Finding};
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
    /// A block whose checksum holds, with bytes the tape does not have.
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
    if got.blocks.iter().filter_map(standard).any(|d| checks(d) && !wants.contains(&d)) {
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
            Finding::Collapsed { .. } => Some(Said::Collapsed),
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

/// Spacing loss takes the short half-waves first: past 4.5 µm a lone sync 1 no longer crosses
/// zero, past 5.8 µm a lone 0 bit's half. The merged sync and the midline read the blocks on to
/// 10.5 µm; at 11 µm the header is read with a bad checksum and the data block, pilot and all,
/// plays as the header's pause. A good deck keeps about 1 µm, a worn one a few, a copy of a copy
/// about 12.
const TREBLE_WHOLE_UM: f64 = 10.5;
const TREBLE_LOSS_UM: f64 = 11.0;
const TREBLE_GRID_UM: [f64; 14] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 10.5, 11.0, 11.5, 12.0];

fn treble_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::BadChecksum, Said::Collapsed]), Vec::new())
}

#[test]
fn treble_loss_reads_whole_to_ten_and_a_half_microns() {
    holds(TREBLE_WHOLE_UM, TREBLE_LOSS_UM, treble_lost(), treble);
}

#[test]
#[ignore]
fn treble_loss_sweep() {
    sweep(&TREBLE_GRID_UM, TREBLE_WHOLE_UM, TREBLE_LOSS_UM, treble_lost(), treble);
}

// Hiss.

fn hissed(db: u32) -> Judged {
    judge(&hiss(&render(&tape()), db, 0x5EED), &tape(), same(db))
}

/// Hiss moves every crossing and cuts false edges into weak lobes; the band rides over it down to
/// 11 dB under the signal (the tape's level over its pauses and all). At 10 dB the bytes still
/// come back, but the start of the data block's pilot drowns and plays as 1.4 s more of the
/// header's pause; at 9 dB both blocks are raw signal, and at 7 dB and under nothing at all is
/// left of the tape. A clean recording lies 34 dB or more over its hiss; a noisy one 14 to 26.
const HISS_WHOLE_DB: u32 = 11;
const HISS_LOSS_DB: u32 = 10;

#[test]
fn hiss_reads_whole_to_eleven_db() {
    holds(HISS_WHOLE_DB, HISS_LOSS_DB, (Outcome::Pause, Vec::new()), hissed);
}

#[test]
#[ignore]
fn hiss_sweep() {
    let grid: Vec<u32> = (0..=30).rev().collect();
    sweep(&grid, HISS_WHOLE_DB, HISS_LOSS_DB, (Outcome::Pause, Vec::new()), hissed);
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
/// and the rest of its bits play as its pause. A hole is 2.1 ms for every 0.1 mm of tape a
/// speck of oxide or a crease takes away.
const DROPOUT_WHOLE_MS: f64 = 1.25;
const DROPOUT_LOSS_MS: f64 = 1.5;
/// Without a header to say how long the block is, a hole of 1 ms already ends it.
const HEADERLESS_DROPOUT_WHOLE_MS: f64 = 0.75;
const HEADERLESS_DROPOUT_LOSS_MS: f64 = 1.0;
const DROPOUT_GRID_MS: [f64; 12] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 5.0, 10.0, 20.0, 40.0];

fn dropout_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Short, Said::Collapsed]), Vec::new())
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

/// The edge detector's floor is a share of the loudest material within seconds, and a block
/// that falls under it so soon after a loud one is taken for dead air: at 0.07 of the header's
/// level the data block, 27 dB over the hiss, plays as the header's pause and nothing else says
/// it was there; it does without the hiss too. A quiet program on a collection tape has been measured at 0.07 of the
/// loud ones around it.
const FADE_WHOLE: f32 = 0.08;
const FADE_LOSS: f32 = 0.07;
const FADE_GRID: [f32; 13] = [0.5, 0.3, 0.2, 0.15, 0.12, 0.1, 0.09, 0.08, 0.07, 0.06, 0.05, 0.03, 0.02];

fn fade_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::Collapsed]), Vec::new())
}

#[test]
fn a_block_fading_after_a_loud_one_reads_whole_to_eight_hundredths() {
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
/// and the block is lost past a swing of ±22% at 1 Hz and ±21% at 10 Hz. A capstan's wow is 0.1
/// to 0.3%, a pinch roller's at 1 to 2 Hz about as much: the walls stand far over any deck.
const WOW_1HZ_WHOLE: f64 = 0.22;
const WOW_1HZ_LOSS: f64 = 0.23;
const WOW_10HZ_WHOLE: f64 = 0.21;
const WOW_10HZ_LOSS: f64 = 0.22;
const WOW_GRID: [f64; 13] = [0.01, 0.02, 0.04, 0.06, 0.08, 0.1, 0.12, 0.15, 0.2, 0.21, 0.22, 0.23, 0.25];

/// The ROM's own windows, fixed, load the data block at 23%: its windows measured on the leader
/// are what loses it.
fn wow_1hz_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), vec![1])
}

fn wow_10hz_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::RawSignal]), Vec::new())
}

#[test]
fn wow_reads_whole_to_twenty_two_percent_at_1_hz() {
    holds(WOW_1HZ_WHOLE, WOW_1HZ_LOSS, wow_1hz_lost(), |d| wowed(1.0, d));
}

#[test]
fn flutter_reads_whole_to_twenty_one_percent_at_10_hz() {
    holds(WOW_10HZ_WHOLE, WOW_10HZ_LOSS, wow_10hz_lost(), |d| wowed(10.0, d));
}

#[test]
#[ignore]
fn wow_sweeps() {
    sweep(&WOW_GRID, WOW_1HZ_WHOLE, WOW_1HZ_LOSS, wow_1hz_lost(), |d| wowed(1.0, d));
    sweep(&WOW_GRID, WOW_10HZ_WHOLE, WOW_10HZ_LOSS, wow_10hz_lost(), |d| wowed(10.0, d));
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
/// into the half before them while the click lasts 2 samples, 159 T; at 3 samples, 238 T, the
/// block is lost, though framing folds two pulses up to 485 T. A click is one or two samples of
/// a crackle or a bad splice.
const CLICK_WHOLE_SAMPLES: usize = 2;
const CLICK_LOSS_SAMPLES: usize = 3;

/// The ROM loads the data block through clicks of 3 samples and more: after an edge it waits 468
/// to 653 T before its first read, and a click inside that never shows.
fn click_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), vec![1])
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
fn a_click_behind_an_edge_reads_whole_to_two_samples() {
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

/// Under a sample to a sync 1 and two to a 0 bit's half the half-waves still read down to
/// 4,410 Hz; at 4,000 Hz both blocks come back with bad checksums. Sound cards record at 8 kHz
/// at the least, and 11 kHz is the sensible floor for a tape.
const RATE_WHOLE_HZ: u32 = 4410;
const RATE_LOSS_HZ: u32 = 4000;

fn rate_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::BadChecksum, Said::Collapsed]), Vec::new())
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
fn a_bounce_in_the_sync_loses_the_block() {
    // The fold takes a bounce between two data half-waves; the sync loop reads one pulse at a
    // time and has none, and the first bit's first half has no half before it to fold into. A
    // bounce of a sample at 44.1 kHz at either sync pulse or at the first bit gives a block read
    // a bit out, its checksum bad, at any width from 20 T. The ROM loads it at every phase: it
    // waits 505 T after sync 1 before its first read for sync 2, and 468 T or more after every
    // other edge, and two edges inside that wait are none.
    for (at, end) in [(0, false), (0, true), (1, false), (2, false)] {
        for width in [20, 79] {
            assert_eq!(
                bounced(at, end, width),
                (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), vec![1]),
                "{width} T at pulse {at}"
            );
        }
    }
}

#[test]
fn a_click_in_the_sync_loses_the_block() {
    // One to three samples inside sync 1 or sync 2 of the data block, which the ROM's waits
    // after an edge hide as they hide a bounce.
    let clean = render(&tape());
    let sync = bits_at(&tape(), 1) - ((667.0 + 735.0) * f64::from(RATE) / 3_500_000.0) as usize;
    for (at, width) in [(3, 1), (3, 3), (12, 1)] {
        assert_eq!(
            judge(&spike(&clean, sync + at, width, 0.5), &tape(), (1.0, 1.0)),
            (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), vec![1]),
            "{width} samples at {at}"
        );
    }
}

// Compounds.

/// The tape `factor` times as long, with flutter of `depth` at 10 Hz on top.
fn off_speed_fluttering(factor: f64, depth: f64) -> Judged {
    let x = wow(&speed(&render(&tape()), factor), RATE, depth, 10.0);
    judge(&x, &tape(), (factor * (1.0 - depth), factor * (1.0 + depth)))
}

/// A tape 20% fast puts the pilot's pairs at 3,613 T, and the leader's window, which stays the
/// ROM's whatever the speed, ends at 3,373 T: flutter of 7% takes the pairs out of it and no
/// block locks, the whole tape raw signal. 17.6% slow, the same flutter is nothing (whole at 20%,
/// as far as the sweep goes). A worn belt runs 5% off.
const FAST_FLUTTER_WHOLE: f64 = 0.06;
const FAST_FLUTTER_LOSS: f64 = 0.07;
const FAST: f64 = 1.0 / 1.2;
const SLOW: f64 = 1.0 / 0.85;

fn fast_flutter_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::RawSignal]), Vec::new())
}

#[test]
fn flutter_on_a_tape_twenty_percent_fast_reads_whole_to_six_percent() {
    holds(FAST_FLUTTER_WHOLE, FAST_FLUTTER_LOSS, fast_flutter_lost(), |d| off_speed_fluttering(FAST, d));
    assert_eq!(off_speed_fluttering(SLOW, 0.2), WHOLE);
}

#[test]
#[ignore]
fn speed_and_flutter_sweeps() {
    let grid = [0.0, 0.02, 0.04, 0.05, FAST_FLUTTER_WHOLE, FAST_FLUTTER_LOSS, 0.08, 0.12];
    sweep(&grid, FAST_FLUTTER_WHOLE, FAST_FLUTTER_LOSS, fast_flutter_lost(), |d| {
        off_speed_fluttering(FAST, d)
    });
    for d in [0.0, 0.04, 0.08, 0.12, 0.16, 0.2] {
        assert_eq!(off_speed_fluttering(SLOW, d), WHOLE, "{d}");
    }
}

/// The tape `factor` times as long, its falls slowed by a time constant of `fall_us`.
fn off_speed_skewed(factor: f64, fall_us: f64) -> Judged {
    let x = azimuth(&speed(&render(&tape()), factor), RATE, 10.0, fall_us);
    judge(&x, &tape(), (factor, factor))
}

/// A fall slowed by a time constant `τ` crosses zero late, and every pair keeps its sum but the
/// sync's first pulse and a block's last bit, which are read alone; at 1 ms the header is lost
/// with a bad checksum. Played 17.6% slow, every half-wave is longer against the same delay, and
/// the wall moves out to between 1 and 1.5 ms. A rise and a fall that differ by a quarter of a
/// millisecond are what one track of a worn head gives.
const AZIMUTH_WHOLE_US: f64 = 900.0;
const AZIMUTH_LOSS_US: f64 = 1000.0;
const SLOW_AZIMUTH_WHOLE_US: f64 = 1000.0;
const SLOW_AZIMUTH_LOSS_US: f64 = 1500.0;

fn azimuth_lost() -> Judged {
    (Outcome::Lost(vec![0], vec![Said::BadChecksum, Said::Collapsed]), Vec::new())
}

#[test]
fn a_late_kind_of_edge_reads_whole_to_900_us() {
    holds(AZIMUTH_WHOLE_US, AZIMUTH_LOSS_US, azimuth_lost(), |t| off_speed_skewed(1.0, t));
}

#[test]
fn a_late_kind_of_edge_on_a_slow_tape_reads_whole_to_a_millisecond() {
    holds(SLOW_AZIMUTH_WHOLE_US, SLOW_AZIMUTH_LOSS_US, azimuth_lost(), |t| off_speed_skewed(SLOW, t));
}

#[test]
#[ignore]
fn azimuth_sweeps() {
    let grid = [100.0, 250.0, 350.0, 500.0, 700.0, AZIMUTH_WHOLE_US, AZIMUTH_LOSS_US];
    sweep(&grid, AZIMUTH_WHOLE_US, AZIMUTH_LOSS_US, azimuth_lost(), |t| off_speed_skewed(1.0, t));
    let grid = [25.0, 100.0, 250.0, 500.0, 700.0, SLOW_AZIMUTH_WHOLE_US, SLOW_AZIMUTH_LOSS_US];
    sweep(&grid, SLOW_AZIMUTH_WHOLE_US, SLOW_AZIMUTH_LOSS_US, azimuth_lost(), |t| off_speed_skewed(SLOW, t));
}

fn fast_hissed(db: u32) -> Judged {
    let f = 1.0 / 1.1;
    judge(&hiss(&speed(&render(&tape()), f), db, 0x5EED), &tape(), (f, f))
}

/// Played 10% fast, every half-wave is shorter against the same hiss, and the wall stays where
/// it is without the speed: whole at 11 dB, the next pilot's start drowned at 10.
#[test]
fn hiss_on_a_tape_ten_percent_fast_reads_whole_to_eleven_db() {
    holds(HISS_WHOLE_DB, HISS_LOSS_DB, (Outcome::Pause, Vec::new()), fast_hissed);
}

#[test]
#[ignore]
fn speed_and_hiss_sweep() {
    sweep(
        &[14, 13, 12, HISS_WHOLE_DB, HISS_LOSS_DB, 9],
        HISS_WHOLE_DB,
        HISS_LOSS_DB,
        (Outcome::Pause, Vec::new()),
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
/// the treble wall where it is; past it nothing at all is left of the tape, no block, no raw
/// signal and no finding. At 5 µm the midline follows a wander up to 0.3, three fifths of the
/// level; at 0.4 again nothing is left. A worn deck's low end carries 10% of the level under
/// 300 Hz on a tape that reads, 23% on one that does not.
const WANDER: f32 = 0.025;
const WANDER_AT_UM: f64 = 5.0;
const WANDER_WHOLE: f32 = 0.3;
const WANDER_LOSS: f32 = 0.4;

fn nothing_left() -> Judged {
    (Outcome::Lost(vec![0, 1], Vec::new()), Vec::new())
}

#[test]
fn treble_loss_with_a_wandering_centre_line_reads_whole_to_ten_and_a_half_microns() {
    holds(TREBLE_WHOLE_UM, TREBLE_LOSS_UM, nothing_left(), |d| dull_wandering(d, WANDER));
}

#[test]
fn a_wandering_centre_line_on_a_dull_tape_reads_whole_to_three_fifths_of_the_level() {
    holds(WANDER_WHOLE, WANDER_LOSS, nothing_left(), |w| dull_wandering(WANDER_AT_UM, w));
}

#[test]
#[ignore]
fn treble_and_wander_sweeps() {
    sweep(&TREBLE_GRID_UM[..12], TREBLE_WHOLE_UM, TREBLE_LOSS_UM, nothing_left(), |d| {
        dull_wandering(d, WANDER)
    });
    let grid = [0.01, 0.025, 0.05, 0.1, 0.15, 0.2, WANDER_WHOLE, WANDER_LOSS, 0.5];
    sweep(&grid, WANDER_WHOLE, WANDER_LOSS, nothing_left(), |w| dull_wandering(WANDER_AT_UM, w));
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
fn pilot_hole(pairs: usize, hole: u64) -> Judged {
    let p = of_blocks(&tape()).unwrap();
    let mut lens: Vec<u64> = p.iter().map(|q| q.tstates.round() as u64).collect();
    let sync = (1..lens.len()).filter(|&i| lens[i] == 667 && lens[i - 1] == 2168).nth(1).unwrap();
    lens[sync - 2 * pairs - 1] = hole;
    let p = common::pulses(&lens);
    let got = decode_pulses(&p, 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    (outcome(&got, &tape(), (1.0, 1.0)), rom_only(&p, 3_500_000, &tape()))
}

/// A hole in the pilot restarts the lock, which needs the ROM's 256 pairs, so the block is lost
/// whenever fewer than 257 pairs follow the hole, and is read whole from 257 on. The ROM's loader
/// loses it the same way, at every phase. Two recordings are this cell: a 25 ms
/// hole 37 pairs before the sync, and a recording cut 200 pairs before it (a lock of 32
/// pulses reads both).
const PILOT_HOLE_WHOLE_PAIRS: usize = 257;
const PILOT_HOLE_LOSS_PAIRS: usize = 256;
/// 25 ms, as measured.
const PILOT_HOLE_T: u64 = 88_290;

/// The data block raw signal, and the pilot pulses before the hole played as the header's
/// pause (`Collapsed`) where the hole lies past the lock's start.
fn pilot_hole_lost(j: &Judged) -> bool {
    matches!(&j.0, Outcome::Lost(blocks, said) if blocks == &[1] && said.contains(&Said::RawSignal))
        && j.1.is_empty()
}

#[test]
fn a_hole_in_the_pilot_loses_the_block_unless_256_pairs_follow_it() {
    // The pilot pulses before the hole play as the header's pause, which is then longer.
    assert_eq!(pilot_hole(PILOT_HOLE_WHOLE_PAIRS, PILOT_HOLE_T), (Outcome::Pause, Vec::new()));
    for n in [PILOT_HOLE_LOSS_PAIRS, 200, 37] {
        let got = pilot_hole(n, PILOT_HOLE_T);
        assert!(pilot_hole_lost(&got), "{n}: {got:?}");
    }
}

#[test]
#[ignore]
fn pilot_hole_sweep() {
    for n in [400, 300, PILOT_HOLE_WHOLE_PAIRS] {
        assert_eq!(pilot_hole(n, PILOT_HOLE_T), (Outcome::Pause, Vec::new()), "{n}");
    }
    for n in [PILOT_HOLE_LOSS_PAIRS, 200, 100, 37] {
        let got = pilot_hole(n, PILOT_HOLE_T);
        assert!(pilot_hole_lost(&got), "{n}: {got:?}");
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
fn a_glitch_in_the_pilot_loses_the_block_the_rom_loads() {
    // Three pulses of 1,300, 3,097 and 1,543 T in the pilot, as a recording has
    // them 147 pairs before its sync: framing takes them for the sync, or restarts, and the
    // block is raw signal, where the ROM's leader loop, which asks each pulse only to be long
    // enough and counts on, loads the block at every phase. A glitch 1,000 pulses out is nothing.
    const GLITCH: [u64; 3] = [1300, 3097, 1543];
    for back in [100, 400] {
        assert_eq!(
            pilot_glitch(&GLITCH, back),
            (Outcome::Lost(vec![1], vec![Said::RawSignal]), vec![1]),
            "{back}"
        );
    }
    assert_eq!(pilot_glitch(&GLITCH, 1000), WHOLE);
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
