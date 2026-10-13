// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! OUT: an emulator's trace of the Z80's `OUT` instructions, five bytes each.
//!
//! A record is a 16-bit T-state counter, a 16-bit port and the value written. A loader makes
//! the tape signal by writing to port `FE`, so each write to a port whose low byte is `FE` is
//! an edge, and the T-states between two edges are a pulse. A counter of `FFFF` marks a wrap:
//! its port field holds the T-states up to the wrap, and the counter starts again from 0. A
//! counter of `FFFE` is a record to ignore. The file has no signature; it is known by its
//! `.out` extension.

use super::ReadError;

const FORMAT: &str = "OUT";
const RECORD: usize = 5;
const WRAP: u16 = 0xFFFF;
const IGNORE: u16 = 0xFFFE;
const TAPE_PORT: u8 = 0xFE;

/// The length of every pulse in T-states. What follows the last edge is the last pulse.
pub(super) fn read(bytes: &[u8]) -> Result<Vec<u64>, ReadError> {
    let malformed = |what: String| ReadError::Malformed { format: FORMAT, what };
    if !bytes.len().is_multiple_of(RECORD) {
        return Err(malformed(format!("{} bytes is not a whole number of records", bytes.len())));
    }
    let mut pulses = Vec::new();
    // `last` is the counter at the last record or wrap, `run` the T-states since the last edge.
    let (mut last, mut run) = (0u16, 0u64);
    for (n, record) in bytes.as_chunks::<RECORD>().0.iter().enumerate() {
        let counter = u16::from_le_bytes([record[0], record[1]]);
        let port = u16::from_le_bytes([record[2], record[3]]);
        let backwards = |to: u16| malformed(format!("record {n} goes back in time, from {last} to {to}"));
        match counter {
            IGNORE => {}
            WRAP => {
                run += u64::from(port.checked_sub(last).ok_or_else(|| backwards(port))?);
                last = 0;
            }
            _ => {
                run += u64::from(counter.checked_sub(last).ok_or_else(|| backwards(counter))?);
                last = counter;
                if record[2] == TAPE_PORT {
                    pulses.push(run);
                    run = 0;
                }
            }
        }
    }
    if pulses.is_empty() {
        return Err(malformed("there is no write to the tape port".to_string()));
    }
    if run > 0 {
        pulses.push(run);
    }
    Ok(pulses)
}
