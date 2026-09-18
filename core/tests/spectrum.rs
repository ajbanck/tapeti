//! Tests for the Spectrum side: the character set, the screen, the BASIC lister
//! and the Z80 disassembler. `test/core.test.ts` compares these against the
//! TypeScript they replaced; what is here is what the core owes on its own.

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
                                                     // Hiding the attributes forces black on white whatever the attribute says.
    data[6144] = 0b0100_0010; // bright red ink on black paper
    let colour = render_screen(&data, 0, ScreenOptions::default());
    assert_eq!(&colour[0..4], &[0xff, 0, 0, 255]);
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

    // The same program with showNumbers on spells out every number.
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

    // Decimal mode spells everything in decimal.
    let dec = disassemble(&[0x21, 0x34, 0x12], 0, 0x8000, 1, DisOptions { hex: false, rom_labels: true });
    assert_eq!(dec[0].text, "LD HL,4660");
    // Without labels, nothing is annotated.
    let bare = disassemble(&[0xc7], 0, 0x8000, 1, DisOptions { hex: true, rom_labels: false });
    assert_eq!(bare[0].text, "RST 0x00");
    // Running off the end stops rather than inventing instructions.
    assert_eq!(disassemble(&[], 0, 0, 10, DisOptions::default()).len(), 0);
}
