// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! What the apps call: describe a recording file, keep it whole as raw signal, or decode it.
//!
//! Kept whole, sampled audio (WAV, VOC, IFF) goes into Direct Recording blocks, a CSW file into a
//! CSW block as it is, and an OUT trace into a CSW block at 3.5 MHz. Decoded, standard blocks are
//! read as the ROM reads them and every stretch between them is kept as raw signal.

use crate::capture::{
    csw_block, csw_of_tstates, direct_recording, spacing, tzx_size, MAX_BLOCK_BYTES, MAX_CSW_RATE,
};
use crate::fallback::raw_signal;
use crate::framing::{next_block, PILOT_PULSE_MOST};
use crate::pulses::{from_runs, from_tstates, Pulse};
use crate::readers::{read, Channel, Csw, ReadError, Recording, Sound};
use crate::waveform::edges;
use crate::waveform::filter::{Filter, FilterError};
use std::fmt;
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
    spacing(rate_hz)
        .ok_or_else(|| ImportError::Unsupported(format!("a sample rate of {rate_hz} Hz is too low")))
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
}

/// How far a decode has come.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub pulses_read: usize,
    pub pulses: usize,
    pub blocks: usize,
}

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

/// Decodes a recording file.
///
/// CSW and OUT go by their pulses, sampled audio by the edges the edge detector finds in it,
/// after `filter`. `window` keeps only the pulses that start inside it, in seconds.
#[allow(clippy::too_many_arguments)]
pub fn decode_file(
    name: &str,
    bytes: &[u8],
    channel: Channel,
    filter: Filter,
    window: Option<(f64, f64)>,
    inflate: Inflate,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Decoded, DecodeError> {
    let (pulses, rate_hz) = match read(name, bytes, channel).map_err(ImportError::from)? {
        Recording::Sound(sound) => {
            let found =
                edges::detect(|| sound.samples(), sound.rate_hz, filter).map_err(ImportError::Filter)?;
            (edges::pulses(&found, sound.rate_hz, sound.len()), sound.rate_hz)
        }
        Recording::Csw(c) => {
            csw_rate(&c)?;
            (from_runs(&csw_runs(&c, inflate)?, c.rate_hz), c.rate_hz)
        }
        Recording::Out(lengths) => (from_tstates(&lengths), TSTATES_PER_SEC),
    };
    let pulses = match window {
        Some((from, to)) => {
            let inside = |p: &Pulse| (from..to).contains(&(p.start / f64::from(rate_hz)));
            pulses.into_iter().filter(inside).collect()
        }
        None => pulses,
    };
    if pulses.is_empty() {
        return Err(no_sound().into());
    }
    decode_pulses(&pulses, rate_hz, progress, cancel)
}

/// The T-states a pause opens with: TZX plays one as 1 ms at the level the block left, then low.
const PAUSE_OPENING: f64 = 3500.0;

/// The T-states of a millisecond.
const MS: f64 = TSTATES_PER_SEC as f64 / 1000.0;

/// Decodes pulses whose `start` counts samples at `grid_hz`.
///
/// Standard blocks are read as the ROM reads them ([`crate::framing`]). What lies between two
/// blocks becomes the first one's pause when it is the most a pause plays, one level or the 1 ms
/// opening and a second; anything else is kept as raw signal on the grid, nothing dropped. A
/// block after a header has the header's length; the rest of a longer one is raw signal.
pub fn decode_pulses(
    pulses: &[Pulse],
    grid_hz: u32,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Decoded, DecodeError> {
    let mut out = Decoded { blocks: Vec::new(), extents: Vec::new(), findings: Vec::new(), rate_hz: grid_hz };
    let grid_t = f64::from(TSTATES_PER_SEC) / f64::from(grid_hz);
    // Where pulse `i` starts on the grid; the end of the last for `i` past it.
    let at = |i: usize| match pulses.get(i) {
        Some(p) => p.start,
        None => pulses.last().map_or(0.0, |p| p.start + p.tstates / grid_t),
    };
    let mut pos = 0;
    let mut expected = None;
    // The last block read, until what follows it is settled.
    let mut open: Option<usize> = None;
    loop {
        let found = next_block(pulses, pos, expected, cancel);
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
        settle(&mut out, open.take(), &stretch, at(pos), at(stretch_end), grid_hz);
        let Some(b) = found else { break };

        let index = out.blocks.len();
        if b.data.iter().fold(0, |x, &d| x ^ d) != 0 {
            out.findings.push(Finding::BadChecksum { block: index });
        }
        if b.short {
            let expected = expected.unwrap_or(0);
            out.findings.push(Finding::Short { block: index, expected, read: b.data.len() });
        }
        expected = header_asks(&b.data);
        out.blocks.push(Block::new(Body::Standard { pause: 0, data: b.data }));
        out.extents.push(Extent { start: at(b.tone), end: at(b.end) });
        open = Some(index);
        pos = b.end;
        progress(Progress { pulses_read: pos, pulses: pulses.len(), blocks: out.blocks.len() });
    }
    Ok(out)
}

/// Places the stretch of pulses from `start` to `end` after the block `open`: its pause when the
/// stretch is pause-shaped, raw signal otherwise. Before any block, a pause-shaped stretch is a
/// Pause block.
fn settle(out: &mut Decoded, open: Option<usize>, stretch: &[Pulse], start: f64, end: f64, grid_hz: u32) {
    let opening = PAUSE_OPENING + f64::from(TSTATES_PER_SEC) / f64::from(grid_hz);
    let pause = match stretch {
        [] => Some(0.0),
        [a] => Some(a.tstates),
        // The opening level may be up to one sample of the grid longer than 1 ms.
        [a, b] if a.tstates <= opening => Some(a.tstates + b.tstates),
        _ => None,
    };
    if let Some(t) = pause {
        // Milliseconds rounded half to even; past what one block's field holds, Pause blocks.
        let mut ms = (t / MS).round_ties_even() as u64;
        if let Some(i) = open {
            let first = ms.min(u64::from(u16::MAX));
            if let Body::Standard { pause, .. } = &mut out.blocks[i].body {
                *pause = first as u16;
            }
            out.extents[i].end = end;
            ms -= first;
        }
        while ms > 0 {
            let part = ms.min(u64::from(u16::MAX));
            out.blocks.push(Block::new(Body::Pause { pause: part as u16 }));
            out.extents.push(Extent { start, end });
            ms -= part;
        }
        return;
    }
    if let Some(block) = raw_signal(stretch, grid_hz) {
        out.findings.push(Finding::RawSignal { block: out.blocks.len() });
        out.blocks.push(block);
        out.extents.push(Extent { start, end });
    }
}

/// The length, flag and checksum included, that a ROM header in `data` asks the next block for.
fn header_asks(data: &[u8]) -> Option<usize> {
    let ok = data.len() == 19 && data[0] == 0 && data[1] <= 3 && data.iter().fold(0, |x, &d| x ^ d) == 0;
    ok.then(|| usize::from(u16::from_le_bytes([data[12], data[13]])) + 2)
}
