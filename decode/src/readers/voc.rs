// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Creative Voice File: 8-bit unsigned mono sound in typed blocks.
//!
//! The header ends with the offset of the first block. A block is a type byte, then (except
//! for the terminator, type 0) a 24-bit length and the body. Read are sound data (1), its
//! continuation (2), silence (3) and the extended header (8) that sets the rate of the sound
//! block after it. Markers, text and repeat loops (4 to 7) are passed over and listed; a repeat
//! is not played. The rest is refused, as is a rate change between sound blocks.

use super::{le16, Encoding, ReadError, Sound, MAX_SECONDS};
use std::borrow::Cow;

pub(super) const SIGNATURE: &[u8] = b"Creative Voice File\x1a";
const FORMAT: &str = "VOC";

/// The rate a type 1 or type 3 block's divisor stands for: `1,000,000 / (256 - divisor)`.
fn rate_from_divisor(divisor: u8) -> u32 {
    (1_000_000.0 / f64::from(256 - u32::from(divisor))).round() as u32
}

/// The rate a type 8 block's 16-bit time constant stands for, mono:
/// `256,000,000 / (65,536 - constant)`.
fn rate_from_time_constant(constant: u16) -> u32 {
    (256_000_000.0 / f64::from(65_536 - u32::from(constant))).round() as u32
}

enum Part<'a> {
    Sound(&'a [u8]),
    /// A number of samples of silence at the given rate.
    Silence(usize, u32),
}

pub(super) fn read(bytes: &[u8]) -> Result<Sound<'static>, ReadError> {
    let truncated = ReadError::Truncated { format: FORMAT };
    let unsupported = |what: String| ReadError::Unsupported { format: FORMAT, what };
    let mut at = usize::from(le16(bytes, 0x14).ok_or(truncated.clone())?);
    let mut rate: Option<u32> = None;
    let mut extended_rate: Option<u32> = None;
    let mut parts = Vec::new();
    let mut skipped = Vec::new();
    while let Some(&kind) = bytes.get(at) {
        if kind == 0 {
            break;
        }
        let head = bytes.get(at + 1..at + 4).ok_or(truncated.clone())?;
        let len = usize::from(head[0]) | usize::from(head[1]) << 8 | usize::from(head[2]) << 16;
        let body = bytes.get(at + 4..at + 4 + len).ok_or(truncated.clone())?;
        match kind {
            1 => {
                let [divisor, codec, ref pcm @ ..] = *body else { return Err(truncated) };
                if codec != 0 {
                    return Err(unsupported(format!("codec {codec} at byte {at}")));
                }
                // An extended block before this one gives the rate the 8-bit divisor cannot.
                let r = extended_rate.take().unwrap_or_else(|| rate_from_divisor(divisor));
                if rate.is_some_and(|prev| prev != r) {
                    return Err(unsupported(format!("a change of sample rate at byte {at}")));
                }
                rate = Some(r);
                parts.push(Part::Sound(pcm));
            }
            2 if rate.is_none() => {
                let what = format!("a continuation block at byte {at} continues nothing");
                return Err(ReadError::Malformed { format: FORMAT, what });
            }
            2 => parts.push(Part::Sound(body)),
            3 => {
                let [lo, hi, divisor, ..] = *body else { return Err(truncated) };
                let samples = usize::from(u16::from_le_bytes([lo, hi])) + 1;
                parts.push(Part::Silence(samples, rate_from_divisor(divisor)));
            }
            4..=7 => skipped.push(format!("block type {kind} at byte {at}")),
            8 => {
                let [lo, hi, pack, mode, ..] = *body else { return Err(truncated) };
                if pack != 0 {
                    return Err(unsupported(format!("packing method {pack} at byte {at}")));
                }
                if mode != 0 {
                    return Err(unsupported(format!("stereo sound at byte {at}")));
                }
                extended_rate = Some(rate_from_time_constant(u16::from_le_bytes([lo, hi])));
            }
            other => return Err(unsupported(format!("block type {other} at byte {at}"))),
        }
        at += 4 + len;
    }
    let rate_hz =
        rate.ok_or_else(|| ReadError::Malformed { format: FORMAT, what: "there is no sound".into() })?;
    // Silence keeps its own rate's duration, at the sound's rate. Seven bytes of silence block
    // can stand for 65,536 samples, so the total is checked before anything is allocated.
    let silence = |n: usize, r: u32| (n as f64 * f64::from(rate_hz) / f64::from(r)).round() as usize;
    let total: f64 = parts
        .iter()
        .map(|p| match p {
            Part::Sound(b) => b.len() as f64,
            Part::Silence(n, r) => silence(*n, *r) as f64,
        })
        .sum();
    if total > f64::from(rate_hz) * f64::from(MAX_SECONDS) {
        let what = format!("its blocks add up to more than {} hours of sound", MAX_SECONDS / 3600);
        return Err(ReadError::Malformed { format: FORMAT, what });
    }
    let mut pcm = Vec::with_capacity(total as usize);
    for part in parts {
        match part {
            Part::Sound(b) => pcm.extend_from_slice(b),
            Part::Silence(n, r) => pcm.resize(pcm.len() + silence(n, r), 128),
        }
    }
    let mut sound = Sound::new(FORMAT, rate_hz, 1, Encoding::U8, Cow::Owned(pcm));
    sound.skipped = skipped;
    Ok(sound)
}
