use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::dsp::Result;
use maac::graph::{GraphProcessor, InstrumentProgram};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use maac::wavetable::{TableBank, Wavetable};
use maac::PlanArtifact;
use std::collections::BTreeMap;
use std::sync::Arc;

#[path = "support/programs.rs"]
mod programs;

const RATE: f64 = 48_000.0;

/// An envelope `id` that is a gate.
fn gate(id: &str) -> String {
    format!(
        "node {id} {{ type = \"synth.adsr/1\"; params = {{ attack = 0s; decay = 0s; sustain = 1; release = 0s; }}; }}\n"
    )
}

/// A modulation of `parameter` of the envelope `amp`.
fn modulation(id: &str, from: &str, parameter: &str, depth: &str) -> String {
    format!(
        "modulate {id} {{ from = &{from}:out; to = &amp.params.{parameter}; depth = {depth}; }}\n"
    )
}

/// The two edges of depth zero from `source`: one read at note-on, one at
/// note-off.
fn zero_edges() -> String {
    modulation("on_zero", "source", "attack", "0s")
        + &modulation("off_zero", "source", "release", "0s")
}

/// The params of a sine at `frequency` Hz, whatever the note, and `phase`.
fn fixed_sine(frequency: i64, phase: &str) -> String {
    format!("params = {{ ratio = 0; frequency = {frequency}Hz; phase = {phase}; level = 1; }};")
}

/// A one-channel instrument whose voice holds `children` and renders the
/// node `output` under the envelope `amplitude`.
fn voice_program(amplitude: &str, output: &str, children: &str) -> InstrumentProgram {
    programs::program(&format!(
        r#"instrument edge_source {{
 channels = 1;
 voice voice {{
  channels = 1; amplitude = &{amplitude}; output = &{output}:out;
  {children}
 }}
}}"#
    ))
}

fn make_runtime(program: &InstrumentProgram, capacity: u32) -> Result<InstrumentRuntime> {
    make_runtime_with_tables(program, capacity, &BTreeMap::new())
}

fn make_runtime_with_tables(
    program: &InstrumentProgram,
    capacity: u32,
    tables: &BTreeMap<String, Arc<TableBank>>,
) -> Result<InstrumentRuntime> {
    let compiled = CompiledInstrument::compile(program, tables)?;
    InstrumentRuntime::new(Arc::new(compiled), capacity, RATE, &BTreeMap::new())
}

/// A gated voice that renders `source`, a node of type `kind` with `fields`,
/// and holds `modulations`.
fn source_program(kind: &str, fields: &str, modulations: &str) -> InstrumentProgram {
    voice_program(
        "amp",
        "source",
        &format!(
            "{}node source {{ type = \"{kind}\"; {fields} }}\n{modulations}",
            gate("amp")
        ),
    )
}

/// `source_program` with a wavetable oscillator for `source`. The bank of its
/// table is built here, not read from an asset, so the oscillator of a
/// resolved program is pointed at it.
fn wavetable_fixture(edges: bool) -> (InstrumentProgram, BTreeMap<String, Arc<TableBank>>) {
    let table = Wavetable {
        id: "cycle".into(),
        cycle_length: 8,
        samples: vec![0.0, 1.0, 0.0, -1.0, 0.0, 1.0, 0.0, -1.0],
    };
    let bank = Arc::new(TableBank::new(&table).unwrap());
    let tables = [("cycle".into(), bank)].into_iter().collect();
    let modulations = if edges { zero_edges() } else { String::new() };
    let mut program = source_program(
        "synth.sine/1",
        "params = { ratio = 1; frequency = 6000Hz; phase = 0; level = 1; };",
        &modulations,
    );
    let source = program
        .voice
        .nodes
        .iter_mut()
        .find(|node| node.id == "source")
        .unwrap();
    source.processor = GraphProcessor::Wavetable {
        table: "cycle".into(),
    };
    source
        .params
        .insert("position".into(), maac::parse_rational("0").unwrap());
    (program, tables)
}

fn lifecycle(program: &InstrumentProgram, off_frame: u64, frames: usize) -> Result<Vec<f64>> {
    lifecycle_with_tables(program, &BTreeMap::new(), off_frame, frames)
}

