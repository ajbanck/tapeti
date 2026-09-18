//! Turning a block list into an edge/pulse stream and then into PCM samples,
//! the port of `src/tzx/audio.ts`. Loops, jumps, calls and returns are followed
//! exactly like an emulator would.
//!
//! Z-RLE CSW blocks arrive already inflated: the crate has no dependencies, so
//! the app does that with pako before handing the tape over (see
//! `src/tzx/audio.ts`).

use crate::parser::bits_per_symbol;
use crate::types::{Block, Body, SymDef};

pub const TSTATES_PER_SEC: u32 = 3_500_000;

/// Silence before and after the rendered tape, in T-states.
pub const LEAD_TSTATES: u64 = TSTATES_PER_SEC as u64 / 2;

/// JavaScript's `Math.round`: halves go up, not away from zero.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

pub trait PulseSink {
    /// Add a pulse of `tstates` length at the current level, then toggle the level.
    fn pulse(&mut self, tstates: u64);
    /// Hold the current level for the given time without toggling.
    fn hold(&mut self, tstates: u64);
    fn set_level(&mut self, level: u8);
    fn level(&self) -> u8;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FlowOptions {
    /// Stop at a "stop the tape if in 48k mode" block.
    pub stop_at_48k: bool,
    /// Safety valve for infinite loops via jumps.
    pub max_steps: Option<u32>,
}

/// Walk the tape in playback order, yielding the block index sequence.
pub fn playback_order(blocks: &[Block], opts: FlowOptions) -> Vec<u32> {
    let mut order: Vec<u32> = Vec::new();
    let max_steps = opts.max_steps.unwrap_or(100_000);
    let mut loop_stack: Vec<(i64, u32)> = Vec::new(); // start, remaining
    let mut call_stack: Vec<(i64, usize)> = Vec::new(); // block, next
    let mut i: i64 = 0;
    let mut steps = 0u32;
    while i >= 0 && (i as usize) < blocks.len() && steps < max_steps {
        steps += 1;
        let b = &blocks[i as usize];
        order.push(i as u32);
        match &b.body {
            Body::Jump { offset } => {
                i += i64::from(*offset);
                continue;
            }
            Body::LoopStart { count } => loop_stack.push((i, u32::from(*count))),
            Body::LoopEnd => {
                if let Some(l) = loop_stack.last_mut() {
                    l.1 = l.1.wrapping_sub(1);
                    if l.1 > 0 {
                        i = l.0 + 1;
                        continue;
                    }
                    loop_stack.pop();
                }
            }
            Body::Call { offsets } => {
                if !offsets.is_empty() {
                    call_stack.push((i, 0));
                    i += i64::from(offsets[0]);
                    continue;
                }
            }
            Body::Return => {
                if let Some(c) = call_stack.last_mut() {
                    c.1 += 1;
                    let (block, next) = (c.0, c.1);
                    let offsets = match &blocks[block as usize].body {
                        Body::Call { offsets } => offsets.clone(),
                        _ => Vec::new(),
                    };
                    if next < offsets.len() {
                        i = block + i64::from(offsets[next]);
                    } else {
                        call_stack.pop();
                        i = block + 1;
                    }
                    continue;
                }
            }
            Body::Stop48 => {
                if opts.stop_at_48k {
                    return order;
                }
            }
            // A pause of zero is "stop the tape".
            Body::Pause { pause: 0 } => return order,
            _ => {}
        }
        i += 1;
    }
    order
}

fn data_bits(data: &[u8], used_bits: u8) -> usize {
    if data.is_empty() {
        return 0;
    }
    (data.len() - 1) * 8 + used_bits.clamp(1, 8) as usize
}

fn bit_at(data: &[u8], i: usize) -> u8 {
    (data[i >> 3] >> (7 - (i & 7))) & 1
}

fn emit_pause(sink: &mut dyn PulseSink, ms: u16) {
    if ms == 0 {
        return;
    }
    // Per spec: a pause forces the level low after at least 1 ms at the current level.
    let t = u64::from(ms) * (TSTATES_PER_SEC as u64 / 1000);
    if sink.level() == 1 {
        sink.pulse(t.min(3500)); // end current high pulse after ~1ms
        sink.hold(t.saturating_sub(3500));
    } else {
        sink.hold(t);
    }
}

fn emit_symbol(sink: &mut dyn PulseSink, sym: &SymDef) {
    match sym.flags & 3 {
        1 => {
            // same as current level: no edge -> first pulse prolongs previous; emulate by holding
            if let Some(first) = sym.pulses.first() {
                sink.hold(u64::from(*first));
                for p in &sym.pulses[1..] {
                    sink.pulse(u64::from(*p));
                }
            }
            return;
        }
        2 => sink.set_level(0),
        3 => sink.set_level(1),
        _ => {}
    }
    for p in &sym.pulses {
        if *p > 0 {
            sink.pulse(u64::from(*p));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_standard(
    sink: &mut dyn PulseSink,
    pilot: u16,
    sync1: u16,
    sync2: u16,
    zero: u16,
    one: u16,
    pilot_len: u16,
    data: &[u8],
    used_bits: u8,
    pause: u16,
) {
    for _ in 0..pilot_len {
        sink.pulse(u64::from(pilot));
    }
    if sync1 != 0 {
        sink.pulse(u64::from(sync1));
    }
    if sync2 != 0 {
        sink.pulse(u64::from(sync2));
    }
    for i in 0..data_bits(data, used_bits) {
        let len = u64::from(if bit_at(data, i) != 0 { one } else { zero });
        sink.pulse(len);
        sink.pulse(len);
    }
    emit_pause(sink, pause);
}

/// Decode CSW v2 RLE pulse lengths (in samples). Z-RLE must be inflated first.
pub fn decode_csw_rle(data: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let v = data[i];
        i += 1;
        if v != 0 {
            out.push(u32::from(v));
        } else {
            if i + 4 > data.len() {
                break;
            }
            out.push(u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]));
            i += 4;
        }
    }
    out
}

/// Emit the pulses of one block (no flow control).
pub fn emit_block(sink: &mut dyn PulseSink, b: &Block) {
    match &b.body {
        Body::Standard { pause, data } => {
            let is_header = data.first().is_some_and(|f| *f < 128);
            let pilot_len = if is_header { 8063 } else { 3223 };
            emit_standard(sink, 2168, 667, 735, 855, 1710, pilot_len, data, 8, *pause);
        }
        Body::Turbo { pilot, sync1, sync2, zero, one, pilot_len, used_bits, pause, data } => {
            emit_standard(sink, *pilot, *sync1, *sync2, *zero, *one, *pilot_len, data, *used_bits, *pause);
        }
        Body::PureTone { pulse_len, count } => {
            for _ in 0..*count {
                sink.pulse(u64::from(*pulse_len));
            }
        }
        Body::PulseSeq { pulses } => {
            for p in pulses {
                sink.pulse(u64::from(*p));
            }
        }
        Body::PureData { zero, one, used_bits, pause, data } => {
            emit_standard(sink, 0, 0, 0, *zero, *one, 0, data, *used_bits, *pause);
        }
        Body::Direct { tstates, pause, used_bits, data } => {
            let bits = data_bits(data, *used_bits);
            let mut run: u64 = 0;
            let mut cur = sink.level();
            for i in 0..bits {
                let bit = bit_at(data, i);
                if bit != cur {
                    if run > 0 {
                        if sink.level() != cur {
                            sink.set_level(cur);
                        }
                        sink.hold(run * u64::from(*tstates));
                    }
                    cur = bit;
                    run = 0;
                }
                run += 1;
            }
            if run > 0 {
                if sink.level() != cur {
                    sink.set_level(cur);
                }
                sink.hold(run * u64::from(*tstates));
            }
            emit_pause(sink, *pause);
        }
        Body::Csw { pause, sample_rate, data, .. } => {
            let rate = if *sample_rate == 0 { 44100.0 } else { f64::from(*sample_rate) };
            let scale = f64::from(TSTATES_PER_SEC) / rate;
            for p in decode_csw_rle(data) {
                sink.pulse(js_round(f64::from(p) * scale) as u64);
            }
            emit_pause(sink, *pause);
        }
        Body::Generalized { pause, totp, pilot_symbols, pilot_stream, totd, data_symbols, data, .. } => {
            if *totp > 0 {
                for run in pilot_stream {
                    let Some(sym) = pilot_symbols.get(usize::from(run.symbol)) else { continue };
                    for _ in 0..run.reps {
                        emit_symbol(sink, sym);
                    }
                }
            }
            if *totd > 0 && !data_symbols.is_empty() {
                let nb = bits_per_symbol(data_symbols.len() as u32) as usize;
                let mut bit_pos = 0usize;
                for _ in 0..*totd {
                    let mut code = 0usize;
                    for _ in 0..nb {
                        let byte = data.get(bit_pos >> 3).copied().unwrap_or(0);
                        code = (code << 1) | usize::from((byte >> (7 - (bit_pos & 7))) & 1);
                        bit_pos += 1;
                    }
                    if let Some(sym) = data_symbols.get(code) {
                        emit_symbol(sink, sym);
                    }
                }
            }
            emit_pause(sink, *pause);
        }
        Body::Pause { pause } => emit_pause(sink, *pause),
        Body::SignalLevel { level } => sink.set_level(if *level != 0 { 1 } else { 0 }),
        _ => {}
    }
}

/// Counts T-states without producing anything.
#[derive(Default)]
pub struct CountingSink {
    pub total: u64,
    pub level: u8,
}

impl PulseSink for CountingSink {
    fn pulse(&mut self, t: u64) {
        self.total += t;
        self.level = 1 - self.level;
    }
    fn hold(&mut self, t: u64) {
        self.total += t;
    }
    fn set_level(&mut self, level: u8) {
        self.level = level;
    }
    fn level(&self) -> u8 {
        self.level
    }
}

/// Records every pulse, for the callers that want the edge stream itself.
#[derive(Default)]
pub struct RecordingSink {
    /// `(tstates, level during those tstates)`.
    pub pulses: Vec<(u64, u8)>,
    pub level: u8,
}

impl PulseSink for RecordingSink {
    fn pulse(&mut self, t: u64) {
        self.pulses.push((t, self.level));
        self.level = 1 - self.level;
    }
    fn hold(&mut self, t: u64) {
        self.pulses.push((t, self.level));
    }
    fn set_level(&mut self, level: u8) {
        self.level = level;
    }
    fn level(&self) -> u8 {
        self.level
    }
}

/// Total T-states for a block in isolation.
pub fn block_duration(b: &Block) -> u64 {
    let mut sink = CountingSink::default();
    emit_block(&mut sink, b);
    sink.total
}

/// Duration in seconds of the whole tape following playback flow.
pub fn tape_duration(blocks: &[Block]) -> (f64, Vec<u32>) {
    let order = playback_order(blocks, FlowOptions::default());
    let mut sink = CountingSink::default();
    for i in &order {
        emit_block(&mut sink, &blocks[*i as usize]);
    }
    (sink.total as f64 / f64::from(TSTATES_PER_SEC), order)
}

#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    pub sample_rate: u32,
    /// false = plain square wave, true = the Spectrum MIC output response.
    pub mic: bool,
    pub amplitude: f64,
}

