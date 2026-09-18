//! The two things the native side must bring that the core does not have:
//! inflating Z-RLE CSW blocks, and the binary search over a timeline the caller
//! already holds.
//!
//! The core is dependency-free on purpose — it compiles to a 249 kB wasm module
//! — so zlib lives out here, exactly as it does on the web side, where
//! `src/tzx/audio.ts` inflates with pako before calling in. Everything that
//! turns a tape into pulses goes through [`playable`] first.

use std::borrow::Cow;

use tapeti_core::audio::{self, RenderOptions, Timeline};
use tapeti_core::types::{Block, Body};

/// RLE bytes of a CSW block; Z-RLE (compression 2) is inflated with zlib. A
/// corrupt block plays silence rather than garbage, which is what the web build
/// decides too.
fn csw_rle_data(data: &[u8]) -> Vec<u8> {
    miniz_oxide::inflate::decompress_to_vec_zlib(data).unwrap_or_default()
}

/// The tape as the core should see it: compressed CSW blocks with their data
/// already inflated. A tape without them is borrowed, not copied.
pub fn playable(blocks: &[Block]) -> Cow<'_, [Block]> {
    let needs = blocks.iter().any(|b| matches!(&b.body, Body::Csw { compression: 2, .. }));
    if !needs {
        return Cow::Borrowed(blocks);
    }
    Cow::Owned(
        blocks
            .iter()
            .map(|b| match &b.body {
                Body::Csw { pause, sample_rate, compression: 2, pulse_count, data } => Block {
                    uid: b.uid,
                    body: Body::Csw {
                        pause: *pause,
                        sample_rate: *sample_rate,
                        compression: 1,
                        pulse_count: *pulse_count,
                        data: csw_rle_data(data),
                    },
                },
                _ => b.clone(),
            })
            .collect(),
    )
}

/// Total T-states for a block in isolation.
pub fn block_duration(b: &Block) -> u64 {
    let one = [b.clone()];
    audio::block_duration(&playable(&one)[0])
}

/// Duration in seconds of the whole tape, following playback flow.
pub fn tape_duration(blocks: &[Block]) -> (f64, Vec<u32>) {
    audio::tape_duration(&playable(blocks))
}

pub fn playback_timeline(blocks: &[Block], order: &[u32]) -> Timeline {
    audio::playback_timeline(&playable(blocks), order)
}

pub fn render_tape(blocks: &[Block], opts: RenderOptions, order: &[u32]) -> Vec<f32> {
    audio::render_tape(&playable(blocks), opts, order)
}

/// Index into the playback order of the block playing at `tstates` (the first
/// block during the lead-in). Stays out of the core for the same reason
/// `positionAt` stays in TypeScript: it is a binary search over an array the
/// caller already holds, run once per frame while the tape plays.
pub fn position_at(starts: &[u64], tstates: u64) -> usize {
    if starts.is_empty() {
        return 0;
    }
    let (mut lo, mut hi) = (0usize, starts.len() - 1);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if starts[mid] <= tstates {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_block_playing_at_a_moment() {
        let starts = [0u64, 100, 250, 900];
        assert_eq!(position_at(&starts, 0), 0);
        assert_eq!(position_at(&starts, 99), 0);
        assert_eq!(position_at(&starts, 100), 1);
        assert_eq!(position_at(&starts, 899), 2);
        assert_eq!(position_at(&starts, 100_000), 3);
        assert_eq!(position_at(&[], 5), 0);
    }

    #[test]
    fn leaves_a_tape_without_compressed_csw_alone() {
        let blocks = vec![Block::new(Body::Pause { pause: 100 })];
        assert!(matches!(playable(&blocks), Cow::Borrowed(_)));
    }
}
