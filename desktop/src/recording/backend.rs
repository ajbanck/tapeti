// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The decoder's side of `recording.rs`: the only file that calls `tapeti-decode`. What it hands
//! back is plain data ([`Decode`], [`Loader`], [`DecodeError`], [`Progress`]), so the jobs,
//! dialogs and splicing that read it would work over any other way of decoding.

use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tapeti_core::audio::TSTATES_PER_SEC;
use tapeti_core::types::Body;
use tapeti_decode::entry::{self, Channels, Decoded, Finding as Found};
use tapeti_decode::pulses::Pulse;
use tapeti_decode::readers::Channel;
use tapeti_decode::waveform::filter::Filter;

use super::{Decode, DecodeError, Edge, EdgeSpan, Finding, Loader, Progress};
use crate::state::{ChannelChoice, DecodeChoice, NoteClass};

/// Tells a decode to stop, from any thread. It stops before the next block it would read.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// The ROM's loader, the one the decoder reads.
const ROM: &str = "rom";

/// The loaders the dialogs offer.
pub fn loaders() -> Vec<Loader> {
    vec![Loader {
        name: ROM,
        title: "ROM loader",
        about: "Standard blocks, read as the Spectrum's ROM reads them; anything else is kept as raw signal.",
    }]
}

/// Whether the decoder takes `choice`.
pub fn check(choice: DecodeChoice) -> Result<(), String> {
    match choice.loader {
        None | Some(ROM) => Ok(()),
        Some(name) => Err(format!("There is no loader called \"{name}\".")),
    }
}

/// About how much memory decoding the file at `path` peaks at: the file, which is read whole,
/// and its pulses and edges. Measured on an hour of data blocks at 44.1 kHz, 16 bits: 2.6 times
/// the file in mono, 2.5 in stereo. A recording with long silences has fewer pulses and needs less.
pub fn peak_bytes(path: &Path) -> Result<Option<u64>, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    Ok(Some(len * 13 / 5))
}

fn channels_of(choice: ChannelChoice) -> Channels {
    match choice {
        ChannelChoice::Best => Channels::Best,
        ChannelChoice::Mix => Channels::Only(Channel::Mix),
        ChannelChoice::Left => Channels::Only(Channel::Left),
        ChannelChoice::Right => Channels::Only(Channel::Right),
    }
}

/// Decode the recording at `path` as `choice` says, or only `window` of it. `window` is on the
/// recording's own grid (`Decode::rate_hz`), and positions come back on the whole file's.
pub fn decode_file(
    path: &Path,
    choice: DecodeChoice,
    window: Option<Range<u64>>,
    progress: &mut dyn FnMut(Progress),
    cancel: &Cancel,
) -> Result<Decode, DecodeError> {
    check(choice).map_err(DecodeError::Failed)?;
    let name = crate::files::file_name(path);
    let bytes = std::fs::read(path).map_err(|e| DecodeError::Failed(format!("{name}: {e}")))?;
    let filter = |rate_hz| if choice.filter { entry::tape_band(rate_hz) } else { Filter::None };
    // The band-pass is designed for the file's rate, which only reading it tells.
    let rate_hz = match choice.filter {
        true => entry::describe(&name, &bytes, &crate::tape::inflate).map_err(failed)?.rate_hz,
        false => 0,
    };
    let window = window.map(|w| w.start as f64..w.end as f64);
    let decoded = entry::decode_file(
        &name,
        &bytes,
        channels_of(choice.channel),
        filter(rate_hz),
        window,
        &crate::tape::inflate,
        &mut |p| progress(Progress { blocks: p.blocks, done: p.done }),
        &cancel.0,
    )
    .map_err(|e| match e {
        entry::DecodeError::Cancelled => DecodeError::Cancelled,
        e => failed(e),
    })?;
    Ok(from_decoded(path, decoded, true))
}

