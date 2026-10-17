// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Creative Voice File: sound in typed blocks.
//!
//! The header ends with the offset of the first block. A block is a type byte, then (except
//! for the terminator, type 0) a 24-bit length and the body. Read are sound data (1), its
//! continuation (2), silence (3), the extended header (8) that sets the rate of the sound
//! block after it, and version 1.20's sound data with its own header (9). Types 1 and 8 hold
//! 8-bit unsigned mono; type 9 also 16-bit signed and stereo. Markers, text and repeat loops (4
//! to 7) are passed over and listed; a repeat is not played. The rest is refused, as is a
//! change of rate or of sample format between sound blocks.

use super::{le16, Channel, Encoding, ReadError, Sound, MAX_SECONDS};
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

/// What a sound block holds: its rate, how a sample is stored, and how many channels.
type Layout = (u32, Encoding, u16);

enum Part<'a> {
    Sound(&'a [u8]),
    /// A number of samples of silence at the given rate.
    Silence(usize, u32),
}

pub(super) fn read(bytes: &[u8], channel: Channel) -> Result<Sound<'static>, ReadError> {
    let truncated = ReadError::Truncated { format: FORMAT };
    let unsupported = |what: String| ReadError::Unsupported { format: FORMAT, what };
    let mut at = usize::from(le16(bytes, 0x14).ok_or(truncated.clone())?);
    let mut layout: Option<Layout> = None;
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
                same_layout(&mut layout, (r, Encoding::U8, 1), at)?;
                parts.push(Part::Sound(pcm));
            }
            2 if layout.is_none() => {
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
            9 => {
                let header = body.get(..12).ok_or(truncated.clone())?;
                let rate = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
                let (bits, channels) = (header[4], u16::from(header[5]));
                let codec = u16::from_le_bytes([header[6], header[7]]);
                let encoding = match (codec, bits) {
                    (0, 8) => Encoding::U8,
                    (4, 16) => Encoding::I16,
                    _ => return Err(unsupported(format!("codec {codec} of {bits} bits at byte {at}"))),
                };
                if !(1..=2).contains(&channels) {
                    return Err(unsupported(format!("{channels} channels at byte {at}")));
                }
                if rate == 0 {
                    let what = format!("a sample rate of 0 at byte {at}");
                    return Err(ReadError::Malformed { format: FORMAT, what });
                }
                same_layout(&mut layout, (rate, encoding, channels), at)?;
                parts.push(Part::Sound(&body[12..]));
            }
            other => return Err(unsupported(format!("block type {other} at byte {at}"))),
        }
        at += 4 + len;
    }
    let (rate_hz, encoding, channels) =
        layout.ok_or_else(|| ReadError::Malformed { format: FORMAT, what: "there is no sound".into() })?;
    let frame = encoding.width() * usize::from(channels);
    // Silence keeps its own rate's duration, at the sound's rate. Seven bytes of silence block
    // can stand for 65,536 samples, so the total is checked before anything is allocated.
    let silence = |n: usize, r: u32| (n as f64 * f64::from(rate_hz) / f64::from(r)).round() as usize;
    let total: f64 = parts
        .iter()
        .map(|p| match p {
            Part::Sound(b) => (b.len() / frame) as f64,
            Part::Silence(n, r) => silence(*n, *r) as f64,
        })
        .sum();
    if total > f64::from(rate_hz) * f64::from(MAX_SECONDS) {
        let what = format!("its blocks add up to more than {} hours of sound", MAX_SECONDS / 3600);
        return Err(ReadError::Malformed { format: FORMAT, what });
    }
    // Unsigned samples are silent at 128, signed ones at 0.
    let quiet = if encoding == Encoding::U8 { 128 } else { 0 };
    let mut pcm = Vec::with_capacity(total as usize * frame);
    for part in parts {
        match part {
            // A frame cut short at the end of a block would move every sample after it.
            Part::Sound(b) => pcm.extend_from_slice(&b[..b.len() / frame * frame]),
            Part::Silence(n, r) => pcm.resize(pcm.len() + silence(n, r) * frame, quiet),
        }
    }
    let mut sound = Sound::new(FORMAT, rate_hz, channels, encoding, Cow::Owned(pcm));
    sound.skipped = skipped;
    sound.channel = channel;
    Ok(sound)
}

/// Records the layout of the sound block at byte `at`, or refuses it where an earlier one had
/// another: the samples are one stream, at one rate and in one format.
fn same_layout(layout: &mut Option<Layout>, this: Layout, at: usize) -> Result<(), ReadError> {
    let unsupported = |what: String| ReadError::Unsupported { format: FORMAT, what };
    match *layout {
        Some((rate, ..)) if rate != this.0 => {
            Err(unsupported(format!("a change of sample rate at byte {at}")))
        }
        Some(prev) if prev != this => Err(unsupported(format!("a change of sample format at byte {at}"))),
        _ => {
            *layout = Some(this);
            Ok(())
        }
    }
}
