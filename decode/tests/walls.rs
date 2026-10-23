// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The decoder's walls: for each defect of the cassette chain (`common::degrade`), the last step
//! a fast test reads a tape whole through, and the loss one step past that (which blocks are
//! missing and how the decode says so). A block whose checksum holds over bytes the tape does
//! not have is wrong unless the decode flags a problem of it (a block cut short can check out
//! with its last byte filled to 0).
//!
//! Each fast test has an `#[ignore]`d sweep that checks every step up to the wall and no wrong
//! bytes past it, run with `cargo test --release -p tapeti-decode --test walls -- --ignored`; it
//! also runs the ROM's loader over the same pulses at every step and names any block the ROM
//! loads that framing does not. There is none up to any wall; past the 1 Hz wow and the flutter
//! on a slow tape, the ROM still loads what framing loses.
//!
//! Where the signal as it is loses something, the band-pass decode is folded in
//! (`entry::decode_file`); several walls stand where it carries them, as their comments say. A
//! wall measured through the band-pass alone, as a user who picks that filter gets it, is only
//! the treble one.

mod common;

use common::degrade::*;
use common::rom::unread;
use common::same_tape;
use std::fmt::Debug;
use std::sync::atomic::AtomicBool;
use tapeti_core::audio::{encode_wav, playback_order, render_tape, FlowOptions, RenderOptions};
use tapeti_core::types::{Block, Body};
use tapeti_decode::entry::{decode_file, decode_pulses, tape_band, Channels, Decoded, Finding, PilotHeard};
use tapeti_decode::pulses::{of_blocks, Pulse};
use tapeti_decode::readers::Channel;
use tapeti_decode::waveform::edges::{detect_pulses, detect_pulses_band, Axis, Band};
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
    /// No block read in the whole recording, and no pilot heard in it.
    NoBlock,
    /// No block read in the whole recording, where a reading heard a pilot in pieces the ROM
    /// never locks on.
    NoBlockPieces,
    /// No block read in the whole recording, where a reading heard a pilot the ROM locks on.
    NoBlockPilot,
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
            Finding::NoBlock { pilot: PilotHeard::None, .. } => Some(Said::NoBlock),
            Finding::NoBlock { pilot: PilotHeard::Pieces, .. } => Some(Said::NoBlockPieces),
            Finding::NoBlock { pilot: PilotHeard::Whole, .. } => Some(Said::NoBlockPilot),
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

/// Hiss moves every crossing and cuts false edges into weak lobes, passed over as glitches in a
/// pilot. The signal as it is reads whole to 10 dB under it; from 9 the band-pass, which takes
/// out the hiss above the tape's tones, carries the wall, and from 7, where the noise line takes
/// the whole recording for dead air, its decode is kept whole, to 5 dB. At 4 the data block's
/// checksum fails, and a sync the hiss makes inside the header's pilot reads a byte there, the
/// header's pilot behind it still the header's; at 1 no block is read, and the decode says so.
/// A clean recording lies 34 dB or more over its hiss; a noisy one 14 to 26.
const HISS_WHOLE_DB: u32 = 5;
const HISS_LOSS_DB: u32 = 4;

fn hiss_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::RawSignal, Said::Collapsed]), Vec::new())
}

#[test]
fn hiss_reads_whole_to_five_db() {
    holds(HISS_WHOLE_DB, HISS_LOSS_DB, hiss_lost(), hissed);
}

/// Past the hiss wall the band-pass reads what the signal as it is does not, and where no block
/// reads at all the tape's pilot is still heard: as runs of a few dozen pairs the hiss breaks
/// it into, short of the ROM's 256, at 1 dB on the band-pass's zero line some 250 of them, which
/// together hold a leader many times over (`fallback::pilot_in_pieces`). Hiss alone as loud
/// holds no run of even four pairs: no pilot.
#[test]
fn hiss_that_leaves_the_signal_no_block_is_read_through_the_band_pass_and_a_pilot_in_pieces_is_said() {
    let at = |x: &[f32]| decode(&encode_wav(x, RATE, 16), Channels::Only(Channel::Mix));
    let x = render(&tape());
    assert_eq!(at(&hiss(&x, 7, 0x5EED)).findings[0], Finding::BandPassOnly);
    for db in [1, 0] {
        let none = at(&hiss(&x, db, 0x5EED));
        assert!(
            matches!(none.findings[..], [Finding::NoBlock { pilot: PilotHeard::Pieces, .. }]),
            "{db} dB: {:?}",
            none.findings
        );
        assert!(none.findings[0].is_problem());
        let alone = at(&hiss_under(&vec![0.0; x.len()], rms(&x), db, 0x5EED));
        assert!(
            matches!(alone.findings[..], [Finding::NoBlock { pilot: PilotHeard::None, .. }]),
            "hiss alone at {db} dB: {:?}",
            alone.findings
        );
    }
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
/// Without a header to say how long the block is, a hole of 1 ms ends it as the signal is; the
/// band-pass, whose ringing carries an edge over the hole, reads it to 1 ms, and 1.25 ends it.
const HEADERLESS_DROPOUT_WHOLE_MS: f64 = 1.0;
const HEADERLESS_DROPOUT_LOSS_MS: f64 = 1.25;
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
fn a_dropout_in_a_headerless_block_reads_whole_to_a_millisecond() {
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

/// A click a sample behind an edge leaves two short pulses that fold into a half-wave beside
/// them, which the ROM's wait of 468 to 653 T after an edge never sees, while the sample before
/// the click and the click together last under 485 T. The signal as it is, and the ROM at some
/// phases, lose the block from 6 samples; the band-pass, which rounds the click off, reads it to
/// 7, and 8 lose it too. A click is one or two samples of a crackle or a bad splice.
const CLICK_WHOLE_SAMPLES: usize = 7;
const CLICK_LOSS_SAMPLES: usize = 8;

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
/// more, and no fold can tell its pulses from a bit's: on the signal as it is the block is lost
/// from 0.3 on a level of 0.5. Through the band-pass a click of one sample is rounded off under
/// the band at any height (tried to ten times the level), and the block reads from there.
const CLICK_INSIDE_HEIGHTS: [f32; 6] = [0.05, 0.2, 0.3, 0.5, 1.0, 5.0];

#[test]
fn a_click_behind_an_edge_reads_whole_to_seven_samples() {
    holds(CLICK_WHOLE_SAMPLES, CLICK_LOSS_SAMPLES, click_lost(), clicked_at_edges);
}

#[test]
fn a_click_of_one_sample_inside_a_half_wave_reads_whole_at_any_height() {
    for h in [0.3, 5.0] {
        assert_eq!(clicked_inside(h), WHOLE, "at {h}");
    }
}

#[test]
#[ignore]
fn click_sweeps() {
    let grid: Vec<usize> = (1..=10).collect();
    sweep(&grid, CLICK_WHOLE_SAMPLES, CLICK_LOSS_SAMPLES, click_lost(), clicked_at_edges);
    for h in CLICK_INSIDE_HEIGHTS {
        assert_eq!(clicked_inside(h), WHOLE, "at {h}");
    }
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
    // same, whole at 5 dB and the data block lost at 4.
    let at = |db: u32| judge_at(&hiss(&render(&tape()), db, 0x5EED), RATE, 8, &tape(), (1.0, 1.0));
    holds(HISS_WHOLE_DB, HISS_LOSS_DB, hiss_lost(), at);
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

/// A fall slowed by a time constant `τ` crosses zero late; every pair keeps its sum but the
/// sync's first pulse and a block's last bit, which are read alone. From 3.5 ms a run of 0 bits
/// loses its short lobes and the fade band carries the wall, from 4 ms the band-pass, from 5 ms
/// the midline, and further on the band-pass decode kept whole: the tape reads whole to a fall
/// of 45 ms, two hundred times what a worn head gives; at 47.5 the header comes back with a bad
/// checksum, the rest of it said lost, and the data block is not read. Played 17.6% slow, from
/// 2 ms the pilot's long halves pass 3,644 T, where the ROM's sync loop runs out, and the fade
/// band reads the header; it reads whole to a fall of 4,750 µs, and at 5,000 the data block's
/// pilot rises out of the header's pause 3.5% late. A rise and a fall that differ by a quarter
/// of a millisecond are what one track of a worn head gives.
const AZIMUTH_WHOLE_US: f64 = 45_000.0;
const AZIMUTH_LOSS_US: f64 = 47_500.0;
const SLOW_AZIMUTH_WHOLE_US: f64 = 4750.0;
const SLOW_AZIMUTH_LOSS_US: f64 = 5000.0;

fn azimuth_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::BadChecksum, Said::Lost]), Vec::new())
}

