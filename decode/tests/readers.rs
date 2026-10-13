// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The recording readers, on files built here byte by byte and on WAVs from `tapeti-core`'s
//! encoder. The last test cuts and damages every one of them and expects an error, never a
//! panic.

use tapeti_decode::readers::{read, Channel, Csw, Encoding, ReadError, Recording, Sound, MAX_SECONDS};

fn sound<'a>(name: &str, bytes: &'a [u8], channel: Channel) -> Sound<'a> {
    match read(name, bytes, channel) {
        Ok(Recording::Sound(s)) => s,
        Ok(_) => panic!("{name}: not read as sound"),
        Err(e) => panic!("{name}: {e}"),
    }
}

fn csw(bytes: &[u8]) -> Csw<'_> {
    match read("x.csw", bytes, Channel::Mix) {
        Ok(Recording::Csw(c)) => c,
        Ok(_) => panic!("not read as CSW"),
        Err(e) => panic!("{e}"),
    }
}

fn error(name: &str, bytes: &[u8]) -> ReadError {
    match read(name, bytes, Channel::Mix) {
        Err(e) => e,
        Ok(_) => panic!("{name}: read, but should not be"),
    }
}

fn chunk(id: &[u8], body: &[u8], big_endian: bool) -> Vec<u8> {
    let len = body.len() as u32;
    let mut out = id.to_vec();
    out.extend(if big_endian { len.to_be_bytes() } else { len.to_le_bytes() });
    out.extend(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn fmt_chunk(tag: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
    let align = channels * bits / 8;
    let mut b = Vec::new();
    b.extend(tag.to_le_bytes());
    b.extend(channels.to_le_bytes());
    b.extend(rate.to_le_bytes());
    b.extend((rate * u32::from(align)).to_le_bytes());
    b.extend(align.to_le_bytes());
    b.extend(bits.to_le_bytes());
    b
}

fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32 + 4).to_le_bytes());
    out.extend(b"WAVE");
    out.extend(body);
    out
}

fn wav(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
    riff(&[chunk(b"fmt ", &fmt_chunk(tag, channels, rate, bits), false), chunk(b"data", data, false)])
}

