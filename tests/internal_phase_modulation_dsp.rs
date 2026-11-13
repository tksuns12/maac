use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::dsp::{DspEngine, Result};
use maac::graph::{GraphProcessor, InstrumentProgram};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use maac::wavetable::{TableBank, Wavetable};
use maac::PlanArtifact;
use std::collections::BTreeMap;
use std::sync::Arc;

#[path = "support/programs.rs"]
mod programs;

const RATE: f64 = 48_000.0;

/// The params of an envelope that is a gate.
const GATE: &str = "attack = 0s; decay = 0s; sustain = 1; release = 0s;";

/// A sine `mod` held at one.
const CONSTANT_SOURCE: &str = r#"node mod { type = "synth.sine/1"; params = { ratio = 0; frequency = 0Hz; phase = 1/4; level = 1; }; }"#;

const OSCILLATORS: [&str; 4] = [
    "synth.sine/1",
    "synth.saw/1",
    "synth.square/1",
    "synth.triangle/1",
];

fn modulation(id: &str, from: &str, target: &str, parameter: &str, depth: &str) -> String {
    format!(
        "modulate {id} {{ from = &{from}:out; to = &{target}.params.{parameter}; depth = {depth}; }}\n"
    )
}

fn sine(id: &str, frequency: i64, phase: &str) -> String {
    format!(
        "node {id} {{ type = \"synth.sine/1\"; params = {{ {} }}; }}\n",
        oscillator_params(frequency, phase)
    )
}

/// A one-channel instrument whose voice holds `children` and renders the
/// node `output` under the envelope `amp`, which has the params `amp`.
fn voice_program(amp: &str, output: &str, children: &str, controls: &str) -> InstrumentProgram {
    programs::program(&format!(
        r#"instrument phase_voice {{
 channels = 1;
 voice voice {{
  channels = 1; amplitude = &amp; output = &{output}:out;
  node amp {{ type = "synth.adsr/1"; params = {{ {amp} }}; }}
  {children}
 }}
 {controls}
}}"#
    ))
}

/// A gated voice that renders `target`, a node of type `kind` with `params`.
/// The held source `mod` stands beside it when a modulation reads it.
fn phase_program(kind: &str, params: &str, modulations: &str, controls: &str) -> InstrumentProgram {
    let source = if modulations.contains("&mod:out") {
        CONSTANT_SOURCE
    } else {
        ""
    };
    voice_program(
        GATE,
        "target",
        &format!(
            "node target {{ type = \"{kind}\"; params = {{ {params} }}; }}\n{source}\n{modulations}"
        ),
        controls,
    )
}

/// `phase_program` with a wavetable oscillator at 6 kHz for `target`. The
/// bank of its table is built by the test, not read from an asset, so the
/// oscillator of a resolved program is pointed at it.
fn wavetable_program(table: &str, phase: &str, modulations: &str) -> InstrumentProgram {
    let mut program = phase_program(
        "synth.sine/1",
        &oscillator_params(6_000, phase),
        modulations,
        "",
    );
    let target = program
        .voice
        .nodes
        .iter_mut()
        .find(|node| node.id == "target")
        .unwrap();
    target.processor = GraphProcessor::Wavetable {
        table: table.to_owned(),
    };
    target
        .params
        .insert("position".into(), maac::parse_rational("0").unwrap());
    program
}

fn make_runtime(
    program: &InstrumentProgram,
    capacity: u32,
    tables: &BTreeMap<String, Arc<TableBank>>,
) -> Result<InstrumentRuntime> {
    let compiled = CompiledInstrument::compile(program, tables)?;
    InstrumentRuntime::new(Arc::new(compiled), capacity, RATE, &BTreeMap::new())
}

fn render_frames(
    program: &InstrumentProgram,
    tables: &BTreeMap<String, Arc<TableBank>>,
    frames: usize,
) -> Result<Vec<f64>> {
    let mut runtime = make_runtime(program, 1, tables)?;
    runtime.note_on("voice", 440.0, 1.0, 0)?;
    (0..frames as u64)
        .map(|frame| Ok(runtime.render(frame)?[0]))
        .collect()
}

fn oscillator_params(frequency: i64, phase: &str) -> String {
    format!("ratio = 0; frequency = {frequency}Hz; phase = {phase}; level = 1;")
}

fn lfo_params(frequency: i64, phase: &str) -> String {
    format!("frequency = {frequency}Hz; phase = {phase}; level = 1;")
}

