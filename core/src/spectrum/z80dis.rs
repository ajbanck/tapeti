//! Table-free Z80 disassembler following the x/y/z/p/q decoding scheme, the
//! port of `src/spectrum/z80dis.ts`. Handles CB, ED, DD, FD, DDCB and FDCB
//! prefixes including undocumented forms.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisLine {
    pub addr: u32,
    pub bytes: Vec<u8>,
    pub text: String,
    /// Absolute target of a jump/call, if any (for ROM labels).
    pub target: Option<u32>,
}

const R: [&str; 8] = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
const RP: [&str; 4] = ["BC", "DE", "HL", "SP"];
const RP2: [&str; 4] = ["BC", "DE", "HL", "AF"];
const CC: [&str; 8] = ["NZ", "Z", "NC", "C", "PO", "PE", "P", "M"];
const ALU: [&str; 8] = ["ADD A,", "ADC A,", "SUB ", "SBC A,", "AND ", "XOR ", "OR ", "CP "];
const ROT: [&str; 8] = ["RLC", "RRC", "RL", "RR", "SLA", "SRA", "SLL", "SRL"];
const IM: [&str; 8] = ["0", "0/1", "1", "2", "0", "0/1", "1", "2"];
const BLI: [[&str; 4]; 4] = [
    ["LDI", "CPI", "INI", "OUTI"],
    ["LDD", "CPD", "IND", "OUTD"],
    ["LDIR", "CPIR", "INIR", "OTIR"],
    ["LDDR", "CPDR", "INDR", "OTDR"],
];

