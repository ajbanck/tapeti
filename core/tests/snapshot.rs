//! The snapshot loader, run for real: a small Z80 interpreter with T-state
//! counting executes the generated loader against the pulses the tape's blocks
//! produce, and the machine it leaves behind is compared with the snapshot.
//!
//! The interpreter knows only the instructions the loader uses and panics on
//! anything else, which is itself a check that the code went where it should.
//! There is no ROM: `0052` holds the `RET` the relocator counts on, and the
//! ROM's `LD_BYTES` at `0562` is stood in for by copying the block in.

use tapeti_core::audio::{emit_block, RecordingSink};
use tapeti_core::describe::decode_header;
use tapeti_core::snapshot::{
    pack, parse_snapshot, snapshot_to_blocks, LoaderOptions, Registers, Snapshot, SnapshotKind, FLAG_FINAL,
    SPEED_BPS,
};
use tapeti_core::spectrum::basic::{list_basic, BasicOptions};
use tapeti_core::types::{Block, Body};

const PAGE: usize = 16384;
const PROG: u16 = 0x5ccb;

struct Rng(u32);
impl Rng {
    fn byte(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as u8
    }
    fn fill(&mut self, out: &mut [u8]) {
        for b in out {
            *b = self.byte();
        }
    }
}

// ---- the machine -----------------------------------------------------------

const FLAG_C: u8 = 0x01;
const FLAG_Z: u8 = 0x40;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Model {
    Spectrum48,
    Spectrum128,
    Scorpion,
}

struct Machine {
    ram: Vec<Vec<u8>>,
    rom: Vec<u8>,
    model: Model,
    port_7ffd: u8,
    port_1ffd: u8,
    port_fffd: u8,
    ay: [u8; 16],
    border: u8,
    // B C D E H L (F) A, so an opcode's register field indexes it directly
    r8: [u8; 8],
    alt: [u8; 8],
    ix: u16,
    iy: u16,
    sp: u16,
    pc: u16,
    i: u8,
    r: u8,
    iff1: bool,
    im: u8,
    t: u64,
    /// `(end time, level)` per pulse.
    tape: Vec<(u64, u8)>,
    tape_at: usize,
    final_block: Option<Vec<u8>>,
}

const B: usize = 0;
const C: usize = 1;
const D: usize = 2;
const E: usize = 3;
const H: usize = 4;
const L: usize = 5;
const F: usize = 6;
const A: usize = 7;

impl Machine {
    fn new(model: Model) -> Self {
        let mut rom = vec![0u8; PAGE];
        rom[0x52] = 0xc9;
        Machine {
            // Memory is never empty when a loader starts.
            ram: (0..16).map(|p| vec![0xa5 ^ p as u8; PAGE]).collect(),
            rom,
            model,
            port_7ffd: 0x10,
            port_1ffd: 0,
            port_fffd: 0,
            ay: [0xff; 16],
            border: 0xff,
            r8: [0; 8],
            alt: [0; 8],
            ix: 0,
            iy: 0x5c3a,
            sp: 0xff40,
            pc: 0,
            i: 0x3f,
            r: 0,
            iff1: true,
            im: 1,
            t: 0,
            tape: Vec::new(),
            tape_at: 0,
            final_block: None,
        }
    }

    fn page_at(&self, addr: u16) -> usize {
        match addr >> 14 {
            1 => 5,
            2 => 2,
            _ => match self.model {
                Model::Spectrum48 => 0,
                Model::Spectrum128 => usize::from(self.port_7ffd & 7),
                Model::Scorpion => usize::from(self.port_7ffd & 7 | (self.port_1ffd & 0x10) >> 1),
            },
        }
    }

    fn peek(&self, addr: u16) -> u8 {
        if addr < 0x4000 {
            self.rom[usize::from(addr)]
        } else {
            self.ram[self.page_at(addr)][usize::from(addr) & 0x3fff]
        }
    }

    fn poke(&mut self, addr: u16, v: u8) {
        if addr >= 0x4000 {
            let page = self.page_at(addr);
            self.ram[page][usize::from(addr) & 0x3fff] = v;
        }
    }

    fn peek16(&self, addr: u16) -> u16 {
        u16::from(self.peek(addr)) | u16::from(self.peek(addr.wrapping_add(1))) << 8
    }