fn wavetable_fixture() -> (Wavetable, BTreeMap<String, Arc<TableBank>>) {
    let table = Wavetable {
        id: "phase_cycle".into(),
        cycle_length: 8,
        samples: vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0, 0.0],
    };
    let bank = Arc::new(TableBank::new(&table).unwrap());
    let tables = [(table.id.clone(), bank)].into_iter().collect();
    (table, tables)
}

/// The params of each native source of a phase, at `phase`.
fn native_sources(phase: &str) -> Vec<(&'static str, String)> {
    OSCILLATORS
        .into_iter()
        .map(|kind| (kind, oscillator_params(6_000, phase)))
        .chain([("synth.lfo/1", lfo_params(100, phase))])
        .collect()
}

#[test]
fn voice_phase_capture_matches_static_references_for_all_native_sources() {
    let edges = [
        modulation("z_small", "mod", "target", "phase", "1/4"),
        modulation("m_positive", "mod", "target", "phase", "128"),
        modulation("a_negative", "mod", "target", "phase", "-128"),
    ]
    .concat();
    for ((kind, params), (_, static_params)) in
        native_sources("0").into_iter().zip(native_sources("1/4"))
    {
        let captured = phase_program(kind, &params, &edges, "");
        let static_reference = phase_program(kind, &static_params, "", "");
        let captured_bits = render_frames(&captured, &BTreeMap::new(), 8)
            .unwrap()
            .into_iter()
            .map(f64::to_bits)
            .collect::<Vec<_>>();
        let reference_bits = render_frames(&static_reference, &BTreeMap::new(), 8)
            .unwrap()
            .into_iter()
            .map(f64::to_bits)
            .collect::<Vec<_>>();
        assert_eq!(captured_bits, reference_bits);
    }

    let (table, tables) = wavetable_fixture();
    let captured = wavetable_program(&table.id, "0", &edges);
    let static_reference = wavetable_program(&table.id, "1/4", "");
    assert_eq!(
        render_frames(&captured, &tables, 8)
            .unwrap()
            .into_iter()
            .map(f64::to_bits)
            .collect::<Vec<_>>(),
        render_frames(&static_reference, &tables, 8)
            .unwrap()
            .into_iter()
            .map(f64::to_bits)
            .collect::<Vec<_>>()
    );
}