/// Renders pulses straight into a sample buffer in [-1, 1].
pub struct SampleSink {
    pub level: u8,
    out: Vec<f32>,
    n: usize,
    /// Fractional sample accumulator, in T-states.
    acc: f64,
    t_per_sample: f64,
    amp: f64,
    mic: bool,
    alpha: f64,
    prev_in: f64,
    prev_out: f64,
}

impl SampleSink {
    /// `capacity` is the expected sample count; the buffer grows if it is exceeded.
    pub fn new(opts: RenderOptions, capacity: usize) -> Self {
        // MIC emulation: the Spectrum MIC socket is AC-coupled through a small capacitor,
        // so a step decays exponentially. Model as a first-order high-pass filter.
        let rc = 0.0013; // seconds, roughly matches the observed decay
        let dt = 1.0 / f64::from(opts.sample_rate);
        SampleSink {
            level: 0,
            out: vec![0.0; capacity],
            n: 0,
            acc: 0.0,
            t_per_sample: f64::from(TSTATES_PER_SEC) / f64::from(opts.sample_rate),
            amp: opts.amplitude,
            mic: opts.mic,
            alpha: rc / (rc + dt),
            prev_in: 0.0,
            prev_out: 0.0,
        }
    }

    fn emit(&mut self, tstates: u64) {
        self.acc += tstates as f64;
        let n = (self.acc / self.t_per_sample).floor();
        self.acc -= n * self.t_per_sample;
        if n <= 0.0 {
            return;
        }
        let n = n as usize;
        let end = self.n + n;
        if end > self.out.len() {
            self.out.resize(end.max(self.out.len() * 2).max(1024), 0.0);
        }
        let x = if self.level != 0 { 1.0 } else { -1.0 };
        if !self.mic {
            let v = (x * self.amp) as f32;
            self.out[self.n..end].fill(v);
        } else {
            for i in self.n..end {
                let y = self.alpha * (self.prev_out + x - self.prev_in);
                self.prev_in = x;
                self.prev_out = y;
                self.out[i] = (y.clamp(-1.0, 1.0) * self.amp) as f32;
            }
        }
        self.n = end;
    }

