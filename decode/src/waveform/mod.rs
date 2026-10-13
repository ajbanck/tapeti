// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! Work on a recording's samples: the band-pass filter that cleans hiss and hum off them, and
//! the edge detector that finds the pulses in them.

pub mod edges;
pub mod filter;
