// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The C ABI the browser calls for recordings, beside the core's own in the same module.
//!
//! Buffers go in and out as they do for the core (`core_alloc`, `core_free`, an answer of a `u32`
//! length and that much wire payload). A request is the core's snapshot request, three `bytes`:
//! the file, the Z-RLE data of a CSW file inflated (empty otherwise; the page has zlib, this
//! crate does not), and the file's name.

use crate::entry::{decode_file, describe, keep_as_raw_signal, tape_band, Channels, Finding, Kept, Progress};
use crate::readers::{read, Channel, Recording};
use crate::waveform::filter::Filter;
use std::sync::atomic::AtomicBool;
use tapeti_core::parser::ParsedTape;
use tapeti_core::wire::{
    decode_snapshot_request, encode_bytes, encode_error, encode_recording_summary, encode_tape,
};
use tapeti_core::writer::required_version;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "decode")]
extern "C" {
    /// The page's, for its status bar: the blocks read so far and the share of the decode done,
    /// in thousandths.
    fn decode_progress(blocks: u32, done: u32);
}

/// Passes on to the page how far a decode has come, each time what it would show changes.
fn reporter() -> impl FnMut(Progress) {
    let mut last = None;
    move |p: Progress| {
        let now = (p.blocks as u32, (p.done.clamp(0.0, 1.0) * 1000.0) as u32);
        if last != Some(now) {
            last = Some(now);
            #[cfg(target_arch = "wasm32")]
            // SAFETY: the page supplies the import, and it takes only numbers.
            unsafe {
                decode_progress(now.0, now.1)
            };
        }
    }
}

unsafe fn slice<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

/// An answer laid out as the core lays out its own: its `u32` length, then the payload.
unsafe fn finish(payload: Vec<u8>) -> *mut u8 {
    let out = tapeti_core::wasm::core_alloc(4 + payload.len());
    if out.is_null() {
        return out;
    }
    std::ptr::copy_nonoverlapping((payload.len() as u32).to_le_bytes().as_ptr(), out, 4);
    std::ptr::copy_nonoverlapping(payload.as_ptr(), out.add(4), payload.len());
    out
}

/// The compressed data of a Z-RLE CSW file, for the page to inflate; empty for any other file.
///
/// # Safety
/// `ptr` must point at `len` bytes of snapshot request.
#[no_mangle]
pub unsafe extern "C" fn decode_csw_zrle(ptr: *const u8, len: usize) -> *mut u8 {
    finish(match decode_snapshot_request(slice(ptr, len)) {
        Ok((file, _, name)) => match read(&name, &file, Channel::Mix) {
            Ok(Recording::Csw(c)) if c.compression == 2 => encode_bytes(c.data),
            _ => encode_bytes(&[]),
        },
        Err(e) => encode_error(&e.0),
    })
}

/// What the import dialog shows of a recording.
///
/// # Safety
/// `ptr` must point at `len` bytes of snapshot request.
#[no_mangle]
pub unsafe extern "C" fn decode_describe(ptr: *const u8, len: usize) -> *mut u8 {
    let answer = || -> Result<Vec<u8>, String> {
        let (file, inflated, name) = decode_snapshot_request(slice(ptr, len)).map_err(|e| e.0)?;
        let inflate = |_: &[u8]| Ok(inflated.clone());
        let s = describe(&name, &file, &inflate).map_err(|e| e.to_string())?;
        Ok(encode_recording_summary(
            s.format,
            s.rate_hz,
            s.channels,
            s.samples as u64,
            s.missing_bytes,
            &s.skipped,
            s.kept == Kept::Csw,
            s.blocks as u32,
            s.tzx_size as u64,
        ))
    };
    finish(answer().unwrap_or_else(|message| encode_error(&message)))
}

/// The recording as a tape, answered as a parsed tape is.
///
/// Decoded into blocks when `decode` is 1, kept as raw signal when 0. `channel`: 0 both mixed, 1
/// left, 2 right, 3 whichever decodes best (both mixed when kept as raw signal). `filter`: 1 the
/// tape band-pass. While it decodes, the module's import `decode.decode_progress` is told the
/// blocks read so far and the thousandths of the decode done, whenever either changes. Each
/// finding is a warning of three tab-separated fields: `P` for a problem or `I` for information,
/// the block's index or `-` for the whole recording, then the text.
///
/// # Safety
/// `ptr` must point at `len` bytes of snapshot request.
#[no_mangle]
pub unsafe extern "C" fn decode_recording(
    ptr: *const u8,
    len: usize,
    decode: u32,
    channel: u32,
    filter: u32,
) -> *mut u8 {
    let answer = || -> Result<Vec<u8>, String> {
        let (file, inflated, name) = decode_snapshot_request(slice(ptr, len)).map_err(|e| e.0)?;
        let inflate = |_: &[u8]| Ok(inflated.clone());
        let channels = match channel {
            1 => Channels::Only(Channel::Left),
            2 => Channels::Only(Channel::Right),
            3 => Channels::Best,
            _ => Channels::Only(Channel::Mix),
        };
        let channel = match channels {
            Channels::Only(channel) => channel,
            Channels::Best => Channel::Mix,
        };
        let filter = match filter {
            1 => tape_band(describe(&name, &file, &inflate).map_err(|e| e.to_string())?.rate_hz),
            _ => Filter::None,
        };
        let (blocks, warnings) = if decode == 1 {
            let d = decode_file(
                &name,
                &file,
                channels,
                filter,
                None,
                &inflate,
                &mut reporter(),
                &AtomicBool::new(false),
            )
            .map_err(|e| e.to_string())?;
            let warnings = d
                .findings
                .iter()
                .map(|f| {
                    let class = if matches!(
                        f,
                        Finding::BadChecksum { .. }
                            | Finding::Short { .. }
                            | Finding::PartialFinalByte { .. }
                    ) {
                        'P'
                    } else {
                        'I'
                    };
                    let block = f.block().map_or("-".to_string(), |b| b.to_string());
                    format!("{class}\t{block}\t{}", f.text())
                })
                .collect();
            (d.blocks, warnings)
        } else {
            (
                keep_as_raw_signal(&name, &file, channel, filter, &inflate).map_err(|e| e.to_string())?,
                Vec::new(),
            )
        };
        let v = required_version(&blocks);
        Ok(encode_tape(&ParsedTape { blocks, major: v.major, minor: v.minor, warnings }))
    };
    finish(answer().unwrap_or_else(|message| encode_error(&message)))
}
