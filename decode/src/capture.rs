// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! A signal kept as it is, in Direct Recording (TZX 0x15) or CSW (0x18) blocks.
//!
//! Nothing is interpreted, so whatever loader made the signal can read it back. Sampled audio is
//! one bit per sample in Direct Recording blocks, spaced at the recording's own rate: 3,500,000
//! T-states a second divided by it, rounded, which plays it at most `0.5 / spacing` fast or slow
//! (0.7% at 48 kHz). A block holds at most 16,777,215 bytes, about 50 minutes at 44.1 kHz; a
//! longer signal continues in the next block with no pause between them. Pulse lengths, from a
//! CSW file or an OUT trace, go into one CSW block, which holds them exactly.

use tapeti_core::audio::{decode_csw_rle, TSTATES_PER_SEC};
use tapeti_core::types::{Block, Body};

/// The most data bytes a Direct Recording block's 24-bit length can count.
pub const MAX_BLOCK_BYTES: usize = 0xFF_FFFF;

/// The T-states between two bits for a recording at `rate_hz`, or `None` for a rate too low for
/// the block's 16-bit field.
pub fn spacing(rate_hz: u32) -> Option<u16> {
    let t = (f64::from(TSTATES_PER_SEC) / f64::from(rate_hz)).round();
    (1.0..=f64::from(u16::MAX)).contains(&t).then_some(t as u16)
}

/// Direct Recording blocks holding `levels`, one per sample, true for high. `max_bytes` is where a
/// block ends and the next begins: [`MAX_BLOCK_BYTES`] but for tests.
pub fn direct_recording(
    spacing: u16,
    levels: impl IntoIterator<Item = bool>,
    max_bytes: usize,
) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    let mut bits = 0usize;
    let finish = |data: Vec<u8>, bits: usize| {
        // The last byte's used bits: 8 when the bits fill it.
        let used_bits = match bits % 8 {
            0 => 8,
            n => n as u8,
        };
        Block::new(Body::Direct { tstates: spacing, pause: 0, used_bits, data })
    };
    for high in levels {
        if bits.is_multiple_of(8) {
            if data.len() == max_bytes {
                blocks.push(finish(std::mem::take(&mut data), bits));
                bits = 0;
            }
            data.push(0);
        }
        if high {
            *data.last_mut().expect("a byte was pushed") |= 0x80 >> (bits % 8);
        }
        bits += 1;
    }
    if !data.is_empty() {
        blocks.push(finish(data, bits));
    }
    blocks
}

/// The fastest rate a CSW block's 24-bit field can hold.
pub const MAX_CSW_RATE: u32 = 0xFF_FFFF;

/// A CSW block holding a CSW file's data as it is, compressed or not. `pulse_count` is the
/// file's own count; a version 1 file has none, and its run lengths are counted.
///
/// A CSW block has no starting level: its first pulse continues the level the previous block
/// left, whatever the file's polarity flag said.
pub fn csw_block(rate_hz: u32, compression: u8, pulse_count: Option<u32>, data: &[u8]) -> Block {
    let pulse_count = pulse_count.unwrap_or_else(|| decode_csw_rle(data).len() as u32);
    Block::new(Body::Csw { pause: 0, sample_rate: rate_hz, compression, pulse_count, data: data.to_vec() })
}

/// A CSW block holding pulse lengths in T-states, at 3,500,000 samples a second so each is exact.
///
/// A pulse of no length is two edges at once: left out, it leaves the pulses either side at one
/// level, and they join.
pub fn csw_of_tstates(lengths: &[u64]) -> Block {
    // Pulses alternate between two levels; `level` is the parity of each one's index.
    let mut joined: Vec<u64> = Vec::with_capacity(lengths.len());
    let mut last_level = None;
    for (i, &t) in lengths.iter().enumerate() {
        if t == 0 {
            continue;
        }
        let level = i % 2;
        match joined.last_mut() {
            Some(last) if last_level == Some(level) => *last += t,
            _ => joined.push(t),
        }
        last_level = Some(level);
    }
    let mut data = Vec::with_capacity(joined.len());
    for &t in &joined {
        // A run of 1 to 255 is one byte; a longer one is a 0 and its 32-bit length. A pulse over
        // 2^32 T-states (20 minutes) is held as the longest a run can say.
        match u8::try_from(t) {
            Ok(n) => data.push(n),
            Err(_) => {
                data.push(0);
                data.extend((t.min(u64::from(u32::MAX)) as u32).to_le_bytes());
            }
        }
    }
    let pulse_count = joined.len() as u32;
    Block::new(Body::Csw { pause: 0, sample_rate: TSTATES_PER_SEC, compression: 1, pulse_count, data })
}

/// The size of a TZX file holding `samples` bits in Direct Recording blocks: the 10-byte file
/// header, and per block its ID and 8-byte header.
pub fn tzx_size(samples: usize) -> usize {
    let bytes = samples.div_ceil(8);
    10 + bytes + 9 * bytes.div_ceil(MAX_BLOCK_BYTES).max(1)
}
