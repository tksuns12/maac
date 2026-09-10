use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::dsp::{DspEngine, Result};
use maac::graph::{
    Control, ControlTarget, GraphNode, GraphProcessor, GraphProgram, GraphStage, InstrumentProgram,
    Modulation, ParameterTarget, ProgramSource,
};
use maac::plan::{PortRef, Rational};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use maac::wavetable::{TableBank, Wavetable};
use maac::PlanArtifact;
use std::collections::BTreeMap;
use std::sync::Arc;

const RATE: f64 = 48_000.0;

fn r(numerator: i64, denominator: i64) -> Rational {
    Rational::new(numerator.into(), denominator.into())
}

fn params<I, S>(values: I) -> BTreeMap<String, Rational>
where
    I: IntoIterator<Item = (S, Rational)>,
    S: Into<String>,
{
    values
        .into_iter()
        .map(|(name, value)| (name.into(), value))
        .collect()
}

fn node<I, S>(id: &str, processor: GraphProcessor, values: I) -> GraphNode
where
    I: IntoIterator<Item = (S, Rational)>,
    S: Into<String>,
{
    GraphNode {
        id: id.into(),
        processor,
        params: params(values),
    }
}

fn port(node: &str) -> PortRef {
    PortRef::new(node, "out").unwrap()
}

fn modulation(id: &str, from: &str, target: &str, parameter: &str, depth: Rational) -> Modulation {
    Modulation {
        id: id.into(),
        from: port(from),
        to: ParameterTarget {
            node: target.into(),
            parameter: parameter.into(),
        },
        depth,
    }
}

fn amp_node() -> GraphNode {
    node(
        "amp",
        GraphProcessor::Adsr,
        [
            ("attack", r(0, 1)),
            ("decay", r(0, 1)),
            ("sustain", r(1, 1)),
            ("release", r(0, 1)),
        ],
    )
}

