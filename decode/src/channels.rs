// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The channel of a stereo recording, chosen by decoding each.
//!
//! A real stereo capture's channels are not copies of one another: the two heads sit a few
//! samples apart, and the gap drifts along the tape. A sync half-wave lasts about eight samples
//! at 44.1 kHz, so mixed, the channels can cancel the short half-waves at places and lose
//! blocks that either channel alone reads. No statistic of the signal tells beforehand which
//! channel reads best, so the mix, the left and the right channel are each decoded, and the
//! decodes are judged by what they deliver.
//!
//! A Spectrum loads a block by its checksum, and nothing else on a tape is proved, so a decode
//! is judged by its verified bytes, never by how many blocks it counts. The shape score of the
//! decodes names the incumbent, ties going to the mix, so a recording whose channels agree
//! decodes exactly as its mix. The pick moves off the incumbent only to a decode that delivers
//! every verified image the incumbent does and more verified bytes; a program's header adds a
//! name, which is no loaded material.

use std::collections::BTreeSet;
use tapeti_core::types::{Block, Body};

/// A header's 19 bytes: flag `0x00`, type, name, length, two parameters, checksum.
const HEADER_LEN: usize = 19;

fn xor_ok(data: &[u8]) -> bool {
    data.iter().fold(0, |x, &d| x ^ d) == 0
}

fn image(b: &Block) -> Option<&[u8]> {
    match &b.body {
        Body::Standard { data, .. } => Some(data),
        _ => None,
    }
}

/// The length, flag and checksum included, that a block shaped like a header declares for the
/// block after it, its checksum good or not.
fn declares(data: &[u8]) -> Option<usize> {
    (data.len() == HEADER_LEN && data[0] == 0x00)
        .then(|| usize::from(u16::from_le_bytes([data[12], data[13]])) + 2)
}

/// Whether the block at `i` is a header with a good checksum whose next block of bytes answers it:
/// as long as it declares, its checksum good. Pauses and raw signal between the two do not count.
fn heads_pair(blocks: &[Block], i: usize) -> bool {
    let Some(data) = image(&blocks[i]) else { return false };
    let Some(len) = declares(data).filter(|_| xor_ok(data)) else { return false };
    blocks[i + 1..].iter().find_map(image).is_some_and(|d| d.len() == len && xor_ok(d))
}

/// The shortest block under a flag other than `0xFF` that proves its bytes. Its checksum is a
/// chance of one in 256 at any length, and the chance-valid junk found inside correctly read
/// tapes runs from 4 to 24 bytes; a custom loader's blocks run to thousands.
const OTHER_FLAG_LEAST: usize = 64;

/// A block whose bytes prove themselves, its checksum good and not one byte over and over (a run
/// of one byte checks out at every even length): three bytes or more under flag `0xFF`,
/// [`OTHER_FLAG_LEAST`] under another, never under the header flag `0x00`, where a long block
/// that checks out is a misread of something else.
fn verified(b: &Block) -> Option<&[u8]> {
    let data = image(b)?;
    let long_enough = match data.first() {
        Some(0xFF) => data.len() >= 3,
        Some(0x00) | None => false,
        Some(_) => data.len() >= OTHER_FLAG_LEAST,
    };
    (long_enough && data.windows(2).any(|w| w[0] != w[1]) && xor_ok(data)).then_some(data)
}

/// The header that heads the pair at `i`, and the block answering it, when that block carries
/// payload.
fn program_at(blocks: &[Block], i: usize) -> Option<(&[u8], &Block)> {
    if !heads_pair(blocks, i) {
        return None;
    }
    let leg = blocks[i + 1..].iter().find(|b| image(b).is_some())?;
    let data = image(leg)?;
    let carries = data.len() >= 3 && data.windows(2).any(|w| w[0] != w[1]);
    carries.then(|| (image(&blocks[i]).expect("a pair's header has bytes"), leg))
}

/// What a decode delivers: the programs it proves and its verified bytes, each counted once.
///
/// A program is its header's bytes, counted once with the bytes of the block answering it, so
/// a second copy adds nothing. A verified block no header answers counts once for its bytes.
pub fn delivered(blocks: &[Block]) -> (usize, usize) {
    let mut programs: std::collections::BTreeMap<&[u8], usize> = std::collections::BTreeMap::new();
    let mut answered: BTreeSet<&[u8]> = BTreeSet::new();
    for i in 0..blocks.len() {
        if let Some((header, leg)) = program_at(blocks, i) {
            programs.entry(header).or_insert(verified(leg).map_or(0, <[u8]>::len));
            answered.extend(image(leg));
        }
    }
    let unanswered: BTreeSet<&[u8]> =
        blocks.iter().filter_map(verified).filter(|d| !answered.contains(d)).collect();
    (programs.len(), programs.values().sum::<usize>() + unanswered.iter().map(|d| d.len()).sum::<usize>())
}

/// Every image [`delivered`] counts: each verified block, and the header of each program.
fn images(blocks: &[Block]) -> BTreeSet<&[u8]> {
    let mut out: BTreeSet<&[u8]> = blocks.iter().filter_map(verified).collect();
    out.extend((0..blocks.len()).filter_map(|i| program_at(blocks, i)).map(|(header, _)| header));
    out
}

/// The shape of a decode: blocks that check out and headers that are answered, then payload.
///
/// The blocks are those of three bytes or more whose checksum is good, the headers those whose
/// next block is as long as they declare. A one-bit slip through
/// a whole block keeps its checksum good about half the time but spoils the length a header
/// declares, so the pairs tell a slipped decode from a true one where the checksums cannot.
pub fn shape(blocks: &[Block]) -> (usize, usize) {
    let (mut count, mut bytes, mut pairs) = (0, 0, 0);
    let mut pending = None;
    for data in blocks.iter().filter_map(image) {
        match declares(data) {
            Some(len) => pending = Some(len),
            None => {
                if pending.take() == Some(data.len()) {
                    pairs += 1;
                }
            }
        }
        if data.len() >= 3 && xor_ok(data) {
            count += 1;
            bytes += data.len() - 2;
        }
    }
    (count + pairs, bytes)
}

/// Which of `decodes`, in the order mix, left, right, to keep.
///
/// The shape names the incumbent, the earliest on a tie. The pick is, among the decodes that
/// deliver every image the incumbent does, the one with the most verified bytes, then the most
/// programs, then the incumbent, then the best shape, then the earliest.
pub fn pick(decodes: &[&[Block]]) -> usize {
    let n = decodes.len();
    let shapes: Vec<_> = decodes.iter().map(|d| shape(d)).collect();
    let incumbent = (0..n).max_by_key(|&i| (shapes[i], std::cmp::Reverse(i))).expect("a decode to pick from");
    let held = images(decodes[incumbent]);
    (0..n)
        .filter(|&i| held.is_subset(&images(decodes[i])))
        .max_by_key(|&i| {
            let (programs, bytes) = delivered(decodes[i]);
            (bytes, programs, i == incumbent, shapes[i], std::cmp::Reverse(i))
        })
        .expect("the incumbent delivers all of itself")
}
