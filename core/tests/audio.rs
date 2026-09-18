//! Audio tests: the playback flow, the pulses each block makes, the sample
//! rendering and the WAV encoding. `test/core.test.ts` compares these against
//! the TypeScript they replaced, sample for sample.

use tapeti_core::audio::{
    block_duration, decode_csw_rle, emit_block, encode_wav, playback_order, playback_timeline, render_length,
    render_tape, tape_duration, CountingSink, FlowOptions, PulseSink, RecordingSink, RenderOptions,
    LEAD_TSTATES, TSTATES_PER_SEC,
};
use tapeti_core::types::{create_body, Block, Body, SymDef};

fn b(body: Body) -> Block {
    Block::new(body)
}

fn block(id: u8) -> Block {
    Block::new(create_body(id))
}

fn square(sample_rate: u32) -> RenderOptions {
    RenderOptions { sample_rate, mic: false, amplitude: 0.8 }
}

#[test]
fn follows_loops_jumps_and_calls() {
    // A loop of three passes over one block.
    let mut blocks = vec![block(0x24), block(0x12), block(0x25), block(0x20)];
    if let Body::LoopStart { count } = &mut blocks[0].body {
        *count = 3;
    }
    assert_eq!(playback_order(&blocks, FlowOptions::default()), vec![0, 1, 2, 1, 2, 1, 2, 3]);

    // A call sequence visits each target and comes back.
    let blocks = vec![
        b(Body::Call { offsets: vec![2, 3] }),
        b(Body::Pause { pause: 1 }),
        b(Body::PureTone { pulse_len: 1, count: 1 }),
        b(Body::Return),
    ];
    // Each target is played, then the block after the call - and the walk keeps
    // going from there, so the tail of the tape plays again.
    assert_eq!(playback_order(&blocks, FlowOptions::default()), vec![0, 2, 3, 3, 1, 2, 3]);

    // "Stop the tape" ends playback; a 48k stop only when asked.
    let stop = vec![b(Body::Pause { pause: 0 }), b(Body::PureTone { pulse_len: 1, count: 1 })];
    assert_eq!(playback_order(&stop, FlowOptions::default()), vec![0]);
    let stop48 = vec![b(Body::Stop48), b(Body::PureTone { pulse_len: 1, count: 1 })];
    assert_eq!(playback_order(&stop48, FlowOptions::default()), vec![0, 1]);
    assert_eq!(playback_order(&stop48, FlowOptions { stop_at_48k: true, max_steps: None }), vec![0]);

    // A jump to itself is caught by the step limit rather than hanging.
    let spin = vec![b(Body::Jump { offset: 0 })];
    assert_eq!(playback_order(&spin, FlowOptions { stop_at_48k: false, max_steps: Some(10) }).len(), 10);
}

