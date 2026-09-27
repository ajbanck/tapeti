// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck
// Portions Copyleft (C) 1997-2001 ThunderWare Research Center, written by Martijn van der Heide
// (Taper, tpbasic.c, GPL-2.0-or-later)

//! Does a tokenised line say something a Spectrum would accept?
//!
//! [`tokenise_line`](super::source::tokenise_line) turns text into bytes without checking
//! what they mean, so `10 PRINT AND` comes out as a program line. This is the second half,
//! after TAPER's `tpbasic.c`: a recursive-descent walk against the 48K ROM's syntax tables
//! (the command classes at 1A48 in "The Complete Spectrum ROM Disassembly"). Every keyword
//! has a `Kind` and a list of what must follow it: a *class* (one of the ROM's fifteen
//! operand shapes) or a literal byte (`(`, `,`, `TO`). [`check_line`] walks statement by
//! statement against those lists, typing expressions as it goes.
//!
//! Only the 48K ROM is known: Interface 1, Microdrive and disc syntax (CAT, FORMAT, MOVE,
//! ERASE, OPEN #, CLOSE #, `LOAD *`, `SAVE !`) is taken as read rather than guessed at,
//! since a false positive blocks a real edit. Also unchecked: only AND and OR need a left
//! operand (so `PRINT =1` passes; guarding the others must still allow `a=b=c`), and PLAY is
//! checked only as a list of strings, not against what the 128K ROM allows after it.
//!
//! Deliberate departures from TAPER's own checker:
//! - TAPER's `TokenBracket` is global, so it lets a nested expression run past an operator
//!   and rejects `LET a$=a$( TO LEN a$-1)` (the standard way to drop a string's last
//!   character); here the flag is passed per operand instead.
//! - `DEF FN a()=1` (no arguments) is accepted; TAPER reports it as an empty parameter list
//!   even though the Spectrum allows it.
//! - A statement may start with `:` (`IF a THEN : PRINT 1`); TAPER reports that as ending
//!   where it should not.
//! - No comma list or PRINT item loop spins on an operand that consumes nothing, which is
//!   what `DATA 1)` and `PRINT !` do to TAPER's classes 13 and 5.
//! - Running off the end of the line reads ENTER rather than whatever lies behind it; both
//!   count brackets first, but TAPER's pointer leaves the buffer if one is still unclosed.

use super::charset::token_name;
use super::source::number_len;

/// What a byte is when a statement or an expression runs into it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// A character, an operator, or a word like THEN that its command handles:
    /// nothing of its own to check.
    Plain,
    /// A command, which only stands at the start of a statement.
    Command,
    /// INK to OVER: a command, and an item PRINT and PLOT take.
    Colour,
    /// A function giving a number.
    Number,
    /// A function giving a string.
    Text,
    /// AT and TAB, which only PRINT and LPRINT take.
    Item,
}

/// The type of an expression. The ROM keeps the same one bit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ty {
    Num,
    Str,
}

// The ROM's operand classes, as TAPER numbers them. Twelve to fifteen are not
// the ROM's: they stand for the shapes its class 5 and 6 routines special-case.
const VAR: u8 = 1; // a variable (LET, FN, DIM)
const ASSIGN: u8 = 2; // what LET assigns, of the variable's own type
const OPT_NUM: u8 = 3; // a number, or nothing at all
const LETTER: u8 = 4; // a one letter numeric variable
const ITEMS: u8 = 5; // the items of PRINT, LPRINT and INPUT
const NUM: u8 = 6; // a number
const COLOUR: u8 = 7; // a colour item's value, which is a number
const NUM2: u8 = 8; // two numbers with a comma between
const PLOT: u8 = 9; // colour items, then two numbers
const STR: u8 = 10; // a string
const TAPE: u8 = 11; // LOAD, SAVE and the rest of the cassette commands
const STRINGS: u8 = 12; // strings with commas between
const EXPRS: u8 = 13; // expressions with commas between
const VARS: u8 = 14; // variables with commas between
const DEFFN: u8 = 15; // DEF FN's name, its parameters and its body

const END: u8 = 0x0d;
const MARK: u8 = 0x0e; // the five bytes of a number, stripped down to its marker
const FN: u8 = 0xa8;
const INKEY: u8 = 0xa6;
const SCREEN: u8 = 0xaa;
const AT: u8 = 0xac;
const TAB: u8 = 0xad;
const CODE: u8 = 0xaf;
const USR: u8 = 0xc0;
const OR: u8 = 0xc5;
const AND: u8 = 0xc6;
const LINE: u8 = 0xca;
const THEN: u8 = 0xcb;
const TO: u8 = 0xcc;
const STEP: u8 = 0xcd;
const MERGE: u8 = 0xd5;
const VERIFY: u8 = 0xd6;
const LPRINT: u8 = 0xe0;
const DATA: u8 = 0xe4;
const DIM: u8 = 0xe9;
const REM: u8 = 0xea;
const FOR: u8 = 0xeb;
const INPUT: u8 = 0xee;
const LOAD: u8 = 0xef;
const PRINT: u8 = 0xf5;
const SAVE: u8 = 0xf8;
const IF: u8 = 0xfa;
const DRAW: u8 = 0xfc;

