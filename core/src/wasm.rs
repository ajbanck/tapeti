//! The C ABI the wasm build exposes to JavaScript. No wasm-bindgen: the whole
//! interface is a byte buffer in and a byte buffer out, so the module loads
//! with a plain `WebAssembly.instantiate` and needs no generated glue.
//!
//! JavaScript owns both buffers: it calls [`core_alloc`], writes the tape,
//! calls [`core_parse_tape`], reads the `u32` payload length at the returned
//! pointer followed by that many bytes of [`crate::wire`] payload, then frees
//! both with [`core_free`]. `src/tzx/core.ts` is the other end.

use crate::audio::{
    block_duration, decode_csw_rle, emit_block, encode_wav, playback_order, playback_timeline, render_length,
    render_tape, tape_duration, FlowOptions, RecordingSink, RenderOptions,
};
use crate::bits::{add_bits, drop_bits, flip_bytes, join_bits, shift_left_bits, shift_right_bits, BitData};
use crate::compare::{blocks_equal, compare_tapes, find_matches, BlockCompareMode, TapeCompareMode};
use crate::consistency::check_consistency;
use crate::content::{basic_score, content_labels, detect_content};
use crate::convert::convert_block;
use crate::describe::{block_length, checksum, decode_header, describe_block, encode_header};
use crate::parser::{parse_tap, parse_tape, parse_tzx, ParsedTape};
use crate::pokes::{decode_pokes, encode_pokes, pokes_to_text, text_to_pokes};
use crate::programs::{detect_programs, group_ranges, tape_title};
use crate::spectrum::basic::{
    basic_to_text, decode_number, format_number, list_basic, list_variables, BasicOptions,
};
use crate::spectrum::charset::char_table;
use crate::spectrum::screen::{has_flash, render_screen, ScreenOptions};
use crate::spectrum::z80dis::{disassemble, DisOptions};
use crate::types::Block;
use crate::wire::{
    decode_basic_lines, decode_bit_data, decode_blocks, decode_blocks_and_order, decode_header_info,
    decode_pokes_info, encode_basic_lines, encode_bit_data, encode_blocks_answer, encode_bytes,
    encode_comparison, encode_content, encode_described, encode_dis_lines, encode_duration, encode_error,
    encode_f64, encode_header_info, encode_issues, encode_opt_string, encode_pokes_info, encode_programs,
    encode_pulses, encode_ranges, encode_samples, encode_strings, encode_tap, encode_tape, encode_timeline,
    encode_u32s, encode_u8, encode_variables, encode_version, WIRE_VERSION,
};
use crate::writer::{required_version, save_version, serialize_block, serialize_tap, serialize_tzx, Version};
use std::alloc::{alloc, dealloc, Layout};

fn layout(len: usize) -> Layout {
    Layout::from_size_align(len, 1).expect("byte layout")
}

/// The wire format this module speaks; `core.ts` refuses a mismatch.
#[no_mangle]
pub extern "C" fn core_wire_version() -> u32 {
    WIRE_VERSION as u32
}

/// `len` bytes for JavaScript to write into. Free it with [`core_free`] and the
/// same length.
#[no_mangle]
pub extern "C" fn core_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    unsafe { alloc(layout(len)) }
}

/// # Safety
/// `ptr` must come from [`core_alloc`] with the same `len`.
#[no_mangle]
pub unsafe extern "C" fn core_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    dealloc(ptr, layout(len));
}

/// Parse a tape, TZX or TAP by signature. Returns a buffer holding a `u32`
/// payload length and that many bytes of wire payload, to be freed with
/// `core_free(ptr, 4 + length)`.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_parse_tape(ptr: *const u8, len: usize) -> *mut u8 {
    respond(ptr, len, parse_tape)
}

/// As [`core_parse_tape`], but the file must carry the TZX signature; without
/// it the result is an error payload.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_parse_tzx(ptr: *const u8, len: usize) -> *mut u8 {
    respond(ptr, len, parse_tzx)
}

/// As [`core_parse_tape`], but the bytes are always read as TAP.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_parse_tap(ptr: *const u8, len: usize) -> *mut u8 {
    respond(ptr, len, |b| Ok(parse_tap(b)))
}

