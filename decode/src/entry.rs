// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! What the apps call: describe a recording file, keep it whole as raw signal, or decode it.
//!
//! Kept whole, sampled audio (WAV, VOC, IFF) goes into Direct Recording blocks, a CSW file into a
//! CSW block as it is, and an OUT trace into a CSW block at 3.5 MHz. Decoded, standard blocks are
//! read as the ROM reads them, and what lies between them is kept as raw signal where it is shaped
//! like a block and plays as their pause where it is not.

use crate::capture::{
    csw_block, csw_of_tstates, direct_recording, spacing, tzx_size, MAX_BLOCK_BYTES, MAX_CSW_RATE,
};
use crate::fallback::{
    block_shaped, burst_behind, pilot_in_pieces, raw_signal, rom_pilot, signal, sort, Part,
};
use crate::fold::{fold, vote, Source};
use crate::framing::{next_block, PILOT_PULSE_MOST};
use crate::proof::{self, Proof};
use crate::pulses::{from_runs, from_tstates, Pulse};
use crate::readers::{read, Channel, Csw, ReadError, Recording, Sound};
use crate::waveform::edges;
use crate::waveform::filter::{Filter, FilterError};
use std::fmt;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use tapeti_core::audio::TSTATES_PER_SEC;
use tapeti_core::types::{Block, Body};

/// Why a recording could not be kept.
#[derive(Clone, Debug, PartialEq)]
pub enum ImportError {
    Read(ReadError),
    Filter(FilterError),
    /// Read, but not something that can be kept: what, in words.
    Unsupported(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Read(e) => e.fmt(f),
            ImportError::Filter(e) => write!(f, "the filter: {e}"),
            ImportError::Unsupported(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<ReadError> for ImportError {
    fn from(e: ReadError) -> Self {
        ImportError::Read(e)
    }
}

/// Inflates a Z-RLE CSW file's data. The apps have zlib; this crate does not.
pub type Inflate<'a> = &'a dyn Fn(&[u8]) -> Result<Vec<u8>, String>;

/// The block type a recording is kept in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// One bit per sample (TZX 0x15): sampled audio.
    DirectRecording,
    /// Pulse lengths (TZX 0x18): a CSW file or an OUT trace.
    Csw,
}

/// What a recording file holds, and what keeping it makes.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// The format's name: WAV, VOC, IFF/8SVX, CSW or OUT.
    pub format: &'static str,
    /// Samples a second; an OUT trace counts T-states, 3,500,000 a second.
    pub rate_hz: u32,
    pub channels: u16,
    pub samples: usize,
    /// Bytes of audio the header announces that the file does not hold.
    pub missing_bytes: u64,
    /// Blocks the reader passed over, each in words.
    pub skipped: Vec<String>,
    pub kept: Kept,
    /// How many blocks keep it.
    pub blocks: usize,
    /// The size of the TZX file that keeps it.
    pub tzx_size: usize,
}

impl Summary {
    pub fn seconds(&self) -> f64 {
        self.samples as f64 / f64::from(self.rate_hz)
    }
}

/// The filter the apps offer for a recording at `rate_hz`.
///
/// A gentle second-order Butterworth band from 300 Hz to 4 kHz, kept below half the rate. It holds
/// the standard loader's tones (the 807 Hz pilot, bits at 1 and 2 kHz) with room for faster
/// loaders.
pub fn tape_band(rate_hz: u32) -> Filter {
    Filter::Butterworth { order: 2, low_hz: 300.0, high_hz: 4000f64.min(0.45 * f64::from(rate_hz)) }
}

/// A TZX file of one CSW block: the 10-byte file header, the block's ID and 14-byte header, its
/// data.
fn csw_tzx_size(data_len: usize) -> usize {
    10 + 15 + data_len
}

/// What reading `bytes` as a recording finds, or why it cannot be kept.
pub fn describe(name: &str, bytes: &[u8], inflate: Inflate) -> Result<Summary, ImportError> {
    let summary = match read(name, bytes, Channel::Mix)? {
        Recording::Sound(s) => {
            spacing_for(s.rate_hz)?;
            let samples = s.len();
            Summary {
                format: s.format,
                rate_hz: s.rate_hz,
                channels: s.channels,
                samples,
                missing_bytes: s.missing_bytes,
                skipped: s.skipped.clone(),
                kept: Kept::DirectRecording,
                blocks: samples.div_ceil(8).div_ceil(MAX_BLOCK_BYTES).max(1),
                tzx_size: tzx_size(samples),
            }
        }
        Recording::Csw(c) => {
            csw_rate(&c)?;
            let runs = csw_runs(&c, inflate)?;
            Summary {
                format: "CSW",
                rate_hz: c.rate_hz,
                channels: 1,
                samples: runs.iter().map(|&n| n as usize).sum(),
                missing_bytes: 0,
                skipped: Vec::new(),
                kept: Kept::Csw,
                blocks: 1,
                tzx_size: csw_tzx_size(c.data.len()),
            }
        }
        Recording::Out(lengths) => {
            let data_len = match csw_of_tstates(&lengths).body {
                Body::Csw { data, .. } => data.len(),
                _ => unreachable!("csw_of_tstates makes a CSW block"),
            };
            Summary {
                format: "OUT",
                rate_hz: TSTATES_PER_SEC,
                channels: 1,
                samples: lengths.iter().map(|&t| t as usize).sum(),
                missing_bytes: 0,
                skipped: Vec::new(),
                kept: Kept::Csw,
                blocks: 1,
                tzx_size: csw_tzx_size(data_len),
            }
        }
    };
    if summary.samples == 0 {
        return Err(no_sound());
    }
    Ok(summary)
}

/// The recording as the blocks [`describe`] said.
///
/// Sampled audio reads `channel` and goes through `filter`; a sample is high when it is above the
/// average of them all, which allows for a recording that does not sit on zero. `channel` and
/// `filter` do not apply to a CSW file or an OUT trace, whose pulses are kept exactly.
pub fn keep_as_raw_signal(
    name: &str,
    bytes: &[u8],
    channel: Channel,
    filter: Filter,
    inflate: Inflate,
) -> Result<Vec<Block>, ImportError> {
    let blocks = match read(name, bytes, channel)? {
        Recording::Sound(sound) => {
            let spacing = spacing_for(sound.rate_hz)?;
            direct_recording(spacing, levels(&sound, filter)?, MAX_BLOCK_BYTES)
        }
        Recording::Csw(c) => {
            csw_rate(&c)?;
            // Run lengths as they are are checked against the count here; Z-RLE data was checked
            // when it was described, and is kept without inflating it.
            if c.compression == 1 {
                csw_runs(&c, inflate)?;
            }
            vec![csw_block(c.rate_hz, c.compression, c.pulse_count, c.data)]
        }
        Recording::Out(lengths) => vec![csw_of_tstates(&lengths)],
    };
    if blocks.is_empty() {
        return Err(no_sound());
    }
    Ok(blocks)
}

/// Each sample of `sound` through `filter`, true when above the average of them all.
fn levels<'a>(sound: &'a Sound, filter: Filter) -> Result<impl Iterator<Item = bool> + 'a, ImportError> {
    let design = || filter.design(sound.rate_hz).map_err(ImportError::Filter);
    let mean = mean(design()?.apply(sound.samples()));
    Ok(design()?.apply(sound.samples()).map(move |v| f64::from(v) > mean))
}

fn no_sound() -> ImportError {
    ImportError::Unsupported("the recording holds no sound".into())
}