/// Every token from SPECTRUM (0xa3) to COPY (0xff): what it is, and what has to
/// follow it. A byte of 32 or more in the operand list is that byte itself.
const TOKEN_SYNTAX: [(Kind, &[u8]); 93] = [
    (Kind::Command, &[]),                                      // SPECTRUM
    (Kind::Command, &[STRINGS]),                               // PLAY
    (Kind::Number, &[]),                                       // RND
    (Kind::Text, &[]),                                         // INKEY$
    (Kind::Number, &[]),                                       // PI
    (Kind::Number, &[VAR, b'(', EXPRS, b')']),                 // FN
    (Kind::Number, &[b'(', NUM2, b')']),                       // POINT
    (Kind::Text, &[b'(', NUM2, b')']),                         // SCREEN$
    (Kind::Number, &[b'(', NUM2, b')']),                       // ATTR
    (Kind::Item, &[NUM2]),                                     // AT
    (Kind::Item, &[NUM]),                                      // TAB
    (Kind::Text, &[STR]),                                      // VAL$
    (Kind::Number, &[STR]),                                    // CODE
    (Kind::Number, &[STR]),                                    // VAL
    (Kind::Number, &[STR]),                                    // LEN
    (Kind::Number, &[NUM]),                                    // SIN
    (Kind::Number, &[NUM]),                                    // COS
    (Kind::Number, &[NUM]),                                    // TAN
    (Kind::Number, &[NUM]),                                    // ASN
    (Kind::Number, &[NUM]),                                    // ACS
    (Kind::Number, &[NUM]),                                    // ATN
    (Kind::Number, &[NUM]),                                    // LN
    (Kind::Number, &[NUM]),                                    // EXP
    (Kind::Number, &[NUM]),                                    // INT
    (Kind::Number, &[NUM]),                                    // SQR
    (Kind::Number, &[NUM]),                                    // SGN
    (Kind::Number, &[NUM]),                                    // ABS
    (Kind::Number, &[NUM]),                                    // PEEK
    (Kind::Number, &[NUM]),                                    // IN
    (Kind::Number, &[NUM]),                                    // USR
    (Kind::Text, &[NUM]),                                      // STR$
    (Kind::Text, &[NUM]),                                      // CHR$
    (Kind::Number, &[NUM]),                                    // NOT
    (Kind::Number, &[OPT_NUM]),                                // BIN
    (Kind::Plain, &[]),                                        // OR
    (Kind::Plain, &[]),                                        // AND
    (Kind::Plain, &[]),                                        // <=
    (Kind::Plain, &[]),                                        // >=
    (Kind::Plain, &[]),                                        // <>
    (Kind::Plain, &[]),                                        // LINE
    (Kind::Plain, &[]),                                        // THEN
    (Kind::Plain, &[]),                                        // TO
    (Kind::Plain, &[]),                                        // STEP
    (Kind::Command, &[DEFFN]),                                 // DEF FN
    (Kind::Command, &[TAPE]),                                  // CAT
    (Kind::Command, &[TAPE]),                                  // FORMAT
    (Kind::Command, &[TAPE]),                                  // MOVE
    (Kind::Command, &[TAPE]),                                  // ERASE
    (Kind::Command, &[TAPE]),                                  // OPEN #
    (Kind::Command, &[TAPE]),                                  // CLOSE #
    (Kind::Command, &[TAPE]),                                  // MERGE
    (Kind::Command, &[TAPE]),                                  // VERIFY
    (Kind::Command, &[NUM2]),                                  // BEEP
    (Kind::Command, &[PLOT, b',', NUM]),                       // CIRCLE
    (Kind::Colour, &[COLOUR]),                                 // INK
    (Kind::Colour, &[COLOUR]),                                 // PAPER
    (Kind::Colour, &[COLOUR]),                                 // FLASH
    (Kind::Colour, &[COLOUR]),                                 // BRIGHT
    (Kind::Colour, &[COLOUR]),                                 // INVERSE
    (Kind::Colour, &[COLOUR]),                                 // OVER
    (Kind::Command, &[NUM2]),                                  // OUT
    (Kind::Command, &[ITEMS]),                                 // LPRINT
    (Kind::Command, &[OPT_NUM]),                               // LLIST
    (Kind::Command, &[]),                                      // STOP
    (Kind::Command, &[VARS]),                                  // READ
    (Kind::Command, &[EXPRS]),                                 // DATA
    (Kind::Command, &[OPT_NUM]),                               // RESTORE
    (Kind::Command, &[]),                                      // NEW
    (Kind::Command, &[NUM]),                                   // BORDER
    (Kind::Command, &[]),                                      // CONTINUE
    (Kind::Command, &[VAR, b'(', EXPRS, b')']),                // DIM
    (Kind::Command, &[ITEMS]),                                 // REM
    (Kind::Command, &[LETTER, b'=', NUM, TO, NUM, STEP, NUM]), // FOR
    (Kind::Command, &[NUM]),                                   // GO TO
    (Kind::Command, &[NUM]),                                   // GO SUB
    (Kind::Command, &[ITEMS]),                                 // INPUT
    (Kind::Command, &[TAPE]),                                  // LOAD
    (Kind::Command, &[OPT_NUM]),                               // LIST
    (Kind::Command, &[VAR, b'=', ASSIGN]),                     // LET
    (Kind::Command, &[NUM]),                                   // PAUSE
    (Kind::Command, &[LETTER]),                                // NEXT
    (Kind::Command, &[NUM2]),                                  // POKE
    (Kind::Command, &[ITEMS]),                                 // PRINT
    (Kind::Command, &[PLOT]),                                  // PLOT
    (Kind::Command, &[OPT_NUM]),                               // RUN
    (Kind::Command, &[TAPE]),                                  // SAVE
    (Kind::Command, &[OPT_NUM]),                               // RANDOMIZE
    (Kind::Command, &[NUM, THEN]),                             // IF
    (Kind::Command, &[]),                                      // CLS
    (Kind::Command, &[PLOT, b',', NUM]),                       // DRAW
    (Kind::Command, &[OPT_NUM]),                               // CLEAR
    (Kind::Command, &[]),                                      // RETURN
    (Kind::Command, &[]),                                      // COPY
];

/// How deep brackets and operators may nest before we call it nonsense. A line
/// can be 64K of `(`, and the walk below is recursive.
const MAX_DEPTH: u32 = 48;

