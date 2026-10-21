// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The readings of one recording: one kept, the others folded into it.
//!
//! A recording is read more than once where one reading can lose what another holds. A real
//! stereo capture's channels are not copies of one another: the two heads sit a few samples
//! apart, and the gap drifts along the tape. A sync half-wave lasts about eight samples at
//! 44.1 kHz, so mixed, the channels can cancel the short half-waves at places and lose blocks
//! that either channel alone reads; the mix, the left and the right channel are each read.
//! Sampled audio is read on the zero line and on the signal's own midline ([`Axis`]), for a worn
//! tape whose centre wanders. No statistic of the signal tells beforehand which reading is best,
//! so each is decoded, one is kept ([`pick`]) and the others are folded into it ([`fold`]).
//! A block none of them reads whole is read again from all of them, byte by byte ([`vote`]).
//!
//! A Spectrum loads a block by its checksum, and nothing else on a tape is proved, so a decode
//! is judged by its verified bytes, never by how many blocks it counts. The shape score of the
//! decodes names the incumbent, ties going to the first (the mix, the zero line), so a recording
//! whose readings agree decodes exactly as its first. The pick moves off the incumbent only to a
//! decode that delivers every verified image the incumbent does and more verified bytes; a
//! program's header adds a name, which is no loaded material.

use crate::entry::{Decoded, Extent, Finding, Loss};
use crate::proof::{self, asks, header_shaped, image, Proof};
use crate::readers::Channel;
use crate::waveform::edges::Axis;
use std::collections::BTreeSet;
use tapeti_core::types::{Block, Body};

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
/// A block counts at [`Proof::Checksum`] or better; a header counts when it is shaped like one
/// whose next block is as long as it declares, checksum or not, since a one-bit slip keeps a
/// checksum good about half the time while spoiling the declared length. Without that pairing, a
/// channel that reads pilot tones and headers as runs of 0xFF would outscore one that reads every
/// block whole.
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

/// Which of `decodes`, in the order they were read, to keep.
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

/// Which of `readings` to keep, in the order they were read, `None` for one that read nothing:
/// [`pick`], with each block weighed as the fold would let it climb.
///
/// Keeping a reading puts each of its blocks over what the others read at that place, so a
/// block is weighed by the rule a block folded in is held to (`told_apart`): one that checks
/// out, where another reading reads it at the same place and fails, and the checksum cannot
/// tell the two apart, weighs as that failed reading.
pub fn pick_reading(readings: &[Option<&Decoded>]) -> usize {
    let views: Vec<std::borrow::Cow<'_, [Block]>> = readings
        .iter()
        .enumerate()
        .map(|(i, r)| match r {
            Some(d) => {
                witnessed(d, readings.iter().enumerate().filter(|&(k, _)| k != i).filter_map(|(_, r)| *r))
            }
            None => std::borrow::Cow::Borrowed(&[][..]),
        })
        .collect();
    let views: Vec<&[Block]> = views.iter().map(|v| &v[..]).collect();
    pick(&views)
}

/// The blocks of `d`, each block that checks out weighed as a failed reading of it in one of
/// `others` that the checksum cannot tell it from ([`pick_reading`]).
///
/// A block whose last byte the decode filled out proves nothing ([`proof::filled_out`]): it is
/// weighed with the bits it did not read set, under which the checksum the 0s complete fails.
fn witnessed<'a, 'b>(
    d: &'a Decoded,
    others: impl Iterator<Item = &'b Decoded> + Clone,
) -> std::borrow::Cow<'a, [Block]> {
    let mut out = std::borrow::Cow::Borrowed(&d.blocks[..]);
    for i in 0..d.blocks.len() {
        let Some(data) = image(&d.blocks[i]) else { continue };
        let read = d.findings.iter().find_map(|f| match f {
            Finding::PartialFinalByte { block, used_bits, .. } if *block == i => Some(*used_bits),
            _ => None,
        });
        if let Some(read) = read {
            if let Body::Standard { data, .. } = &mut out.to_mut()[i].body {
                *data.last_mut().expect("a byte to fill out") |= 0xFF >> read;
            }
            continue;
        }
        if proof::of(data) < Proof::Checksum {
            continue;
        }
        let span = d.extents[i];
        let against = others.clone().find_map(|r| {
            (0..r.blocks.len()).find_map(|k| {
                image(&r.blocks[k]).filter(|failed| {
                    r.proof_of(k, proof::of(failed)) < Proof::Checksum
                        && overlap(&r.extents[k], &span)
                        && !told_apart(failed, data)
                })
            })
        });
        if let Some(failed) = against {
            if let Body::Standard { data, .. } = &mut out.to_mut()[i].body {
                *data = failed.to_vec();
            }
        }
    }
    out
}