fn spacing_for(rate_hz: u32) -> Result<u16, ImportError> {
    spacing(rate_hz).ok_or_else(|| {
        // Over twice the clock a sample is under half a T-state, which rounds to none.
        let side = if f64::from(rate_hz) > 2.0 * f64::from(TSTATES_PER_SEC) { "high" } else { "low" };
        ImportError::Unsupported(format!("a sample rate of {rate_hz} Hz is too {side}"))
    })
}

fn csw_rate(c: &Csw) -> Result<(), ImportError> {
    if c.rate_hz > MAX_CSW_RATE {
        let what = format!("a CSW rate of {} Hz is more than a TZX CSW block holds", c.rate_hz);
        return Err(ImportError::Unsupported(what));
    }
    Ok(())
}

fn csw_runs(c: &Csw, inflate: Inflate) -> Result<Vec<u32>, ImportError> {
    let inflated = match c.compression {
        2 => {
            Some(inflate(c.data).map_err(|e| ImportError::Unsupported(format!("CSW: the Z-RLE data: {e}")))?)
        }
        _ => None,
    };
    Ok(c.pulses(inflated.as_deref())?)
}

fn mean(samples: impl Iterator<Item = f32>) -> f64 {
    let (sum, n) = samples.fold((0.0f64, 0usize), |(s, n), v| (s + f64::from(v), n + 1));
    if n == 0 {
        0.0
    } else {
        sum / n as f64
    }
}

/// Where a decoded block came from, in the source's samples: the recording's, or T-states for a
/// source that has none. The pause is the block's too.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub start: f64,
    pub end: f64,
    /// Where the block's own signal ends and its pause begins: `end` for a block with no pause,
    /// `start` for a pause block.
    pub signal_end: f64,
}

/// What of a block that did not read lies in a gap ([`Finding::Lost`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loss {
    /// A pilot tone the ROM locks on, and signal behind it (`fallback::burst_behind`).
    Pilot,
    /// The rest of the block in front, whose bits stopped short of its end: shorter than its
    /// header asked for, inside a byte, or with no header and its checksum wrong, and signal
    /// behind it.
    Rest,
}

/// What a recording that reads no block holds of a pilot tone ([`Finding::NoBlock`]), from
/// nothing up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PilotHeard {
    /// No pilot tone.
    None,
    /// A pilot tone broken into pieces the ROM never locks on (`fallback::pilot_in_pieces`).
    Pieces,
    /// A pilot tone the ROM locks on (`fallback::rom_pilot`).
    Whole,
}

/// Something a decode found, about the block at the index it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// The bytes do not add up to their checksum.
    BadChecksum { block: usize },
    /// The header in front asked for more bytes than the signal holds.
    Short { block: usize, expected: usize, read: usize },
    /// A stretch no reader took, kept as raw signal.
    RawSignal { block: usize },
    /// Pulses in a gap, dead air or not shaped like a block, played as the pause of the block
    /// that holds them; `pilot` where a pilot the ROM locks on is among them, with nothing behind
    /// it to say a block was lost (`fallback::rom_pilot`).
    Collapsed { block: usize, pulses: usize, pilot: bool },
    /// Pulses in a gap that hold a block that did not read, `what` of it, played as the pause of
    /// the block that holds them, or before the first block left out with the lead-in (`None`).
    Lost { block: Option<usize>, pulses: usize, what: Loss },
    /// What lies before the first block, `ms` long, left out: a tape starts with its first
    /// block. `pulses` it holds that are not a plain pause, none where it is silence.
    LeadIn { ms: u64, pulses: usize },
    /// No block read in the whole recording, `ms` long, said in place of [`Finding::LeadIn`];
    /// `pulses` it holds that are not a plain pause, none where it is silence, and `pilot` what
    /// a reading of it holds of a pilot tone: any is a tape that did not read.
    NoBlock { ms: u64, pulses: usize, pilot: PilotHeard },
    /// Of a stereo recording's mix and its two channels, the decode of `kept` delivers most.
    Channel { kept: Channel },
    /// The block's last byte holds only `used_bits` bits read from the tape, from its top bit;
    /// the rest are 0s, since a standard block cannot say how many bits its last byte holds.
    /// `completes` where the 0s make the checksum hold, which then proves nothing
    /// ([`proof::filled_out`]).
    PartialFinalByte { block: usize, used_bits: u8, completes: bool },
    /// A 0x00 byte read behind the header, a tape copier's, was left out
    /// ([`crate::framing::RawBlock::stray_zero`]).
    StrayZero { block: usize },
    /// Of the decodes on the zero line and on the signal's own midline, the midline's delivers
    /// most ([`edges::Axis`]).
    Midline,
    /// The block was read from `channel`, which proves it more surely than the channel kept
    /// ([`crate::fold::fold`]).
    FromChannel { block: usize, channel: Channel },
    /// The block was read on `axis`, which proves it more surely than the line kept
    /// ([`crate::fold::fold`]).
    FromAxis { block: usize, axis: edges::Axis },
    /// The block was read as two of the mix and the two channels both read it, where the one
    /// kept read other bytes whose checksum held too ([`crate::fold::fold`]).
    BothChannels { block: usize },
    /// The sync pair had merged into a half-wave beside it, or lay past the ROM's window, and the
    /// block was read where its bits are ([`crate::framing::DECIDE`]).
    MergedSync { block: usize },
    /// The block was read again on a band that follows the signal's level down through a
    /// dropout, where the first read stopped ([`edges::Band::Fade`]).
    FadeBand { block: usize },
    /// The pilot tone in front held fewer pairs than the ROM locks on, and the block stands on
    /// its own proof ([`crate::framing::SHORT_LEADER_PAIRS`]).
    ShortLeader { block: usize },
    /// The raw signal is the run-on of the block in front: the bits it reads past the length it
    /// was asked for ([`crate::framing::RawBlock::bits_end`]).
    RunOn { block: usize },
    /// None of the readings read the block whole, and it was read byte by byte as most of them
    /// read each byte, of `readings` that hold it ([`crate::fold::vote`]).
    Voted { block: usize, readings: usize },
    /// `bits` of the block were read from two half-waves that differ as no saver writes a bit's:
    /// where the signal lost a lobe and three half-waves merged into one, or where one kind of
    /// edge runs late; read by their sum, as the ROM reads them
    /// ([`crate::framing::UNLIKE_HALVES`]). A reading of the block through fewer stands above it
    /// ([`crate::fold::fold`]).
    UnlikeHalves { block: usize, bits: usize },
    /// The tape slowed inside the block past the speed its pilot measured, as a snag in the
    /// transport holds it back, and the block was read again at the speed of the bits before
    /// each pair, where its checksum holds ([`crate::framing::SPEED_BITS`]).
    Snag { block: usize },
    /// The block was read through the band-pass filter the apps offer, which proves it more
    /// surely than the signal as it is ([`tape_band`]).
    BandPass { block: usize },
    /// The signal as it is reads no block, and the decode through the band-pass filter, which
    /// does, is kept whole ([`tape_band`]).
    BandPassOnly,
}

impl Finding {
    /// The block it is about; `None` for the recording as a whole.
    pub fn block(&self) -> Option<usize> {
        self.clone().block_mut().copied()
    }