/// The body of one program line: everything after the four byte header, up to
/// and including the ENTER. `Ok` means the Spectrum would take it.
pub fn check_line(body: &[u8], basic128: bool) -> Result<(), String> {
    let s = strip(body);
    brackets_match(&s)?;
    let mut c = Check { s, at: 0, statement: 0, basic128, depth: 0, let_ty: Ty::Num };
    match c.line() {
        Ok(()) => Ok(()),
        Err(message) if c.statement > 1 => Err(format!("statement {}: {message}", c.statement)),
        Err(message) => Err(message),
    }
}

/// The line with everything the syntax does not read taken out: spaces, colour
/// controls and their operands, and the five bytes behind a number. The number
/// marker itself stays, because a DEF FN parameter is nothing else. Strings keep their
/// bytes, so what is inside one cannot end a statement, and REM takes the rest
/// of the line with it.
fn strip(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut in_string = false;
    let mut p = 0;
    while p < body.len() {
        let c = body[p];
        p += 1;
        if in_string {
            if c == END {
                break;
            }
            out.push(c);
            in_string = c != b'"';
            continue;
        }
        match c {
            END => break,
            b'"' => {
                in_string = true;
                out.push(c);
            }
            REM => {
                out.push(c);
                break;
            }
            MARK => {
                out.push(c);
                p = (p + 5).min(body.len());
            }
            0x10..=0x15 => p += 1,
            0x16 | 0x17 => p += 2,
            0x00..=0x20 => {}
            _ => out.push(c),
        }
    }
    out.push(END);
    out
}

/// Counted over the whole line before anything else, as TAPER does it: the walk
/// below reads a bracket as the end of what opened it, and would run off the end
/// of a line that closes one too few.
fn brackets_match(s: &[u8]) -> Result<(), String> {
    let mut open = 0i32;
    let mut in_string = false;
    for &c in s {
        match c {
            b'"' => in_string = !in_string,
            b'(' if !in_string => open += 1,
            b')' if !in_string => {
                open -= 1;
                if open < 0 {
                    return Err("a bracket is closed that was never opened".to_string());
                }
            }
            _ => {}
        }
    }
    if open > 0 {
        return Err("a bracket is left open".to_string());
    }
    Ok(())
}

struct Check {
    s: Vec<u8>,
    at: usize,
    statement: u32,
    basic128: bool,
    depth: u32,
    /// The type of the variable LET is assigning, for its class 2.
    let_ty: Ty,
}

impl Check {
    fn peek(&self) -> u8 {
        self.s.get(self.at).copied().unwrap_or(END)
    }

    fn kind(&self, c: u8) -> Kind {
        self.syntax(c).0
    }

    fn syntax(&self, c: u8) -> (Kind, &'static [u8]) {
        if c < 0xa3 || (c < 0xa5 && !self.basic128) {
            return (Kind::Plain, &[]);
        }
        TOKEN_SYNTAX[usize::from(c) - 0xa3]
    }

    /// What to call a byte in a message.
    fn name(&self, c: u8) -> String {
        match c {
            END => "the end of the line".to_string(),
            b':' => "the end of the statement".to_string(),
            MARK => "a number".to_string(),
            _ => match token_name(c, self.basic128) {
                Some(name) => format!("\"{name}\""),
                None if (0x20..0x7f).contains(&c) => format!("\"{}\"", char::from(c)),
                None => format!("the byte {c:02X}"),
            },
        }
    }

    fn at_end(&self) -> bool {
        matches!(self.peek(), b':' | END)
    }

    /// An operand has to start here, not stop.
    fn want_more(&self) -> Result<(), String> {
        if self.at_end() {
            return Err("the statement stops short".to_string());
        }
        Ok(())
    }

    fn need(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() != byte {
            return Err(format!("expected {}, but got {}", self.name(byte), self.name(self.peek())));
        }
        self.at += 1;
        Ok(())
    }

    /// Everything up to the next statement, taken as read.
    fn skip_statement(&mut self) {
        let mut in_string = false;
        while self.at < self.s.len() {
            let c = self.s[self.at];
            if c == b'"' {
                in_string = !in_string;
            } else if !in_string && (c == b':' || c == END) {
                return;
            }
            self.at += 1;
        }
    }

    // ---- statements --------------------------------------------------------

    fn line(&mut self) -> Result<(), String> {
        while self.peek() != END {
            // An empty statement is legal, if unusual: "IF a THEN : PRINT 1".
            if self.peek() == b':' {
                self.at += 1;
                continue;
            }
            self.statement += 1;
            let keyword = self.peek();
            self.at += 1;
            if keyword == REM {
                return Ok(());
            }
            let (kind, classes) = self.syntax(keyword);
            if kind != Kind::Command && kind != Kind::Colour {
                return Err(format!("{} does not start a statement", self.name(keyword)));
            }
            let mut n = 0;
            let mut brackets = false;
            while n < classes.len() {
                let class = classes[n];
                n += 1;
                if self.peek() == END {
                    if class == OPT_NUM || class == ITEMS {
                        continue;
                    }
                    // FOR needs no STEP, and DRAW no third value.
                    if (keyword == FOR && class == STEP) || (keyword == DRAW && class == b',') {
                        break;
                    }
                    return Err("the statement stops short".to_string());
                }
                if class >= 32 {
                    if self.peek() != class {
                        if self.peek() == b':'
                            && ((keyword == FOR && class == STEP) || (keyword == DRAW && class == b','))
                        {
                            break;
                        }
                        return Err(format!(
                            "expected {}, but got {}",
                            self.name(class),
                            self.name(self.peek())
                        ));
                    }
                    // The brackets a token brings along hold a whole expression.
                    brackets = self.peek() == b'(';
                    self.at += 1;
                } else {
                    self.class(class, keyword, brackets)?;
                }
            }
            // What follows THEN is the next statement, not the end of this one.
            if keyword != IF && !self.at_end() {
                return Err(format!("{} is not where a statement ends", self.name(self.peek())));
            }
        }
        Ok(())
    }