fn slow_azimuth_lost() -> Judged {
    (Outcome::Pause, Vec::new())
}

#[test]
fn a_late_kind_of_edge_reads_whole_to_45_ms() {
    holds(AZIMUTH_WHOLE_US, AZIMUTH_LOSS_US, azimuth_lost(), |t| off_speed_skewed(1.0, t));
}

#[test]
fn a_late_kind_of_edge_on_a_slow_tape_reads_whole_to_4750_us() {
    holds(SLOW_AZIMUTH_WHOLE_US, SLOW_AZIMUTH_LOSS_US, slow_azimuth_lost(), |t| off_speed_skewed(SLOW, t));
}

#[test]
#[ignore]
fn azimuth_sweeps() {
    let grid = [
        100.0,
        500.0,
        1000.0,
        2000.0,
        3000.0,
        4000.0,
        5000.0,
        6000.0,
        8000.0,
        10_000.0,
        15_000.0,
        20_000.0,
        30_000.0,
        40_000.0,
        AZIMUTH_WHOLE_US,
        AZIMUTH_LOSS_US,
        100_000.0,
    ];
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

/// Played 10% fast, every half-wave is shorter against the same hiss, and the band-pass reads
/// it a step less far than at the tape's speed: whole at 6 dB, and at 5 every block read but a
/// stretch of the data block's pause kept as raw signal, a run of hiss shaped like a block.
const FAST_HISS_WHOLE_DB: u32 = 6;
const FAST_HISS_LOSS_DB: u32 = 5;

fn fast_hiss_lost() -> Judged {
    (Outcome::Lost(Vec::new(), vec![Said::RawSignal, Said::Collapsed]), Vec::new())
}

#[test]
fn hiss_on_a_tape_ten_percent_fast_reads_whole_to_six_db() {
    holds(FAST_HISS_WHOLE_DB, FAST_HISS_LOSS_DB, fast_hiss_lost(), fast_hissed);
}

#[test]
#[ignore]
fn speed_and_hiss_sweep() {
    sweep(
        &[14, 12, 10, 9, 8, 7, FAST_HISS_WHOLE_DB, FAST_HISS_LOSS_DB, 4, 2, 0],
        FAST_HISS_WHOLE_DB,
        FAST_HISS_LOSS_DB,
        fast_hiss_lost(),
        fast_hissed,
    );
}

/// The tape through `microns` of spacing, with a wandering centre line: a triangle of `wander`
/// at 120 Hz.
fn dull_wandering(microns: f64, wander: f32) -> Judged {
    judge(&hum(&spacing_loss(&render(&tape()), RATE, microns), RATE, 120.0, wander), &tape(), (1.0, 1.0))
}

/// Treble loss shrinks the short lobes and a wander lifts them off the zero line; the midline
/// reads them about the signal's own centre. A wander of 0.025 leaves the treble wall where it
/// is (`dead_air_takes_the_noise_line_out_of_the_signal`); past it both blocks come back from the
/// midline with bad checksums, where the zero line reads no block at all. At 5 µm of treble loss
/// the midline reads whole to a wander of 0.9, where the zero line has read no block since 0.4;
/// at 1.0 the midline's data block stops short, its filled-out checksum proving nothing, and the
/// band-pass, which takes the 120 Hz wander out, reads whole to 1.1; at 1.2 neither block is
/// read, the header's stretch kept as raw signal. A worn deck's low end carries 10% of the level
/// under 300 Hz on a tape that reads, 23% on one that does not.
const WANDER: f32 = 0.025;
const WANDER_AT_UM: f64 = 5.0;
const WANDER_WHOLE: f32 = 1.1;
const WANDER_LOSS: f32 = 1.2;

fn wander_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::RawSignal]), Vec::new())
}

fn treble_wander_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::BadChecksum]), Vec::new())
}

#[test]
fn treble_loss_with_a_wandering_centre_line_reads_whole_to_eleven_and_a_half_microns() {
    holds(TREBLE_WHOLE_UM, TREBLE_LOSS_UM, treble_wander_lost(), |d| dull_wandering(d, WANDER));
}