fn constant_source() -> GraphNode {
    node(
        "mod",
        GraphProcessor::Sine,
        [
            ("ratio", r(0, 1)),
            ("frequency", r(0, 1)),
            ("phase", r(1, 4)),
            ("level", r(1, 1)),
        ],
    )
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

fn phase_program(
    processor: GraphProcessor,
    target_params: BTreeMap<String, Rational>,
    modulations: Vec<Modulation>,
    controls: BTreeMap<String, Control>,
) -> InstrumentProgram {
    let mut nodes = vec![amp_node()];
    nodes.push(node("target", processor, target_params));
    if modulations.iter().any(|edge| edge.from.node == "mod") {
        nodes.push(constant_source());
    }
    InstrumentProgram {
        id: "phase_voice".into(),
        voice: GraphProgram {
            channels: 1,
            nodes,
            connections: Vec::new(),
            modulations,
            output: port("target"),
            amplitude: Some("amp".into()),
        },
        shared: None,
        controls,
        source: ProgramSource {
            file: "internal-phase.maac".into(),
            object: "phase_voice".into(),
            span: None,
        },
    }
}

fn oscillator_params(frequency: i64, phase: i64) -> BTreeMap<String, Rational> {
    params([
        ("ratio", r(0, 1)),
        ("frequency", r(frequency, 1)),
        ("phase", r(phase, 1)),
        ("level", r(1, 1)),
    ])
}

fn lfo_params(frequency: i64, phase: i64) -> BTreeMap<String, Rational> {
    params([
        ("frequency", r(frequency, 1)),
        ("phase", r(phase, 1)),
        ("level", r(1, 1)),
    ])
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

fn wavetable_params(frequency: i64, phase: i64) -> BTreeMap<String, Rational> {
    params([
        ("ratio", r(0, 1)),
        ("frequency", r(frequency, 1)),
        ("phase", r(phase, 1)),
        ("level", r(1, 1)),
        ("position", r(0, 1)),
    ])
}

#[test]
fn voice_phase_capture_matches_static_references_for_all_native_sources() {
    let edges = vec![
        modulation("z_small", "mod", "target", "phase", r(1, 4)),
        modulation("m_positive", "mod", "target", "phase", r(128, 1)),
        modulation("a_negative", "mod", "target", "phase", r(-128, 1)),
    ];
    for (processor, params) in [
        (GraphProcessor::Sine, oscillator_params(6_000, 0)),
        (GraphProcessor::Saw, oscillator_params(6_000, 0)),
        (GraphProcessor::Square, oscillator_params(6_000, 0)),
        (GraphProcessor::Triangle, oscillator_params(6_000, 0)),
        (GraphProcessor::Lfo, lfo_params(100, 0)),
    ] {
        let mut static_params = params.clone();
        static_params.insert("phase".into(), r(1, 4));
        let captured = phase_program(processor.clone(), params, edges.clone(), BTreeMap::new());
        let static_reference = phase_program(processor, static_params, Vec::new(), BTreeMap::new());
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
    let captured = phase_program(
        GraphProcessor::Wavetable {
            table: table.id.clone(),
        },
        wavetable_params(6_000, 0),
        edges.clone(),
        BTreeMap::new(),
    );
    let mut static_params = wavetable_params(6_000, 0);
    static_params.insert("phase".into(), r(1, 4));
    let static_reference = phase_program(
        GraphProcessor::Wavetable { table: table.id },
        static_params,
        Vec::new(),
        BTreeMap::new(),
    );
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
    let targets = [
        (
            GraphProcessor::Sine,
            oscillator_params(6_000, 0),
            BTreeMap::new(),
        ),
        (
            GraphProcessor::Saw,
            oscillator_params(6_000, 0),
            BTreeMap::new(),
        ),
        (
            GraphProcessor::Square,
            oscillator_params(6_000, 0),
            BTreeMap::new(),
        ),
        (
            GraphProcessor::Triangle,
            oscillator_params(6_000, 0),
            BTreeMap::new(),
        ),
        (GraphProcessor::Lfo, lfo_params(100, 0), BTreeMap::new()),
    ];
    for (processor, zero_params, controls) in targets {
        let mut one_params = zero_params.clone();
        one_params.insert("phase".into(), r(0, 1));
        let zero = phase_program(
            processor.clone(),
            zero_params,
            vec![modulation("phase_zero", "mod", "target", "phase", r(0, 1))],
            controls.clone(),
        );
        let one = phase_program(
            processor,
            one_params,
            vec![modulation("phase_one", "mod", "target", "phase", r(1, 1))],
            controls,
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
    let zero = phase_program(
        GraphProcessor::Wavetable {
            table: table.id.clone(),
        },
        wavetable_params(6_000, 0),
        vec![modulation("phase_zero", "mod", "target", "phase", r(0, 1))],
        BTreeMap::new(),
    );
    let one = phase_program(
        GraphProcessor::Wavetable { table: table.id },
        wavetable_params(6_000, 0),
        vec![modulation("phase_one", "mod", "target", "phase", r(1, 1))],
        BTreeMap::new(),
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
    let controls = [(
        "phase_bias".into(),
        Control {
            target: ControlTarget {
                graph: GraphStage::Voice,
                node: "target".into(),
                parameter: "phase".into(),
            },
            default: r(1, 4),
        },
    )]
    .into_iter()
    .collect();
    let program = phase_program(
        GraphProcessor::Lfo,
        lfo_params(0, 0),
        vec![modulation("phase_edge", "mod", "target", "phase", r(0, 1))],
        controls,
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
    for depth in [r(2, 1), r(-1, 1)] {
        let program = phase_program(
            GraphProcessor::Sine,
            oscillator_params(0, 0),
            vec![modulation("invalid_phase", "mod", "target", "phase", depth)],
            BTreeMap::new(),
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
    let mut amp = amp_node();
    amp.params.insert("release".into(), r(1, 1_000));
    let program = InstrumentProgram {
        id: "phase_off".into(),
        voice: GraphProgram {
            channels: 1,
            nodes: vec![
                amp,
                node("target", GraphProcessor::Sine, oscillator_params(0, 0)),
                node(
                    "source",
                    GraphProcessor::Sine,
                    [
                        ("ratio", r(0, 1)),
                        ("frequency", r(12_000, 1)),
                        ("phase", r(1, 4)),
                        ("level", r(1, 1)),
                    ],
                ),
            ],
            connections: Vec::new(),
            modulations: vec![
                modulation("on_phase", "source", "target", "phase", r(1, 4)),
                modulation("off_release", "source", "amp", "release", r(0, 1)),
            ],
            output: port("target"),
            amplitude: Some("amp".into()),
        },
        shared: None,
        controls: BTreeMap::new(),
        source: ProgramSource {
            file: "internal-phase.maac".into(),
            object: "phase_off".into(),
            span: None,
        },
    };
    let mut runtime = make_runtime(&program, 1, &BTreeMap::new()).unwrap();
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap(), &[1.0]);
    assert_eq!(runtime.render(1).unwrap(), &[1.0]);
    runtime.note_off("voice", 2).unwrap();
    assert_eq!(runtime.render(2).unwrap(), &[1.0]);
}

#[test]
fn chained_phase_and_adsr_onset_captures_have_no_lag_or_double_advance() {
    let program = InstrumentProgram {
        id: "phase_chain".into(),
        voice: GraphProgram {
            channels: 1,
            nodes: vec![
                amp_node(),
                node(
                    "source",
                    GraphProcessor::Sine,
                    [
                        ("ratio", r(0, 1)),
                        ("frequency", r(0, 1)),
                        ("phase", r(1, 4)),
                        ("level", r(1, 1)),
                    ],
                ),
                node("phase_a", GraphProcessor::Sine, oscillator_params(0, 0)),
                node("phase_b", GraphProcessor::Sine, oscillator_params(0, 0)),
            ],
            connections: Vec::new(),
            modulations: vec![
                modulation("source_to_a", "source", "phase_a", "phase", r(1, 4)),
                modulation("a_to_b", "phase_a", "phase_b", "phase", r(1, 4)),
                modulation("b_to_attack", "phase_b", "amp", "attack", r(1, 48_000)),
            ],
            output: port("phase_b"),
            amplitude: Some("amp".into()),
        },
        shared: None,
        controls: BTreeMap::new(),
        source: ProgramSource {
            file: "internal-phase.maac".into(),
            object: "phase_chain".into(),
            span: None,
        },
    };
    let mut runtime = make_runtime(&program, 1, &BTreeMap::new()).unwrap();
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap(), &[0.0]);
    assert_eq!(runtime.render(1).unwrap(), &[1.0]);
}