#[test]
fn phase_capture_uses_initial_expression_and_public_control_for_overlapping_voices() {
    let source = SourceBundle::new(
        "internal-phase.maac",
        r#"maac 1;
project song { score = [0q, 1/6000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }
tempo clock { points = [(0q, 60bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument local {
 channels = 1;
 voice v {
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp { type = "synth.adsr/1"; params = { attack = 0s; decay = 0s; sustain = 1; release = 0s; }; }
  node osc { type = "synth.sine/1"; params = { ratio = 0; frequency = 0Hz; phase = 0; level = 1; }; }
  node pressure { type = "synth.pressure/1"; }
  node timbre { type = "synth.timbre/1"; }
  modulate z_pressure { from = &pressure:out; to = &osc.params.phase; depth = 1/2; }
  modulate a_timbre { from = &timbre:out; to = &osc.params.phase; depth = -1/2; }
 }
 control phase_bias { target = &v.osc.params.phase; default = 1/4; }
}
node sound { instrument = &local; config = { voices = 2; }; }
curve pressure_a { clock = normalized; points = [(0, 1/2, step), (1/4, 0, step), (1, 0, step)]; }
curve pressure_b { clock = normalized; points = [(0, 1, step), (1/4, 0, step), (1, 0, step)]; }
curve timbre_a { clock = normalized; points = [(0, 1/2, step), (1/4, 1, step), (1, 1, step)]; }
curve timbre_b { clock = normalized; points = [(0, 0, step), (1/4, 1, step), (1, 1, step)]; }
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
    let artifact = compile_bundle_artifact(&source).expect("phase source should compile");
    let retained = PlanArtifact::from_json(&artifact.to_json().unwrap()).unwrap();
    let mut engine = DspEngine::new_artifact(&retained).unwrap();
    let mut first = Vec::new();
    engine
        .render(|frame| {
            first.push(frame[0]);
            Ok(())
        })
        .unwrap();
    assert_eq!(first, vec![1.0, 1.0, 0.0, 0.0, -1.0, -1.0, 0.0, 0.0]);

    let mut second = Vec::new();
    engine
        .render(|frame| {
            second.push(frame[0]);
            Ok(())
        })
        .unwrap();
    assert_eq!(first, second);
}

#[test]
fn phase_endpoints_zero_and_one_capture_the_same_voice_state() {
    for (kind, params) in native_sources("0") {
        let zero = phase_program(
            kind,
            &params,
            &modulation("phase_zero", "mod", "target", "phase", "0"),
            "",
        );
        let one = phase_program(
            kind,
            &params,
            &modulation("phase_one", "mod", "target", "phase", "1"),
            "",
        );
        assert_eq!(
            render_frames(&zero, &BTreeMap::new(), 5)
                .unwrap()
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            render_frames(&one, &BTreeMap::new(), 5)
                .unwrap()
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>()
        );
    }

    let (table, tables) = wavetable_fixture();
    let zero = wavetable_program(
        &table.id,
        "0",
        &modulation("phase_zero", "mod", "target", "phase", "0"),
    );
    let one = wavetable_program(
        &table.id,
        "0",
        &modulation("phase_one", "mod", "target", "phase", "1"),
    );
    assert_eq!(
        render_frames(&zero, &tables, 5)
            .unwrap()
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>(),
        render_frames(&one, &tables, 5)
            .unwrap()
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>()
    );
}

#[test]
fn lfo_phase_public_control_is_captured_once_and_reset_replays() {
    let program = phase_program(
        "synth.lfo/1",
        &lfo_params(0, "0"),
        &modulation("phase_edge", "mod", "target", "phase", "0"),
        "control phase_bias { target = &voice.target.params.phase; default = 1/4; }",
    );
    let mut runtime = make_runtime(&program, 1, &BTreeMap::new()).unwrap();
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap(), &[1.0]);
    runtime
        .update_controls(&[("phase_bias".into(), 3.0 / 4.0)].into_iter().collect())
        .unwrap();
    assert_eq!(runtime.render(1).unwrap(), &[1.0]);
    runtime.reset(&BTreeMap::new()).unwrap();
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap(), &[1.0]);
}

#[test]
fn invalid_phase_is_rejected_at_note_on_with_range_error() {
    for depth in ["2", "-1"] {
        let program = phase_program(
            "synth.sine/1",
            &oscillator_params(0, "0"),
            &modulation("invalid_phase", "mod", "target", "phase", depth),
            "",
        );
        let mut runtime = make_runtime(&program, 1, &BTreeMap::new()).unwrap();
        let error = runtime
            .note_on("voice", 440.0, 1.0, 0)
            .expect_err("out-of-range captured phase must fail at note-on");
        assert_eq!(error.code(), "E_RANGE");
    }
}

#[test]
fn note_off_preview_does_not_recapture_or_reset_voice_phase() {
    let program = voice_program(
        "attack = 0s; decay = 0s; sustain = 1; release = 1/1000s;",
        "target",
        &[
            sine("target", 0, "0"),
            sine("source", 12_000, "1/4"),
            modulation("on_phase", "source", "target", "phase", "1/4"),
            modulation("off_release", "source", "amp", "release", "0s"),
        ]
        .concat(),
        "",
    );
    let mut runtime = make_runtime(&program, 1, &BTreeMap::new()).unwrap();
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap(), &[1.0]);
    assert_eq!(runtime.render(1).unwrap(), &[1.0]);
    runtime.note_off("voice", 2).unwrap();
    assert_eq!(runtime.render(2).unwrap(), &[1.0]);
}

#[test]
fn chained_phase_and_adsr_onset_captures_have_no_lag_or_double_advance() {
    let program = voice_program(
        GATE,
        "phase_b",
        &[
            sine("source", 0, "1/4"),
            sine("phase_a", 0, "0"),
            sine("phase_b", 0, "0"),
            modulation("source_to_a", "source", "phase_a", "phase", "1/4"),
            modulation("a_to_b", "phase_a", "phase_b", "phase", "1/4"),
            modulation("b_to_attack", "phase_b", "amp", "attack", "1/48000s"),
        ]
        .concat(),
        "",
    );
    let mut runtime = make_runtime(&program, 1, &BTreeMap::new()).unwrap();
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap(), &[0.0]);
    assert_eq!(runtime.render(1).unwrap(), &[1.0]);
}
