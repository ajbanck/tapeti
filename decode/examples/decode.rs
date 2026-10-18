// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Decodes a tape recording into a TZX file, as the apps' "Blocks, decoded" does.
//!
//! `cargo run --release -p tapeti-decode --example decode -- INPUT [OUTPUT.tzx] [--left|--right|--mix]
//! [--filter] [--raw]`. With no channel a stereo recording is read from whichever channel reads
//! best. The findings, where each block lies in the recording (`extent N: start signal-end end`,
//! in samples) and the time taken go to stderr. A compressed (Z-RLE) CSW file needs zlib,
//! which this crate does not have, so it is refused.

use std::sync::atomic::AtomicBool;
use std::time::Instant;
use tapeti_decode::entry::{decode_file, describe, keep_as_raw_signal, tape_band, Channels};
use tapeti_decode::readers::Channel;
use tapeti_decode::waveform::filter::Filter;

fn main() {
    if let Err(e) = run() {
        eprintln!("decode: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let (mut paths, mut channels, mut filter, mut raw) = (Vec::new(), Channels::Best, false, false);
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--left" => channels = Channels::Only(Channel::Left),
            "--right" => channels = Channels::Only(Channel::Right),
            "--mix" => channels = Channels::Only(Channel::Mix),
            "--filter" => filter = true,
            "--raw" => raw = true,
            _ if arg.starts_with('-') => return Err(format!("unknown switch {arg}")),
            _ => paths.push(arg),
        }
    }
    let input =
        paths.first().ok_or("usage: decode INPUT [OUTPUT.tzx] [--left|--right|--mix] [--filter] [--raw]")?;
    let output = paths.get(1).cloned().unwrap_or_else(|| {
        let stem = input.rsplit_once('.').map_or(input.as_str(), |(stem, _)| stem);
        format!("{stem}.tzx")
    });
    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let no_zlib = |_: &[u8]| -> Result<Vec<u8>, String> { Err("a Z-RLE CSW file needs zlib".into()) };
    let summary = describe(input, &bytes, &no_zlib).map_err(|e| e.to_string())?;
    let filter = if filter { tape_band(summary.rate_hz) } else { Filter::None };
    let started = Instant::now();
    let blocks = if raw {
        let channel = match channels {
            Channels::Only(channel) => channel,
            Channels::Best => Channel::Mix,
        };
        keep_as_raw_signal(input, &bytes, channel, filter, &no_zlib).map_err(|e| e.to_string())?
    } else {
        let cancel = AtomicBool::new(false);
        let mut first = None;
        let mut progress = |p: tapeti_decode::entry::Progress| {
            if p.blocks > 0 {
                first.get_or_insert(started.elapsed().as_secs_f64());
            }
        };
        let d = decode_file(input, &bytes, channels, filter, None, &no_zlib, &mut progress, &cancel)
            .map_err(|e| e.to_string())?;
        if let Some(first) = first {
            eprintln!("first block read after {first:.2} s");
        }
        for f in &d.findings {
            match f.block() {
                Some(block) => eprintln!("block {block}: {}", f.text()),
                None => eprintln!("{}", f.text()),
            }
        }
        for (k, e) in d.extents.iter().enumerate() {
            eprintln!("extent {k}: {:.0} {:.0} {:.0}", e.start, e.signal_end, e.end);
        }
        d.blocks
    };
    let seconds = started.elapsed().as_secs_f64();
    let tzx = tapeti_core::writer::serialize_tzx(&blocks, None).map_err(|e| e.to_string())?;
    std::fs::write(&output, tzx).map_err(|e| format!("{output}: {e}"))?;
    eprintln!(
        "{output}: {} blocks from {:.1} s of recording in {seconds:.2} s",
        blocks.len(),
        summary.seconds()
    );
    Ok(())
}
