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
use crate::fallback::{block_shaped, raw_signal, sort, Part};
use crate::framing::{next_block, proven_header, PILOT_PULSE_MOST};
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
    /// that holds them.
    Collapsed { block: usize, pulses: usize },
    /// Silence before the first block, `ms` long, left out: it holds nothing but its length,
    /// and a tape starts with its first block.
    LeadIn { ms: u64 },
    /// Of a stereo recording's mix and its two channels, the decode of `kept` delivers most.
    Channel { kept: Channel },
    /// The block's last byte holds only `used_bits` bits read from the tape, from its top bit;
    /// the rest are 0s, since a standard block cannot say how many bits its last byte holds.
    PartialFinalByte { block: usize, used_bits: u8 },
    /// A 0x00 byte read behind the header, a tape copier's, was left out ([`crate::framing::RawBlock::stray_zero`]).
    StrayZero { block: usize },
    /// Of the decodes on the zero line and on the signal's own midline, the midline's delivers
    /// most ([`edges::Axis`]).
    Midline,
    /// The block was read from `channel`, where the channel kept read it in error or not at
    /// all ([`crate::channels::combine`]).
    FromChannel { block: usize, channel: Channel },
    /// The block was read as the left and the right channel both read it, where the mix kept
    /// read other bytes whose checksum held too ([`crate::channels::outvote`]).
    BothChannels { block: usize },
    /// The sync pair had merged into a half-wave beside it, or lay past the ROM's window, and the
    /// block was read where its bits are ([`crate::framing::DECIDE`]).
    MergedSync { block: usize },
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
            | Finding::BothChannels { block } => Some(block),
            Finding::LeadIn { .. } | Finding::Channel { .. } | Finding::Midline => None,
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
            Finding::Collapsed { pulses, .. } => {
                format!("The pause held {pulses} pulses of noise; they play as silence.")
            }
            Finding::LeadIn { ms } => {
                format!("lead-in: {:.1} s of silence before the first block, left out", ms as f64 / 1000.0)
            }
            Finding::Channel { kept: Channel::Mix } => {
                "Read from both channels mixed: neither alone reads more.".into()
            }
            Finding::Channel { kept } => format!(
                "Read from the {} channel: of the mix and each channel alone, it reads the most.",
                if kept == Channel::Left { "left" } else { "right" }
            ),
            Finding::FromChannel { channel, .. } => format!(
                "Read from the {} channel: the channel kept did not read it whole.",
                match channel {
                    Channel::Left => "left",
                    Channel::Right => "right",
                    Channel::Mix => "mixed",
                }
            ),
            Finding::BothChannels { .. } => {
                "Read as the left and right channels both read it: the mix read other bytes, whose checksum held by chance.".into()
            }
            Finding::Midline => {
                "Read about the signal's own centre line, which wanders off zero: it reads more.".into()
            }
            Finding::PartialFinalByte { used_bits, .. } => format!(
                "The last byte holds only {used_bits} bit{} read from the tape; the rest are 0.",
                if used_bits == 1 { "" } else { "s" }
            ),
            Finding::StrayZero { .. } => {
                "A stray 0x00 byte behind the header, which a tape copier leaves, is left out.".into()
            }
            Finding::MergedSync { .. } => {
                "The sync pulses are merged into the signal beside them or distorted; the bits are read where the block is.".into()
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
    /// most ([`crate::channels`]).
    #[default]
    Best,
    Only(Channel),
}

/// Decodes a recording file.
///
/// CSW and OUT go by their pulses, sampled audio by the edges the edge detector finds in it,
/// after `filter`. `window` keeps only the pulses that start inside it, in the source's samples
/// as [`Pulse::start`] counts them; positions stay those of the whole file. A stereo recording
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
    let stereo = matches!(read(name, bytes, Channel::Mix), Ok(Recording::Sound(s)) if s.channels == 2);
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
    decode_channel(name, bytes, channel, filter, window, inflate, progress, cancel)
}

