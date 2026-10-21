// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Proof: how far a block's bytes are shown to be what the tape holds, as one ranked ladder.
//!
//! A Spectrum loads a block by its checksum and nothing else, and the checksum is the XOR of
//! every byte: a chance of one in 256 at any length, blind to an even number of errors in one
//! bit position (a reading half a bit out flips the same bit in many bytes and keeps it often),
//! and clean over one value repeated any even number of times. So a checksum alone is weak
//! proof, a header's fields with its checksum strong, and a header answered by a block of its
//! length strongest. Every place in the crate that judges a reading asks here, and two readings
//! that both check out are decided by rank, never by which came first.

use tapeti_core::types::{Block, Body};

/// A ROM header's length, flag and checksum included.
pub const HEADER_LEN: usize = 19;

/// The shortest block under a flag other than `0xFF` whose checksum alone is taken as proof.
///
/// The chance-valid junk found inside correctly read tapes runs from 4 to 24 bytes; a custom
/// loader's blocks run to thousands.
const OTHER_FLAG_LEAST: usize = 64;

/// The Spectrum's printable characters, which a header's name is made of.
const PRINTABLE: std::ops::RangeInclusive<u8> = 0x20..=0x7F;

/// What a block's bytes prove, from nothing up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Proof {
    /// The checksum does not hold, or there is less than a flag and a checksum.
    None,
    /// The checksum holds over bytes that a misread leaves as readily as a tape: two bytes, one
    /// value over and over, or a header's 19 bytes under flag `0x00` whose fields prove none.
    Trivial,
    /// The checksum holds over three bytes or more of more than one value: one chance in 256.
    Checksum,
    /// A checksum good at a length and under a flag past the junk that checks out by chance:
    /// three bytes under the ROM's data flag `0xFF`, 64 (`OTHER_FLAG_LEAST`) under any other
    /// but the header's, under which a long block that checks out is a misread of something
    /// else.
    Verified,
    /// As long as the header in front asked, its checksum good ([`answering`]).
    Answer,
    /// A header its fields prove: flag `0x00`, a type the ROM saves, a printable name, and a
    /// checksum good over them. A reading a bit out is none: its spaces become `0x40` and its
    /// characters drag their neighbours' top bits in.
    Header,
}

/// Whether the bytes XOR to zero, the ROM's own test and nothing more.
pub fn xor_ok(data: &[u8]) -> bool {
    data.iter().fold(0, |x, &d| x ^ d) == 0
}

/// The length, flag and checksum included, that 19 bytes under flag `0x00` declare for the block
/// after them, checksum or not: a block shaped like a header.
pub fn header_shaped(data: &[u8]) -> Option<usize> {
    (data.len() == HEADER_LEN && data[0] == 0x00)
        .then(|| usize::from(u16::from_le_bytes([data[12], data[13]])) + 2)
}

/// The length, flag and checksum included, that a ROM header in `data` asks the next block for.
///
/// What the ROM obeys: 19 bytes, flag `0x00`, a type it saves (0 to 3; it looks past any
/// other), and a checksum that holds, over more than one value behind the flag: a flag and 18
/// copies of one value fold clean whatever it is (17 copies of `v` XOR to `v`, the checksum's
/// own value), so nineteen zeros ask for nothing. A name is not asked to be printable here, as
/// the ROM does not ask it: this is what the tape makes the ROM do, [`Proof::Header`] how far
/// it is trusted.
pub fn asks(data: &[u8]) -> Option<usize> {
    let len = header_shaped(data)?;
    (data[1] <= 3 && data[2..].iter().any(|&b| b != data[1]) && xor_ok(data)).then_some(len)
}

/// What `data` proves on its own, with no header in front to answer.
pub fn of(data: &[u8]) -> Proof {
    if data.len() < 2 || !xor_ok(data) {
        return Proof::None;
    }
    if header_shaped(data).is_some() {
        let header = asks(data).is_some() && data[2..12].iter().all(|c| PRINTABLE.contains(c));
        return if header { Proof::Header } else { Proof::Trivial };
    }
    if data.len() < 3 || data.windows(2).all(|w| w[0] == w[1]) {
        return Proof::Trivial;
    }
    let past_chance = match data[0] {
        0xFF => true,
        0x00 => false,
        _ => data.len() >= OTHER_FLAG_LEAST,
    };
    if past_chance {
        Proof::Verified
    } else {
        Proof::Checksum
    }
}

/// What `data` proves as the block a header in front asked `asked` bytes for: an answer where it
/// is that long and its checksum holds over more than one value, what it proves alone otherwise.
pub fn answering(data: &[u8], asked: usize) -> Proof {
    let own = of(data);
    if data.len() == asked && own >= Proof::Checksum {
        Proof::Answer
    } else {
        own
    }
}

/// The bytes of a block of data, if it is one.
pub fn image(b: &Block) -> Option<&[u8]> {
    match &b.body {
        Body::Standard { data, .. } => Some(data),
        _ => None,
    }
}

/// What `data` proves between `front` and `behind`, the nearest blocks of data either side.
///
/// As the answer to `front` where that is a ROM header ([`answering`]); and as a ROM header
/// that `behind` answers, an answer at least, whatever its name: a header and the block of
/// its length behind it prove each other, as a pair the ROM would load.
pub fn between(front: Option<&[u8]>, data: &[u8], behind: Option<&[u8]>) -> Proof {
    let own = match front.and_then(asks) {
        Some(n) => answering(data, n),
        None => of(data),
    };
    let answered = asks(data).zip(behind).is_some_and(|(n, b)| answering(b, n) == Proof::Answer);
    if answered {
        own.max(Proof::Answer)
    } else {
        own
    }
}

/// What block `i` of `blocks` proves, the header in front and the block behind counted
/// ([`between`]).
///
/// Either is the nearest block of data that way; pauses and raw signal between do not count.
/// Nothing for a block that is no data.
pub fn at(blocks: &[Block], i: usize) -> Proof {
    let Some(data) = image(&blocks[i]) else { return Proof::None };
    between(blocks[..i].iter().rev().find_map(image), data, blocks[i + 1..].iter().find_map(image))
}