/// Where a reading folded into the one kept came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// One of a stereo recording's mix and two channels.
    Channel(Channel),
    /// One channel's reading on the zero line or on its midline.
    Axis(Axis),
}

/// What a stretch of a reading holds, from nothing up: the one ladder every fold climbs.
///
/// Silence, then signal that proves nothing (raw signal, a checksum that fails, a block
/// [`Proof::Trivial`]), then a block by its [`Proof`] from [`Proof::Checksum`] up. At one rank,
/// ties go, in order: to the ROM's own sync pair over a form of it where the two readings differ
/// in most of their bytes ([`Finding::MergedSync`]); to fewer bits read through halves no saver
/// writes ([`Finding::UnlikeHalves`]), each where a lobe was lost and the bits behind it may be
/// one out; to the ordinary band over the
/// fade band ([`Finding::FadeBand`]); then to more readings alike. A checksum is blind to one
/// bit wrong in two bytes and holds by chance over a reading a bit out one time in 256, so two
/// readings that both check out are told apart only by these; where they tie on all of them, the
/// reading kept stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Standing {
    Silence,
    Unproven,
    Proven { proof: Proof, rom_sync: bool, unlike: std::cmp::Reverse<usize>, ordinary: bool, alike: usize },
}

/// How far apart, in samples, two readings of one boundary may lie: where one decode ends a block
/// and another starts the next.
const POSITION_TOL: f64 = 1000.0;

/// Folds into `base`, the reading kept, each block of `donors` that stands higher on the
/// `Standing` ladder than what `base` holds at its place.
///
/// The place is the recording's. A donor's block stands against the run of `base`'s blocks
/// after the last whose signal ends before the block's starts and before the first that starts
/// where its signal ends, either within `POSITION_TOL` (a quarter of the block's signal where
/// that is less). It is ranked as it would stand there, between `base`'s blocks of data either
/// side ([`proof::between`]): a block one reading reads whole answers the header another
/// reading read, and a header the block behind it answers is as good as that answer. The run
/// stands at its best block, at silence where it holds nothing proved and no raw signal and
/// `base` played a pause there.
///
/// What enters:
/// - a block past chance ([`Proof::Verified`] or more) that stands higher than the run, and
///   that the checksum tells from a read of it that proves nothing opening the run
///   (`told_apart`). It takes the run's place with its own start and signal; where the run
///   ends in a read, the rest of a failed read or the raw signal it left, it ends where the run
///   did, its pause to there, and where `base` heard silence it comes as the donor has it,
///   its pause ending where `base`'s next block starts;
/// - at one rank, the same block read otherwise, where the run is one block of data as long:
///   its bytes, at `base`'s place and pause;
/// - raw signal where `base` heard silence and read nothing proved. A stereo recording can hold
///   a stretch on one track alone, a custom loader's data on one head and a dropout on the
///   other; it proves nothing a ROM block proves, but `base` played it as a pause, its claim
///   that it heard nothing there, and the donor kept it as signal. It comes as the donor has it,
///   its pause likewise ending where `base`'s next block starts, from another channel and
///   between two of `base`'s blocks. Two heads hear a stretch apart, but the two lines of one
///   channel read the same samples, and that one cuts its noise into pulses shaped like a block
///   where the other calls it a pause is no second hearing. At the recording's edges `base`'s
///   silence is its lead-in or its tail, where a recording that starts or ends inside a
///   neighbour holds a cut piece of it on either track. A run-on ([`Finding::RunOn`]) is the
///   donor's reading of the tail of the block in front, which `base` read to its own end: it
///   comes only behind that block taken from the donor.
///
/// Nothing a donor failed to read enters. The block before a block put in plays its pause up
/// to where that block starts, and what a donor said of the block (its sync, its band, its
/// leader, a stray zero, a partial last byte, unlike halves, that it is raw signal) comes with
/// it. Donors are
/// asked in order, each block by block against `base` as folded so far; each block placed is
/// reported ([`Finding::FromChannel`], [`Finding::FromAxis`], or [`Finding::BothChannels`] where
/// readings alike decided).
pub fn fold(base: &mut Decoded, donors: &[(Source, &Decoded)]) {
    if donors.is_empty() {
        return;
    }
    let kept = base.clone();
    let readings: Vec<&Decoded> = std::iter::once(&kept).chain(donors.iter().map(|&(_, d)| d)).collect();
    for &(source, donor) in donors {
        for j in 0..donor.blocks.len() {
            fold_block(base, source, donor, j, &readings);
        }
    }
}

