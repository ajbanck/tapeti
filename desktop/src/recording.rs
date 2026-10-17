// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Opens a tape recording (WAV, VOC, IFF, CSW) by decoding it into blocks.
//!
//! [`backend`] is the only file that knows the decoder's own types; it turns what it
//! returns into the plain data defined here: [`Decode`], [`Loader`], [`DecodeError`],
//! [`Progress`]. Everything else reads only that: provenance, the dialogs, splicing a
//! decode into a tape, and the background job ([`Jobs`]).
//!
//! A decode takes seconds to minutes and up to gigabytes, so it runs on a thread of
//! its own. `files.rs` queues a request; `App::frame` hands the queue to
//! [`Jobs::pump`] before the row caches are rebuilt, since that is where a tape can
//! be swapped, and the same call collects what has finished.

mod backend;

use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Instant;

use tapeti_core::types::{Block, Body, Uid};

use crate::actions::Then;
use crate::dialogs::Dialog;
use crate::files::file_name;
use crate::state::{
    ChannelChoice, DecodeChoice, Landing, NoteClass, Recording, RecordingNote, RecordingRequest, Side, Store,
};
use backend::Cancel;

// ---- what a decode returns -----------------------------------------------------------

/// A finished decode, as plain data.
#[derive(Clone, Debug, Default)]
pub struct Decode {
    /// The file decoded; for a signal handed over as pulses, the label it was given.
    pub path: PathBuf,
    /// Every block, in order, each under a fresh uid.
    pub blocks: Vec<Block>,
    /// The grid positions are counted on: samples a second for audio, `None` for
    /// input that has no such grid.
    pub rate_hz: Option<u32>,
    /// Where each block lies in the source, on that grid, where the decode knows:
    /// both ends, or nothing.
    pub extents: Vec<Option<Range<u64>>>,
    /// What the decoder has to say, one finding per line of its own report.
    pub findings: Vec<Finding>,
    /// What the load itself noted, before any finding: the first lines of the report.
    pub notes: Vec<String>,
    /// The measured rows (signal, scan, …), labelled; none for a tape read as blocks.
    pub measured: Vec<(String, Vec<String>)>,
    /// The spans of raw signal at the edges of the decode.
    pub edges: Vec<EdgeSpan>,
    /// How many data blocks' checksums hold.
    pub checksums_valid: usize,
    /// Whether nothing better than raw signal came back: the same signal again in
    /// other blocks, and not worth an edit.
    pub floor_only: bool,
}

/// One thing the decoder said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub class: NoteClass,
    pub text: String,
    /// The row it is about, in [`Decode::blocks`]; `None` for the recording as a whole.
    pub row: Option<usize>,
}

/// Which end of the decode a span of raw signal is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Before the first block: a capture's lead-in, a window's near cut.
    Lead,
    /// After the last: a capture's run-out, a window's far cut.
    Trail,
}

/// Raw signal at an edge of a decode: how many pulses, and the rows it was kept as.
/// An empty range means it was folded into the pause before it; `None` means it made no row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeSpan {
    pub edge: Edge,
    pub pulses: usize,
    pub rows: Option<Range<usize>>,
}

/// Why a decode returned nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// It was told to stop, which is nothing to tell anybody about.
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Cancelled => f.write_str("the decode was cancelled"),
            DecodeError::Failed(why) => f.write_str(why),
        }
    }
}

/// What a running decode reports: the blocks read so far, and the share of the work done,
/// from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub blocks: usize,
    pub done: f32,
}

/// A loader the decoder offers, as it describes it to a person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loader {
    /// The name to ask for it by.
    pub name: &'static str,
    pub title: &'static str,
    pub about: &'static str,
}

/// What the store keeps of a decode: `d.blocks`' uids are what the notes and extents
/// are keyed by.
pub fn provenance(name: &str, choice: DecodeChoice, d: &Decode) -> Recording {
    let notes = d
        .findings
        .iter()
        .map(|f| RecordingNote {
            class: f.class,
            text: f.text.clone(),
            block: f.row.and_then(|at| d.blocks.get(at)).map(|b| b.uid),
        })
        .collect();
    let mut report = d.notes.clone();
    report.extend(d.findings.iter().map(|f| f.text.clone()));
    let extents = d
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            let (from, to) = extent_seconds(d, i)?;
            Some((b.uid, ((from * 1000.0) as u32, (to * 1000.0) as u32)))
        })
        .collect();
    Recording {
        name: name.to_string(),
        path: d.path.clone(),
        choice,
        notes,
        report,
        measured: d.measured.clone(),
        extents,
        rate_hz: d.rate_hz,
        inserted: Vec::new(),
    }
}

/// Where `d.blocks[i]` lies in the recording, in seconds, if the decode knew.
pub fn extent_seconds(d: &Decode, i: usize) -> Option<(f64, f64)> {
    let rate = f64::from(d.rate_hz?);
    let at = d.extents.get(i)?.as_ref()?;
    Some((at.start as f64 / rate, at.end as f64 / rate))
}

// ---- how to decode ---------------------------------------------------------------

/// Whether the pane's tape can be decoded again: it came from a recording, and
/// nothing is being decoded into the pane already.
pub fn can_redecode(store: &Store, jobs: &Jobs, side: Side) -> bool {
    store.tape(side).recording.as_ref().is_some_and(Recording::is_of_the_tape)
        && jobs.landing_in(side).is_none()
}

/// The re-decode dialog's state: the choice being made, for the tape in `side`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedecodeState {
    pub side: Side,
    pub choice: DecodeChoice,
    /// `None` decodes the whole recording and replaces the tape's blocks; a stretch
    /// decodes that window of it and replaces the run it names.
    pub stretch: Option<Stretch>,
}

impl RedecodeState {
    /// Opens on how the tape was decoded last.
    pub fn new(store: &Store, side: Side) -> RedecodeState {
        let choice = store.tape(side).recording.as_ref().map(|r| r.choice).unwrap_or_default();
        RedecodeState { side, choice, stretch: None }
    }

    /// The same dialog for the selected run alone; `None` where [`stretch_of`] is.
    pub fn for_stretch(store: &Store, side: Side) -> Option<RedecodeState> {
        let stretch = stretch_of(store, side)?;
        Some(RedecodeState { stretch: Some(stretch), ..RedecodeState::new(store, side) })
    }
}
/// The dialog's body. Returns whether it is done with: a decode was asked for,
/// or it was cancelled. The decode is a request like any other, so the memory
/// question and the status bar work as they do for Open.
pub fn redecode_body(
    ui: &mut egui::Ui,
    store: &mut Store,
    s: &mut RedecodeState,
    tok: &crate::theme::Tokens,
) -> bool {
    use crate::widgets as w;
    let Some(recording) = store.tape(s.side).recording.as_ref() else { return true };
    // A stretch is read from the recording its blocks came from, which on a tape with
    // a recording inserted is not the tape's own.
    let source = match s.stretch {
        None => recording,
        Some(st) => match recording.source_of(st.first) {
            Some(source) => source,
            None => return true,
        },
    };
    let (name, path) = (source.name.clone(), source.path.clone());
    match s.stretch {
        None => ui.label(format!("Decode \"{name}\" again, replacing the blocks of this tape.")),
        Some(st) => ui.label(format!(
            "Decode {} of \"{name}\" again, replacing {}.",
            stretch_label(st.from_ms, st.to_ms),
            if st.blocks == 1 { "this block".to_string() } else { format!("these {} blocks", st.blocks) },
        )),
    };
    ui.add_space(6.0);

    loader_row(ui, tok);
    let channels = [
        (ChannelChoice::Best, "Whichever reads best".to_string()),
        (ChannelChoice::Mix, "Mix of left and right".to_string()),
        (ChannelChoice::Left, "Left".to_string()),
        (ChannelChoice::Right, "Right".to_string()),
    ];
    ui.horizontal(|ui| {
        ui.add_sized([70.0, 18.0], egui::Label::new("Channel").halign(egui::Align::LEFT));
        w::combo(ui, "redecode-channel", &mut s.choice.channel, &channels, 220.0, true);
    });
    w::check(ui, "Band-pass filter the recording first", &mut s.choice.filter, true);
    w::note(
        ui,
        tok,
        "The filter helps some recordings with hum or hiss and costs others blocks: try both. \
         Decoding again is an edit, and can be undone.",
    );

    let mut done = false;
    crate::dialogs::footer(ui, |ui| {
        if ui.button("Decode").clicked() {
            store.recordings.push(RecordingRequest {
                side: s.side,
                path,
                into: match s.stretch {
                    None => Landing::Replace,
                    Some(st) => Landing::Stretch {
                        first: st.first,
                        last: st.last,
                        from_ms: st.from_ms,
                        to_ms: st.to_ms,
                    },
                },
                choice: s.choice,
                // The memory a window needs is the window's; the question Open asks
                // is about the whole file.
                confirmed: s.stretch.is_some(),
            });
            done = true;
        }
        if ui.button("Cancel").clicked() {
            done = true;
        }
    });
    done
}

/// The loader a decode reads with, and what it reads. One until a second is written, so there is
/// nothing to choose.
fn loader_row(ui: &mut egui::Ui, tok: &crate::theme::Tokens) {
    let loader = backend::loaders().remove(0);
    ui.horizontal(|ui| {
        ui.add_sized([70.0, 18.0], egui::Label::new("Loader").halign(egui::Align::LEFT));
        ui.label(loader.title);
    });
    crate::widgets::note(ui, tok, loader.about);
}

// ---- the loader a decode read with -----------------------------------------------------

/// A way of reading: the decoder's default, or a loader by name.
type Reading = Option<&'static str>;

/// What to call a reading where a person reads it.
fn reading_title(reading: Reading) -> &'static str {
    match reading {
        None => backend::loaders()[0].title,
        Some(name) => backend::loaders().into_iter().find(|l| l.name == name).map_or(name, |l| l.title),
    }
}

// ---- decoding a block's own signal ---------------------------------------------------
//
// Taper's "decode entry": turns a block that carries a signal (a direct recording, a
// CSW block, or the pulse sequences and tones a decoder leaves where it could not read)
// into the data blocks that signal holds, in place. It works on any tape, not only one
// decoded here: the pulses are the core's own rendering of the blocks, fed to the
// decoder through the same door as a caller-supplied signal.
//
// Not a background job: finding a recording's edges is what makes a decode take long, and
// there are none here. Pulses go straight to framing, a fraction of a second for a whole
// tape, and running it synchronously means the blocks cannot change between the question
// and the answer.

/// The grid the pulses are counted on: T-states, the unit they already have, so a
/// position that comes back is exact. For a signal the caller supplies, this is a
/// label for the grid the decoder is told to use, never a resampling.
const TSTATE_GRID_HZ: u32 = 3_500_000;