#[test]
fn a_wandering_centre_line_on_a_dull_tape_reads_whole_to_eleven_tenths() {
    holds(WANDER_WHOLE, WANDER_LOSS, wander_lost(), |w| dull_wandering(WANDER_AT_UM, w));
}

/// The tape with hum at mains rate, a triangle of `amplitude` at 50 Hz.
fn hummed(amplitude: f32) -> Judged {
    judge(&hum(&render(&tape()), RATE, 50.0, amplitude), &tape(), (1.0, 1.0))
}

/// The DC blocker passes 50 Hz at 0.98; past 0.5 the hum and the tape together pass full scale
/// and the recording clips on its crests, where at 1.4 the tape stands a tenth of its height. It
/// reads whole to a hum of 1.4, the midline reading the header and the band-pass the data block
/// where the zero line has read none since 0.6; at 1.5 every block comes back with a bad
/// checksum, and from 1.6 none is read, as the decode says. Mains hum on a deck is a few percent
/// of the level.
const HUM_WHOLE: f32 = 1.4;
const HUM_LOSS: f32 = 1.5;

fn hum_lost() -> Judged {
    (Outcome::Lost(vec![0, 1], vec![Said::BadChecksum, Said::Collapsed, Said::Lost]), Vec::new())
}

#[test]
fn hum_at_mains_rate_reads_whole_to_nearly_three_times_the_tapes_level() {
    holds(HUM_WHOLE, HUM_LOSS, hum_lost(), hummed);
}

#[test]
#[ignore]
fn hum_sweep() {
    sweep(
        &[0.05, 0.1, 0.3, 0.5, 0.8, 1.0, 1.3, HUM_WHOLE, HUM_LOSS, 2.0],
        HUM_WHOLE,
        HUM_LOSS,
        hum_lost(),
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

/// Every wall's tape holds 2.25 s of silence in 10.8 s; the edge detector measures the noise and
/// its floor over the quietest seconds, the fifth of ten (`QUIET_SECONDS`), mostly signal on
/// these tapes. Treble loss, alone and with a wander, keeps its wall in any dead air: whole at
/// 11.5 µm with two or ten seconds of it, lost at 12. Hiss, measured against the tape and not
/// the dead air too, reads whole to 8 dB; at 7 the data block's checksum fails on every line and
/// through the band-pass. On the bare tape the band-pass decode, kept whole where the plain one
/// reads no block from 7 dB, reads to 5; over dead air the noise line stands on the hiss alone,
/// the plain decode reads the header, and the band-pass loses the data block. A wander on a dull
/// tape keeps its wall, 1.1, since it too lies over the dead air.
/// Whole, the last block's pause holds the dead air behind it.
#[test]
fn dead_air_takes_the_noise_line_out_of_the_signal() {
    let treble_wander = |x: &[f32]| hum(&spacing_loss(x, RATE, TREBLE_WHOLE_UM), RATE, 120.0, WANDER);
    assert_eq!(in_dead_air(2.0, treble_wander), (Outcome::Pause, Vec::new()));
    assert_eq!(in_dead_air(10.0, |x| spacing_loss(x, RATE, TREBLE_WHOLE_UM)), (Outcome::Pause, Vec::new()));
    assert_eq!(
        in_dead_air(10.0, |x| spacing_loss(x, RATE, TREBLE_LOSS_UM)),
        (Outcome::Lost(vec![0, 1], vec![Said::RawSignal, Said::Collapsed, Said::Lost]), Vec::new())
    );
    assert_eq!(in_dead_air(10.0, |x| hiss_as_bare(x, 8)), (Outcome::Pause, Vec::new()));
    assert_eq!(
        in_dead_air(10.0, |x| hiss_as_bare(x, 7)),
        (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::RawSignal, Said::Collapsed]), Vec::new())
    );
    let wandering = |w: f32| move |x: &[f32]| hum(&spacing_loss(x, RATE, WANDER_AT_UM), RATE, 120.0, w);
    assert_eq!(in_dead_air(10.0, wandering(WANDER_WHOLE)), (Outcome::Pause, Vec::new()));
    assert_eq!(in_dead_air(10.0, wandering(WANDER_LOSS)), wander_lost());
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
/// read wrong, each read on two lines or more and on four where the band-pass is walked, so one
/// or two such steps are to be expected; no reading of any of them holds the tape's bytes, so no
/// rule could read them right. The second's block runs on
/// past the length its header asked, which the saver never writes; no recording has shown a
/// header's block running on to judge a rule against, and its two run-ons are pulses in the
/// pause of a header read right.
const DEAD_AIR_CHANCE: [&str; 5] = [
    // A 245-byte headerless misread behind a header read wrong.
    "treble 12 in 5 s",
    // Both lines slip in the dip by two and four bytes, and the midline's checks out.
    "dip, hiss 17 in 2 s",
    // Four points past the wall, every line slips two bits out near byte 96, and the band-pass
    // zero line's slipped reading checks out.
    "wow 1 Hz 0.26 in 5 s",
    // In front of the tape, which the band-pass reads whole, a byte of hiss read as a block
    // whose checksum is that byte again.
    "fast hiss 3 in 2 s",
    // The data block read through the band-pass behind a short leader and seven unlike halves,
    // its checksum holding over bytes it does not have.
    "hiss over all 1 in 2 s",
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
    let grid = [0.01, 0.05, 0.1, 0.2, 0.4, 0.6, 0.8, 1.0, WANDER_WHOLE, WANDER_LOSS, 1.5];
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
/// with fewer, at every phase; the decoder locks on 16 (`SHORT_LEADER_PAIRS`) behind the one the
/// leader takes unchecked, so the block reads whole at 17 pairs, its pause as long as the tape's
/// but for the hole's own time (`framing::tone_start`): a 25 ms hole 37 pairs before the sync,
/// and a cut 200 pairs before it, are both this cell. At 16 pairs no tone is left that makes the
/// stretch block-shaped: it plays as the header's pause, and the pilot before the hole, which the
/// ROM locks on, is said lost there.
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
    // The pilot pulses before the hole are the block's own pilot, not the header's pause; the
    // block is read, and nothing is said lost.
    for n in [257, 37, PILOT_HOLE_WHOLE_PAIRS] {
        assert_eq!(pilot_hole(n, PILOT_HOLE_T), WHOLE, "{n}");
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
        assert_eq!(pilot_hole(n, PILOT_HOLE_T), WHOLE, "{n}");
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
/// out by about as much on the signal as it is: whole at 20 dB, a bad checksum at 19. The
/// band-pass takes out the hiss above the tape's tones and reads it to 14 dB; at 13 the data
/// block's checksum is bad. At 11.5 dB with two such dips in a row, each loses a byte, and no
/// channel reads whole.
const DIP_HISS_WHOLE_DB: u32 = 14;
const DIP_HISS_LOSS_DB: u32 = 13;
const DIP_DEPTH: f32 = 0.25;

fn dip_hiss_lost() -> Judged {
    (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::RawSignal, Said::Collapsed]), Vec::new())
}

#[test]
fn a_dip_to_a_quarter_under_hiss_reads_whole_to_fourteen_db() {
    holds(DIP_HISS_WHOLE_DB, DIP_HISS_LOSS_DB, dip_hiss_lost(), |db| hissed_dip(db, DIP_DEPTH));
}

#[test]
#[ignore]
fn dip_and_hiss_sweep() {
    sweep(
        &[30, 25, 20, 17, DIP_HISS_WHOLE_DB, DIP_HISS_LOSS_DB, 11],
        DIP_HISS_WHOLE_DB,
        DIP_HISS_LOSS_DB,
        dip_hiss_lost(),
        |db| hissed_dip(db, DIP_DEPTH),
    );
}

// Low-pass.

/// The tape through a low-pass of three one-pole sections at `hz` (`low_pass`), under hiss
/// `db` under its level where `db` is given.
fn low_passed(hz: f64, db: Option<u32>) -> Judged {
    let x = low_pass(&render(&tape()), RATE, hz, 3);
    let x = match db {
        Some(db) => hiss_under(&x, rms(&render(&tape())), db, 0x5EED),
        None => x,
    };
    judge(&x, &tape(), same(hz))
}

/// Three poles delay every crossing by their group delay, more for a long lobe than a short one,
/// so the half-waves change width, which no midline undoes; but the pairs keep their sums to the
/// split, and the tape reads whole down to a corner of 800 Hz. At 750 Hz both blocks come back
/// with bad checksums, and the ROM loads neither at every phase. Under 20 dB of hiss the
/// narrowed lobes cross nearer the band: whole to 1,400 Hz, and at 1,250 the data block's
/// checksum fails. A deck's own roll-off lies past 10 kHz; a resampler's at 4 kHz.
const LOW_PASS_WHOLE_HZ: f64 = 800.0;
const LOW_PASS_LOSS_HZ: f64 = 750.0;
const LOW_PASS_HISS_WHOLE_HZ: f64 = 1400.0;
const LOW_PASS_HISS_LOSS_HZ: f64 = 1250.0;
const LOW_PASS_GRID_HZ: [f64; 10] =
    [4000.0, 3000.0, 2000.0, 1500.0, 1400.0, 1250.0, 1000.0, 900.0, 800.0, 750.0];

#[test]
fn a_three_pole_low_pass_reads_whole_down_to_800_hz() {
    holds(
        LOW_PASS_WHOLE_HZ,
        LOW_PASS_LOSS_HZ,
        (Outcome::Lost(vec![0, 1], vec![Said::BadChecksum, Said::Lost]), Vec::new()),
        |hz| low_passed(hz, None),
    );
    holds(
        LOW_PASS_HISS_WHOLE_HZ,
        LOW_PASS_HISS_LOSS_HZ,
        (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), Vec::new()),
        |hz| low_passed(hz, Some(20)),
    );
}

