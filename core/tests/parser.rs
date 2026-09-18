//! Parser tests. Two halves:
//!
//! * `fixtures` — every dump in `tests/fixtures/`, generated from the
//!   TypeScript parser by `node scripts/dump-blocks.mjs`, must be reproduced
//!   byte for byte by this crate. That is the differential test.
//! * the rest — the hand-built cases from `test/tzx.test.ts`, ported.

use std::path::{Path, PathBuf};
use tapeti_core::dump::dump_tape;
use tapeti_core::parser::{bits_per_symbol, is_tzx, parse_tap, parse_tape, parse_tzx};
use tapeti_core::types::Body;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn tzx(body: &[u8]) -> Vec<u8> {
    let mut v = b"ZXTape!\x1a\x01\x14".to_vec();
    v.extend_from_slice(body);
    v
}

#[test]
fn matches_the_typescript_dumps() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixtures directory")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "dump"))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no fixtures in {}", dir.display());
    for fixture in names {
        let text = std::fs::read_to_string(&fixture).unwrap();
        let (head, expected) = text.split_once('\n').expect("fixture header line");
        let tape_path = repo_root().join(head.strip_prefix("file ").expect("file header"));
        let bytes = std::fs::read(&tape_path).unwrap_or_else(|e| panic!("{}: {e}", tape_path.display()));
        let tape = parse_tape(&bytes).expect("parses");
        let got = dump_tape(&tape);
        if got != expected {
            let g: Vec<&str> = got.lines().collect();
            let e: Vec<&str> = expected.lines().collect();
            for i in 0..g.len().max(e.len()) {
                let (a, b) = (g.get(i).unwrap_or(&""), e.get(i).unwrap_or(&""));
                assert_eq!(a, b, "{} line {}", fixture.display(), i + 1);
            }
            panic!("{}: dumps differ", fixture.display());
        }
    }
}

#[test]
fn detects_the_signature() {
    assert!(is_tzx(&tzx(&[0x20, 0xe8, 0x03])));
    assert!(!is_tzx(b"ZXTape!"));
    assert!(!is_tzx(&[19, 0, 0, 0]));
    assert!(parse_tzx(b"nope").is_err());
}

#[test]
fn preserves_unknown_blocks() {
    // Unknown ID 0x5b with a dword length of 3, then a pause block.
    let file = tzx(&[0x5b, 3, 0, 0, 0, 1, 2, 3, 0x20, 0xe8, 0x03]);
    let tape = parse_tzx(&file).unwrap();
    assert_eq!(tape.warnings, Vec::<String>::new());
    assert_eq!((tape.major, tape.minor), (1, 20));
    assert_eq!(tape.blocks.len(), 2);
    assert_eq!(tape.blocks[0].id(), 0x5b);
    assert!(tape.blocks[0].body.is_unknown());
    assert_eq!(tape.blocks[0].body, Body::Unknown { id: 0x5b, raw: vec![3, 0, 0, 0, 1, 2, 3] });
    assert_eq!(tape.blocks[1].body, Body::Pause { pause: 1000 });
}

#[test]
fn keeps_deprecated_blocks_raw() {
    let tape = parse_tzx(&tzx(&[0x34, 1, 2, 3, 4, 5, 6, 7, 8])).unwrap();
    assert_eq!(tape.blocks[0].body, Body::Unknown { id: 0x34, raw: vec![1, 2, 3, 4, 5, 6, 7, 8] });
    // 0x40 carries a byte then a 24-bit length; all four length bytes stay.
    let tape = parse_tzx(&tzx(&[0x40, 0, 2, 0, 0, 0xaa, 0xbb])).unwrap();
    assert_eq!(tape.blocks[0].body, Body::Unknown { id: 0x40, raw: vec![0, 2, 0, 0, 0xaa, 0xbb] });
}

#[test]
fn reports_a_truncated_block_and_keeps_the_rest() {
    // A standard block that claims 10 data bytes but has 2.
    let tape = parse_tzx(&tzx(&[0x10, 0xe8, 0x03, 10, 0, 1, 2])).unwrap();
    assert_eq!(
        tape.warnings,
        vec!["Block 1 (ID 10) at offset 10: Unexpected end of file at offset 15".to_string()]
    );
    assert_eq!(tape.blocks.len(), 1);
    assert_eq!(tape.blocks[0].body, Body::Unknown { id: 0x10, raw: vec![0xe8, 0x03, 10, 0, 1, 2] });
}