    /// Number of samples written so far.
    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn finish(mut self) -> Vec<f32> {
        self.out.truncate(self.n);
        self.out
    }
}

impl PulseSink for SampleSink {
    fn pulse(&mut self, t: u64) {
        self.emit(t);
        self.level = 1 - self.level;
    }
    fn hold(&mut self, t: u64) {
        self.emit(t);
    }
    fn set_level(&mut self, level: u8) {
        self.level = level;
    }
    fn level(&self) -> u8 {
        self.level
    }
}

/// Where each block of the playback order starts, without rendering.
pub struct Timeline {
    /// T-state at which each entry of `order` starts playing (after the lead-in).
    pub starts: Vec<u64>,
    /// Total T-states including lead-in and lead-out; equals the rendered length.
    pub total: u64,
}

pub fn playback_timeline(blocks: &[Block], order: &[u32]) -> Timeline {
    let mut sink = CountingSink::default();
    let mut starts = Vec::with_capacity(order.len());
    sink.hold(LEAD_TSTATES);
    for i in order {
        starts.push(sink.total);
        emit_block(&mut sink, &blocks[*i as usize]);
    }
    sink.hold(LEAD_TSTATES);
    Timeline { starts, total: sink.total }
}

/// Sample count `render_tape` will produce for the given order, without rendering.
pub fn render_length(blocks: &[Block], sample_rate: u32, order: &[u32]) -> usize {
    let total = playback_timeline(blocks, order).total as f64;
    (total / (f64::from(TSTATES_PER_SEC) / f64::from(sample_rate))).floor() as usize
}

pub fn render_tape(blocks: &[Block], opts: RenderOptions, order: &[u32]) -> Vec<f32> {
    // Counting first lets the sink allocate the whole buffer once.
    let mut sink = SampleSink::new(opts, render_length(blocks, opts.sample_rate, order) + 1);
    sink.hold(LEAD_TSTATES);
    for i in order {
        emit_block(&mut sink, &blocks[*i as usize]);
    }
    sink.hold(LEAD_TSTATES);
    sink.finish()
}

/// Encode samples as a 16- or 8-bit mono PCM WAV file.
pub fn encode_wav(samples: &[f32], sample_rate: u32, bits: u16) -> Vec<u8> {
    let bytes_per_sample = usize::from(bits / 8);
    let data_len = samples.len() * bytes_per_sample;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * bytes_per_sample as u32).to_le_bytes());
    out.extend_from_slice(&(bytes_per_sample as u16).to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        let s = f64::from(*s).clamp(-1.0, 1.0);
        if bits == 16 {
            let v = js_round(s * 32767.0) as i32 as i16;
            out.extend_from_slice(&v.to_le_bytes());
        } else {
            out.push(js_round((s + 1.0) * 127.5) as i32 as u8);
        }
    }
    out
}
