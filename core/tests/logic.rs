//! Tests for the modules stage 2 moved over: descriptions, content detection,
//! consistency and program structure. The TypeScript versions of these live in
//! `test/tzx.test.ts` and `test/programs.test.ts`; `test/core.test.ts` compares
//! the two implementations directly, so what is here is what the core owes on
//! its own, plus the two predicates the TypeScript still keeps a copy of.

use tapeti_core::consistency::{check_consistency, Severity};
use tapeti_core::content::{basic_score, block_body, detect_content, ContentKind, Source};
use tapeti_core::describe::{block_length, checksum, describe_block, encode_header, is_metadata, HeaderInfo};
use tapeti_core::programs::{detect_programs, group_ranges, tape_title, ProgramSource};
use tapeti_core::types::{ArchiveEntry, Block, Body, SelectEntry};

fn b(body: Body) -> Block {
    Block::new(body)
}

/// A standard block holding a ROM header, flag and checksum included.
fn header_block(kind: u8, name: &str, length: u16, param1: u16, param2: u16) -> Block {
    let h = HeaderInfo { kind, type_name: String::new(), name: name.to_string(), length, param1, param2 };
    let mut data = vec![0x00];
    data.extend_from_slice(&encode_header(&h)[1..18]);
    data.push(checksum(&data));
    b(Body::Standard { pause: 1000, data })
}

fn data_block(len: usize, fill: u8) -> Block {
    let mut data = vec![0xff];
    data.extend(std::iter::repeat_n(fill, len));
    data.push(0);
    b(Body::Standard { pause: 1000, data })
}

#[test]
fn describes_headers_and_blocks() {
    // param1 is the autostart line; 32768 and up means there is none.
    let prog = header_block(0, "demo      ", 131, 32768, 20);
    assert_eq!(describe_block(&prog, false), "Std speed Prog: demo; L: 131, P: 20, S: none");
    assert_eq!(describe_block(&prog, true), "Std speed Prog: demo; L: 83, P: 14, S: none");
    let code = header_block(3, "demo.scr  ", 6912, 16384, 32768);
    assert_eq!(describe_block(&code, false), "Std speed Bytes: demo.scr; S: 16384, L: 6912");
    let nums = header_block(1, "nums      ", 40, 0x4100, 0);
    assert_eq!(describe_block(&nums, false), "Std speed Num: nums; L: 40, var a$");
    let chars = header_block(2, "chars     ", 40, 0x0100, 0);
    assert_eq!(describe_block(&chars, false), "Std speed Char: chars; L: 40, var a");
    // An autostart line number shows instead of "none".
    let auto = header_block(0, "auto      ", 10, 20, 5);
    assert_eq!(describe_block(&auto, false), "Std speed Prog: auto; L: 10, P: 5, S: 20");

    assert_eq!(describe_block(&b(Body::Pause { pause: 0 }), false), "Stop the tape");
    assert_eq!(describe_block(&b(Body::Pause { pause: 50 }), false), "Pause 50 ms");
    assert_eq!(describe_block(&b(Body::Jump { offset: -3 }), false), "Jump -3");
    assert_eq!(describe_block(&b(Body::Jump { offset: 3 }), false), "Jump +3");
    assert_eq!(describe_block(&b(Body::Text { text: "Two\r\nlines".into() }), false), "Text: Two");
    assert_eq!(
        describe_block(&b(Body::Custom { ident: "  POKEs  ".into(), data: vec![] }), false),
        "Custom info POKEs"
    );
    assert_eq!(
        describe_block(&b(Body::Unknown { id: 0x34, raw: vec![] }), false),
        "Emulation info (deprecated) (ID 34)"
    );
    assert_eq!(describe_block(&b(Body::Unknown { id: 0x5b, raw: vec![] }), false), "Unknown block (ID 5B)");
}

