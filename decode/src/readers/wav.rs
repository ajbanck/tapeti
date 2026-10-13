// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! WAV: a RIFF file with a `fmt ` chunk and a `data` chunk. PCM of 8, 16 or 24 bits and 32-bit
//! float, mono or stereo, in the plain or the extensible format header.

use super::{le16, le32, Channel, Encoding, ReadError, Sound};
use std::borrow::Cow;

const FORMAT: &str = "WAV";
const PCM: u16 = 1;
const FLOAT: u16 = 3;
/// `WAVE_FORMAT_EXTENSIBLE`: the real format is the first two bytes of the sub-format GUID.
const EXTENSIBLE: u16 = 0xFFFE;

pub(super) fn read(bytes: &[u8], channel: Channel) -> Result<Sound<'_>, ReadError> {
    let malformed = |what: &str| ReadError::Malformed { format: FORMAT, what: what.to_string() };
    let unsupported = |what: String| ReadError::Unsupported { format: FORMAT, what };
    let mut fmt = None;
    let mut data = None;
    // Chunks follow the 12-byte RIFF header, each an id, a 32-bit length and a body padded to
    // an even length. The walk stops at the end of the file, whatever the RIFF length says.
    let mut at = 12usize;
    while let (Some(id), Some(len)) = (bytes.get(at..at + 4), le32(bytes, at + 4)) {
        let body = at + 8;
        let declared = len as usize;
        let end = body.saturating_add(declared).min(bytes.len());
        match id {
            b"fmt " if fmt.is_none() => fmt = Some(&bytes[body..end]),
            b"data" if data.is_none() => data = Some((body, end, declared - (end - body))),
            _ => {}
        }
        at = body.saturating_add(declared).saturating_add(declared & 1);
    }
    let fmt = fmt.ok_or_else(|| malformed("there is no fmt chunk"))?;
    let (start, end, missing) = data.ok_or_else(|| malformed("there is no data chunk"))?;
    if fmt.len() < 16 {
        return Err(ReadError::Truncated { format: FORMAT });
    }
    let mut tag = le16(fmt, 0).unwrap_or_default();
    let channels = le16(fmt, 2).unwrap_or_default();
    let rate_hz = le32(fmt, 4).unwrap_or_default();
    let bits = le16(fmt, 14).unwrap_or_default();
    if tag == EXTENSIBLE {
        // The sub-format GUID sits at byte 24 of the extended header.
        tag = le16(fmt, 24).ok_or(ReadError::Truncated { format: FORMAT })?;
    }
    let encoding = match (tag, bits) {
        (PCM, 8) => Encoding::U8,
        (PCM, 16) => Encoding::I16,
        (PCM, 24) => Encoding::I24,
        (FLOAT, 32) => Encoding::F32,
        (PCM, b) => return Err(unsupported(format!("{b}-bit PCM"))),
        (FLOAT, b) => return Err(unsupported(format!("{b}-bit float"))),
        (t, _) => return Err(unsupported(format!("compression type {t}"))),
    };
    if !(1..=2).contains(&channels) {
        return Err(unsupported(format!("{channels} channels")));
    }
    if rate_hz == 0 {
        return Err(malformed("the sample rate is 0"));
    }
    let mut sound = Sound::new(FORMAT, rate_hz, channels, encoding, Cow::Borrowed(&bytes[start..end]));
    sound.missing_bytes = missing as u64;
    sound.channel = channel;
    Ok(sound)
}