#[test]
#[ignore]
fn low_pass_sweeps() {
    let lost = (Outcome::Lost(vec![0, 1], vec![Said::BadChecksum, Said::Lost]), Vec::new());
    sweep(&LOW_PASS_GRID_HZ, LOW_PASS_WHOLE_HZ, LOW_PASS_LOSS_HZ, lost, |hz| low_passed(hz, None));
    let lost = (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), Vec::new());
    sweep(&LOW_PASS_GRID_HZ[..6], LOW_PASS_HISS_WHOLE_HZ, LOW_PASS_HISS_LOSS_HZ, lost, |hz| {
        low_passed(hz, Some(20))
    });
}

// Seams the walls' tape crosses with a pause, and the defects at them.

/// A data block no header asks for, the walls' header and its data block, with `pause` ms
/// behind the first two: 0 is a tape saved with no pause, whose blocks run into the next
/// block's pilot.
fn three_blocks(pause: u16) -> Vec<Block> {
    let mut a = vec![0xFF];
    a.extend((0..100u32).map(|i| (i * 53 + 7) as u8));
    let mut t = tape();
    if let Body::Standard { pause: p, .. } = &mut t[0].body {
        *p = pause;
    }
    vec![Block::new(Body::Standard { pause, data: with_checksum(a) }), t[0].clone(), t[1].clone()]
}

/// `defect` on [`three_blocks`] with `pause`, judged at any speed.
fn on_three(pause: u16, defect: impl Fn(&[f32]) -> Vec<f32>) -> Judged {
    let t = three_blocks(pause);
    judge(&defect(&render(&t)), &t, (0.0, f64::MAX))
}

/// A defect of the walls, on samples at [`RATE`].
type Defect = Box<dyn Fn(&[f32]) -> Vec<f32>>;