/// Whether a block is signal that might decode into something better: anything that
/// plays but is not already bytes under a loader's timing.
fn is_undecoded_signal(b: &Block) -> bool {
    matches!(
        b.body,
        Body::Direct { .. }
            | Body::Csw { .. }
            | Body::PulseSeq { .. }
            | Body::PureTone { .. }
            | Body::PureData { .. }
            | Body::Generalized { .. }
    )
}

/// What "decode" would act on in `side`: the selection (or the block under the cursor,
/// or a collapsed group), if it is one unbroken run with undecoded signal in it. Blocks
/// picked from here and there have no one signal between them.
pub fn signal_run(store: &Store, side: Side) -> Option<std::ops::Range<usize>> {
    let t = store.tape(side);
    let mut picked = t.unit_indices(t.cursor);
    picked.sort_unstable();
    let (first, last) = (*picked.first()?, *picked.last()?);
    let unbroken = last - first + 1 == picked.len();
    (unbroken && t.blocks[first..=last].iter().any(is_undecoded_signal)).then_some(first..last + 1)
}

/// The blocks as the pulses they play, in T-states: the core's rendering, with
/// consecutive stretches at one level joined (a pulse is what lies between two edges,
/// and the core emits a pause, or a run of equal samples, as several pieces of one).
/// Flow control is not followed: a loop inside the run plays once.
fn pulses_of(blocks: &[Block]) -> Vec<u64> {
    let mut sink = tapeti_core::audio::RecordingSink::default();
    for b in crate::tape::playable(blocks).iter() {
        tapeti_core::audio::emit_block(&mut sink, b);
    }
    let mut pulses: Vec<u64> = Vec::new();
    let mut level = None;
    for (tstates, l) in sink.pulses {
        match pulses.last_mut() {
            Some(last) if level == Some(l) => *last += tstates,
            _ => pulses.push(tstates),
        }
        level = Some(l);
    }
    pulses
}

/// Decode the signal of `run` and put what it holds in its place, as one edit. `Ok` is
/// how many blocks there are now; `Err` says why nothing was changed, above all that
/// the decoder found no more in the signal than was already there.
pub fn decode_run(
    store: &mut Store,
    side: Side,
    run: std::ops::Range<usize>,
    loader: Option<&'static str>,
) -> Result<(usize, Reading), String> {
    let choice = DecodeChoice { loader, ..DecodeChoice::default() };
    backend::check(choice)?;
    let t = store.tape(side);
    let replaced = &t.blocks[run.clone()];
    let label = format!("blocks of {}", t.name);
    let pulses = pulses_of(replaced);
    let what = if run.len() == 1 { "This block".to_string() } else { format!("These {} blocks", run.len()) };
    if let Some(why) = too_little_signal(&what, &pulses) {
        return Err(why);
    }
    let decoded =
        backend::decode_pulses(&label, &pulses, TSTATE_GRID_HZ, choice).map_err(|e| e.to_string())?;
    let read_as = choice.loader;

    // Raw signal in, raw signal out: the decoder kept everything through its floor,
    // which is the same tape in other blocks. That is not worth an edit.
    if decoded.floor_only {
        return Err(format!(
            "{what}, read with {}, gave no data blocks, only {}: the same signal in other blocks.",
            reading_title(read_as),
            block_counts(&decoded.blocks),
        ));
    }

    // What the sub-decode knows is about a signal that starts at zero. Where the tape
    // knows where the first replaced block lay in its recording, positions are moved
    // there; where it does not, there is no recording to have a position in.
    let mut found = provenance(&label, choice, &decoded);
    let source = t.recording.as_ref().and_then(|r| r.source_of(replaced[0].uid));
    let base = source.map(|r| r.extents[&replaced[0].uid].0);
    let source = source.map(|r| r.path.clone());
    match base {
        Some(base) => found.extents.values_mut().for_each(|e| *e = (e.0 + base, e.1 + base)),
        None => found.extents.clear(),
    }
    let count = decoded.blocks.len();
    if !store.splice_decoded(side, run, decoded.blocks) {
        return Err("The tape is locked.".to_string());
    }
    if let Some(own) = &mut store.tape_mut(side).recording {
        match source {
            Some(path) => own.absorb_from(&path, found),
            None => own.absorb(found),
        }
    }
    Ok((count, read_as))
}

/// Fewer edges than this is no tape block: the shortest pilot is hundreds of them.
const FEWEST_EDGES: usize = 64;

/// Why a signal is not worth handing to the decoder, when it is not: a lead-in or a
/// gap recorded as it was, silence at one level, which is what a Direct Recording at
/// the start of a tape often is. Said explicitly, since "no data blocks" alone would
/// leave the reader to work it out.
fn too_little_signal(what: &str, pulses: &[u64]) -> Option<String> {
    let secs = pulses.iter().sum::<u64>() as f64 / f64::from(TSTATE_GRID_HZ);
    match pulses.len() {
        0 | 1 => Some(format!("{what} is silence: {secs:.1} s at one level, with no signal in it to read.")),
        n if n < FEWEST_EDGES => {
            Some(format!("{what} holds {n} pulses in {secs:.1} s, too few for any tape block to be in it."))
        }
        _ => None,
    }
}

/// "2 Pause, 5 Pulse sequence": what a decode found, by block type.
fn block_counts(found: &[Block]) -> String {
    let mut counts: Vec<(u8, usize)> = Vec::new();
    for b in found {
        match counts.iter_mut().find(|(id, _)| *id == b.id()) {
            Some((_, n)) => *n += 1,
            None => counts.push((b.id(), 1)),
        }
    }
    if counts.is_empty() {
        return "nothing".into();
    }
    let names = counts
        .iter()
        .map(|(id, n)| format!("{n} {}", tapeti_core::types::block_name(*id).unwrap_or("Unknown")));
    names.collect::<Vec<_>>().join(", ")
}

/// The dialog's state: which blocks, and the loader to read them as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeRunState {
    pub side: Side,
    pub loader: Option<&'static str>,
}

/// The dialog's body; returns whether it is done with. Only the loader is asked: a
/// channel and a filter belong to a capture, and this signal is past both.
pub fn decode_run_body(
    ui: &mut egui::Ui,
    store: &mut Store,
    s: &mut DecodeRunState,
    tok: &crate::theme::Tokens,
) -> bool {
    use crate::widgets as w;
    let Some(run) = signal_run(store, s.side) else { return true };
    let n = run.len();
    ui.label(format!(
        "Read the signal of {} as a tape, and replace {} with the blocks it holds.",
        if n == 1 { "this block".to_string() } else { format!("these {n} blocks") },
        if n == 1 { "it" } else { "them" },
    ));
    ui.add_space(6.0);
    loader_row(ui, tok);
    w::note(ui, tok, "Nothing is changed unless data blocks are found. The change can be undone.");

    let mut done = false;
    crate::dialogs::footer(ui, |ui| {
        if ui.button("Decode").clicked() {
            match decode_run(store, s.side, run, s.loader) {
                Ok((count, read_as)) => store.set_status(format!(
                    "Decoded {n} block{} into {count}, read as {}",
                    if n == 1 { "" } else { "s" },
                    reading_title(read_as),
                )),
                Err(why) => store.message("Nothing decoded", vec![why]),
            }
            done = true;
        }
        if ui.button("Cancel").clicked() {
            done = true;
        }
    });
    done
}

// ---- decoding one stretch of the recording again -------------------------------------
//
// The audio-domain counterpart of decode entry, for a block that decoded badly rather
// than not at all: the recording is still there, the block knows where it lay, and the
// decoder can decode just that window, with positions returned on the whole file's grid.
// One block of an hour of tape can then be retried with another loader, channel or
// filter for the cost of its own few seconds. The decoder also measures its thresholds
// over the window alone, so a quiet passage sometimes reads on its own where it did not
// as part of the whole tape.

/// How far past the run's own extent the window reaches, where there is room: a read
/// that starts a little early costs nothing, and one that ends a little late is what
/// the decoder asks for ("a block at the window's edge is cut").
const STRETCH_MARGIN_MS: u32 = 1000;

/// A run of blocks and the stretch of the recording they came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stretch {
    pub first: Uid,
    pub last: Uid,
    pub blocks: usize,
    pub from_ms: u32,
    pub to_ms: u32,
}

/// What "decode this stretch again" would act on in `side`: the selection, if it is one
/// unbroken run on a tape that still has its recording, and both ends of it are known.
/// The window is the run's extent and a margin, but never into a neighbour that is
/// staying: what the window holds replaces the run, and half of a neighbour decoded a
/// second time is not something to splice in beside the whole of it.
pub fn stretch_of(store: &Store, side: Side) -> Option<Stretch> {
    let t = store.tape(side);
    let mut picked = t.unit_indices(t.cursor);
    picked.sort_unstable();
    let (first, last) = (*picked.first()?, *picked.last()?);
    if last - first + 1 != picked.len() {
        return None;
    }
    // The recording the run came from: the tape's own, or one that was inserted.
    // Positions are read in that one alone, the neighbours' included: a block from
    // another recording lies somewhere else entirely, in a file this window is not in.
    let recording = t.recording.as_ref()?.source_of(t.blocks[first].uid)?;
    recording.rate_hz?;
    let extent = |i: usize| recording.extents.get(&t.blocks[i].uid).copied();
    let (start, end) = (extent(first)?.0, extent(last)?.1);
    let before = (0..first).rev().find_map(extent).map_or(0, |e| e.1);
    let after = (last + 1..t.blocks.len()).find_map(extent).map_or(u32::MAX, |e| e.0);
    Some(Stretch {
        first: t.blocks[first].uid,
        last: t.blocks[last].uid,
        blocks: picked.len(),
        from_ms: start.saturating_sub(STRETCH_MARGIN_MS).max(before.min(start)),
        to_ms: end.saturating_add(STRETCH_MARGIN_MS).min(after.max(end)),
    })
}

/// `0:12.3–0:17.1`, as the editor footer writes a position in the recording.
fn stretch_label(from_ms: u32, to_ms: u32) -> String {
    let time = |ms: u32| crate::fmt::time(f64::from(ms) / 1000.0);
    format!("{}–{}", time(from_ms), time(to_ms))
}