    /// The block it is about, to move it along when blocks go in or out before it.
    pub(crate) fn block_mut(&mut self) -> Option<&mut usize> {
        match self {
            Finding::BadChecksum { block }
            | Finding::Short { block, .. }
            | Finding::RawSignal { block }
            | Finding::Collapsed { block, .. }
            | Finding::StrayZero { block }
            | Finding::PartialFinalByte { block, .. }
            | Finding::MergedSync { block }
            | Finding::FromChannel { block, .. }
            | Finding::FromAxis { block, .. }
            | Finding::BothChannels { block }
            | Finding::FadeBand { block }
            | Finding::ShortLeader { block }
            | Finding::RunOn { block }
            | Finding::Voted { block, .. }
            | Finding::UnlikeHalves { block, .. }
            | Finding::Snag { block }
            | Finding::BandPass { block }
            | Finding::Lost { block: Some(block), .. } => Some(block),
            Finding::Lost { block: None, .. }
            | Finding::LeadIn { .. }
            | Finding::NoBlock { .. }
            | Finding::Channel { .. }
            | Finding::Midline
            | Finding::BandPassOnly => None,
        }
    }

    /// Whether it says something of the tape is missing or wrong, rather than how it was read.
    pub fn is_problem(&self) -> bool {
        match self {
            Finding::BadChecksum { .. }
            | Finding::Short { .. }
            | Finding::PartialFinalByte { .. }
            | Finding::Lost { .. } => true,
            Finding::NoBlock { pilot, .. } => *pilot != PilotHeard::None,
            Finding::RawSignal { .. }
            | Finding::Collapsed { .. }
            | Finding::LeadIn { .. }
            | Finding::Channel { .. }
            | Finding::Midline
            | Finding::FromChannel { .. }
            | Finding::FromAxis { .. }
            | Finding::BothChannels { .. }
            | Finding::FadeBand { .. }
            | Finding::ShortLeader { .. }
            | Finding::RunOn { .. }
            | Finding::StrayZero { .. }
            | Finding::MergedSync { .. }
            | Finding::Voted { .. }
            | Finding::UnlikeHalves { .. }
            | Finding::Snag { .. }
            | Finding::BandPass { .. }
            | Finding::BandPassOnly => false,
        }
    }

    /// What it says, in words, without the block it is about.
    pub fn text(&self) -> String {
        match *self {
            Finding::BadChecksum { .. } => "The checksum does not match the bytes.".into(),
            Finding::Short { expected, read, .. } => {
                format!("The header asks for {expected} bytes; the signal holds {read}.")
            }
            Finding::RawSignal { .. } => "Kept as raw signal.".into(),
            Finding::Collapsed { pulses, pilot: false, .. } => {
                format!("The pause held {pulses} pulses of noise; they play as silence.")
            }
            Finding::Collapsed { pulses, pilot: true, .. } => format!(
                "The pause held {pulses} pulses that read as no block, a pilot tone a Spectrum locks on among them; they play as silence."
            ),
            // No block stands in front of the lead-in to have stopped short.
            Finding::Lost { block: None, pulses, .. } => format!(
                "The lead-in holds a block that did not read: a pilot tone a Spectrum locks on, and signal behind it. Its {pulses} pulses are left out with it."
            ),
            Finding::Lost { block: Some(_), pulses, what: Loss::Pilot } => format!(
                "The pause holds a block that did not read: a pilot tone a Spectrum locks on, and signal behind it. Its {pulses} pulses play as silence."
            ),
            Finding::Lost { block: Some(_), pulses, what: Loss::Rest } => format!(
                "The block stops short of its end: the {pulses} pulses behind where it stops play as its pause."
            ),
            Finding::LeadIn { ms, pulses: 0 } => {
                format!("lead-in: {:.1} s of silence before the first block, left out", ms as f64 / 1000.0)
            }
            Finding::LeadIn { ms, pulses } => format!(
                "lead-in: {:.1} s before the first block, left out, with {pulses} pulses that read as no block",
                ms as f64 / 1000.0
            ),
            Finding::NoBlock { ms, pulses: 0, .. } => {
                format!("No block: the recording is {:.1} s of silence.", ms as f64 / 1000.0)
            }
            Finding::NoBlock { ms, pulses, pilot } => format!(
                "No block reads in the recording: {pulses} pulses over {:.1} s, {}",
                ms as f64 / 1000.0,
                match pilot {
                    PilotHeard::Whole => "a pilot tone a Spectrum locks on among them, of a tape that did not read.",
                    PilotHeard::Pieces => {
                        "a pilot tone among them in pieces too short for a Spectrum to lock on, of a tape that did not read."
                    }
                    PilotHeard::None => "and no pilot tone among them.",
                }
            ),
            Finding::Channel { kept: Channel::Mix } => {
                "Read from both channels mixed: neither alone reads more.".into()
            }
            Finding::Channel { kept } => format!(
                "Read from the {} channel: of the mix and each channel alone, it reads the most.",
                if kept == Channel::Left { "left" } else { "right" }
            ),
            Finding::FromChannel { channel, .. } => format!(
                "Read from the {} channel, which proves it more surely than the channel kept.",
                match channel {
                    Channel::Left => "left",
                    Channel::Right => "right",
                    Channel::Mix => "mixed",
                }
            ),
            Finding::FromAxis { axis, .. } => match axis {
                edges::Axis::Zero => {
                    "Read on the zero line, which proves it more surely than the signal's own centre line.".into()
                }
                edges::Axis::Midline => {
                    "Read about the signal's own centre line, which proves it more surely than the zero line.".into()
                }
            },
            Finding::BothChannels { .. } => {
                "Read as two of the mix and the two channels both read it: the one kept read other bytes, whose checksum held by chance.".into()
            }
            Finding::Midline => {
                "Read about the signal's own centre line, which wanders off zero: it reads more.".into()
            }
            Finding::PartialFinalByte { used_bits, completes, .. } => format!(
                "The last byte holds only {used_bits} bit{} read from the tape; the rest are 0{}",
                if used_bits == 1 { "" } else { "s" },
                if completes { ", and they make the checksum hold: it proves nothing here." } else { "." }
            ),
            Finding::StrayZero { .. } => {
                "A stray 0x00 byte behind the header, which a tape copier leaves, is left out.".into()
            }
            Finding::FadeBand { .. } => {
                "The signal fades out for a moment; the block was read again following its level down.".into()
            }
            Finding::MergedSync { .. } => {
                "The sync pulses are merged into the signal beside them or distorted; the bits are read where the block is.".into()
            }
            Finding::RunOn { .. } => {
                "The block in front reads on past the length it was asked for; the bits behind it are kept as raw signal.".into()
            }
            Finding::Voted { readings, .. } => format!(
                "None of the {readings} readings of the channels and centre lines reads it whole; each byte is the one most of them read where they are in step, and the checksum holds."
            ),
            Finding::UnlikeHalves { bits, .. } => format!(
                "{bits} of its bits {} read from two half-waves that differ as no saver writes them: the signal lost a lobe there, or one kind of edge runs late. Read as a Spectrum reads them.",
                if bits == 1 { "is" } else { "are" }
            ),
            Finding::BandPass { .. } => {
                "Read through a band-pass filter, which proves it more surely than the signal as it is.".into()
            }
            Finding::BandPassOnly => "Read through a band-pass filter: the signal as it is reads no block.".into(),
            Finding::Snag { .. } => {
                "The tape slows inside the block, as a snag in the transport holds it back; its bits were read at the speed of the bits before each one, and the checksum holds.".into()
            }
            Finding::ShortLeader { .. } => {
                "The pilot tone in front is too short for a Spectrum to load the block, cut or broken by a dropout; it was read from what is left, a header its fields prove or the block a header asked for.".into()
            }
        }
    }
}