#[test]
fn measures_blocks_for_the_length_column() {
    assert_eq!(block_length(&b(Body::Standard { pause: 0, data: vec![1, 2, 3] })), 3);
    assert_eq!(block_length(&b(Body::PureTone { pulse_len: 2168, count: 1000 })), 1000);
    assert_eq!(block_length(&b(Body::PulseSeq { pulses: vec![1, 2, 3, 4] })), 4);
    assert_eq!(block_length(&b(Body::Unknown { id: 0x5b, raw: vec![1, 2] })), 2);
    // Everything else is measured by what it writes, minus the ID byte.
    assert_eq!(block_length(&b(Body::Pause { pause: 100 })), 2);
    assert_eq!(block_length(&b(Body::GroupEnd)), 0);
}

/// `isMetadata` is one of the two predicates the TypeScript keeps its own copy
/// of (`describe.ts`), so both sides assert this same table; `test/core.test.ts`
/// holds the other half.
#[test]
fn classifies_metadata_blocks() {
    let metadata = [0x21, 0x22, 0x30, 0x31, 0x32, 0x33, 0x35, 0x5a];
    for id in [
        0x10u8, 0x11, 0x12, 0x13, 0x14, 0x15, 0x18, 0x19, 0x20, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2a,
        0x2b, 0x21, 0x22, 0x30, 0x31, 0x32, 0x33, 0x35, 0x5a,
    ] {
        let block = match id {
            0x21 => b(Body::GroupStart { name: String::new() }),
            0x22 => b(Body::GroupEnd),
            0x30 => b(Body::Text { text: String::new() }),
            0x31 => b(Body::Message { time: 0, text: String::new() }),
            0x32 => b(Body::Archive { entries: vec![] }),
            0x33 => b(Body::Hardware { entries: vec![] }),
            0x35 => b(Body::Custom { ident: String::new(), data: vec![] }),
            0x5a => b(Body::Glue { raw: vec![] }),
            0x10 => b(Body::Standard { pause: 0, data: vec![] }),
            0x11 => b(Body::Turbo {
                pilot: 0,
                sync1: 0,
                sync2: 0,
                zero: 0,
                one: 0,
                pilot_len: 0,
                used_bits: 8,
                pause: 0,
                data: vec![],
            }),
            0x12 => b(Body::PureTone { pulse_len: 0, count: 0 }),
            0x13 => b(Body::PulseSeq { pulses: vec![] }),
            0x14 => b(Body::PureData { zero: 0, one: 0, used_bits: 8, pause: 0, data: vec![] }),
            0x15 => b(Body::Direct { tstates: 0, pause: 0, used_bits: 8, data: vec![] }),
            0x18 => b(Body::Csw { pause: 0, sample_rate: 0, compression: 1, pulse_count: 0, data: vec![] }),
            0x19 => b(Body::Generalized {
                pause: 0,
                totp: 0,
                npp: 0,
                pilot_symbols: vec![],
                pilot_stream: vec![],
                totd: 0,
                npd: 0,
                data_symbols: vec![],
                data: vec![],
            }),
            0x20 => b(Body::Pause { pause: 0 }),
            0x23 => b(Body::Jump { offset: 0 }),
            0x24 => b(Body::LoopStart { count: 0 }),
            0x25 => b(Body::LoopEnd),
            0x26 => b(Body::Call { offsets: vec![] }),
            0x27 => b(Body::Return),
            0x28 => b(Body::Select { entries: vec![] }),
            0x2a => b(Body::Stop48),
            _ => b(Body::SignalLevel { level: 0 }),
        };
        assert_eq!(is_metadata(&block), metadata.contains(&id), "block {id:02x}");
    }
    // Unknown blocks count as metadata, except the two deprecated C64 data ones.
    assert!(is_metadata(&b(Body::Unknown { id: 0x5b, raw: vec![] })));
    assert!(!is_metadata(&b(Body::Unknown { id: 0x16, raw: vec![] })));
    assert!(!is_metadata(&b(Body::Unknown { id: 0x17, raw: vec![] })));
}

