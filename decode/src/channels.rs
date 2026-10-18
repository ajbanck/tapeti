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
//! name, which is no loaded material. Where the mix is kept, a block the two channels read
//! alike and the mix otherwise, every reading checking out, takes the channels' bytes
//! ([`outvote`]).

use crate::entry::{Decoded, Extent, Finding};
use crate::proof::{self, asks, header_shaped, image, Proof};
use crate::readers::Channel;
use std::collections::{BTreeSet, HashMap};
use tapeti_core::types::{Block, Body};

/// Whether `b` is what a failed read leaves: a block of data whose checksum is bad, or a stretch
/// kept as raw signal.
fn failed_piece(b: &Block) -> bool {
    match &b.body {
        Body::Standard { data, .. } => proof::of(data) == Proof::None,
        Body::Csw { .. } => true,
        _ => false,
    }
}

/// The block that answers the ROM header at `i` ([`Proof::Answer`]): its next block of bytes,
/// pauses and raw signal between the two not counting.
fn answered(blocks: &[Block], i: usize) -> Option<&Block> {
    let len = image(&blocks[i]).and_then(asks)?;
    let leg = blocks[i + 1..].iter().find(|b| image(b).is_some())?;
    (proof::answering(image(leg)?, len) == Proof::Answer).then_some(leg)
}

/// A block whose bytes alone prove themselves as loaded material ([`Proof::Verified`]). A
/// header is a name, no loaded material, and counts with the program it heads.
fn verified(b: &Block) -> Option<&[u8]> {
    image(b).filter(|d| proof::of(d) == Proof::Verified)
}