/// A decoded recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub blocks: Vec<Block>,
    /// One per block.
    pub extents: Vec<Extent>,
    pub findings: Vec<Finding>,
    /// The source's samples a second: 3,500,000 for T-states.
    pub rate_hz: u32,
    /// How many pulses before the first block read, and after the last, were not a plain pause
    /// and went into one or the lead-in all the same ([`Finding::Collapsed`]). A decode of part
    /// of a recording that begins or ends inside a neighbour cuts that neighbour there.
    pub collapsed_ends: [usize; 2],
}

impl Decoded {
    /// Whether the decode filled block `i`'s last byte out with 0s past the bits it read
    /// ([`Finding::PartialFinalByte`]).
    pub fn filled_out(&self, i: usize) -> bool {
        self.findings.iter().any(|f| matches!(f, Finding::PartialFinalByte { block, .. } if *block == i))
    }

    /// What block `i` proves, given `proof` from its bytes: no more than [`proof::filled_out`]
    /// allows where its last byte was filled out.
    pub fn proof_of(&self, i: usize, proof: Proof) -> Proof {
        if self.filled_out(i) {
            proof::filled_out(proof)
        } else {
            proof
        }
    }
}

/// How far a decode has come.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub pulses_read: usize,
    pub pulses: usize,
    /// Blocks read so far: the most any of the decodes has read yet, of the channels of a stereo
    /// recording read for [`Channels::Best`], and of the zero line and the midline.
    pub blocks: usize,
    /// The share of the whole decode done, from 0 to 1. Finding the edges in sampled audio takes
    /// most of the time, before any block is read, and is counted in it.
    pub done: f32,
}

/// The share of a decode of sampled audio taken by finding its edges: they are found in about
/// 97 % of the time (a five-minute WAV at 44.1 kHz in 0.39 of 0.40 s).
const EDGES_SHARE: f32 = 0.95;

/// The samples a pass reads between two reports of how far the edge detector has come.
const REPORT_EVERY: usize = 1 << 16;

/// Why a decode ended without a result.
#[derive(Clone, Debug, PartialEq)]
pub enum DecodeError {
    Import(ImportError),
    /// The cancel flag was set.
    Cancelled,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Import(e) => e.fmt(f),
            DecodeError::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<ImportError> for DecodeError {
    fn from(e: ImportError) -> Self {
        DecodeError::Import(e)
    }
}

/// Which channel of a stereo recording a decode reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Channels {
    /// The mix, the left and the right channel each in turn, keeping the decode that delivers
    /// most and folding the others into it ([`crate::fold`]).
    #[default]
    Best,
    Only(Channel),
}

