//! Pure TZX/TAP data layer, the Rust port of `src/tzx/`. No I/O, no UI.
//!
//! The block model and the parser,
//! plus the wasm ABI (`wasm`) and the byte format (`wire`) the TypeScript app
//! calls them through.

pub mod audio;
pub mod bits;
pub mod bytes;
pub mod compare;
pub mod consistency;
pub mod content;
pub mod convert;
pub mod describe;
pub mod dump;
pub mod parser;
pub mod pokes;
pub mod programs;
pub mod spectrum;
pub mod types;
pub mod wasm;
pub mod wire;
pub mod writer;
