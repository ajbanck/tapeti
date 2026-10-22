// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The proof ladder: what each rung takes and refuses, and the ROM's own test beside it.

use tapeti_core::types::{Block, Body};
use tapeti_decode::proof::{answering, asks, at, header_shaped, of, xor_ok, Proof};

fn checksummed(mut data: Vec<u8>) -> Vec<u8> {
    data.push(data.iter().fold(0, |x, &d| x ^ d));
    data
}

/// A header of type `kind` named `name`, asking for `len` bytes of data.
fn header(kind: u8, name: &[u8; 10], len: u16) -> Vec<u8> {
    let mut h = vec![0x00, kind];
    h.extend(name);
    h.extend(len.to_le_bytes());
    h.extend([0x00, 0x80, 0x00, 0x80]);
    checksummed(h)
}

fn standard(data: Vec<u8>) -> Block {
    Block::new(Body::Standard { pause: 1000, data })
}

#[test]
fn the_rungs_from_nothing_up() {
    // Less than a flag and a checksum, or a checksum that fails.
    assert_eq!(of(&[]), Proof::None);
    assert_eq!(of(&[0xFF]), Proof::None);
    assert_eq!(of(&[0xFF, 0x01, 0x02]), Proof::None);
    // Holds, over what a misread leaves as readily as a tape.
    assert_eq!(of(&[0xFF, 0xFF]), Proof::Trivial, "two bytes");
    assert_eq!(of(&[0xFF; 6]), Proof::Trivial, "one value over and over");
    assert_eq!(of(&[0x00; 19]), Proof::Trivial, "nineteen zeros");
    // One chance in 256.
    assert_eq!(of(&checksummed(vec![0x07, 0x01, 0x02])), Proof::Checksum);
    assert_eq!(of(&checksummed(vec![0x07; 63])), Proof::Trivial, "a run, however long");
    assert_eq!(of(&checksummed((0..63u8).map(|k| k.wrapping_mul(7)).collect())), Proof::Checksum);
    // Past the chance junk: the ROM's data flag at three bytes, another flag at 64.
    assert_eq!(of(&checksummed(vec![0xFF, 0x01, 0x02])), Proof::Verified);
    assert_eq!(of(&checksummed((0..63u8).map(|k| k | 0x04).collect())), Proof::Verified);
    // Never under the header flag, where a long block that checks out is a misread.
    assert_eq!(of(&checksummed((0..63u8).map(|k| k & !0x01).collect())), Proof::Checksum);
}

#[test]
fn a_header_is_one_in_every_field() {
    let h = header(3, b"PROGRAM   ", 100);
    assert_eq!(of(&h), Proof::Header);
    assert_eq!(asks(&h), Some(102));
    // The copyright sign is the last printable character.
    assert_eq!(of(&header(0, b"\x7f 1986    ", 1)), Proof::Header);
    // A type the ROM looks past asks for nothing and proves nothing.
    assert_eq!(of(&header(4, b"PROGRAM   ", 100)), Proof::Trivial);
    assert_eq!(asks(&header(4, b"PROGRAM   ", 100)), None);
    // A name the ROM does not check: it still asks, but it is trusted no further than any 19
    // bytes under flag 0x00 that check out.
    let odd = header(3, b"\x90\x91\x92\x93      ", 100);
    assert_eq!(asks(&odd), Some(102));
    assert_eq!(of(&odd), Proof::Trivial);
    // A header read one bit out: its spaces become 0x40, its letters drag their neighbours' top
    // bits in, and the checksum goes with the shift and still holds.
    let bits: Vec<bool> = h.iter().flat_map(|&b| (0..8).rev().map(move |k| b >> k & 1 == 1)).collect();
    let shifted: Vec<u8> = bits[1..]
        .chunks(8)
        .map(|byte| byte.iter().fold(0, |b, &one| b << 1 | u8::from(one)))
        .take(19)
        .collect();
    assert_ne!(of(&shifted), Proof::Header, "{}", xor_ok(&shifted));
    assert_eq!(header_shaped(&shifted).is_some(), shifted[0] == 0x00);
    // A bad checksum is nothing, printable name or not.
    let mut bad = h.clone();
    bad[18] ^= 0x01;
    assert_eq!(of(&bad), Proof::None);
    assert_eq!(asks(&bad), None);
    assert_eq!(header_shaped(&bad), Some(102), "still shaped like one");
}

#[test]
fn an_answer_is_as_long_as_the_header_asked() {
    let data = checksummed([vec![0xFF], (0..100u8).map(|k| k.wrapping_mul(3)).collect()].concat());
    assert_eq!(answering(&data, 102), Proof::Answer);
    assert_eq!(answering(&data, 103), Proof::Verified, "the wrong length proves what it does alone");
    // Nor under a flag the ROM does not ask for behind a header: a reading a bit late, whose
    // flag 0xFF shifted is 0xFE, can check out at the length asked.
    let mut late = data.clone();
    late[0] = 0xFE;
    late[101] ^= 0x01;
    assert_eq!(answering(&late, 102), Proof::Verified);
    // Not over a run, which checks out at every even length.
    assert_eq!(answering(&[0x00; 102], 102), Proof::Trivial);
    // Nor with a bad checksum.
    let mut bad = data.clone();
    bad[5] ^= 0x01;
    assert_eq!(answering(&bad, 102), Proof::None);
    // In a tape, the header in front is the nearest block of data; a pause between does not
    // count, and a header a type the ROM looks past asks for nothing.
    let tape = vec![
        standard(header(3, b"CODE      ", 100)),
        Block::new(Body::Pause { pause: 500 }),
        standard(data.clone()),
        standard(header(4, b"NOT ASKED ", 100)),
        standard(data.clone()),
        standard(bad.clone()),
    ];
    let ranks: Vec<Proof> = (0..tape.len()).map(|i| at(&tape, i)).collect();
    assert_eq!(
        ranks,
        [Proof::Header, Proof::None, Proof::Answer, Proof::Trivial, Proof::Verified, Proof::None]
    );
    // A header whose name is no text (colour codes, a token) proves no more than any 19 bytes
    // that check out, until the block behind it answers it: then the pair proves each other.
    let coded = header(0, b"\x10\x02MINER3  ", 100);
    assert_eq!(of(&coded), Proof::Trivial);
    let tape = vec![standard(coded.clone()), Block::new(Body::Pause { pause: 500 }), standard(data.clone())];
    assert_eq!(at(&tape, 0), Proof::Answer);
    assert_eq!(at(&[standard(coded.clone()), standard(bad.clone())], 0), Proof::Trivial);
}

#[test]
fn the_ladder_is_ordered() {
    let rungs = [Proof::None, Proof::Trivial, Proof::Checksum, Proof::Verified, Proof::Answer, Proof::Header];
    assert!(rungs.windows(2).all(|w| w[0] < w[1]));
}
