//! TZX/TAP data layer: block model, parsing and writing. No I/O, no UI, no dependencies.
//!
//! The desktop app links this crate directly. The web app reaches it through the `wasm` ABI
//! and the `wire` byte format, whose other end is `web/src/tzx/core.ts` and `web/src/tzx/wire.ts`.

pub mod audio;
pub mod bits;
pub mod bytes;
pub mod compare;
pub mod consistency;
pub mod content;
pub mod convert;
pub mod describe;
pub mod dump;
pub mod hash;
pub mod parser;
pub mod pokes;
pub mod programs;
pub mod snapshot;
pub mod spectrum;
pub mod tables;
pub mod types;
pub mod wasm;
pub mod wire;
pub mod writer;
