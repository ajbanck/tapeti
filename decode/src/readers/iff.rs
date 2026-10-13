// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! IFF/8SVX: the Amiga's sampled sound. Signed 8-bit mono in a `BODY` chunk, its rate in the
//! `VHDR` chunk; every number big-endian, every chunk padded to an even length. Only
//! uncompressed sound is read, not the Fibonacci-delta packing.

use super::{Encoding, ReadError, Sound};
use std::borrow::Cow;

const FORMAT: &str = "IFF/8SVX";

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

pub(super) fn read(bytes: &[u8]) -> Result<Sound<'_>, ReadError> {
    let malformed = |what: &str| ReadError::Malformed { format: FORMAT, what: what.to_string() };
    let mut vhdr = None;
    let mut body = None;
    // Chunks follow the 12-byte FORM header; the walk stops at the end of the file.
    let mut at = 12usize;
    while let (Some(id), Some(len)) = (bytes.get(at..at + 4), be32(bytes, at + 4)) {
        let start = at + 8;
        let declared = len as usize;
        let end = start.saturating_add(declared).min(bytes.len());
        match id {
            b"VHDR" => vhdr = Some(&bytes[start..end]),
            b"BODY" if body.is_none() => body = Some((start, end, declared - (end - start))),
            _ => {}
        }
        at = start.saturating_add(declared).saturating_add(declared & 1);
    }
    let vhdr = vhdr.ok_or_else(|| malformed("there is no VHDR chunk"))?;
    let (start, end, missing) = body.ok_or_else(|| malformed("there is no BODY chunk"))?;
    // VHDR: three 32-bit sample counts, the rate (16 bits), the octave count, the compression.
    let (Some(rate), Some(&compression)) = (vhdr.get(12..14), vhdr.get(15)) else {
        return Err(ReadError::Truncated { format: FORMAT });
    };
    let rate_hz = u32::from(u16::from_be_bytes([rate[0], rate[1]]));
    if compression != 0 {
        return Err(ReadError::Unsupported { format: FORMAT, what: format!("compression {compression}") });
    }
    if rate_hz == 0 {
        return Err(malformed("the sample rate is 0"));
    }
    let mut sound = Sound::new(FORMAT, rate_hz, 1, Encoding::I8, Cow::Borrowed(&bytes[start..end]));
    sound.missing_bytes = missing as u64;
    Ok(sound)
}