#[test]
fn reports_a_csw_block_that_cannot_hold_its_own_header() {
    let tape = parse_tzx(&tzx(&[0x18, 4, 0, 0, 0, 1, 2, 3, 4])).unwrap();
    assert_eq!(
        tape.warnings,
        vec![
            "Block 1 (ID 18) at offset 10: CSW block length 4 is shorter than its 10-byte header".to_string()
        ]
    );
    assert_eq!(tape.blocks.len(), 1);
    assert_eq!(tape.blocks[0].body, Body::Unknown { id: 0x18, raw: vec![4, 0, 0, 0, 1, 2, 3, 4] });
}

#[test]
fn parses_a_csw_block() {
    let tape = parse_tzx(&tzx(&[
        0x18, 12, 0, 0, 0, // length: 10 header bytes plus 2 of data
        0xe8, 0x03, // pause 1000
        0x44, 0xac, 0, // sample rate 44100
        1, // RLE
        3, 0, 0, 0, // pulse count 3
        0x0a, 0x14,
    ]))
    .unwrap();
    assert_eq!(tape.warnings, Vec::<String>::new());
    assert_eq!(
        tape.blocks[0].body,
        Body::Csw { pause: 1000, sample_rate: 44100, compression: 1, pulse_count: 3, data: vec![0x0a, 0x14] }
    );
}

#[test]
fn parses_every_block_type() {
    let tape = parse_tzx(&tzx(&[
        0x12, 0x78, 0x08, 0x7f, 0x1f, // pure tone 2168 x 8063
        0x13, 2, 0x9b, 0x02, 0xdf, 0x02, // pulse sequence 667, 735
        0x15, 0x4f, 0, 0, 0, 5, 2, 0, 0, 0xa5, 0x5a, // direct recording
        0x21, 5, b'G', b'r', b'o', b'u', b'p', // group start
        0x22, // group end
        0x23, 0xfe, 0xff, // jump -2
        0x24, 3, 0, 0x25, // loop start 3, loop end
        0x26, 2, 0, 1, 0, 0xff, 0xff, // call 1, -1
        0x27, // return
        0x28, 8, 0, 1, 2, 0, 4, b'L', b'o', b'a', b'd', // select
        0x2a, 0, 0, 0, 0, // stop if 48k
        0x2b, 1, 0, 0, 0, 1, // signal level high
        0x30, 2, b'h', b'i', // text
        0x31, 5, 2, b'h', b'i', // message
        0x32, 6, 0, 1, 0, 3, b'a', b'b', b'c', // archive info
        0x33, 1, 0, 1, 0, // hardware
        0x35, b'P', b'O', b'K', b'E', b's', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ',
        2, 0, 0, 0, 7, 8, // custom info
        0x5a, b'X', b'T', b'a', b'p', b'e', b'!', 0x1a, 1, 20, // glue
    ]))
    .unwrap();
    assert_eq!(tape.warnings, Vec::<String>::new());
    let bodies: Vec<&Body> = tape.blocks.iter().map(|b| &b.body).collect();
    assert_eq!(bodies[0], &Body::PureTone { pulse_len: 2168, count: 8063 });
    assert_eq!(bodies[1], &Body::PulseSeq { pulses: vec![667, 735] });
    assert_eq!(bodies[2], &Body::Direct { tstates: 79, pause: 0, used_bits: 5, data: vec![0xa5, 0x5a] });
    assert_eq!(bodies[3], &Body::GroupStart { name: "Group".to_string() });
    assert_eq!(bodies[4], &Body::GroupEnd);
    assert_eq!(bodies[5], &Body::Jump { offset: -2 });
    assert_eq!(bodies[6], &Body::LoopStart { count: 3 });
    assert_eq!(bodies[7], &Body::LoopEnd);
    assert_eq!(bodies[8], &Body::Call { offsets: vec![1, -1] });
    assert_eq!(bodies[9], &Body::Return);
    match bodies[10] {
        Body::Select { entries } => {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].offset, 2);
            assert_eq!(entries[0].text, "Load");
        }
        b => panic!("expected a select block, got {b:?}"),
    }
    assert_eq!(bodies[11], &Body::Stop48);
    assert_eq!(bodies[12], &Body::SignalLevel { level: 1 });
    assert_eq!(bodies[13], &Body::Text { text: "hi".to_string() });
    assert_eq!(bodies[14], &Body::Message { time: 5, text: "hi".to_string() });
    match bodies[15] {
        Body::Archive { entries } => {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].kind, 0);
            assert_eq!(entries[0].text, "abc");
        }
        b => panic!("expected an archive block, got {b:?}"),
    }
    match bodies[16] {
        Body::Hardware { entries } => {
            assert_eq!(entries.len(), 1);
            assert_eq!((entries[0].kind, entries[0].id, entries[0].info), (0, 1, 0));
        }
        b => panic!("expected a hardware block, got {b:?}"),
    }
    assert_eq!(bodies[17], &Body::Custom { ident: "POKEs           ".to_string(), data: vec![7, 8] });
    assert_eq!(bodies[18], &Body::Glue { raw: b"XTape!\x1a\x01\x14".to_vec() });
    assert_eq!(bodies.len(), 19);
}

