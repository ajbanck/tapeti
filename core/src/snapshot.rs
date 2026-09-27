// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck
// Portions Copyright (C) 1997-2001 Martijn van der Heide (Taper, tpsnap.c), GPL-2.0-or-later

//! Turning a snapshot (.z80, .sna) into a tape that loads it: a BASIC loader
//! followed by the memory as compressed turbo blocks.
//!
//! The method and the Z80 code are TAPER's (`tpsnap.c`, Copyleft (C) 1997-2001
//! ThunderWare Research Center, written by Martijn van der Heide, GPL 2 or
//! later). A relocator in the BASIC program's variables area copies a 512 byte
//! loader to `BE00`. For each 16K page the loader pages it in, loads the block
//! backwards so it ends at the top of the page, and unpacks it forwards in place,
//! the packing being the .z80 file's own `ED ED count value` scheme. A last stage
//! moves itself and a stack frame into the bottom three screen lines, puts back
//! what the loader sat on, pops every register and jumps into the program. The
//! bottom three pixel lines and the bottom attribute row are lost, as is whatever
//! the snapshot had in its paged-in ROMs.
//!
//! Where this differs from TAPER, on purpose:
//! - the packer checks the unpacker's two blind spots, a lone `ED` at the end and output
//!   overtaking input, and a page that trips one goes out unpacked; TAPER copied the last
//!   four bytes raw, which the unpacker can misread
//! - an unpacked page may be shorter than 16K (the one under the loader is), so the loader
//!   works its length out from where it ends
//! - a 128K tape always carries the block that replaces the loader, because the 128K
//!   loader always waits for it
//! - pages of zeroes are sent rather than skipped: memory is not empty when the loader starts
//! - the relocator starts with `DI`, since an interrupt between its `CALL` and the `POP`
//!   that reads the return address back would overwrite it
//! - IFF1, not IFF2, decides between `EI` and `DI`; the .sna border comes from the header;
//!   v3 headers of 55 bytes and 128K .sna files are read
//! - the loader never resets port 7FFD before its last stage (every table entry already
//!   leaves the 48K ROM selected), which makes room in the 128K loader for the two
//!   changes above
//! - the BASIC lines carry their real lengths and no colour codes that stop a LIST, so the
//!   program reads like any other

use crate::describe::{checksum, encode_header, HeaderInfo, HEADER_TYPE_NAMES};
use crate::types::{Block, Body};

const PAGE: usize = 16384;
const SCREEN: usize = 6912;
/// The loader ends where the top 16K starts, whatever its length.
const LOADER_END: u16 = 0xc000;
const LOADER_LEN: usize = 512;
/// A Scorpion has sixteen pages to list, which takes a longer loader.
const LOADER_LEN_SCORPION: usize = 768;
/// Where the tape loading routine sits, up against the loader's end.
const TURBO_ORG: u16 = 0xbf55;
/// Where the last stages run from: the bottom three pixel lines of the screen,
/// one line each, with the stack frame at the end of the last.
const AY_FRAME_AT: u16 = 0x54e0;
const AY_STAGE_AT: u16 = 0x55e0;
const LAST_STAGE_AT: u16 = 0x56e0;
/// Flag byte of a page block.
pub const FLAG_PAGE: u8 = 0xaa;
/// Flag byte of the block that replaces the loader.
pub const FLAG_FINAL: u8 = 0x55;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotKind {
    Z80,
    Sna,
}

