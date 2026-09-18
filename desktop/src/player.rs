//! Playback, the port of `src/state/player.ts`.
//!
//! Web Audio takes a whole `AudioBuffer` and plays it; cpal takes a callback and
//! asks for frames. The rendered tape therefore lives in an `Arc<[f32]>` that
//! the audio thread reads and a cursor the UI thread watches — the same three
//! numbers the web build exposes as signals (`playing`, `playingBlock`,
//! `playPos`), read here as one `Progress`.
//!
//! The tape is rendered at the device's own sample rate, so nothing resamples.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Error as CpalError, SampleFormat};
use tapeti_core::audio::{RenderOptions, TSTATES_PER_SEC};
use tapeti_core::types::Block;

use crate::state::Side;
use crate::tape::{playback_timeline, position_at, render_tape};

/// What the status bar and the block list read while a tape plays.
#[derive(Clone, Copy, Default)]
pub struct Progress {
    pub playing: bool,
    /// Which pane's tape is playing, so only its list shows the marker.
    pub side: Option<Side>,
    /// Block index in the tape, or -1.
    pub block: i32,
    pub elapsed: f64,
    pub total: f64,
}

struct Shared {
    /// Samples handed to the device so far.
    at: AtomicUsize,
    done: AtomicBool,
}

pub struct Player {
    /// Dropping the stream stops playback; kept only while something plays.
    stream: Option<cpal::Stream>,
    shared: Arc<Shared>,
    side: Option<Side>,
    order: Vec<u32>,
    starts: Vec<u64>,
    rate: u32,
    total_samples: usize,
    block: i32,
}

impl Default for Player {
    fn default() -> Self {
        Player {
            stream: None,
            shared: Arc::new(Shared { at: AtomicUsize::new(0), done: AtomicBool::new(true) }),
            side: None,
            order: Vec::new(),
            starts: Vec::new(),
            rate: 44_100,
            total_samples: 0,
            block: -1,
        }
    }
}

impl Player {
    pub fn playing(&self) -> bool {
        self.stream.is_some() && !self.shared.done.load(Ordering::Relaxed)
    }

    /// Called once a frame: advances the block marker and notices the end of the
    /// tape, the way the `requestAnimationFrame` tick does on the web.
    pub fn poll(&mut self) -> Progress {
        if self.stream.is_none() {
            return Progress::default();
        }
        if self.shared.done.load(Ordering::Relaxed) {
            self.stop();
            return Progress::default();
        }
        let at = self.shared.at.load(Ordering::Relaxed).min(self.total_samples);
        let elapsed = at as f64 / f64::from(self.rate);
        let tstates = (elapsed * f64::from(TSTATES_PER_SEC)) as u64;
        self.block = self.order.get(position_at(&self.starts, tstates)).map_or(-1, |i| *i as i32);
        Progress {
            playing: true,
            side: self.side,
            block: self.block,
            elapsed,
            total: self.total_samples as f64 / f64::from(self.rate),
        }
    }

    /// Render `order` of `blocks` and start playing it. Returns what to put in
    /// the status bar when there is nothing to play or no device to play on.
    pub fn play(
        &mut self,
        blocks: &[Block],
        order: Vec<u32>,
        side: Option<Side>,
        mic: bool,
    ) -> Option<String> {
        self.stop();
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            return Some("No audio output device".into());
        };
        let config = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => return Some(format!("No audio output configuration: {e}")),
        };
        let rate = config.sample_rate();
        let channels = config.channels() as usize;
        let opts = RenderOptions { sample_rate: rate, mic, amplitude: 1.0 };
        let samples: Arc<[f32]> = render_tape(blocks, opts, &order).into();
        if samples.is_empty() {
            return Some("Nothing to play".into());
        }
        let timeline = playback_timeline(blocks, &order);

        let shared = Arc::new(Shared { at: AtomicUsize::new(0), done: AtomicBool::new(false) });
        let stream = match build_stream(&device, &config, samples.clone(), channels, shared.clone()) {
            Ok(s) => s,
            Err(e) => return Some(format!("Cannot play: {e}")),
        };
        if let Err(e) = stream.play() {
            return Some(format!("Cannot play: {e}"));
        }
        self.total_samples = samples.len();
        self.rate = rate;
        self.order = order;
        self.starts = timeline.starts;
        self.side = side;
        self.shared = shared;
        self.stream = Some(stream);
        self.block = -1;
        None
    }

    pub fn stop(&mut self) {
        self.stream = None;
        self.side = None;
        self.block = -1;
        self.order.clear();
        self.starts.clear();
        self.total_samples = 0;
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    samples: Arc<[f32]>,
    channels: usize,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, CpalError> {
    let err = |e| eprintln!("audio: {e}");
    let cfg: cpal::StreamConfig = config.config();
    // One mono buffer fanned out to every channel; `next` is the frame cursor
    // the UI reads back as elapsed time.
    macro_rules! stream {
        ($t:ty, $conv:expr) => {{
            let shared = shared.clone();
            let samples = samples.clone();
            let conv: fn(f32) -> $t = $conv;
            device.build_output_stream(
                cfg.clone(),
                move |out: &mut [$t], _: &cpal::OutputCallbackInfo| {
                    let mut at = shared.at.load(Ordering::Relaxed);
                    for frame in out.chunks_mut(channels) {
                        let v = samples.get(at).copied().unwrap_or(0.0);
                        for s in frame.iter_mut() {
                            *s = conv(v);
                        }
                        at += 1;
                    }
                    shared.at.store(at, Ordering::Relaxed);
                    if at >= samples.len() {
                        shared.done.store(true, Ordering::Relaxed);
                    }
                },
                err,
                None,
            )
        }};
    }
    match config.sample_format() {
        SampleFormat::F32 => stream!(f32, |v| v),
        SampleFormat::I16 => stream!(i16, |v| (v.clamp(-1.0, 1.0) * 32767.0) as i16),
        SampleFormat::U16 => stream!(u16, |v| ((v.clamp(-1.0, 1.0) * 32767.0) as i32 + 32768) as u16),
        SampleFormat::I32 => stream!(i32, |v| (f64::from(v.clamp(-1.0, 1.0)) * 2_147_483_647.0) as i32),
        other => Err(CpalError::with_message(
            cpal::ErrorKind::InvalidInput,
            format!("unsupported sample format {other:?}"),
        )),
    }
}