fn i16s(values: &[i16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn voc(blocks: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"Creative Voice File\x1a".to_vec();
    out.extend(0x1Au16.to_le_bytes());
    out.extend(0x010Au16.to_le_bytes());
    out.extend(0x1129u16.to_le_bytes());
    for (kind, body) in blocks {
        out.push(*kind);
        out.extend(&(body.len() as u32).to_le_bytes()[..3]);
        out.extend(body);
    }
    out.push(0);
    out
}

/// A VOC sound block: rate divisor, codec 0, then the samples.
fn voc_sound(divisor: u8, pcm: &[u8]) -> (u8, Vec<u8>) {
    (1, [&[divisor, 0][..], pcm].concat())
}

fn voc_silence(samples: u16, divisor: u8) -> (u8, Vec<u8>) {
    let [lo, hi] = (samples - 1).to_le_bytes();
    (3, vec![lo, hi, divisor])
}

fn iff(rate: u16, compression: u8, body: &[u8]) -> Vec<u8> {
    let mut vhdr = vec![0u8; 20];
    vhdr[12..14].copy_from_slice(&rate.to_be_bytes());
    vhdr[14] = 1;
    vhdr[15] = compression;
    let chunks =
        [chunk(b"NAME", b"tap", true), chunk(b"VHDR", &vhdr, true), chunk(b"BODY", body, true)].concat();
    let mut out = b"FORM".to_vec();
    out.extend((chunks.len() as u32 + 4).to_be_bytes());
    out.extend(b"8SVX");
    out.extend(chunks);
    out
}

fn csw_v1(rate: u16, flags: u8, data: &[u8]) -> Vec<u8> {
    let mut out = b"Compressed Square Wave\x1a".to_vec();
    out.extend([1, 1]);
    out.extend(rate.to_le_bytes());
    out.extend([1, flags, 0, 0, 0]);
    out.extend(data);
    out
}

fn csw_v2(rate: u32, count: u32, compression: u8, flags: u8, extension: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = b"Compressed Square Wave\x1a".to_vec();
    out.extend([2, 0]);
    out.extend(rate.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend([compression, flags, extension.len() as u8]);
    out.extend(b"Tapeti tests    ");
    out.extend(extension);
    out.extend(data);
    out
}

fn out_records(records: &[(u16, u16)]) -> Vec<u8> {
    records
        .iter()
        .flat_map(|&(counter, port)| {
            [counter.to_le_bytes(), port.to_le_bytes()].concat().into_iter().chain([0])
        })
        .collect()
}

#[test]
fn a_wav_from_the_core_encoder_reads_back() {
    let values = [0.0, 0.5, -0.5, 1.0, -1.0];
    let bytes = tapeti_core::audio::encode_wav(&values, 22050, 16);
    let s = sound("a.wav", &bytes, Channel::Mix);
    assert_eq!((s.format, s.rate_hz, s.channels, s.encoding, s.len()), ("WAV", 22050, 1, Encoding::I16, 5));
    for (got, want) in s.samples().zip(values) {
        assert!((got - want).abs() < 1.0 / 16384.0, "{got} for {want}");
    }
    let bytes = tapeti_core::audio::encode_wav(&values, 11025, 8);
    let s = sound("a.wav", &bytes, Channel::Mix);
    assert_eq!(s.encoding, Encoding::U8);
    for (got, want) in s.samples().zip(values) {
        assert!((got - want).abs() <= 1.0 / 128.0, "{got} for {want}");
    }
}

#[test]
fn a_stereo_wav_reads_one_channel_or_the_mix() {
    let bytes = wav(1, 2, 44100, 16, &i16s(&[1000, -2000, 3000, 4000]));
    let read_as =
        |c| sound("s.wav", &bytes, c).samples().map(|v| (v * 32768.0).round() as i32).collect::<Vec<_>>();
    assert_eq!(read_as(Channel::Left), [1000, 3000]);
    assert_eq!(read_as(Channel::Right), [-2000, 4000]);
    assert_eq!(read_as(Channel::Mix), [-500, 3500]);
}

#[test]
fn wavs_of_24_bits_float_and_the_extensible_header_are_read() {
    let s24 = wav(1, 1, 48000, 24, &[0x00, 0x00, 0x40, 0x00, 0x00, 0xC0]);
    assert_eq!(sound("a.wav", &s24, Channel::Mix).samples().collect::<Vec<_>>(), [0.5, -0.5]);
    let f32s: Vec<u8> = [0.25f32, -0.75].iter().flat_map(|v| v.to_le_bytes()).collect();
    let float = wav(3, 1, 48000, 32, &f32s);
    assert_eq!(sound("a.wav", &float, Channel::Mix).samples().collect::<Vec<_>>(), [0.25, -0.75]);
    // The extensible header: the plain 16 bytes, a 2-byte extension size, valid bits, the
    // channel mask, then the sub-format GUID whose first two bytes are the real format.
    let mut fmt = fmt_chunk(0xFFFE, 1, 44100, 16);
    fmt.extend(22u16.to_le_bytes());
    fmt.extend(16u16.to_le_bytes());
    fmt.extend(4u32.to_le_bytes());
    fmt.extend(1u16.to_le_bytes());
    fmt.extend([0u8; 14]);
    let ext = riff(&[chunk(b"fmt ", &fmt, false), chunk(b"data", &i16s(&[16384]), false)]);
    assert_eq!(sound("a.wav", &ext, Channel::Mix).samples().collect::<Vec<_>>(), [0.5]);
}

#[test]
fn a_wav_skips_chunks_it_does_not_know_and_their_padding() {
    let bytes = riff(&[
        chunk(b"LIST", b"odd", false),
        chunk(b"fmt ", &fmt_chunk(1, 1, 8000, 8), false),
        chunk(b"fact", &[1, 2, 3, 4, 5], false),
        chunk(b"data", &[128, 255, 0], false),
    ]);
    let s = sound("a.wav", &bytes, Channel::Mix);
    assert_eq!(s.samples().collect::<Vec<_>>(), [0.0, 127.0 / 128.0, -1.0]);
}

#[test]
fn a_cut_short_wav_reads_what_is_there_and_says_how_much_is_missing() {
    let mut bytes = wav(1, 1, 8000, 16, &i16s(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
    bytes.truncate(bytes.len() - 7);
    let s = sound("a.wav", &bytes, Channel::Mix);
    assert_eq!(s.len(), 6, "the half sample at the end is dropped");
    assert_eq!(s.missing_bytes, 7);
}

#[test]
fn a_wav_says_what_it_does_not_read() {
    let no_fmt = riff(&[chunk(b"data", &[1, 2], false)]);
    let no_data = riff(&[chunk(b"fmt ", &fmt_chunk(1, 1, 8000, 8), false)]);
    let cases = [
        (no_fmt, "WAV: there is no fmt chunk"),
        (no_data, "WAV: there is no data chunk"),
        (wav(2, 1, 8000, 4, &[0]), "WAV: compression type 2 is not supported"),
        (wav(1, 1, 8000, 12, &[0, 0]), "WAV: 12-bit PCM is not supported"),
        (wav(1, 6, 8000, 8, &[0; 6]), "WAV: 6 channels is not supported"),
        (wav(1, 1, 0, 8, &[0]), "WAV: the sample rate is 0"),
    ];
    for (bytes, message) in cases {
        assert_eq!(error("a.wav", &bytes).to_string(), message);
    }
}

#[test]
fn a_voc_joins_its_sound_continuation_and_silence_and_lists_what_it_skipped() {
    // Divisor 156 is 1,000,000 / 100 = 10,000 Hz.
    let bytes =
        voc(&[voc_sound(156, &[128, 192]), (5, b"hi\0".to_vec()), (2, vec![64]), voc_silence(3, 156)]);
    let s = sound("a.voc", &bytes, Channel::Left);
    assert_eq!((s.format, s.rate_hz, s.channels, s.len()), ("VOC", 10000, 1, 6));
    assert_eq!(s.samples().collect::<Vec<_>>(), [0.0, 0.5, -0.5, 0.0, 0.0, 0.0]);
    assert_eq!(s.skipped, ["block type 5 at byte 34"]);
}

#[test]
fn a_voc_extended_block_sets_the_rate_and_silence_keeps_its_duration() {
    // Time constant 59,731 is 256,000,000 / 5,805 = 44,100 Hz; the sound block's divisor of 0
    // is then not used.
    let bytes = voc(&[(8, vec![0x53, 0xE9, 0, 0]), voc_sound(0, &[128; 4])]);
    assert_eq!(sound("a.voc", &bytes, Channel::Mix).rate_hz, 44100);
    // 100 samples of silence at 5,000 Hz (divisor 56) are 200 at 10,000 Hz.
    let bytes = voc(&[voc_sound(156, &[128]), voc_silence(100, 56)]);
    assert_eq!(sound("a.voc", &bytes, Channel::Mix).len(), 201);
}

#[test]
fn a_voc_says_what_it_does_not_read() {
    let cases = [
        (
            voc(&[voc_sound(156, &[1]), voc_sound(100, &[2])]),
            "VOC: a change of sample rate at byte 33 is not supported",
        ),
        (voc(&[(2, vec![1])]), "VOC: a continuation block at byte 26 continues nothing"),
        (
            voc(&[(8, vec![0, 0x80, 0, 1]), voc_sound(0, &[1])]),
            "VOC: stereo sound at byte 26 is not supported",
        ),
        (voc(&[(1, vec![156, 4, 0, 0])]), "VOC: codec 4 at byte 26 is not supported"),
        (voc(&[(9, vec![0; 12])]), "VOC: block type 9 at byte 26 is not supported"),
        (voc(&[(5, b"text\0".to_vec())]), "VOC: there is no sound"),
    ];
    for (bytes, message) in cases {
        assert_eq!(error("a.voc", &bytes).to_string(), message);
    }
}

#[test]
fn a_voc_whose_silence_claims_hours_is_refused_before_it_is_built() {
    // Each block is 65,536 samples at 3,906 Hz, 16.8 s; about 860 of them pass four hours.
    let hours = MAX_SECONDS / 3600;
    let blocks: Vec<_> =
        std::iter::once(voc_sound(156, &[128])).chain((0..1000).map(|_| voc_silence(65535, 0))).collect();
    let message = error("a.voc", &voc(&blocks)).to_string();
    assert_eq!(message, format!("VOC: its blocks add up to more than {hours} hours of sound"));
}

#[test]
fn an_iff_reads_signed_bytes_past_a_padded_chunk() {
    let bytes = iff(8000, 0, &[0x00, 0x7F, 0x80]);
    let s = sound("a.iff", &bytes, Channel::Mix);
    assert_eq!((s.format, s.rate_hz, s.encoding), ("IFF/8SVX", 8000, Encoding::I8));
    assert_eq!(s.samples().collect::<Vec<_>>(), [0.0, 127.0 / 128.0, -1.0]);
    let mut cut = iff(8000, 0, &[1, 2, 3, 4]);
    cut.truncate(cut.len() - 3);
    assert_eq!(sound("a.iff", &cut, Channel::Mix).missing_bytes, 3);
}

#[test]
fn an_iff_says_what_it_does_not_read() {
    assert_eq!(error("a.iff", &iff(8000, 1, &[1])).to_string(), "IFF/8SVX: compression 1 is not supported");
    assert_eq!(error("a.iff", &iff(0, 0, &[1])).to_string(), "IFF/8SVX: the sample rate is 0");
}

#[test]
fn a_csw_gives_its_run_lengths_and_starting_level() {
    // A run over 255 samples is a 0 byte and the length in 32 bits.
    let data = [3, 0, 0x10, 0x27, 0, 0, 5];
    let file = csw_v1(44100, 1, &data);
    let c = csw(&file);
    assert_eq!(
        (c.version, c.rate_hz, c.compression, c.pulse_count, c.starts_high),
        ((1, 1), 44100, 1, None, true)
    );
    assert_eq!(c.pulses(None).unwrap(), [3, 10000, 5]);
    let file = csw_v2(22050, 3, 1, 0, b"ab", &data);
    let c = csw(&file);
    assert_eq!(
        (c.version, c.rate_hz, c.pulse_count, c.starts_high, c.data),
        ((2, 0), 22050, Some(3), false, &data[..])
    );
    assert_eq!(c.pulses(None).unwrap(), [3, 10000, 5]);
}

#[test]
fn a_csw_whose_count_disagrees_with_its_data_is_refused() {
    let file = csw_v2(22050, 4, 1, 0, b"", &[3, 4, 5]);
    let c = csw(&file);
    assert_eq!(c.pulses(None).unwrap_err().to_string(), "CSW: the header counts 4 pulses, the data holds 3");
}

#[test]
fn z_rle_data_is_inflated_by_the_caller() {
    let file = csw_v2(44100, 2, 2, 0, b"", b"zlib bytes");
    let c = csw(&file);
    assert_eq!(c.data, b"zlib bytes");
    let refused = c.pulses(None).unwrap_err();
    assert_eq!(refused.to_string(), "CSW: Z-RLE data that has not been inflated is not supported");
    assert_eq!(c.pulses(Some(&[4, 5])).unwrap(), [4, 5]);
    let v1 = {
        let mut b = csw_v1(44100, 0, &[1]);
        b[0x1B] = 2;
        b
    };
    assert_eq!(error("a.csw", &v1).to_string(), "CSW: compression 2 in a version 1 file is not supported");
}

#[test]
fn an_out_trace_turns_writes_to_the_tape_port_into_pulses() {
    let records = [
        (100, 0x00FE),   // edge: 100
        (150, 0x10FE),   // edge on another FE port: 50
        (160, 0x001F),   // another port: not an edge
        (0xFFFE, 0),     // ignored
        (0xFFFF, 65000), // the counter wraps after 65,000
        (200, 0x00FE),   // edge: 65,000 - 150 + 200
        (300, 0x0001),   // the tail after the last edge: 100
    ];
    let pulses = match read("TAPE.OUT", &out_records(&records), Channel::Mix) {
        Ok(Recording::Out(p)) => p,
        _ => panic!("not read as an OUT trace"),
    };
    assert_eq!(pulses, [100, 50, 65050, 100]);
}

#[test]
fn an_out_trace_says_what_is_wrong_with_it() {
    assert_eq!(error("a.out", &[0; 7]).to_string(), "OUT: 7 bytes is not a whole number of records");
    let back = out_records(&[(100, 0xFE), (50, 0xFE)]);
    assert_eq!(error("a.out", &back).to_string(), "OUT: record 1 goes back in time, from 100 to 50");
    let quiet = out_records(&[(100, 0x1F)]);
    assert_eq!(error("a.out", &quiet).to_string(), "OUT: there is no write to the tape port");
}

#[test]
fn a_file_is_known_by_its_signature_and_an_out_trace_by_its_name() {
    let wav = wav(1, 1, 8000, 8, &[128]);
    assert!(matches!(read("tape.bin", &wav, Channel::Mix), Ok(Recording::Sound(_))));
    assert!(matches!(read("tape", &csw_v1(8000, 0, &[1]), Channel::Mix), Ok(Recording::Csw(_))));
    let trace = out_records(&[(10, 0xFE)]);
    assert!(matches!(read("tape.Out", &trace, Channel::Mix), Ok(Recording::Out(_))));
    assert_eq!(error("tape.wav", &trace), ReadError::Unrecognised);
    assert_eq!(error("tape.tzx", b"ZXTape!\x1a\x01\x14"), ReadError::Unrecognised);
}

#[test]
fn no_reader_panics_on_a_cut_or_damaged_file() {
    let files: Vec<(&str, Vec<u8>)> = vec![
        ("a.wav", wav(1, 2, 8000, 16, &i16s(&[1, -1, 2, -2]))),
        ("a.wav", wav(3, 1, 8000, 32, &[0; 8])),
        (
            "a.voc",
            voc(&[(8, vec![0x53, 0xE9, 0, 0]), voc_sound(0, &[1, 2]), (2, vec![3]), voc_silence(4, 156)]),
        ),
        ("a.iff", iff(8000, 0, &[1, 2, 3])),
        ("a.csw", csw_v1(8000, 1, &[3, 0, 1, 2, 3, 4])),
        ("a.csw", csw_v2(8000, 2, 1, 0, b"x", &[3, 4])),
        ("a.out", out_records(&[(10, 0xFE), (0xFFFF, 20), (5, 0xFE)])),
    ];
    let exercise = |name: &str, bytes: &[u8]| match read(name, bytes, Channel::Mix) {
        Ok(Recording::Sound(s)) => {
            let _ = s.samples().count();
        }
        Ok(Recording::Csw(c)) => {
            let _ = c.pulses(Some(&[0, 0xFF]));
            let _ = c.pulses(None);
        }
        _ => {}
    };
    for (name, bytes) in &files {
        for len in 0..=bytes.len() {
            exercise(name, &bytes[..len]);
        }
        for at in 0..bytes.len() {
            for value in [0x00, 0x01, 0x7F, 0x80, 0xFF] {
                let mut damaged = bytes.clone();
                damaged[at] = value;
                exercise(name, &damaged);
            }
        }
    }
}
