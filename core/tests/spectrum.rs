//! Tests for the Spectrum side: the character set, the screen, the BASIC lister
//! and the Z80 disassembler. `test/core.test.ts` holds the same to recorded answers
//! through the wasm build; what is here is what the core owes on its own.

use tapeti_core::spectrum::basic::{
    basic_to_text, decode_number, format_number, list_basic, list_variables, BasicOptions,
};
use tapeti_core::spectrum::charset::{char_table, dump_char, token_name, zx_char};
use tapeti_core::spectrum::screen::{has_flash, render_screen, ScreenOptions, DEFAULT_ATTR};
use tapeti_core::spectrum::z80dis::{disassemble, DisOptions};

#[test]
fn maps_the_character_set() {
    assert_eq!(zx_char(b'A', true), "A");
    assert_eq!(zx_char(0x60, true), "£");
    assert_eq!(zx_char(0x7f, true), "©");
    assert_eq!(zx_char(0x80, true), " "); // the empty block graphic
    assert_eq!(zx_char(0x8f, true), "█");
    assert_eq!(zx_char(0x90, true), "①"); // UDGs show as circled numbers
    assert_eq!(zx_char(0xf5, true), "PRINT");
    assert_eq!(zx_char(0xf5, false), ".");
    assert_eq!(dump_char(0xf5), "·");
    assert_eq!(token_name(0xa5, false), Some("RND"));
    assert_eq!(token_name(0xa3, false), None);
    assert_eq!(token_name(0xa3, true), Some("SPECTRUM"));
    // The tables the app indexes hold all 256, and agree with the functions.
    for (kind, f) in [(0u32, zx_char(0xf5, true)), (1, zx_char(0xf5, false)), (2, dump_char(0xf5))] {
        assert_eq!(char_table(kind).len(), 256);
        assert_eq!(char_table(kind)[0xf5], f);
    }
}

#[test]
fn renders_a_screen() {
    // One set pixel in the top left, black ink on white paper.
    let mut data = vec![0u8; 6912];
    data[0] = 0x80;
    for a in data.iter_mut().skip(6144) {
        *a = DEFAULT_ATTR;
    }
    let px = render_screen(&data, 0, ScreenOptions::default());
    assert_eq!(px.len(), 256 * 192 * 4);
    assert_eq!(&px[0..4], &[0, 0, 0, 255]); // ink
    assert_eq!(&px[4..8], &[0xd7, 0xd7, 0xd7, 255]); // paper
    data[6144] = 0b0100_0010; // bright red ink on black paper
    let colour = render_screen(&data, 0, ScreenOptions::default());
    assert_eq!(&colour[0..4], &[0xff, 0, 0, 255]);
    // Hiding the attributes forces black on white whatever the attribute says.
    let plain = render_screen(&data, 0, ScreenOptions { hide_attributes: true, flash_phase: false });
    assert_eq!(&plain[0..4], &[0, 0, 0, 255]);

    // FLASH swaps ink and paper on the second phase.
    data[6144] = 0x80 | 0b0000_0111; // flashing white ink on black paper
    let a = render_screen(&data, 0, ScreenOptions { hide_attributes: false, flash_phase: false });
    let b = render_screen(&data, 0, ScreenOptions { hide_attributes: false, flash_phase: true });
    assert_eq!(&a[0..4], &[0xd7, 0xd7, 0xd7, 255]);
    assert_eq!(&b[0..4], &[0, 0, 0, 255]);
    assert!(has_flash(&data, 0));
    assert!(!has_flash(&[0u8; 6912], 0));

    // A block that stops short still renders: missing pixels blank, missing
    // attributes black on white.
    let short = render_screen(&[0xff; 32], 0, ScreenOptions::default());
    assert_eq!(&short[0..4], &[0, 0, 0, 255]);
    // DEFAULT_ATTR is black on white without BRIGHT, so the paper is 0xd7.
    assert_eq!(&short[(191 * 256 * 4)..(191 * 256 * 4 + 4)], &[0xd7, 0xd7, 0xd7, 255]);
}