/// The decode `decode` makes of the mix, the left or the right channel that delivers most. A
/// channel that cannot be decoded delivers nothing; when none can, the mix's error stands.
fn best_channel(
    mut decode: impl FnMut(Channel, &mut dyn FnMut(Progress)) -> Result<Decoded, DecodeError>,
    progress: &mut dyn FnMut(Progress),
) -> Result<Decoded, DecodeError> {
    const ORDER: [Channel; 3] = [Channel::Mix, Channel::Left, Channel::Right];
    let mut decodes = Vec::with_capacity(ORDER.len());
    let mut most = 0;
    for (i, channel) in ORDER.into_iter().enumerate() {
        let got = decode(channel, &mut |p| {
            most = most.max(p.blocks);
            let done = (i as f32 + p.done) / ORDER.len() as f32;
            progress(Progress { blocks: most, done, ..p });
        });
        if matches!(got, Err(DecodeError::Cancelled)) {
            return got;
        }
        decodes.push(got);
    }
    let (i, decodes) = best_index(decodes)?;
    let mut decodes: Vec<Option<Decoded>> = decodes.into_iter().map(Result::ok).collect();
    let mut kept = decodes[i].take().expect("the decode picked is one that read");
    let donors: Vec<(Channel, &Decoded)> =
        ORDER.iter().zip(&decodes).filter_map(|(&c, d)| d.as_ref().map(|d| (c, d))).collect();
    if let (Channel::Mix, [_, Some(left), Some(right)]) = (ORDER[i], &decodes[..]) {
        crate::channels::outvote(&mut kept, left, right);
    }
    crate::channels::combine(&mut kept, &donors);
    kept.findings.insert(0, Finding::Channel { kept: ORDER[i] });
    Ok(kept)
}

/// Which of `decodes` delivers most ([`crate::channels::pick`]), the earliest on a tie, and that
/// decode. One that failed delivers nothing; when all failed, the first one's error stands.
fn best_of(decodes: Vec<Result<Decoded, DecodeError>>) -> Result<(usize, Decoded), DecodeError> {
    let (i, mut decodes) = best_index(decodes)?;
    Ok((i, decodes.swap_remove(i)?))
}

/// Which of `decodes` [`best_of`] picks, with them all.
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
    let views: Vec<&[Block]> =
        decodes.iter().map(|d| d.as_ref().map_or(&[][..], |d| &d.blocks[..])).collect();
    let i = crate::channels::pick(&views);
    Ok((i, decodes))
}

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
) -> Result<Decoded, DecodeError> {
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
    decode_pulses_from(&in_window(pulses)?, rate_hz, progress, cancel, (0.0, 1.0))
}

