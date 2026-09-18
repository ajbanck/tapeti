//! Changing a block's type while keeping whatever fields carry over (the block
//! editor's type menu). The port of `src/tzx/convert.ts`.
//!
//! The TypeScript copies fields by name — "if the new type has `pause` and the
//! old one had a number there, carry it". Here that becomes an explicit read
//! and write per field, which is longer but says out loud what carries over.

use crate::types::{create_body, Block, Body, PilotRun, RomTimings, SymDef};

/// The numeric fields that carry over between types, in the order the
/// TypeScript lists them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Pause,
    UsedBits,
    Pilot,
    Sync1,
    Sync2,
    Zero,
    One,
    PilotLen,
}

const CARRIED: [Field; 8] = [
    Field::Pause,
    Field::UsedBits,
    Field::Pilot,
    Field::Sync1,
    Field::Sync2,
    Field::Zero,
    Field::One,
    Field::PilotLen,
];

fn get(body: &Body, f: Field) -> Option<u16> {
    Some(match (f, body) {
        (Field::Pause, Body::Standard { pause, .. })
        | (Field::Pause, Body::Turbo { pause, .. })
        | (Field::Pause, Body::PureData { pause, .. })
        | (Field::Pause, Body::Direct { pause, .. })
        | (Field::Pause, Body::Csw { pause, .. })
        | (Field::Pause, Body::Generalized { pause, .. })
        | (Field::Pause, Body::Pause { pause }) => *pause,
        (Field::UsedBits, Body::Turbo { used_bits, .. })
        | (Field::UsedBits, Body::PureData { used_bits, .. })
        | (Field::UsedBits, Body::Direct { used_bits, .. }) => *used_bits as u16,
        (Field::Pilot, Body::Turbo { pilot, .. }) => *pilot,
        (Field::Sync1, Body::Turbo { sync1, .. }) => *sync1,
        (Field::Sync2, Body::Turbo { sync2, .. }) => *sync2,
        (Field::Zero, Body::Turbo { zero, .. }) | (Field::Zero, Body::PureData { zero, .. }) => *zero,
        (Field::One, Body::Turbo { one, .. }) | (Field::One, Body::PureData { one, .. }) => *one,
        (Field::PilotLen, Body::Turbo { pilot_len, .. }) => *pilot_len,
        _ => return None,
    })
}

fn set(body: &mut Body, f: Field, v: u16) {
    match (f, body) {
        (Field::Pause, Body::Standard { pause, .. })
        | (Field::Pause, Body::Turbo { pause, .. })
        | (Field::Pause, Body::PureData { pause, .. })
        | (Field::Pause, Body::Direct { pause, .. })
        | (Field::Pause, Body::Csw { pause, .. })
        | (Field::Pause, Body::Generalized { pause, .. })
        | (Field::Pause, Body::Pause { pause }) => *pause = v,
        (Field::UsedBits, Body::Turbo { used_bits, .. })
        | (Field::UsedBits, Body::PureData { used_bits, .. })
        | (Field::UsedBits, Body::Direct { used_bits, .. }) => *used_bits = v as u8,
        (Field::Pilot, Body::Turbo { pilot, .. }) => *pilot = v,
        (Field::Sync1, Body::Turbo { sync1, .. }) => *sync1 = v,
        (Field::Sync2, Body::Turbo { sync2, .. }) => *sync2 = v,
        (Field::Zero, Body::Turbo { zero, .. }) | (Field::Zero, Body::PureData { zero, .. }) => *zero = v,
        (Field::One, Body::Turbo { one, .. }) | (Field::One, Body::PureData { one, .. }) => *one = v,
        (Field::PilotLen, Body::Turbo { pilot_len, .. }) => *pilot_len = v,
        _ => {}
    }
}

/// The data payload of a block, for the types that carry one.
fn data_of(body: &Body) -> Option<&Vec<u8>> {
    match body {
        Body::Standard { data, .. }
        | Body::Turbo { data, .. }
        | Body::PureData { data, .. }
        | Body::Direct { data, .. }
        | Body::Csw { data, .. }
        | Body::Generalized { data, .. }
        | Body::Custom { data, .. } => Some(data),
        _ => None,
    }
}