/// Decodes a recording file.
///
/// CSW and OUT go by their pulses, sampled audio by the edges the edge detector finds in it after
/// `filter`. Where no filter was asked for and the decode of sampled audio still loses something
/// (no block at all, a problem, raw signal, or a header whose block it has the time for and does
/// not hold), it is decoded again through the band the apps offer ([`tape_band`]) and folded in
/// ([`Finding::BandPass`]): the band takes out the hum, hiss and wander outside the tape's tones
/// but rings, reading some defects further and others less far than the unfiltered signal, so a
/// block enters from it only where it proves more than what stands there; only where the
/// unfiltered decode reads no block at all is the band's kept whole instead
/// ([`Finding::BandPassOnly`]). A recording that reads whole, or a stereo one whose channels make
/// up for each other, pays nothing for the second pass. `window` keeps only the pulses that start
/// inside it, in the source's samples as [`Pulse::start`] counts them; positions stay those of the
/// whole file. A stereo recording
/// read for [`Channels::Best`] is decoded three times, each a third of [`Progress::done`].
#[allow(clippy::too_many_arguments)]
pub fn decode_file(
    name: &str,
    bytes: &[u8],
    channels: Channels,
    filter: Filter,
    window: Option<std::ops::Range<f64>>,
    inflate: Inflate,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Decoded, DecodeError> {
    let (sound, stereo) = match read(name, bytes, Channel::Mix) {
        Ok(Recording::Sound(s)) => (Some(s.rate_hz), s.channels == 2),
        _ => (None, false),
    };
    let Some(band) = sound.filter(|_| filter == Filter::None).map(tape_band) else {
        return decode_through(name, bytes, channels, filter, window, inflate, stereo, progress, cancel);
    };
    let mut most = 0;
    let mut first = |p: Progress| {
        most = most.max(p.blocks);
        progress(Progress { done: PLAIN_SHARE * p.done, ..p });
    };
    let mut kept =
        decode_through(name, bytes, channels, filter, window.clone(), inflate, stereo, &mut first, cancel)?;
    let loses = kept.blocks.is_empty()
        || kept.findings.iter().any(|f| f.is_problem() || matches!(f, Finding::RawSignal { .. }))
        || crate::fold::owes_a_block(&kept);
    if loses {
        let mut second = |p: Progress| {
            let done = PLAIN_SHARE + (1.0 - PLAIN_SHARE) * p.done;
            progress(Progress { blocks: most.max(p.blocks), done, ..p });
        };
        // A band the rate cannot hold is no reading.
        match decode_through(name, bytes, channels, band, window, inflate, stereo, &mut second, cancel) {
            // A decode of no block read nothing, and has no tie to keep.
            Ok(mut filtered) if kept.blocks.is_empty() && !filtered.blocks.is_empty() => {
                filtered.findings.insert(0, Finding::BandPassOnly);
                kept = filtered;
            }
            Ok(filtered) => fold(&mut kept, &[(Source::Band, &filtered)]),
            Err(DecodeError::Cancelled) => return Err(DecodeError::Cancelled),
            Err(_) => {}
        }
    }
    progress(Progress { pulses_read: 0, pulses: 0, blocks: most, done: 1.0 });
    Ok(kept)
}

/// The share of a decode of sampled audio [`decode_file`] gives the signal as it is, where the
/// band-pass may follow: about as much work again, and done only where something is lost.
const PLAIN_SHARE: f32 = 0.5;

/// [`decode_file`] through `filter` alone; `stereo` whether the recording has two channels.
#[allow(clippy::too_many_arguments)]
fn decode_through(
    name: &str,
    bytes: &[u8],
    channels: Channels,
    filter: Filter,
    window: Option<std::ops::Range<f64>>,
    inflate: Inflate,
    stereo: bool,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Decoded, DecodeError> {
    let channel = match channels {
        Channels::Only(channel) => channel,
        Channels::Best if stereo => {
            return best_channel(
                |channel, progress| {
                    decode_channel(name, bytes, channel, filter, window.clone(), inflate, progress, cancel)
                },
                progress,
            );
        }
        Channels::Best => Channel::Mix,
    };
    decode_channel(name, bytes, channel, filter, window, inflate, progress, cancel).map(|(d, _)| d)
}

/// The decode `decode` makes of the mix, the left or the right channel that delivers most, the
/// other two folded into it ([`crate::fold::fold`]), and the blocks none of them reads whole
/// read again byte by byte from every reading of every channel ([`crate::fold::vote`]). A
/// channel that cannot be decoded delivers nothing; when none can, the mix's error stands.
fn best_channel(
    mut decode: impl FnMut(Channel, &mut dyn FnMut(Progress)) -> Result<Readings, DecodeError>,
    progress: &mut dyn FnMut(Progress),
) -> Result<Decoded, DecodeError> {
    const ORDER: [Channel; 3] = [Channel::Mix, Channel::Left, Channel::Right];
    let mut decodes = Vec::with_capacity(ORDER.len());
    let mut readings = Vec::new();
    let mut most = 0;
    for (i, channel) in ORDER.into_iter().enumerate() {
        let got = decode(channel, &mut |p| {
            most = most.max(p.blocks);
            let done = (i as f32 + p.done) / ORDER.len() as f32;
            progress(Progress { blocks: most, done, ..p });
        });
        if matches!(got, Err(DecodeError::Cancelled)) {
            return Err(DecodeError::Cancelled);
        }
        decodes.push(got.map(|(kept, read)| {
            readings.extend(read);
            kept
        }));
    }
    let (i, mut kept) = keep_and_fold(decodes, |k| Source::Channel(ORDER[k]))?;
    vote(&mut kept, &readings.iter().collect::<Vec<_>>());
    kept.findings.insert(0, Finding::Channel { kept: ORDER[i] });
    Ok(kept)
}

/// The decode of `decodes` that delivers most (`best_index`), the others folded into it
/// ([`fold`]), each from `source` of its index; and which it is.
fn keep_and_fold(
    decodes: Vec<Result<Decoded, DecodeError>>,
    source: impl Fn(usize) -> Source,
) -> Result<(usize, Decoded), DecodeError> {
    let (i, decodes) = best_index(decodes)?;
    let mut decodes: Vec<Option<Decoded>> = decodes.into_iter().map(Result::ok).collect();
    let mut kept = decodes[i].take().expect("the decode picked is one that read");
    let donors: Vec<(Source, &Decoded)> =
        decodes.iter().enumerate().filter_map(|(k, d)| d.as_ref().map(|d| (source(k), d))).collect();
    fold(&mut kept, &donors);
    Ok((i, kept))
}

/// Which of `decodes` delivers most ([`crate::fold::pick_reading`]), the earliest on a tie, with
/// them all. One that failed delivers nothing; when all failed, the first one's error stands.
#[allow(clippy::type_complexity)]
fn best_index(
    mut decodes: Vec<Result<Decoded, DecodeError>>,
) -> Result<(usize, Vec<Result<Decoded, DecodeError>>), DecodeError> {
    if decodes.iter().any(|d| matches!(d, Err(DecodeError::Cancelled))) {
        return Err(DecodeError::Cancelled);
    }
    if decodes.iter().all(Result::is_err) {
        return Err(decodes.swap_remove(0).expect_err("every decode failed"));
    }
    let readings: Vec<Option<&Decoded>> = decodes.iter().map(|d| d.as_ref().ok()).collect();
    let i = crate::fold::pick_reading(&readings);
    Ok((i, decodes))
}

/// What `decode_channel` and `decode_sound` give: the decode, and the readings it was kept
/// and folded from, one for each line it was read on (only one for pulses).
type Readings = (Decoded, Vec<Decoded>);

/// Decodes `channel` of a recording file, as [`decode_file`] says.
#[allow(clippy::too_many_arguments)]
fn decode_channel(
    name: &str,
    bytes: &[u8],
    channel: Channel,
    filter: Filter,
    window: Option<std::ops::Range<f64>>,
    inflate: Inflate,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Readings, DecodeError> {
    let in_window = |pulses: Vec<Pulse>| -> Result<Vec<Pulse>, DecodeError> {
        let pulses: Vec<Pulse> = match &window {
            Some(window) => pulses.into_iter().filter(|p| window.contains(&p.start)).collect(),
            None => pulses,
        };
        if pulses.is_empty() {
            return Err(no_sound().into());
        }
        Ok(pulses)
    };
    let (pulses, rate_hz) = match read(name, bytes, channel).map_err(ImportError::from)? {
        Recording::Sound(sound) => return decode_sound(&sound, filter, in_window, progress, cancel),
        Recording::Csw(c) => {
            csw_rate(&c)?;
            (from_runs(&csw_runs(&c, inflate)?, c.rate_hz), c.rate_hz)
        }
        Recording::Out(lengths) => (from_tstates(&lengths), TSTATES_PER_SEC),
    };
    let read = decode_pulses_from(&in_window(pulses)?, rate_hz, progress, cancel, (0.0, 1.0))?;
    Ok((read.clone(), vec![read]))
}

/// Decodes sampled audio, its edges found on the zero line and on the signal's own midline
/// ([`edges::Axis`]), keeping the decode that delivers most, the zero line's on a tie, and
/// folding the other into it, as the channels are ([`crate::fold`]). `in_window` keeps the
/// pulses asked for.
///
/// A worn tape's centre can wander further than its shrunken short half-waves swing, so the zero
/// line never crosses and two bits merge into one, where the midline's lobes are each their own
/// excursion; a wander that wide also leaves the zero line no pilot to lock on, so a decode of no
/// block at all is not taken as whole and the midline is tried too. Nothing says beforehand which
/// line reads best, so both are decoded and a block is kept from whichever proves it more surely;
/// the midline is skipped only once the zero line's decode holds nothing but whole blocks and
/// plain pauses, since neither the pick nor the fold could then take anything more from it. On
/// either line, a block a header asks for that does not read whole is read again on the fade band
/// (`read_faded`).
fn decode_sound(
    sound: &Sound,
    filter: Filter,
    in_window: impl Fn(Vec<Pulse>) -> Result<Vec<Pulse>, DecodeError>,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Readings, DecodeError> {
    use std::cell::{Cell, RefCell};
    const AXES: [edges::Axis; 2] = [edges::Axis::Zero, edges::Axis::Midline];
    // The samples run out when the flag is set, so a cancel ends the edge detector's passes too:
    // the statistics pass, the zero line's edges and quiet pulses, and the midline's edges, which
    // takes its quiet pulses from the zero line's pass.
    let read = Cell::new(0usize);
    let all = ((1 + 2 + 1) * sound.len()).max(1);
    let edges_done = || EDGES_SHARE * (read.get().min(all) as f32 / all as f32);
    // Reading the blocks of each line takes its share of what is left.
    let per_axis = (1.0 - EDGES_SHARE) / AXES.len() as f32;
    let (blocks_done, most) = (Cell::new(0.0f32), Cell::new(0usize));
    let report = RefCell::new(&mut *progress);
    let samples = || {
        sound.samples().take_while(|_| !cancel.load(Ordering::Relaxed)).inspect(|_| {
            read.set(read.get() + 1);
            if read.get().is_multiple_of(REPORT_EVERY) {
                let done = edges_done() + blocks_done.get();
                (report.borrow_mut())(Progress { pulses_read: 0, pulses: 0, blocks: most.get(), done });
            }
        })
    };
    let decodes = edges::detect_pulses_each(samples, sound.rate_hz, filter, &AXES, |axis, pulses| {
        if cancel.load(Ordering::Relaxed) {
            return ControlFlow::Break(Err(DecodeError::Cancelled));
        }
        let from = edges_done() + blocks_done.get();
        blocks_done.set(blocks_done.get() + per_axis);
        let mut each = |p: Progress| {
            most.set(most.get().max(p.blocks));
            (report.borrow_mut())(Progress { blocks: most.get(), ..p });
        };
        let got = in_window(pulses).and_then(|p| {
            let read = decode_pulses_from(&p, sound.rate_hz, &mut each, cancel, (from, from + per_axis))?;
            let faded = || {
                let band = edges::detect_pulses_band(samples, sound.rate_hz, filter, axis, edges::Band::Fade);
                in_window(band.map_err(ImportError::Filter)?)
            };
            read_faded(read, &p, faded, sound.rate_hz, cancel)
        });
        let whole = got.as_ref().is_ok_and(|d| {
            !d.blocks.is_empty()
                && !d.findings.iter().any(|f| {
                    matches!(
                        f,
                        Finding::BadChecksum { .. }
                            | Finding::Short { .. }
                            | Finding::PartialFinalByte { .. }
                            | Finding::RawSignal { .. }
                            | Finding::Collapsed { .. }
                            | Finding::Lost { .. }
                    )
                })
        });
        if whole {
            ControlFlow::Break(got)
        } else {
            ControlFlow::Continue(got)
        }
    })
    .map_err(ImportError::Filter)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(DecodeError::Cancelled);
    }
    let readings = decodes.iter().filter_map(|d| d.as_ref().ok().cloned()).collect();
    let (i, mut kept) = keep_and_fold(decodes, |k| Source::Axis(AXES[k]))?;
    if AXES[i] == edges::Axis::Midline {
        kept.findings.insert(0, Finding::Midline);
    }
    progress(Progress { pulses_read: 0, pulses: 0, blocks: most.get(), done: 1.0 });
    Ok((kept, readings))
}

/// `read`, the decode of the `ordinary` pulses, with each block owed bytes it does not hold
/// (`unanswered`) read again from the fade band's pulses ([`edges::Band::Fade`]), which `faded`
/// cuts when one is owed.
///
/// A dropout narrower than the ordinary band's window leaves no edge across it, so the block ends
/// there and the bytes are not in those pulses at all; read again from the fade band at the length
/// its header asks for, a block stands only where its checksum is good there, never as the
/// ordinary read filled out with 0x00 bytes (which also keeps the checksum) or with a byte a
/// lower-arming band reads past the block's end. Each answer's stretch, from its pilot to where
/// its bits stop, takes the fade band's pulses in place of the ordinary ones and the whole is
/// decoded again, so the pauses and blocks around it are read as ever ([`Finding::FadeBand`]);
/// what is still owed is asked again of that decode, and the passes end once none answers
/// something new, since each answer takes a stretch no other has taken.
fn read_faded(
    read: Decoded,
    ordinary: &[Pulse],
    faded: impl FnOnce() -> Result<Vec<Pulse>, DecodeError>,
    grid_hz: u32,
    cancel: &AtomicBool,
) -> Result<Decoded, DecodeError> {
    let to_t = f64::from(TSTATES_PER_SEC) / f64::from(grid_hz);
    let end_of = |p: &Pulse| p.start + p.tstates / to_t;
    let (mut faded, mut band) = (Some(faded), Vec::new());
    // Each answer: its bytes and the stretch it takes, in order.
    let mut answers: Vec<(Vec<u8>, f64, f64)> = Vec::new();
    // The owed already looked for, by where and how much.
    let mut asked: Vec<(u64, usize)> = Vec::new();
    let (mut kept, mut merged_now): (Decoded, Option<Vec<Pulse>>) = (read, None);
    loop {
        let heard = merged_now.as_deref().unwrap_or(ordinary);
        let owed: Vec<Owed> = unanswered(&kept, heard)
            .into_iter()
            .filter(|o| !asked.contains(&(o.from.to_bits(), o.want)))
            .collect();
        if owed.is_empty() {
            return Ok(kept);
        }
        if let Some(cut) = faded.take() {
            band = cut()?;
        }
        let start_at = |i: usize| band.get(i).map_or_else(|| band.last().map_or(0.0, end_of), |p| p.start);
        let mut found = Vec::new();
        for Owed { had, from: look, until, want, behind } in owed {
            asked.push((look.to_bits(), want));
            let Some(at) = band.iter().position(|p| p.start >= look) else { continue };
            let Some(b) = next_block(&band, at, Some(want), cancel) else { continue };
            // From the run the leader locked on: a pilot's front before a break in it is the
            // ordinary band's to read, the fade band only where the block was lost.
            let (from, to) = (start_at(b.lock), start_at(b.bits_end));
            let zeros_behind =
                b.data.starts_with(had) && b.data[had.len().min(b.data.len())..].iter().all(|&d| d == 0);
            // A header owed by its flag answers as a header in every field; a block behind one, as
            // the block it asks for.
            let answers_it = b.used_bits == 8
                && match behind {
                    Some(_) => proof::between(None, &b.data, behind) >= Proof::Answer && to <= until,
                    None if had.first() == Some(&0x00) && want == proof::HEADER_LEN => {
                        proof::of(&b.data) == Proof::Header
                    }
                    None => proof::answering(&b.data, want) == Proof::Answer,
                };
            let free = |a: &(Vec<u8>, f64, f64)| a.2 <= from || to <= a.1;
            if answers_it && !zeros_behind && from < until && answers.iter().chain(&found).all(free) {
                found.push((b.data, from, to));
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(DecodeError::Cancelled);
        }
        if found.is_empty() {
            return Ok(kept);
        }
        let mut all = answers.clone();
        all.extend(found);
        all.sort_by(|a, b| a.1.total_cmp(&b.1));
        // The ordinary pulses up to each stretch, the fade band's inside it, the pulse that runs
        // across either end cut there.
        let mut merged = Vec::with_capacity(ordinary.len());
        let mut rest = ordinary;
        for (_, from, to) in &all {
            let before = rest.partition_point(|p| p.start < *from);
            merged.extend_from_slice(&rest[..before]);
            if let Some(last) = merged.last_mut().filter(|p| end_of(p) > *from) {
                last.tstates = (from - last.start) * to_t;
            }
            merged.extend(band.iter().filter(|p| p.start >= *from && p.start < *to));
            rest = &rest[before..];
            let after = rest.partition_point(|p| p.start < *to);
            if let Some(across) = after.checked_sub(1).map(|k| rest[k]).filter(|p| end_of(p) > *to) {
                merged.push(Pulse {
                    tstates: (end_of(&across) - to) * to_t,
                    start: *to,
                    quiet: across.quiet,
                });
            }
            rest = &rest[after..];
        }
        merged.extend_from_slice(rest);
        let mut again = decode_pulses_from(&merged, grid_hz, &mut |_| {}, cancel, (0.0, 1.0))?;
        merged_now = Some(merged);
        for (data, from, to) in &all {
            let found = (0..again.blocks.len()).find(|&i| {
                matches!(&again.blocks[i].body, Body::Standard { data: d, .. } if d == data)
                    && again.extents[i].start < *to
                    && *from < again.extents[i].end
            });
            // Read again from the same pulses, the answer is there; where it is not, nothing
            // stands.
            let Some(block) = found else { return Ok(kept) };
            again.findings.push(Finding::FadeBand { block });
        }
        answers = all;
        kept = again;
    }
}

/// A block a decode owes: the bytes read in its place, where to look for it from, before where its
/// pilot must start, and the length owed; and for a header owed in front of a block, that block,
/// which must answer it.
struct Owed<'a> {
    had: &'a [u8],
    from: f64,
    until: f64,
    want: usize,
    behind: Option<&'a [u8]>,
}

/// The blocks owed bytes they do not hold, in order.
///
/// Behind a ROM header, the block it asks for: from where the header's signal ends to the end of
/// the next block of bytes, which does not answer it ([`proof::answering`]) whatever lies
/// between, or to the end of the recording where no block of bytes follows: a block whose
/// pilot or sync a dropout took was read as nothing. And each block under the header's flag
/// `0x00` that is no header its fields prove, owed a header's 19 bytes, which the ROM asks for
/// by that flag.
///
/// And a header in front of a block under the data flag `0xFF` that no header asks for, where a
/// block did not read in front of it: the pause before it holds a ROM pilot with bits behind
/// ([`Finding::Lost`]), or `pulses`, the ones `d` was read from, show the raw signal before it
/// does. The ROM loads such a block behind its header, and a dropout at the header's sync or flag
/// leaves the ordinary band no block there to owe anything. Nothing says how long the lost block
/// was, so only a header the block behind it answers is taken ([`proof::between`]), which a
/// chance checksum is not.
fn unanswered<'a>(d: &'a Decoded, pulses: &[Pulse]) -> Vec<Owed<'a>> {
    let image = proof::image;
    let mut owed = Vec::new();
    for (i, b) in d.blocks.iter().enumerate() {
        let Some(data) = image(b) else { continue };
        let extent = d.extents[i];
        if data.first() == Some(&0x00) && d.proof_of(i, proof::of(data)) < Proof::Header {
            owed.push(Owed {
                had: data,
                from: extent.start,
                until: extent.end,
                want: proof::HEADER_LEN,
                behind: None,
            });
        }
        if data.first() == Some(&0xFF) && d.proof_of(i, proof::at(&d.blocks, i)) == Proof::Verified {
            let front = i.checked_sub(1);
            let lost = d
                .findings
                .iter()
                .any(|f| matches!(f, Finding::Lost { block, what: Loss::Pilot, .. } if *block == front));
            let raw = front
                .filter(|&k| matches!(d.blocks[k].body, Body::Csw { .. } | Body::Direct { .. }))
                .is_some_and(|k| {
                    let e = d.extents[k];
                    let held = &pulses[pulses.partition_point(|p| p.start < e.start)
                        ..pulses.partition_point(|p| p.start < e.signal_end)];
                    rom_pilot(held).is_some_and(|pilot| burst_behind(held, pilot))
                });
            if lost || raw {
                let from = match front {
                    Some(k) if raw => d.extents[k].start,
                    Some(k) => d.extents[k].signal_end,
                    None => 0.0,
                };
                owed.push(Owed {
                    had: &[],
                    from,
                    until: extent.start,
                    want: proof::HEADER_LEN,
                    behind: Some(data),
                });
            }
        }
        let Some(want) = proof::asks(data) else { continue };
        let from = extent.signal_end;
        match (i + 1..d.blocks.len()).find(|&j| image(&d.blocks[j]).is_some()) {
            Some(leg) => {
                let data = image(&d.blocks[leg]).expect("found by its bytes");
                if d.proof_of(leg, proof::answering(data, want)) != Proof::Answer {
                    owed.push(Owed { had: data, from, until: d.extents[leg].end, want, behind: None });
                }
            }
            None => owed.push(Owed { had: &[], from, until: f64::INFINITY, want, behind: None }),
        }
    }
    // A block behind a header can be owed twice, as the header's answer and by its own flag: the
    // header's request, the one the ROM loads it by, comes first, and an answer to it leaves no
    // room for the second.
    owed.sort_by(|a, b| a.from.total_cmp(&b.from));
    owed.dedup_by(|b, a| a.from == b.from);
    owed
}

/// The T-states a pause opens with: TZX plays one as 1 ms at the level the block left, then low.
const PAUSE_OPENING: f64 = 3500.0;

/// The T-states of a millisecond.
const MS: f64 = TSTATES_PER_SEC as f64 / 1000.0;

/// The fewest pulses a byte of a block's run-on takes: its eight pairs. A folded bounce only adds
/// pulses.
const BYTE_PULSES: usize = 16;

/// Decodes pulses whose `start` counts samples at `grid_hz`.
///
/// Standard blocks are read as the ROM reads them ([`crate::framing`]). What lies between two
/// blocks becomes the first one's pause when it is the most a pause plays, one level or the 1 ms
/// opening and a second; anything else is sorted ([`crate::fallback`]): its gaps play as pauses
/// and its block-shaped bursts are kept as raw signal on the grid. A block after a header has the
/// header's length; the rest of a longer one is raw signal.
pub fn decode_pulses(
    pulses: &[Pulse],
    grid_hz: u32,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Decoded, DecodeError> {
    decode_pulses_from(pulses, grid_hz, progress, cancel, (0.0, 1.0))
}

/// [`decode_pulses`], with [`Progress::done`] counted from the first to the second of `span`.
fn decode_pulses_from(
    pulses: &[Pulse],
    grid_hz: u32,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
    span: (f32, f32),
) -> Result<Decoded, DecodeError> {
    let (from, to) = span;
    let mut out = Decoded {
        blocks: Vec::new(),
        extents: Vec::new(),
        findings: Vec::new(),
        rate_hz: grid_hz,
        collapsed_ends: [0; 2],
    };
    let grid_t = f64::from(TSTATES_PER_SEC) / f64::from(grid_hz);
    // Where pulse `i` starts on the grid; the end of the last for `i` past it.
    let at = |i: usize| match pulses.get(i) {
        Some(p) => p.start,
        None => pulses.last().map_or(0.0, |p| p.start + p.tstates / grid_t),
    };
    let mut pos = 0;
    let mut expected = None;
    // Where the bits of the last block read stop, past its end when it ran on.
    let mut bits_end = 0;
    // Whether the last block read stopped short of its end ([`Loss::Rest`]).
    let mut cut = false;
    loop {
        // From where the bits stop: a run-on's 1 bits are as long as pilot, and the next block's
        // pilot must not start among them.
        let found = next_block(pulses, bits_end.max(pos), expected, cancel);
        if cancel.load(Ordering::Relaxed) {
            return Err(DecodeError::Cancelled);
        }
        let stretch_end = found.as_ref().map_or(pulses.len(), |b| b.tone);
        let mut stretch: Vec<Pulse> = pulses[pos..stretch_end].to_vec();
        if let (Some(b), Some(last)) = (&found, stretch.last_mut()) {
            // After a pause the level is low and a pilot starts at that level, so its first half
            // has no edge of its own: it ends the stretch's last pulse, when that pulse is longer
            // than any the ROM takes as pilot.
            if last.tstates > PILOT_PULSE_MOST {
                last.tstates -= b.pilot_half;
            }
        }
        let keep = bits_end.clamp(pos, stretch_end) - pos;
        let collapsed = settle(&mut out, &stretch, keep, cut, at(stretch_end), grid_hz);
        if pos == 0 {
            out.collapsed_ends[0] = collapsed;
        }
        let Some(b) = found else {
            if pos > 0 {
                out.collapsed_ends[1] = collapsed;
            }
            break;
        };

        let index = out.blocks.len();
        let data = b.data;
        if b.used_bits < 8 {
            let completes = proof::xor_ok(&data);
            out.findings.push(Finding::PartialFinalByte { block: index, used_bits: b.used_bits, completes });
        }
        if b.stray_zero {
            out.findings.push(Finding::StrayZero { block: index });
        }
        let bad = proof::of(&data) == Proof::None;
        if bad {
            out.findings.push(Finding::BadChecksum { block: index });
        }
        if b.merged_sync {
            out.findings.push(Finding::MergedSync { block: index });
        }
        if b.short_leader {
            out.findings.push(Finding::ShortLeader { block: index });
        }
        if b.unlike > 0 {
            out.findings.push(Finding::UnlikeHalves { block: index, bits: b.unlike });
        }
        if b.snag {
            out.findings.push(Finding::Snag { block: index });
        }
        if b.short {
            let expected = expected.unwrap_or(0);
            out.findings.push(Finding::Short { block: index, expected, read: data.len() });
        }
        // Without a header to say how long it is, a block ends where its bits stop, and a wrong
        // checksum says they stopped before the saver's did; so do bits that stop inside a byte.
        cut = b.short || b.used_bits < 8 || expected.is_none() && bad;
        expected = proof::asks(&data);
        out.blocks.push(Block::new(Body::Standard { pause: 0, data }));
        out.extents.push(Extent { start: at(b.tone), end: at(b.end), signal_end: at(b.end) });
        pos = b.end;
        bits_end = b.bits_end;
        let done = from + (to - from) * (pos as f32 / pulses.len() as f32);
        progress(Progress { pulses_read: pos, pulses: pulses.len(), blocks: out.blocks.len(), done });
    }
    if out.blocks.is_empty() {
        // The whole recording is the lead-in, and there is no first block for it to lead into.
        let lead_in = out.findings.iter().position(|f| matches!(f, Finding::LeadIn { .. }));
        let (ms, held) = match lead_in.map(|k| out.findings.remove(k)) {
            Some(Finding::LeadIn { ms, pulses }) => (ms, pulses),
            _ => (0, 0),
        };
        let pilot = if rom_pilot(pulses).is_some() {
            PilotHeard::Whole
        } else if pilot_in_pieces(pulses) {
            PilotHeard::Pieces
        } else {
            PilotHeard::None
        };
        out.findings.push(Finding::NoBlock { ms, pulses: held, pilot });
    }
    progress(Progress {
        pulses_read: pulses.len(),
        pulses: pulses.len(),
        blocks: out.blocks.len(),
        done: to,
    });
    Ok(out)
}

/// Places `stretch`, which ends at `end`, after the blocks read so far. A pause-shaped stretch is
/// the last block's pause; any other is sorted, its first `keep` pulses kept as signal. A gap too
/// short to round to a millisecond is kept with the signal, since a pause of 0 ms is no pause.
/// Before any block, a gap is the lead-in, left out and reported. How many pulses went into
/// gaps that are not a plain pause.
///
/// A gap that holds a block that did not read says so ([`Finding::Lost`]): a pilot tone with
/// signal behind it, or, `cut` where the block in front stopped short of its end, signal from
/// where it stopped on, `BYTE_PULSES` pulses of it with no gap among them; a block's pause
/// opens with its 1 ms level and then a gap.
fn settle(out: &mut Decoded, stretch: &[Pulse], keep: usize, cut: bool, end: f64, grid_hz: u32) -> usize {
    let at = |k: usize| stretch.get(k).map_or(end, |p| p.start);
    let length = |r: &std::ops::Range<usize>| stretch[r.clone()].iter().map(|p| p.tstates).sum::<f64>();
    let opening = PAUSE_OPENING + f64::from(TSTATES_PER_SEC) / f64::from(grid_hz);
    // A pause as a player leaves it: nothing, one level, or the 1 ms opening and the rest. The
    // opening level may be up to one sample of the grid longer than 1 ms.
    let pause_shaped = |p: &[Pulse]| match p {
        [] | [_] => true,
        [a, _] => a.tstates <= opening,
        _ => false,
    };
    if keep == 0 && pause_shaped(stretch) {
        add_pause(out, stretch.iter().map(|p| p.tstates).sum(), at(0), end, 0);
        return 0;
    }
    let mut collapsed = 0;
    let mut parts: Vec<Part> = Vec::new();
    for part in sort(stretch, keep) {
        let part = match part {
            Part::Gap(r) if ms(length(&r)) == 0 => Part::Signal(r),
            part => part,
        };
        match (parts.last_mut(), part) {
            (Some(Part::Signal(r)), Part::Signal(n)) => r.end = n.end,
            (_, part) => parts.push(part),
        }
    }
    for part in parts {
        match part {
            Part::Gap(r) => {
                let gap = &stretch[r.clone()];
                let pulses = if pause_shaped(gap) { 0 } else { r.len() };
                let held = add_pause(out, length(&r), at(r.start), at(r.end), pulses);
                if pulses == 0 {
                    continue;
                }
                collapsed += pulses;
                let rest =
                    cut && r.start == 0 && gap.get(..BYTE_PULSES).is_some_and(|p| p.iter().all(signal));
                let pilot = rom_pilot(gap);
                let lost = if rest {
                    Some(Loss::Rest)
                } else {
                    pilot.clone().filter(|p| burst_behind(gap, p.clone())).map(|_| Loss::Pilot)
                };
                match (lost, held) {
                    (Some(what), block) => out.findings.push(Finding::Lost { block, pulses, what }),
                    (None, Some(block)) => {
                        out.findings.push(Finding::Collapsed { block, pulses, pilot: pilot.is_some() })
                    }
                    (None, None) => {}
                }
            }
            Part::Signal(r) => {
                // Raw signal holds what some loader can read: a burst shaped like a block, or a
                // block's run-on past its header's length, whole bytes of it. A gap too short
                // for a millisecond goes in only beside one of those.
                debug_assert!(
                    (r.start < keep && keep >= BYTE_PULSES) || block_shaped(&stretch[r.clone()]),
                    "raw signal of {} pulses holds no block, {keep} kept",
                    r.len()
                );
                if let Some(block) = raw_signal(&stretch[r.clone()], grid_hz) {
                    out.findings.push(Finding::RawSignal { block: out.blocks.len() });
                    if r.start == 0 && keep > 0 {
                        out.findings.push(Finding::RunOn { block: out.blocks.len() });
                    }
                    out.blocks.push(block);
                    out.extents.push(Extent { start: at(r.start), end: at(r.end), signal_end: at(r.end) });
                }
            }
        }
    }
    collapsed
}

/// `t` T-states in milliseconds, rounded half to even.
fn ms(t: f64) -> u64 {
    (t / MS).round_ties_even() as u64
}

/// Adds a pause of `t` T-states, from `start` to `end`, to the last block, and past what its field
/// holds as Pause blocks; the block it starts in, if any. Before any block it is the lead-in,
/// which holds `pulses` that are not a plain pause.
fn add_pause(out: &mut Decoded, t: f64, start: f64, end: f64, pulses: usize) -> Option<usize> {
    let mut ms = ms(t);
    let Some(last) = out.blocks.len().checked_sub(1) else {
        if ms > 0 {
            out.findings.push(Finding::LeadIn { ms, pulses });
        }
        return None;
    };
    let held = match &mut out.blocks[last].body {
        Body::Standard { pause, .. } | Body::Csw { pause, .. } | Body::Pause { pause } => Some(pause),
        _ => None,
    };
    let mut first = None;
    if let Some(pause) = held {
        let part = ms.min(u64::from(u16::MAX - *pause));
        *pause += part as u16;
        ms -= part;
        out.extents[last].end = end;
        first = Some(last);
    }
    while ms > 0 {
        let part = ms.min(u64::from(u16::MAX));
        first.get_or_insert(out.blocks.len());
        out.blocks.push(Block::new(Body::Pause { pause: part as u16 }));
        out.extents.push(Extent { start, end, signal_end: start });
        ms -= part;
    }
    first
}
