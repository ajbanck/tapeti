// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Writer tests: the round trips of `web/test/tzx.test.ts`, the version rules and the
//! TAP export.

use tapeti_core::parser::{parse_tap, parse_tzx};
use tapeti_core::types::{ArchiveEntry, Block, Body, HardwareEntry, PilotRun, SelectEntry, SymDef};
use tapeti_core::writer::{
    check_tap, check_tzx, required_version, save_version, serialize_block, serialize_tap, serialize_tzx,
    Version, WriteError,
};

fn b(body: Body) -> Block {
    Block::new(body)
}

fn data() -> Vec<u8> {
    vec![0x00, 3, 65, 66, 67, 0x55]
}

/// One of every block type the editor can create, with content in the data ones.
fn every_block() -> Vec<Block> {
    vec![
        b(Body::Standard { pause: 1000, data: data() }),
        b(Body::Turbo {
            pilot: 2168,
            sync1: 667,
            sync2: 735,
            zero: 855,
            one: 1710,
            pilot_len: 3223,
            used_bits: 8,
            pause: 1000,
            data: data(),
        }),
        b(Body::PureTone { pulse_len: 2168, count: 8063 }),
        b(Body::PulseSeq { pulses: vec![667, 735] }),
        b(Body::PureData { zero: 855, one: 1710, used_bits: 8, pause: 1000, data: data() }),
        b(Body::Direct { tstates: 79, pause: 0, used_bits: 8, data: data() }),
        b(Body::Csw { pause: 0, sample_rate: 44100, compression: 1, pulse_count: 0, data: data() }),
        b(Body::Generalized {
            pause: 1000,
            totp: 2,
            npp: 2,
            pilot_symbols: vec![
                SymDef { flags: 0, pulses: vec![2168] },
                SymDef { flags: 1, pulses: vec![667, 735] },
            ],
            pilot_stream: vec![PilotRun { symbol: 0, reps: 8063 }, PilotRun { symbol: 1, reps: 1 }],
            totd: 48,
            npd: 2,
            data_symbols: vec![
                SymDef { flags: 0, pulses: vec![855, 855] },
                SymDef { flags: 0, pulses: vec![1710, 1710] },
            ],
            data: data(),
        }),
        b(Body::Pause { pause: 1000 }),
        b(Body::GroupStart { name: "Group".into() }),
        b(Body::GroupEnd),
        b(Body::Jump { offset: 1 }),
        b(Body::LoopStart { count: 2 }),
        b(Body::LoopEnd),
        b(Body::Call { offsets: vec![1] }),
        b(Body::Return),
        b(Body::Select {
            entries: vec![tapeti_core::types::SelectEntry { offset: 1, text: "Selection".into() }],
        }),
        b(Body::Stop48),
        b(Body::SignalLevel { level: 0 }),
        b(Body::Text { text: String::new() }),
        b(Body::Message { time: 5, text: String::new() }),
        b(Body::Archive { entries: vec![ArchiveEntry { kind: 0, text: String::new() }] }),
        b(Body::Hardware { entries: vec![HardwareEntry { kind: 0, id: 1, info: 0 }] }),
        b(Body::Custom { ident: "POKEs           ".into(), data: data() }),
        b(Body::Glue { raw: vec![0x58, 0x54, 0x61, 0x70, 0x65, 0x21, 0x1a, 1, 20] }),
    ]
}

#[test]
fn serializes_every_block_and_parses_it_back() {
    let blocks = every_block();
    let bytes = serialize_tzx(&blocks, None).unwrap();
    let parsed = parse_tzx(&bytes).unwrap();
    assert_eq!(parsed.warnings, Vec::<String>::new());
    assert_eq!(parsed.blocks.len(), blocks.len());
    for (got, want) in parsed.blocks.iter().zip(&blocks) {
        assert_eq!(got.body, want.body, "block {:02x}", want.id());
    }
    // And writing them again gives the same file.
    assert_eq!(serialize_tzx(&parsed.blocks, None).unwrap(), bytes);
}