/// The rows of a window's result that are its cut, not tape: a cut pulse that fits no
/// block can come back as a raw block in front of the first one read, depending on a
/// few samples of alignment. The decoder names which rows those are ([`Decode::edges`]):
/// only the rows of an edge span are dropped, never rows guessed by position, since a
/// real burst that began at the edge would be shaved by that guess.
///
/// An empty run is not a row: it means the span was folded into the pause of the block
/// before it, which stays.
fn edge_rows(d: &Decode) -> Vec<usize> {
    let mut rows: Vec<usize> = d.edges.iter().filter_map(|e| e.rows.clone()).flatten().collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// Rows of a window's result that lie wholly outside the run it was cut for, `run` being
/// the run's own place in the recording in milliseconds. The margin stops at a neighbour
/// whose place is known; where it is not, a second of margin can reach into the end of a
/// neighbour's ROM pilot, which is long enough that the decoder reads that whole
/// neighbour again. It is still on the tape, so splicing it in would put it there twice.
fn neighbours_read_again(d: &Decode, run: (u32, u32)) -> Vec<usize> {
    // Positions were kept in whole milliseconds; a block that ends where the run
    // begins may read a little past it.
    const ROUNDING_MS: f64 = 5.0;
    let Some(rate) = d.rate_hz.map(f64::from) else { return Vec::new() };
    let ms = |at: u64| at as f64 / rate * 1000.0;
    (0..d.blocks.len())
        .filter(|i| {
            d.extents.get(*i).and_then(Option::as_ref).is_some_and(|e| {
                ms(e.end) <= f64::from(run.0) + ROUNDING_MS || ms(e.start) >= f64::from(run.1) - ROUNDING_MS
            })
        })
        .collect()
}

/// A finished decode of a stretch, put where the run was, if the run is still there.
fn finish_stretch(store: &mut Store, job: &Job, d: Decode, first: Uid, last: Uid) -> bool {
    let t = store.tape(job.side);
    let at = |uid: Uid| t.blocks.iter().position(|b| b.uid == uid);
    let run = match (at(first), at(last)) {
        (Some(i), Some(j)) if i <= j => i..j + 1,
        _ => {
            let why = "The blocks it was decoded for are no longer on the tape.".to_string();
            store.message("Nothing replaced", vec![format!("{}: {why}", job.name)]);
            return false;
        }
    };
    // The same bar as decode entry: signal kept through the floor is the recording
    // again in other blocks, and is not worth an edit.
    if d.floor_only {
        store.message("Nothing decoded", vec![format!("No data blocks were found in {}.", job.name)]);
        return false;
    }
    let found = provenance(&job.name, job.choice, &d);
    // Where the run itself lies, as the tape knew it before this decode: what the window
    // also read outside that is a neighbour, decoded a second time.
    let own = store.tape(job.side).recording.as_ref().and_then(|r| r.source_of(first));
    let lies = own.and_then(|r| Some((r.extents.get(&first)?.0, r.extents.get(&last)?.1)));
    let mut dropped = edge_rows(&d);
    let again = lies.map_or_else(Vec::new, |run| neighbours_read_again(&d, run));
    dropped.extend(&again);
    dropped.sort_unstable();
    dropped.dedup();
    let cut = overreach(&d);
    let (valid, path) = (d.checksums_valid, d.path);
    let mut decoded = d.blocks;
    // Highest first, so the rows still to go keep their numbers.
    for row in dropped.into_iter().rev() {
        decoded.remove(row);
    }
    let data = decoded.iter().filter(|b| b.body.is_data_block()).count();
    let (was, now) = (run.len(), decoded.len());
    if !store.splice_decoded(job.side, run, decoded) {
        return false; // locked, and `commit` has said so
    }
    if let Some(own) = &mut store.tape_mut(job.side).recording {
        own.absorb_from(&path, found);
    }
    store.set_status(format!(
        "Decoded {} again as {}: {was} block{} became {now}, {valid} of {data} data blocks check out{}",
        job.name,
        reading_title(job.choice.loader),
        if was == 1 { "" } else { "s" },
        match again.len() {
            0 => cut,
            n => format!(
                " — the window also read {n} neighbouring block{}, left as it was",
                if n == 1 { "" } else { "s" }
            ),
        },
    ));
    true
}

/// More pulses than this at an edge of a window are signal the window cut, not the
/// silence or the single cut pulse every window has there.
const CUT_IS_SIGNAL: usize = 8;

/// What to say when the window reached into signal it did not take. The margin stops
/// at a neighbour whose place in the recording is known; where it is not known (the
/// block was edited into being, or pasted in), a second of margin can reach into it.
/// The decoder counts what the window left at each edge: that signal is still on the
/// tape, in the neighbour, but the stretch may have been read with half of something
/// else in it.
fn overreach(d: &Decode) -> String {
    let cut = |at: Edge| -> usize {
        let spans = d.edges.iter().filter(|e| e.edge == at).map(|e| e.pulses);
        spans.filter(|pulses| *pulses > CUT_IS_SIGNAL).sum()
    };
    match (cut(Edge::Lead), cut(Edge::Trail)) {
        (0, 0) => String::new(),
        (lead, 0) => format!(" — the window cut into {lead} pulses of signal before it"),
        (0, trail) => format!(" — the window cut into {trail} pulses of signal after it"),
        (lead, trail) => format!(" — the window cut into signal on both sides ({lead} and {trail} pulses)"),
    }
}

// ---- decoding in the background ------------------------------------------------

/// Ask before a decode that the decoder estimates will peak above this. It holds the
/// whole capture as samples: an hour of 44.1 kHz stereo is about 3 GB.
pub const WARN_ABOVE_BYTES: u64 = 1 << 30;

/// One recording being decoded.
pub struct Job {
    pub side: Side,
    pub name: String,
    into: Landing,
    choice: DecodeChoice,
    /// The pane's tape when the decode started: if it is another one by the time
    /// the result arrives, the pane was edited or reloaded meanwhile.
    base_gen: u64,
    cancel: Cancel,
    /// How far the decode has come, for the status bar. Feedback only: the
    /// decoder's final listing is the authority, and may differ.
    seen: Arc<Seen>,
    result: Receiver<Result<Decode, DecodeError>>,
    started: Instant,
}

impl Job {
    pub fn blocks_so_far(&self) -> usize {
        self.seen.blocks.load(Ordering::Relaxed)
    }

    /// The share of the decode done, in thousandths.
    pub fn thousandths_done(&self) -> u32 {
        self.seen.done.load(Ordering::Relaxed)
    }
}

/// What a decode thread has reported, read by the UI thread.
#[derive(Default)]
struct Seen {
    blocks: AtomicUsize,
    /// In thousandths.
    done: AtomicU32,
}

/// The decodes in flight: at most one per pane.
pub struct Jobs {
    jobs: Vec<Job>,
    /// [`WARN_ABOVE_BYTES`], as a field so a test can ask about the question
    /// without a gigabyte of audio.
    pub warn_above: u64,
}

impl Default for Jobs {
    fn default() -> Self {
        Jobs { jobs: Vec::new(), warn_above: WARN_ABOVE_BYTES }
    }
}

impl Jobs {
    /// The decode to show in the status bar: the active pane's, else any.
    pub fn showing(&self, active: Side) -> Option<&Job> {
        self.jobs.iter().find(|j| j.side == active).or_else(|| self.jobs.first())
    }

    /// The decode that will land in `side`, if there is one.
    pub fn landing_in(&self, side: Side) -> Option<&Job> {
        self.jobs.iter().find(|j| j.side == side)
    }

    /// Stop the decode into `side`. The thread ends soon after, before the next block it would
    /// read or the next sample of a pass over sampled audio, but nothing here waits for it: the
    /// job is forgotten now, and the thread finds nobody listening when it ends.
    pub fn cancel(&mut self, side: Side) -> bool {
        let before = self.jobs.len();
        self.jobs.retain(|j| {
            if j.side == side {
                j.cancel.cancel();
            }
            j.side != side
        });
        self.jobs.len() != before
    }

    /// Once a frame, before the row caches are rebuilt: start what `files.rs`
    /// queued, and load what has finished. Returns whether a pane changed.
    pub fn pump(&mut self, store: &mut Store, ctx: &egui::Context) -> bool {
        for request in std::mem::take(&mut store.recordings) {
            self.start(store, ctx, request, Cancel::new());
        }
        let mut changed = false;
        let mut i = 0;
        while i < self.jobs.len() {
            match self.jobs[i].result.try_recv() {
                Err(TryRecvError::Empty) => i += 1,
                Ok(result) => {
                    let job = self.jobs.remove(i);
                    changed |= finish(store, job, result);
                }
                // The thread died without an answer: a panic inside the decode,
                // which `crashlog.rs` has written down.
                Err(TryRecvError::Disconnected) => {
                    let job = self.jobs.remove(i);
                    store.message(
                        "Cannot decode recording",
                        vec![format!("{}: the decoder stopped unexpectedly.", job.name)],
                    );
                }
            }
        }
        changed
    }

    fn start(&mut self, store: &mut Store, ctx: &egui::Context, request: RecordingRequest, cancel: Cancel) {
        let name = match request.into {
            Landing::Stretch { from_ms, to_ms, .. } => {
                format!("{} of {}", stretch_label(from_ms, to_ms), file_name(&request.path))
            }
            _ => file_name(&request.path),
        };
        if !request.confirmed {
            match backend::peak_bytes(&request.path) {
                Ok(Some(peak)) if peak > self.warn_above => {
                    let lines = vec![
                        format!("Decoding \"{name}\" will need about {} of memory.", gigabytes(peak)),
                        "The whole recording is held in memory while it is decoded.".to_string(),
                    ];
                    store.dialog =
                        Some(Dialog::confirm("Decode this recording?", lines, Then::Decode(request)));
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    store.message("Cannot decode recording", vec![format!("{name}: {e}")]);
                    return;
                }
            }
        }
        // One decode per pane: a second recording opened into it replaces the first.
        self.cancel(request.side);

        let seen = Arc::new(Seen::default());
        let (send, result) = channel();
        // Asked here, where a wrong name can be said to the user.
        if let Err(e) = backend::check(request.choice) {
            store.message("Cannot decode recording", vec![e]);
            return;
        }
        let mut window = None;
        if let Landing::Stretch { from_ms, to_ms, .. } = request.into {
            // Milliseconds here, the decoder's own grid there.
            let source = store.tape(request.side).recording.as_ref();
            let source = source.and_then(|r| r.sources().find(|s| s.path == request.path));
            let Some(rate) = source.and_then(|s| s.rate_hz) else {
                store.message(
                    "Cannot decode recording",
                    vec![format!("{name}: its positions are not known.")],
                );
                return;
            };
            let on_grid = |ms: u32| u64::from(ms) * u64::from(rate) / 1000;
            window = Some(on_grid(from_ms)..on_grid(to_ms));
        }
        let worker = Worker {
            path: request.path.clone(),
            choice: request.choice,
            window,
            cancel: cancel.clone(),
            seen: seen.clone(),
            ctx: ctx.clone(),
        };
        std::thread::Builder::new()
            .name("decode".into())
            .spawn(move || {
                // Nobody listening means the job was cancelled and forgotten.
                let _ = send.send(worker.run());
                worker.ctx.request_repaint();
            })
            .expect("spawn the decode thread");
        store.set_status(format!("Decoding {name}…"));
        self.jobs.push(Job {
            side: request.side,
            name,
            into: request.into,
            choice: request.choice,
            base_gen: store.tape(request.side).generation(),
            cancel,
            seen,
            result,
            started: Instant::now(),
        });
    }
}

/// What the decode thread owns.
struct Worker {
    path: PathBuf,
    /// Checked on the UI thread, where a bad loader name can be said to the user.
    choice: DecodeChoice,
    /// The stretch to read, on the recording's grid; `None` reads all of it.
    window: Option<Range<u64>>,
    cancel: Cancel,
    seen: Arc<Seen>,
    ctx: egui::Context,
}

impl Worker {
    fn run(&self) -> Result<Decode, DecodeError> {
        let mut heard = |p: Progress| {
            let done = (p.done.clamp(0.0, 1.0) * 1000.0) as u32;
            let blocks = self.seen.blocks.swap(p.blocks, Ordering::Relaxed);
            // A frame only when what the status bar says changes.
            if self.seen.done.swap(done, Ordering::Relaxed) != done || blocks != p.blocks {
                self.ctx.request_repaint();
            }
        };
        backend::decode_file(&self.path, self.choice, self.window.clone(), &mut heard, &self.cancel)
    }
}

/// Put a finished decode where it was asked for. Returns whether a pane changed.
fn finish(store: &mut Store, job: Job, result: Result<Decode, DecodeError>) -> bool {
    let decoded = match result {
        Ok(decoded) => decoded,
        Err(DecodeError::Cancelled) => return false,
        Err(e) => {
            store.message("Cannot decode recording", vec![format!("{}: {e}", job.name)]);
            return false;
        }
    };
    if let Landing::Stretch { first, last, .. } = job.into {
        return finish_stretch(store, &job, decoded, first, last);
    }
    let recording = provenance(&job.name, job.choice, &decoded);
    let decoded = decoded.blocks;
    let count = decoded.len();
    let took = job.started.elapsed().as_secs_f64();
    store.set_status(format!("Decoded {}: {count} blocks in {took:.1} s", job.name));
    if job.into == Landing::Replace {
        return store.replace_decoded(job.side, decoded, recording);
    }
    if job.into == Landing::Cursor {
        let t = store.tape(job.side);
        let at = if t.cursor < 0 { t.blocks.len() } else { t.cursor as usize };
        store.insert_blocks(job.side, at, decoded);
        // The blocks came from this recording, not the tape's own (if it has one), so
        // they keep their own file and positions instead of becoming the tape's
        // recording: otherwise "Decode Recording Again" on a TZX with a WAV inserted
        // would replace the tape with the WAV.
        let path = recording.path.clone();
        store
            .tape_mut(job.side)
            .recording
            .get_or_insert_with(Recording::default)
            .absorb_from(&path, recording);
        store.touch_view();
        return true;
    }
    // Opening asked about unsaved work before the decode began. If the pane has
    // been edited since, that answer is stale: ask again, holding the result.
    let t = store.tape(job.side);
    if t.generation() != job.base_gen && t.dirty() {
        let lines = vec![format!(
            "\"{}\" is decoded, but the {} tape \"{}\" has unsaved changes.",
            job.name,
            if job.side == 0 { "left" } else { "right" },
            t.name
        )];
        let then = Then::LoadDecoded {
            side: job.side,
            name: job.name,
            blocks: decoded,
            recording: Box::new(recording),
        };
        store.dialog = Some(Dialog::confirm("Discard changes?", lines, then));
        return false;
    }
    store.tape_mut(job.side).load_unsaved(job.name, decoded, Some(recording));
    store.active = job.side;
    true
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / f64::from(1u32 << 30))
}
#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::path::Path;
    use tapeti_core::audio::{encode_wav, playback_order, render_tape, FlowOptions, RenderOptions};

    use crate::app::App;
    use crate::menu::Menu;
    use crate::settings::Settings;

    /// A ROM block: flag, payload, and the XOR of both as its checksum.
    pub(super) fn rom_block(flag: u8, payload: &[u8]) -> Vec<u8> {
        let mut data = vec![flag];
        data.extend_from_slice(payload);
        data.push(data.iter().fold(0, |x, b| x ^ b));
        data
    }

    /// A header for `len` bytes of CODE at 32768, named "synthetic".
    pub(super) fn header(len: u16) -> Vec<u8> {
        let mut h = vec![3];
        h.extend_from_slice(b"synthetic ");
        h.extend_from_slice(&len.to_le_bytes());
        h.extend_from_slice(&32768u16.to_le_bytes());
        h.extend_from_slice(&32768u16.to_le_bytes());
        rom_block(0x00, &h)
    }

    pub(super) fn standard_data(blocks: &[Block]) -> Vec<&Vec<u8>> {
        blocks
            .iter()
            .filter_map(|b| match &b.body {
                Body::Standard { data, .. } => Some(data),
                _ => None,
            })
            .collect()
    }

    /// A two-block tape, and the WAV the core renders it to.
    pub(super) fn synthetic_recording() -> (Vec<Block>, Vec<u8>, f64) {
        let code: Vec<u8> = (0..600u16).map(|i| (i * 7 % 251) as u8).collect();
        let tape = vec![
            Block::new(Body::Standard { pause: 1000, data: header(code.len() as u16) }),
            Block::new(Body::Standard { pause: 2000, data: rom_block(0xff, &code) }),
        ];
        let wav = render_wav(&tape);
        // A 16-bit mono WAV: a 44-byte header, then two bytes a sample.
        let seconds = (wav.len() - 44) as f64 / 2.0 / 44_100.0;
        (tape, wav, seconds)
    }

    /// `tape` as the core plays it, into a 44.1 kHz WAV.
    pub(super) fn render_wav(tape: &[Block]) -> Vec<u8> {
        let order = playback_order(tape, FlowOptions::default());
        let options = RenderOptions { sample_rate: 44_100, mic: true, amplitude: 0.8 };
        encode_wav(&render_tape(tape, options, &order), 44_100, 16)
    }

    // ---- the Open path -------------------------------------------------------

    /// The synthetic recording as a file, removed again when the test ends.
    struct TempWav(PathBuf);

    impl TempWav {
        fn new(test: &str) -> TempWav {
            let path = std::env::temp_dir().join(format!("tapeti-{}-{test}.wav", std::process::id()));
            std::fs::write(&path, synthetic_recording().1).expect("write the synthetic recording");
            TempWav(path)
        }
    }

    impl Drop for TempWav {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Opens `path` the way every route to a recording does, through `files::open_paths` and
    /// its dialog, and answers the dialog with its default: decode it.
    fn open(store: &mut Store, side: Side, path: &Path, insert_at_cursor: bool) {
        crate::files::open_paths(store, side, &[path.to_path_buf()], insert_at_cursor);
        match store.dialog.take() {
            Some(Dialog::Recording(s)) => {
                assert!(s.decode, "the dialog offers to decode");
                crate::files::decode_recording(store, side, path, DecodeChoice::default(), insert_at_cursor);
            }
            _ => panic!("opening a recording asks how first"),
        }
    }

    fn app() -> (egui::Context, App) {
        let ctx = egui::Context::default();
        let store = Store::new(Settings::default());
        let app = App::build(&ctx, Menu::headless(), store, Instant::now(), 0, false);
        (ctx, app)
    }

    fn draw(ctx: &egui::Context, app: &mut App) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
            ..Default::default()
        };
        ctx.run_ui(input, |ui| app.frame(ui)).drop_without_applying_deltas();
    }

    /// Draw frames until no decode is running, the way the window would.
    fn draw_until_decoded(ctx: &egui::Context, app: &mut App) {
        let deadline = Instant::now() + std::time::Duration::from_secs(60);
        loop {
            draw(ctx, app);
            if app.recordings.jobs.is_empty() && app.store.recordings.is_empty() {
                return;
            }
            assert!(Instant::now() < deadline, "the decode never finished");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_recording_is_told_from_a_tape_by_its_extension() {
        use crate::files::{is_recording, stem};
        assert!(is_recording("side a.WAV") && is_recording("x.csw"));
        assert!(!is_recording("game.tzx") && !is_recording("wav"));
        assert_eq!(stem("side a.wav"), "side a", "Save offers side a.tzx, not side a.wav.tzx");
        assert_eq!(stem("game.tzx"), "game");
    }

    /// Opens through `files::open_paths`, the entry point every route to a recording uses.
    #[test]
    fn opening_a_recording_decodes_it_into_the_pane() {
        let wav = TempWav::new("open");
        let (ctx, mut app) = app();
        open(&mut app.store, 1, &wav.0, false);
        assert_eq!(app.store.recordings.len(), 1, "opening only queues the decode");
        assert!(app.store.tape(1).blocks.is_empty());

        draw(&ctx, &mut app);
        assert_eq!(app.recordings.showing(0).map(|j| j.side), Some(1), "the status bar has a decode to show");
        draw_until_decoded(&ctx, &mut app);
        draw(&ctx, &mut app);

        let t = app.store.tape(1);
        assert_eq!(standard_data(&t.blocks), standard_data(&synthetic_recording().0));
        assert_eq!(t.name, crate::files::file_name(&wav.0));
        assert!(t.path.is_none(), "Save must not write a TZX over the recording");
        assert!(t.dirty(), "a decode that was never saved is unsaved work");
        assert_eq!(app.store.active, 1);
        assert!(app.store.dialog.is_none());
        assert_eq!(app.rows[1].rows.len(), t.blocks.len(), "the rows describe the decoded tape");
    }

    #[test]
    fn a_recording_can_be_inserted_at_the_cursor() {
        let wav = TempWav::new("insert");
        let (ctx, mut app) = app();
        app.store.tape_mut(0).load("t.tzx".into(), None, vec![Block::new(Body::Pause { pause: 1 })], None);
        open(&mut app.store, 0, &wav.0, true);
        draw_until_decoded(&ctx, &mut app);
        let t = app.store.tape(0);
        assert_eq!(t.name, "t.tzx", "inserting keeps the tape it was inserted into");
        assert!(t.blocks.len() > 1 && t.can_undo(), "the insert is an edit, and undoable");
    }

    /// A cancelled decode ends in `Cancelled`, not shown as an error, and `cancel`
    /// forgets the job at once rather than waiting for the thread to notice.
    #[test]
    fn a_cancelled_decode_leaves_the_pane_alone() {
        let wav = TempWav::new("cancel");
        let (ctx, mut app) = app();
        let request = RecordingRequest {
            side: 0,
            path: wav.0.clone(),
            into: Landing::Tape,
            choice: DecodeChoice::default(),
            confirmed: true,
        };

        // Stopped before it starts: the decode's first poll sees it.
        let stopped = Cancel::new();
        stopped.cancel();
        app.recordings.start(&mut app.store, &ctx, request.clone(), stopped);
        draw_until_decoded(&ctx, &mut app);
        assert!(app.store.tape(0).blocks.is_empty() && app.store.dialog.is_none());

        // Stopped from the status bar: gone this frame, whatever the thread does.
        app.recordings.start(&mut app.store, &ctx, request, Cancel::new());
        assert!(app.recordings.cancel(0));
        assert!(app.recordings.showing(0).is_none());
        std::thread::sleep(std::time::Duration::from_millis(300));
        draw(&ctx, &mut app);
        assert!(app.store.tape(0).blocks.is_empty(), "a forgotten decode still reached the pane");
    }

    /// Above the threshold the decode waits for an answer, and "yes" is a
    /// `Then`, like every other confirmation in the app.
    #[test]
    fn a_recording_that_needs_a_lot_of_memory_asks_first() {
        let wav = TempWav::new("memory");
        let (ctx, mut app) = app();
        app.recordings.warn_above = 0;
        open(&mut app.store, 0, &wav.0, false);
        draw(&ctx, &mut app);
        assert!(app.recordings.showing(0).is_none(), "the decode started without asking");
        let Some(Dialog::Confirm { then, .. }) = app.store.dialog.take() else {
            panic!("no question was asked");
        };
        crate::actions::run_then(&mut app, then);
        draw_until_decoded(&ctx, &mut app);
        assert!(!app.store.tape(0).blocks.is_empty(), "agreeing did not start the decode");
    }

    /// Opening asks about unsaved work before the decode; an edit made while it
    /// runs makes that answer stale, and the result waits for a new one.
    #[test]
    fn a_pane_edited_during_the_decode_is_asked_about_again() {
        let wav = TempWav::new("edited");
        let (ctx, mut app) = app();
        open(&mut app.store, 0, &wav.0, false);
        draw(&ctx, &mut app);
        app.store.insert_blocks(0, 0, vec![Block::new(Body::Pause { pause: 7 })]);
        draw_until_decoded(&ctx, &mut app);
        assert_eq!(app.store.tape(0).blocks.len(), 1, "the edit was overwritten without asking");
        let Some(Dialog::Confirm { then, .. }) = app.store.dialog.take() else {
            panic!("no question was asked");
        };
        crate::actions::run_then(&mut app, then);
        assert_eq!(standard_data(&app.store.tape(0).blocks).len(), 2);
    }

    // ---- what the decoder had to say --------------------------------------------

    /// A decode put together by hand: three blocks that know where they lie but the
    /// last, a stretch that did not decode and the row it was kept as, a repair to
    /// the first, and a finding about the whole recording.
    fn decode_with_findings() -> Decode {
        let finding = |class, text: &str, row| Finding { class, text: text.into(), row };
        Decode {
            path: PathBuf::from("findings.wav"),
            blocks: vec![
                Block::new(Body::Standard { pause: 1000, data: header(4) }),
                Block::new(Body::PulseSeq { pulses: vec![300, 400, 500] }),
                Block::new(Body::Standard { pause: 0, data: rom_block(0xff, &[1, 2, 3, 4]) }),
            ],
            rate_hz: Some(44_100),
            extents: vec![Some(0..44_100), Some(44_100..88_200), None],
            findings: vec![
                finding(NoteClass::Info, "kept as raw signal", Some(0)),
                finding(NoteClass::Problem, "undecoded signal at 0:01.1: 3 pulses kept", Some(1)),
                finding(NoteClass::Info, "the recording is mono", None),
            ],
            notes: vec!["read as WAV".into()],
            measured: vec![("signal".into(), vec!["44100 Hz, mono".into()])],
            ..Decode::default()
        }
    }

    #[test]
    fn findings_land_on_the_blocks_they_are_about() {
        let decoded = decode_with_findings();
        let recording = provenance("findings.wav", DecodeChoice::default(), &decoded);
        let uid = |i: usize| decoded.blocks[i].uid;

        let about =
            |class| recording.notes.iter().filter(|n| n.class == class).map(|n| n.block).collect::<Vec<_>>();
        assert_eq!(about(NoteClass::Problem), [Some(uid(1))], "the finding named the block");
        assert_eq!(about(NoteClass::Info), [Some(uid(0)), None], "None is about the recording as a whole");
        assert_eq!(recording.report[0], "read as WAV", "the load's own notes come first");
        assert_eq!(recording.report.len(), 4);
        assert_eq!(recording.extents[&uid(1)], (1000, 2000));
        assert!(!recording.extents.contains_key(&uid(2)), "the capture ends inside the last block");
        assert!(recording.measured.iter().any(|(axis, _)| axis == "signal"), "the measured rows came along");
    }

    /// The decoder's problems are consistency warnings, on the row they are about,
    /// and they stay with that block when the tape is edited around it.
    #[test]
    fn findings_are_consistency_issues_that_follow_their_block() {
        use tapeti_core::consistency::Severity;
        let decoded = decode_with_findings();
        let recording = provenance("findings.wav", DecodeChoice::default(), &decoded);
        let mut store = Store::new(Settings::default());
        store.tape_mut(0).load_unsaved("findings.wav".into(), decoded.blocks, Some(recording));

        let from_recording = |store: &Store| -> Vec<(i32, Severity)> {
            let issues = store.tape(0).issues(0);
            issues
                .iter()
                .filter(|i| i.message.starts_with("Recording: "))
                .map(|i| (i.block, i.severity))
                .collect()
        };
        assert_eq!(from_recording(&store), [(1, Severity::Warning)], "information is not an issue");

        store.insert_blocks(0, 0, vec![Block::new(Body::Pause { pause: 1 })]);
        assert_eq!(from_recording(&store), [(2, Severity::Warning)], "the notes did not move");
        store.delete_indices(0, vec![2]);
        assert_eq!(from_recording(&store), [], "a deleted block kept its warning");
        store.undo(0);
        assert_eq!(from_recording(&store).len(), 1, "undo brings the block back, and what is known about it");
    }

    /// The three places a decoded tape shows where it came from, drawn: the row
    /// mark, Tape Info's section and the consistency dialog, and the editor's
    /// footer with the cursor on a block that has an extent.
    #[test]
    fn a_decoded_tape_draws_what_it_knows() {
        // `TAPETI_SHOTS=dir cargo test …` leaves the pictures there to be looked
        // at. Each is drawn by an app of its own: a second capture on one
        // context has lost the font texture the first one was given.
        let shots = std::env::var_os("TAPETI_SHOTS").map(PathBuf::from);
        let views =
            [("main", None), ("info", Some(Dialog::tape_info(0))), ("check", Some(Dialog::consistency(0)))];
        for (name, dialog) in views {
            let decoded = decode_with_findings();
            let recording = provenance("findings.wav", DecodeChoice::default(), &decoded);
            let (ctx, mut app) = app();
            app.store.tape_mut(0).load_unsaved("findings.wav".into(), decoded.blocks, Some(recording));
            app.store.set_cursor(0, 1, crate::state::SelectMode::Single);
            app.store.dialog = dialog;
            // Enough frames for a dialog to finish fading in.
            let canvas = crate::shot::capture(&ctx, &mut app, (1100.0, 720.0), 12);
            assert!(app.rows[0].issues.contains_key(&1), "the undecoded block has no mark in the list");
            assert!(!app.rows[0].issues.contains_key(&0), "a repair is not a warning");
            if let Some(dir) = &shots {
                std::fs::write(dir.join(format!("recording-{name}.png")), canvas.to_png())
                    .expect("write the picture");
            }
        }
    }

    // ---- decoding again ----------------------------------------------------------

    /// Decode, then decode again with a loader named: the blocks are replaced as one
    /// edit, the tape remembers how it was decoded (what the dialog reopens on, and
    /// what the measured rows report), and undo brings back the first decode.
    #[test]
    fn decoding_again_replaces_the_blocks_and_can_be_undone() {
        let wav = TempWav::new("again");
        let (ctx, mut app) = app();
        assert!(!can_redecode(&app.store, &app.recordings, 0), "an empty pane has nothing to decode again");
        open(&mut app.store, 0, &wav.0, false);
        draw_until_decoded(&ctx, &mut app);
        assert!(can_redecode(&app.store, &app.recordings, 0));
        assert!(!can_redecode(&app.store, &app.recordings, 1));
        let first: Vec<_> = app.store.tape(0).blocks.iter().map(|b| b.uid).collect();
        let first_extent = app.store.tape(0).recording.as_ref().unwrap().extents[&first[0]];

        // What the dialog's Decode button pushes.
        let mut state = RedecodeState::new(&app.store, 0);
        assert_eq!(state.choice, DecodeChoice::default());
        state.choice.loader = Some("rom");
        app.store.recordings.push(RecordingRequest {
            side: 0,
            path: app.store.tape(0).recording.as_ref().unwrap().path.clone(),
            into: Landing::Replace,
            choice: state.choice,
            confirmed: false,
        });
        draw(&ctx, &mut app);
        assert!(!can_redecode(&app.store, &app.recordings, 0), "one decode per pane");
        draw_until_decoded(&ctx, &mut app);

        let t = app.store.tape(0);
        assert_eq!(standard_data(&t.blocks), standard_data(&synthetic_recording().0));
        assert!(t.blocks.iter().all(|b| !first.contains(&b.uid)), "these are new blocks");
        assert_eq!(t.name, crate::files::file_name(&wav.0), "it is still the same tape");
        let recording = t.recording.as_ref().unwrap();
        assert_eq!(RedecodeState::new(&app.store, 0).choice.loader, Some("rom"));
        let read = &recording.measured.iter().find(|(axis, _)| axis == "Loader").expect("a loader row").1;
        assert!(read.join(" ").contains("ROM"), "the report does not say which loader read it: {read:?}");

        assert!(t.can_undo());
        app.store.undo(0);
        let t = app.store.tape(0);
        assert_eq!(t.blocks.iter().map(|b| b.uid).collect::<Vec<_>>(), first);
        assert_eq!(
            t.recording.as_ref().unwrap().extents[&first[0]],
            first_extent,
            "undo lost where the block lay"
        );
    }

    /// A locked tape refuses the edit, and keeps the decode it had.
    #[test]
    fn a_locked_tape_is_not_decoded_over() {
        let decoded = decode_with_findings();
        let recording = provenance("findings.wav", DecodeChoice::default(), &decoded);
        let mut store = Store::new(Settings::default());
        store.tape_mut(0).load_unsaved(
            "findings.wav".into(),
            decoded.blocks.clone(),
            Some(recording.clone()),
        );
        store.locked = true;
        let again = decode_with_findings().blocks;
        assert!(!store.replace_decoded(0, again, Recording { name: "other".into(), ..recording.clone() }));
        assert_eq!(store.tape(0).blocks, decoded.blocks);
        assert_eq!(store.tape(0).recording.as_ref().unwrap().name, "findings.wav");
    }

    /// The dialog, drawn; and its Cancel and Decode are what close it.
    #[test]
    fn the_redecode_dialog_draws() {
        let decoded = decode_with_findings();
        let recording = provenance("findings.wav", DecodeChoice::default(), &decoded);
        let (ctx, mut app) = app();
        app.store.tape_mut(0).load_unsaved("findings.wav".into(), decoded.blocks, Some(recording));
        let mut state = RedecodeState::new(&app.store, 0);
        // The loader's line is drawn under its row, and must wrap inside the dialog
        // rather than widen it.
        state.choice.loader = Some("rom");
        app.store.dialog = Some(Dialog::Redecode(state));
        let canvas = crate::shot::capture(&ctx, &mut app, (1100.0, 720.0), 12);
        assert!(matches!(app.store.dialog, Some(Dialog::Redecode(_))), "the dialog closed by itself");
        if let Some(dir) = std::env::var_os("TAPETI_SHOTS").map(PathBuf::from) {
            std::fs::write(dir.join("recording-redecode.png"), canvas.to_png()).expect("write the picture");
        }
    }

    /// The count and share the status bar shows while a decode runs. No frame-driven test
    /// can see them, since every recording made here decodes before a frame is drawn, so the
    /// worker is run directly here and checked afterwards.
    #[test]
    fn the_worker_counts_the_blocks_the_decoder_streams() {
        let wav = TempWav::new("count");
        let worker = Worker {
            path: wav.0.clone(),
            choice: DecodeChoice::default(),
            window: None,
            cancel: Cancel::new(),
            seen: Arc::new(Seen::default()),
            ctx: egui::Context::default(),
        };
        let decoded = worker.run().expect("the synthetic recording decodes");
        let seen = worker.seen.blocks.load(Ordering::Relaxed);
        assert!(seen > 0, "nothing was counted: the status bar would say 0 blocks throughout");
        assert!(
            seen <= decoded.blocks.len() + 2,
            "{seen} counted for {} blocks: passes were added up",
            decoded.blocks.len()
        );
        assert_eq!(worker.seen.done.load(Ordering::Relaxed), 1000, "and it ends all done");
    }

    // ---- decoding a block's own signal -------------------------------------------

    /// The header and data block of the synthetic tape, as the edges they play:
    /// `(tstates, level)` with equal levels joined, the way a sampler sees them.
    fn edges_of(tape: &[Block]) -> Vec<(u64, u8)> {
        let mut sink = tapeti_core::audio::RecordingSink::default();
        for b in tape {
            tapeti_core::audio::emit_block(&mut sink, b);
        }
        let mut out: Vec<(u64, u8)> = Vec::new();
        for (t, level) in sink.pulses {
            match out.last_mut() {
                Some(last) if last.1 == level => last.0 += t,
                _ => out.push((t, level)),
            }
        }
        out
    }

    /// The tape sampled into one Direct Recording block, 79 T-states a sample
    /// (44.3 kHz), one bit a sample: TZX 0x15 as a sampler writes it.
    fn as_direct_recording(tape: &[Block]) -> Block {
        let mut bits: Vec<u8> = Vec::new();
        let (mut at, mut sampled) = (0u64, 0u64);
        for (t, level) in edges_of(tape) {
            at += t;
            while sampled * 79 < at {
                bits.push(level);
                sampled += 1;
            }
        }
        let mut data = vec![0u8; bits.len().div_ceil(8)];
        for (i, bit) in bits.iter().enumerate() {
            data[i / 8] |= bit << (7 - i % 8);
        }
        let rest = (bits.len() % 8) as u8;
        let used_bits = if rest == 0 { 8 } else { rest };
        Block::new(Body::Direct { tstates: 79, pause: 0, used_bits, data })
    }

    /// The same tape as a Z-RLE CSW block (TZX 0x18), pulse lengths in samples at
    /// 44.1 kHz. The pulses the decoder is handed are the core's rendering of it, not
    /// the file's.
    fn as_csw(tape: &[Block]) -> Block {
        let mut rle = Vec::new();
        let edges = edges_of(tape);
        for (t, _) in &edges {
            let samples = (*t as f64 * 44_100.0 / 3_500_000.0).round() as u32;
            match u8::try_from(samples) {
                Ok(n) if n > 0 => rle.push(n),
                _ => {
                    rle.push(0);
                    rle.extend_from_slice(&samples.to_le_bytes());
                }
            }
        }
        Block::new(Body::Csw {
            pause: 0,
            sample_rate: 44_100,
            compression: 2,
            pulse_count: edges.len() as u32,
            data: miniz_oxide::deflate::compress_to_vec_zlib(&rle, 6),
        })
    }

    fn store_with(blocks: Vec<Block>) -> Store {
        let mut store = Store::new(Settings::default());
        store.tape_mut(0).load("signal.tzx".into(), None, blocks, None);
        store.set_cursor(0, 0, crate::state::SelectMode::Single);
        store
    }

    /// A pause, or a run of equal samples, reaches the sink as several pieces of
    /// one pulse; a pulse is what lies between two edges. Nothing is lost by
    /// joining them, and every pulse starts where the one before it ended.
    #[test]
    fn the_pulses_of_a_block_are_whole_and_add_up_to_it() {
        let tape = synthetic_recording().0;
        let pulses = pulses_of(&tape);
        let played: u64 = tape.iter().map(crate::tape::block_duration).sum();
        assert_eq!(pulses.iter().sum::<u64>(), played);
        assert!(pulses.iter().all(|p| *p > 0));
        // The pause after each block is one long pulse, not a 1 ms piece and the rest.
        assert_eq!(pulses.iter().filter(|p| **p > 3_000_000).count(), 2);
    }

    /// Taper's decode entry, for both kinds of sampled block: the signal becomes
    /// the two blocks it holds, as one edit that undo takes back.
    #[test]
    fn a_sampled_block_decodes_into_the_blocks_it_holds() {
        let tape = synthetic_recording().0;
        for sampled in [as_direct_recording(&tape), as_csw(&tape)] {
            let kind = sampled.id();
            let mut store =
                store_with(vec![Block::new(Body::Text { text: "before".into() }), sampled.clone()]);
            store.set_cursor(0, 1, crate::state::SelectMode::Single);
            let run = signal_run(&store, 0).expect("a sampled block is signal");
            assert_eq!(run, 1..2);

            let (count, _) =
                decode_run(&mut store, 0, run, None).unwrap_or_else(|e| panic!("block {kind:02x}: {e}"));
            let t = store.tape(0);
            assert_eq!(standard_data(&t.blocks), standard_data(&tape), "block {kind:02x}");
            assert_eq!(t.blocks.len(), 1 + count);
            assert!(matches!(t.blocks[0].body, Body::Text { .. }), "the block before it was touched");
            assert_eq!(t.cursor, 1, "the cursor is on the first new block");
            assert_eq!(t.selected.len(), count, "and what came out is selected");

            store.undo(0);
            assert_eq!(store.tape(0).blocks[1], sampled, "undo did not bring the signal back");
        }
    }

    /// What a decoder leaves where it could not read (a tone, a pulse pair, a bit
    /// stream, each in a block of its own) is one signal between them: a standard
    /// block in pieces, decoded as one.
    #[test]
    fn a_run_of_raw_blocks_decodes_as_the_one_signal_it_is() {
        let head = header(4);
        let mut store = store_with(vec![
            Block::new(Body::PureTone { pulse_len: 2168, count: 8063 }),
            Block::new(Body::PulseSeq { pulses: vec![667, 735] }),
            Block::new(Body::PureData {
                zero: 855,
                one: 1710,
                used_bits: 8,
                pause: 1000,
                data: head.clone(),
            }),
        ]);
        assert_eq!(signal_run(&store, 0), Some(0..1), "one block under the cursor");
        store.select_all(0);
        let run = signal_run(&store, 0).expect("the three are one run");
        assert_eq!(run, 0..3);
        decode_run(&mut store, 0, run, Some("rom")).expect("a ROM block in three pieces");
        assert_eq!(standard_data(&store.tape(0).blocks), [&head]);
    }

    /// No signal to read, or none that holds anything: the tape is left exactly
    /// as it was, without an undo step for an edit that did not happen.
    #[test]
    fn a_signal_that_holds_nothing_changes_nothing() {
        let noise = Body::PulseSeq { pulses: (0..200u16).map(|i| 300 + (i * 37 % 900)).collect() };
        let mut store =
            store_with(vec![Block::new(noise), Block::new(Body::Standard { pause: 0, data: header(4) })]);
        let before = store.tape(0).blocks.clone();
        let why = decode_run(&mut store, 0, 0..1, None).expect_err("noise decoded into something");
        assert!(why.contains("gave no data blocks, only"), "{why}");
        assert_eq!(store.tape(0).blocks, before);
        assert!(!store.tape(0).can_undo());

        // Silence recorded as it was is the lead-in a tape of direct recordings often
        // starts with (ZXDB 27972's first block is 3.7 s of it); it says so, rather
        // than that the decoder found nothing.
        let silence = Body::Direct { tstates: 79, pause: 0, used_bits: 8, data: vec![0; 22150] };
        let mut store = store_with(vec![Block::new(silence)]);
        let why = decode_run(&mut store, 0, 0..1, None).expect_err("silence decoded into something");
        assert_eq!(why, "This block is silence: 4.0 s at one level, with no signal in it to read.");
        assert!(!store.tape(0).can_undo());

        // Bytes under a loader's timing are not "undecoded signal", alone or picked
        // from here and there.
        store.set_cursor(0, 1, crate::state::SelectMode::Single);
        assert_eq!(signal_run(&store, 0), None);
        let mut store = store_with(vec![
            Block::new(Body::PulseSeq { pulses: vec![1] }),
            Block::new(Body::Pause { pause: 1 }),
            Block::new(Body::PulseSeq { pulses: vec![2] }),
        ]);
        store.set_cursor(0, 2, crate::state::SelectMode::Toggle);
        assert_eq!(signal_run(&store, 0), None, "two blocks with one between them are not a run");

        let mut locked = store_with(vec![as_direct_recording(&synthetic_recording().0)]);
        locked.locked = true;
        assert!(decode_run(&mut locked, 0, 0..1, None).is_err());
        assert!(matches!(locked.tape(0).blocks[0].body, Body::Direct { .. }));
    }

    /// On a tape that came from a recording, what comes out knows where it lay in
    /// that recording: the replaced block's start, plus where the decoder found
    /// it in the signal. On any other tape there is no recording to lie in.
    #[test]
    fn decoded_blocks_inherit_their_place_in_the_recording() {
        let tape = synthetic_recording().0;
        let sampled = as_direct_recording(&tape);
        let mut store = store_with(vec![sampled.clone()]);
        let recording = Recording {
            name: "side-a.wav".into(),
            path: PathBuf::from("/recordings/side-a.wav"),
            extents: [(sampled.uid, (60_000, 66_000))].into_iter().collect(),
            ..Recording::default()
        };
        store.tape_mut(0).recording = Some(recording);
        decode_run(&mut store, 0, 0..1, None).unwrap();
        let t = store.tape(0);
        let recording = t.recording.as_ref().unwrap();
        assert!(recording.inserted.is_empty(), "its own recording's blocks were filed as another's");
        let (from, to) = recording.extents[&t.blocks[0].uid];
        assert!((60_000..61_000).contains(&from) && to > from, "the header lies at {from}–{to} ms");

        let mut plain = store_with(vec![as_direct_recording(&tape)]);
        decode_run(&mut plain, 0, 0..1, None).unwrap();
        assert!(plain.tape(0).recording.is_none(), "a tape from a file gained a recording");
    }

    #[test]
    fn the_decode_signal_dialog_draws() {
        let (ctx, mut app) = app();
        let sampled = as_direct_recording(&synthetic_recording().0);
        app.store.tape_mut(0).load("signal.tzx".into(), None, vec![sampled], None);
        app.store.set_cursor(0, 0, crate::state::SelectMode::Single);
        app.store.dialog = Some(Dialog::DecodeRun(DecodeRunState { side: 0, loader: Some("rom") }));
        let canvas = crate::shot::capture(&ctx, &mut app, (1100.0, 720.0), 12);
        assert!(matches!(app.store.dialog, Some(Dialog::DecodeRun(_))), "the dialog closed by itself");
        if let Some(dir) = std::env::var_os("TAPETI_SHOTS").map(PathBuf::from) {
            std::fs::write(dir.join("recording-decode-run.png"), canvas.to_png()).expect("write the picture");
        }
    }

    // ---- decoding one stretch of the recording again ------------------------------

    /// Two programs on one tape, as a recording on disk: four blocks, so the ones
    /// in the middle have a neighbour on each side and an extent with both ends.
    fn two_program_wav(test: &str) -> (Vec<Block>, TempWav) {
        let first: Vec<u8> = (0..300u16).map(|i| (i * 7 % 251) as u8).collect();
        let second: Vec<u8> = (0..200u16).map(|i| (i * 13 % 241) as u8).collect();
        let tape = vec![
            Block::new(Body::Standard { pause: 1000, data: header(first.len() as u16) }),
            Block::new(Body::Standard { pause: 2000, data: rom_block(0xff, &first) }),
            Block::new(Body::Standard { pause: 1000, data: header(second.len() as u16) }),
            Block::new(Body::Standard { pause: 3000, data: rom_block(0xff, &second) }),
        ];
        let wav = render_wav(&tape);
        let path = std::env::temp_dir().join(format!("tapeti-{}-{test}.wav", std::process::id()));
        std::fs::write(&path, wav).expect("write the synthetic recording");
        (tape, TempWav(path))
    }

    /// An app with that recording decoded into the left pane.
    fn decoded_app(test: &str) -> (egui::Context, App, Vec<Block>, TempWav) {
        let (tape, wav) = two_program_wav(test);
        let (ctx, mut app) = app();
        open(&mut app.store, 0, &wav.0, false);
        draw_until_decoded(&ctx, &mut app);
        assert_eq!(
            standard_data(&app.store.tape(0).blocks),
            standard_data(&tape),
            "the fixture did not decode"
        );
        (ctx, app, tape, wav)
    }

    /// The window is the run's extent and a margin, and stops at a neighbour that
    /// is staying; and there is no stretch to decode where the tape has no
    /// recording, the run is broken, or an end of it is not known.
    #[test]
    fn the_window_is_the_run_and_a_margin_but_never_a_neighbour() {
        let (_ctx, mut app, _tape, _wav) = decoded_app("window");
        let extents = app.store.tape(0).recording.as_ref().unwrap().extents.clone();
        let uids: Vec<Uid> = app.store.tape(0).blocks.iter().map(|b| b.uid).collect();
        let uid = |i: usize| uids[i];
        let (first, second, third) = (extents[&uid(0)], extents[&uid(1)], extents[&uid(2)]);

        app.store.set_cursor(0, 1, crate::state::SelectMode::Single);
        let st = stretch_of(&app.store, 0).expect("the data block has a stretch");
        assert_eq!((st.first, st.last, st.blocks), (uid(1), uid(1), 1));
        assert!(
            st.from_ms >= first.1.min(second.0) && st.from_ms <= second.0,
            "{st:?} starts inside block 0"
        );
        assert!(st.to_ms >= second.1 && st.to_ms <= third.0.max(second.1), "{st:?} runs into block 2");

        app.store.set_cursor(0, 0, crate::state::SelectMode::Single);
        let st = stretch_of(&app.store, 0).unwrap();
        assert_eq!(
            st.from_ms,
            first.0.saturating_sub(STRETCH_MARGIN_MS),
            "nothing before it: the whole margin"
        );

        app.store.set_cursor(0, 2, crate::state::SelectMode::Toggle);
        assert_eq!(stretch_of(&app.store, 0), None, "blocks 0 and 2 are not a run");
        app.store.set_cursor(0, 1, crate::state::SelectMode::Single);
        app.store.set_cursor(0, 2, crate::state::SelectMode::Range);
        assert_eq!(stretch_of(&app.store, 0).map(|s| s.blocks), Some(2));

        // The last block of the tape: the recording runs out under its pause, and its end
        // is the recording's. It is the block a worn tape most often gets wrong.
        let last = app.store.tape(0).blocks.len() - 1;
        app.store.set_cursor(0, last as i32, crate::state::SelectMode::Single);
        let st = stretch_of(&app.store, 0).expect("the last block can be read again");
        assert_eq!(
            st.to_ms,
            extents[&uid(last)].1 + STRETCH_MARGIN_MS,
            "past the end of the file, which the decoder clamps"
        );
        app.store.tape_mut(0).recording = None;
        app.store.set_cursor(0, 1, crate::state::SelectMode::Single);
        assert_eq!(stretch_of(&app.store, 0), None);
    }

    /// The three menu commands are enabled where their dialogs have something to act
    /// on, and open them: the whole recording, the selected run's stretch of it, and
    /// the signal in a block that holds some.
    #[test]
    fn the_decode_commands_follow_the_tape_and_open_their_dialogs() {
        use crate::commands::{enabled, run};
        let (_ctx, mut app, tape, _wav) = decoded_app("commands");
        app.store.set_cursor(0, 1, crate::state::SelectMode::Single);
        assert!(enabled(&app, "redecode", 0) && enabled(&app, "redecode-stretch", 0));
        assert!(
            !enabled(&app, "redecode", 1) && !enabled(&app, "redecode-stretch", 1),
            "the right pane is empty"
        );
        assert!(!enabled(&app, "decode-signal", 0), "a standard block holds no signal to decode");

        assert!(run(&mut app, "redecode", 0));
        assert!(matches!(&app.store.dialog, Some(Dialog::Redecode(s)) if s.stretch.is_none()));
        app.store.dialog = None;
        assert!(run(&mut app, "redecode-stretch", 0));
        let first = app.store.tape(0).blocks[1].uid;
        assert!(
            matches!(&app.store.dialog, Some(Dialog::Redecode(s)) if s.stretch.is_some_and(|st| st.first == first))
        );
        app.store.dialog = None;

        app.store.tape_mut(1).load("signal.tzx".into(), None, vec![as_direct_recording(&tape)], None);
        app.store.set_cursor(1, 0, crate::state::SelectMode::Single);
        assert!(enabled(&app, "decode-signal", 1));
        assert!(!enabled(&app, "redecode", 1), "a tape from a file has no recording to read again");
        assert!(run(&mut app, "decode-signal", 1));
        assert!(matches!(&app.store.dialog, Some(Dialog::DecodeRun(s)) if s.side == 1));
    }

    /// The point of it: a block that is wrong on the tape is read again from its own
    /// few seconds of the recording. Its neighbours are not touched (they are the same
    /// blocks, not equal ones), it lands where it was, it knows where it lies in the
    /// whole recording, and undo brings the damaged one back.
    #[test]
    fn a_damaged_block_is_restored_from_its_own_stretch() {
        let (ctx, mut app, tape, _wav) = decoded_app("restore");
        let kept: Vec<_> = app.store.tape(0).blocks.iter().map(|b| b.uid).collect();
        let was = app.store.tape(0).recording.as_ref().unwrap().extents[&kept[1]];

        // Damage block 1 the way an edit would: a new body under the same uid.
        let damaged = Block { uid: kept[1], body: Body::Standard { pause: 2000, data: vec![0xff, 1, 2, 3] } };
        app.store.commit(0, |bl| {
            bl[1] = damaged.clone();
            crate::state::Edit { cursor: Some(1), selected: None }
        });
        app.store.set_cursor(0, 1, crate::state::SelectMode::Single);

        // What the dialog's Decode button pushes.
        let state = RedecodeState::for_stretch(&app.store, 0).expect("the block still knows where it lay");
        let st = state.stretch.unwrap();
        app.store.recordings.push(RecordingRequest {
            side: 0,
            path: app.store.tape(0).recording.as_ref().unwrap().path.clone(),
            into: Landing::Stretch { first: st.first, last: st.last, from_ms: st.from_ms, to_ms: st.to_ms },
            choice: state.choice,
            confirmed: true,
        });
        draw_until_decoded(&ctx, &mut app);
        assert!(app.store.dialog.is_none(), "{:?}", app.store.status());

        let t = app.store.tape(0);
        assert_eq!(standard_data(&t.blocks), standard_data(&tape), "the block was not read back");
        let now: Vec<_> = t.blocks.iter().map(|b| b.uid).collect();
        assert_eq!((now[0], now[2], now[3]), (kept[0], kept[2], kept[3]), "a neighbour was replaced");
        assert_ne!(now[1], kept[1]);
        let lies = t.recording.as_ref().unwrap().extents[&now[1]];
        assert!(
            lies.0.abs_diff(was.0) < 50 && lies.1.abs_diff(was.1) < 50,
            "{lies:?} is not where {was:?} was"
        );
        assert!(app.store.status().contains("1 of 1 data blocks check out"), "{}", app.store.status());

        // And the first retry is not the last: what came back knows where it lies, end
        // included, though the window stopped exactly where its pause did.
        app.store.set_cursor(0, 1, crate::state::SelectMode::Single);
        let again = stretch_of(&app.store, 0).expect("a block read again can be read again");
        assert_eq!((again.from_ms, again.to_ms), (st.from_ms, st.to_ms));

        app.store.undo(0);
        assert_eq!(app.store.tape(0).blocks[1], damaged);
    }

    /// The decode takes time and names its blocks by uid: if they have gone by the
    /// time it is back, nothing is replaced and the user is told.
    #[test]
    fn a_stretch_whose_blocks_have_gone_replaces_nothing() {
        let (ctx, mut app, _tape, _wav) = decoded_app("gone");
        app.store.set_cursor(0, 1, crate::state::SelectMode::Single);
        let st = stretch_of(&app.store, 0).unwrap();
        app.store.recordings.push(RecordingRequest {
            side: 0,
            path: app.store.tape(0).recording.as_ref().unwrap().path.clone(),
            into: Landing::Stretch { first: st.first, last: st.last, from_ms: st.from_ms, to_ms: st.to_ms },
            choice: DecodeChoice::default(),
            confirmed: true,
        });
        draw(&ctx, &mut app);
        app.store.delete_indices(0, vec![1]);
        let after_delete = app.store.tape(0).blocks.clone();
        draw_until_decoded(&ctx, &mut app);
        assert_eq!(app.store.tape(0).blocks, after_delete);
        assert!(matches!(app.store.dialog, Some(Dialog::Message { .. })), "nobody was told");
    }

    #[test]
    fn the_stretch_dialog_draws() {
        let (ctx, mut app, _tape, _wav) = decoded_app("dialog");
        app.store.set_cursor(0, 1, crate::state::SelectMode::Single);
        let state = RedecodeState::for_stretch(&app.store, 0).unwrap();
        app.store.dialog = Some(Dialog::Redecode(state));
        let canvas = crate::shot::capture(&ctx, &mut app, (1100.0, 720.0), 12);
        assert!(matches!(app.store.dialog, Some(Dialog::Redecode(_))));
        if let Some(dir) = std::env::var_os("TAPETI_SHOTS").map(PathBuf::from) {
            std::fs::write(dir.join("recording-stretch.png"), canvas.to_png()).expect("write the picture");
        }
    }

    /// Which rows of a window's result are its cut is the decoder's to say, per span:
    /// an edge span's rows go, and an empty run names no row at all, since it was
    /// folded into a pause. Only more than a handful of pulses at an edge counts as
    /// an overreach.
    #[test]
    fn only_the_rows_the_decoder_calls_its_cut_are_dropped() {
        let span = |edge, pulses, rows| EdgeSpan { edge, pulses, rows };
        let d = Decode {
            edges: vec![
                span(Edge::Lead, 1, Some(0..1)),
                span(Edge::Trail, 40, Some(3..3)),
                span(Edge::Trail, 2, None),
            ],
            ..Decode::default()
        };
        assert_eq!(edge_rows(&d), [0]);
        assert_eq!(overreach(&d), " — the window cut into 40 pulses of signal after it");
    }

    /// A clean recording opens clean. The silence before the tape is something every
    /// capture has; the decoder reports it as information, and a consistency check
    /// that warned about it on every tape would teach people not to read it.
    #[test]
    fn a_clean_recording_opens_without_a_warning() {
        use tapeti_core::consistency::Severity;
        let (_ctx, app, _tape, _wav) = decoded_app("clean");
        let t = app.store.tape(0);
        let recording = t.recording.as_ref().unwrap();
        assert!(recording.report.iter().any(|l| l.starts_with("lead-in")), "{:?}", recording.report);
        let warned: Vec<_> = t.issues(0).into_iter().filter(|i| i.severity != Severity::Info).collect();
        assert!(warned.is_empty(), "{warned:?}");
        assert_eq!(recording.extents.len(), t.blocks.len(), "every block lies somewhere in the recording");
    }

    // ---- a recording inserted into a tape -----------------------------------------

    /// A recording inserted into a tape that came from a file does not become the
    /// tape's recording: there is still no recording of the whole tape, so nothing
    /// offers to decode one again, which would otherwise replace the TZX with the WAV.
    /// The inserted blocks keep their own file, and a stretch of that can be re-read.
    #[test]
    fn an_inserted_recording_does_not_become_the_tapes() {
        let wav = TempWav::new("inserted");
        let (ctx, mut app) = app();
        let own = vec![Block::new(Body::Text { text: "a tape from a file".into() })];
        app.store.tape_mut(0).load("game.tzx".into(), Some(PathBuf::from("/tapes/game.tzx")), own, None);
        app.store.set_cursor(0, 0, crate::state::SelectMode::Single);
        open(&mut app.store, 0, &wav.0, true);
        draw_until_decoded(&ctx, &mut app);

        let t = app.store.tape(0);
        assert_eq!(t.name, "game.tzx");
        assert!(t.blocks.len() > 1, "nothing was inserted");
        let recording = t.recording.as_ref().expect("the inserted blocks know where they came from");
        assert!(!recording.is_of_the_tape() && recording.inserted.len() == 1);
        assert!(
            !can_redecode(&app.store, &app.recordings, 0),
            "a TZX offered to be decoded again from a WAV"
        );

        let inserted = t.blocks.iter().position(|b| matches!(b.body, Body::Standard { .. })).unwrap();
        assert!(recording.extent(t.blocks[inserted].uid).is_some(), "the footer has nothing to say");
        app.store.set_cursor(0, inserted as i32, crate::state::SelectMode::Single);
        let state = RedecodeState::for_stretch(&app.store, 0).expect("a stretch of the inserted recording");
        assert!(state.stretch.is_some());
        // Tape Info draws for it, with no "Decoded from" section to draw.
        app.store.dialog = Some(Dialog::tape_info(0));
        draw(&ctx, &mut app);
    }

    /// Two recordings on one tape: each block's position is in its own, so a window
    /// for an inserted block is cut from the inserted file and clamped against its
    /// own neighbours, and decoding the tape's recording again is still about the
    /// tape's. Re-reading the inserted block reads the right file.
    #[test]
    fn a_stretch_is_read_from_the_recording_its_blocks_came_from() {
        let (ctx, mut app, _tape, tape_wav) = decoded_app("two-a");
        let other = TempWav::new("two-b");
        app.store.set_cursor(0, 1, crate::state::SelectMode::Single);
        open(&mut app.store, 0, &other.0, true);
        draw_until_decoded(&ctx, &mut app);

        let recording = app.store.tape(0).recording.clone().unwrap();
        assert_eq!(recording.path, tape_wav.0, "the tape's recording changed");
        assert_eq!(recording.inserted.len(), 1);
        assert_eq!(recording.inserted[0].path, other.0);
        assert!(can_redecode(&app.store, &app.recordings, 0));

        // The first inserted block is early in its own recording, though it sits
        // after a block that lies six seconds into the tape's.
        let blocks = app.store.tape(0).blocks.clone();
        let at = blocks.iter().position(|b| recording.inserted[0].extents.contains_key(&b.uid)).unwrap();
        app.store.set_cursor(0, at as i32, crate::state::SelectMode::Single);
        let st = stretch_of(&app.store, 0).unwrap();
        let own = recording.inserted[0].extents[&blocks[at].uid];
        assert!(st.from_ms <= own.0 && own.0 < 2000, "{st:?} is not a position in the inserted recording");

        let before = standard_data(&blocks).into_iter().cloned().collect::<Vec<_>>();
        app.store.recordings.push(RecordingRequest {
            side: 0,
            path: other.0.clone(),
            into: Landing::Stretch { first: st.first, last: st.last, from_ms: st.from_ms, to_ms: st.to_ms },
            choice: DecodeChoice::default(),
            confirmed: true,
        });
        draw_until_decoded(&ctx, &mut app);
        assert!(app.store.dialog.is_none());
        let t = app.store.tape(0);
        assert_eq!(standard_data(&t.blocks).into_iter().cloned().collect::<Vec<_>>(), before);
        let again = t.recording.as_ref().unwrap();
        assert_eq!(again.inserted.len(), 1, "the re-read was filed as a third recording");
        assert!(again.inserted[0].extents.contains_key(&t.blocks[at].uid), "or under the tape's own");
    }

    /// A window that reaches into the next block says so, in the decoder's own count
    /// of what it left there; one that stops where its block does has nothing to say.
    #[test]
    fn a_window_that_cuts_into_a_neighbour_says_so() {
        let (_tape, wav) = two_program_wav("overreach");
        let decode = |window: Option<Range<u64>>| {
            let (choice, cancel) = (DecodeChoice::default(), Cancel::new());
            backend::decode_file(&wav.0, choice, window, &mut |_| {}, &cancel).unwrap()
        };
        let whole = decode(None);
        assert_eq!(overreach(&whole), "", "a whole recording's lead-in is not an overreach");
        let data = whole.extents[1].clone().unwrap();
        assert_eq!(overreach(&decode(Some(data.clone()))), "");
        // Half a second into the next block's pilot.
        let said = overreach(&decode(Some(data.start..data.end + 22_050)));
        assert!(said.contains("pulses of signal after it"), "{said:?}");
        // And into the data bits of the block before it.
        let said = overreach(&decode(Some(data.start - 46_000..data.end)));
        assert!(said.contains("before it"), "{said:?}");
    }

    /// A margin that takes in the end of a neighbour's pilot gets the neighbour read
    /// again, whole: a ROM pilot is that forgiving. It is still on the tape, so it is
    /// left out of what is spliced in, and the status bar says the window reached it.
    #[test]
    fn a_neighbour_the_window_read_again_is_not_spliced_in_twice() {
        let (ctx, mut app, tape, _wav) = decoded_app("twice");
        app.store.set_cursor(0, 1, crate::state::SelectMode::Single);
        let st = stretch_of(&app.store, 0).unwrap();
        // What the window would be if the header before it did not know where it lay:
        // three seconds early, inside that header's pilot.
        app.store.recordings.push(RecordingRequest {
            side: 0,
            path: app.store.tape(0).recording.as_ref().unwrap().path.clone(),
            into: Landing::Stretch {
                first: st.first,
                last: st.last,
                from_ms: st.from_ms - 3000,
                to_ms: st.to_ms,
            },
            choice: DecodeChoice::default(),
            confirmed: true,
        });
        draw_until_decoded(&ctx, &mut app);
        assert_eq!(
            standard_data(&app.store.tape(0).blocks),
            standard_data(&tape),
            "the header is there twice"
        );
        assert_eq!(app.store.tape(0).blocks.len(), 4);
        let status = app.store.status().to_string();
        assert!(status.contains("also read 1 neighbouring block"), "{status}");
    }
}