#[test]
fn decodes_and_formats_numbers() {
    let n = |b: [u8; 5]| decode_number(&b, 0);
    assert_eq!(n([0x00, 0x00, 0x01, 0x00, 0x00]), 1.0);
    assert_eq!(n([0x00, 0xff, 0xff, 0xff, 0x00]), -1.0);
    assert_eq!(n([0x00, 0x00, 0x39, 0x30, 0x00]), 12345.0);
    assert_eq!(n([0x81, 0x00, 0x00, 0x00, 0x00]), 1.0);
    assert_eq!(n([0x80, 0x80, 0x00, 0x00, 0x00]), -0.5);
    assert!(decode_number(&[0, 0, 0], 0).is_nan());

    // The listing's spelling, JavaScript's quirks included.
    assert_eq!(format_number(f64::NAN), "?");
    assert_eq!(format_number(0.0), "0");
    assert_eq!(format_number(-42.0), "-42");
    assert_eq!(format_number(0.5), "0.5");
    assert_eq!(format_number(1.0 / 3.0), "0.33333333");
    assert_eq!(format_number(std::f64::consts::PI), "3.1415927");
    assert_eq!(format_number(1e15), "1.0000000e+15");
    assert_eq!(format_number(1e-7), "1.0000000e-7");
    assert_eq!(format_number(f64::INFINITY), "Infinity");
    // A tie rounds up, the way JavaScript's toPrecision does.
    assert_eq!(format_number(12345678.5), "12345679");
}

#[test]
fn lists_a_basic_program() {
    // 10 PRINT "hi"
    let prog = [0x00, 0x0a, 0x06, 0x00, 0xf5, 0x22, 0x68, 0x69, 0x22, 0x0d];
    let opts = BasicOptions::default();
    let lines = list_basic(&prog, 0, prog.len(), opts);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].number, 10);
    assert_eq!(basic_to_text(&lines, opts), "  10 PRINT \"hi\"");

    // A hidden 5-byte number that disagrees with the text is shown anyway.
    let altered = [0x00, 0x14, 0x0b, 0x00, 0xf1, 0x61, 0x3d, 0x31, 0x0e, 0x00, 0x00, 0x63, 0x00, 0x00, 0x0d];
    let lines = list_basic(&altered, 0, altered.len(), opts);
    assert!(basic_to_text(&lines, opts).contains("{99}"), "{}", basic_to_text(&lines, opts));

    // The same program with show_numbers on spells out every number.
    let plain = [0x00, 0x14, 0x0b, 0x00, 0xf1, 0x61, 0x3d, 0x31, 0x0e, 0x00, 0x00, 0x01, 0x00, 0x00, 0x0d];
    let quiet = list_basic(&plain, 0, plain.len(), opts);
    assert!(!basic_to_text(&quiet, opts).contains('{'));
    let loud = BasicOptions { show_numbers: true, ..opts };
    let shown = list_basic(&plain, 0, plain.len(), loud);
    assert!(basic_to_text(&shown, loud).contains("{1}"));

    // A zero-length line is reported rather than looped on.
    let broken = [0x00, 0x0a, 0x00, 0x00, 0x00, 0x0b, 0x02, 0x00, 0xf5, 0x0d];
    let lines = list_basic(&broken, 0, broken.len(), opts);
    assert_eq!(lines[0].error.as_deref(), Some("zero length line"));
    assert!(lines.len() > 1);
}

#[test]
fn lists_variables() {
    // A variable's header byte is its type in the top three bits and the
    // letter's place in the alphabet in the low five.
    let head = |kind: u8, letter: char| (kind << 5) | (letter as u8 - 0x60);
    let vars = [
        head(0b100, 'a'),
        0x00,
        0x00,
        0x2a,
        0x00,
        0x00, // a = 42
        head(0b011, 'b'),
        0x03,
        0x00,
        0x68,
        0x69,
        0x21, // b$ = "hi!"
        0x80,
    ];
    let out = list_variables(&vars, 0, vars.len());
    assert_eq!(out.len(), 2);
    assert_eq!((out[0].name.as_str(), out[0].kind.as_str(), out[0].value.as_str()), ("a", "number", "42"));
    assert_eq!(
        (out[1].name.as_str(), out[1].kind.as_str(), out[1].value.as_str()),
        ("b$", "string", "\"hi!\"")
    );
    assert_eq!(out[1].size, 6);
    // Garbage stops the listing with an entry saying so.
    let garbage = [0x01u8, 0x02, 0x03];
    let out = list_variables(&garbage, 0, garbage.len());
    assert_eq!(out.last().unwrap().kind, "garbage");
}