/// Decodes sampled audio, its edges found on the zero line and on the signal's own midline
/// ([`edges::Axis`]), keeping the decode that delivers most, the zero line's on a tie, as the
/// channels are chosen ([`crate::channels`]). `in_window` keeps the pulses asked for.
///
/// A worn tape's centre can wander further than its shrunken short half-waves swing, so that
/// they never cross zero and two bits merge into one; about the midline every lobe is its own
/// excursion. No statistic says beforehand which line a recording reads best on, and on a
/// clean one both read the same, so both are decoded and judged by what they deliver. Where
/// the zero line's decode holds nothing but whole blocks and plain pauses, the midline is not
/// walked: the pick leaves a decode only for one that delivers more, and there is nothing more.
fn decode_sound(
    sound: &Sound,
    filter: Filter,
    in_window: impl Fn(Vec<Pulse>) -> Result<Vec<Pulse>, DecodeError>,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Decoded, DecodeError> {
    use std::cell::{Cell, RefCell};
    const AXES: [edges::Axis; 2] = [edges::Axis::Zero, edges::Axis::Midline];
    // The samples run out when the flag is set, so a cancel ends the edge detector's passes over
    // a long recording too, not just the reading of blocks after them. They are counted as they
    // go, against the passes the detector makes at most: the statistics once, then the edges and
    // the quiet pulses on each line.
    let read = Cell::new(0usize);
    let all = ((1 + 2 * AXES.len()) * sound.len()).max(1);
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
    let decodes = edges::detect_pulses_each(samples, sound.rate_hz, filter, &AXES, |_, pulses| {
        if cancel.load(Ordering::Relaxed) {
            return ControlFlow::Break(Err(DecodeError::Cancelled));
        }
        let from = edges_done() + blocks_done.get();
        blocks_done.set(blocks_done.get() + per_axis);
        let mut each = |p: Progress| {
            most.set(most.get().max(p.blocks));
            (report.borrow_mut())(Progress { blocks: most.get(), ..p });
        };
        let got = in_window(pulses)
            .and_then(|p| decode_pulses_from(&p, sound.rate_hz, &mut each, cancel, (from, from + per_axis)));
        let whole = got.as_ref().is_ok_and(|d| {
            !d.findings.iter().any(|f| {
                matches!(
                    f,
                    Finding::BadChecksum { .. }
                        | Finding::Short { .. }
                        | Finding::RawSignal { .. }
                        | Finding::Collapsed { .. }
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
    let (i, mut kept) = best_of(decodes)?;
    if AXES[i] == edges::Axis::Midline {
        kept.findings.insert(0, Finding::Midline);
    }
    progress(Progress { pulses_read: 0, pulses: 0, blocks: most.get(), done: 1.0 });
    Ok(kept)
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
        let collapsed = settle(&mut out, &stretch, keep, at(stretch_end), grid_hz);
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
            out.findings.push(Finding::PartialFinalByte { block: index, used_bits: b.used_bits });
        }
        if b.stray_zero {
            out.findings.push(Finding::StrayZero { block: index });
        }
        if data.iter().fold(0, |x, &d| x ^ d) != 0 {
            out.findings.push(Finding::BadChecksum { block: index });
        }
        if b.merged_sync {
            out.findings.push(Finding::MergedSync { block: index });
        }
        if b.short {
            let expected = expected.unwrap_or(0);
            out.findings.push(Finding::Short { block: index, expected, read: data.len() });
        }
        expected = header_asks(&data);
        out.blocks.push(Block::new(Body::Standard { pause: 0, data }));
        out.extents.push(Extent { start: at(b.tone), end: at(b.end), signal_end: at(b.end) });
        pos = b.end;
        bits_end = b.bits_end;
        let done = from + (to - from) * (pos as f32 / pulses.len() as f32);
        progress(Progress { pulses_read: pos, pulses: pulses.len(), blocks: out.blocks.len(), done });
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
fn settle(out: &mut Decoded, stretch: &[Pulse], keep: usize, end: f64, grid_hz: u32) -> usize {
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
        add_pause(out, stretch.iter().map(|p| p.tstates).sum(), at(0), end);
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
                let held = add_pause(out, length(&r), at(r.start), at(r.end));
                if !pause_shaped(&stretch[r.clone()]) {
                    collapsed += r.len();
                    if let Some(block) = held {
                        out.findings.push(Finding::Collapsed { block, pulses: r.len() });
                    }
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
/// holds as Pause blocks; the block it starts in, if any. Before any block it is the lead-in.
fn add_pause(out: &mut Decoded, t: f64, start: f64, end: f64) -> Option<usize> {
    let mut ms = ms(t);
    let Some(last) = out.blocks.len().checked_sub(1) else {
        if ms > 0 {
            out.findings.push(Finding::LeadIn { ms });
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

/// The length, flag and checksum included, that a ROM header in `data` asks the next block for.
fn header_asks(data: &[u8]) -> Option<usize> {
    proven_header(data).then(|| usize::from(u16::from_le_bytes([data[12], data[13]])) + 2)
}
