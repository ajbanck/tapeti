//! TZX block model, the port of `src/tzx/types.ts`.
//!
//! Where TypeScript puts `id` and `uid` on every block object, Rust splits the
//! two: [`Block`] holds the `uid` the UI uses for selection and keys, and
//! [`Body`] is the tagged union, with the block ID implied by the variant (and
//! carried explicitly only by [`Body::Unknown`]). `isUnknown` therefore becomes
//! a match, so the `UnknownBlock.id` narrowing trap from CLAUDE.md cannot
//! happen here.
//!
//! Only the block model is ported in stage 0. `BLOCK_NAMES`, `CREATABLE_IDS`,
//! the hardware tables and `createBlock` are UI-facing and come with the later
//! stages.

use std::sync::atomic::{AtomicU32, Ordering};

pub type Uid = u32;

static NEXT_UID: AtomicU32 = AtomicU32::new(1);

pub fn new_uid() -> Uid {
    NEXT_UID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SymDef {
    /// b0-b1 polarity
    pub flags: u8,
    /// Pulse lengths; shorter symbols are padded with 0 when written.
    pub pulses: Vec<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PilotRun {
    pub symbol: u8,
    pub reps: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectEntry {
    /// Relative, signed.
    pub offset: i16,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub kind: u8,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HardwareEntry {
    pub kind: u8,
    pub id: u8,
    pub info: u8,
}

/// Standard ROM loader timings.
pub struct RomTimings;
impl RomTimings {
    pub const PILOT: u16 = 2168;
    pub const SYNC1: u16 = 667;
    pub const SYNC2: u16 = 735;
    pub const ZERO: u16 = 855;
    pub const ONE: u16 = 1710;
    pub const PILOT_HEADER: u16 = 8063;
    pub const PILOT_DATA: u16 = 3223;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// 0x10 Standard speed data
    Standard { pause: u16, data: Vec<u8> },
    /// 0x11 Turbo speed data
    Turbo {
        pilot: u16,
        sync1: u16,
        sync2: u16,
        zero: u16,
        one: u16,
        pilot_len: u16,
        used_bits: u8,
        pause: u16,
        data: Vec<u8>,
    },
    /// 0x12 Pure tone
    PureTone { pulse_len: u16, count: u16 },
    /// 0x13 Pulse sequence
    PulseSeq { pulses: Vec<u16> },
    /// 0x14 Pure data
    PureData { zero: u16, one: u16, used_bits: u8, pause: u16, data: Vec<u8> },
    /// 0x15 Direct recording
    Direct { tstates: u16, pause: u16, used_bits: u8, data: Vec<u8> },
    /// 0x18 CSW recording
    Csw {
        pause: u16,
        sample_rate: u32,
        /// 1 RLE, 2 Z-RLE
        compression: u8,
        pulse_count: u32,
        data: Vec<u8>,
    },
    /// 0x19 Generalized data
    Generalized {
        pause: u16,
        totp: u32,
        npp: u8,
        /// Length = asp (0 -> 256)
        pilot_symbols: Vec<SymDef>,
        /// Length = totp
        pilot_stream: Vec<PilotRun>,
        totd: u32,
        npd: u8,
        /// Length = asd
        data_symbols: Vec<SymDef>,
        data: Vec<u8>,
    },
    /// 0x20 Pause / Stop the tape
    Pause { pause: u16 },
    /// 0x21 Group start
    GroupStart { name: String },
    /// 0x22 Group end
    GroupEnd,
    /// 0x23 Jump to block
    Jump { offset: i16 },
    /// 0x24 Loop start
    LoopStart { count: u16 },
    /// 0x25 Loop end
    LoopEnd,
    /// 0x26 Call sequence
    Call { offsets: Vec<i16> },
    /// 0x27 Return from sequence
    Return,
    /// 0x28 Select block
    Select { entries: Vec<SelectEntry> },
    /// 0x2a Stop the tape if in 48K mode
    Stop48,
    /// 0x2b Set signal level
    SignalLevel { level: u8 },
    /// 0x30 Text description
    Text { text: String },
    /// 0x31 Message
    Message { time: u8, text: String },
    /// 0x32 Archive info
    Archive { entries: Vec<ArchiveEntry> },
    /// 0x33 Hardware type
    Hardware { entries: Vec<HardwareEntry> },
    /// 0x35 Custom info
    Custom {
        /// 16 chars, space padded
        ident: String,
        data: Vec<u8>,
    },
    /// 0x5a Glue, 9 bytes
    Glue { raw: Vec<u8> },
    /// Anything the editor does not model, kept verbatim so it round-trips:
    /// the body bytes after the ID byte, including any length prefix.
    Unknown { id: u8, raw: Vec<u8> },
}

impl Body {
    pub fn id(&self) -> u8 {
        match self {
            Body::Standard { .. } => 0x10,
            Body::Turbo { .. } => 0x11,
            Body::PureTone { .. } => 0x12,
            Body::PulseSeq { .. } => 0x13,
            Body::PureData { .. } => 0x14,
            Body::Direct { .. } => 0x15,
            Body::Csw { .. } => 0x18,
            Body::Generalized { .. } => 0x19,
            Body::Pause { .. } => 0x20,
            Body::GroupStart { .. } => 0x21,
            Body::GroupEnd => 0x22,
            Body::Jump { .. } => 0x23,
            Body::LoopStart { .. } => 0x24,
            Body::LoopEnd => 0x25,
            Body::Call { .. } => 0x26,
            Body::Return => 0x27,
            Body::Select { .. } => 0x28,
            Body::Stop48 => 0x2a,
            Body::SignalLevel { .. } => 0x2b,
            Body::Text { .. } => 0x30,
            Body::Message { .. } => 0x31,
            Body::Archive { .. } => 0x32,
            Body::Hardware { .. } => 0x33,
            Body::Custom { .. } => 0x35,
            Body::Glue { .. } => 0x5a,
            Body::Unknown { id, .. } => *id,
        }
    }

    /// `isDataBlock`: blocks that turn bytes into pulses.
    pub fn is_data_block(&self) -> bool {
        matches!(
            self,
            Body::Standard { .. }
                | Body::Turbo { .. }
                | Body::PureData { .. }
                | Body::Direct { .. }
                | Body::Generalized { .. }
        )
    }

    /// `hasData`: blocks with a byte payload the data window can view/edit.
    pub fn has_data(&self) -> bool {
        self.is_data_block() || matches!(self, Body::Custom { .. } | Body::Csw { .. })
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Body::Unknown { .. })
    }

    /// The payload of a block that [`Body::has_data`], if any.
    pub fn data(&self) -> Option<&[u8]> {
        match self {
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
}

/// Block types the user can create from the UI, in menu order.
pub const CREATABLE_IDS: [u8; 25] = [
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x18, 0x19, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28,
    0x2a, 0x2b, 0x30, 0x31, 0x32, 0x33, 0x35, 0x5a,
];

/// A fresh block of the given id with sensible defaults, the port of
/// `createBlock`. An id the editor does not model becomes an empty unknown
/// block, as it does there.
pub fn create_body(id: u8) -> Body {
    match id {
        0x10 => Body::Standard { pause: 1000, data: Vec::new() },
        0x11 => Body::Turbo {
            pilot: RomTimings::PILOT,
            sync1: RomTimings::SYNC1,
            sync2: RomTimings::SYNC2,
            zero: RomTimings::ZERO,
            one: RomTimings::ONE,
            pilot_len: RomTimings::PILOT_DATA,
            used_bits: 8,
            pause: 1000,
            data: Vec::new(),
        },
        0x12 => Body::PureTone { pulse_len: 2168, count: 8063 },
        0x13 => Body::PulseSeq { pulses: vec![667, 735] },
        0x14 => Body::PureData { zero: 855, one: 1710, used_bits: 8, pause: 1000, data: Vec::new() },
        0x15 => Body::Direct { tstates: 79, pause: 0, used_bits: 8, data: Vec::new() },
        0x18 => Body::Csw { pause: 0, sample_rate: 44100, compression: 1, pulse_count: 0, data: Vec::new() },
        0x19 => Body::Generalized {
            pause: 1000,
            totp: 0,
            npp: 2,
            pilot_symbols: Vec::new(),
            pilot_stream: Vec::new(),
            totd: 0,
            npd: 2,
            data_symbols: vec![
                SymDef { flags: 0, pulses: vec![855, 855] },
                SymDef { flags: 0, pulses: vec![1710, 1710] },
            ],
            data: Vec::new(),
        },
        0x20 => Body::Pause { pause: 1000 },
        0x21 => Body::GroupStart { name: "Group".to_string() },
        0x22 => Body::GroupEnd,
        0x23 => Body::Jump { offset: 1 },
        0x24 => Body::LoopStart { count: 2 },
        0x25 => Body::LoopEnd,
        0x26 => Body::Call { offsets: vec![1] },
        0x27 => Body::Return,
        0x28 => Body::Select { entries: vec![SelectEntry { offset: 1, text: "Selection".to_string() }] },
        0x2a => Body::Stop48,
        0x2b => Body::SignalLevel { level: 0 },
        0x30 => Body::Text { text: String::new() },
        0x31 => Body::Message { time: 5, text: String::new() },
        0x32 => Body::Archive { entries: vec![ArchiveEntry { kind: 0, text: String::new() }] },
        0x33 => Body::Hardware { entries: vec![HardwareEntry { kind: 0, id: 1, info: 0 }] },
        0x35 => Body::Custom { ident: "Custom          ".to_string(), data: Vec::new() },
        0x5a => Body::Glue { raw: vec![0x58, 0x54, 0x61, 0x70, 0x65, 0x21, 0x1a, 1, 20] },
        _ => Body::Unknown { id, raw: Vec::new() },
    }
}

/// The spec's name for a block ID, as `BLOCK_NAMES` in `types.ts` lists them.
/// Only the ones the descriptions need; the UI keeps the full table until it
/// moves to Rust in stage 4.
pub fn block_name(id: u8) -> Option<&'static str> {
    Some(match id {
        0x10 => "Standard speed data",
        0x11 => "Turbo speed data",
        0x12 => "Pure tone",
        0x13 => "Pulse sequence",
        0x14 => "Pure data",
        0x15 => "Direct recording",
        0x16 => "C64 ROM type data (deprecated)",
        0x17 => "C64 turbo data (deprecated)",
        0x18 => "CSW recording",
        0x19 => "Generalized data",
        0x20 => "Pause / Stop the tape",
        0x21 => "Group start",
        0x22 => "Group end",
        0x23 => "Jump to block",
        0x24 => "Loop start",
        0x25 => "Loop end",
        0x26 => "Call sequence",
        0x27 => "Return from sequence",
        0x28 => "Select block",
        0x2a => "Stop the tape if in 48K mode",
        0x2b => "Set signal level",
        0x30 => "Text description",
        0x31 => "Message",
        0x32 => "Archive info",
        0x33 => "Hardware type",
        0x34 => "Emulation info (deprecated)",
        0x35 => "Custom info",
        0x40 => "Snapshot (deprecated)",
        0x5a => "Glue",
        _ => return None,
    })
}

/// A block as the app holds it: a body plus the uid the UI keys on.
#[derive(Clone, Debug)]
pub struct Block {
    pub uid: Uid,
    pub body: Body,
}

impl Block {
    /// A block with a fresh uid.
    pub fn new(body: Body) -> Self {
        Block { uid: new_uid(), body }
    }

    pub fn id(&self) -> u8 {
        self.body.id()
    }

    /// Deep copy with a new uid, the port of `cloneBlock`. (`deepClone` has no
    /// counterpart: `Clone` is the deep copy here.)
    pub fn clone_fresh(&self) -> Self {
        Block::new(self.body.clone())
    }
}

/// Blocks compare by content; uids are identity, not data, and the tests strip
/// them for exactly this reason.
impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        self.body == other.body
    }
}
impl Eq for Block {}