#[test]
fn disassembles_the_tricky_prefixes() {
    let d = |bytes: &[u8], count: usize| -> Vec<String> {
        disassemble(bytes, 0, 0x8000, count, DisOptions::default()).into_iter().map(|l| l.text).collect()
    };
    assert_eq!(d(&[0x00], 1), vec!["NOP"]);
    assert_eq!(d(&[0x21, 0x34, 0x12], 1), vec!["LD HL,0x1234"]);
    assert_eq!(d(&[0xdd, 0x21, 0x34, 0x12], 1), vec!["LD IX,0x1234"]);
    assert_eq!(d(&[0xdd, 0x7e, 0x05], 1), vec!["LD A,(IX+0x05)"]);
    assert_eq!(d(&[0xfd, 0x7e, 0xfb], 1), vec!["LD A,(IY-0x05)"]);
    assert_eq!(d(&[0xdd, 0x66, 0x05], 1), vec!["LD H,(IX+0x05)"]); // H stays H here
    assert_eq!(d(&[0xdd, 0x64], 1), vec!["LD IXH,IXH"]); // but not here
    assert_eq!(d(&[0xcb, 0x40], 1), vec!["BIT 0,B"]);
    assert_eq!(d(&[0xdd, 0xcb, 0x05, 0x46], 1), vec!["BIT 0,(IX+0x05)"]);
    assert_eq!(d(&[0xdd, 0xcb, 0x05, 0x00], 1), vec!["RLC (IX+0x05),B"]); // undocumented
    assert_eq!(d(&[0xed, 0xb0], 1), vec!["LDIR"]);
    assert_eq!(d(&[0xed, 0x4b, 0x34, 0x12], 1), vec!["LD BC,(0x1234)"]);
    assert_eq!(d(&[0xed, 0x00], 1), vec!["NOP*"]);
    assert_eq!(d(&[0xdd, 0xdd, 0xfd, 0x00], 1), vec!["NOP"]); // prefix chain
    assert_eq!(d(&[0x76], 1), vec!["HALT"]);
    assert_eq!(d(&[0xc7], 1), vec!["RST 0x00  ; START"]); // ROM label
    assert_eq!(d(&[0xcd, 0x56, 0x05], 1), vec!["CALL 0x0556  ; LD-BYTES"]);

    // Relative jumps are resolved against the address they land after.
    let lines = disassemble(&[0x18, 0xfe], 0, 0x8000, 1, DisOptions::default());
    assert_eq!(lines[0].text, "JR 0x8000");
    assert_eq!(lines[0].target, Some(0x8000));
    assert_eq!(lines[0].bytes, vec![0x18, 0xfe]);

    let dec = disassemble(
        &[0x21, 0x34, 0x12],
        0,
        0x8000,
        1,
        DisOptions { hex: false, rom_labels: true, ..DisOptions::default() },
    );
    assert_eq!(dec[0].text, "LD HL,4660");
    let bare = disassemble(
        &[0xc7],
        0,
        0x8000,
        1,
        DisOptions { hex: true, rom_labels: false, ..DisOptions::default() },
    );
    assert_eq!(bare[0].text, "RST 0x00");
    // Running off the end stops rather than inventing instructions.
    assert_eq!(disassemble(&[], 0, 0, 10, DisOptions::default()).len(), 0);
}

// ---- BASIC as text ---------------------------------------------------------

/// Every BASIC program on a tape, as the bytes between PROG and VARS.
fn programs(blocks: &[tapeti_core::types::Block]) -> Vec<Vec<u8>> {
    use tapeti_core::content::{detect_content, ContentKind};
    (0..blocks.len())
        .filter_map(|i| {
            let c = detect_content(blocks, i);
            let len = usize::from(c.prog_len?);
            let data = blocks[i].body.data()?;
            (c.kind == ContentKind::Basic).then(|| data[1..1 + len].to_vec())
        })
        .collect()
}