    // ---- the operand classes ----------------------------------------------

    /// `brackets` is set inside the brackets a token brought along (`POINT (`,
    /// `FN (`), where a whole expression may stand rather than one tight operand.
    /// It is for this operand only: a function met inside it takes its own.
    fn class(&mut self, class: u8, keyword: u8, brackets: bool) -> Result<(), String> {
        match class {
            VAR => {
                // FN and DIM name a variable and bracket it themselves.
                let slicing = if keyword == FN || keyword == DIM { -1 } else { 2 };
                let (ty, _) = self.a_variable(slicing)?;
                self.let_ty = ty;
                Ok(())
            }
            ASSIGN => {
                let want = self.let_ty;
                if self.value(keyword, None, brackets)? != want {
                    Err(match want {
                        Ty::Num => "a number variable takes a number".to_string(),
                        Ty::Str => "a string variable takes a string".to_string(),
                    })
                } else {
                    Ok(())
                }
            }
            OPT_NUM => {
                if self.at_end() {
                    return Ok(());
                }
                self.class(NUM, keyword, brackets)
            }
            LETTER => {
                let (ty, len) = self.a_variable(0)?;
                if len != 1 || ty != Ty::Num {
                    return Err(format!("{} counts with a one letter number", self.name(keyword)));
                }
                Ok(())
            }
            ITEMS => self.items(keyword),
            NUM | COLOUR => {
                // USR takes a string too: USR "a" is where a UDG lives.
                let want = if keyword == USR { None } else { Some(Ty::Num) };
                self.value(keyword, want, brackets)?;
                Ok(())
            }
            NUM2 => {
                self.class(NUM, keyword, brackets)?;
                self.need(b',')?;
                self.class(NUM, keyword, brackets)
            }
            PLOT => {
                while self.kind(self.peek()) == Kind::Colour {
                    self.at += 1;
                    self.class(COLOUR, keyword, brackets)?;
                    self.want_more()?;
                    self.need(b';')?;
                }
                self.want_more()?;
                self.class(NUM2, keyword, brackets)
            }
            STR => {
                self.value(keyword, Some(Ty::Str), brackets)?;
                Ok(())
            }
            TAPE => self.tape(keyword),
            STRINGS | EXPRS => self.list(class, keyword, brackets),
            VARS => self.variables(),
            DEFFN => self.def_fn(),
            _ => Ok(()),
        }
    }

    /// An expression that has to be there, and to be of the type asked for.
    fn value(&mut self, keyword: u8, want: Option<Ty>, brackets: bool) -> Result<Ty, String> {
        self.want_more()?;
        let before = self.at;
        let ty = self.expression(keyword, 0, brackets)?;
        if self.at == before {
            return Err(format!("{} is not the start of a value", self.name(self.peek())));
        }
        match want {
            Some(w) if w != ty => Err(match w {
                Ty::Num => "expected a number, and this is a string".to_string(),
                Ty::Str => "expected a string, and this is a number".to_string(),
            }),
            _ => Ok(ty),
        }
    }

    /// Class 5: what PRINT, LPRINT and INPUT take, in any number and order.
    fn items(&mut self, keyword: u8) -> Result<(), String> {
        loop {
            while matches!(self.peek(), b';' | b',' | b'\'') {
                self.at += 1;
            }
            if self.at_end() {
                return Ok(());
            }
            let c = self.peek();
            let before = self.at;
            if c == b'#' {
                self.at += 1;
                self.value(keyword, Some(Ty::Num), false)?;
            } else if self.kind(c) == Kind::Colour || c == TAB {
                self.at += 1;
                self.class(NUM, keyword, false)?;
            } else if c == AT {
                self.at += 1;
                self.class(NUM2, keyword, false)?;
            } else if keyword == INPUT && c == LINE {
                self.at += 1;
                if self.a_variable(0)?.0 != Ty::Str {
                    return Err("INPUT LINE reads into a string".to_string());
                }
            } else {
                self.expression(keyword, 0, false)?;
            }
            if self.at == before {
                return Err(format!("{} is not something {} takes", self.name(c), self.name(keyword)));
            }
        }
    }

    /// Classes 12 and 13: expressions with commas between them, which for FN and
    /// DIM stop at the closing bracket their token asked for.
    fn list(&mut self, class: u8, keyword: u8, brackets: bool) -> Result<(), String> {
        if keyword == FN && self.peek() == b')' {
            return Ok(()); // FN f() takes no arguments at all
        }
        loop {
            let before = self.at;
            let ty = self.expression(keyword, 0, brackets)?;
            if self.at == before {
                return Err(format!("{} is not the start of a value", self.name(self.peek())));
            }
            if class == STRINGS && ty != Ty::Str {
                return Err(format!("{} takes strings", self.name(keyword)));
            }
            if keyword == DIM && ty != Ty::Num {
                return Err("a dimension is a number".to_string());
            }
            if self.at_end() || self.peek() == b')' {
                return Ok(());
            }
            self.need(b',')?;
        }
    }

    /// Class 14: the variables READ reads into.
    fn variables(&mut self) -> Result<(), String> {
        loop {
            self.a_variable(2)?;
            if self.at_end() {
                return Ok(());
            }
            self.need(b',')?;
        }
    }