/// The header that heads the pair at `i`, and the block answering it.
fn program_at(blocks: &[Block], i: usize) -> Option<(&[u8], &Block)> {
    let leg = answered(blocks, i)?;
    Some((image(&blocks[i]).expect("a pair's header has bytes"), leg))
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
/// The blocks are those whose checksum holds over more than a misread leaves
/// ([`Proof::Checksum`] or better), the headers those shaped like one whose next block is as
/// long as they declare, checksum or not. A one-bit slip through a whole block keeps its
/// checksum good about half the time but spoils the length a header declares, so the pairs tell
/// a slipped decode from a true one where the checksums cannot. A channel that reads pilot
/// tones and headers as runs of 0xFF cuts them into blocks that check out; counted, they would
/// name it the incumbent over a decode that reads every block whole.
pub fn shape(blocks: &[Block]) -> (usize, usize) {
    let (mut count, mut bytes, mut pairs) = (0, 0, 0);
    let mut pending = None;
    for data in blocks.iter().filter_map(image) {
        match header_shaped(data) {
            Some(len) => pending = Some(len),
            None => {
                if pending.take() == Some(data.len()) {
                    pairs += 1;
                }
            }
        }
        if proof::of(data) >= Proof::Checksum {
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

/// The length a ROM header in `data` asks for ([`asks`]): what anchors a block of one decode to
/// the same block of another.
fn anchor(data: &[u8]) -> Option<usize> {
    asks(data)
}

/// Whether `data` answers a header that asks for `len` bytes ([`Proof::Answer`]).
fn answers(data: &[u8], len: usize) -> bool {
    proof::answering(data, len) == Proof::Answer
}

/// Puts into `base`, the mix's decode, the bytes the left and the right channel both read where
/// the mix read other bytes, each checking out.
///
/// The checksum is the XOR of every byte, so a read that errs in the same bit of two bytes
/// holds it; and the mix is the likelier to err so, two channels a few samples apart smearing
/// every change between a short half-wave and a long one. The channels are two heads' reads of
/// the tape, and for both to err alike, in the same bits, where their mix reads otherwise, is
/// far less likely than for the mix alone to err. So where each channel reads a block whose
/// extent overlaps a block of `base`, all three as long, their checksums good and not one byte
/// over and over, and the two channels read the same bytes and `base` others, theirs stand, at
/// `base`'s place and pause ([`Finding::BothChannels`]).
pub fn outvote(base: &mut Decoded, left: &Decoded, right: &Decoded) {
    let whole = |data: &[u8]| proof::of(data) >= Proof::Checksum;
    let read_at = |d: &Decoded, at: &Extent| -> Option<Vec<u8>> {
        let j = (0..d.blocks.len()).find(|&j| image(&d.blocks[j]).is_some() && overlap(&d.extents[j], at))?;
        image(&d.blocks[j]).filter(|data| whole(data)).map(<[u8]>::to_vec)
    };
    for i in 0..base.blocks.len() {
        let at = base.extents[i];
        let Body::Standard { data, .. } = &mut base.blocks[i].body else { continue };
        if !whole(data) {
            continue;
        }
        let (Some(l), Some(r)) = (read_at(left, &at), read_at(right, &at)) else { continue };
        if l == r && l.len() == data.len() && l != *data {
            *data = l;
            base.findings.push(Finding::BothChannels { block: i });
        }
    }
}

/// Puts into `base` a donor's ordinary read of each block `base` read on the fade band.
///
/// The donor's block is the same length at an overlapping extent, both checksums good, the
/// bytes different, and no fade band read itself ([`Finding::FadeBand`]).
///
/// The fade band is a last resort: it reads only a block the ordinary band could not, and its
/// reading stands on the length asked for and the checksum alone, which a read with two
/// flipped bits in one column passes. A donor's ordinary read of the same place is the answer
/// the fade band stood in for, so the last resort gives way to it; donors are asked in order,
/// and the block keeps `base`'s place and pause ([`Finding::FromChannel`]).
pub fn faded_legs(base: &mut Decoded, donors: &[(Channel, &Decoded)]) {
    let faded = |d: &Decoded, i: usize| d.findings.contains(&Finding::FadeBand { block: i });
    for i in 0..base.blocks.len() {
        let Some(read) = image(&base.blocks[i]).filter(|d| proof::of(d) >= Proof::Checksum && faded(base, i))
        else {
            continue;
        };
        let ordinary = donors.iter().find_map(|&(channel, d)| {
            (0..d.blocks.len()).find_map(|j| {
                let other = image(&d.blocks[j])?;
                (other.len() == read.len()
                    && other != read
                    && proof::of(other) >= Proof::Checksum
                    && !faded(d, j)
                    && overlap(&d.extents[j], &base.extents[i]))
                .then(|| (channel, other.to_vec()))
            })
        });
        let Some((channel, bytes)) = ordinary else { continue };
        if let Body::Standard { data, .. } = &mut base.blocks[i].body {
            *data = bytes;
        }
        base.findings.retain(|f| *f != Finding::FadeBand { block: i });
        base.findings.push(Finding::FromChannel { block: i, channel });
    }
}

/// Whether two extents of one recording overlap.
fn overlap(a: &Extent, b: &Extent) -> bool {
    a.start < b.end && b.start < a.end
}

/// The data-bearing blocks of `blocks`, by index, in tape order.
fn data_view(blocks: &[Block]) -> Vec<(usize, &[u8])> {
    blocks.iter().enumerate().filter_map(|(i, b)| image(b).map(|d| (i, d))).collect()
}

/// Per header, each time a decode reads it: where, and the block that answers it where the pair
/// is whole.
type Occurrences<'a> = HashMap<&'a [u8], Vec<(usize, Option<(usize, &'a [u8])>)>>;

/// What goes where in the decode kept.
enum Splice {
    /// The blocks in the range, a failed read of a leg in pieces, become one with these bytes and
    /// this pause.
    Replace { range: std::ops::Range<usize>, data: Vec<u8>, pause: u16 },
    /// The block goes in behind the header at the index, at the donor's extent.
    Insert { header: usize, block: Block, extent: Extent },
    /// The checksum-bad pieces in the range, between a header and its own read of the leg, go.
    Drop(std::ops::Range<usize>),
}

/// Folds into `base`, the decode kept, the data blocks each of `donors` reads where `base`'s
/// read failed.
///
/// A header and its block from another channel stand where `base` reads the same header and no
/// block that answers it.
/// The channels of a stereo recording lose different blocks, so no one decode may hold every
/// program, and a block clean in another is the tape's own, under proof:
/// - the anchor is the header itself, byte for byte, checksum good, and both decodes must read
///   it as often, so that the n-th time is the same program in both; where the counts differ
///   nothing is guessed. Both reads of the header must overlap in the recording;
/// - the donor's pair must be whole: a header its fields prove ([`Proof::Header`]), and a block
///   behind that answers it ([`Proof::Answer`]). A block read a bit out cannot be either;
/// - `base`'s pair must have failed, its next block of data not the length declared, its
///   checksum bad, or one byte over and over. The failed block is replaced where it lies at the
///   donor's block, with every checksum-bad piece and every stretch kept as raw signal behind it
///   that starts inside the donor's block (each a failed read of the same signal), and keeps
///   `base`'s pause and extent, the donor's pause where the last piece is raw signal, which
///   played through the pause `base` never heard; with none, or where it is shaped like a header
///   (the next header misread, which a replacement would take off the tape), the donor's block
///   goes in behind the header, with its own.
///   Where `base` reads the donor's block itself behind pieces that are no block, the pieces go;
///   where it holds that block anywhere else at the same place, nothing is done.
///
/// Donors are asked in order, the first fix standing. Each block placed is reported
/// ([`Finding::FromChannel`]).
pub fn combine(base: &mut Decoded, donors: &[(Channel, &Decoded)]) {
    for &(channel, donor) in donors {
        let mut plan: Vec<(usize, Splice)> = Vec::new();
        {
            let donor_view = data_view(&donor.blocks);
            let mut donor_occ: Occurrences = HashMap::new();
            for (k, &(hi, d)) in donor_view.iter().enumerate() {
                let Some(len) = anchor(d) else { continue };
                let next = donor_view.get(k + 1).copied();
                let whole = proof::of(d) == Proof::Header && next.is_some_and(|(_, l)| answers(l, len));
                donor_occ.entry(d).or_default().push((hi, if whole { next } else { None }));
            }
            let base_view = data_view(&base.blocks);
            let mut counts: HashMap<&[u8], usize> = HashMap::new();
            for &(_, d) in &base_view {
                if anchor(d).is_some() {
                    *counts.entry(d).or_default() += 1;
                }
            }
            let mut seen: HashMap<&[u8], usize> = HashMap::new();
            for (k, &(hi, header)) in base_view.iter().enumerate() {
                let Some(len) = anchor(header) else { continue };
                let n = {
                    let c = seen.entry(header).or_default();
                    *c += 1;
                    *c - 1
                };
                let next = base_view.get(k + 1).copied();
                if next.is_some_and(|(_, l)| answers(l, len)) {
                    continue;
                }
                let Some(list) = donor_occ.get(header).filter(|l| l.len() == counts[header]) else {
                    continue;
                };
                let Some(&(donor_hi, Some((li, leg)))) = list.get(n) else { continue };
                if !overlap(&base.extents[hi], &donor.extents[donor_hi]) {
                    continue;
                }
                let leg_at = donor.extents[li];
                let own = own_leg_behind_strays(base, hi, leg, &leg_at);
                if own.is_none()
                    && (hi + 1..base.blocks.len())
                        .any(|j| image(&base.blocks[j]) == Some(leg) && overlap(&base.extents[j], &leg_at))
                {
                    continue;
                }
                let follower = next
                    .filter(|&(fi, d)| header_shaped(d).is_none() && overlap(&base.extents[fi], &leg_at))
                    .map(|(fi, _)| fi);
                let splice = match (own, follower) {
                    (Some(li), _) => (hi + 1, Splice::Drop(hi + 1..li)),
                    (None, Some(fi)) => {
                        let mut end = fi + 1;
                        while base.blocks.get(end).is_some_and(failed_piece)
                            && leg_at.start <= base.extents[end].start
                            && base.extents[end].start < leg_at.end
                        {
                            end += 1;
                        }
                        let pause = match (&base.blocks[end - 1].body, &donor.blocks[li].body) {
                            (Body::Csw { .. }, Body::Standard { pause, .. }) => *pause,
                            (Body::Standard { pause, .. } | Body::Csw { pause, .. }, _) => *pause,
                            _ => unreachable!("a piece of a read is a block of data or raw signal"),
                        };
                        (fi, Splice::Replace { range: fi..end, data: leg.to_vec(), pause })
                    }
                    (None, None) => {
                        let pause = match donor.blocks[li].body {
                            Body::Standard { pause, .. } => pause,
                            _ => unreachable!("a leg is a block of data"),
                        };
                        let block = Block::new(Body::Standard { pause, data: leg.to_vec() });
                        (hi, Splice::Insert { header: hi, block, extent: leg_at })
                    }
                };
                plan.push(splice);
            }
        }
        // Back to front, so that a splice never moves the place of one still to come.
        plan.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        for (_, splice) in plan {
            match splice {
                Splice::Replace { range, data, pause } => {
                    let last = range.end - 1;
                    let extent = Extent {
                        start: base.extents[range.start].start,
                        end: base.extents[last].end,
                        signal_end: base.extents[last].signal_end,
                    };
                    let at = range.start;
                    splice_into(base, range, vec![(Block::new(Body::Standard { pause, data }), extent)]);
                    base.findings.push(Finding::FromChannel { block: at, channel });
                }
                Splice::Insert { header, block, extent } => {
                    end_pause_at(base, header, extent.start);
                    splice_into(base, header + 1..header + 1, vec![(block, extent)]);
                    base.findings.push(Finding::FromChannel { block: header + 1, channel });
                }
                Splice::Drop(range) => {
                    let at = range.start;
                    splice_into(base, range, Vec::new());
                    base.findings.push(Finding::FromChannel { block: at, channel });
                }
            }
        }
        unheard_signal(base, channel, donor);
    }
}

/// How far apart, in samples, two views of one boundary may lie: where one decode ends a block
/// and another starts the next.
const POSITION_TOL: f64 = 1000.0;

/// Puts into `base` each stretch of raw signal `donor` kept where `base` heard silence and read
/// nothing proved.
///
/// A stereo recording can hold a stretch on one track alone: a custom loader's data recorded on
/// one head, a dropout on the other. Such data proves nothing a ROM block proves, so the folds
/// above never move it, and whichever channel is kept decides whether the signal is on the tape
/// at all. The evidence is each side's own verdict: `base` played the stretch as a pause, its
/// claim that it heard nothing there; `donor` kept it as raw signal, so by its measure it is
/// signal and not noise.
///
/// The stretch goes in at its own place. What it replaces is what `base` holds between its last
/// block whose signal ends before the stretch starts and its first block that starts at or past
/// the stretch's end (each within [`POSITION_TOL`]): it must be silence, a pause of that block's
/// running into the stretch or pause blocks, and nothing else but reads that prove nothing
/// (under [`Proof::Checksum`]). Nothing either side of the stretch is cut or
/// doubled: the block before keeps its pause up to the stretch, which keeps the donor's.
fn unheard_signal(base: &mut Decoded, channel: Channel, donor: &Decoded) {
    for di in 0..donor.blocks.len() {
        if !matches!(donor.blocks[di].body, Body::Csw { .. } | Body::Direct { .. }) {
            continue;
        }
        let span = donor.extents[di];
        let Some(after) =
            (0..base.blocks.len()).find(|&i| base.extents[i].start + POSITION_TOL >= span.signal_end)
        else {
            continue;
        };
        let Some(before) = (0..after)
            .rev()
            .find(|&i| !is_pause(&base.blocks[i]) && base.extents[i].signal_end <= span.start + POSITION_TOL)
        else {
            continue;
        };
        let run = before + 1..after;
        let proves_nothing = |b: &Block| match &b.body {
            Body::Pause { .. } => true,
            Body::Standard { data, .. } => proof::of(data) < Proof::Checksum,
            _ => false,
        };
        let pause_into =
            pause_of(&base.blocks[before]).is_some_and(|p| p > 0) && base.extents[before].end > span.start;
        let silence = pause_into || base.blocks[run.clone()].iter().any(is_pause);
        if !silence || !base.blocks[run.clone()].iter().all(proves_nothing) {
            continue;
        }
        // The block before plays its pause up to where the stretch starts.
        let ms = |samples: f64| (samples.max(0.0) / f64::from(base.rate_hz) * 1000.0).round() as u16;
        let held = ms(span.start - base.extents[before].signal_end);
        if let Some(pause) = pause_mut(&mut base.blocks[before]) {
            *pause = held;
        }
        base.extents[before].end =
            base.extents[before].end.min(span.start).max(base.extents[before].signal_end);
        let at = run.start;
        splice_into(base, run, vec![(donor.blocks[di].clone(), span)]);
        base.findings.push(Finding::FromChannel { block: at, channel });
    }
}

/// Ends the pause of `base`'s block `i` where a block put in behind it starts, at `at`: `base`
/// played that stretch as the pause while it heard nothing there.
fn end_pause_at(base: &mut Decoded, i: usize, at: f64) {
    let extent = base.extents[i];
    if extent.end <= at {
        return;
    }
    let held = ((at - extent.signal_end).max(0.0) / f64::from(base.rate_hz) * 1000.0).round() as u16;
    if let Some(pause) = pause_mut(&mut base.blocks[i]) {
        *pause = (*pause).min(held);
    }
    base.extents[i].end = at.max(extent.signal_end);
}

fn is_pause(b: &Block) -> bool {
    matches!(b.body, Body::Pause { .. })
}

/// The pause a block plays after its signal, where it has one.
fn pause_of(b: &Block) -> Option<u16> {
    match b.body {
        Body::Standard { pause, .. } | Body::Csw { pause, .. } | Body::Direct { pause, .. } => Some(pause),
        _ => None,
    }
}

fn pause_mut(b: &mut Block) -> Option<&mut u16> {
    match &mut b.body {
        Body::Standard { pause, .. } | Body::Csw { pause, .. } | Body::Direct { pause, .. } => Some(pause),
        _ => None,
    }
}

/// The index of `base`'s own read of `leg`, the donor's block at `leg_at`, behind strays: where
/// a block of the same bytes overlaps the donor's and every block between it and the header at
/// `header` is a checksum-bad block of data. A read can cut a fragment between a header and a
/// leg it reads whole; replaced, the fragment would put the leg in twice.
fn own_leg_behind_strays(base: &Decoded, header: usize, leg: &[u8], leg_at: &Extent) -> Option<usize> {
    for j in header + 1..base.blocks.len() {
        let data = image(&base.blocks[j])?;
        if data == leg {
            return (j > header + 1 && overlap(&base.extents[j], leg_at)).then_some(j);
        }
        if proof::of(data) != Proof::None {
            return None;
        }
    }
    None
}

/// Puts `with` in place of `range` in `base`, its findings about the blocks there dropped and
/// those after moved along.
fn splice_into(base: &mut Decoded, range: std::ops::Range<usize>, with: Vec<(Block, Extent)>) {
    let (start, end, n) = (range.start, range.end, with.len());
    let (blocks, extents): (Vec<Block>, Vec<Extent>) = with.into_iter().unzip();
    base.blocks.splice(range.clone(), blocks);
    base.extents.splice(range, extents);
    base.findings.retain_mut(|f| match f.block_mut() {
        Some(b) if *b >= end => {
            *b = *b + n - (end - start);
            true
        }
        Some(b) => *b < start,
        None => true,
    });
}
