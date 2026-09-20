//! Bit-stream helpers for the data window's Drop / Add / Shift operations, the
//! port of `src/tzx/bits.ts`.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitData {
    pub data: Vec<u8>,
    /// Used bits in the last byte (1-8); ignored when data is empty.
    pub used_bits: u8,
}

pub fn total_bits(d: &BitData) -> usize {
    if d.data.is_empty() {
        return 0;
    }
    (d.data.len() - 1) * 8 + d.used_bits.clamp(1, 8) as usize
}

pub fn get_bit(d: &[u8], i: usize) -> u8 {
    (d[i >> 3] >> (7 - (i & 7))) & 1
}

/// Build a byte array from a bit provider.
pub fn from_bits(n: usize, bit: impl Fn(usize) -> u8) -> BitData {
    let len = n.div_ceil(8);
    let mut out = vec![0u8; len];
    for i in 0..n {
        if bit(i) != 0 {
            out[i >> 3] |= 0x80 >> (i & 7);
        }
    }
    let used = if n == 0 { 8 } else { n - (len - 1) * 8 };
    BitData { data: out, used_bits: used as u8 }
}

pub fn drop_bits(d: &BitData, n: usize) -> BitData {
    let total = total_bits(d).saturating_sub(n);
    from_bits(total, |i| get_bit(&d.data, i))
}

pub fn add_bits(d: &BitData, n: usize) -> BitData {
    let before = total_bits(d);
    from_bits(before + n, |i| if i < before { get_bit(&d.data, i) } else { 0 })
}

pub fn shift_left_bits(d: &BitData, n: usize) -> BitData {
    let total = total_bits(d).saturating_sub(n);
    from_bits(total, |i| get_bit(&d.data, i + n))
}

pub fn shift_right_bits(d: &BitData, n: usize) -> BitData {
    let before = total_bits(d);
    from_bits(before + n, |i| if i < n { 0 } else { get_bit(&d.data, i - n) })
}

/// Concatenate several bit streams (used by "view selected as one").
pub fn join_bits(parts: &[BitData]) -> BitData {
    let lens: Vec<usize> = parts.iter().map(total_bits).collect();
    let total: usize = lens.iter().sum();
    from_bits(total, |mut i| {
        let mut k = 0;
        while k < parts.len() && i >= lens[k] {
            i -= lens[k];
            k += 1;
        }
        get_bit(&parts[k].data, i)
    })
}

/// Reverse the bits of every byte.
pub fn flip_bytes(d: &[u8]) -> Vec<u8> {
    d.iter().map(|b| b.reverse_bits()).collect()
}

/// The usual turbo loader encryption, SpeedLock's among them: what ends up in
/// memory is `(loaded byte XOR x) + y`.
///
/// ```text
/// LD   A,x
/// XOR  L            ; the byte just loaded
/// ADD  A,y
/// LD   (IX+0),A
/// ```
pub fn decrypt_bytes(d: &[u8], xor: u8, add: u8) -> Vec<u8> {
    d.iter().map(|b| (b ^ xor).wrapping_add(add)).collect()
}

/// The other way: the bytes a tape must hold for the loader to store these.
pub fn encrypt_bytes(d: &[u8], xor: u8, add: u8) -> Vec<u8> {
    d.iter().map(|b| b.wrapping_sub(add) ^ xor).collect()
}

/// The values the SpeedLock versions use unless a game changes them.
pub const CRYPT_PRESETS: [(&str, u8, u8); 2] = [("SpeedLock 2/3", 0x98, 0x0b), ("SpeedLock 4-7", 0xc1, 0x11)];