/// A headerless block with no pause behind it ends where the next block's pilot starts, and only
/// the pilot says so; the defects of the walls, at a deck's range and well past it, leave that
/// seam whole. Under wow and hum it holds to the tape's own walls, whole to 16% of wow at 1 Hz
/// and to hum at 1.4: measured against the block's own pilot, seconds earlier, the next pilot
/// under wow lies out of the 10% a run allows, and under clipped hum one half of every crest
/// pairs with neither neighbour, so byte by byte at the speed of the bits just read both are the
/// pilot (`framing::tone_at`), and so is the data block's pilot the header runs into, at the
/// header's own last bits (`framing::bits_end`). Flutter at 10 Hz reads every block to 22%, the
/// tape's own wall with a pause: a lead-in each of whose halves is longer than any 1 bit's ends
/// the block at any bit (`framing::lead_in_at`). At 18% and 20% the header's bits run on into its
/// block's pilot and the stub is kept as raw signal; at 24% the headerless block is lost, as the
/// paused tape loses a block. Past the seam it goes first under hiss: whole to 6 dB, and at 5 the
/// header behind the headerless block reads with a bad checksum where with a pause it reads (the
/// tape's own wall is 5). A deck's wow is 0.3%.
#[test]
fn a_tape_saved_with_no_pause_reads_whole_under_the_walls_defects() {
    let lvl = rms(&render(&tape()));
    let whole: Vec<(&str, Defect)> = vec![
        ("clean", Box::new(|x: &[f32]| x.to_vec())),
        ("hiss 20", Box::new(move |x: &[f32]| hiss_under(x, lvl, 20, 0x5EED))),
        ("hiss 6", Box::new(move |x: &[f32]| hiss_under(x, lvl, 6, 0x5EED))),
        ("treble 3", Box::new(|x: &[f32]| spacing_loss(x, RATE, 3.0))),
        ("treble 10", Box::new(|x: &[f32]| spacing_loss(x, RATE, 10.0))),
        ("wow 1 Hz 0.1", Box::new(|x: &[f32]| wow(x, RATE, 0.1, 1.0))),
        ("wow 1 Hz 0.16", Box::new(|x: &[f32]| wow(x, RATE, 0.16, 1.0))),
        ("flutter 0.08", Box::new(|x: &[f32]| wow(x, RATE, 0.08, 10.0))),
        ("flutter 0.16", Box::new(|x: &[f32]| wow(x, RATE, 0.16, 10.0))),
        ("flutter 0.22", Box::new(|x: &[f32]| wow(x, RATE, 0.22, 10.0))),
        ("clip", Box::new(|x: &[f32]| clip(&spacing_loss(x, RATE, 3.0), 16.0))),
        ("invert", Box::new(|x: &[f32]| invert(x))),
        ("ripple 0.9", Box::new(|x: &[f32]| ripple(x, RATE, 0.9, 2400.0))),
        ("azimuth 250", Box::new(|x: &[f32]| azimuth(x, RATE, 10.0, 250.0))),
        ("azimuth 4000", Box::new(|x: &[f32]| azimuth(x, RATE, 10.0, 4000.0))),
        ("wander 0.9", Box::new(|x: &[f32]| hum(&spacing_loss(x, RATE, 5.0), RATE, 120.0, 0.9))),
        ("hum 1.4", Box::new(|x: &[f32]| hum(x, RATE, 50.0, 1.4))),
        ("low pass", Box::new(|x: &[f32]| low_pass(x, RATE, 1000.0, 3))),
        ("fast", Box::new(|x: &[f32]| speed(x, 1.0 / 1.2))),
        ("slow", Box::new(|x: &[f32]| speed(x, 1.0 / 0.85))),
    ];
    for (name, defect) in &whole {
        assert_eq!(on_three(0, defect), WHOLE, "{name}");
    }
    let t = three_blocks(0);
    for (rate, bits) in [(8000, 8), (4410, 16)] {
        assert_eq!(
            judge_at(&downsample(&render(&t), RATE, rate), rate, bits, &t, (0.0, f64::MAX)),
            WHOLE,
            "{rate} Hz"
        );
    }
    assert_eq!(judge_at(&resample_sharp(&render(&t), RATE, 5000), 5000, 16, &t, (0.0, f64::MAX)), WHOLE);
    let mut y = vec![0.0; ms(5000.0)];
    y.extend(render(&t));
    y.extend(vec![0.0; ms(5000.0)]);
    assert_eq!(judge(&y, &t, (0.0, f64::MAX)), WHOLE, "in dead air");
    for lag in [2.0, 7.0, 20.0] {
        let (l, r) = head_skew(&render(&t), lag);
        let got = decode(&stereo_wav(&l, &r, RATE, 16), Channels::Best);
        assert_eq!(outcome(&got, &t, (0.0, f64::MAX)), Outcome::Whole, "head skew {lag}");
    }
    // One step past each.
    let hissed = |db| move |x: &[f32]| hiss_under(x, lvl, db, 0x5EED);
    assert_eq!(
        on_three(0, hissed(5)),
        (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), Vec::new())
    );
    assert_eq!(on_three(1000, hissed(5)), WHOLE);
    let wowed = |d| move |x: &[f32]| wow(x, RATE, d, 1.0);
    let cut = (Outcome::Lost(vec![2], vec![Said::BadChecksum, Said::Short, Said::Lost]), vec![2]);
    assert_eq!(on_three(0, wowed(0.17)), cut);
    assert_eq!(on_three(1000, wowed(0.16)), cut);
    let fluttered = |d| move |x: &[f32]| wow(x, RATE, d, 10.0);
    assert_eq!(
        on_three(0, fluttered(0.18)),
        (Outcome::Lost(Vec::new(), vec![Said::RawSignal, Said::Collapsed]), Vec::new())
    );
    assert_eq!(on_three(1000, fluttered(0.18)), WHOLE);
    assert_eq!(on_three(0, fluttered(0.2)), (Outcome::Lost(Vec::new(), vec![Said::RawSignal]), Vec::new()));
    assert_eq!(
        on_three(0, fluttered(0.24)),
        (Outcome::Lost(vec![0], vec![Said::RawSignal, Said::Collapsed]), Vec::new())
    );
    assert_eq!(on_three(1000, fluttered(0.24)), (Outcome::Lost(vec![0], vec![Said::RawSignal]), Vec::new()));
}

/// The tape's level dropping or rising tenfold where one block runs into the next with no
/// pause, a copy spliced from two sources: the band follows the level up, and down over the
/// ±10 ms of the envelope, in which the quieter pilot's first half-waves are under it. Every
/// byte is read; where the level drops, the quiet pilot's first milliseconds play as a pause of
/// the block in front, which the tape does not have.
#[test]
fn a_level_that_jumps_where_a_block_runs_into_the_next_reads_every_byte() {
    let t = three_blocks(0);
    let x = render(&t);
    let first = render(&t[..1]).len();
    let at = |lo: f32, hi: f32| {
        let y: Vec<f32> = x.iter().enumerate().map(|(i, &v)| v * if i < first { lo } else { hi }).collect();
        judge(&hiss_under(&y, 0.5, 50, 1), &t, (0.0, f64::MAX))
    };
    for (lo, hi) in [(0.1, 1.0), (0.3, 1.0), (1.0, 0.3)] {
        assert_eq!(at(lo, hi), WHOLE, "{lo} then {hi}");
    }
    assert_eq!(at(1.0, 0.1), (Outcome::Lost(Vec::new(), Vec::new()), Vec::new()));
}