    fn fetch(&mut self) -> u8 {
        let v = self.peek(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    fn fetch16(&mut self) -> u16 {
        let lo = self.fetch();
        u16::from(lo) | u16::from(self.fetch()) << 8
    }

    /// An opcode fetch: as `fetch`, and the refresh register counts it.
    fn opcode(&mut self) -> u8 {
        self.r = (self.r & 0x80) | (self.r.wrapping_add(1) & 0x7f);
        self.fetch()
    }

    fn push(&mut self, v: u16) {
        self.sp = self.sp.wrapping_sub(1);
        self.poke(self.sp, (v >> 8) as u8);
        self.sp = self.sp.wrapping_sub(1);
        self.poke(self.sp, v as u8);
    }

    fn pop(&mut self) -> u16 {
        let v = self.peek16(self.sp);
        self.sp = self.sp.wrapping_add(2);
        v
    }

    fn pair(&self, hi: usize) -> u16 {
        u16::from(self.r8[hi]) << 8 | u16::from(self.r8[hi + 1])
    }

    fn set_pair(&mut self, hi: usize, v: u16) {
        self.r8[hi] = (v >> 8) as u8;
        self.r8[hi + 1] = v as u8;
    }

    fn af(&self) -> u16 {
        u16::from(self.r8[A]) << 8 | u16::from(self.r8[F])
    }

    fn flag(&self, f: u8) -> bool {
        self.r8[F] & f != 0
    }

    fn set_zc(&mut self, z: bool, c: bool) {
        self.r8[F] = (self.r8[F] & !(FLAG_Z | FLAG_C)) | if z { FLAG_Z } else { 0 } | u8::from(c);
    }

    fn set_z(&mut self, z: bool) {
        self.r8[F] = (self.r8[F] & !FLAG_Z) | if z { FLAG_Z } else { 0 };
    }

    /// Register or `(HL)` by an opcode's three-bit field; the second value is
    /// the extra T-states `(HL)` costs.
    fn get(&self, field: u8) -> (u8, u64) {
        match field {
            6 => (self.peek(self.pair(H)), 3),
            7 => (self.r8[A], 0),
            n => (self.r8[usize::from(n)], 0),
        }
    }

    fn set(&mut self, field: u8, v: u8) -> u64 {
        match field {
            6 => {
                self.poke(self.pair(H), v);
                3
            }
            7 => {
                self.r8[A] = v;
                0
            }
            n => {
                self.r8[usize::from(n)] = v;
                0
            }
        }
    }

    fn alu(&mut self, op: u8, v: u8) {
        let a = self.r8[A];
        match op {
            4 => self.r8[A] = a & v,
            5 => self.r8[A] = a ^ v,
            6 => self.r8[A] = a | v,
            7 => {
                self.set_zc(a == v, a < v);
                return;
            }
            _ => panic!("ALU operation {op} at {:04x}", self.pc),
        }
        self.set_zc(self.r8[A] == 0, false);
    }

    fn condition(&self, cc: u8) -> bool {
        match cc {
            0 => !self.flag(FLAG_Z),
            1 => self.flag(FLAG_Z),
            2 => !self.flag(FLAG_C),
            3 => self.flag(FLAG_C),
            _ => panic!("condition {cc}"),
        }
    }

    fn ear(&mut self, at: u64) -> u8 {
        while self.tape_at < self.tape.len() && at >= self.tape[self.tape_at].0 {
            self.tape_at += 1;
        }
        self.tape.get(self.tape_at).or(self.tape.last()).map_or(0, |p| p.1)
    }

    fn port_out(&mut self, port: u16, v: u8) {
        if port & 1 == 0 {
            self.border = v & 7;
        } else if self.model == Model::Scorpion && port & 0xe002 == 0 {
            // Only the Scorpion tells 1FFD from 7FFD; a 128K takes both as 7FFD.
            self.port_1ffd = v;
        } else if port & 0x8002 == 0 {
            if self.model != Model::Spectrum48 && self.port_7ffd & 0x20 == 0 {
                self.port_7ffd = v;
            }
        } else if port & 0xc002 == 0xc000 {
            self.port_fffd = v;
        } else if port & 0xc002 == 0x8000 {
            self.ay[usize::from(self.port_fffd & 15)] = v;
        }
    }

    fn step(&mut self) {
        let at = self.pc;
        let op = self.opcode();
        self.t += match op {
            0x01 | 0x11 | 0x21 => {
                let v = self.fetch16();
                self.set_pair(usize::from(op >> 3), v);
                10
            }
            0x31 => {
                self.sp = self.fetch16();
                10
            }
            0x03 | 0x13 | 0x23 => {
                let hi = usize::from(op >> 3);
                self.set_pair(hi, self.pair(hi).wrapping_add(1));
                6
            }
            0x0b | 0x1b | 0x2b => {
                let hi = usize::from(op >> 3) - 1;
                self.set_pair(hi, self.pair(hi).wrapping_sub(1));
                6
            }
            0x3b => {
                self.sp = self.sp.wrapping_sub(1);
                6
            }
            0x09 => {
                let (sum, carry) = self.pair(H).overflowing_add(self.pair(B));
                self.set_pair(H, sum);
                self.r8[F] = (self.r8[F] & !FLAG_C) | u8::from(carry);
                11
            }
            0x08 => {
                for n in [F, A] {
                    std::mem::swap(&mut self.r8[n], &mut self.alt[n]);
                }
                4
            }
            0xd9 => {
                for n in [B, C, D, E, H, L] {
                    std::mem::swap(&mut self.r8[n], &mut self.alt[n]);
                }
                4
            }
            0x12 => {
                self.poke(self.pair(D), self.r8[A]);
                7
            }
            0x10 => {
                let d = self.fetch() as i8;
                self.r8[B] = self.r8[B].wrapping_sub(1);
                if self.r8[B] != 0 {
                    self.pc = self.pc.wrapping_add(d as u16);
                    13
                } else {
                    8
                }
            }
            0x18 => {
                let d = self.fetch() as i8;
                self.pc = self.pc.wrapping_add(d as u16);
                12
            }
            0x20 | 0x28 | 0x30 | 0x38 => {
                let d = self.fetch() as i8;
                if self.condition((op >> 3) & 3) {
                    self.pc = self.pc.wrapping_add(d as u16);
                    12
                } else {
                    7
                }
            }
            0x1f => {
                let a = self.r8[A];
                self.r8[A] = a >> 1 | u8::from(self.flag(FLAG_C)) << 7;
                self.r8[F] = (self.r8[F] & !FLAG_C) | (a & 1);
                4
            }
            0x0f => {
                let a = self.r8[A];
                self.r8[A] = a.rotate_right(1);
                self.r8[F] = (self.r8[F] & !FLAG_C) | (a & 1);
                4
            }
            0x37 => {
                self.r8[F] |= FLAG_C;
                4
            }
            0xf3 | 0xfb => {
                self.iff1 = op == 0xfb;
                4
            }
            // INC r / DEC r
            _ if op & 0xc6 == 0x04 => {
                let field = (op >> 3) & 7;
                let (v, extra) = self.get(field);
                let v = if op & 1 == 0 { v.wrapping_add(1) } else { v.wrapping_sub(1) };
                self.set(field, v);
                self.set_z(v == 0);
                4 + extra * 7 / 3
            }
            // LD r,n
            _ if op & 0xc7 == 0x06 => {
                let v = self.fetch();
                7 + self.set((op >> 3) & 7, v)
            }
            0x76 => panic!("HALT at {at:04x}"),
            // LD r,r'
            0x40..=0x7f => {
                let (v, extra) = self.get(op & 7);
                4 + extra + self.set((op >> 3) & 7, v)
            }
            0x80..=0xbf => {
                let (v, extra) = self.get(op & 7);
                self.alu((op >> 3) & 7, v);
                4 + extra
            }
            0xe6 | 0xee | 0xf6 | 0xfe => {
                let v = self.fetch();
                self.alu((op >> 3) & 7, v);
                7
            }
            0xc1 | 0xd1 | 0xe1 => {
                let v = self.pop();
                self.set_pair(usize::from((op >> 3) & 6), v);
                10
            }
            0xf1 => {
                let v = self.pop();
                self.r8[A] = (v >> 8) as u8;
                self.r8[F] = v as u8;
                10
            }
            0xc5 | 0xd5 | 0xe5 => {
                self.push(self.pair(usize::from((op >> 3) & 6)));
                11
            }
            0xc0 | 0xc8 | 0xd0 | 0xd8 => {
                if self.condition((op >> 3) & 3) {
                    self.pc = self.pop();
                    11
                } else {
                    5
                }
            }
            0xc9 => {
                self.pc = self.pop();
                10
            }
            0xc3 => {
                self.pc = self.fetch16();
                10
            }
            0xcd => {
                let to = self.fetch16();
                self.push(self.pc);
                self.pc = to;
                17
            }
            0xcf => panic!("the loader reported a tape loading error (RST 8 from {at:04x}) t={} tape_at={}/{} B={:02x} C={:02x} H={:02x} L={:02x} DE={:04x} IX={:04x} A'={:02x} F'={:02x}", self.t, self.tape_at, self.tape.len(), self.r8[B], self.r8[C], self.r8[H], self.r8[L], self.pair(D), self.ix, self.alt[A], self.alt[F]),
            0xd3 => {
                let n = self.fetch();
                self.port_out(u16::from(self.r8[A]) << 8 | u16::from(n), self.r8[A]);
                11
            }
            0xdb => {
                let n = self.fetch();
                assert_eq!(n, 0xfe, "IN from port {n:02x}");
                self.r8[A] = 0xbf | self.ear(self.t + 8) << 6;
                11
            }
            0xcb => {
                let op = self.opcode();
                assert_eq!(op & 0xf8, 0x10, "CB {op:02x} at {at:04x}");
                let (v, extra) = self.get(op & 7);
                let out = v << 1 | u8::from(self.flag(FLAG_C));
                self.set(op & 7, out);
                self.set_zc(out == 0, v & 0x80 != 0);
                8 + extra * 7 / 3
            }
            0xed => {
                let op = self.opcode();
                match op {
                    0x51 | 0x79 => {
                        let v = self.get((op >> 3) & 7).0;
                        self.port_out(self.pair(B), v);
                        12
                    }
                    0x47 => {
                        self.i = self.r8[A];
                        9
                    }
                    0x4f => {
                        self.r = self.r8[A];
                        9
                    }
                    0x46 | 0x56 | 0x5e => {
                        self.im = match op {
                            0x46 => 0,
                            0x56 => 1,
                            _ => 2,
                        };
                        8
                    }
                    0xa0 | 0xb0 => {
                        let mut t = 0;
                        loop {
                            let v = self.peek(self.pair(H));
                            self.poke(self.pair(D), v);
                            self.set_pair(H, self.pair(H).wrapping_add(1));
                            self.set_pair(D, self.pair(D).wrapping_add(1));
                            self.set_pair(B, self.pair(B).wrapping_sub(1));
                            if op == 0xa0 || self.pair(B) == 0 {
                                break t + 16;
                            }
                            t += 21;
                        }
                    }
                    _ => panic!("ED {op:02x} at {at:04x}"),
                }
            }
            0xdd | 0xfd => {
                let prefix = op;
                let op = self.opcode();
                let index = if prefix == 0xdd { self.ix } else { self.iy };
                let set_index = |m: &mut Machine, v: u16| {
                    if prefix == 0xdd {
                        m.ix = v
                    } else {
                        m.iy = v
                    }
                };
                match op {
                    0x21 => {
                        let v = self.fetch16();
                        set_index(self, v);
                        14
                    }
                    0x09 => {
                        set_index(self, index.wrapping_add(self.pair(B)));
                        15
                    }
                    0x2b => {
                        set_index(self, index.wrapping_sub(1));
                        10
                    }
                    0xe5 => {
                        self.push(index);
                        15
                    }
                    0xe1 => {
                        let v = self.pop();
                        set_index(self, v);
                        14
                    }
                    // LD r,(IX+d)
                    _ if op & 0xc7 == 0x46 => {
                        let d = self.fetch() as i8;
                        let v = self.peek(index.wrapping_add(d as u16));
                        self.set((op >> 3) & 7, v);
                        19
                    }
                    // LD (IX+d),r
                    0x70..=0x77 => {
                        let d = self.fetch() as i8;
                        let v = self.get(op & 7).0;
                        self.poke(index.wrapping_add(d as u16), v);
                        19
                    }
                    _ => panic!("{prefix:02x} {op:02x} at {at:04x}"),
                }
            }
            _ => panic!("opcode {op:02x} at {at:04x}"),
        };
    }

    /// The ROM's `LD_BYTES`, entered where the loader enters it.
    fn rom_load(&mut self) {
        let block = self.final_block.take().expect("the loader asked for a block the tape does not have");
        if self.model != Model::Spectrum48 {
            assert_eq!(self.port_7ffd & 0x10, 0x10, "the 48K ROM must be in to call it");
            assert_eq!(self.port_1ffd & 0x07, 0, "and not replaced by RAM or a service ROM");
        }
        assert_eq!(self.alt[A], FLAG_FINAL, "flag asked for");
        assert_eq!(self.alt[F] & (FLAG_C | FLAG_Z), FLAG_C, "LOAD, not VERIFY");
        let len = usize::from(self.pair(D));
        assert_eq!(block.len(), len + 2);
        assert_eq!(block[0], FLAG_FINAL);
        assert_eq!(block.iter().fold(0, |c, b| c ^ b), 0, "checksum");
        for (n, b) in block[1..=len].iter().enumerate() {
            self.poke(self.ix.wrapping_add(n as u16), *b);
        }
        self.r8[F] |= FLAG_C;
        self.pc = self.pop();
    }
}

// ---- running a tape --------------------------------------------------------

fn data_of(b: &Block) -> &[u8] {
    b.body.data().expect("a data block")
}

/// Load the tape's BASIC, start it where `RANDOMIZE USR` would, and run until
/// the program counter arrives where the snapshot says.
fn run(blocks: &[Block], snap: &Snapshot, model: Model) -> Machine {
    assert!(matches!(blocks[0].body, Body::Text { .. }));
    let header = decode_header(data_of(&blocks[1])).expect("a header");
    assert_eq!(header.kind, 0);
    assert_eq!(header.param1, 0, "autostart");
    let program = data_of(&blocks[2]);
    assert_eq!(program.len(), usize::from(header.length) + 2);
    assert_eq!(program.iter().fold(0, |c, b| c ^ b), 0);

    let mut m = Machine::new(model);
    for (n, b) in program[1..program.len() - 1].iter().enumerate() {
        m.poke(PROG + n as u16, *b);
    }
    m.pc = PROG + header.param2;

    let mut sink = RecordingSink::default();
    let mut end = 0;
    for b in &blocks[3..] {
        // Every block plays, the last one too: its pilot is the edge that ends
        // the last bit of the page before it.
        emit_block(&mut sink, b);
        if data_of(b)[0] == FLAG_FINAL {
            assert!(matches!(b.body, Body::Standard { .. }), "the ROM loads this one");
            m.final_block = Some(data_of(b).to_vec());
        } else {
            assert!(m.final_block.is_none(), "pages come before the last block");
        }
    }
    for (len, level) in sink.pulses {
        end += len;
        m.tape.push((end, level));
    }

    let limit = end + 3_500_000;
    let mut last = 0;
    while !(m.pc == snap.regs.pc && (0x56e0..0x5710).contains(&last)) {
        assert!(m.t < limit, "still loading after the tape ended, at {:04x}", m.pc);
        if m.pc == 0x0562 {
            m.rom_load();
            continue;
        }
        last = m.pc;
        m.step();
    }
    m
}

fn assert_restored(m: &Machine, snap: &Snapshot, border: u8) {
    let r = &snap.regs;
    let got = Registers {
        af: m.af(),
        bc: m.pair(B),
        de: m.pair(D),
        hl: m.pair(H),
        af_alt: u16::from(m.alt[A]) << 8 | u16::from(m.alt[F]),
        bc_alt: u16::from(m.alt[B]) << 8 | u16::from(m.alt[C]),
        de_alt: u16::from(m.alt[D]) << 8 | u16::from(m.alt[E]),
        hl_alt: u16::from(m.alt[H]) << 8 | u16::from(m.alt[L]),
        ix: m.ix,
        iy: m.iy,
        sp: m.sp,
        pc: m.pc,
        i: m.i,
        r: m.r,
        iff1: m.iff1,
        im: m.im,
    };
    assert_eq!(&got, r);
    assert_eq!(m.border, border);
    assert!(m.final_block.is_none(), "a block was left on the tape");
    if snap.is_128k {
        assert_eq!(m.port_7ffd, snap.port_7ffd);
        assert_eq!(m.port_fffd, snap.port_fffd);
        assert_eq!(m.ay, snap.ay);
    } else if m.model != Model::Spectrum48 {
        assert_eq!(m.port_7ffd, 0x10);
    }
    if snap.is_scorpion {
        assert_eq!(m.port_1ffd, snap.port_1ffd);
    }
    for (n, page) in snap.pages.iter().enumerate() {
        let Some(want) = page else { continue };
        let got = &m.ram[n];
        for (at, (w, g)) in want.iter().zip(got).enumerate() {
            // The last three pixel lines and the last attribute row are the loader's.
            let lost = n == 5 && ((0x14e0..0x1800).contains(&at) || (0x1ae0..0x1b00).contains(&at));
            assert!(lost || w == g, "page {n} offset {at:04x}: {g:02x}, snapshot has {w:02x}");
        }
    }
}

// ---- snapshots to try ------------------------------------------------------

fn registers() -> Registers {
    Registers {
        af: 0x12d7,
        bc: 0x3456,
        de: 0x789a,
        hl: 0xbcde,
        af_alt: 0xf00f,
        bc_alt: 0x1357,
        de_alt: 0x2468,
        hl_alt: 0x9bdf,
        ix: 0xaaa1,
        iy: 0x5c3a,
        sp: 0xfe00,
        pc: 0x9123,
        i: 0x81,
        r: 0x85,
        iff1: true,
        im: 2,
    }
}

/// Mostly empty memory with some noise in it, so the blocks stay short.
fn snapshot_48k(rng: &mut Rng) -> Snapshot {
    let mut snap = Snapshot { regs: registers(), border: 5, port_7ffd: 0x10, ..Snapshot::default() };
    for n in [5, 2, 0] {
        let mut page = vec![0u8; PAGE];
        rng.fill(&mut page[0x100..0x300]);
        rng.fill(&mut page[0x3f00..0x3f40]);
        page[0x1000..0x1100].fill(0xed);
        page[0x2000] = 0xed;
        snap.pages[n] = Some(page);
    }
    snap
}

fn options(speed: u8, border: u8) -> LoaderOptions<'static> {
    LoaderOptions { name: "test.z80", speed, border, compress_all: false, screen: None }
}

fn page_lengths(blocks: &[Block]) -> Vec<usize> {
    blocks[3..].iter().map(|b| data_of(b).len() - 2).collect()
}

#[test]
fn a_48k_snapshot_loads_at_every_speed() {
    for speed in 0..SPEED_BPS.len() as u8 {
        let mut snap = snapshot_48k(&mut Rng(speed as u32 + 1));
        snap.regs.iff1 = speed & 1 == 0;
        snap.regs.im = speed % 3;
        let blocks = snapshot_to_blocks(&snap, &options(speed, 3)).unwrap();
        // text, header, program, three pages, and what was under the loader
        assert_eq!(blocks.len(), 7, "speed {speed}");
        assert_eq!(matches!(blocks[3].body, Body::Standard { .. }), speed == 0);
        let m = run(&blocks, &snap, [Model::Spectrum48, Model::Spectrum128][usize::from(speed >= 2)]);
        assert_restored(&m, &snap, 3);
    }
}

#[test]
fn an_empty_loader_space_is_cleared_instead_of_loaded() {
    let mut snap = snapshot_48k(&mut Rng(7));
    snap.pages[2].as_mut().unwrap()[0x3e00..].fill(0);
    let blocks = snapshot_to_blocks(&snap, &options(3, 0)).unwrap();
    assert_eq!(blocks.len(), 6);
    // The tape must not stop on a data bit.
    assert!(matches!(blocks[5].body, Body::Turbo { pause: 1000, .. }));
    let m = run(&blocks, &snap, Model::Spectrum48);
    assert_restored(&m, &snap, 0);
}

#[test]
fn pages_that_do_not_pack_go_out_as_they_are() {
    let mut rng = Rng(99);
    let mut snap = snapshot_48k(&mut rng);
    // Noise does not pack at all: the page under the loader is 15872 bytes raw.
    rng.fill(snap.pages[2].as_mut().unwrap());
    // Enough noise that the packed page would load across the screen.
    rng.fill(&mut snap.pages[5].as_mut().unwrap()[0x1b00..]);
    // Packs, but ends in a lone ED the unpacker would read past.
    snap.pages[0].as_mut().unwrap()[PAGE - 1] = 0xed;
    let blocks = snapshot_to_blocks(&snap, &options(3, 1)).unwrap();
    assert_eq!(page_lengths(&blocks), [PAGE, PAGE - 512, PAGE, 512]);
    let m = run(&blocks, &snap, Model::Spectrum48);
    assert_restored(&m, &snap, 1);

    // "Fastest" packs the screen's page regardless.
    let fastest = LoaderOptions { compress_all: true, ..options(3, 1) };
    let blocks = snapshot_to_blocks(&snap, &fastest).unwrap();
    assert!(page_lengths(&blocks)[0] < PAGE);
    let m = run(&blocks, &snap, Model::Spectrum48);
    assert_restored(&m, &snap, 1);
}

#[test]
fn a_128k_snapshot_loads_with_its_paging_and_sound() {
    let mut rng = Rng(128);
    let mut snap = snapshot_48k(&mut rng);
    snap.is_128k = true;
    snap.port_7ffd = 0x13;
    snap.port_fffd = 9;
    snap.ay = std::array::from_fn(|n| 0x10 + n as u8);
    for n in [1, 3, 4, 6, 7] {
        let mut page = vec![n as u8; PAGE];
        rng.fill(&mut page[0x2000..0x2080]);
        snap.pages[n] = Some(page);
    }
    // Nothing under the loader: the block is on the tape all the same, because
    // the 128K loader waits for it.
    snap.pages[2].as_mut().unwrap()[0x3e00..].fill(0);
    let blocks = snapshot_to_blocks(&snap, &options(3, 6)).unwrap();
    assert_eq!(blocks.len(), 3 + 8 + 1);
    // The fullest the 512 byte loader gets: eight pages and a loading screen.
    let screen = vec![0u8; 6912];
    let with_screen = LoaderOptions { screen: Some(&screen), ..options(3, 6) };
    let blocks = snapshot_to_blocks(&snap, &with_screen).unwrap();
    assert_eq!(blocks.len(), 3 + 9 + 1);
    let m = run(&blocks, &snap, Model::Spectrum128);
    assert_restored(&m, &snap, 6);
    let m = run(&blocks, &snap, Model::Spectrum128);
    assert_restored(&m, &snap, 6);
}

#[test]
fn a_scorpion_snapshot_loads_all_sixteen_pages() {
    let mut rng = Rng(256);
    let mut snap = snapshot_48k(&mut rng);
    snap.is_128k = true;
    snap.is_scorpion = true;
    snap.port_7ffd = 0x14;
    snap.port_1ffd = 0x10; // page 12 at the top
    snap.ay = std::array::from_fn(|n| 0x20 + n as u8);
    for n in (1..16).filter(|n| ![2, 5].contains(n)) {
        let mut page = vec![n as u8; PAGE];
        rng.fill(&mut page[0x1000..0x1040]);
        snap.pages[n] = Some(page);
    }
    // What sits under this longer loader is three quarters of a K.
    rng.fill(&mut snap.pages[2].as_mut().unwrap()[0x3d00..]);
    let blocks = snapshot_to_blocks(&snap, &options(3, 4)).unwrap();
    assert_eq!(blocks.len(), 3 + 16 + 1);
    assert_eq!(blocks[0].body, Body::Text { text: "Scorpion 256K snapshot - 6000 bps".to_string() });
    assert_eq!(*page_lengths(&blocks).last().unwrap(), 768);
    let m = run(&blocks, &snap, Model::Scorpion);
    assert_restored(&m, &snap, 4);
}

#[test]
fn a_loading_screen_goes_first_and_its_page_last() {
    let mut rng = Rng(5);
    let snap = snapshot_48k(&mut rng);
    let mut screen = vec![0u8; 6912];
    rng.fill(&mut screen[..400]);
    screen[6144..].fill(0x38);
    let opts = LoaderOptions { screen: Some(&screen), ..options(3, 2) };
    let blocks = snapshot_to_blocks(&snap, &opts).unwrap();
    assert_eq!(blocks.len(), 8);
    let m = run(&blocks, &snap, Model::Spectrum48);
    assert_restored(&m, &snap, 2);

    // A screen that ends in a lone ED cannot be unpacked on its own.
    screen[6911] = 0xed;
    let opts = LoaderOptions { screen: Some(&screen), ..options(3, 2) };
    let blocks = snapshot_to_blocks(&snap, &opts).unwrap();
    let m = run(&blocks, &snap, Model::Spectrum48);
    assert_restored(&m, &snap, 2);

    let short = LoaderOptions { screen: Some(&screen[..100]), ..options(3, 2) };
    assert!(snapshot_to_blocks(&snap, &short).is_err());
}

#[test]
fn the_basic_loader_lists() {
    let snap = snapshot_48k(&mut Rng(3));
    let opts = LoaderOptions { name: "/games/Jet Set Willy.sna", ..options(2, 0) };
    let blocks = snapshot_to_blocks(&snap, &opts).unwrap();
    assert_eq!(blocks[0].body, Body::Text { text: "48K snapshot - 3000 bps".to_string() });
    let header = decode_header(data_of(&blocks[1])).unwrap();
    assert_eq!(header.name, "Jet Set Wi");
    let program = &data_of(&blocks[2])[1..];
    let lines = list_basic(program, 0, usize::from(header.param2), BasicOptions::default());
    assert_eq!(lines.len(), 2);
    let text: String = lines.iter().map(|l| format!("{l:?}")).collect();
    assert!(text.contains("RANDOMIZE") && text.contains("USR"), "{text}");
}

// ---- reading the files -----------------------------------------------------

fn z80_header(snap: &Snapshot, pc: u16, flags: u8) -> Vec<u8> {
    let r = &snap.regs;
    let mut h = vec![(r.af >> 8) as u8, r.af as u8];
    for w in [r.bc, r.hl, pc, r.sp] {
        h.extend_from_slice(&w.to_le_bytes());
    }
    h.extend_from_slice(&[r.i, r.r & 0x7f, flags | r.r >> 7 | snap.border << 1]);
    for w in [r.de, r.bc_alt, r.de_alt, r.hl_alt] {
        h.extend_from_slice(&w.to_le_bytes());
    }
    h.extend_from_slice(&[(r.af_alt >> 8) as u8, r.af_alt as u8]);
    for w in [r.iy, r.ix] {
        h.extend_from_slice(&w.to_le_bytes());
    }
    h.extend_from_slice(&[u8::from(r.iff1), u8::from(r.iff1), r.im]);
    assert_eq!(h.len(), 30);
    h
}

fn assert_same(a: &Snapshot, b: &Snapshot) {
    assert_eq!(a.regs, b.regs);
    assert!(a.pages == b.pages, "memory differs");
    assert_eq!(
        (a.is_128k, a.is_scorpion, a.border, a.port_7ffd, a.port_1ffd, a.port_fffd, a.ay),
        (b.is_128k, b.is_scorpion, b.border, b.port_7ffd, b.port_1ffd, b.port_fffd, b.ay)
    );
}

#[test]
fn z80_version_1_reads_packed_and_not() {
    let snap = snapshot_48k(&mut Rng(11));
    let ram: Vec<u8> = [5, 2, 0].iter().flat_map(|n| snap.pages[*n].clone().unwrap()).collect();
    let mut plain = z80_header(&snap, snap.regs.pc, 0);
    plain.extend_from_slice(&ram);
    assert_same(&parse_snapshot(&plain, SnapshotKind::Z80).unwrap(), &snap);
    let mut packed = z80_header(&snap, snap.regs.pc, 0x20);
    packed.extend_from_slice(&pack(&ram));
    packed.extend_from_slice(&[0, 0xed, 0xed, 0]);
    assert_same(&parse_snapshot(&packed, SnapshotKind::Z80).unwrap(), &snap);
    assert!(parse_snapshot(&packed[..packed.len() - 40], SnapshotKind::Z80).is_err());
    assert!(parse_snapshot(&[0; 10], SnapshotKind::Z80).is_err());
}

#[test]
fn z80_versions_2_and_3_read_48k_and_128k() {
    // Hardware 9 is the Pentagon: 128K memory, and read as that.
    for (extra, hardware_48, hardware_128) in [(23usize, 0u8, 3u8), (54, 0, 4), (55, 1, 12), (54, 3, 9)] {
        for is_128k in [false, true] {
            let mut snap = snapshot_48k(&mut Rng(12));
            snap.is_128k = is_128k;
            if is_128k {
                snap.port_7ffd = 0x17;
                snap.port_fffd = 3;
                snap.ay = std::array::from_fn(|n| n as u8 * 3);
                for n in [1, 3, 4, 6, 7] {
                    snap.pages[n] = Some(vec![n as u8; PAGE]);
                }
            }
            let mut file = z80_header(&snap, 0, 0);
            file.extend_from_slice(&(extra as u16).to_le_bytes());
            let mut more = vec![0u8; extra];
            more[..2].copy_from_slice(&snap.regs.pc.to_le_bytes());
            more[2] = if is_128k { hardware_128 } else { hardware_48 };
            more[3] = snap.port_7ffd;
            more[6] = snap.port_fffd;
            more[7..23].copy_from_slice(&snap.ay);
            file.extend_from_slice(&more);
            for (n, page) in snap.pages.iter().enumerate() {
                let Some(page) = page else { continue };
                let number =
                    if is_128k { n as u8 + 3 } else { [5, 0, 4, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0][n] };
                // One page stored unpacked, as version 3.05 allows.
                if n == 2 {
                    file.extend_from_slice(&[0xff, 0xff, number]);
                    file.extend_from_slice(page);
                } else {
                    let packed = pack(page);
                    file.extend_from_slice(&(packed.len() as u16).to_le_bytes());
                    file.push(number);
                    file.extend_from_slice(&packed);
                }
            }
            assert_same(&parse_snapshot(&file, SnapshotKind::Z80).unwrap(), &snap);
            assert!(parse_snapshot(&file[..file.len() - 1], SnapshotKind::Z80).is_err());
        }
    }
}

fn sna_header(snap: &Snapshot, sp: u16) -> Vec<u8> {
    let r = &snap.regs;
    let mut h = vec![r.i];
    for w in [r.hl_alt, r.de_alt, r.bc_alt, r.af_alt, r.hl, r.de, r.bc, r.iy, r.ix] {
        h.extend_from_slice(&w.to_le_bytes());
    }
    h.extend_from_slice(&[if r.iff1 { 4 } else { 0 }, r.r]);
    h.extend_from_slice(&r.af.to_le_bytes());
    h.extend_from_slice(&sp.to_le_bytes());
    h.extend_from_slice(&[r.im, snap.border]);
    assert_eq!(h.len(), 27);
    h
}

#[test]
fn z80_reads_a_scorpion() {
    let mut snap = snapshot_48k(&mut Rng(14));
    snap.is_128k = true;
    snap.is_scorpion = true;
    snap.port_7ffd = 0x11;
    snap.port_1ffd = 0x10;
    for n in (1..16).filter(|n| ![2, 5].contains(n)) {
        snap.pages[n] = Some(vec![n as u8 + 0x70; PAGE]);
    }
    let mut file = z80_header(&snap, 0, 0);
    file.extend_from_slice(&55u16.to_le_bytes());
    let mut more = vec![0u8; 55];
    more[..2].copy_from_slice(&snap.regs.pc.to_le_bytes());
    more[2] = 10;
    more[3] = snap.port_7ffd;
    more[54] = snap.port_1ffd;
    file.extend_from_slice(&more);
    for (n, page) in snap.pages.iter().enumerate() {
        let packed = pack(page.as_ref().unwrap());
        file.extend_from_slice(&(packed.len() as u16).to_le_bytes());
        file.push(n as u8 + 3);
        file.extend_from_slice(&packed);
    }
    assert_same(&parse_snapshot(&file, SnapshotKind::Z80).unwrap(), &snap);
}

#[test]
fn sna_reads_48k_and_128k() {
    let mut snap = snapshot_48k(&mut Rng(13));
    // 48K: the program counter is on the stack, which is in page 0 here.
    let sp = snap.regs.sp - 2;
    let at = usize::from(sp) - 0xc000;
    snap.pages[0].as_mut().unwrap()[at..at + 2].copy_from_slice(&snap.regs.pc.to_le_bytes());
    let mut file = sna_header(&snap, sp);
    for n in [5, 2, 0] {
        file.extend_from_slice(snap.pages[n].as_ref().unwrap());
    }
    assert_same(&parse_snapshot(&file, SnapshotKind::Sna).unwrap(), &snap);
    assert!(parse_snapshot(&file[..file.len() - 1], SnapshotKind::Sna).is_err());
    let mut in_rom = file.clone();
    in_rom[23..25].copy_from_slice(&0x1000u16.to_le_bytes());
    assert!(parse_snapshot(&in_rom, SnapshotKind::Sna).is_err());

    // 128K, with page 3 at the top and then with page 2 there, which is stored twice.
    snap.is_128k = true;
    for n in [1, 3, 4, 6, 7] {
        snap.pages[n] = Some(vec![n as u8 + 0x40; PAGE]);
    }
    for top in [3usize, 2] {
        snap.port_7ffd = 0x10 | top as u8;
        let mut file = sna_header(&snap, snap.regs.sp);
        for n in [5, 2, top] {
            file.extend_from_slice(snap.pages[n].as_ref().unwrap());
        }
        file.extend_from_slice(&snap.regs.pc.to_le_bytes());
        file.extend_from_slice(&[snap.port_7ffd, 0]);
        for n in (0..8).filter(|n| ![5, 2, top].contains(n)) {
            file.extend_from_slice(snap.pages[n].as_ref().unwrap());
        }
        assert_eq!(file.len(), if top == 2 { 147_487 } else { 131_103 });
        assert_same(&parse_snapshot(&file, SnapshotKind::Sna).unwrap(), &snap);
    }
}

#[test]
fn kinds_go_by_extension() {
    assert_eq!(SnapshotKind::from_name("a.Z80"), Some(SnapshotKind::Z80));
    assert_eq!(SnapshotKind::from_name("dir.z80/b.sna"), Some(SnapshotKind::Sna));
    assert_eq!(SnapshotKind::from_name("c.tzx"), None);
}