/// The ROM entry points the listing annotates.
pub fn rom_label(addr: u32) -> Option<&'static str> {
    Some(match addr {
        0x0000 => "START",
        0x0008 => "ERROR-1",
        0x0010 => "PRINT-A",
        0x0018 => "GET-CHAR",
        0x0020 => "NEXT-CHAR",
        0x0028 => "FP-CALC",
        0x0030 => "BC-SPACES",
        0x0038 => "MASK-INT",
        0x0053 => "ERROR-2",
        0x0066 => "RESET",
        0x0074 => "CH-ADD+1",
        0x007d => "SKIP-OVER",
        0x0095 => "TOKENS",
        0x028e => "KEY-SCAN",
        0x02bf => "KEYBOARD",
        0x031e => "K-TEST",
        0x0333 => "K-DECODE",
        0x03b5 => "BEEPER",
        0x03f8 => "BEEP",
        0x04c2 => "SA-BYTES",
        0x04d0 => "SA-FLAG",
        0x053f => "SA/LD-RET",
        0x0556 => "LD-BYTES",
        0x056b => "LD-START",
        0x05e3 => "LD-EDGE-2",
        0x05e7 => "LD-EDGE-1",
        0x0605 => "SAVE-ETC",
        0x0802 => "LD-BLOCK",
        0x0808 => "LD-CONTRL",
        0x08b6 => "ME-CONTRL",
        0x0970 => "SA-CONTRL",
        0x09a1 => "SA-ALL",
        0x0a11 => "PO-TAB",
        0x0adc => "PO-STORE",
        0x0b03 => "PO-FETCH",
        0x0b24 => "PO-ANY",
        0x0b7f => "PR-ALL",
        0x0bdb => "PO-ATTR",
        0x0c0a => "PO-MSG",
        0x0c3b => "PO-SAVE",
        0x0c55 => "PO-SCR",
        0x0d4d => "TEMPS",
        0x0d6b => "CLS",
        0x0daf => "CL-ALL",
        0x0dd9 => "CL-SET",
        0x0dfe => "CL-SC-ALL",
        0x0e44 => "CL-LINE",
        0x0e9b => "CL-ADDR",
        0x0eac => "COPY",
        0x0edf => "CLEAR-PRB",
        0x0f2c => "EDITOR",
        0x0f81 => "ADD-CHAR",
        0x111d => "ED-COPY",
        0x11cb => "START/NEW",
        0x1219 => "RAM-CHECK",
        0x12a2 => "MAIN-EXEC",
        0x12cf => "MAIN-1",
        0x1303 => "MAIN-4",
        0x1391 => "REPORT-G",
        0x155d => "MAIN-ADD",
        0x15c4 => "REPORT-J",
        0x15d4 => "WAIT-KEY",
        0x15e6 => "INPUT-AD",
        0x15ef => "OUT-CODE",
        0x15f2 => "PRINT-A-2",
        0x1601 => "CHAN-OPEN",
        0x1615 => "CHAN-FLAG",
        0x1655 => "MAKE-ROOM",
        0x1664 => "POINTERS",
        0x16b0 => "SET-MIN",
        0x16e5 => "CLOSE",
        0x1736 => "OPEN",
        0x1795 => "AUTO-LIST",
        0x17f5 => "LIST",
        0x17f9 => "LLIST",
        0x1855 => "OUT-LINE",
        0x196e => "LINE-ADDR",
        0x1980 => "CP-LINES",
        0x19b8 => "NEXT-ONE",
        0x19e8 => "RECLAIM-2",
        0x1a1b => "OUT-NUM-1",
        0x1b17 => "LINE-SCAN",
        0x1b8a => "LINE-RUN",
        0x1bb2 => "REM",
        0x1c79 => "CLASS-06",
        0x1cdb => "IF",
        0x1d03 => "FOR",
        0x1e42 => "RESTORE",
        0x1e4f => "RANDOMIZE",
        0x1e5f => "CONTINUE",
        0x1e67 => "GO-TO",
        0x1e7a => "OUT",
        0x1e80 => "POKE",
        0x1e94 => "FIND-INT1",
        0x1e99 => "FIND-INT2",
        0x1ea1 => "RUN",
        0x1eac => "CLEAR",
        0x1eed => "GO-SUB",
        0x1f05 => "TEST-ROOM",
        0x1f23 => "RETURN",
        0x1f3a => "PAUSE",
        0x1f54 => "BREAK-KEY",
        0x1fc3 => "LPRINT",
        0x1fcd => "PRINT",
        0x203c => "PR-STRING",
        0x2070 => "INPUT",
        0x2294 => "BORDER",
        0x22aa => "PIXEL-ADD",
        0x22cb => "POINT-SUB",
        0x22dc => "PLOT",
        0x2307 => "STK-TO-BC",
        0x2314 => "STK-TO-A",
        0x2320 => "CIRCLE",
        0x2382 => "DRAW",
        0x24b7 => "DRAW-LINE",
        0x24fb => "SCANNING",
        0x2530 => "SYNTAX-Z",
        0x2634 => "STK-CONST",
        0x2c88 => "ALPHANUM",
        0x2d1b => "NUMERIC",
        0x2d28 => "STACK-A",
        0x2d2b => "STACK-BC",
        0x2d3b => "INT-TO-FP",
        0x2da2 => "FP-TO-BC",
        0x2dd5 => "FP-TO-A",
        0x2de3 => "PRINT-FP",
        0x335e => "CALCULATE",
        0x33c0 => "STK-DATA",
        0x3d00 => "CHAR-SET",
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug)]
pub struct DisOptions {
    pub hex: bool,
    pub rom_labels: bool,
}

impl Default for DisOptions {
    fn default() -> Self {
        DisOptions { hex: true, rom_labels: true }
    }
}

fn h8(v: u32, hex: bool) -> String {
    if hex {
        format!("0x{:0>2X}", v)
    } else {
        v.to_string()
    }
}

fn h16(v: u32, hex: bool) -> String {
    if hex {
        format!("0x{:0>4X}", v)
    } else {
        v.to_string()
    }
}

fn disp(d: i32, hex: bool) -> String {
    let sign = if d < 0 { '-' } else { '+' };
    format!("{sign}{}", h8(d.unsigned_abs(), hex))
}

/// Reads bytes for one instruction, remembering them for the listing.
struct Cursor<'a> {
    data: &'a [u8],
    p: usize,
    bytes: Vec<u8>,
}

