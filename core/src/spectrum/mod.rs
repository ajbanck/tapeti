// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The Spectrum side of the app: the character set, the screen, the BASIC
//! lister, BASIC as text (`source`) and the Z80 disassembler.

pub mod basic;
pub mod charset;
pub mod romnames;
pub mod screen;
pub mod source;
pub mod syntax;
pub mod z80dis;