fn set_data(body: &mut Body, value: Vec<u8>) {
    match body {
        Body::Standard { data, .. }
        | Body::Turbo { data, .. }
        | Body::PureData { data, .. }
        | Body::Direct { data, .. }
        | Body::Csw { data, .. }
        | Body::Generalized { data, .. }
        | Body::Custom { data, .. } => *data = value,
        _ => {}
    }
}

/// Text-like fields: description/message text and group names carry over both ways.
fn text_of(body: &Body) -> Option<&String> {
    match body {
        Body::Text { text } | Body::Message { text, .. } => Some(text),
        _ => None,
    }
}

fn name_of(body: &Body) -> Option<&String> {
    match body {
        Body::GroupStart { name } => Some(name),
        _ => None,
    }
}

/// ROM pilot length for a standard block: headers (flag < 128) use the longer pilot.
fn rom_pilot_len(data: &[u8]) -> u16 {
    if !data.is_empty() && data[0] < 128 {
        RomTimings::PILOT_HEADER
    } else {
        RomTimings::PILOT_DATA
    }
}

fn data_symbols(zero: u16, one: u16) -> Vec<SymDef> {
    vec![SymDef { flags: 0, pulses: vec![zero, zero] }, SymDef { flags: 0, pulses: vec![one, one] }]
}

fn rom_pilot(reps: u16) -> (Vec<SymDef>, Vec<PilotRun>) {
    (
        vec![
            SymDef { flags: 0, pulses: vec![RomTimings::PILOT] },
            SymDef { flags: 0, pulses: vec![RomTimings::SYNC1, RomTimings::SYNC2] },
        ],
        vec![PilotRun { symbol: 0, reps }, PilotRun { symbol: 1, reps: 1 }],
    )
}

/// Convert `b` to block type `id`, keeping its uid and every field the new type
/// shares with the old one (pause, data, used bits, timings, text). Unknown
/// blocks are returned unchanged.
pub fn convert_block(b: &Block, id: u8) -> Block {
    if b.body.is_unknown() || b.id() == id {
        return b.clone();
    }
    let mut body = create_body(id);
    for f in CARRIED {
        if let Some(v) = get(&b.body, f) {
            set(&mut body, f, v);
        }
    }
    let old_data = data_of(&b.body).cloned();
    if let Some(data) = old_data.clone() {
        set_data(&mut body, data);
    }
    if id == 0x11 && b.id() == 0x10 {
        if let Body::Turbo { pilot_len, .. } = &mut body {
            *pilot_len = rom_pilot_len(old_data.as_deref().unwrap_or(&[]));
        }
    }
    if id == 0x19 {
        if let Some(data) = &old_data {
            let bits = data.len() as u32 * 8;
            let (symbols, stream, datasyms) = match &b.body {
                Body::Turbo { pilot, sync1, sync2, zero, one, pilot_len, .. } => (
                    Some(vec![
                        SymDef { flags: 0, pulses: vec![*pilot] },
                        SymDef { flags: 0, pulses: vec![*sync1, *sync2] },
                    ]),
                    Some(vec![PilotRun { symbol: 0, reps: *pilot_len }, PilotRun { symbol: 1, reps: 1 }]),
                    Some(data_symbols(*zero, *one)),
                ),
                Body::Standard { .. } => {
                    let (s, st) = rom_pilot(rom_pilot_len(data));
                    (Some(s), Some(st), None)
                }
                Body::PureData { zero, one, .. } => (None, None, Some(data_symbols(*zero, *one))),
                _ => (None, None, None),
            };
            if let Body::Generalized {
                totd, totp, npp, pilot_symbols, pilot_stream, data_symbols: ds, ..
            } = &mut body
            {
                *totd = bits;
                if let (Some(s), Some(st)) = (symbols, stream) {
                    *npp = 2;
                    *totp = 2;
                    *pilot_symbols = s;
                    *pilot_stream = st;
                }
                if let Some(d) = datasyms {
                    *ds = d;
                }
            }
        }
    }
    // Text-like fields carry over in both directions.
    if let Some(text) = text_of(&b.body).or_else(|| name_of(&b.body)) {
        match &mut body {
            Body::Text { text: t } | Body::Message { text: t, .. } => *t = text.clone(),
            _ => {}
        }
    }
    if let Some(text) = text_of(&b.body) {
        if let Body::GroupStart { name } = &mut body {
            *name = text.clone();
        }
    }
    Block { uid: b.uid, body }
}
