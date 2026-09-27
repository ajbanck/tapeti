// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The file checksums against their published test vectors (RFC 1321 for MD5,
//! FIPS 180 for SHA-1, the "123456789" check value for CRC-32), and against
//! Python's hashlib and zlib at the lengths where the padding crosses a block.

use tapeti_core::hash::{crc32, file_hashes, FileHashes};

fn hashes(bytes: &[u8]) -> (String, String, String) {
    let FileHashes { crc32, md5, sha1 } = file_hashes(bytes);
    (crc32, md5, sha1)
}

#[test]
fn the_published_vectors() {
    let cases: [(&[u8], &str, &str, &str); 4] = [
        (b"", "00000000", "d41d8cd98f00b204e9800998ecf8427e", "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
        (b"abc", "352441c2", "900150983cd24fb0d6963f7d28e17f72", "a9993e364706816aba3e25717850c26c9cd0d89d"),
        (
            b"The quick brown fox jumps over the lazy dog",
            "414fa339",
            "9e107d9d372bb6826bd81d3542a419d6",
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12",
        ),
        (
            b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
            "7ca94a72",
            "57edf4a22be3c955ac49da2e2107b67a",
            "50abf5706a150990a08b2c5ea40fa0e585554732",
        ),
    ];
    for (input, c, m, s) in cases {
        assert_eq!(hashes(input), (c.into(), m.into(), s.into()), "{:?}", String::from_utf8_lossy(input));
    }
    assert_eq!(crc32(b"123456789"), 0xcbf43926);
}

#[test]
fn a_million_bytes() {
    let (_, md5, sha1) = hashes(&vec![b'a'; 1_000_000]);
    assert_eq!(md5, "7707d6ae4e027c70eea2a935c2296f21");
    assert_eq!(sha1, "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
}

#[test]
fn every_length_where_the_padding_changes() {
    let cases = [
        (55, "fd4fdad4", "6912ee65fff2d9f9ce2508cddf8bcda0", "8ae2d46729cfe68ff927af5eec9c7d1b66d65ac2"),
        (56, "ebfc1395", "51fdd1acda72405dfdfa03fcb85896d7", "636e2ec698dac903498e648bd2f3af641d3c88cb"),
        (63, "dbdea683", "48a6295221902e8e0938f773a7185e72", "6d942da0c4392b123528f2905c713a3ce28364bd"),
        (64, "100ece8c", "b2d3f56bc197fd985d5965079b5e7148", "c6138d514ffa2135bfce0ed0b8fac65669917ec7"),
        (65, "40c06fd8", "8bd7053801c768420faf816fadba971c", "69bd728ad6e13cd76ff19751fde427b00e395746"),
        (119, "9353a1a6", "1c772251899a7ff007400b888d6b2042", "41c89d06001bab4ab78736b44efe7ce18ce6ae08"),
        (120, "23455e6e", "b7ba1efc6022e9ed272f00b8831e26e6", "d3dbd653bd8597b7475321b60a36891278e6a04a"),
        (1000, "721746a6", "a24f1e3ef66950e1327f210e3997ba2c", "c9c960a0b925474fab83942cc27d504fc24ac37b"),
    ];
    for (n, c, m, s) in cases {
        let bytes: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
        assert_eq!(hashes(&bytes), (c.into(), m.into(), s.into()), "{n} bytes");
    }
}