    /// Class 15: DEF FN's one letter name, its parameters and its body.
    fn def_fn(&mut self) -> Result<(), String> {
        let (_, len) = self.a_variable(-1)?;
        if len != 1 {
            return Err("a function has a one letter name".to_string());
        }
        if self.peek() == b'(' {
            self.at += 1;
            while self.peek() != b')' {
                self.want_more()?;
                if self.a_variable(-1)?.1 != 1 {
                    return Err("a parameter has a one letter name".to_string());
                }
                // Our own tokeniser leaves each parameter the five bytes the
                // Spectrum's editor does; the marker is all that survives strip.
                if self.peek() == MARK {
                    self.at += 1;
                }
                if self.peek() != b')' {
                    self.need(b',')?;
                }
            }
            self.at += 1;
        }
        self.need(b'=')?;
        self.value(0xce, None, false)?;
        Ok(())
    }

    /// Class 11. LOAD, SAVE, VERIFY and MERGE off tape are checked; the rest is
    /// Interface 1 and disc syntax this does not know.
    fn tape(&mut self, keyword: u8) -> Result<(), String> {
        if !matches!(keyword, LOAD | SAVE | VERIFY | MERGE) || matches!(self.peek(), b'*' | b'!') {
            self.skip_statement();
            return Ok(());
        }
        if self.at_end() || matches!(self.peek(), CODE | DATA | LINE | SCREEN) {
            return Err(format!("{} wants a name", self.name(keyword)));
        }
        self.class(STR, keyword, false)?;
        if self.at_end() {
            return Ok(());
        }
        match self.peek() {
            CODE => {
                if keyword == MERGE {
                    return Err("MERGE reads a program, not CODE".to_string());
                }
                self.at += 1;
                if self.at_end() {
                    return Ok(());
                }
                self.class(NUM, keyword, false)?;
                if self.peek() == b',' {
                    self.at += 1;
                    self.class(NUM, keyword, false)?;
                } else if !self.at_end() {
                    return Err(format!("expected \",\", but got {}", self.name(self.peek())));
                }
            }
            SCREEN => self.at += 1,
            DATA => {
                self.at += 1;
                self.want_more()?;
                if self.a_variable(-1)?.1 != 1 {
                    return Err("an array has a one letter name".to_string());
                }
                self.need(b'(')?;
                self.need(b')')?;
            }
            LINE if keyword == SAVE => {
                self.at += 1;
                self.class(NUM, keyword, false)?;
            }
            c => return Err(format!("{} is not a kind of file", self.name(c))),
        }
        Ok(())
    }

    // ---- variables ---------------------------------------------------------

    /// A variable that has to be here.
    fn a_variable(&mut self, slicing: i8) -> Result<(Ty, usize), String> {
        match self.variable(slicing)? {
            Some(v) => Ok(v),
            None => Err(format!("{} is not a variable", self.name(self.peek()))),
        }
    }

    /// A variable at the cursor, with whatever slice or index follows it:
    /// `slicing` is -1 to leave any bracket alone (DEF FN, FN and DIM name the
    /// variable and bracket it themselves), 0 for a plain name, 1 for a slice or
    /// an index, 2 for an index only.
    fn variable(&mut self, slicing: i8) -> Result<Option<(Ty, usize)>, String> {
        if !self.peek().is_ascii_alphabetic() {
            return Ok(None);
        }
        let mut len = 1;
        self.at += 1;
        while self.peek().is_ascii_alphanumeric() {
            self.at += 1;
            len += 1;
        }
        let mut ty = Ty::Num;
        if self.peek() == b'$' {
            if len > 1 {
                return Err("a string variable has a one letter name".to_string());
            }
            self.at += 1;
            ty = Ty::Str;
        }
        if slicing < 0 || self.peek() != b'(' {
            return Ok(Some((ty, len)));
        }
        if len > 1 {
            return Err("an array has a one letter name".to_string());
        }
        if slicing == 0 {
            return Err(format!("{} takes a variable, not a slice of one", self.name(self.peek())));
        }
        self.at += 1;
        if self.peek() == b')' {
            return Err("an empty index is no index".to_string());
        }
        let mut array = false;
        if self.peek() != TO {
            self.index()?;
            if self.peek() == b')' {
                self.at += 1;
                return Ok(Some((ty, len)));
            }
        }
        if self.peek() == b',' {
            array = true;
        } else if self.peek() == TO {
            if slicing == 2 {
                return Err("TO slices a string, it does not index an array".to_string());
            }
            if ty == Ty::Num {
                return Err("only a string can be sliced".to_string());
            }
        } else {
            return Err(format!("expected \",\", but got {}", self.name(self.peek())));
        }
        loop {
            self.at += 1; // the comma, or the TO of a slice
            self.index()?;
            if self.peek() == b')' {
                break;
            }
            if !array || self.peek() != b',' {
                return Err(format!("expected \")\", but got {}", self.name(self.peek())));
            }
        }
        self.at += 1;
        Ok(Some((ty, len)))
    }

    /// One index or slice bound: a number, and a whole expression may stand for
    /// it. A missing one is the open end of `a$(2 TO )`.
    fn index(&mut self) -> Result<(), String> {
        if self.expression(b'(', 0, true)? != Ty::Num {
            return Err("an index is a number".to_string());
        }
        Ok(())
    }

    // ---- expressions -------------------------------------------------------