#[test]
fn a_block_writes_the_same_bytes_alone_as_in_a_tape() {
    let blocks = every_block();
    let tape = serialize_tzx(&blocks, Some(Version { major: 1, minor: 20 })).unwrap();
    let mut concatenated = Vec::new();
    for block in &blocks {
        concatenated.extend_from_slice(&serialize_block(block));
    }
    assert_eq!(&tape[10..], &concatenated[..]);
}

#[test]
fn computes_the_lowest_possible_version() {
    let v = |body: Body| required_version(&[b(body)]);
    assert_eq!(v(Body::Standard { pause: 0, data: vec![] }), Version { major: 1, minor: 0 });
    assert_eq!(v(Body::Custom { ident: String::new(), data: vec![] }), Version { major: 1, minor: 1 });
    assert_eq!(v(Body::LoopStart { count: 2 }), Version { major: 1, minor: 10 });
    assert_eq!(v(Body::Stop48), Version { major: 1, minor: 12 });
    assert_eq!(v(Body::SignalLevel { level: 1 }), Version { major: 1, minor: 20 });
    // An archive entry's kind and any line breaks in its text set its own version floor.
    let archive = |kind: u8, text: &str| {
        required_version(&[b(Body::Archive { entries: vec![ArchiveEntry { kind, text: text.into() }] })])
    };
    assert_eq!(archive(0, "Title"), Version { major: 1, minor: 0 });
    assert_eq!(archive(4, "en"), Version { major: 1, minor: 10 });
    assert_eq!(archive(0, "two\nlines"), Version { major: 1, minor: 10 });
    assert_eq!(archive(6, "5 pounds"), Version { major: 1, minor: 12 });
    // Hardware ids move the floor about as unevenly.
    let hardware = |kind: u8, id: u8| {
        required_version(&[b(Body::Hardware { entries: vec![HardwareEntry { kind, id, info: 0 }] })])
    };
    assert_eq!(hardware(0, 0x01), Version { major: 1, minor: 0 });
    assert_eq!(hardware(0, 0x16), Version { major: 1, minor: 2 });
    assert_eq!(hardware(0, 0x1a), Version { major: 1, minor: 12 });
    assert_eq!(hardware(0, 0x1c), Version { major: 1, minor: 13 });
    assert_eq!(hardware(0, 0x20), Version { major: 1, minor: 20 });
    assert_eq!(hardware(0x10, 0), Version { major: 1, minor: 20 });
    // We cannot judge an unknown block, so assume the newest spec.
    assert_eq!(
        required_version(&[b(Body::Unknown { id: 0x5b, raw: vec![] })]),
        Version { major: 1, minor: 20 }
    );
    // The highest floor in the tape wins.
    assert_eq!(required_version(&every_block()), Version { major: 1, minor: 20 });
}

#[test]
fn keeps_the_loaded_version_when_it_is_newer() {
    let plain = [b(Body::Standard { pause: 0, data: vec![] })];
    assert_eq!(save_version(&plain, None), Version { major: 1, minor: 0 });
    assert_eq!(save_version(&plain, Some(Version { major: 1, minor: 20 })), Version { major: 1, minor: 20 });
    assert_eq!(
        save_version(&[b(Body::SignalLevel { level: 0 })], Some(Version { major: 1, minor: 0 })),
        Version { major: 1, minor: 20 }
    );
    // An unaltered 1.20 tape of plain blocks round-trips byte for byte.
    let bytes = serialize_tzx(&plain, Some(Version { major: 1, minor: 20 })).unwrap();
    let p = parse_tzx(&bytes).unwrap();
    let v = save_version(&p.blocks, Some(Version { major: p.major, minor: p.minor }));
    assert_eq!(serialize_tzx(&p.blocks, Some(v)).unwrap(), bytes);
}

