// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The three checksums a tape file is known by: CRC32, MD5 and SHA-1.
//!
//! Taken over the bytes of the file as read, never over what saving would write:
//! the writer rebuilds the file and may raise its version, and a hash of that
//! would match nothing. Written out here because the core has no
//! dependencies; `core/tests/hash.rs` holds each to its published test vectors.

/// A file's three checksums, in lowercase hex as a DAT file writes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHashes {
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
}

pub fn file_hashes(bytes: &[u8]) -> FileHashes {
    FileHashes { crc32: format!("{:08x}", crc32(bytes)), md5: hex(&md5(bytes)), sha1: hex(&sha1(bytes)) }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// CRC-32 as zip and PNG have it: reflected, polynomial 0xEDB88320.
pub fn crc32(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    };
    !bytes.iter().fold(!0u32, |c, &b| TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8))
}

/// The message padded to whole 64-byte blocks: a 1 bit, zeros, then the length
/// in bits, little-endian for MD5 and big-endian for SHA-1.
fn padded(bytes: &[u8], big_endian: bool) -> Vec<u8> {
    let bits = (bytes.len() as u64).wrapping_mul(8);
    let mut m = bytes.to_vec();
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&if big_endian { bits.to_be_bytes() } else { bits.to_le_bytes() });
    m
}

/// The `i`th four bytes of a block.
fn word(chunk: &[u8], i: usize) -> [u8; 4] {
    [chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]
}

/// MD5, RFC 1321.
pub fn md5(bytes: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15,
        21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
        0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
        0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
        0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
        0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];
    let mut h: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    // `padded` is whole 64-byte blocks, so every chunk is a full one.
    for chunk in padded(bytes, false).chunks(64) {
        let m: [u32; 16] = std::array::from_fn(|i| u32::from_le_bytes(word(chunk, i)));
        let [mut a, mut b, mut c, mut d] = h;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let rotated = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]).rotate_left(S[i]);
            (a, d, c) = (d, c, b);
            b = b.wrapping_add(rotated);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d]) {
            *x = x.wrapping_add(y);
        }
    }
    std::array::from_fn(|i| h[i / 4].to_le_bytes()[i % 4])
}

/// SHA-1, FIPS 180-4.
pub fn sha1(bytes: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
    for chunk in padded(bytes, true).chunks(64) {
        let mut w = [0u32; 80];
        for (i, x) in w.iter_mut().take(16).enumerate() {
            *x = u32::from_be_bytes(word(chunk, i));
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i / 20 {
                0 => ((b & c) | (!b & d), 0x5a827999),
                1 => (b ^ c ^ d, 0x6ed9eba1),
                2 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    std::array::from_fn(|i| h[i / 4].to_be_bytes()[i % 4])
}