/// Every BASIC program the samples hold, and the one the snapshot loader writes:
/// colour controls in a string, VAL "..." and all.
fn sample_programs() -> Vec<Vec<u8>> {
    use tapeti_core::snapshot::{snapshot_to_blocks, LoaderOptions, Snapshot};

    let mut found = Vec::new();
    for name in ["Tapeti demo.tzx", "Tapeti demo (variant).tzx"] {
        let bytes = std::fs::read(format!("../test/samples/{name}")).unwrap();
        found.extend(programs(&tapeti_core::parser::parse_tape(&bytes).unwrap().blocks));
    }
    let mut snap = Snapshot::default();
    snap.pages[5] = Some(vec![0; 16384]);
    let opts = LoaderOptions { name: "x", speed: 2, border: 0, compress_all: false, screen: None };
    found.extend(programs(&snapshot_to_blocks(&snap, &opts).unwrap()));
    assert!(found.len() >= 3, "only {} programs to try", found.len());
    found
}

/// A program written out as text and typed back in is the same program, byte for
/// byte — through the tokeniser, not by way of the lines an edit leaves alone.
#[test]
fn programs_survive_being_text() {
    use tapeti_core::spectrum::source::{basic_source, tokenise_line, SourceOptions};

    let opts = SourceOptions::default();
    for program in sample_programs() {
        for line in basic_source(&program, 0, program.len(), opts) {
            let again = tokenise_line(&line.text, opts).unwrap_or_else(|e| panic!("{}: {e}", line.text));
            assert_eq!(again, &program[line.offset..line.offset + line.len], "{}", line.text);
        }
    }
}

/// Nothing the samples or the snapshot loader hold is something the syntax check
/// turns down. A check that shouts at a working program is worse than none.
#[test]
fn the_syntax_check_lets_real_programs_through() {
    use tapeti_core::spectrum::source::{basic_source, tokenise_line, SourceOptions};

    let opts = SourceOptions { check_syntax: true, ..SourceOptions::default() };
    let mut lines = 0;
    for program in sample_programs() {
        for line in basic_source(&program, 0, program.len(), opts) {
            tokenise_line(&line.text, opts).unwrap_or_else(|e| panic!("{}: {e}", line.text));
            lines += 1;
        }
    }
    assert!(lines >= 15, "only {lines} lines to try");
}

/// The Insert dialog's empty program is one the rest of the app takes for BASIC:
/// detected as such from its header, consistent, and ready to be given lines.
#[test]
fn an_empty_program_is_a_program() {
    use tapeti_core::consistency::check_consistency;
    use tapeti_core::content::{detect_content, ContentKind};
    use tapeti_core::describe::{checksum, decode_header, empty_program};
    use tapeti_core::spectrum::source::{edit_basic, SourceOptions};
    use tapeti_core::types::Block;

    let blocks: Vec<Block> = empty_program().into_iter().map(Block::new).collect();
    let header = decode_header(blocks[0].body.data().unwrap()).unwrap();
    assert_eq!((header.kind, header.length, header.param1, header.param2), (0, 0, 0x8000, 0));
    let content = detect_content(&blocks, 1);
    assert_eq!((content.kind, content.prog_len), (ContentKind::Basic, Some(0)));
    assert_eq!(checksum(blocks[1].body.data().unwrap()), 0);
    assert!(check_consistency(&blocks, 0).is_empty(), "{:?}", check_consistency(&blocks, 0));

    let program = edit_basic(&[], 0, 0, "10 PRINT \"hi\"\n20 GO TO 10", SourceOptions::default()).unwrap();
    assert_eq!(program.len(), 4 + 6 + 4 + 10);
}

// ---- the disassembler past the port ---------------------------------------

fn listing(code: &[u8], base: u32, opts: DisOptions, symbols: &str) -> Vec<String> {
    use tapeti_core::spectrum::romnames::Symbols;
    use tapeti_core::spectrum::z80dis::disassemble_with;
    disassemble_with(code, 0, base, 1000, opts, &Symbols::parse(symbols).0)
        .into_iter()
        .map(|l| l.text)
        .collect()
}