/// Run one of the parsers over the input buffer and lay its result out for
/// JavaScript: a `u32` payload length followed by the payload.
unsafe fn respond(
    ptr: *const u8,
    len: usize,
    parse: impl Fn(&[u8]) -> Result<ParsedTape, String>,
) -> *mut u8 {
    let input: &[u8] = if ptr.is_null() || len == 0 { &[] } else { std::slice::from_raw_parts(ptr, len) };
    let payload = match parse(input) {
        Ok(tape) => encode_tape(&tape),
        Err(message) => encode_error(&message),
    };
    finish(payload)
}

/// Copy a payload into a buffer JavaScript can read: its `u32` length, then the
/// payload itself.
unsafe fn finish(payload: Vec<u8>) -> *mut u8 {
    let out = core_alloc(4 + payload.len());
    if out.is_null() {
        return out;
    }
    std::ptr::copy_nonoverlapping((payload.len() as u32).to_le_bytes().as_ptr(), out, 4);
    std::ptr::copy_nonoverlapping(payload.as_ptr(), out.add(4), payload.len());
    out
}

/// Write the blocks as a TZX file. `major`/`minor` of `0xffff` means "use the
/// lowest version these blocks need".
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_serialize_tzx(ptr: *const u8, len: usize, major: u32, minor: u32) -> *mut u8 {
    let version = if major == 0xffff || minor == 0xffff {
        None
    } else {
        Some(Version { major: major as u8, minor: minor as u8 })
    };
    with_blocks(ptr, len, |blocks| encode_bytes(&serialize_tzx(blocks, version)))
}

/// Write the blocks as a TAP file, with the indices of those left out.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_serialize_tap(ptr: *const u8, len: usize) -> *mut u8 {
    with_blocks(ptr, len, |blocks| {
        let (bytes, skipped) = serialize_tap(blocks);
        encode_tap(&bytes, &skipped)
    })
}

/// Write the blocks with no file header, ID byte and body each. For one block
/// this is the block's own bytes, which is what the comparison and the size
/// display need.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_serialize_blocks(ptr: *const u8, len: usize) -> *mut u8 {
    with_blocks(ptr, len, |blocks| {
        let mut out = Vec::new();
        for b in blocks {
            out.extend_from_slice(&serialize_block(b));
        }
        encode_bytes(&out)
    })
}

/// The lowest TZX version that can represent these blocks.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_required_version(ptr: *const u8, len: usize) -> *mut u8 {
    with_blocks(ptr, len, |blocks| {
        let v = required_version(blocks);
        encode_version(v.major, v.minor)
    })
}

/// As [`core_required_version`], but never below the version the tape was
/// loaded with. `0xffff` for a tape that was not loaded from a TZX file.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_save_version(
    ptr: *const u8,
    len: usize,
    loaded_major: u32,
    loaded_minor: u32,
) -> *mut u8 {
    let loaded = if loaded_major == 0xffff || loaded_minor == 0xffff {
        None
    } else {
        Some(Version { major: loaded_major as u8, minor: loaded_minor as u8 })
    };
    with_blocks(ptr, len, |blocks| {
        let v = save_version(blocks, loaded);
        encode_version(v.major, v.minor)
    })
}

/// Decode a block list from JavaScript, run `f` over it and lay the answer out
/// the way [`respond`] does.
unsafe fn with_blocks(ptr: *const u8, len: usize, f: impl Fn(&[Block]) -> Vec<u8>) -> *mut u8 {
    let input: &[u8] = if ptr.is_null() || len == 0 { &[] } else { std::slice::from_raw_parts(ptr, len) };
    let payload = match decode_blocks(input) {
        Ok(blocks) => f(&blocks),
        Err(e) => encode_error(&e.0),
    };
    finish(payload)
}

// ---- descriptions, content, consistency and programs ----------------------

/// The list description of the first block in the payload, with its length
/// column. One block per call, because the UI asks per row and caches.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_describe_block(ptr: *const u8, len: usize, hex: u32) -> *mut u8 {
    with_blocks(ptr, len, |blocks| match blocks.first() {
        Some(b) => encode_described(&describe_block(b, hex != 0), block_length(b)),
        None => encode_described("", 0),
    })
}