/// The other predicate the TypeScript keeps: `blockBody` in `content.ts`.
#[test]
fn strips_flag_and_checksum_bytes() {
    let d = [0xff, 1, 2, 3, 0x55];
    assert_eq!(block_body(&d, true, true), &[1, 2, 3]);
    assert_eq!(block_body(&d, true, false), &[1, 2, 3, 0x55]);
    assert_eq!(block_body(&d, false, true), &[0xff, 1, 2, 3]);
    assert_eq!(block_body(&d, false, false), &d[..]);
    assert_eq!(block_body(&[], true, true), &[] as &[u8]);
    assert_eq!(block_body(&[7], true, true), &[] as &[u8]);
}

#[test]
fn detects_what_blocks_hold() {
    // A header block, then the BASIC program it announces.
    let blocks = vec![header_block(0, "demo      ", 40, 0, 20), data_block(40, 0)];
    assert_eq!(detect_content(&blocks, 0).kind, ContentKind::Header);
    let basic = detect_content(&blocks, 1);
    assert_eq!(basic.kind, ContentKind::Basic);
    assert_eq!(basic.base, 23755);
    assert_eq!(basic.prog_len, Some(20));
    assert_eq!(basic.source, Source::Header);

    // A screen, wherever its header says to load it.
    let blocks = vec![header_block(3, "scr       ", 6912, 40000, 0), data_block(6912, 0)];
    let screen = detect_content(&blocks, 1);
    assert_eq!(screen.kind, ContentKind::Screen);
    assert_eq!(screen.label, "SCREEN 40000");
    assert_eq!(screen.base, 16384);

    // Short and long bodies still use the header, and say so.
    let blocks = vec![header_block(3, "scr       ", 6912, 16384, 0), data_block(100, 0)];
    assert_eq!(detect_content(&blocks, 1).label, "SCREEN (short)");
    let blocks = vec![header_block(3, "code      ", 120, 32768, 0), data_block(130, 0)];
    let code = detect_content(&blocks, 1);
    assert_eq!(code.label, "CODE 32768 (long)");
    assert_eq!(code.base, 32768);
    assert_eq!(code.expected_length, Some(120));

    // Without a header: a 6912-byte body is a screen, BASIC lines are BASIC.
    assert_eq!(detect_content(&[data_block(6912, 1)], 0).label, "SCREEN?");
    let basic =
        b(Body::Standard { pause: 0, data: vec![0xff, 0x00, 0x0a, 0x05, 0x00, 1, 2, 3, 4, 0x0d, 0x00] });
    let found = detect_content(&[basic], 0);
    assert_eq!(found.kind, ContentKind::Basic);
    assert_eq!(found.source, Source::Heuristic);
    assert_eq!(detect_content(&[data_block(7, 0xc3)], 0).kind, ContentKind::Data);
    assert_eq!(detect_content(&[b(Body::Standard { pause: 0, data: vec![] })], 0).kind, ContentKind::Empty);
    // Not a data block, and out of range.
    assert_eq!(detect_content(&[b(Body::Pause { pause: 0 })], 0).kind, ContentKind::Data);
    assert_eq!(detect_content(&[], 0).source, Source::None);
}

#[test]
fn scores_basic_programs() {
    // A line number that runs backwards is not a program.
    assert_eq!(basic_score(&[0x27, 0x10, 5, 0, 1, 2, 3, 4, 5]), 0.0);
    // One well-formed line covers the whole stream.
    assert_eq!(basic_score(&[0x00, 0x0a, 0x02, 0x00, 1, 0x0d]), 1.0);
}