/// Folds block `j` of `donor` into `base`, as [`fold`] says.
fn fold_block(base: &mut Decoded, source: Source, donor: &Decoded, j: usize, readings: &[&Decoded]) {
    let span = donor.extents[j];
    let data = image(&donor.blocks[j]);
    let raw = matches!(donor.blocks[j].body, Body::Csw { .. } | Body::Direct { .. });
    if data.is_none() && !(raw && matches!(source, Source::Channel(_))) {
        return;
    }
    let (before, run) = run_at(base, &span);
    // A run-on is said of the block in front, the donor's own reading of its tail, where `base`
    // read that block to its own end; it goes only behind that block taken from the donor.
    let taken = |b: usize| {
        base.findings.iter().any(|f| match (f, source) {
            (Finding::FromChannel { block, channel }, Source::Channel(c)) => *block == b && *channel == c,
            (Finding::FromAxis { block, axis }, Source::Axis(a)) => *block == b && *axis == a,
            _ => false,
        })
    };
    if donor.findings.contains(&Finding::RunOn { block: j }) && !before.is_some_and(taken) {
        return;
    }
    let there = match data {
        None => Standing::Unproven,
        Some(d) => {
            let front = base.blocks[..run.start].iter().rev().find_map(image);
            let behind = base.blocks[run.end..].iter().find_map(image);
            let proof = donor.proof_of(j, proof::between(front, d, behind));
            if proof < Proof::Checksum {
                return;
            }
            standing_of(donor, j, proof, readings)
        }
    };
    let mut here = run
        .clone()
        .map(|i| (i, base.proof_of(i, proof::at(&base.blocks, i))))
        .filter(|&(_, proof)| proof >= Proof::Checksum)
        .map(|(i, proof)| standing_of(base, i, proof, readings))
        .max()
        .unwrap_or_else(|| {
            let pause_into = before.is_some_and(|b| {
                pause_of(&base.blocks[b]).is_some_and(|p| p > 0) && base.extents[b].end > span.start
            });
            let pieces = &base.blocks[run.clone()];
            let signal = pieces.iter().any(|b| matches!(b.body, Body::Csw { .. } | Body::Direct { .. }));
            if !signal && (pause_into || pieces.iter().any(is_pause)) {
                Standing::Silence
            } else {
                Standing::Unproven
            }
        });
    let single = (run.len() == 1).then(|| image(&base.blocks[run.start])).flatten();
    if let (Standing::Proven { rom_sync, .. }, Some(d), Some(b)) = (&mut here, data, single) {
        // A form of the sync risks a reading out of step, which differs from the true one in
        // nearly every byte; two readings that differ in a few have the same start, and their
        // syncs say nothing about which is right.
        let differ = d.iter().zip(b).filter(|(x, y)| x != y).count();
        if d.len() != b.len() || 2 * differ <= d.len() {
            *rom_sync = matches!(there, Standing::Proven { rom_sync: true, .. });
        }
    }
    let (tie, by_votes) = match (here, there) {
        (
            Standing::Proven { proof: a, rom_sync: s, unlike: u, ordinary: o, .. },
            Standing::Proven { proof: b, rom_sync: t, unlike: v, ordinary: p, .. },
        ) if a == b => (true, s == t && u == v && o == p),
        _ => (false, false),
    };
    let enters = there > here
        && match there {
            Standing::Unproven => {
                here == Standing::Silence && before.is_some() && run.end < base.blocks.len()
            }
            Standing::Proven { .. } if tie => {
                single.is_some_and(|b| data.is_some_and(|d| d.len() == b.len() && d != b))
            }
            Standing::Proven { proof, .. } => {
                proof >= Proof::Verified
                    && run.clone().next().and_then(|i| image(&base.blocks[i]).map(|b| (i, b))).is_none_or(
                        |(i, failed)| {
                            base.proof_of(i, proof::of(failed)) >= Proof::Checksum
                                || data.is_some_and(|d| told_apart(failed, d))
                        },
                    )
            }
            Standing::Silence => false,
        };
    if !enters {
        return;
    }
    let at = run.start;
    let (block, extent) = if tie {
        let mut block = base.blocks[at].clone();
        if let (Body::Standard { data: bytes, .. }, Some(d)) = (&mut block.body, data) {
            *bytes = d.to_vec();
        }
        (block, base.extents[at])
    } else {
        let mut block = donor.blocks[j].clone();
        match run.clone().next_back().filter(|&k| !is_pause(&base.blocks[k])) {
            Some(last) => {
                let end = base.extents[last].end.max(span.signal_end);
                if let Some(pause) = pause_mut(&mut block) {
                    *pause = ms(end - span.signal_end, base.rate_hz);
                }
                (block, Extent { start: span.start, signal_end: span.signal_end, end })
            }
            // Into silence it comes as the donor has it, its pause ending where `base`'s next
            // block starts, so that nothing after it moves.
            None => match base.extents.get(run.end).map(|e| e.start.max(span.signal_end)) {
                Some(next) if next < span.end => {
                    if let Some(pause) = pause_mut(&mut block) {
                        *pause = ms(next - span.signal_end, base.rate_hz);
                    }
                    (block, Extent { end: next, ..span })
                }
                _ => (block, span),
            },
        }
    };
    if let (false, Some(b)) = (tie, before) {
        end_pause_at(base, b, extent.start);
    }
    if !tie && run.clone().all(|k| is_pause(&base.blocks[k])) {
        // Put into the pause behind `before`, or before the first block, it is the block whose
        // pilot `base` heard there and did not read.
        base.findings
            .retain(|f| !matches!(f, Finding::Lost { block, what: Loss::Pilot, .. } if *block == before));
    }
    splice_into(base, run, vec![(block, extent)]);
    for f in &donor.findings {
        let said = matches!(
            f,
            Finding::RawSignal { .. }
                | Finding::MergedSync { .. }
                | Finding::FadeBand { .. }
                | Finding::ShortLeader { .. }
                | Finding::RunOn { .. }
                | Finding::StrayZero { .. }
                | Finding::PartialFinalByte { .. }
                | Finding::UnlikeHalves { .. }
        );
        if said && f.block() == Some(j) {
            let mut f = f.clone();
            *f.block_mut().expect("about a block") = at;
            base.findings.push(f);
        }
    }
    base.findings.push(match source {
        _ if by_votes => Finding::BothChannels { block: at },
        Source::Channel(channel) => Finding::FromChannel { block: at, channel },
        Source::Axis(axis) => Finding::FromAxis { block: at, axis },
    });
}