/// The content label of every block in the tape, in one call.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_content_labels(ptr: *const u8, len: usize) -> *mut u8 {
    with_blocks(ptr, len, |blocks| encode_strings(&content_labels(blocks)))
}

/// What block `index` of the payload contains. The app sends the block and the
/// one before it, which is all the detection looks at.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_detect_content(ptr: *const u8, len: usize, index: u32) -> *mut u8 {
    with_blocks(ptr, len, |blocks| encode_content(&detect_content(blocks, index as usize)))
}

/// Structure, useless blocks, infinite loops and cross nesting. `base` is the
/// number shown for the first block.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_check_consistency(ptr: *const u8, len: usize, base: i32) -> *mut u8 {
    with_blocks(ptr, len, |blocks| encode_issues(&check_consistency(blocks, base)))
}

/// The programs a collection tape holds.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_detect_programs(ptr: *const u8, len: usize) -> *mut u8 {
    with_blocks(ptr, len, |blocks| encode_programs(&detect_programs(blocks)))
}

/// Start/end pairs for the groups and loops in the tape.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_group_ranges(ptr: *const u8, len: usize) -> *mut u8 {
    with_blocks(ptr, len, |blocks| encode_ranges(&group_ranges(blocks)))
}

/// The tape's title from its Archive info block, if it has one.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_tape_title(ptr: *const u8, len: usize) -> *mut u8 {
    with_blocks(ptr, len, |blocks| encode_opt_string(tape_title(blocks).as_deref()))
}

// ---- calls that take plain bytes rather than blocks -----------------------

/// Decode a standard ROM header, if these bytes are one.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_decode_header(ptr: *const u8, len: usize) -> *mut u8 {
    let data = slice(ptr, len);
    finish(encode_header_info(decode_header(data).as_ref()))
}

/// Build the 19 bytes of a standard ROM header.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded header.
#[no_mangle]
pub unsafe extern "C" fn core_encode_header(ptr: *const u8, len: usize) -> *mut u8 {
    let payload = match decode_header_info(slice(ptr, len)) {
        Ok(h) => encode_bytes(&encode_header(&h)),
        Err(e) => encode_error(&e.0),
    };
    finish(payload)
}

/// XOR checksum of the bytes, as the ROM loader computes it.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_checksum(ptr: *const u8, len: usize) -> *mut u8 {
    finish(encode_u8(checksum(slice(ptr, len))))
}

/// How much of this byte stream parses as BASIC lines, from 0 to 1.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_basic_score(ptr: *const u8, len: usize) -> *mut u8 {
    finish(encode_f64(basic_score(slice(ptr, len))))
}

unsafe fn slice<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

// ---- converting, comparing, bits and POKEs --------------------------------

/// Convert the first block of the payload to block type `id`.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_convert_block(ptr: *const u8, len: usize, id: u32) -> *mut u8 {
    with_blocks(ptr, len, |blocks| match blocks.first() {
        Some(b) => encode_blocks_answer(&[convert_block(b, id as u8)]),
        None => encode_blocks_answer(&[]),
    })
}

/// Are the payload's two blocks equal under this compare mode?
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_blocks_equal(ptr: *const u8, len: usize, mode: u32) -> *mut u8 {
    with_blocks(ptr, len, |blocks| match blocks {
        [a, b, ..] => encode_u8(blocks_equal(a, b, block_mode(mode)) as u8),
        _ => encode_u8(0),
    })
}

/// Compare two tapes sent as one list: `split` blocks of left, then right.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_compare_tapes(
    ptr: *const u8,
    len: usize,
    split: u32,
    block_mode_id: u32,
    tape_mode_id: u32,
) -> *mut u8 {
    with_blocks(ptr, len, |blocks| {
        let split = (split as usize).min(blocks.len());
        let (left, right) = blocks.split_at(split);
        let (l, r, identical) =
            compare_tapes(left, right, block_mode(block_mode_id), tape_mode(tape_mode_id));
        encode_comparison(&l, &r, identical)
    })
}