    fn expression(&mut self, keyword: u8, level: u32, brackets: bool) -> Result<Ty, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("the line nests deeper than the Spectrum would follow".to_string());
        }
        let ty = self.scan(keyword, level, brackets);
        self.depth -= 1;
        ty
    }

    fn scan(&mut self, keyword: u8, level: u32, brackets: bool) -> Result<Ty, String> {
        // The type of the piece being read, and of the whole, which an equation
        // or a logical operator settles ahead of its right hand side.
        let mut sub = Ty::Num;
        let mut ty = Ty::Num;
        let mut sub_known = false;
        let mut total_known = false;
        let conflict = || "two types meet in one expression".to_string();
        if matches!(self.peek(), b'+' | b'-') {
            sub_known = true;
            self.at += 1;
        }
        let mut more = true;
        while more {
            let c = self.peek();
            if c == b'(' {
                self.at += 1;
                let inner = self.expression(b'(', level + 1, false)?;
                if sub_known && inner != sub {
                    return Err(conflict());
                }
                sub = inner;
                sub_known = true;
                self.at += 1; // the closing bracket the recursion left standing
            } else if c == b')' {
                // Left for whoever opened it, so ATTR (1,2) can close its own.
                break;
            } else if c == b':' || c == END {
                if level > 0 {
                    return Err("a bracket is left open".to_string());
                }
                break;
            } else if c == MARK || c.is_ascii_digit() || c == b'.' {
                if sub_known && sub != Ty::Num {
                    return Err(conflict());
                }
                sub = Ty::Num;
                sub_known = true;
                let digits = number_len(&self.s[self.at..], false);
                self.at += digits.max(usize::from(c != MARK));
                if self.peek() == MARK {
                    self.at += 1;
                }
            } else if c == b'"' {
                if sub_known && sub != Ty::Str {
                    return Err(conflict());
                }
                sub = Ty::Str;
                sub_known = true;
                // Two in a row put a quote in the string, so read them all.
                while self.peek() == b'"' {
                    self.at += 1;
                    while self.peek() != b'"' {
                        if self.peek() == END {
                            return Err("a string is not closed".to_string());
                        }
                        self.at += 1;
                    }
                    self.at += 1;
                }
                if self.peek() == b'(' {
                    self.slice()?;
                }
            } else if let Some((var, _)) = self.variable(1)? {
                if sub_known && var != sub {
                    return Err(conflict());
                }
                sub = var;
                sub_known = true;
            } else {
                match self.kind(c) {
                    Kind::Command | Kind::Colour => {
                        return Err(format!("{} is a command, and this is an expression", self.name(c)))
                    }
                    Kind::Plain => {}
                    kind => {
                        self.at += 1;
                        if kind == Kind::Item {
                            if keyword != PRINT && keyword != LPRINT {
                                return Err(format!("only PRINT takes {}", self.name(c)));
                            }
                        } else if c == USR && self.peek() == b'"' {
                            self.udg_name()?;
                            continue;
                        } else {
                            let this = if kind == Kind::Number { Ty::Num } else { Ty::Str };
                            if sub_known && this != sub {
                                return Err(conflict());
                            }
                            sub = this;
                            sub_known = true;
                        }
                        let mut own = false;
                        for &class in self.syntax(c).1 {
                            self.want_more()?;
                            if class >= 32 {
                                own = self.peek() == b'(';
                                self.need(class)?;
                            } else {
                                self.class(class, c, own)?;
                            }
                        }
                        if c == INKEY && self.peek() == b'#' {
                            self.at += 1;
                            self.value(c, Some(Ty::Num), false)?;
                        }
                    }
                }
            }
            // What comes between this piece and the next.
            let c = self.peek();
            if c == OR || c == AND {
                if !sub_known && !total_known {
                    return Err(format!("{} wants a value on its left", self.name(c)));
                }
                if !total_known {
                    ty = sub;
                }
                if c == OR && ty != Ty::Num {
                    return Err("OR wants a number on its left".to_string());
                }
                self.at += 1;
                // The recursion takes the whole of the right hand side, so what
                // it gives back is the answer.
                if self.expression(c, 0, false)? != Ty::Num {
                    return Err(format!("{} wants a number on its right", self.name(c)));
                }
                if !sub_known {
                    total_known = true;
                    // "x$ AND y" is a string; everything else is a number.
                    ty = if c == AND && ty == Ty::Str { Ty::Str } else { Ty::Num };
                    sub = ty;
                    sub_known = true;
                }
                more = false;
            } else if Self::equation(c) && level > 0 {
                // Between brackets an equation is a value of its own: LET a=(b=1).
                ty = Ty::Num;
                total_known = true;
                sub_known = false;
                self.at += 1;
            } else if !matches!(self.kind(keyword), Kind::Number | Kind::Text) || brackets {
                if c == b'+' {
                    self.at += 1;
                } else if matches!(c, b'-' | b'*' | b'/' | b'^') {
                    if sub_known && sub != Ty::Num {
                        return Err(conflict());
                    }
                    self.at += 1;
                } else if Self::equation(c) {
                    ty = Ty::Num;
                    total_known = true;
                    sub_known = false;
                    self.at += 1;
                } else {
                    more = false;
                }
            } else {
                // A function takes the tightest operand it can: in CHR$ 65+"x"
                // the plus belongs to whatever CHR$ itself belongs to.
                more = false;
            }
        }
        Ok(if total_known { ty } else { sub })
    }

    fn equation(c: u8) -> bool {
        matches!(c, b'=' | b'<' | b'>' | 0xc7 | 0xc8 | 0xc9)
    }

    /// `USR "a"`, which is the address of a user defined graphic.
    fn udg_name(&mut self) -> Result<(), String> {
        self.at += 1;
        let c = self.peek().to_ascii_uppercase();
        if !(b'A'..=b'U').contains(&c) {
            return Err("USR of a string wants one letter, A to U".to_string());
        }
        self.at += 1;
        if self.peek() != b'"' {
            return Err("USR of a string wants one letter, A to U".to_string());
        }
        self.at += 1;
        Ok(())
    }

    /// A slice of a string that is written out: `"abcde"(2 TO 4)`.
    fn slice(&mut self) -> Result<(), String> {
        self.at += 1;
        if self.peek() == b')' {
            self.at += 1;
            return Ok(());
        }
        if self.peek() != TO {
            self.index()?;
            if self.peek() == b')' {
                self.at += 1;
                return Ok(());
            }
        }
        if self.peek() != TO {
            return Err(format!("expected \"TO\", but got {}", self.name(self.peek())));
        }
        self.at += 1;
        if self.peek() != b')' {
            self.index()?;
        }
        self.need(b')')
    }
}