/// Decode a signal the caller already has: `pulses` are the lengths of the pulses that make it
/// up, one edge to the next, counted on a grid of `grid_hz`. `label` stands where a file name
/// would.
pub fn decode_pulses(
    label: &str,
    pulses: &[u64],
    grid_hz: u32,
    choice: DecodeChoice,
) -> Result<Decode, DecodeError> {
    check(choice).map_err(DecodeError::Failed)?;
    let scale = f64::from(TSTATES_PER_SEC) / f64::from(grid_hz);
    let mut at = 0u64;
    let pulses: Vec<Pulse> = pulses
        .iter()
        .map(|&len| {
            let pulse = Pulse { tstates: len as f64 * scale, start: at as f64, quiet: false };
            at += len;
            pulse
        })
        .collect();
    let decoded = entry::decode_pulses(&pulses, grid_hz, &mut |_| {}, &AtomicBool::new(false))
        .map_err(|e| DecodeError::Failed(e.to_string()))?;
    Ok(from_decoded(Path::new(label), decoded, false))
}

fn failed(e: impl std::fmt::Display) -> DecodeError {
    DecodeError::Failed(e.to_string())
}

/// Everything a decode said, as plain data. `audio`: whether it was a recording file rather than
/// a signal handed over as pulses, which has no measured rows.
fn from_decoded(path: &Path, d: Decoded, audio: bool) -> Decode {
    let n = d.blocks.len();
    let findings = d
        .findings
        .iter()
        .map(|f| Finding {
            class: match f {
                Found::BadChecksum { .. } | Found::Short { .. } | Found::PartialFinalByte { .. } => {
                    NoteClass::Problem
                }
                Found::RawSignal { .. }
                | Found::Collapsed { .. }
                | Found::LeadIn { .. }
                | Found::Channel { .. }
                | Found::Midline
                | Found::FromChannel { .. }
                | Found::BothChannels { .. }
                | Found::FadeBand { .. }
                | Found::StrayZero { .. }
                | Found::MergedSync { .. } => NoteClass::Info,
            },
            text: f.text(),
            row: f.block(),
        })
        .collect();
    // A stretch of raw signal at either end of the decode is that end's.
    let raw = |i: usize| d.findings.contains(&Found::RawSignal { block: i });
    let pulses = |i: usize| match &d.blocks[i].body {
        Body::Csw { pulse_count, .. } => *pulse_count as usize,
        _ => 0,
    };
    // Where the end went into a pause instead, it is no row: the pause stays.
    let [lead, trail] = d.collapsed_ends;
    let mut edges = Vec::new();
    if n > 0 && raw(0) {
        edges.push(EdgeSpan { edge: Edge::Lead, pulses: pulses(0), rows: Some(0..1) });
    } else if lead > 0 {
        edges.push(EdgeSpan { edge: Edge::Lead, pulses: lead, rows: Some(0..0) });
    }
    if n > 1 && raw(n - 1) {
        edges.push(EdgeSpan { edge: Edge::Trail, pulses: pulses(n - 1), rows: Some(n - 1..n) });
    } else if trail > 0 {
        edges.push(EdgeSpan { edge: Edge::Trail, pulses: trail, rows: Some(n..n) });
    }
    let standard: Vec<&Vec<u8>> = d
        .blocks
        .iter()
        .filter_map(|b| match &b.body {
            Body::Standard { data, .. } => Some(data),
            _ => None,
        })
        .collect();
    let checksums_valid = standard.iter().filter(|data| data.iter().fold(0, |x, &b| x ^ b) == 0).count();
    let measured = if audio {
        vec![
            ("Signal".to_string(), vec![format!("{} samples a second", d.rate_hz)]),
            ("Loader".to_string(), vec![loaders()[0].title.to_string()]),
        ]
    } else {
        Vec::new()
    };
    Decode {
        path: path.to_path_buf(),
        rate_hz: Some(d.rate_hz),
        extents: d.extents.iter().map(|e| Some(e.start.round() as u64..e.end.round() as u64)).collect(),
        findings,
        notes: Vec::new(),
        measured,
        edges,
        checksums_valid,
        floor_only: standard.is_empty(),
        blocks: d.blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapeti_core::types::Block;

    fn with_checksum(bytes: &[u8]) -> Vec<u8> {
        let mut data = vec![0xFF];
        data.extend(bytes);
        data.push(data.iter().fold(0, |x, &d| x ^ d));
        data
    }

    fn lengths(blocks: &[Block]) -> Vec<u64> {
        tapeti_decode::pulses::of_blocks(blocks).unwrap().iter().map(|p| p.tstates as u64).collect()
    }

    #[test]
    fn a_tapes_own_signal_decodes_back_to_its_blocks() {
        let blocks = vec![Block::new(Body::Standard { pause: 1000, data: with_checksum(&[1, 2, 3]) })];
        let d = decode_pulses("signal", &lengths(&blocks), TSTATES_PER_SEC, DecodeChoice::default()).unwrap();
        assert_eq!(d.blocks, blocks);
        assert_eq!(d.checksums_valid, 1);
        assert!(!d.floor_only && d.findings.is_empty() && d.edges.is_empty());
        assert_eq!(d.extents.len(), 1);
    }

    #[test]
    fn findings_name_their_rows_and_raw_ends_are_edges() {
        let mut bad = with_checksum(&[9, 9]);
        bad[1] ^= 1;
        // A turbo block the ROM cannot read, at either end: a tone with a payload, kept raw.
        let turbo = |pause| {
            Block::new(Body::Turbo {
                pilot: 1500,
                sync1: 400,
                sync2: 450,
                zero: 500,
                one: 1000,
                pilot_len: 300,
                used_bits: 8,
                pause,
                data: vec![0x5A, 0xC3],
            })
        };
        let blocks = vec![turbo(1000), Block::new(Body::Standard { pause: 1000, data: bad }), turbo(0)];
        let d = decode_pulses("signal", &lengths(&blocks), TSTATES_PER_SEC, DecodeChoice::default()).unwrap();
        assert_eq!(d.blocks.len(), 3);
        assert!(d.findings.iter().any(|f| f.row == Some(1) && f.class == NoteClass::Problem));
        assert_eq!(d.edges.len(), 2);
        assert_eq!((d.edges[0].edge, d.edges[0].rows.clone()), (Edge::Lead, Some(0..1)));
        assert_eq!((d.edges[1].edge, d.edges[1].rows.clone()), (Edge::Trail, Some(2..3)));
    }

    #[test]
    fn noise_at_the_ends_is_an_edge_with_no_rows() {
        // Pulses shaped like no block, at both ends: they play as the lead-in and the pause, and
        // the edges count them without naming a row.
        let blocks = vec![
            Block::new(Body::PulseSeq {
                pulses: vec![300, 5000, 777, 12000, 50, 900, 4000, 650, 2000, 7000],
            }),
            Block::new(Body::Standard { pause: 1000, data: with_checksum(&[1, 2]) }),
            Block::new(Body::PulseSeq {
                pulses: vec![123, 4567, 890, 10_000, 3000, 6000, 700, 8000, 1500, 2500],
            }),
        ];
        let d = decode_pulses("signal", &lengths(&blocks), TSTATES_PER_SEC, DecodeChoice::default()).unwrap();
        assert_eq!(d.blocks.len(), 1);
        assert_eq!(d.edges.len(), 2);
        assert_eq!((d.edges[0].edge, d.edges[0].rows.clone()), (Edge::Lead, Some(0..0)));
        assert_eq!((d.edges[1].edge, d.edges[1].rows.clone()), (Edge::Trail, Some(1..1)));
        assert!(d.edges.iter().all(|e| e.pulses > 8), "{:?}", d.edges);
    }

    #[test]
    fn silence_alone_is_floor_only_and_an_unknown_loader_is_refused() {
        let d = decode_pulses("signal", &[3500, 350_000], TSTATES_PER_SEC, DecodeChoice::default()).unwrap();
        assert!(d.floor_only);
        let choice = DecodeChoice { loader: Some("speedlock"), ..DecodeChoice::default() };
        assert!(check(choice).is_err());
        assert!(loaders().iter().all(|l| check(DecodeChoice { loader: Some(l.name), ..choice }).is_ok()));
    }
}
