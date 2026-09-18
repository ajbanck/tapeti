//! Tests for the third group of stage 2: converting block types, comparing
//! blocks and tapes, the bit operations and the POKEs block.

use tapeti_core::bits::{
    add_bits, drop_bits, flip_bytes, join_bits, shift_left_bits, shift_right_bits, total_bits, BitData,
};
use tapeti_core::compare::{
    blocks_equal, compare_tapes, find_matches, BlockCompareMode, CompareResult, TapeCompareMode,
};
use tapeti_core::convert::convert_block;
use tapeti_core::pokes::{
    decode_pokes, encode_pokes, pokes_to_text, text_to_pokes, Poke, PokesInfo, Trainer,
};
use tapeti_core::types::{create_body, Block, Body, RomTimings, SymDef};

fn b(body: Body) -> Block {
    Block::new(body)
}

fn std_block(data: Vec<u8>) -> Block {
    b(Body::Standard { pause: 1000, data })
}

#[test]
fn converts_between_block_types() {
    // Standard to turbo: pause and data carry over, a header flag picks the long pilot.
    let std = Block { uid: 7, body: Body::Standard { pause: 1234, data: vec![0, 1, 2] } };
    let turbo = convert_block(&std, 0x11);
    assert_eq!(turbo.uid, 7);
    match &turbo.body {
        Body::Turbo { pause, data, pilot_len, .. } => {
            assert_eq!(*pause, 1234);
            assert_eq!(data, &vec![0, 1, 2]);
            assert_eq!(*pilot_len, RomTimings::PILOT_HEADER);
        }
        other => panic!("expected a turbo block, got {other:?}"),
    }
    // A data block (flag 0xff) gets the shorter pilot.
    let data_block = std_block(vec![0xff, 1]);
    match convert_block(&data_block, 0x11).body {
        Body::Turbo { pilot_len, .. } => assert_eq!(pilot_len, RomTimings::PILOT_DATA),
        other => panic!("expected a turbo block, got {other:?}"),
    }

    // Turbo to generalized: its timings become the symbol definitions.
    let turbo = b(Body::Turbo {
        pilot: 2000,
        sync1: 600,
        sync2: 700,
        zero: 800,
        one: 1600,
        pilot_len: 3000,
        used_bits: 8,
        pause: 0,
        data: vec![0; 4],
    });
    match convert_block(&turbo, 0x19).body {
        Body::Generalized { totd, pilot_symbols, pilot_stream, data_symbols, .. } => {
            assert_eq!(totd, 32);
            assert_eq!(pilot_symbols[0].pulses, vec![2000]);
            assert_eq!(pilot_symbols[1].pulses, vec![600, 700]);
            assert_eq!(pilot_stream[0].reps, 3000);
            assert_eq!(
                data_symbols,
                vec![
                    SymDef { flags: 0, pulses: vec![800, 800] },
                    SymDef { flags: 0, pulses: vec![1600, 1600] },
                ]
            );
        }
        other => panic!("expected a generalized block, got {other:?}"),
    }

    // Text carries between group names and text blocks, both ways.
    let group = b(Body::GroupStart { name: "Level 2".into() });
    assert_eq!(convert_block(&group, 0x30).body, Body::Text { text: "Level 2".into() });
    let text = b(Body::Text { text: "Hello".into() });
    assert_eq!(convert_block(&text, 0x21).body, Body::GroupStart { name: "Hello".into() });

    // To a pause block: the pause survives, the data does not.
    let std = b(Body::Standard { pause: 500, data: vec![0; 9] });
    assert_eq!(convert_block(&std, 0x20).body, Body::Pause { pause: 500 });

    // Unknown blocks and same-type conversions come back unchanged.
    let unknown = b(Body::Unknown { id: 0x5b, raw: vec![1, 2] });
    assert_eq!(convert_block(&unknown, 0x10).body, unknown.body);
    assert_eq!(convert_block(&std, 0x10).body, std.body);
    // Every target type is a real block of that type.
    for id in tapeti_core::types::CREATABLE_IDS {
        let out = convert_block(&std, id);
        assert_eq!(out.id(), id, "converting to {id:02x}");
        assert_eq!(out.uid, std.uid);
        assert_eq!(std::mem::discriminant(&out.body), std::mem::discriminant(&create_body(id)));
    }
}

#[test]
fn compares_blocks_by_mode() {
    let a = b(Body::Standard { pause: 1, data: vec![1, 2] });
    let c = b(Body::Standard { pause: 2, data: vec![1, 2] });
    assert!(blocks_equal(&a, &c, BlockCompareMode::Data));
    assert!(blocks_equal(&a, &c, BlockCompareMode::DataTimings));
    assert!(!blocks_equal(&a, &c, BlockCompareMode::DataTimingsPauses));

    // In "data" mode a standard and a turbo block with the same bytes match.
    let turbo = b(Body::Turbo {
        pilot: 0,
        sync1: 0,
        sync2: 0,
        zero: 0,
        one: 0,
        pilot_len: 0,
        used_bits: 8,
        pause: 0,
        data: vec![1, 2],
    });
    assert!(blocks_equal(&a, &turbo, BlockCompareMode::Data));
    assert!(!blocks_equal(&a, &turbo, BlockCompareMode::DataTimings));

    // Unknown blocks match only other unknown blocks with the same id and bytes.
    let u1 = b(Body::Unknown { id: 0x5b, raw: vec![1] });
    let u2 = b(Body::Unknown { id: 0x5b, raw: vec![1] });
    let u3 = b(Body::Unknown { id: 0x5c, raw: vec![1] });
    assert!(blocks_equal(&u1, &u2, BlockCompareMode::DataTimings));
    assert!(!blocks_equal(&u1, &u3, BlockCompareMode::DataTimings));
    assert!(!blocks_equal(&u1, &a, BlockCompareMode::DataTimings));
}