/// Blocks of the payload matching its first block, which is the needle. `skip`
/// is where the needle sits in the haystack, or `0xffff_ffff` when it is not in
/// it; the haystack starts at the payload's second block.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_find_matches(ptr: *const u8, len: usize, skip: u32, mode: u32) -> *mut u8 {
    with_blocks(ptr, len, |blocks| match blocks.split_first() {
        Some((needle, haystack)) => {
            let skip = if skip == u32::MAX { None } else { Some(skip as usize) };
            encode_u32s(&find_matches(needle, haystack, block_mode(mode), skip))
        }
        None => encode_u32s(&[]),
    })
}

/// Drop, add or shift bits of the payload's bit stream, or join all of them.
/// `op` is 0 drop, 1 add, 2 shift left, 3 shift right, 4 join.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded bit streams.
#[no_mangle]
pub unsafe extern "C" fn core_bits(ptr: *const u8, len: usize, op: u32, n: u32) -> *mut u8 {
    let payload = match decode_bit_data(slice(ptr, len)) {
        Ok(parts) => {
            let n = n as usize;
            let first = parts.first().cloned().unwrap_or(BitData { data: Vec::new(), used_bits: 8 });
            let out = match op {
                0 => drop_bits(&first, n),
                1 => add_bits(&first, n),
                2 => shift_left_bits(&first, n),
                3 => shift_right_bits(&first, n),
                _ => join_bits(&parts),
            };
            encode_bit_data(&out)
        }
        Err(e) => encode_error(&e.0),
    };
    finish(payload)
}

/// Reverse the bits of every byte.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_flip_bytes(ptr: *const u8, len: usize) -> *mut u8 {
    finish(encode_bytes(&flip_bytes(slice(ptr, len))))
}

/// Read a 'POKEs' custom info block.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_decode_pokes(ptr: *const u8, len: usize) -> *mut u8 {
    finish(match decode_pokes(slice(ptr, len)) {
        Ok(info) => encode_pokes_info(&info),
        Err(e) => encode_error(&e.0),
    })
}

/// Write a 'POKEs' custom info block.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded POKEs.
#[no_mangle]
pub unsafe extern "C" fn core_encode_pokes(ptr: *const u8, len: usize) -> *mut u8 {
    finish(match decode_pokes_info(slice(ptr, len)) {
        Ok(info) => encode_bytes(&encode_pokes(&info)),
        Err(e) => encode_error(&e.0),
    })
}

/// The editor's text for these POKEs.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded POKEs.
#[no_mangle]
pub unsafe extern "C" fn core_pokes_to_text(ptr: *const u8, len: usize, hex: u32) -> *mut u8 {
    finish(match decode_pokes_info(slice(ptr, len)) {
        Ok(info) => encode_opt_string(Some(&pokes_to_text(&info, hex != 0))),
        Err(e) => encode_error(&e.0),
    })
}

/// Parse the editor's text back into POKEs; a bad line comes back as an error.
///
/// # Safety
/// `ptr` must point at `len` readable bytes of Latin-1 text.
#[no_mangle]
pub unsafe extern "C" fn core_text_to_pokes(ptr: *const u8, len: usize, hex: u32) -> *mut u8 {
    let text = String::from_utf8_lossy(slice(ptr, len));
    finish(match text_to_pokes(&text, hex != 0) {
        Ok(info) => encode_pokes_info(&info),
        Err(message) => encode_error(&message),
    })
}

fn block_mode(id: u32) -> BlockCompareMode {
    match id {
        0 => BlockCompareMode::Data,
        2 => BlockCompareMode::DataTimingsPauses,
        _ => BlockCompareMode::DataTimings,
    }
}

fn tape_mode(id: u32) -> TapeCompareMode {
    match id {
        0 => TapeCompareMode::DataBlocks,
        2 => TapeCompareMode::All,
        _ => TapeCompareMode::IgnoreMetadata,
    }
}

// ---- the Spectrum side ----------------------------------------------------