#[test]
fn emits_the_pulses_a_block_makes() {
    // A pure tone is its pulses, alternating level.
    let tone = b(Body::PureTone { pulse_len: 100, count: 3 });
    let mut sink = RecordingSink::default();
    emit_block(&mut sink, &tone);
    assert_eq!(sink.pulses, vec![(100, 0), (100, 1), (100, 0)]);
    assert_eq!(block_duration(&tone), 300);

    // A standard block leads with a pilot, two sync pulses and two per bit.
    let std = b(Body::Standard { pause: 0, data: vec![0x00, 0x00] });
    let mut sink = RecordingSink::default();
    emit_block(&mut sink, &std);
    assert_eq!(sink.pulses.len(), 8063 + 2 + 16 * 2);
    assert_eq!(sink.pulses[0].0, 2168);
    assert_eq!(sink.pulses[8063].0, 667);
    assert_eq!(sink.pulses[8064].0, 735);
    assert_eq!(sink.pulses[8065].0, 855); // a zero bit

    // A pause holds the line, and forces it low after a millisecond of high.
    let mut sink = RecordingSink::default();
    sink.set_level(1);
    emit_block(&mut sink, &b(Body::Pause { pause: 10 }));
    assert_eq!(sink.pulses, vec![(3500, 1), (35000 - 3500, 0)]);
    assert_eq!(block_duration(&b(Body::Pause { pause: 10 })), 35000);

    // Setting the signal level makes no pulse at all.
    let mut sink = RecordingSink::default();
    emit_block(&mut sink, &b(Body::SignalLevel { level: 1 }));
    assert!(sink.pulses.is_empty());
    assert_eq!(sink.level, 1);

    // A generalized block plays its pilot runs and then its data symbols.
    let gen = b(Body::Generalized {
        pause: 0,
        totp: 1,
        npp: 1,
        pilot_symbols: vec![SymDef { flags: 0, pulses: vec![1000] }],
        pilot_stream: vec![tapeti_core::types::PilotRun { symbol: 0, reps: 2 }],
        totd: 8,
        npd: 1,
        data_symbols: vec![SymDef { flags: 0, pulses: vec![500] }, SymDef { flags: 0, pulses: vec![900] }],
        data: vec![0b1010_1010],
    });
    let mut sink = RecordingSink::default();
    emit_block(&mut sink, &gen);
    assert_eq!(sink.pulses.len(), 2 + 8);
    assert_eq!(sink.pulses[2].0, 900); // first data bit is a one
    assert_eq!(sink.pulses[3].0, 500);

    // Unknown blocks are silent.
    let mut sink = CountingSink::default();
    emit_block(&mut sink, &b(Body::Unknown { id: 0x5b, raw: vec![1, 2, 3] }));
    assert_eq!(sink.total, 0);
}

#[test]
fn decodes_csw_run_lengths() {
    assert_eq!(decode_csw_rle(&[3, 4, 0, 0x10, 0x27, 0, 0, 7]), vec![3, 4, 10000, 7]);
    // A long run cut short stops rather than reading past the end.
    assert_eq!(decode_csw_rle(&[0, 1, 2]), Vec::<u32>::new());
}

#[test]
fn renders_samples_and_wav() {
    // Half a second of lead-in, a tone, half a second of lead-out.
    let tone = b(Body::PureTone { pulse_len: 35000, count: 2 }); // 10 ms per pulse
    let blocks = [tone];
    let order = playback_order(&blocks, FlowOptions::default());
    let samples = render_tape(&blocks, square(8000), &order);
    // The count is exact to within the sample the fractional accumulator may owe.
    let predicted = render_length(&blocks, 8000, &order) as i64;
    assert!((samples.len() as i64 - predicted).abs() <= 1, "{} vs {predicted}", samples.len());
    // The lead-in is silence at the low level, which is -1 times the amplitude.
    assert_eq!(samples[0], -0.8);
    // The tone's first pulse is high.
    let after_lead = (LEAD_TSTATES as f64 / (f64::from(TSTATES_PER_SEC) / 8000.0)) as usize;
    assert_eq!(samples[after_lead + 1], -0.8);
    assert_eq!(samples[after_lead + 90], 0.8);

    // MIC mode decays after an edge where square mode holds.
    let mic = render_tape(&blocks, RenderOptions { sample_rate: 8000, mic: true, amplitude: 0.8 }, &order);
    assert!(mic[after_lead + 90].abs() < 0.8);

    // The timeline says where each block starts, lead-in included.
    let timeline = playback_timeline(&blocks, &order);
    assert_eq!(timeline.starts, vec![LEAD_TSTATES]);
    assert_eq!(timeline.total, LEAD_TSTATES * 2 + 70000);
    let (seconds, _) = tape_duration(&blocks);
    assert!((seconds - 0.02).abs() < 1e-9);

    // A WAV file starts with its header and holds two bytes per sample.
    let wav = encode_wav(&samples, 8000, 16);
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(wav.len(), 44 + samples.len() * 2);
    let wav8 = encode_wav(&samples, 8000, 8);
    assert_eq!(wav8.len(), 44 + samples.len());
    // Full scale in both directions, and the clamping outside it.
    let edges = [-1.5f32, -1.0, 0.0, 1.0, 1.5];
    let wav = encode_wav(&edges, 8000, 16);
    assert_eq!(&wav[44..], &[0x01, 0x80, 0x01, 0x80, 0, 0, 0xff, 0x7f, 0xff, 0x7f]);
}
