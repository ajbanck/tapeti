// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Tape recordings to `tapeti-core` blocks. No I/O, and no dependency but `tapeti-core`.

pub mod capture;
pub mod entry;
pub mod pulses;
pub mod readers;
pub mod waveform;