#[test]
fn writes_tap_files_and_reports_what_it_dropped() {
    let blocks = vec![
        b(Body::Standard { pause: 1000, data: vec![0, 1, 2] }),
        b(Body::GroupStart { name: "meta".into() }), // dropped silently
        b(Body::PureTone { pulse_len: 2168, count: 10 }), // cannot be a TAP block
        b(Body::PureData { zero: 855, one: 1710, used_bits: 8, pause: 0, data: vec![0xff, 9] }),
        b(Body::Generalized {
            pause: 0,
            totp: 0,
            npp: 2,
            pilot_symbols: vec![],
            pilot_stream: vec![],
            totd: 0,
            npd: 2,
            data_symbols: vec![],
            data: vec![],
        }), // no data, so nothing to write
    ];
    let (bytes, skipped) = serialize_tap(&blocks).unwrap();
    assert_eq!(bytes, vec![3, 0, 0, 1, 2, 2, 0, 0xff, 9]);
    assert_eq!(skipped, vec![2, 4]);
    // What comes back is standard blocks with the default pause.
    let back = parse_tap(&bytes);
    assert_eq!(back.warnings, Vec::<String>::new());
    assert_eq!(back.blocks.len(), 2);
    assert_eq!(back.blocks[0].body, Body::Standard { pause: 1000, data: vec![0, 1, 2] });
}

#[test]
fn writes_what_the_typescript_used_to_paper_over() {
    // A group name longer than its byte length field, cut at 255.
    let long = "x".repeat(300);
    let bytes = serialize_block(&b(Body::GroupStart { name: long }));
    assert_eq!(bytes[0], 0x21);
    assert_eq!(bytes[1], 255);
    assert_eq!(bytes.len(), 2 + 255);

    // A short ident is padded to 16 characters.
    let bytes = serialize_block(&b(Body::Custom { ident: "POKEs".into(), data: vec![] }));
    assert_eq!(&bytes[1..17], b"POKEs           ");

    // A glue block that is not 9 bytes is replaced by the canonical one.
    let bytes = serialize_block(&b(Body::Glue { raw: vec![1, 2] }));
    assert_eq!(&bytes[1..], b"XTape!\x1a\x01\x14");

    // Symbol pulses shorter than npp are padded with zeroes, and data shorter
    // than the symbol count needs is padded too.
    let bytes = serialize_block(&b(Body::Generalized {
        pause: 0,
        totp: 0,
        npp: 2,
        pilot_symbols: vec![],
        pilot_stream: vec![],
        totd: 16,
        npd: 3,
        data_symbols: vec![SymDef { flags: 0, pulses: vec![855] }, SymDef { flags: 0, pulses: vec![] }],
        data: vec![0xaa],
    }));
    let parsed = parse_tzx(
        &serialize_tzx(
            &[b(Body::Generalized {
                pause: 0,
                totp: 0,
                npp: 2,
                pilot_symbols: vec![],
                pilot_stream: vec![],
                totd: 16,
                npd: 3,
                data_symbols: vec![
                    SymDef { flags: 0, pulses: vec![855] },
                    SymDef { flags: 0, pulses: vec![] },
                ],
                data: vec![0xaa],
            })],
            None,
        )
        .unwrap(),
    )
    .unwrap();
    match &parsed.blocks[0].body {
        Body::Generalized { data_symbols, data, .. } => {
            assert_eq!(data_symbols[0].pulses, vec![855]); // the padding is trimmed on the way back
            assert_eq!(data, &vec![0xaa, 0x00]); // 16 symbols of 1 bit = 2 bytes
        }
        other => panic!("expected a generalized block, got {other:?}"),
    }
    assert!(bytes.len() > 10);
}

/// What `serialize_tzx` says of `body` as the second block of a tape.
fn refusal(body: Body) -> Option<String> {
    let blocks = [b(Body::Pause { pause: 100 }), b(body)];
    match serialize_tzx(&blocks, None) {
        Ok(_) => None,
        Err(e) => {
            assert_eq!(e.block, 1);
            Some(e.what)
        }
    }
}