const FULL: DisOptions = DisOptions { hex: true, rom_labels: true, sysvars: true, literals: true };

/// Behind RST 08 is a report code, behind RST 28 the calculator's literals up to
/// end-calc; read as instructions they put everything after them out of step.
#[test]
fn restarts_carry_their_literals() {
    let code = [
        0xef, // RST 28
        0xa1, // stk-one
        0x34, 0x40, 0xb0, 0x00, 0x0a, // stk-data: the small integer 10
        0x0f, // addition
        0x33, 0x02, // jump +2, from the byte that says so
        0x31, // duplicate
        0x86, // series-06 ... cut short here by:
        0x38, // (a constant, as far as the calculator knows)
    ];
    let lines = listing(&code[..11], 0x8000, FULL, "");
    assert_eq!(
        lines,
        [
            "RST 0x28  ; FP-CALC",
            "DEFB 0xA1  ; stk-one",
            "DEFB 0x34,0x40,0xB0,0x00,0x0A  ; stk-data 10",
            "DEFB 0x0F  ; addition",
            "DEFB 0x33,0x02  ; jump to 0x800B",
            "DEFB 0x31  ; duplicate",
        ]
    );
    // end-calc hands back to code, and a report code is one byte.
    let lines = listing(&[0xef, 0x38, 0xcf, 0x1a, 0xc9, 0xcf, 0x77], 0x8000, FULL, "");
    assert_eq!(
        lines,
        [
            "RST 0x28  ; FP-CALC",
            "DEFB 0x38  ; end-calc",
            "RST 0x08  ; ERROR-1",
            "DEFB 0x1A  ; R Tape loading error",
            "RET",
            "RST 0x08  ; ERROR-1",
            "DEFB 0x77  ; not a report",
        ]
    );
    // A series is followed by that many packed constants: a half, then a tenth-ish.
    let lines = listing(&[0xef, 0x82, 0x30, 0x00, 0xf1, 0x4c, 0xcc, 0xcc, 0xcc, 0x38], 0x8000, FULL, "");
    assert_eq!(lines[1], "DEFB 0x82  ; series-02");
    assert_eq!(lines[2], "DEFB 0x30,0x00  ; 0.5");
    assert!(lines[3].starts_with("DEFB 0xF1,0x4C,0xCC,0xCC,0xCC  ; "), "{}", lines[3]);
    assert_eq!(lines[4], "DEFB 0x38  ; end-calc");
    // Off, they are the instructions they look like.
    assert_eq!(
        listing(&[0xcf, 0x1a], 0x8000, DisOptions::default(), ""),
        ["RST 0x08  ; ERROR-1", "LD A,(DE)"]
    );
}

#[test]
fn operands_name_system_variables_and_the_users_symbols() {
    let code = [
        0x2a, 0x4b, 0x5c, //       LD HL,(5C4B)
        0xfd, 0xcb, 0x01, 0x6e, // BIT 5,(IY+1)
        0xfd, 0x36, 0x3e, 0x00, // LD (IY+0x3E),0
        0xdd, 0x36, 0x01, 0x00, // LD (IX+1),0: not IY, not a variable
        0xcd, 0x10, 0x80, //       CALL 8010
        0x21, 0x00, 0x90, //       LD HL,9000
        0x18, 0xfe, //             JR to itself
    ];
    let lines = listing(&code, 0x8000, FULL, "$8010 DRAW_IT\n$9000 TABLE\n$8015 LOOP");
    assert_eq!(
        lines,
        [
            "LD HL,(0x5C4B)  ; VARS",
            "BIT 5,(IY+0x01)  ; FLAGS",
            "LD (IY+0x3E),0x00  ; FRAMES",
            "LD (IX+0x01),0x00",
            "CALL 0x8010  ; DRAW_IT",
            "LD HL,0x9000  ; TABLE",
            "LOOP:",
            "JR 0x8015  ; LOOP",
        ]
    );
    // A name of the user's wins over the ROM's.
    assert_eq!(listing(&[0xcd, 0x56, 0x05], 0x8000, FULL, "$0556 LOADER"), ["CALL 0x0556  ; LOADER"]);
    assert_eq!(listing(&[0xcd, 0x56, 0x05], 0x8000, FULL, ""), ["CALL 0x0556  ; LD-BYTES"]);
}