/// Reads again, byte by byte, each block of `base` that the header in front asks for and that
/// proves nothing.
///
/// Such a block's checksum fails, holds over what a misread leaves, or holds only by the last
/// byte the decode filled out (which then has no vote).
/// A byte is taken from a majority of `readings` in step there, three at least
/// (`by_majority`), since two readings can share a wrong byte where both tracks carry the
/// tape's own defects; the block is replaced only where the result answers the header
/// ([`Proof::Answer`], [`Finding::Voted`]). `readings` are those the decode was kept and folded
/// from, not `base`.
pub fn vote(base: &mut Decoded, readings: &[&Decoded]) {
    for i in 0..base.blocks.len() {
        let Some(data) = image(&base.blocks[i]) else { continue };
        let Some(asked) = base.blocks[..i].iter().rev().find_map(image).and_then(asks) else { continue };
        if data.len() != asked || base.proof_of(i, proof::of(data)) >= Proof::Checksum {
            continue;
        }
        let span = base.extents[i];
        let at: Vec<&[u8]> = readings
            .iter()
            .filter_map(|r| {
                (0..r.blocks.len()).filter(|&k| overlap(&r.extents[k], &span)).find_map(|k| {
                    let d = image(&r.blocks[k])?;
                    Some(if r.filled_out(k) { &d[..d.len() - 1] } else { d })
                })
            })
            .collect();
        let Some(voted) = by_majority(&at, asked).filter(|v| proof::answering(v, asked) >= Proof::Answer)
        else {
            continue;
        };
        if let Body::Standard { data, .. } = &mut base.blocks[i].body {
            *data = voted;
        }
        // What was said of the block's bytes and how they were read is said of none of them now;
        // what its pause holds stays.
        base.findings.retain(|f| {
            f.block() != Some(i)
                || matches!(f, Finding::Collapsed { .. } | Finding::Lost { what: Loss::Pilot, .. })
        });
        base.findings.push(Finding::Voted { block: i, readings: at.len() });
    }
}

