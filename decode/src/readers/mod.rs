// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Tape recording files read from their bytes: WAV, VOC and IFF/8SVX as samples, CSW as its
//! pulse data, and an emulator's OUT trace as pulses.
//!
//! Sampled audio is not converted up front. A [`Sound`] keeps the file's own bytes, borrowed
//! where they lie in one piece, and turns one sample into an `f32` when it is asked for, so a
//! long recording costs its file and nothing more.

use std::borrow::Cow;
use std::fmt;

mod csw;
mod iff;
mod out;
mod voc;
mod wav;

pub use csw::Csw;

/// The longest recording a reader expands: longer than any cassette. A file whose blocks claim
/// more (a VOC file's silence blocks can) is refused before its samples are allocated.
pub const MAX_SECONDS: u32 = 4 * 60 * 60;

/// Which channel of a stereo recording to read. A mono recording reads the same either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Channel {
    /// The average of left and right.
    #[default]
    Mix,
    Left,
    Right,
}

/// A recording file, read.
pub enum Recording<'a> {
    Sound(Sound<'a>),
    Csw(Csw<'a>),
    /// An emulator's OUT trace: the length of every pulse in T-states, exactly.
    Out(Vec<u64>),
}

/// Reads `bytes` as whichever recording format they are. `name` is the file's name, which is
/// how an OUT trace is recognised: unlike the others, it has no signature.
pub fn read<'a>(name: &str, bytes: &'a [u8], channel: Channel) -> Result<Recording<'a>, ReadError> {
    let is = |sig: &[u8], at: usize| bytes.get(at..at + sig.len()) == Some(sig);
    if is(b"RIFF", 0) && is(b"WAVE", 8) {
        wav::read(bytes, channel).map(Recording::Sound)
    } else if is(voc::SIGNATURE, 0) {
        voc::read(bytes).map(Recording::Sound)
    } else if is(b"FORM", 0) && is(b"8SVX", 8) {
        iff::read(bytes).map(Recording::Sound)
    } else if is(csw::SIGNATURE, 0) {
        csw::read(bytes).map(Recording::Csw)
    } else if name.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("out")) {
        out::read(bytes).map(Recording::Out)
    } else {
        Err(ReadError::Unrecognised)
    }
}

/// Why a file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// None of the recording formats: no known signature, and not named `.out`.
    Unrecognised,
    /// The file ends inside a header or a block that its format requires.
    Truncated { format: &'static str },
    /// A valid file of its format, using something these readers do not read.
    Unsupported { format: &'static str, what: String },
    /// The file breaks its own format's rules.
    Malformed { format: &'static str, what: String },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Unrecognised => write!(f, "not a tape recording this app reads"),
            ReadError::Truncated { format } => write!(f, "{format}: the file is cut short"),
            ReadError::Unsupported { format, what } => write!(f, "{format}: {what} is not supported"),
            ReadError::Malformed { format, what } => write!(f, "{format}: {what}"),
        }
    }
}

impl std::error::Error for ReadError {}

/// How a sample is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// Unsigned 8 bit, centred on 128 (WAV, VOC).
    U8,
    /// Signed 8 bit (IFF/8SVX).
    I8,
    /// Signed little-endian integers of 2 or 3 bytes (WAV).
    I16,
    I24,
    /// Little-endian IEEE float (WAV).
    F32,
}

impl Encoding {
    fn width(self) -> usize {
        match self {
            Encoding::U8 | Encoding::I8 => 1,
            Encoding::I16 => 2,
            Encoding::I24 => 3,
            Encoding::F32 => 4,
        }
    }

    /// The sample at the start of `b`, scaled to -1.0..1.0.
    fn get(self, b: &[u8]) -> f32 {
        match self {
            Encoding::U8 => (f32::from(b[0]) - 128.0) / 128.0,
            Encoding::I8 => f32::from(b[0] as i8) / 128.0,
            Encoding::I16 => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0,
            Encoding::I24 => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            Encoding::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        }
    }
}

/// Sampled audio, one channel of it as chosen when the file was read.
pub struct Sound<'a> {
    /// The format's name, for messages.
    pub format: &'static str,
    pub rate_hz: u32,
    /// Channels in the file; the samples are one of them, or their mix.
    pub channels: u16,
    pub encoding: Encoding,
    /// Bytes of audio the file's header announces that the file does not hold. The samples
    /// are what is there; this is how much of the recording is missing.
    pub missing_bytes: u64,
    /// What the reader passed over without reading: a VOC file's text and marker blocks.
    pub skipped: Vec<String>,
    data: Cow<'a, [u8]>,
    channel: Channel,
}

impl<'a> Sound<'a> {
    fn new(
        format: &'static str,
        rate_hz: u32,
        channels: u16,
        encoding: Encoding,
        data: Cow<'a, [u8]>,
    ) -> Self {
        Sound {
            format,
            rate_hz,
            channels,
            encoding,
            missing_bytes: 0,
            skipped: Vec::new(),
            data,
            channel: Channel::Mix,
        }
    }

    fn frame_bytes(&self) -> usize {
        self.encoding.width() * usize::from(self.channels)
    }

    /// The number of samples (of frames, in a stereo file).
    pub fn len(&self) -> usize {
        self.data.len() / self.frame_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn seconds(&self) -> f64 {
        self.len() as f64 / f64::from(self.rate_hz)
    }

    /// Sample `i`, scaled to -1.0..1.0. Panics past [`Sound::len`].
    pub fn sample(&self, i: usize) -> f32 {
        let w = self.encoding.width();
        let frame = &self.data[i * self.frame_bytes()..];
        if self.channels == 1 {
            return self.encoding.get(frame);
        }
        let (left, right) = (self.encoding.get(frame), self.encoding.get(&frame[w..]));
        match self.channel {
            Channel::Mix => (left + right) / 2.0,
            Channel::Left => left,
            Channel::Right => right,
        }
    }

    /// Every sample in order.
    pub fn samples(&self) -> impl Iterator<Item = f32> + '_ {
        (0..self.len()).map(|i| self.sample(i))
    }
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