/// The tape with a snag over the data block's run of 0xFF bytes, slowing it to `depth` from
/// `back` ms before its last edge.
fn snagged_ones(back: f64, depth: f64) -> Judged {
    let clean = render(&tape());
    let edges = edges_in(&clean, bits_at(&tape(), 1), clean.len());
    let end = edges[edges.len() - 2];
    judge(&snag(&clean, end - ms(back), ms(70.0), ms(15.0), ms(15.0), depth), &tape(), same(depth))
}

/// A run of 1 bits slowed by a snag past 1.14 of the pilot's speed pairs as the next block's
/// pilot does (`framing::pilot_run_at`), and the block ends there, short of its header's length,
/// where the ROM, which stops only at that length, loads it: lost from 1.2. A snag over the data
/// block's last 0xFF bytes, from where they start or their middle, reads whole to the snag's own
/// wall regardless, since only a pilot as long as the ROM's leader ends a block short.
#[test]
fn a_snag_over_a_run_of_one_bits_reads_whole() {
    for back in [170.0, 120.0] {
        for depth in [1.2, 1.3, 1.5] {
            assert_eq!(snagged_ones(back, depth), WHOLE, "{back} ms, {depth}");
        }
    }
}

/// Eight clicks of `width` samples and `height` in the middle of half-waves of the data block
/// from its `first` edge on, every 250th, or one where `first` is 0, the flag's first half.
fn clicked_halves(first: usize, width: usize, height: f32) -> Judged {
    let clean = render(&tape());
    let edges = edges_in(&clean, bits_at(&tape(), 1), clean.len());
    let mut x = clean.clone();
    let (mut put, mut i) = (0, first);
    while put < if first == 0 { 1 } else { 8 } {
        if edges[i + 1] - edges[i] >= 20 {
            x = spike(&x, (edges[i] + edges[i + 1]) / 2 - width / 2, width, height);
            put += 1;
            i += 250;
        }
        i += 1;
    }
    judge(&x, &tape(), same(width))
}

/// A click inside a half-wave splits it in three, its middle pulse as wide as the click, and
/// past two samples no fold takes it: the ROM, back from its wait after the click's first edge,
/// finds the level changed again and takes that for an edge, and loads the block at no phase.
/// At the flag's first half as anywhere in the block: whole to two samples of half the level,
/// lost at three. A click is one or two samples of a crackle or a bad splice.
#[test]
fn a_click_inside_a_half_wave_reads_whole_to_two_samples() {
    for first in [0, 2000] {
        for h in [0.2, 0.5] {
            assert_eq!(clicked_halves(first, 2, h), WHOLE, "from {first}, {h}");
        }
    }
    assert_eq!(
        clicked_halves(0, 3, 0.5),
        (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::Collapsed]), Vec::new())
    );
    assert_eq!(
        clicked_halves(2000, 3, 0.5),
        (Outcome::Lost(vec![1], vec![Said::BadChecksum, Said::RawSignal]), Vec::new())
    );
}

/// A click behind an edge, as the click wall puts them in the bits, at each seam of the data
/// block: its first half-waves out of the header's pause, the first pairs of its pilot and the
/// last, the flag's first edges, its last bits and its pause; and the header's last bits. The
/// ROM's wait after an edge hides it, and the fold takes it into a half-wave beside it, or
/// the pause plays it as noise.
#[test]
fn a_click_behind_an_edge_reads_whole_at_every_seam() {
    let clean = render(&tape());
    let edges = edges_in(&clean, block_at(&tape(), 1), clean.len());
    let header = edges_in(&clean, header_bits_at(), block_at(&tape(), 1));
    let sync = bits_at(&tape(), 1) - ((667.0 + 735.0) * f64::from(RATE) / 3_500_000.0) as usize;
    let n = edges.len();
    let mut at: Vec<usize> = [0, 1, 2, 4, 40, 3000, 3215].iter().map(|&k| edges[k] + 1).collect();
    at.extend([18, 20, 40, 60, 100].map(|d| sync + d));
    at.extend((n - 5..n).map(|k| edges[k] + 1));
    at.extend([10, 60, 2000].map(|d| edges[n - 1] + d));
    at.extend((header.len() - 5..header.len()).map(|k| header[k] + 1));
    for x in at {
        for w in [1, 3] {
            assert_eq!(judge(&spike(&clean, x, w, 0.5), &tape(), (1.0, 1.0)), WHOLE, "{w} samples at {x}");
        }
    }
}