#[test]
fn a_block_too_large_for_its_field_is_refused_rather_than_cut() {
    let symbols = |n: usize| vec![SymDef { flags: 0, pulses: vec![855] }; n];
    let generalized = |n: usize| Body::Generalized {
        pause: 0,
        totp: 0,
        npp: 1,
        pilot_symbols: vec![],
        pilot_stream: vec![],
        totd: 1,
        npd: 1,
        data_symbols: symbols(n),
        data: vec![0],
    };
    let select = |n: usize, text: &str| Body::Select {
        entries: vec![SelectEntry { offset: 1, text: text.to_string() }; n],
    };
    let cases: [(Body, Body, &str); 9] = [
        (
            Body::Standard { pause: 0, data: vec![0; 65535] },
            Body::Standard { pause: 0, data: vec![0; 65536] },
            "a standard block holds at most 65535 bytes, and this one has 65536",
        ),
        (
            Body::Direct { tstates: 79, pause: 0, used_bits: 8, data: vec![0; 0xFF_FFFF] },
            Body::Direct { tstates: 79, pause: 0, used_bits: 8, data: vec![0; 0x100_0000] },
            "a direct recording holds at most 16777215 bytes, and this one has 16777216",
        ),
        (
            Body::PulseSeq { pulses: vec![100; 255] },
            Body::PulseSeq { pulses: vec![100; 256] },
            "a pulse sequence holds at most 255 pulses, and this one has 256",
        ),
        (
            Body::Csw { pause: 0, sample_rate: 0xFF_FFFF, compression: 1, pulse_count: 1, data: vec![1] },
            Body::Csw { pause: 0, sample_rate: 0x100_0000, compression: 1, pulse_count: 1, data: vec![1] },
            "a CSW block's rate holds at most 16777215 Hz, and this one has 16777216",
        ),
        (
            generalized(256),
            generalized(257),
            "a generalized block's data alphabet holds at most 256 symbols, and this one has 257",
        ),
        (
            Body::Call { offsets: vec![1; 65535] },
            Body::Call { offsets: vec![1; 65536] },
            "a call sequence holds at most 65535 calls, and this one has 65536",
        ),
        (
            select(255, "a"),
            select(256, "a"),
            "a select block holds at most 255 entries, and this one has 256",
        ),
        (
            Body::Hardware { entries: vec![HardwareEntry { kind: 0, id: 0, info: 0 }; 255] },
            Body::Hardware { entries: vec![HardwareEntry { kind: 0, id: 0, info: 0 }; 256] },
            "a hardware info block holds at most 255 entries, and this one has 256",
        ),
        (
            Body::Archive { entries: vec![ArchiveEntry { kind: 0, text: "x".repeat(254) }; 255] },
            Body::Archive { entries: vec![ArchiveEntry { kind: 0, text: "x".repeat(255) }; 255] },
            "an archive info block holds at most 65535 bytes, and this one has 65536",
        ),
    ];
    for (fits, too_large, what) in cases {
        assert_eq!(refusal(fits.clone()), None, "{what}");
        let parsed = parse_tzx(&serialize_tzx(&[b(fits.clone())], None).unwrap()).unwrap();
        assert_eq!(parsed.blocks, [b(fits)], "the largest that fits reads back whole: {what}");
        assert_eq!(refusal(too_large).as_deref(), Some(what));
    }
    // 255 entries whose texts are cut at 255 bytes still make more than 65,535 bytes.
    let long = select(255, &"x".repeat(300));
    assert_eq!(
        refusal(long).as_deref(),
        Some("a select block holds at most 65535 bytes, and this one has 65791")
    );
}

#[test]
fn a_tap_block_is_refused_past_its_16_bit_length_where_a_tzx_block_is_not() {
    let turbo = |n: usize| {
        b(Body::Turbo {
            pilot: 2168,
            sync1: 667,
            sync2: 735,
            zero: 855,
            one: 1710,
            pilot_len: 3223,
            used_bits: 8,
            pause: 1000,
            data: vec![0; n],
        })
    };
    let blocks = [turbo(10), turbo(65536)];
    assert_eq!(check_tzx(&blocks), Ok(()));
    let refused =
        WriteError { block: 1, what: "a TAP block holds at most 65535 bytes, and this one has 65536".into() };
    assert_eq!(check_tap(&blocks), Err(refused.clone()));
    assert_eq!(serialize_tap(&blocks), Err(refused.clone()));
    assert_eq!(refused.to_string(), "block 1: a TAP block holds at most 65535 bytes, and this one has 65536");
    assert!(serialize_tap(&[turbo(65535)]).is_ok());
}