impl SnapshotKind {
    /// By extension: neither format has a signature.
    pub fn from_name(name: &str) -> Option<Self> {
        let ext = name.rsplit('.').next()?.to_ascii_lowercase();
        match ext.as_str() {
            "z80" => Some(SnapshotKind::Z80),
            "sna" => Some(SnapshotKind::Sna),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registers {
    pub af: u16,
    pub bc: u16,
    pub de: u16,
    pub hl: u16,
    pub af_alt: u16,
    pub bc_alt: u16,
    pub de_alt: u16,
    pub hl_alt: u16,
    pub ix: u16,
    pub iy: u16,
    pub sp: u16,
    pub pc: u16,
    pub i: u8,
    pub r: u8,
    pub iff1: bool,
    /// 0, 1 or 2
    pub im: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub regs: Registers,
    /// Pages through port 7FFD: the 128K, +2, +3, Pentagon and Scorpion.
    pub is_128k: bool,
    /// A Scorpion ZS 256: sixteen pages, the upper eight through bit 4 of port 1FFD.
    pub is_scorpion: bool,
    /// RAM pages by their 128K number. A 48K machine has 5 at 4000, 2 at 8000
    /// and 0 at C000, as a 128K one does after a reset.
    pub pages: [Option<Vec<u8>>; 16],
    pub border: u8,
    pub port_7ffd: u8,
    /// Only restored on a Scorpion.
    pub port_1ffd: u8,
    pub port_fffd: u8,
    pub ay: [u8; 16],
}

impl Snapshot {
    /// The display file and attributes, as the loading screen would be.
    pub fn screen(&self) -> &[u8] {
        self.pages[5].as_deref().map(|p| &p[..SCREEN]).unwrap_or(&[])
    }
}

pub fn parse_snapshot(bytes: &[u8], kind: SnapshotKind) -> Result<Snapshot, String> {
    match kind {
        SnapshotKind::Z80 => parse_z80(bytes),
        SnapshotKind::Sna => parse_sna(bytes),
    }
}

fn word(b: &[u8], at: usize) -> u16 {
    u16::from(b[at]) | u16::from(b[at + 1]) << 8
}

/// Undo the .z80 run-length packing into exactly `out_len` bytes.
pub fn unpack(input: &[u8], out_len: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(out_len);
    let mut i = 0;
    while i < input.len() && out.len() < out_len {
        if input[i] == 0xed && input.get(i + 1) == Some(&0xed) {
            let count = usize::from(*input.get(i + 2)?);
            let value = *input.get(i + 3)?;
            let count = count.min(out_len - out.len());
            out.resize(out.len() + count, value);
            i += 4;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    (out.len() == out_len).then_some(out)
}

/// The .z80 run-length packing: five or more of a byte, or two or more `ED`,
/// become `ED ED count value`; a byte straight after a lone `ED` never starts
/// a run, or the two would read as a marker.
pub fn pack(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut after_ed = false;
    let mut i = 0;
    while i < input.len() {
        let value = input[i];
        let run = input[i..].iter().take(255).take_while(|b| **b == value).count();
        if (value == 0xed && run >= 2) || (run >= 5 && !after_ed) {
            out.extend_from_slice(&[0xed, 0xed, run as u8, value]);
            i += run;
            after_ed = false;
        } else {
            out.push(value);
            after_ed = value == 0xed;
            i += 1;
        }
    }
    out
}

/// Can the loader unpack this where it lands? The packed bytes sit at the end
/// of the space they unpack into, so the output must never pass the input, and
/// a lone `ED` at the very end would make the unpacker look one byte past it.
fn unpacks_in_place(packed: &[u8], unpacked_len: usize) -> bool {
    if packed.len() >= unpacked_len {
        return false;
    }
    let mut read = unpacked_len - packed.len();
    let mut written = 0;
    let mut i = 0;
    while i < packed.len() {
        if written > read {
            return false;
        }
        if packed[i] == 0xed {
            match packed.get(i + 1) {
                None => return false,
                Some(0xed) => {
                    written += usize::from(packed[i + 2]);
                    read += 4;
                    i += 4;
                    continue;
                }
                Some(_) => {}
            }
        }
        written += 1;
        read += 1;
        i += 1;
    }
    written == unpacked_len
}

fn parse_z80(b: &[u8]) -> Result<Snapshot, String> {
    let bad = || "Not a valid .z80 snapshot".to_string();
    if b.len() < 30 {
        return Err(bad());
    }
    let flags = if b[12] == 0xff { 1 } else { b[12] };
    let mut snap = Snapshot {
        regs: Registers {
            af: u16::from(b[0]) << 8 | u16::from(b[1]),
            bc: word(b, 2),
            hl: word(b, 4),
            pc: word(b, 6),
            sp: word(b, 8),
            i: b[10],
            r: (b[11] & 0x7f) | (flags & 1) << 7,
            de: word(b, 13),
            bc_alt: word(b, 15),
            de_alt: word(b, 17),
            hl_alt: word(b, 19),
            af_alt: u16::from(b[21]) << 8 | u16::from(b[22]),
            iy: word(b, 23),
            ix: word(b, 25),
            iff1: b[27] != 0,
            im: b[29] & 3,
        },
        border: (flags >> 1) & 7,
        port_7ffd: 0x10,
        ..Snapshot::default()
    };
    if snap.regs.pc != 0 {
        // Version 1: one 48K image.
        let body = &b[30..];
        let ram =
            if flags & 0x20 != 0 { unpack(body, 3 * PAGE) } else { body.get(..3 * PAGE).map(<[u8]>::to_vec) };
        let ram = ram.ok_or_else(bad)?;
        set_48k_ram(&mut snap, &ram);
        return Ok(snap);
    }
    if b.len() < 32 {
        return Err(bad());
    }
    let extra = usize::from(word(b, 30));
    let mut pos = 32 + extra;
    if extra < 23 || b.len() < pos {
        return Err(bad());
    }
    snap.regs.pc = word(b, 32);
    let hardware = b[34];
    snap.is_128k = if extra == 23 { hardware >= 3 } else { hardware >= 4 };
    snap.is_scorpion = extra != 23 && hardware == 10;
    if snap.is_scorpion && extra >= 55 {
        snap.port_1ffd = b[86];
    }
    if snap.is_128k {
        snap.port_7ffd = b[35];
        snap.port_fffd = b[38];
        snap.ay.copy_from_slice(&b[39..55]);
    }
    while pos + 3 <= b.len() {
        let length = word(b, pos);
        let number = b[pos + 2];
        pos += 3;
        let stored = if length == 0xffff { PAGE } else { usize::from(length) };
        let data = b.get(pos..pos + stored).ok_or_else(bad)?;
        pos += stored;
        let page = match (snap.is_128k, number) {
            (true, 3..=10) => usize::from(number - 3),
            (true, 11..=18) if snap.is_scorpion => usize::from(number - 3),
            (false, 4) => 2,
            (false, 5) => 0,
            (false, 8) => 5,
            // ROMs and interface memory: nothing a tape can put back.
            _ => continue,
        };
        let ram = if length == 0xffff { Some(data.to_vec()) } else { unpack(data, PAGE) };
        snap.pages[page] = Some(ram.ok_or_else(bad)?);
    }
    if snap.pages[5].is_none() {
        return Err(bad());
    }
    Ok(snap)
}

fn set_48k_ram(snap: &mut Snapshot, ram: &[u8]) {
    snap.pages[5] = Some(ram[..PAGE].to_vec());
    snap.pages[2] = Some(ram[PAGE..2 * PAGE].to_vec());
    snap.pages[0] = Some(ram[2 * PAGE..3 * PAGE].to_vec());
}

fn parse_sna(b: &[u8]) -> Result<Snapshot, String> {
    const HEADER: usize = 27;
    let bad = || "Not a valid .sna snapshot".to_string();
    if b.len() < HEADER + 3 * PAGE {
        return Err(bad());
    }
    let mut snap = Snapshot {
        regs: Registers {
            i: b[0],
            hl_alt: word(b, 1),
            de_alt: word(b, 3),
            bc_alt: word(b, 5),
            af_alt: word(b, 7),
            hl: word(b, 9),
            de: word(b, 11),
            bc: word(b, 13),
            iy: word(b, 15),
            ix: word(b, 17),
            iff1: b[19] & 4 != 0,
            r: b[20],
            af: word(b, 21),
            sp: word(b, 23),
            im: b[25] & 3,
            pc: 0,
        },
        border: b[26] & 7,
        port_7ffd: 0x10,
        ..Snapshot::default()
    };
    let ram = &b[HEADER..HEADER + 3 * PAGE];
    if b.len() == HEADER + 3 * PAGE {
        // 48K: the program counter was pushed before the file was written.
        let sp = usize::from(snap.regs.sp);
        if !(0x4000..0xffff).contains(&sp) {
            return Err("This .sna snapshot has its stack in ROM, so where it resumes is unknown".to_string());
        }
        snap.regs.pc = word(ram, sp - 0x4000);
        snap.regs.sp = snap.regs.sp.wrapping_add(2);
        set_48k_ram(&mut snap, ram);
        return Ok(snap);
    }
    let tail = HEADER + 3 * PAGE;
    if b.len() < tail + 4 {
        return Err(bad());
    }
    snap.is_128k = true;
    snap.regs.pc = word(b, tail);
    snap.port_7ffd = b[tail + 2];
    let top = usize::from(snap.port_7ffd & 7);
    let rest: Vec<usize> = (0..8usize).filter(|p| *p != 5 && *p != 2 && *p != top).collect();
    if b.len() != tail + 4 + rest.len() * PAGE {
        return Err(bad());
    }
    snap.pages[5] = Some(ram[..PAGE].to_vec());
    snap.pages[2] = Some(ram[PAGE..2 * PAGE].to_vec());
    snap.pages[top] = Some(ram[2 * PAGE..].to_vec());
    for (n, page) in rest.into_iter().enumerate() {
        let at = tail + 4 + n * PAGE;
        snap.pages[page] = Some(b[at..at + PAGE].to_vec());
    }
    Ok(snap)
}

// ---- the tape --------------------------------------------------------------

/// What the loading routine needs to know about a speed: its two constants, and
/// the block timings that go with them. TAPER's table; the derivation is in
/// `tpsnap.c`.
struct Speed {
    bps: u16,
    compare: u8,
    delay: u8,
    pilot: u16,
    sync: u16,
    zero: u16,
}

const SPEEDS: [Speed; 4] = [
    Speed { bps: 1500, compare: 0x80 + 41, delay: 20, pilot: 2168, sync: 667, zero: 855 },
    Speed { bps: 2250, compare: 0x80 + 24, delay: 11, pilot: 2000, sync: 600, zero: 518 },
    Speed { bps: 3000, compare: 0x80 + 18, delay: 7, pilot: 1900, sync: 550, zero: 389 },
    Speed { bps: 6000, compare: 0x80 + 7, delay: 3, pilot: 1700, sync: 450, zero: 195 },
];

/// Bits per second of each speed, in the order [`LoaderOptions::speed`] counts them.
pub const SPEED_BPS: [u16; 4] = [SPEEDS[0].bps, SPEEDS[1].bps, SPEEDS[2].bps, SPEEDS[3].bps];
pub const DEFAULT_SPEED: u8 = 2;

#[derive(Clone, Debug)]
pub struct LoaderOptions<'a> {
    /// The snapshot's file name; its stem names the BASIC program.
    pub name: &'a str,
    /// Index into [`SPEED_BPS`]. The first is ROM timing, in standard blocks.
    pub speed: u8,
    /// Border once loaded, and the colour of the loading stripes.
    pub border: u8,
    /// Pack the screen's page even when the packed bytes would show on screen
    /// while they load.
    pub compress_all: bool,
    /// A 6912 byte loading screen to show instead of the snapshot's own.
    pub screen: Option<&'a [u8]>,
}

struct Asm(Vec<u8>, u16);

impl Asm {
    fn at(org: u16) -> Self {
        Asm(Vec::new(), org)
    }
    /// A relative jump's displacement, from the byte after it to `target`.
    fn jr_to(&mut self, operand: usize, target: usize) {
        self.0[operand] = (target as isize - (operand as isize + 1)) as i8 as u8;
    }
    fn patch_w(&mut self, operand: usize, v: u16) {
        self.0[operand..operand + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn b(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.extend_from_slice(bytes);
        self
    }
    fn w(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn here(&self) -> u16 {
        self.1 + self.0.len() as u16
    }
}

/// The ten characters of the tape header: the file's stem, in ASCII.
fn program_name(file: &str) -> String {
    let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
    let name: String = stem.chars().filter(|c| (' '..='~').contains(c)).take(10).collect();
    if name.trim().is_empty() {
        "snapshot".to_string()
    } else {
        name
    }
}

/// The BASIC lines; the machine code follows them as the variables area.
fn basic_lines() -> Vec<u8> {
    fn line(out: &mut Vec<u8>, body: &[u8]) {
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&(body.len() as u16 + 1).to_le_bytes());
        out.extend_from_slice(body);
        out.push(0x0d);
    }
    let mut out = Vec::new();
    // 0 REM, with the credits where a LIST shows them
    let mut rem = vec![0xea];
    rem.extend_from_slice(b"\x16\x02\x05Tapeti Snapshot Loader");
    rem.extend_from_slice(b"\x16\x04\x03Loader from TAPER, (C) 1998");
    rem.extend_from_slice(b"\x16\x06\x03ThunderWare Research Center");
    line(&mut out, &rem);
    // 0 BORDER PI-PI: PAPER PI-PI: INK PI-PI: CLS: PRINT "...":
    //   RANDOMIZE USR (PEEK VAL "23627"+VAL "256"*PEEK VAL "23628")
    let mut run = Vec::new();
    run.extend_from_slice(b"\xe7\xa7-\xa7:\xda\xa7-\xa7:\xd9\xa7-\xa7:\xfb:");
    run.extend_from_slice(b"\xf5\"\x16\x0a\x06\x10\x05Loading snapshot ...");
    run.extend_from_slice(
        b"\x16\x0c\x09\x10\x06\x11\x02\x12\x01 PLEASE WAIT \x16\x00\x00\x12\x00\x11\x00\x10\x00\"",
    );
    run.extend_from_slice(b":\xf9\xc0(\xbe\xb0\"23627\"+\xb0\"256\"*\xbe\xb0\"23628\")");
    line(&mut out, &run);
    out
}

/// Runs at VARS, wherever that is: find out, and move the loader into place.
fn relocator(org: u16, len: usize) -> Vec<u8> {
    let mut a = Asm::at(0);
    a.b(&[
        0xf3, //             DI
        0xcd, 0x52, 0x00, // CALL 0052        ; a RET: leaves our address below SP
        0x3b, //             DEC  SP
        0x3b, //             DEC  SP
        0xe1, //             POP  HL
        0x01, 0x12, 0x00, // LD   BC,0012     ; from there to the loader
        0x09, //             ADD  HL,BC
    ]);
    a.b(&[0x11]).w(org); //         LD   DE,loader
    a.b(&[0x01]).w(len as u16); //  LD   BC,its length
    a.b(&[0xed, 0xb0]); //          LDIR
    a.b(&[0xc3]).w(org); //         JP   loader
    a.0
}

/// The tape loading routine, at `BF55`. The ROM's, reworked to load downwards
/// and to take its timing from two constants; it keeps the EAR bit at 40 and
/// never checks BREAK.
fn turbo_loader(speed: &Speed, border: u8) -> Vec<u8> {
    let mut a = Asm::at(TURBO_ORG);
    a.b(&[
        0xcd, 0x5b, 0xbf, // LD_BLOCK  CALL LD_BYTES
        0xd8, //             RET  C
        0xcf, 0x1a, //       RST  0008 / DEFB 1A   ; R Tape loading error
        0x14, //   LD_BYTES  INC  D
        0x08, //             EX   AF,AF'
        0x15, //             DEC  D
        0x3e, 0x08, //       LD   A,08
        0xd3, 0xfe, //       OUT  (FE),A
        0xdb, 0xfe, //       IN   A,(FE)
        0xe6, 0x40, //       AND  40
        0x4f, //             LD   C,A
        0xbf, //             CP   A
        0xc0, //   LD_BREAK  RET  NZ
        0xcd, 0xca, 0xbf, // LD_START CALL LD_EDGE_1
        0x30, 0xfa, //       JR   NC,LD_BREAK
        0x26, 0x00, //       LD   H,00
        0x06, 0x75, // LD_LEADER LD B,75
        0xcd, 0xc6, 0xbf, // CALL LD_EDGE_2
        0x30, 0xf1, //       JR   NC,LD_BREAK
        0x3e, 0xb0, //       LD   A,B0
        0xb8, //             CP   B
        0x30, 0xed, //       JR   NC,LD_START
        0x24, //             INC  H
        0x20, 0xf1, //       JR   NZ,LD_LEADER
        0x06, 0xb0, // LD_SYNC LD B,B0
        0xcd, 0xca, 0xbf, // CALL LD_EDGE_1
        0x30, 0xe2, //       JR   NC,LD_BREAK
        0x78, //             LD   A,B
        0xfe, 0xc1, //       CP   C1
        0x30, 0xf4, //       JR   NC,LD_SYNC
        0xcd, 0xca, 0xbf, // CALL LD_EDGE_1
        0xd0, //             RET  NC
        0x26, 0x00, //       LD   H,00
        0x06, 0x80, //       LD   B,80
        0x18, 0x17, //       JR   LD_MARKER
        0x08, //   LD_LOOP   EX   AF,AF'
        0x20, 0x05, //       JR   NZ,LD_FLAG
        0xdd, 0x75, 0x00, // LD   (IX+00),L
        0x18, 0x09, //       JR   LD_NEXT
        0xcb, 0x11, // LD_FLAG RL C
        0xad, //             XOR  L
        0xc0, //             RET  NZ
        0x79, //             LD   A,C
        0x1f, //             RRA
        0x4f, //             LD   C,A
        0x18, 0x03, //       JR   LD_FLNEXT
        0xdd, 0x2b, // LD_NEXT DEC IX              ; downwards
        0x1b, //             DEC  DE
        0x08, //   LD_FLNEXT EX   AF,AF'
        0x06, 0x82, //       LD   B,82
        0x2e, 0x01, // LD_MARKER LD L,01
        0xcd, 0xc6, 0xbf, // LD_8_BITS CALL LD_EDGE_2
        0xd0, //             RET  NC
        0x3e, //             LD   A,compare
    ]);
    a.b(&[speed.compare]);
    a.b(&[
        0xb8, //             CP   B
        0xcb, 0x15, //       RL   L
        0x06, 0x80, //       LD   B,80
        0x30, 0xf3, //       JR   NC,LD_8_BITS
        0x7c, //             LD   A,H
        0xad, //             XOR  L
        0x67, //             LD   H,A
        0x7a, //             LD   A,D
        0xb3, //             OR   E
        0x20, 0xd3, //       JR   NZ,LD_LOOP
        0x7c, //             LD   A,H
        0xfe, 0x01, //       CP   01
        0xc9, //             RET
        0xcd, 0xca, 0xbf, // LD_EDGE_2 CALL LD_EDGE_1
        0xd0, //             RET  NC
        0x3e, //   LD_EDGE_1 LD   A,delay
    ]);
    a.b(&[speed.delay]);
    a.b(&[
        0x3d, //   LD_DELAY  DEC  A
        0x20, 0xfd, //       JR   NZ,LD_DELAY
        0xa7, //             AND  A
        0x04, //   LD_SAMPLE INC  B
        0xc8, //             RET  Z
        0xdb, 0xfe, //       IN   A,(FE)
        0xa9, //             XOR  C
        0xe6, 0x40, //       AND  40
        0x28, 0xf7, //       JR   Z,LD_SAMPLE
        0x79, //             LD   A,C
        0xee, //             XOR  40+stripe colour
    ]);
    // Black on a black border would show nothing: stripe in blue instead.
    a.b(&[0x40 | if border == 0 { 1 } else { border }]);
    a.b(&[
        0x4f, //             LD   C,A
        0xe6, 0x07, //       AND  07
        0xf6, 0x08, //       OR   08
        0xd3, 0xfe, //       OUT  (FE),A
        0x37, //             SCF
        0xc9, //             RET
    ]);
    a.0
}

/// Unpacks `BC` bytes at `HL` to the start of the 16K that `HL` is in; returns
/// at once for `BC` = 0, which is how an unpacked page passes through.
const UNPACKER: [u8; 40] = [
    0x7c, //             LD   A,H
    0xe6, 0xc0, //       AND  C0
    0x57, //             LD   D,A
    0x1e, 0x00, //       LD   E,00
    0x78, //   CHECKEND  LD   A,B
    0xb1, //             OR   C
    0xc8, //             RET  Z
    0x7e, //             LD   A,(HL)
    0xfe, 0xed, //       CP   ED
    0x20, 0x05, //       JR   NZ,COPY
    0x23, //             INC  HL
    0xbe, //             CP   (HL)
    0x28, 0x05, //       JR   Z,EXPAND
    0x2b, //             DEC  HL
    0xed, 0xa0, // COPY  LDI
    0x18, 0xef, //       JR   CHECKEND
    0x23, //   EXPAND    INC  HL
    0xc5, //             PUSH BC
    0x46, //             LD   B,(HL)
    0x23, //             INC  HL
    0x7e, //             LD   A,(HL)
    0x23, //             INC  HL
    0x12, //   FILL      LD   (DE),A
    0x13, //             INC  DE
    0x10, 0xfc, //       DJNZ FILL
    0xc1, //             POP  BC
    0x0b, 0x0b, 0x0b, 0x0b, // DEC BC (x4)
    0x18, 0xde, //       JR   CHECKEND
];

/// Clears the loader's 512 bytes; runs from the screen when the snapshot has
/// zeroes there, which saves loading them.
fn clear_loader(org: u16, len: usize) -> Vec<u8> {
    let mut a = Asm::at(0);
    a.b(&[0x21]).w(org); //             LD   HL,loader
    a.b(&[0x11]).w(org + 1); //         LD   DE,loader+1
    a.b(&[0x01]).w(len as u16 - 1); //  LD   BC,length-1
    a.b(&[
        0x75, //         LD   (HL),L      ; the loader starts on a 256 byte boundary
        0xed, 0xb0, //   LDIR
        0xc9, //         RET              ; to the last stage
    ]);
    a.0
}

/// One page as it goes on the tape.
struct PageBlock {
    /// What goes out on port 7FFD before loading.
    paging: u8,
    /// High byte of the last address loaded; the low byte is always FF.
    end_hi: u8,
    /// Packed length, or 0 for a page sent as it is.
    packed_len: u16,
    /// In loading order: last address first.
    bytes: Vec<u8>,
}

fn page_block(data: &[u8], base: u16, paging: u8, may_cover_screen: bool) -> PageBlock {
    let packed = pack(data);
    let covers_screen = base == 0x4000 && packed.len() > PAGE - SCREEN;
    let usable = unpacks_in_place(&packed, data.len()) && (may_cover_screen || !covers_screen);
    let (mut bytes, packed_len) = if usable {
        let n = packed.len() as u16;
        (packed, n)
    } else {
        (data.to_vec(), 0)
    };
    bytes.reverse();
    PageBlock { paging, end_hi: ((usize::from(base) + data.len() - 1) >> 8) as u8, packed_len, bytes }
}

/// The blocks of a tape that loads `snap`.
pub fn snapshot_to_blocks(snap: &Snapshot, opts: &LoaderOptions) -> Result<Vec<Block>, String> {
    let speed = SPEEDS.get(usize::from(opts.speed)).ok_or("Unknown loading speed")?;
    let border = opts.border & 7;
    if let Some(screen) = opts.screen {
        if screen.len() != SCREEN {
            return Err(format!("A loading screen is {SCREEN} bytes; this file has {}", screen.len()));
        }
    }
    let scorpion = snap.is_128k && snap.is_scorpion;
    let loader_len = if scorpion { LOADER_LEN_SCORPION } else { LOADER_LEN };
    let org = LOADER_END - loader_len as u16;
    let zeroes = vec![0u8; PAGE];
    let page5 = snap.pages[5].as_deref().ok_or("The snapshot has no screen memory")?;
    let under_loader = &snap.pages[2].as_deref().unwrap_or(&zeroes)[PAGE - loader_len..];
    // The 128K loader always waits for this block, so a 128K tape always sends it.
    let reload = snap.is_128k || under_loader.iter().any(|b| *b != 0);

    // ---- the pages, in loading order ----
    // Page 0 comes last, which leaves both paging ports as a reset does.
    let mut order: Vec<usize> = match (snap.is_128k, scorpion) {
        (false, _) => vec![5, 2, 0],
        (true, false) => vec![5, 2, 1, 3, 4, 6, 7, 0],
        (true, true) => vec![5, 2, 1, 3, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0],
    };
    let mut pages: Vec<PageBlock> = Vec::new();
    if let Some(screen) = opts.screen {
        // The screen goes first and its page last, so it stays up while the rest loads.
        order.rotate_left(1);
        let alone = pack(screen);
        if unpacks_in_place(&alone, SCREEN) {
            let mut bytes = alone;
            bytes.reverse();
            pages.push(PageBlock { paging: 0x10, end_hi: 0x7f, packed_len: bytes.len() as u16, bytes });
        } else {
            // Unpacked it has to be a whole page: fill it up with what comes after the screen.
            let mut whole = screen.to_vec();
            whole.extend_from_slice(&page5[SCREEN..]);
            pages.push(page_block(&whole, 0x4000, 0x10, true));
        }
    }
    for page in order {
        let Some(data) = snap.pages[page].as_deref() else { continue };
        let (base, len) = match page {
            5 => (0x4000, PAGE),
            2 => (0x8000, PAGE - loader_len),
            _ => (0xc000, PAGE),
        };
        // 10 alone keeps a 128K machine on the 48K ROM with page 0 in, so a 48K
        // snapshot loads there too. Bit 7 asks the Scorpion loader for bit 4 of 1FFD.
        let paging = match (snap.is_128k, page) {
            (false, _) => 0x10,
            (true, 0..=7) => 0x10 | page as u8,
            (true, _) => 0x90 | (page as u8 & 7),
        };
        pages.push(page_block(&data[..len], base, paging, opts.compress_all));
    }

    // ---- the last stage and its stack frame ----
    let r = &snap.regs;
    let mut last = Asm::at(0);
    last.b(&[
        0xf1, //             POP  AF
        0xd3, 0xfe, //       OUT  (FE),A      ; border
        0xc1, //             POP  BC          ; 7FFD
        0xf1, //             POP  AF
        0xed, 0x79, //       OUT  (C),A       ; paging as the snapshot had it
    ]);
    if scorpion {
        last.b(&[
            0xc1, //         POP  BC          ; 1FFD
            0xf1, //         POP  AF
            0xed, 0x79, //   OUT  (C),A
        ]);
    }
    last.b(&[
        0xdd, 0xe1, //       POP  IX
        0xf1, //             POP  AF
        0x08, //             EX   AF,AF'
        0xe1, //             POP  HL
        0x7c, //             LD   A,H
        0xed, 0x47, //       LD   I,A
        0x7d, //             LD   A,L
        0xed, 0x4f, //       LD   R,A         ; nine more fetches follow
        0xe1, //             POP  HL
        0xd1, //             POP  DE
        0xc1, //             POP  BC
        0xf1, //             POP  AF
        0x31, //             LD   SP,nn
    ]);
    last.w(r.sp);
    last.b(&[0xed, [0x46, 0x56, 0x5e][usize::from(r.im.min(2))]]); // IM n
    last.b(&[if r.iff1 { 0xfb } else { 0xf3 }]); //                   EI / DI
    last.b(&[0xc3]).w(r.pc); //                                      JP   nn
    let last = last.0;

    // Where the block that replaces the loader (or the clearing) returns to.
    let resume: u16 = if snap.is_128k { AY_STAGE_AT } else { LAST_STAGE_AT };
    let mut frame = Asm::at(0);
    frame.w(resume);
    frame.b(&[0, border]);
    frame.w(0x7ffd);
    frame.b(&[0, if snap.is_128k { snap.port_7ffd } else { 0x10 }]);
    if scorpion {
        frame.w(0x1ffd);
        frame.b(&[0, snap.port_1ffd]);
    }
    frame.w(r.ix);
    frame.b(&[r.af_alt as u8, (r.af_alt >> 8) as u8]);
    frame.b(&[(r.r.wrapping_sub(9) & 0x7f) | (r.r & 0x80), r.i]);
    frame.w(r.hl).w(r.de).w(r.bc);
    frame.b(&[r.af as u8, (r.af >> 8) as u8]);
    let frame = frame.0;
    // The frame ends two bytes short of the attributes.
    let frame_at = 0x57fe - frame.len() as u16;
    let ay_frame: Vec<u8> = (0..16).rev().flat_map(|reg| [0, snap.ay[reg]]).collect();

    let mut ay_stage = Asm::at(0);
    ay_stage.b(&[0x31]).w(AY_FRAME_AT); // LD   SP,AY frame
    ay_stage.b(&[
        0x16, 0x10, //       LD   D,10
        0x01, 0xfd, 0xff, // NEXT LD BC,FFFD
        0x15, //             DEC  D
        0xed, 0x51, //       OUT  (C),D       ; pick the register
        0xf1, //             POP  AF
        0x06, 0xbf, //       LD   B,BF
        0xed, 0x79, //       OUT  (C),A       ; set it
        0x7a, //             LD   A,D
        0xa7, //             AND  A
        0x20, 0xf1, //       JR   NZ,NEXT
    ]);
    ay_stage.b(&[0x31]).w(frame_at + 2); // LD SP,main frame, past the return address
    ay_stage.b(&[0x06, 0xff]); //             LD   B,FF
    ay_stage.b(&[0x3e, snap.port_fffd]); //   LD   A,n
    ay_stage.b(&[0xed, 0x79]); //             OUT  (C),A  ; the register it had selected
    ay_stage.b(&[0xc3]).w(LAST_STAGE_AT); // JP last stage
    let ay_stage = ay_stage.0;
    let clear = clear_loader(org, loader_len);

    // ---- the loader ----
    let mut a = Asm::at(org);
    a.b(&[
        0xf3, //             DI
        0x31, 0x00, 0xc0, // LD   SP,C000
        0xdd, 0x21, //       LD   IX,table
    ]);
    let table_operand = a.0.len();
    a.w(0);
    a.b(&[0xfd, 0x21]).w(r.iy); //  LD IY,nn     ; these two sets stay as they are
    a.b(&[0x21]).w(r.hl_alt);
    a.b(&[0x11]).w(r.de_alt);
    a.b(&[0x01]).w(r.bc_alt);
    a.b(&[0xd9]); //                EXX
    let next = a.0.len();
    a.b(&[
        0xdd, 0x7e, 0x00, // NEXT LD A,(IX+00)
        0xa7, //             AND  A
        0x28, 0x00, //       JR   Z,DONE
    ]);
    let done_operand = a.0.len() - 1;
    a.b(&[
        0xdd, 0x66, 0x01, // LD   H,(IX+01)
        0x2e, 0xff, //       LD   L,FF
        0xdd, 0x5e, 0x02, // LD   E,(IX+02)
        0xdd, 0x56, 0x03, // LD   D,(IX+03)
        0x01, 0xfd, 0x7f, // LD   BC,7FFD
        0xed, 0x79, //       OUT  (C),A
    ]);
    if scorpion {
        a.b(&[
            0xe6, 0x80, //   AND  80          ; bit 7 of the entry
            0x0f, 0x0f, 0x0f, // RRCA (x3)    ; is bit 4 of 1FFD
            0x06, 0x1f, //   LD   B,1F
            0xed, 0x79, //   OUT  (C),A
        ]);
    }
    a.b(&[
        0x01, 0x04, 0x00, // LD   BC,0004
        0xdd, 0x09, //       ADD  IX,BC
        0xdd, 0xe5, //       PUSH IX
        0xd5, //             PUSH DE          ; the packed length, 0 for unpacked
        0x7a, //             LD   A,D
        0xb3, //             OR   E
        0x20, 0x05, //       JR   NZ,LOAD
        0x7c, //             LD   A,H         ; unpacked: from the end of the
        0xe6, 0x3f, //       AND  3F          ; block down to the start of its 16K
        0x3c, //             INC  A
        0x57, //             LD   D,A
        0xe5, //   LOAD      PUSH HL
        0xdd, 0xe1, //       POP  IX
    ]);
    a.b(&[0x3e, FLAG_PAGE]); //     LD   A,AA
    a.b(&[
        0x37, //             SCF
        0xcd, //             CALL LD_BLOCK
    ]);
    a.w(TURBO_ORG);
    a.b(&[
        0xc1, //             POP  BC
        0xdd, 0xe5, //       PUSH IX
        0xe1, //             POP  HL
        0x23, //             INC  HL          ; the first byte loaded last
        0xcd, //             CALL unpacker
    ]);
    let unpacker_operand = a.0.len();
    a.w(0);
    a.b(&[
        0xdd, 0xe1, //       POP  IX
        0x18, 0x00, //       JR   NEXT
    ]);
    let here = a.0.len();
    a.jr_to(here - 1, next);
    a.jr_to(done_operand, here);
    // DONE: everything but the loader's own bytes is in. Whichever page came
    // last, its entry left the 48K ROM in (bit 4) and 1FFD at 0, which is all
    // the rest needs: nothing from here on touches the top 16K.
    a.b(&[
        0x21, 0xe0, 0x5a, // LD   HL,5AE0     ; hide what is about to go on screen
        0x06, 0x20, //       LD   B,20
        0x3e, //             LD   A,paper and ink both the border's
    ]);
    a.b(&[border | border << 3]);
    a.b(&[
        0x77, //   FILL      LD   (HL),A
        0x23, //             INC  HL
        0x10, 0xfc, //       DJNZ FILL
        0x21, //             LD   HL,last stage
    ]);
    let stages_operand = a.0.len();
    a.w(0);
    // LD DE,to / LD C,length / LDIR, one after the other through what follows the table
    let copy = |a: &mut Asm, to: u16, len: usize| {
        a.b(&[0x11]).w(to).b(&[0x0e, len as u8, 0xed, 0xb0]);
    };
    copy(&mut a, LAST_STAGE_AT, last.len());
    if snap.is_128k {
        copy(&mut a, AY_STAGE_AT, ay_stage.len());
    }
    copy(&mut a, frame_at, frame.len());
    if snap.is_128k {
        copy(&mut a, AY_FRAME_AT, ay_frame.len());
    }
    a.b(&[0x31]).w(frame_at); //    LD   SP,frame    ; it starts with the return address
    if !reload {
        a.b(&[0x11]).w(AY_STAGE_AT); // LD DE,..     ; free on a 48K machine
        a.b(&[0x0e, clear.len() as u8]); // LD C,length
        a.b(&[
            0xd5, //         PUSH DE
            0xed, 0xb0, //   LDIR
            0xc9, //         RET
        ]);
    } else {
        a.b(&[0xdd, 0x21]).w(org); //          LD   IX,loader
        a.b(&[0x11]).w(loader_len as u16); //  LD   DE,its length
        a.b(&[0x3e, FLAG_FINAL]); //           LD   A,55
        a.b(&[
            0xbc, //             CP   H           ; carry set and not zero: LOAD
            0x08, //             EX   AF,AF'
            0xc3, 0x62, 0x05, // JP   0562        ; the ROM's LD_BYTES, past its own setup
        ]);
    }
    let unpacker_at = a.here();
    a.b(&UNPACKER);
    let table_at = a.here();
    for p in &pages {
        a.b(&[p.paging, p.end_hi]).w(p.packed_len);
    }
    a.b(&[0]);
    let stages_at = a.here();
    a.b(&last);
    if snap.is_128k {
        a.b(&ay_stage);
    }
    a.b(&frame);
    if snap.is_128k {
        a.b(&ay_frame);
    } else if !reload {
        a.b(&clear);
    }
    a.patch_w(table_operand, table_at);
    a.patch_w(unpacker_operand, unpacker_at);
    a.patch_w(stages_operand, stages_at);
    let mut code = a.0;
    let turbo = turbo_loader(speed, border);
    let turbo_at = usize::from(TURBO_ORG - org);
    // The stack grows down from the loader's end: six words at its deepest.
    if code.len() > turbo_at || turbo_at + turbo.len() > loader_len - 16 {
        return Err(format!("The loader does not fit: {} bytes of code, {} of room", code.len(), turbo_at));
    }
    code.resize(turbo_at, 0);
    code.extend_from_slice(&turbo);
    code.resize(loader_len, 0);

    // ---- the tape ----
    let mut blocks = Vec::new();
    let machine = match (snap.is_128k, scorpion) {
        (false, _) => "48K",
        (true, false) => "128K",
        (true, true) => "Scorpion 256K",
    };
    blocks.push(Block::new(Body::Text { text: format!("{machine} snapshot - {} bps", speed.bps) }));
    let lines = basic_lines();
    let mut program = lines.clone();
    program.extend_from_slice(&relocator(org, loader_len));
    program.extend_from_slice(&code);
    let header = encode_header(&HeaderInfo {
        kind: 0,
        type_name: HEADER_TYPE_NAMES[0].to_string(),
        name: program_name(opts.name),
        length: program.len() as u16,
        param1: 0,
        param2: lines.len() as u16,
    });
    blocks.push(Block::new(Body::Standard { pause: 1000, data: header }));
    blocks.push(Block::new(Body::Standard { pause: 1000, data: framed(0xff, &program) }));
    let count = pages.len();
    for (n, p) in pages.into_iter().enumerate() {
        // No gap between the pages: the pilot is what covers the unpacking. The
        // tape must not end on a data bit though, or its last edge never comes.
        let pause = if n + 1 == count && !reload { 1000 } else { 0 };
        let data = framed(FLAG_PAGE, &p.bytes);
        blocks.push(Block::new(if opts.speed == 0 {
            Body::Standard { pause, data }
        } else {
            Body::Turbo {
                pilot: speed.pilot,
                sync1: speed.sync,
                sync2: speed.sync,
                zero: speed.zero,
                one: speed.zero * 2,
                // one second
                pilot_len: (3_500_000 / u32::from(speed.pilot)) as u16,
                used_bits: 8,
                pause,
                data,
            }
        }));
    }
    if reload {
        blocks.push(Block::new(Body::Standard { pause: 1000, data: framed(FLAG_FINAL, under_loader) }));
    }
    Ok(blocks)
}

/// Flag, payload, checksum.
fn framed(flag: u8, payload: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(payload.len() + 2);
    data.push(flag);
    data.extend_from_slice(payload);
    data.push(checksum(&data));
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_round_trips() {
        let cases: [&[u8]; 6] = [
            &[],
            &[1, 2, 3],
            &[0; 1000],
            &[0xed, 0xed, 0xed, 1, 0xed, 0, 0, 0, 0, 0, 0, 0, 7],
            &[5, 5, 5, 5, 0xed],
            &[0xed, 0xed],
        ];
        for case in cases {
            assert_eq!(unpack(&pack(case), case.len()).as_deref(), Some(case));
        }
        // A byte after a lone ED is never the start of a run.
        assert_eq!(pack(&[0xed, 0, 0, 0, 0, 0, 0]), [0xed, 0, 0xed, 0xed, 5, 0]);
    }

    #[test]
    fn in_place_check_knows_the_unpackers_blind_spots() {
        let mut page = vec![0u8; 100];
        assert!(unpacks_in_place(&pack(&page), 100));
        // ends in a lone ED: the unpacker would look past the end
        page[99] = 0xed;
        assert!(!unpacks_in_place(&pack(&page), 100));
        // ED pairs at the end shrink when unpacked, so the output catches up
        let mut page = vec![0u8; 100];
        page[98] = 0xed;
        page[99] = 0xed;
        assert!(!unpacks_in_place(&pack(&page), 100));
        // nothing gained
        assert!(!unpacks_in_place(&[1, 2, 3], 3));
    }

    #[test]
    fn names_come_from_the_file_stem() {
        assert_eq!(program_name("C:\\games\\Manic Miner.z80"), "Manic Mine");
        assert_eq!(program_name("/tmp/a.b.sna"), "a.b");
        assert_eq!(program_name(".z80"), "snapshot");
    }
}
