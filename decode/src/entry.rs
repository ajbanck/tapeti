// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! What the apps call: describe a recording file, and keep it whole as raw signal.
//!
//! Sampled audio (WAV, VOC, IFF) is kept in Direct Recording blocks, a CSW file in a CSW block
//! as it is, and an OUT trace in a CSW block at 3.5 MHz.

use crate::capture::{
    csw_block, csw_of_tstates, direct_recording, spacing, tzx_size, MAX_BLOCK_BYTES, MAX_CSW_RATE,
};
use crate::readers::{read, Channel, Csw, ReadError, Recording};
use crate::waveform::filter::{Filter, FilterError};
use std::fmt;
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
            let design = || filter.design(sound.rate_hz).map_err(ImportError::Filter);
            let mean = mean(design()?.apply(sound.samples()));
            let levels = design()?.apply(sound.samples()).map(move |v| f64::from(v) > mean);
            direct_recording(spacing, levels, MAX_BLOCK_BYTES)
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