/// All 256 characters of one of the character tables: 0 with tokens expanded,
/// 1 without, 2 the hex dump's single-cell version. The app builds the table
/// once instead of asking per byte. Takes an unused buffer so every entry point
/// has the same shape on the JavaScript side.
///
/// # Safety
/// `ptr`/`len` are ignored.
#[no_mangle]
pub unsafe extern "C" fn core_char_table(_ptr: *const u8, _len: usize, kind: u32) -> *mut u8 {
    finish(encode_strings(&char_table(kind)))
}

/// Render a screen dump into RGBA pixels. `flags`: 1 hide attributes, 2 flash phase.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_render_screen(ptr: *const u8, len: usize, offset: i32, flags: u32) -> *mut u8 {
    let opts = ScreenOptions { hide_attributes: flags & 1 != 0, flash_phase: flags & 2 != 0 };
    finish(encode_bytes(&render_screen(slice(ptr, len), i64::from(offset), opts)))
}

/// Does the screen's attribute area use FLASH anywhere?
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_has_flash(ptr: *const u8, len: usize, offset: i32) -> *mut u8 {
    finish(encode_u8(has_flash(slice(ptr, len), i64::from(offset)) as u8))
}

/// List a BASIC program area. `flags`: 1 show numbers, 2 128k tokens, 4 Spectrum format.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_list_basic(
    ptr: *const u8,
    len: usize,
    start: u32,
    end: u32,
    flags: u32,
) -> *mut u8 {
    let lines = list_basic(slice(ptr, len), start as usize, end as usize, basic_options(flags));
    finish(encode_basic_lines(&lines))
}

/// Render a listing as plain text.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded BASIC lines.
#[no_mangle]
pub unsafe extern "C" fn core_basic_to_text(ptr: *const u8, len: usize, flags: u32) -> *mut u8 {
    finish(match decode_basic_lines(slice(ptr, len)) {
        Ok(lines) => encode_opt_string(Some(&basic_to_text(&lines, basic_options(flags)))),
        Err(e) => encode_error(&e.0),
    })
}

/// List the variables area.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_list_variables(ptr: *const u8, len: usize, start: u32, end: u32) -> *mut u8 {
    finish(encode_variables(&list_variables(slice(ptr, len), start as usize, end as usize)))
}

/// Disassemble `count` instructions. `flags`: 1 hex, 2 ROM labels.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_disassemble(
    ptr: *const u8,
    len: usize,
    offset: u32,
    base: u32,
    count: u32,
    flags: u32,
) -> *mut u8 {
    let opts = DisOptions { hex: flags & 1 != 0, rom_labels: flags & 2 != 0 };
    let lines = disassemble(slice(ptr, len), offset as usize, base, count as usize, opts);
    finish(encode_dis_lines(&lines))
}

/// Decode a 5-byte Sinclair floating point number.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_decode_number(ptr: *const u8, len: usize, offset: u32) -> *mut u8 {
    finish(encode_f64(decode_number(slice(ptr, len), offset as usize)))
}

/// The listing's spelling of a number.
///
/// # Safety
/// `ptr`/`len` are ignored.
#[no_mangle]
pub unsafe extern "C" fn core_format_number(_ptr: *const u8, _len: usize, v: f64) -> *mut u8 {
    finish(encode_opt_string(Some(&format_number(v))))
}

fn basic_options(flags: u32) -> BasicOptions {
    BasicOptions { show_numbers: flags & 1 != 0, basic128: flags & 2 != 0, speccy_format: flags & 4 != 0 }
}

// ---- audio ----------------------------------------------------------------

/// The order the blocks play in. `flags`: 1 stop at a 48k stop block.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_playback_order(ptr: *const u8, len: usize, flags: u32) -> *mut u8 {
    let opts = FlowOptions { stop_at_48k: flags & 1 != 0, max_steps: None };
    with_blocks(ptr, len, |blocks| encode_u32s(&playback_order(blocks, opts)))
}

/// T-states the first block of the payload takes on its own.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_block_duration(ptr: *const u8, len: usize) -> *mut u8 {
    with_blocks(ptr, len, |blocks| encode_f64(blocks.first().map(block_duration).unwrap_or(0) as f64))
}