/// The pulses of `blocks` with pulse `i` split by a bounce of two `width` T-state pulses, at
/// its start or with `end` at its end, decoded, and what the ROM loads that framing does not.
fn bounced_at(blocks: &[Block], i: usize, width: u64, end: bool) -> (Judged, Vec<Finding>) {
    let mut lens: Vec<u64> = of_blocks(blocks).unwrap().iter().map(|q| q.tstates.round() as u64).collect();
    let h = lens[i];
    lens.splice(i..=i, if end { [h - 2 * width, width, width] } else { [width, width, h - 2 * width] });
    let p = common::pulses(&lens);
    let got = decode_pulses(&p, 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    ((outcome(&got, blocks, (0.0, f64::MAX)), rom_only(&p, 3_500_000, blocks)), got.findings)
}

/// The pulse indices of the first sync pulse of each block of `blocks`.
fn syncs_of(blocks: &[Block]) -> Vec<usize> {
    let lens: Vec<u64> = of_blocks(blocks).unwrap().iter().map(|q| q.tstates.round() as u64).collect();
    (1..lens.len()).filter(|&i| lens[i] == 667 && lens[i - 1] == 2168).collect()
}

/// A bounce of a 1-bit capture in a pilot, near its start, breaks the leader loop, which starts
/// again behind it on more than the ROM's 256 pairs; near its end, inside the pulses the sync
/// loop takes one at a time, the ROM's wait after an edge folds it away (`framing::rom_pulse`).
/// At the first edge out of a pause, or in the pause, it plays as noise. Behind a headerless
/// block saved with no pause, a bounce in the next pilot's first half-waves leaves every byte
/// read, the pilot read from in front of it (`framing::tone_start`).
#[test]
fn a_bounce_in_a_pilot_or_a_gap_reads_whole() {
    for blocks in [tape(), headerless()] {
        for &s in &syncs_of(&blocks) {
            for back in [21, 101, 301, 501, 3201] {
                for (w, end) in [(79, false), (79, true), (200, false)] {
                    let ((j, rom), _) = bounced_at(&blocks, s - back, w, end);
                    assert_eq!((j, rom), WHOLE, "{back} back, {w} T, at the end {end}");
                }
            }
        }
    }
    let lens: Vec<u64> = of_blocks(&tape()).unwrap().iter().map(|q| q.tstates.round() as u64).collect();
    let level = lens.iter().position(|&t| t == 3500).unwrap();
    for (i, end) in [(level + 2, false), (level, false), (level, true), (level - 1, true)] {
        for w in [79, 200] {
            assert_eq!(bounced_at(&tape(), i, w, end).0, WHOLE, "pulse {i}, {w} T");
        }
    }
    let t = three_blocks(0);
    let lens: Vec<u64> = of_blocks(&t).unwrap().iter().map(|q| q.tstates.round() as u64).collect();
    let pilot = (syncs_of(&t)[0]..lens.len()).find(|&i| lens[i] == 2168).unwrap();
    for (i, end) in [(pilot, false), (pilot, true), (pilot + 1, false)] {
        let (j, f) = bounced_at(&t, i, 79, end);
        assert_eq!(j, WHOLE, "{f:?}");
    }
}

/// A click that splits one of the first half-waves of the pilot behind a headerless block saved
/// with no pause: the bit loop looks for the next pilot at a byte's first two half-waves, finds
/// none, and reads a byte of it, whose checksum then fails; the ROM, told the block's length,
/// loads it. Looking for the pilot at every half-wave of the next byte reads it, and costs the
/// flutter wall, since a run of 1 bits slowed 26% starts a pilot run inside a byte; it is left.
#[test]
fn a_click_at_the_next_pilot_behind_a_block_with_no_pause_loses_that_block() {
    let t = three_blocks(0);
    let mut lens: Vec<u64> = of_blocks(&t).unwrap().iter().map(|q| q.tstates.round() as u64).collect();
    let pilot = (syncs_of(&t)[0]..lens.len()).find(|&i| lens[i] == 2168).unwrap();
    let h = lens[pilot];
    lens.splice(pilot..=pilot, [(h - 600) / 2, 600, h - 600 - (h - 600) / 2]);
    let p = common::pulses(&lens);
    let got = decode_pulses(&p, 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        (outcome(&got, &t, (0.0, f64::MAX)), rom_only(&p, 3_500_000, &t)),
        (Outcome::Lost(vec![0], vec![Said::BadChecksum]), vec![0])
    );
}

/// The noise line each axis holds its pulses to is measured on the signal that axis cuts
/// (`edges::mark_quiet_on`). Under a wander over a dull tape the zero line holds the wander
/// itself, so half the tape's pulses on it and more are quiet; the midline follows the wander,
/// and its own line marks next to none of the tape's pulses quiet, with or without ten seconds
/// of dead air. Applying the zero line's own measure to the midline's pulses instead marks 57%
/// of them quiet at a wander of 0.4 and 81% at 0.9.
#[test]
fn a_wander_over_a_dull_tape_leaves_the_midlines_pulses_loud() {
    let lvl = rms(&render(&tape()));
    for dead in [0.0, 10_000.0] {
        for w in [0.4f32, 0.9] {
            let mut x = vec![0.0f32; ms(dead)];
            x.extend(render(&tape()));
            x.extend(vec![0.0f32; ms(dead)]);
            let x = hum(&spacing_loss(&hiss_under(&x, lvl, 40, 7), RATE, 5.0), RATE, 120.0, w);
            let quiet = |axis| {
                let p =
                    detect_pulses_band(|| x.iter().copied(), RATE, Filter::None, axis, Band::Max).unwrap();
                let tape: Vec<_> = p.iter().filter(|q| (400.0..=2600.0).contains(&q.tstates)).collect();
                tape.iter().filter(|q| q.quiet).count() as f64 / tape.len() as f64
            };
            assert!(quiet(Axis::Zero) > 0.5, "{dead} ms, wander {w}");
            assert!(quiet(Axis::Midline) < 0.001, "{dead} ms, wander {w}");
        }
    }
}

/// A wander over a dull tape leaves the signal as it is no block at all, raw signal throughout;
/// the band-pass reads the header of three blocks and keeps the other two as raw signal, said
/// lost with a pause behind them or none.
#[test]
fn a_wander_that_leaves_raw_signal_says_what_the_band_pass_did_not_read() {
    let wandering = |w: f32| move |x: &[f32]| hum(&spacing_loss(x, RATE, 5.0), RATE, 120.0, w);
    for pause in [0, 1000] {
        assert_eq!(on_three(pause, wandering(0.9)), WHOLE, "{pause}");
        assert_eq!(
            on_three(pause, wandering(1.1)),
            (Outcome::Lost(vec![0, 2], vec![Said::RawSignal]), Vec::new())
        );
    }
}

// Re-reads of a block its header asked for.

/// What `blocks` read as, as pulses and as a recording at [`RATE`], and through 2 µm of
/// spacing: whether each reading holds every one of `want`, and what the ROM's loader loads at
/// every phase from the pulses that framing does not read.
fn every_reading(blocks: &[Block], want: &[Vec<u8>]) -> ([bool; 3], Vec<usize>) {
    let holds = |got: &Decoded| want.iter().all(|w| got.blocks.iter().filter_map(standard).any(|d| d == w));
    let p = of_blocks(blocks).unwrap();
    let pulses = decode_pulses(&p, 3_500_000, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let order = playback_order(blocks, FlowOptions::default());
    let x = render_tape(blocks, RenderOptions { sample_rate: RATE, mic: false, amplitude: 0.5 }, &order);
    let plain = decode(&encode_wav(&x, RATE, 16), Channels::Only(Channel::Mix));
    let dull = decode(&encode_wav(&spacing_loss(&x, RATE, 2.0), RATE, 16), Channels::Only(Channel::Mix));
    let stds: Vec<Block> =
        want.iter().map(|d| Block::new(Body::Standard { pause: 1000, data: d.clone() })).collect();
    ([holds(&pulses), holds(&plain), holds(&dull)], rom_only(&p, 3_500_000, &stds))
}

/// A CODE header asking for `len` bytes and its data block, its byte behind the flag 0x00,
/// behind a sync pair of `sync1` and `sync2` T: the program, and the two blocks' bytes.
fn stretched_sync(len: u16, sync1: u16, sync2: u16) -> (Vec<Block>, Vec<Vec<u8>>) {
    let mut h = vec![0x00, 0x03];
    h.extend(b"swallowed ");
    h.extend(len.to_le_bytes());
    h.extend([0x00, 0x80, 0x00, 0x80]);
    let h = with_checksum(h);
    let mut leg = vec![0xFF, 0x00];
    leg.extend((1..len).map(|i| (i as u8).wrapping_mul(37).wrapping_add(11)));
    let leg = with_checksum(leg);
    let blocks = vec![
        Block::new(Body::Standard { pause: 1000, data: h.clone() }),
        Block::new(Body::Turbo {
            pilot: 2168,
            sync1,
            sync2,
            zero: 855,
            one: 1710,
            pilot_len: 3223,
            used_bits: 8,
            pause: 100,
            data: leg.clone(),
        }),
    ];
    (blocks, vec![h, leg])
}

/// A first sync pulse stretched into the pilot's window, as a 1-bit capture has them (1,617 /
/// 616 T): the sync loop takes it for pilot, the second sync pulse for the
/// first and the flag's first half for the second, and reads the block half a bit late, its
/// flag's 1 bits gone into what it took for pilot; its checksum fails, and the ROM loads it at
/// no phase. The two pulses sum to a pair under the split, and the bits read from behind them
/// answer the header (`merged::Seam::in_front`), as pulses, as a recording and a dull one. It is
/// read again at the header's length from before its flag.
#[test]
fn a_first_sync_pulse_stretched_into_the_pilot_reads_the_block_its_header_asked_for() {
    for len in [409, 16] {
        for (s1, s2) in [(1617, 616), (1500, 700), (1300, 700), (667, 735)] {
            let (blocks, want) = stretched_sync(len, s1, s2);
            assert_eq!(every_reading(&blocks, &want), ([true; 3], Vec::new()), "{len} bytes, ({s1}, {s2})");
        }
    }
}

/// A Program header behind a pulse standing where the pilot's last half and the sync pair were,
/// `seam` T, with its leg behind the gap and a byte of other material run on behind that with
/// no pause: no form's length decides it, and the sync loop takes it for
/// pilot and the header's first 0 bit for its sync. The program and its two blocks' bytes.
fn misframed(seam: Option<u16>) -> (Vec<Block>, Vec<Vec<u8>>) {
    const DECLARED: u16 = 0x90;
    let mut h = vec![0x00, 0x00];
    h.extend(b"ARCHIVE   ");
    h.extend(DECLARED.to_le_bytes());
    h.extend([0x0A, 0x00, DECLARED as u8, 0x00]);
    let header = with_checksum(h);
    let mut leg = vec![0xFF];
    leg.extend((0..DECLARED).map(|i| (i as u8).wrapping_mul(37).wrapping_add(11)));
    let leg = with_checksum(leg);
    let junk: Vec<u16> =
        (0..8).rev().flat_map(|k| if 0x5Au8 >> k & 1 == 1 { [1710, 1710] } else { [855, 855] }).collect();
    let blocks = vec![
        Block::new(Body::PureTone { pulse_len: 2168, count: 8062 }),
        Block::new(Body::PulseSeq { pulses: seam.map_or(vec![2168, 667, 735], |t| vec![t]) }),
        Block::new(Body::PureData { zero: 855, one: 1710, used_bits: 8, pause: 0, data: header.clone() }),
        Block::new(Body::PulseSeq { pulses: vec![855] }),
        Block::new(Body::Pause { pause: 1000 }),
        Block::new(Body::Standard { pause: 0, data: leg.clone() }),
        Block::new(Body::PulseSeq { pulses: junk }),
        Block::new(Body::Pause { pause: 1000 }),
    ];
    (blocks, vec![header, leg])
}

/// Read a bit late, the header comes out one bit shifted, its checksum holding as a shift often
/// leaves it, and asks for a length shifted a bit, 32 bytes, at which its leg is cut: the ROM,
/// asked for the true length, loads the leg. Read from the ROM's first sync pulse, the flag's
/// first 0 bit, the header is one in every field and stands, and its leg with it
/// (`merged::Seam::in_front`), wherever the pulse in front lies: from 1,900 T, under the pilot,
/// through 2,400 T, a pilot half under hiss, to 2,800 T, where the header is read shifted back by
/// its own bytes. At 3,000 T the backward form decides by its length.
#[test]
fn a_header_merged_into_its_pilot_is_read_from_the_roms_sync_and_its_block_with_it() {
    for seam in [1900, 2100, 2200, 2400, 2600, 2700, 2800, 3000] {
        let (blocks, want) = misframed(Some(seam));
        assert_eq!(every_reading(&blocks, &want), ([true; 3], Vec::new()), "{seam} T");
    }
    let (blocks, want) = misframed(None);
    assert_eq!(every_reading(&blocks, &want), ([true; 3], Vec::new()));
}

/// The misframed header on one track of a stereo recording and the tape as saved on the other:
/// whichever is kept, the reading that holds the header in every field brings in its leg at the
/// length that header asks, where the leg is re-read: each reading
/// reads a block at the length its own header asks.
#[test]
fn a_header_one_track_reads_a_bit_late_comes_with_its_block_from_the_other() {
    for seam in [2400, 2700] {
        let (bad, want) = misframed(Some(seam));
        let (good, _) = misframed(None);
        let render = |b: &[Block]| {
            let order = playback_order(b, FlowOptions::default());
            render_tape(b, RenderOptions { sample_rate: RATE, mic: false, amplitude: 0.5 }, &order)
        };
        let (l, r) = (render(&bad), render(&good));
        let n = l.len().min(r.len());
        for (left, right) in [(&l[..n], &r[..n]), (&r[..n], &l[..n])] {
            let got = decode(&stereo_wav(left, right, RATE, 16), Channels::Best);
            assert!(
                want.iter().all(|w| got.blocks.iter().filter_map(standard).any(|d| d == w)),
                "{seam}: {:?}",
                got.findings
            );
        }
    }
}