/// The snapshot loader's tape routine ends its entry point on RST 08 and the
/// report it wants: with the literals read, the routine behind it lines up.
#[test]
fn the_snapshot_loader_reads_in_step() {
    use tapeti_core::snapshot::{snapshot_to_blocks, LoaderOptions, Snapshot};
    let mut snap = Snapshot::default();
    snap.pages[5] = Some(vec![0; 16384]);
    let opts = LoaderOptions { name: "x", speed: 2, border: 1, compress_all: false, screen: None };
    let blocks = snapshot_to_blocks(&snap, &opts).unwrap();
    let program = blocks[2].body.data().unwrap();
    // The 512 bytes of loader end the block, before its checksum; the routine is at BF55.
    let loader = &program[program.len() - 1 - 512..program.len() - 1];
    let lines = listing(&loader[0x155..], 0xbf55, FULL, "$BF5B LD_BYTES");
    assert_eq!(
        lines[..6],
        [
            "CALL 0xBF5B  ; LD_BYTES",
            "RET C",
            "RST 0x08  ; ERROR-1",
            "DEFB 0x1A  ; R Tape loading error",
            "LD_BYTES:",
            "INC D"
        ]
    );
}

/// Bytes that are no variables at all must list as something, not overflow: the
/// dimensions of a "number array" of noise multiply up past a usize.
#[test]
fn variables_of_noise_do_not_overflow() {
    let mut noise = vec![0x41, 0xff, 0xff, 0x0c];
    noise.extend(std::iter::repeat_n(0xff, 24));
    let listed = list_variables(&noise, 0, noise.len());
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, "number array");
}

#[test]
fn a_listing_can_leave_the_colours_out() {
    let line = [0, 10, 12, 0, 0xf5, b'"', 0x10, 2, 0x16, 1, 2, b'h', b'i', b'"', 0x06, 0x0d];
    let text = |opts| basic_to_text(&list_basic(&line, 0, line.len(), opts), opts);
    assert_eq!(text(BasicOptions::default()), "  10 PRINT \"[INK 2][AT 1,2]hi\"[,]");
    assert_eq!(text(BasicOptions { drop_colours: true, ..BasicOptions::default() }), "  10 PRINT \"hi\"[,]");
}

#[test]
fn hidden_numbers_can_be_listed_in_hex() {
    // 10 PRINT 0,.5,1,70000 — hidden 23760, 0.5, -1 and 70000 behind them.
    let mut body = vec![0xf5, b'0', 0x0e, 0x00, 0x00, 0xd0, 0x5c, 0x00, b','];
    body.extend_from_slice(&[b'.', b'5', 0x0e, 0x80, 0x00, 0x00, 0x00, 0x00, b',']);
    body.extend_from_slice(&[b'1', 0x0e, 0x00, 0xff, 0xff, 0xff, 0x00, b',']);
    body.extend_from_slice(b"70000");
    body.extend_from_slice(&[0x0e, 0x91, 0x08, 0xb8, 0x00, 0x00, 0x0d]);
    let mut line = vec![0x00, 0x0a, body.len() as u8, 0x00];
    line.extend_from_slice(&body);
    let text = |opts| basic_to_text(&list_basic(&line, 0, line.len(), opts), opts);

    let dec = BasicOptions { show_numbers: true, ..BasicOptions::default() };
    let listed = text(dec);
    for want in ["0{23760}", "{0.5}", "{-1}", "{70000}"] {
        assert!(listed.contains(want), "{want} in {listed}");
    }
    // Hex for the whole numbers an address or a byte can be, nothing else.
    let hex = BasicOptions { hex_numbers: true, ..dec };
    let listed = text(hex);
    for want in ["0{5CD0}", "{0.5}", "{-1}", "{70000}"] {
        assert!(listed.contains(want), "{want} in {listed}");
    }
    // A mismatch is shown with show_numbers off too, and in hex as well.
    let listed = text(BasicOptions { hex_numbers: true, ..BasicOptions::default() });
    assert!(listed.contains("0{5CD0}"), "{listed}");
}