#[test]
fn parses_a_generalized_block() {
    // One pilot symbol of two pulses, two data symbols, 8 data symbols packed
    // into one byte (1 bit each).
    let mut body: Vec<u8> = vec![0x19];
    let payload: Vec<u8> = vec![
        0xe8, 0x03, // pause 1000
        1, 0, 0, 0, // totp 1
        2, // npp
        1, // asp
        8, 0, 0, 0, // totd 8
        2, // npd
        2, // asd
        0, 0x78, 0x08, 0, 0, // pilot symbol: flags 0, 2168 then a trimmed zero
        0, 0x7f, 0x1f, // pilot stream: symbol 0 x 8063
        0, 0x57, 0x03, 0x57, 0x03, // data symbol 0: 855, 855
        0, 0xae, 0x06, 0xae, 0x06, // data symbol 1: 1710, 1710
        0xa5, // the 8 data bits
    ];
    body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    body.extend_from_slice(&payload);
    let tape = parse_tzx(&tzx(&body)).unwrap();
    assert_eq!(tape.warnings, Vec::<String>::new());
    match &tape.blocks[0].body {
        Body::Generalized {
            pause,
            totp,
            npp,
            pilot_symbols,
            pilot_stream,
            totd,
            npd,
            data_symbols,
            data,
        } => {
            assert_eq!((*pause, *totp, *npp, *totd, *npd), (1000, 1, 2, 8, 2));
            assert_eq!(pilot_symbols.len(), 1);
            assert_eq!(pilot_symbols[0].pulses, vec![2168]); // trailing zero trimmed
            assert_eq!(pilot_stream.len(), 1);
            assert_eq!((pilot_stream[0].symbol, pilot_stream[0].reps), (0, 8063));
            assert_eq!(data_symbols.len(), 2);
            assert_eq!(data_symbols[1].pulses, vec![1710, 1710]);
            assert_eq!(data, &vec![0xa5]);
        }
        b => panic!("expected a generalized block, got {b:?}"),
    }
}

#[test]
fn counts_bits_per_symbol() {
    assert_eq!(bits_per_symbol(0), 1);
    assert_eq!(bits_per_symbol(1), 1);
    assert_eq!(bits_per_symbol(2), 1);
    assert_eq!(bits_per_symbol(3), 2);
    assert_eq!(bits_per_symbol(4), 2);
    assert_eq!(bits_per_symbol(5), 3);
    assert_eq!(bits_per_symbol(256), 8);
}

#[test]
fn parses_tap_files() {
    // A 19-byte header block and a 3-byte data block, as in test/tzx.test.ts.
    let mut tap: Vec<u8> = vec![19, 0];
    tap.extend_from_slice(&[0; 19]);
    tap.extend_from_slice(&[3, 0, 0xff, 1, 0xfe]);
    let tape = parse_tap(&tap);
    assert_eq!(tape.warnings, Vec::<String>::new());
    assert_eq!((tape.major, tape.minor), (1, 20));
    assert_eq!(tape.blocks.len(), 2);
    assert_eq!(tape.blocks[0].body, Body::Standard { pause: 1000, data: vec![0; 19] });
    assert_eq!(tape.blocks[1].body, Body::Standard { pause: 1000, data: vec![0xff, 1, 0xfe] });
    // Auto-detection sends a file without the signature here.
    assert_eq!(parse_tape(&tap).unwrap().blocks.len(), 2);
}

#[test]
fn warns_about_a_broken_tap_file() {
    let tape = parse_tap(&[4, 0, 1, 2]);
    assert_eq!(tape.warnings, vec!["Truncated TAP block 1: declared 4 bytes, 2 available".to_string()]);
    assert_eq!(tape.blocks[0].body, Body::Standard { pause: 1000, data: vec![1, 2] });

    let tape = parse_tap(&[2, 0, 1, 2, 9]);
    assert_eq!(tape.warnings, vec!["Trailing byte ignored at end of TAP file".to_string()]);
    assert_eq!(tape.blocks.len(), 1);
}

#[test]
fn gives_every_block_a_fresh_uid() {
    let tape = parse_tzx(&tzx(&[0x20, 0xe8, 0x03, 0x20, 0xe8, 0x03])).unwrap();
    assert_ne!(tape.blocks[0].uid, tape.blocks[1].uid);
    // Blocks compare by content, not identity.
    assert_eq!(tape.blocks[0], tape.blocks[1]);
    assert_ne!(tape.blocks[0].clone_fresh().uid, tape.blocks[0].uid);
}