impl Cursor<'_> {
    fn rd(&mut self) -> u8 {
        let v = self.data.get(self.p).copied().unwrap_or(0);
        self.bytes.push(v);
        self.p += 1;
        v
    }

    fn rd16(&mut self) -> u32 {
        let lo = u32::from(self.rd());
        lo | u32::from(self.rd()) << 8
    }

    fn read_disp(&mut self) -> i32 {
        let d = i32::from(self.rd());
        if d >= 128 {
            d - 256
        } else {
            d
        }
    }
}

/// Disassemble `count` instructions from `data` starting at byte `offset`, with
/// address base `base`.
pub fn disassemble(data: &[u8], offset: usize, base: u32, count: usize, opts: DisOptions) -> Vec<DisLine> {
    let hex = opts.hex;
    let mut out: Vec<DisLine> = Vec::new();
    let mut p = offset;
    while out.len() < count && p < data.len() {
        let start = p;
        let mut c = Cursor { data, p, bytes: Vec::new() };
        let addr = base.wrapping_add((start - offset) as u32);
        let mut target: Option<u32> = None;
        #[allow(unused_assignments)] // every branch below sets it
        let mut text = String::new();

        let mut op = c.rd();
        let mut ix: Option<&'static str> = None;
        while op == 0xdd || op == 0xfd {
            ix = Some(if op == 0xdd { "IX" } else { "IY" });
            if c.p >= data.len() {
                break;
            }
            op = c.rd();
        }
        let hl = ix.unwrap_or("HL");
        let hh = ix.map(|i| format!("{i}H")).unwrap_or_else(|| "H".to_string());
        let ll = ix.map(|i| format!("{i}L")).unwrap_or_else(|| "L".to_string());
        let (x, y, z) = (op >> 6, (op >> 3) & 7, op & 7);
        let (p_, q) = (y >> 1, y & 1);
        let rname = |i: u8, d: Option<i32>| -> String {
            if i == 6 {
                return match ix {
                    Some(ix) => format!("({ix}{})", disp(d.unwrap_or(0), hex)),
                    None => "(HL)".to_string(),
                };
            }
            if ix.is_some() && i == 4 {
                return hh.clone();
            }
            if ix.is_some() && i == 5 {
                return ll.clone();
            }
            R[i as usize].to_string()
        };
        let rp = |i: u8| -> String {
            if i == 2 {
                hl.to_string()
            } else {
                RP[i as usize].to_string()
            }
        };
        let rp2 = |i: u8| -> String {
            if i == 2 {
                hl.to_string()
            } else {
                RP2[i as usize].to_string()
            }
        };

        if op == 0xcb {
            let d = if ix.is_some() { c.read_disp() } else { 0 };
            let o = c.rd();
            let (cx, cy, cz) = (o >> 6, (o >> 3) & 7, o & 7);
            let mem = match ix {
                Some(ix) => format!("({ix}{})", disp(d, hex)),
                None => R[cz as usize].to_string(),
            };
            if ix.is_some() && cz != 6 {
                // undocumented: result copied to register
                text = match cx {
                    0 => format!("{} {mem},{}", ROT[cy as usize], R[cz as usize]),
                    1 => format!("BIT {cy},{mem}"),
                    _ => format!("{} {cy},{mem},{}", if cx == 2 { "RES" } else { "SET" }, R[cz as usize]),
                };
            } else if cx == 0 {
                text = format!("{} {mem}", ROT[cy as usize]);
            } else {
                text = format!("{} {cy},{mem}", ["", "BIT", "RES", "SET"][cx as usize]);
            }
        } else if op == 0xed {
            let o = c.rd();
            let (ex, ey, ez) = (o >> 6, (o >> 3) & 7, o & 7);
            let (ep, eq) = (ey >> 1, ey & 1);
            if ex == 1 {
                text = match ez {
                    0 => {
                        if ey == 6 {
                            "IN (C)".to_string()
                        } else {
                            format!("IN {},(C)", R[ey as usize])
                        }
                    }
                    1 => {
                        if ey == 6 {
                            "OUT (C),0".to_string()
                        } else {
                            format!("OUT (C),{}", R[ey as usize])
                        }
                    }
                    2 => format!("{} HL,{}", if eq != 0 { "ADC" } else { "SBC" }, RP[ep as usize]),
                    3 => {
                        let nn = h16(c.rd16(), hex);
                        if eq != 0 {
                            format!("LD {},({nn})", RP[ep as usize])
                        } else {
                            format!("LD ({nn}),{}", RP[ep as usize])
                        }
                    }
                    4 => "NEG".to_string(),
                    5 => if ey == 1 { "RETI" } else { "RETN" }.to_string(),
                    6 => format!("IM {}", IM[ey as usize]),
                    _ => ["LD I,A", "LD R,A", "LD A,I", "LD A,R", "RRD", "RLD", "NOP", "NOP"][ey as usize]
                        .to_string(),
                };
            } else if ex == 2 && ez <= 3 && ey >= 4 {
                text = BLI[(ey - 4) as usize][ez as usize].to_string();
            } else {
                text = "NOP*".to_string();
            }
        } else {
            match x {
                0 => match z {
                    0 => {
                        text = match y {
                            0 => "NOP".to_string(),
                            1 => "EX AF,AF'".to_string(),
                            2 => {
                                let t = rel(&mut c, addr, hex, &mut target);
                                format!("DJNZ {t}")
                            }
                            3 => {
                                let t = rel(&mut c, addr, hex, &mut target);
                                format!("JR {t}")
                            }
                            _ => {
                                let t = rel(&mut c, addr, hex, &mut target);
                                format!("JR {},{t}", CC[(y - 4) as usize])
                            }
                        };
                    }
                    1 => {
                        text = if q != 0 {
                            format!("ADD {hl},{}", rp(p_))
                        } else {
                            format!("LD {},{}", rp(p_), h16(c.rd16(), hex))
                        };
                    }
                    2 => {
                        text = match (q, p_) {
                            (0, 0) => "LD (BC),A".to_string(),
                            (0, 1) => "LD (DE),A".to_string(),
                            (0, 2) => format!("LD ({}),{hl}", h16(c.rd16(), hex)),
                            (0, _) => format!("LD ({}),A", h16(c.rd16(), hex)),
                            (_, 0) => "LD A,(BC)".to_string(),
                            (_, 1) => "LD A,(DE)".to_string(),
                            (_, 2) => format!("LD {hl},({})", h16(c.rd16(), hex)),
                            (_, _) => format!("LD A,({})", h16(c.rd16(), hex)),
                        };
                    }
                    3 => text = format!("{} {}", if q != 0 { "DEC" } else { "INC" }, rp(p_)),
                    4 => {
                        let d = if y == 6 && ix.is_some() { Some(c.read_disp()) } else { None };
                        text = format!("INC {}", rname(y, d));
                    }
                    5 => {
                        let d = if y == 6 && ix.is_some() { Some(c.read_disp()) } else { None };
                        text = format!("DEC {}", rname(y, d));
                    }
                    6 => {
                        let d = if y == 6 && ix.is_some() { Some(c.read_disp()) } else { None };
                        let name = rname(y, d);
                        text = format!("LD {name},{}", h8(u32::from(c.rd()), hex));
                    }
                    _ => {
                        text =
                            ["RLCA", "RRCA", "RLA", "RRA", "DAA", "CPL", "SCF", "CCF"][y as usize].to_string()
                    }
                },
                1 => {
                    if z == 6 && y == 6 {
                        text = "HALT".to_string();
                    } else if ix.is_some() && (y == 6 || z == 6) {
                        let d = c.read_disp();
                        let ix = ix.unwrap();
                        // when (IX+d) is involved, H/L stay plain H/L
                        let dst = if y == 6 {
                            format!("({ix}{})", disp(d, hex))
                        } else {
                            R[y as usize].to_string()
                        };
                        let src = if z == 6 {
                            format!("({ix}{})", disp(d, hex))
                        } else {
                            R[z as usize].to_string()
                        };
                        text = format!("LD {dst},{src}");
                    } else {
                        text = format!("LD {},{}", rname(y, None), rname(z, None));
                    }
                }
                2 => {
                    let d = if z == 6 && ix.is_some() { Some(c.read_disp()) } else { None };
                    text = format!("{}{}", ALU[y as usize], rname(z, d));
                }
                _ => match z {
                    0 => text = format!("RET {}", CC[y as usize]),
                    1 => {
                        text = if q != 0 {
                            match p_ {
                                0 => "RET".to_string(),
                                1 => "EXX".to_string(),
                                2 => format!("JP ({hl})"),
                                _ => format!("LD SP,{hl}"),
                            }
                        } else {
                            format!("POP {}", rp2(p_))
                        };
                    }
                    2 => {
                        let nn = c.rd16();
                        target = Some(nn);
                        text = format!("JP {},{}", CC[y as usize], h16(nn, hex));
                    }
                    3 => {
                        text = match y {
                            0 => {
                                let nn = c.rd16();
                                target = Some(nn);
                                format!("JP {}", h16(nn, hex))
                            }
                            1 => String::new(), // the CB prefix, handled above
                            2 => format!("OUT ({}),A", h8(u32::from(c.rd()), hex)),
                            3 => format!("IN A,({})", h8(u32::from(c.rd()), hex)),
                            4 => format!("EX (SP),{hl}"),
                            5 => "EX DE,HL".to_string(),
                            6 => "DI".to_string(),
                            _ => "EI".to_string(),
                        };
                    }
                    4 => {
                        let nn = c.rd16();
                        target = Some(nn);
                        text = format!("CALL {},{}", CC[y as usize], h16(nn, hex));
                    }
                    5 => {
                        text = if q != 0 {
                            if p_ == 0 {
                                let nn = c.rd16();
                                target = Some(nn);
                                format!("CALL {}", h16(nn, hex))
                            } else {
                                "NOP*".to_string()
                            }
                        } else {
                            format!("PUSH {}", rp2(p_))
                        };
                    }
                    6 => text = format!("{}{}", ALU[y as usize], h8(u32::from(c.rd()), hex)),
                    _ => {
                        target = Some(u32::from(y) * 8);
                        text = format!("RST {}", h8(u32::from(y) * 8, hex));
                    }
                },
            }
        }

        if opts.rom_labels {
            match target.and_then(rom_label) {
                Some(label) => text.push_str(&format!("  ; {label}")),
                None => {
                    // LD HL,nnnn / LD DE,nnnn etc. with a ROM address. The
                    // TypeScript looks for `,$?dddd` at the end, which its own
                    // `0x` spelling never matches, so this only fires in decimal.
                    if let Some(v) = trailing_address(&text, hex) {
                        if let Some(label) = rom_label(v) {
                            text.push_str(&format!("  ; {label}"));
                        }
                    }
                }
            }
        }
        p = c.p;
        out.push(DisLine { addr, bytes: c.bytes, text, target });
    }
    out
}

/// A relative jump target, and the text for it.
fn rel(c: &mut Cursor, addr: u32, hex: bool, target: &mut Option<u32>) -> String {
    let d = c.rd();
    let delta = if d >= 128 { i32::from(d) - 256 } else { i32::from(d) };
    let t = ((addr as i64 + c.bytes.len() as i64 + i64::from(delta)) & 0xffff) as u32;
    *target = Some(t);
    h16(t, hex)
}

/// `/,\$?([0-9A-F]{4})$/` from the TypeScript, ported as it stands.
fn trailing_address(text: &str, hex: bool) -> Option<u32> {
    let b = text.as_bytes();
    if b.len() < 5 {
        return None;
    }
    let digits = &text[b.len() - 4..];
    if !digits.bytes().all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c)) {
        return None;
    }
    let before = b[b.len() - 5];
    let ok = before == b',' || (before == b'$' && b.len() >= 6 && b[b.len() - 6] == b',');
    if !ok {
        return None;
    }
    u32::from_str_radix(digits, if hex { 16 } else { 10 }).ok()
}