/// `n` bytes, each the value a majority of `readings` hold where they are in step, at least
/// three of them; `None` where a byte has no such majority.
///
/// A reading is in step from its first byte up to where it slips: from there it parts from the
/// majority of all the readings in most of its bytes to its end, a byte it lacks parting too, as
/// [`told_apart`] sets a slip aside. That majority is a first pass over every reading, from its
/// first byte, whose slipped tails seldom agree with each other.
fn by_majority(readings: &[&[u8]], n: usize) -> Option<Vec<u8>> {
    const LEAST: usize = 3;
    if readings.len() < LEAST {
        return None;
    }
    // The value more than half of `held` hold.
    let most = |held: &[u8]| {
        held.iter().copied().find(|&v| 2 * held.iter().filter(|&&h| h == v).count() > held.len())
    };
    let first: Vec<Option<u8>> = (0..n)
        .map(|k| most(&readings.iter().filter_map(|r| r.get(k).copied()).collect::<Vec<u8>>()))
        .collect();
    let in_step: Vec<usize> = readings
        .iter()
        .map(|r| {
            let (mut slip, mut parted) = (n, 0);
            for k in (0..n).rev() {
                if r.get(k).copied() != first[k] {
                    parted += 1;
                    if 2 * parted > n - k {
                        slip = k;
                    }
                }
            }
            slip
        })
        .collect();
    (0..n)
        .map(|k| {
            let held: Vec<u8> =
                readings.iter().zip(&in_step).filter(|&(_, &s)| k < s).map(|(r, _)| r[k]).collect();
            if held.len() < LEAST {
                return None;
            }
            most(&held)
        })
        .collect()
}

/// Whether the checksum tells `whole`, a reading of a block that proves itself, from `failed`, a
/// reading that proves nothing: whether the bytes where the two part, outside a slipped tail
/// (set aside as one event), differ by more than XOR is blind to.
///
/// If their differences there XOR to anything but zero, `whole` is the tape's. If they XOR to
/// zero, a single bit wrong in two bytes, which a merged sync or a half-bit slip leaves readily,
/// `whole` checks out whichever of the two is right, and `failed` is as likely to be it.
fn told_apart(failed: &[u8], whole: &[u8]) -> bool {
    let n = failed.len().max(whole.len());
    let parts = |k: usize| failed.get(k) != whole.get(k);
    let (mut tail, mut parted) = (n, 0);
    for k in (0..n).rev() {
        if parts(k) {
            parted += 1;
            if 2 * parted > n - k {
                tail = k;
            }
        }
    }
    let differ = (0..tail).filter(|&k| parts(k)).fold(0, |x, k| x ^ failed[k] ^ whole[k]);
    (0..tail).all(|k| !parts(k)) || differ != 0
}

/// Where block `i` of `d`, of proof `proof` where it stands, sits on the `Standing` ladder.
fn standing_of(d: &Decoded, i: usize, proof: Proof, readings: &[&Decoded]) -> Standing {
    let data = image(&d.blocks[i]).expect("a block of data");
    let span = d.extents[i];
    Standing::Proven {
        proof,
        rom_sync: !d.findings.contains(&Finding::MergedSync { block: i }),
        unlike: std::cmp::Reverse(
            d.findings
                .iter()
                .find_map(|f| match f {
                    Finding::UnlikeHalves { block, bits } if *block == i => Some(*bits),
                    _ => None,
                })
                .unwrap_or(0),
        ),
        ordinary: !d.findings.contains(&Finding::FadeBand { block: i }),
        alike: readings
            .iter()
            .filter(|r| {
                (0..r.blocks.len())
                    .any(|k| image(&r.blocks[k]) == Some(data) && overlap(&r.extents[k], &span))
            })
            .count(),
    }
}

/// The run of `base`'s blocks a block of another reading at `span` stands against, as [`fold`]
/// says, and the block before it, if any.
fn run_at(base: &Decoded, span: &Extent) -> (Option<usize>, std::ops::Range<usize>) {
    let tol = POSITION_TOL.min((span.signal_end - span.start) / 4.0);
    let after = (0..base.blocks.len())
        .find(|&i| base.extents[i].start + tol >= span.signal_end)
        .unwrap_or(base.blocks.len());
    let before = (0..after)
        .rev()
        .find(|&i| !is_pause(&base.blocks[i]) && base.extents[i].signal_end <= span.start + tol);
    (before, before.map_or(0, |b| b + 1)..after)
}

/// Whether the signals of two extents of one recording overlap.
fn overlap(a: &Extent, b: &Extent) -> bool {
    a.start < b.signal_end && b.start < a.signal_end
}

/// `samples` at `rate_hz` in milliseconds, as a pause holds them.
fn ms(samples: f64, rate_hz: u32) -> u16 {
    (samples.max(0.0) / f64::from(rate_hz) * 1000.0).round().min(f64::from(u16::MAX)) as u16
}

/// Ends the pause of `base`'s block `i` where a block put in behind it starts, at `at`: `base`
/// played that stretch as the pause while it heard nothing there.
fn end_pause_at(base: &mut Decoded, i: usize, at: f64) {
    let extent = base.extents[i];
    if extent.end <= at {
        return;
    }
    let held = ms(at - extent.signal_end, base.rate_hz);
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