#[test]
fn compares_tapes_block_by_block() {
    let left = vec![b(Body::Text { text: "note".into() }), std_block(vec![1, 2])];
    let right = vec![std_block(vec![1, 2])];
    // Metadata is ignored, so these two tapes are the same.
    let (l, r, identical) =
        compare_tapes(&left, &right, BlockCompareMode::DataTimings, TapeCompareMode::IgnoreMetadata);
    assert_eq!(l, vec![CompareResult::Ignored, CompareResult::Same]);
    assert_eq!(r, vec![CompareResult::Same]);
    assert!(identical);

    // Counting everything, the text block has nothing to line up with.
    let (l, _r, identical) =
        compare_tapes(&left, &right, BlockCompareMode::DataTimings, TapeCompareMode::All);
    assert_eq!(l[0], CompareResult::Diff);
    assert!(!identical);

    // The needle is skipped where it sits, so a tape of twins finds the other one.
    let blocks = vec![std_block(vec![1]), std_block(vec![1]), std_block(vec![9])];
    assert_eq!(find_matches(&blocks[0], &blocks, BlockCompareMode::Data, Some(0)), vec![1]);
    assert_eq!(find_matches(&blocks[0], &blocks, BlockCompareMode::Data, None), vec![0, 1]);
}

#[test]
fn moves_bits_around() {
    let d = BitData { data: vec![0b1011_0011], used_bits: 8 };
    assert_eq!(total_bits(&d), 8);
    assert_eq!(drop_bits(&d, 3), BitData { data: vec![0b1011_0000], used_bits: 5 });
    assert_eq!(add_bits(&d, 2), BitData { data: vec![0b1011_0011, 0], used_bits: 2 });
    assert_eq!(shift_left_bits(&d, 2), BitData { data: vec![0b1100_1100], used_bits: 6 });
    assert_eq!(shift_right_bits(&d, 1), BitData { data: vec![0b0101_1001, 0b1000_0000], used_bits: 1 });
    assert_eq!(flip_bytes(&[0b1000_0001, 0x0f]), vec![0b1000_0001, 0xf0]);

    // A partial byte only contributes the bits it says it has.
    let partial = BitData { data: vec![0b1010_0000], used_bits: 3 };
    assert_eq!(total_bits(&partial), 3);
    assert_eq!(
        join_bits(&[partial.clone(), partial.clone()]),
        BitData { data: vec![0b1011_0100], used_bits: 6 }
    );
    // Nothing in, nothing out.
    assert_eq!(total_bits(&BitData { data: vec![], used_bits: 8 }), 0);
    assert_eq!(drop_bits(&d, 99), BitData { data: vec![], used_bits: 8 });
}

#[test]
fn reads_and_writes_pokes() {
    let info = PokesInfo {
        description: "Cheats\nfor the demo".into(),
        trainers: vec![Trainer {
            description: "Infinite lives".into(),
            pokes: vec![
                Poke { page: None, addr: 32768, value: Some(255), original: Some(12) },
                Poke { page: Some(3), addr: 65535, value: None, original: None },
            ],
        }],
    };
    let bytes = encode_pokes(&info);
    assert_eq!(decode_pokes(&bytes).unwrap(), info);
    // Line breaks live as CR inside the block.
    assert!(bytes.contains(&b'\r'));

    // A block that runs out of bytes says so.
    assert!(decode_pokes(&[3, 65, 66]).is_err());

    let text = pokes_to_text(&info, false);
    assert_eq!(text, "; Cheats\n; for the demo\n\n[Infinite lives]\nPOKE 32768,255/12\nPOKE 3:65535,?");
    assert_eq!(text_to_pokes(&text, false).unwrap(), info);
    // In hex the same POKEs read and write in hex.
    let hex = pokes_to_text(&info, true);
    assert!(hex.contains("POKE 8000,FF/C"));
    assert_eq!(text_to_pokes(&hex, true).unwrap(), info);

    // $ and 0x are hex whatever the switch says, # is decimal.
    let mixed = text_to_pokes("POKE $8000,#16/0x0f", false).unwrap();
    assert_eq!(
        mixed.trainers[0].pokes[0],
        Poke { page: None, addr: 32768, value: Some(16), original: Some(15) }
    );

    // Numbers wider than the block can hold stay as typed until they are written.
    let wide = text_to_pokes("POKE 32768,255", true).unwrap();
    assert_eq!(wide.trainers[0].pokes[0].addr, 0x32768);
    // The address sits after the two empty descriptions and the two counts,
    // truncated to 16 bits on the way out.
    assert_eq!(encode_pokes(&wide)[5..7], [0x68, 0x27]);

    for bad in ["nonsense", "POKE", "POKE 1", "POKE 1,", "POKE ,2", "POKE 1,2,3", "POKE zz,2"] {
        assert!(text_to_pokes(bad, false).is_err(), "{bad} should not parse");
    }
    assert_eq!(text_to_pokes("POKE 1,2\nnope", false).unwrap_err(), "Line 2: cannot parse \"nope\"");
}