#[cfg(test)]
mod tests {
    use super::super::source::{tokenise_line, SourceOptions};
    use super::*;

    const OPTS: SourceOptions = SourceOptions { basic128: false, any_case: false, check_syntax: false };

    /// The line as the tokeniser writes it, checked.
    #[track_caller]
    fn check(text: &str) -> Result<(), String> {
        let line = tokenise_line(text, OPTS).unwrap_or_else(|e| panic!("{text}: {e}"));
        check_line(&line[4..], false)
    }

    #[track_caller]
    fn bad(text: &str) -> String {
        check(text).unwrap_err()
    }

    #[test]
    fn a_statement_starts_with_a_command() {
        check("10 CLS").unwrap();
        assert!(bad("10 a=1").contains("does not start a statement"));
        assert!(bad("10 PI").contains("does not start a statement"));
        // The empty statement the ROM allows, and the one it does not end.
        check("10 IF a=1 THEN : PRINT 1").unwrap();
        assert!(bad("10 CLS PRINT 1").contains("is not where a statement ends"));
        check("10 CLS: PRINT 1: STOP").unwrap();
    }

    #[test]
    fn the_operands_a_command_wants() {
        check("10 GO TO 100").unwrap();
        assert!(bad("10 GO TO").contains("stops short"));
        assert!(bad("10 GO TO :").contains("stops short"));
        check("10 RANDOMIZE").unwrap(); // a number may follow, or nothing
        check("10 RANDOMIZE USR 32768").unwrap();
        check("10 POKE 23609,50").unwrap();
        assert!(bad("10 POKE 23609").contains("expected \",\""));
        assert!(bad("10 POKE 23609;50").contains("expected \",\""));
        check("10 FOR n=1 TO 10").unwrap();
        check("10 FOR n=1 TO 10 STEP 2").unwrap();
        assert!(bad("10 FOR n=1 THEN 10").contains("expected \"TO\""));
        assert!(bad("10 FOR name=1 TO 10").contains("one letter"));
        check("10 NEXT n").unwrap();
        // DRAW takes two or three, CIRCLE always three.
        check("10 DRAW 10,20").unwrap();
        check("10 DRAW 10,20,1.5").unwrap();
        check("10 CIRCLE 128,88,40").unwrap();
        assert!(bad("10 CIRCLE 128,88").contains("stops short"));
    }

    #[test]
    fn expressions_keep_their_type() {
        check("10 LET a=1").unwrap();
        check("10 LET a$=\"x\"").unwrap();
        check("10 LET a$=CHR$ 65+\"x\"").unwrap();
        check("10 LET a=LEN b$").unwrap();
        assert!(bad("10 LET a=\"x\"").contains("number variable takes a number"));
        assert!(bad("10 LET a$=1").contains("string variable takes a string"));
        assert!(bad("10 PRINT AND b").contains("wants a value on its left"));
        assert!(bad("10 LET a=1+\"x\"").contains("two types meet"));
        assert!(bad("10 LET a=LEN 5").contains("expected a string"));
        assert!(bad("10 PRINT CHR$ \"a\"").contains("expected a number"));
        // An equation is a number whatever it compares.
        check("10 LET a=(b$=\"x\")").unwrap();
        check("10 IF a$<>\"\" AND b>1 THEN GO TO 10").unwrap();
        assert!(bad("10 IF a$ OR b THEN CLS").contains("OR wants a number"));
        // USR takes a letter as well as an address.
        check("10 PRINT USR \"a\"").unwrap();
        assert!(bad("10 PRINT USR \"ab\"").contains("one letter"));
    }

    #[test]
    fn variables_arrays_and_slices() {
        check("10 LET score=score+1").unwrap();
        assert!(bad("10 LET ab$=\"x\"").contains("one letter"));
        check("10 LET a$(2)=\"x\"").unwrap();
        check("10 PRINT a$(2 TO 4)").unwrap();
        check("10 PRINT a$( TO 4)").unwrap();
        check("10 PRINT a$(2 TO )").unwrap();
        check("10 PRINT \"abcde\"(2 TO 4)").unwrap();
        check("10 PRINT b(1,2)").unwrap();
        assert!(bad("10 PRINT a$()").contains("no index"));
        assert!(bad("10 PRINT a(2 TO 4)").contains("only a string can be sliced"));
        assert!(bad("10 PRINT a$(\"x\")").contains("an index is a number"));
        assert!(bad("10 NEXT n(1)").contains("not a slice"));
        check("10 DIM a$(10,32)").unwrap();
        check("10 READ a,b$,c(2)").unwrap();
        assert!(bad("10 READ 1").contains("not a variable"));
    }

    #[test]
    fn the_items_print_takes() {
        check("10 PRINT").unwrap();
        check("10 PRINT AT 1,2;\"x\";TAB 5;a").unwrap();
        check("10 PRINT INK 2;PAPER 7;\"x\"").unwrap();
        check("10 PRINT #2;\"x\"").unwrap();
        check("10 PRINT \"a\",\"b\"'\"c\"").unwrap();
        check("10 INPUT \"name? \";a$").unwrap();
        check("10 INPUT LINE a$").unwrap();
        assert!(bad("10 INPUT LINE a").contains("INPUT LINE reads into a string"));
        assert!(bad("10 PRINT AT 1").contains("expected \",\""));
        // AT and TAB belong to PRINT, not to anything that wants a number.
        assert!(bad("10 LET a=AT 1,2").contains("only PRINT takes"));
        check("10 PLOT INK 1;10,20").unwrap();
        assert!(bad("10 PLOT INK 1,10,20").contains("expected \";\""));
    }