#[test]
fn finds_structural_problems() {
    let issues = check_consistency(&[], 1);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].block, -1);
    assert_eq!(issues[0].severity, Severity::Info);

    let crossing =
        vec![b(Body::GroupStart { name: String::new() }), b(Body::LoopStart { count: 2 }), b(Body::GroupEnd)];
    let issues = check_consistency(&crossing, 1);
    assert!(issues.iter().any(|i| i.message.contains("crosses")));
    assert!(issues.iter().any(|i| i.message.contains("never closed")));

    let jump = vec![b(Body::Jump { offset: 0 })];
    assert!(check_consistency(&jump, 1).iter().any(|i| i.severity == Severity::Error));

    // The block number in a message follows the base the UI numbers from.
    let outside = vec![b(Body::Jump { offset: 9 })];
    assert!(check_consistency(&outside, 1)[0].message.contains("target 10"));
    assert!(check_consistency(&outside, 0)[0].message.contains("target 9"));

    // A call that never returns is reported once, at the call.
    let call = vec![b(Body::Call { offsets: vec![1] }), b(Body::PureTone { pulse_len: 1, count: 1 })];
    assert!(check_consistency(&call, 1).iter().any(|i| i.message.contains("never returns")));

    // A jump that comes back to itself in the same state is an infinite loop.
    let spin = vec![b(Body::Jump { offset: 1 }), b(Body::Jump { offset: -1 })];
    assert!(check_consistency(&spin, 1).iter().any(|i| i.message.contains("Infinite loop")));
}

#[test]
fn finds_groups_and_programs() {
    let group = |name: &str| b(Body::GroupStart { name: name.to_string() });
    let blocks = vec![
        group("outer"),
        group("inner"),
        b(Body::GroupEnd),
        b(Body::LoopStart { count: 2 }),
        b(Body::LoopEnd),
        b(Body::GroupEnd),
    ];
    // Ranges close from the inside out, which is the order the list wants them.
    assert_eq!(group_ranges(&blocks), vec![(1, 2), (3, 4), (0, 5)]);

    let blocks = vec![
        header_block(0, "A         ", 10, 0, 10),
        data_block(10, 0),
        header_block(0, "B         ", 10, 0, 10),
        data_block(10, 0),
    ];
    let programs = detect_programs(&blocks);
    assert_eq!(programs.len(), 2);
    assert_eq!((programs[0].name.as_str(), programs[0].start, programs[0].end), ("A", 0, 1));
    assert_eq!((programs[1].name.as_str(), programs[1].start, programs[1].end), ("B", 2, 3));
    assert_eq!(programs[0].source, ProgramSource::Header);

    // A group holding a program header names the program.
    let blocks = vec![group("Game"), header_block(0, "A         ", 10, 0, 10), b(Body::GroupEnd)];
    let programs = detect_programs(&blocks);
    assert_eq!(programs.len(), 1);
    assert_eq!(programs[0].name, "Game");
    assert_eq!(programs[0].source, ProgramSource::Group);

    // A Select entry starts a program where it points. (A boundary at block 0
    // would not split anything: the first program always starts at the top.)
    let blocks = vec![
        header_block(0, "A         ", 10, 0, 10),
        data_block(4, 0),
        b(Body::Select { entries: vec![SelectEntry { offset: 2, text: "Side B".into() }] }),
        b(Body::PureTone { pulse_len: 1, count: 1 }),
        data_block(4, 0),
    ];
    let programs = detect_programs(&blocks);
    assert_eq!(programs.len(), 2);
    assert_eq!((programs[0].name.as_str(), programs[0].start, programs[0].end), ("A", 0, 3));
    assert_eq!((programs[1].name.as_str(), programs[1].start, programs[1].end), ("Side B", 4, 4));
    assert_eq!(programs[1].source, ProgramSource::Select);

    // Nothing to go on: one program, named after the archive info.
    let blocks = vec![
        b(Body::Archive { entries: vec![ArchiveEntry { kind: 0, text: "The Tape".into() }] }),
        data_block(4, 0),
    ];
    let programs = detect_programs(&blocks);
    assert_eq!(programs.len(), 1);
    assert_eq!(programs[0].name, "The Tape");
    assert_eq!(programs[0].source, ProgramSource::Tape);
    assert_eq!(tape_title(&blocks).as_deref(), Some("The Tape"));
    assert_eq!(tape_title(&[]), None);
    assert!(detect_programs(&[]).is_empty());
}
