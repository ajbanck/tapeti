// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! CSW: a square wave stored as the length of each pulse in samples.
//!
//! Version 1: rate (16 bits) at 0x19, compression at 0x1B, flags at 0x1C, data at 0x20.
//! Version 2: rate (32 bits) at 0x19, pulse count at 0x1D, compression at 0x21, flags at 0x22,
//! header extension length at 0x23, data at 0x34 plus that length. Flag bit 0 is the starting
//! level. The data is a run length per byte, or a 0 byte and a 32-bit run; version 2 may
//! zlib-compress it (Z-RLE).

use super::{le16, le32, ReadError};

pub(super) const SIGNATURE: &[u8] = b"Compressed Square Wave\x1a";
const FORMAT: &str = "CSW";
const RLE: u8 = 1;
const Z_RLE: u8 = 2;

/// A CSW file's header and its pulse data, untouched.
#[derive(Debug)]
pub struct Csw<'a> {
    pub version: (u8, u8),
    pub rate_hz: u32,
    /// 1 for run lengths as they are, 2 for zlib-compressed run lengths (Z-RLE).
    pub compression: u8,
    /// The pulse count a version 2 header states.
    pub pulse_count: Option<u32>,
    /// Whether the signal starts at the high level.
    pub starts_high: bool,
    /// The pulse data as the file stores it.
    pub data: &'a [u8],
}

impl Csw<'_> {
    /// The length of every pulse, in samples. Z-RLE data must be inflated by the caller and
    /// passed as `inflated`, because this crate has no zlib; run lengths as they are ignore it.
    pub fn pulses(&self, inflated: Option<&[u8]>) -> Result<Vec<u32>, ReadError> {
        let runs = match (self.compression, inflated) {
            (Z_RLE, Some(data)) => data,
            (Z_RLE, None) => {
                let what = "Z-RLE data that has not been inflated".to_string();
                return Err(ReadError::Unsupported { format: FORMAT, what });
            }
            _ => self.data,
        };
        let pulses = tapeti_core::audio::decode_csw_rle(runs);
        if let Some(count) = self.pulse_count.filter(|&c| c as usize != pulses.len()) {
            let what = format!("the header counts {count} pulses, the data holds {}", pulses.len());
            return Err(ReadError::Malformed { format: FORMAT, what });
        }
        Ok(pulses)
    }
}

pub(super) fn read(bytes: &[u8]) -> Result<Csw<'_>, ReadError> {
    let truncated = ReadError::Truncated { format: FORMAT };
    let unsupported = |what: String| ReadError::Unsupported { format: FORMAT, what };
    let version = (*bytes.get(0x17).ok_or(truncated.clone())?, *bytes.get(0x18).ok_or(truncated.clone())?);
    let (rate_hz, compression, flags, pulse_count, start) = match version.0 {
        1 => {
            let rate = le16(bytes, 0x19).ok_or(truncated.clone())?;
            let flags = *bytes.get(0x1C).ok_or(truncated.clone())?;
            (u32::from(rate), bytes[0x1B], flags, None, 0x20)
        }
        2 => {
            let rate = le32(bytes, 0x19).ok_or(truncated.clone())?;
            let count = le32(bytes, 0x1D).ok_or(truncated.clone())?;
            let extension = *bytes.get(0x23).ok_or(truncated.clone())?;
            (rate, bytes[0x21], bytes[0x22], Some(count), 0x34 + usize::from(extension))
        }
        major => return Err(unsupported(format!("version {major}.{:02}", version.1))),
    };
    let data = bytes.get(start..).ok_or(truncated)?;
    match compression {
        RLE => {}
        Z_RLE if version.0 == 2 => {}
        c => return Err(unsupported(format!("compression {c} in a version {} file", version.0))),
    }
    if rate_hz == 0 {
        return Err(ReadError::Malformed { format: FORMAT, what: "the sample rate is 0".into() });
    }
    Ok(Csw { version, rate_hz, compression, pulse_count, starts_high: flags & 1 != 0, data })
}