fn lifecycle_with_tables(
    program: &InstrumentProgram,
    tables: &BTreeMap<String, Arc<TableBank>>,
    off_frame: u64,
    frames: usize,
) -> Result<Vec<f64>> {
    let mut runtime = make_runtime_with_tables(program, 1, tables)?;
    runtime.note_on("voice", 440.0, 1.0, 0)?;
    let mut samples = Vec::with_capacity(frames);
    for frame in 0..frames as u64 {
        if frame == off_frame {
            runtime.note_off("voice", frame)?;
        }
        samples.push(runtime.render(frame)?[0]);
    }
    Ok(samples)
}

#[test]
fn note_on_captures_pressure_and_timbre_adsr_values_per_overlapping_voice() {
    let source = SourceBundle::new(
        "internal-event.maac",
        r#"maac 1;
project song { score = [0q, 1/6000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }
tempo clock { points = [(0q, 60bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument local {
 channels = 1;
 voice v {
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp { type = "synth.adsr/1"; params = { attack = 0s; decay = 0s; sustain = 0; release = 0s; }; }
  node osc { type = "synth.sine/1"; params = { ratio = 0; frequency = 0Hz; phase = 1/4; level = 1; }; }
  node pressure { type = "synth.pressure/1"; }
  node timbre { type = "synth.timbre/1"; }
  modulate attack_from_pressure { from = &pressure:out; to = &amp.params.attack; depth = 1/24000s; }
  modulate decay_from_timbre { from = &timbre:out; to = &amp.params.decay; depth = 1/12000s; }
  modulate sustain_from_pressure { from = &pressure:out; to = &amp.params.sustain; depth = 1; }
 }
}
node sound { instrument = &local; config = { voices = 2; }; }
curve pressure_a { clock = normalized; points = [(0, 1/2, step), (1/4, 0, step), (1, 0, step)]; }
curve pressure_b { clock = normalized; points = [(0, 1, step), (1/4, 0, step), (1, 0, step)]; }
curve timbre_a { clock = normalized; points = [(0, 1/2, step), (1/4, 1, step), (1, 1, step)]; }
curve timbre_b { clock = normalized; points = [(0, 1/4, step), (1/4, 1, step), (1, 1, step)]; }
pattern phrase {
 length = 1/6000q;
 note a { at = 0q; dur = 1/12000q; pitch = C4;
  expression p { kind = pressure; curve = &pressure_a; }
  expression t { kind = timbre; curve = &timbre_a; }
 }
 note b { at = 1/24000q; dur = 1/12000q; pitch = C4;
  expression p { kind = pressure; curve = &pressure_b; }
  expression t { kind = timbre; curve = &timbre_b; }
 }
}
track notes { target = &sound:events; }
place play { pattern = &phrase; track = &notes; at = 0q; }
"#,
    );
    let artifact = compile_bundle_artifact(&source).expect("event-rate source should compile");
    let retained = PlanArtifact::from_json(&artifact.to_json().unwrap()).unwrap();
    let mut samples = Vec::new();
    maac::render_artifact(&retained, |frame| {
        samples.push(frame[0]);
        Ok(())
    })
    .unwrap();

    // A is captured as attack=1 frame, decay=2 frames, sustain=1/2. B is
    // captured at its own onset as attack=2 frames, decay=1 frame, sustain=1.
    // The curves step during each gate, so any per-frame event recomputation
    // would change these values after onset.
    assert_eq!(samples.len(), 8);
    let expected = [0.0, 1.0, 0.75, 1.0, 1.0, 1.0, 0.0, 0.0];
    for (frame, (actual, expected)) in samples.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() < 1.0e-12,
            "frame {frame}: {actual} != {expected}"
        );
    }
}

#[test]
fn note_off_captures_filter_pre_state_then_advances_filter_once() {
    let program = voice_program(
        "amp",
        "filter",
        &[
            gate("driver"),
            "node filter { type = \"synth.onepole/1\"; config = { channels = 1; }; params = { cutoff = 12000Hz; }; }\n".to_owned(),
            gate("amp"),
            "connect driver_filter { from = &driver:out; to = &filter:in; }\n".to_owned(),
            "modulate filter_release { from = &filter:out; to = &amp.params.release; depth = 1/1000s; }\n".to_owned(),
        ]
        .concat(),
    );
    let mut runtime = make_runtime(&program, 1).unwrap();
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    const OFF: u64 = 3;
    let mut actual = Vec::new();
    for frame in 0..7 {
        if frame == OFF {
            runtime.note_off("voice", frame).unwrap();
        }
        actual.push(runtime.render(frame).unwrap()[0]);
    }

    let a = (-2.0 * std::f64::consts::PI * 12_000.0 / RATE).exp();
    let prefilter = 1.0 - a.powi((OFF + 1) as i32);
    let previous_filter = 1.0 - a.powi(OFF as i32);
    let release_frames = 48.0 * prefilter;
    for (frame, sample) in actual.iter().enumerate() {
        let expected = if frame < OFF as usize {
            1.0 - a.powi((frame + 1) as i32)
        } else {
            let elapsed = (frame - OFF as usize) as f64;
            let held_release = (1.0 - elapsed / release_frames).max(0.0);
            a.powi((frame - OFF as usize + 1) as i32) * previous_filter * held_release
        };
        assert!(
            (sample - expected).abs() < 1.0e-12,
            "frame {frame}: {sample} != {expected}"
        );
    }
    // The off-frame sample advances from the original state exactly once.
    // A preview incorrectly used as audio would report ypre, and a second
    // advancement would report a^2*(1-a^OFF).
    assert!((actual[OFF as usize] - a * previous_filter).abs() < 1.0e-12);
}

#[test]
fn note_off_reduces_all_adsr_releases_from_the_same_pre_release_snapshot() {
    let program = voice_program(
        "b",
        "source",
        &[
            gate("a"),
            gate("b"),
            format!(
                "node source {{ type = \"synth.sine/1\"; {} }}\n",
                fixed_sine(0, "1/4")
            ),
            "modulate a_to_b_release { from = &a:out; to = &b.params.release; depth = 1/1000s; }\n"
                .to_owned(),
        ]
        .concat(),
    );
    let mut runtime = make_runtime(&program, 1).unwrap();
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap(), &[1.0]);
    runtime.note_off("voice", 1).unwrap();

    // A's declared release is zero. B's release must nevertheless be nonzero
    // because it captures A's pre-release sustain value of one. Sequentially
    // releasing A before evaluating B would free the voice immediately.
    runtime.prune_finished(1).unwrap();
    assert_eq!(runtime.active_voice_count(), 1);
    assert_eq!(runtime.render(1).unwrap(), &[1.0]);
    let expected_frame_two = 1.0 - 1.0 / 48.0;
    assert!((runtime.render(2).unwrap()[0] - expected_frame_two).abs() < 1.0e-12);
}

#[test]
fn zero_depth_edges_preserve_stateful_source_audio_and_reset_repeat() {
    let sources = [
        (
            "synth.sine/1",
            "params = { ratio = 1; frequency = 440Hz; phase = 1/4; level = 1; };",
        ),
        (
            "synth.noise/1",
            "config = { seed = 1; per_note = false; }; params = { level = 1; };",
        ),
        (
            "synth.pluck/1",
            "config = { seed = 1; }; params = { ratio = 1; decay = 3s; damping = 1/2; level = 1; };",
        ),
    ];
    for (kind, fields) in sources {
        let edged = source_program(kind, fields, &zero_edges());
        let baseline = source_program(kind, fields, "");
        let with_edges = lifecycle(&edged, 2, 7).unwrap();
        let without_edges = lifecycle(&baseline, 2, 7).unwrap();
        assert_eq!(
            with_edges
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            without_edges
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            "zero-depth source changed audio for {kind}"
        );

        let mut repeated = make_runtime(&edged, 1).unwrap();
        repeated.note_on("voice", 440.0, 1.0, 0).unwrap();
        let first: Vec<f64> = (0..7)
            .map(|frame| {
                if frame == 2 {
                    repeated.note_off("voice", frame).unwrap();
                }
                repeated.render(frame).unwrap()[0]
            })
            .collect();
        repeated.reset(&BTreeMap::new()).unwrap();
        repeated.note_on("voice", 440.0, 1.0, 0).unwrap();
        let second: Vec<f64> = (0..7)
            .map(|frame| {
                if frame == 2 {
                    repeated.note_off("voice", frame).unwrap();
                }
                repeated.render(frame).unwrap()[0]
            })
            .collect();
        assert_eq!(
            first
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            second
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            "reset did not repeat {kind} source"
        );
    }
}

#[test]
fn zero_depth_wavetable_edges_preserve_phase_and_reset_repeat() {
    let (edged, edged_tables) = wavetable_fixture(true);
    let (baseline, baseline_tables) = wavetable_fixture(false);
    let with_edges = lifecycle_with_tables(&edged, &edged_tables, 2, 7).unwrap();
    let without_edges = lifecycle_with_tables(&baseline, &baseline_tables, 2, 7).unwrap();
    assert_eq!(
        with_edges
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>(),
        without_edges
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>()
    );

    let mut repeated = make_runtime_with_tables(&edged, 1, &edged_tables).unwrap();
    repeated.note_on("voice", 440.0, 1.0, 0).unwrap();
    let first: Vec<f64> = (0..7)
        .map(|frame| {
            if frame == 2 {
                repeated.note_off("voice", frame).unwrap();
            }
            repeated.render(frame).unwrap()[0]
        })
        .collect();
    repeated.reset(&BTreeMap::new()).unwrap();
    repeated.note_on("voice", 440.0, 1.0, 0).unwrap();
    let second: Vec<f64> = (0..7)
        .map(|frame| {
            if frame == 2 {
                repeated.note_off("voice", frame).unwrap();
            }
            repeated.render(frame).unwrap()[0]
        })
        .collect();
    assert_eq!(
        first
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>()
    );
}

/// A 12 kHz sine `source` at `phase` whose output sets the attack and the
/// release of `amp`.
fn event_validation_program(
    phase: &str,
    attack_depth: &str,
    release_depth: &str,
) -> InstrumentProgram {
    source_program(
        "synth.sine/1",
        &fixed_sine(12_000, phase),
        &(modulation("attack", "source", "attack", attack_depth)
            + &modulation("release", "source", "release", release_depth)),
    )
}

#[test]
fn event_sums_are_checked_at_edges_while_between_edge_changes_are_held() {
    let valid_between = event_validation_program("1/4", "1/48000s", "0s");
    let mut runtime = make_runtime(&valid_between, 1).unwrap();
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    // The source is +1 at onset, then reaches -1 at frame 2. Attack remains
    // the captured onset value, so the between-event negative would-be sum is
    // not validated as an event parameter.
    for frame in 0..4 {
        runtime.render(frame).unwrap();
    }

    let invalid_on = event_validation_program("3/4", "1/48000s", "0s");
    let mut invalid_on_runtime = make_runtime(&invalid_on, 1).unwrap();
    let error = invalid_on_runtime
        .note_on("voice", 440.0, 1.0, 0)
        .unwrap_err();
    assert_eq!(error.code(), "E_RANGE");

    let invalid_off = event_validation_program("1/4", "0s", "1/48000s");
    let mut runtime = make_runtime(&invalid_off, 1).unwrap();
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    runtime.render(0).unwrap();
    runtime.render(1).unwrap();
    assert_eq!(runtime.note_off("voice", 2).unwrap_err().code(), "E_RANGE");
}

#[test]
fn zero_depth_does_not_skip_source_parameter_validation() {
    // The library refuses a phase of two in source, so it is set on a
    // resolved program.
    let mut invalid = source_program("synth.sine/1", &fixed_sine(0, "0"), &zero_edges());
    let source = invalid
        .voice
        .nodes
        .iter_mut()
        .find(|node| node.id == "source")
        .unwrap();
    source
        .params
        .insert("phase".into(), maac::parse_rational("2").unwrap());
    let error = CompiledInstrument::compile(&invalid, &BTreeMap::new()).unwrap_err();
    assert_eq!(error.code(), "E_RANGE");
}

#[test]
fn zero_release_reuses_capacity_before_nonzero_release_tail() {
    let held = fixed_sine(0, "1/4");
    let zero = source_program(
        "synth.sine/1",
        &held,
        &modulation("release_zero", "source", "release", "0s"),
    );
    let mut runtime = make_runtime(&zero, 1).unwrap();
    runtime.note_on("old", 440.0, 1.0, 0).unwrap();
    runtime.render(0).unwrap();
    runtime.note_off("old", 1).unwrap();
    runtime.prune_finished(1).unwrap();
    runtime.note_on("new", 440.0, 1.0, 1).unwrap();

    let tail = source_program(
        "synth.sine/1",
        &held,
        &modulation("release_zero", "source", "release", "1/48000s"),
    );
    let mut runtime = make_runtime(&tail, 1).unwrap();
    runtime.note_on("old", 440.0, 1.0, 0).unwrap();
    runtime.render(0).unwrap();
    runtime.note_off("old", 1).unwrap();
    runtime.prune_finished(1).unwrap();
    assert_eq!(runtime.active_voice_count(), 1);
    assert_eq!(
        runtime.note_on("new", 440.0, 1.0, 1).unwrap_err().code(),
        "E_VOICE_LIMIT"
    );
}