    #[test]
    fn functions_take_the_tightest_operand() {
        check("10 LET a=PEEK 23627+256*PEEK 23628").unwrap();
        check("10 RANDOMIZE USR (PEEK 23627+256*PEEK 23628)").unwrap();
        check("10 PRINT INT (a/2)").unwrap();
        check("10 PRINT SCREEN$ (1,2)").unwrap();
        check("10 PRINT ATTR (1,2)+1").unwrap();
        assert!(bad("10 PRINT ATTR 1,2").contains("expected \"(\""));
        check("10 PRINT FN s(2,3)").unwrap();
        check("10 DEF FN s(a,b)=a+b*2.5").unwrap();
        check("10 DEF FN p()=PI").unwrap();
        assert!(bad("10 DEF FN sq(x)=x").contains("one letter"));
        assert!(bad("10 DEF FN s(x)").contains("expected \"=\""));
    }

    #[test]
    fn the_cassette_commands() {
        check("10 LOAD \"\"").unwrap();
        check("10 LOAD \"name\"CODE").unwrap();
        check("10 LOAD \"\"CODE 32768").unwrap();
        check("10 LOAD \"\"CODE 32768,1024").unwrap();
        check("10 LOAD \"\"SCREEN$").unwrap();
        check("10 SAVE \"name\"LINE 10").unwrap();
        check("10 SAVE \"name\"DATA a()").unwrap();
        check("10 VERIFY \"\"").unwrap();
        assert!(bad("10 LOAD CODE").contains("wants a name"));
        assert!(bad("10 MERGE \"\"CODE").contains("MERGE reads a program"));
        assert!(bad("10 LOAD \"\"LINE 10").contains("not a kind of file"));
        // Interface 1 and disc syntax is not 48K BASIC: it goes unread.
        check("10 LOAD *\"m\";1;\"name\"").unwrap();
        check("10 OPEN #4;\"b\"").unwrap();
        check("10 CAT 1").unwrap();
    }

    #[test]
    fn brackets_and_strings_have_to_close() {
        assert!(bad("10 PRINT (1+2").contains("bracket is left open"));
        assert!(bad("10 PRINT 1+2)").contains("never opened"));
        check("10 PRINT ((1+2)*3)").unwrap();
        check("10 REM PRINT (((").unwrap(); // REM takes the rest of the line
                                            // A colon or a keyword inside a string is only text.
        check("10 PRINT \"a: LET\"").unwrap();
        let deep = format!("10 PRINT {}1{}", "(".repeat(60), ")".repeat(60));
        assert!(bad(&deep).contains("nests deeper"));
    }

    /// The shapes a loader is written in, which are what an edit is most likely
    /// to be of.
    #[test]
    fn the_lines_a_loader_is_made_of() {
        for text in [
            "10 CLEAR VAL \"24999\"",
            "20 PAPER 0: INK 0: BORDER 0: CLS",
            "30 PRINT AT 11,6;INK 6;\"PRESS ANY KEY\"",
            "40 IF INKEY$=\"\" THEN GO TO 40",
            "50 LOAD \"\"SCREEN$ : LOAD \"\"CODE 25000",
            "60 POKE 23739,111: RANDOMIZE USR 25000",
            "70 FOR f=0 TO 21: READ a: POKE USR \"a\"+f,a: NEXT f",
            "80 DATA 60,66,129,129,129,129,66,60",
            "90 LET a$=a$( TO LEN a$-1)+CHR$ (CODE b$+1)",
            "100 PRINT #0;AT 0,0;FLASH 1;\"WAIT\";FLASH 0",
            "110 DEF FN r(n)=INT (RND*n)+1",
            "120 IF FN r(6)>3 AND NOT a THEN LET s=s+VAL \"10\": GO SUB 9000",
            "130 PLOT 0,0: DRAW OVER 1;255,175",
            "140 INPUT \"Name? \";LINE n$",
            "150 DIM q(10,10): DIM n$(3,8)",
            "160 SAVE \"game\"CODE 25000,1000: VERIFY \"game\"CODE",
            "170 BEEP .05,n*2: PAUSE 0: RETURN",
            "180 IF a=1 THEN LET b=2: GO TO 30",
            "190 LET a=b(1)*c(2,3)",
            "200 INK 2: PAPER 7: FLASH 0",
            "210 LET a$=STR$ (n*2)+\"!\"",
            "220 PRINT \"x\";: PRINT \"y\"",
            "230 RESTORE 100: READ a$,b",
            "240 LET x=INT (RND*10)+1",
            "250 IF NOT a THEN RETURN",
            "260 FOR i=1 TO LEN a$: PRINT a$(i);: NEXT i",
            "270 POKE 23606,PEEK 23606+1",
            "280 LET a=VAL \"1e3\": LET b$=VAL$ \"a$\"",
            "290 LPRINT TAB 5;\"x\"",
            "300 PAUSE 50: CLS : GO SUB 200",
            "310 PRINT AT 0,0;",
            "320 LET a=(b>c)-(b<c)",
            "330 IF a$(1)=\"y\" OR a$(1)=\"Y\" THEN GO TO 10",
            "340 PLOT 0,0: DRAW 255,0: DRAW 0,175",
            "350 OUT 254,7: BORDER 7",
            "360 LET n=CODE INKEY$",
            "370 PRINT POINT (1,2);ATTR (3,4);SCREEN$ (5,6)",
            "9999 REM (C) nobody at all: GO TO \" not BASIC at all",
        ] {
            check(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        }
    }

    #[test]
    fn the_128k_keywords_are_only_that_with_128k_basic() {
        let basic128 = SourceOptions { basic128: true, ..OPTS };
        let line = tokenise_line("10 PLAY \"abc\"", basic128).unwrap();
        check_line(&line[4..], true).unwrap();
        // The same bytes are the graphics T and U to a 48K program.
        assert!(check_line(&line[4..], false).unwrap_err().contains("does not start a statement"));
    }
}