/// Seconds the whole tape takes, and the order it follows.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_tape_duration(ptr: *const u8, len: usize) -> *mut u8 {
    with_blocks(ptr, len, |blocks| {
        let (seconds, order) = tape_duration(blocks);
        encode_duration(seconds, &order)
    })
}

/// Where each block of the playback order starts.
///
/// # Safety
/// `ptr` must point at `len` bytes of a block list followed by an order.
#[no_mangle]
pub unsafe extern "C" fn core_playback_timeline(ptr: *const u8, len: usize) -> *mut u8 {
    with_order(ptr, len, |blocks, order| encode_timeline(&playback_timeline(blocks, order)))
}

/// How many samples a render of this order would produce.
///
/// # Safety
/// `ptr` must point at `len` bytes of a block list followed by an order.
#[no_mangle]
pub unsafe extern "C" fn core_render_length(ptr: *const u8, len: usize, sample_rate: u32) -> *mut u8 {
    with_order(ptr, len, |blocks, order| encode_f64(render_length(blocks, sample_rate, order) as f64))
}

/// Render the tape to samples. `mic` selects the MIC response over a square wave.
///
/// # Safety
/// `ptr` must point at `len` bytes of a block list followed by an order.
#[no_mangle]
pub unsafe extern "C" fn core_render_tape(
    ptr: *const u8,
    len: usize,
    sample_rate: u32,
    mic: u32,
    amplitude: f64,
) -> *mut u8 {
    let opts = RenderOptions { sample_rate, mic: mic != 0, amplitude };
    with_order(ptr, len, |blocks, order| encode_samples(&render_tape(blocks, opts, order)))
}

/// Render the tape straight to a WAV file, which saves copying the samples out
/// only to send them back in.
///
/// # Safety
/// `ptr` must point at `len` bytes of a block list followed by an order.
#[no_mangle]
pub unsafe extern "C" fn core_render_wav(
    ptr: *const u8,
    len: usize,
    sample_rate: u32,
    mic: u32,
    amplitude: f64,
    bits: u32,
) -> *mut u8 {
    let opts = RenderOptions { sample_rate, mic: mic != 0, amplitude };
    with_order(ptr, len, |blocks, order| {
        let samples = render_tape(blocks, opts, order);
        encode_bytes(&encode_wav(&samples, sample_rate, bits as u16))
    })
}

/// Encode samples (little-endian `f32`) as a WAV file.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_encode_wav(ptr: *const u8, len: usize, sample_rate: u32, bits: u32) -> *mut u8 {
    let raw = slice(ptr, len);
    // Stepping over the bytes rather than `chunks_exact(4)`: clippy 1.98 wants
    // `as_chunks`, which is newer than the oldest toolchain this builds on.
    let samples: Vec<f32> = (0..raw.len() / 4)
        .map(|i| f32::from_le_bytes([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2], raw[i * 4 + 3]]))
        .collect();
    finish(encode_bytes(&encode_wav(&samples, sample_rate, bits as u16)))
}

/// The pulses one block produces, for callers that want the edge stream itself.
///
/// # Safety
/// `ptr` must point at `len` bytes of wire-encoded block list.
#[no_mangle]
pub unsafe extern "C" fn core_block_pulses(ptr: *const u8, len: usize) -> *mut u8 {
    with_blocks(ptr, len, |blocks| {
        let mut sink = RecordingSink::default();
        if let Some(b) = blocks.first() {
            emit_block(&mut sink, b);
        }
        encode_pulses(&sink.pulses)
    })
}

/// CSW v2 RLE pulse lengths, in samples.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn core_decode_csw_rle(ptr: *const u8, len: usize) -> *mut u8 {
    finish(encode_u32s(&decode_csw_rle(slice(ptr, len))))
}

/// Decode a block list plus a playback order, run `f` over them and lay the
/// answer out the way [`respond`] does.
unsafe fn with_order(ptr: *const u8, len: usize, f: impl Fn(&[Block], &[u32]) -> Vec<u8>) -> *mut u8 {
    let payload = match decode_blocks_and_order(slice(ptr, len)) {
        Ok((blocks, order)) => f(&blocks, &order),
        Err(e) => encode_error(&e.0),
    };
    finish(payload)
}
