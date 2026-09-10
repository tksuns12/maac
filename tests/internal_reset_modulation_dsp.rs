use maac::graph::{
    Control, ControlTarget, GraphNode, GraphProcessor, GraphProgram, GraphStage, InstrumentProgram,
    Modulation, ParameterTarget, ProgramSource,
};
use maac::plan::{PortRef, Rational};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

const RATE: f64 = 48_000.0;

fn r(numerator: i64, denominator: i64) -> Rational {
    Rational::new(numerator.into(), denominator.into())
}

fn node(
    id: &str,
    processor: GraphProcessor,
    params: impl IntoIterator<Item = (&'static str, Rational)>,
) -> GraphNode {
    GraphNode {
        id: id.into(),
        processor,
        params: params
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    }
}

fn port(node: &str) -> PortRef {
    PortRef::new(node, "out").unwrap()
}

fn modulation(id: &str, from: &str, target: &str, depth: Rational) -> Modulation {
    Modulation {
        id: id.into(),
        from: port(from),
        to: ParameterTarget {
            node: target.into(),
            parameter: "phase".into(),
        },
        depth,
    }
}

fn voice_graph() -> GraphProgram {
    GraphProgram {
        channels: 1,
        nodes: vec![
            node(
                "amp",
                GraphProcessor::Adsr,
                [
                    ("attack", r(0, 1)),
                    ("decay", r(0, 1)),
                    ("sustain", r(1, 1)),
                    ("release", r(0, 1)),
                ],
            ),
            node(
                "voice",
                GraphProcessor::Sine,
                [
                    ("ratio", r(0, 1)),
                    ("frequency", r(0, 1)),
                    ("phase", r(1, 4)),
                    ("level", r(1, 1)),
                ],
            ),
        ],
        connections: Vec::new(),
        modulations: Vec::new(),
        output: port("voice"),
        amplitude: Some("amp".into()),
    }
}

fn reset_lfo_program(
    target_phase: Rational,
    source_phase: Option<Rational>,
    modulation_from: Option<&str>,
    modulation_depth: Rational,
    controls: BTreeMap<String, Control>,
) -> InstrumentProgram {
    let mut nodes = vec![node(
        "target",
        GraphProcessor::Lfo,
        [
            ("frequency", r(100, 1)),
            ("phase", target_phase),
            ("level", r(1, 1)),
        ],
    )];
    if let Some(phase) = source_phase {
        nodes.push(node(
            "source",
            GraphProcessor::Lfo,
            [("frequency", r(0, 1)), ("phase", phase), ("level", r(1, 1))],
        ));
    }
    let modulations = modulation_from
        .map(|from| vec![modulation("reset_phase", from, "target", modulation_depth)])
        .unwrap_or_default();
    InstrumentProgram {
        id: "reset_phase_voice".into(),
        voice: voice_graph(),
        shared: Some(GraphProgram {
            channels: 1,
            nodes,
            connections: Vec::new(),
            modulations,
            output: port("target"),
            amplitude: None,
        }),
        controls,
        source: ProgramSource {
            file: "internal-reset-phase.maac".into(),
            object: "reset_phase_voice".into(),
            span: None,
        },
    }
}

fn runtime(program: &InstrumentProgram, controls: &BTreeMap<String, f64>) -> InstrumentRuntime {
    let compiled = CompiledInstrument::compile(program, &BTreeMap::new()).unwrap();
    InstrumentRuntime::new(Arc::new(compiled), 1, RATE, controls).unwrap()
}

fn bits(samples: Vec<f64>) -> Vec<u64> {
    samples.into_iter().map(f64::to_bits).collect()
}

fn render(runtime: &mut InstrumentRuntime, frames: usize) -> Vec<f64> {
    (0..frames as u64)
        .map(|frame| runtime.render(frame).unwrap()[0])
        .collect()
}

fn render_input_sequence(runtime: &mut InstrumentRuntime, frames: usize) -> Vec<f64> {
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    let mut samples = vec![runtime.render(0).unwrap()[0]];
    samples.extend((1..frames as u64).map(|frame| runtime.render(frame).unwrap()[0]));
    samples
}

#[test]
fn shared_lfo_reset_phase_capture_matches_static_reference_and_replays() {
    let captured = reset_lfo_program(
        r(0, 1),
        Some(r(1, 4)),
        Some("source"),
        r(1, 4),
        BTreeMap::new(),
    );
    let static_reference = reset_lfo_program(r(1, 4), None, None, r(0, 1), BTreeMap::new());
    let mut actual = runtime(&captured, &BTreeMap::new());
    let mut expected = runtime(&static_reference, &BTreeMap::new());
    let first = render(&mut actual, 8);
    assert_eq!(bits(first.clone()), bits(render(&mut expected, 8)));

    actual.reset(&BTreeMap::new()).unwrap();
    assert_eq!(bits(first), bits(render(&mut actual, 8)));

    for (target_phase, reference_phase) in [(r(0, 1), r(0, 1)), (r(1, 1), r(0, 1))] {
        let endpoint = reset_lfo_program(
            target_phase,
            Some(r(1, 4)),
            Some("source"),
            r(0, 1),
            BTreeMap::new(),
        );
        let reference = reset_lfo_program(reference_phase, None, None, r(0, 1), BTreeMap::new());
        assert_eq!(
            bits(render(&mut runtime(&endpoint, &BTreeMap::new()), 8)),
            bits(render(&mut runtime(&reference, &BTreeMap::new()), 8))
        );
    }
}

#[test]
fn shared_input_phase_captures_silence_before_notes_and_ignores_voice_samples() {
    let controls = [(
        "phase_bias".into(),
        Control {
            target: ControlTarget {
                graph: GraphStage::Shared,
                node: "target".into(),
                parameter: "phase".into(),
            },
            default: r(1, 4),
        },
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let instance_controls = [("phase_bias".into(), 3.0 / 4.0)]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let captured = reset_lfo_program(r(0, 1), None, Some("input"), r(1, 4), controls.clone());
    let static_reference = reset_lfo_program(r(0, 1), None, None, r(0, 1), controls);
    let mut actual = runtime(&captured, &instance_controls);
    let mut expected = runtime(&static_reference, &instance_controls);
    let first = render_input_sequence(&mut actual, 8);
    assert_eq!(
        bits(first.clone()),
        bits(render_input_sequence(&mut expected, 8))
    );
    assert!((first[0] + 1.0).abs() < 1.0e-12);
    assert!(first[1] < -0.99);

    actual.reset(&instance_controls).unwrap();
    expected.reset(&instance_controls).unwrap();
    let replay_actual = render_input_sequence(&mut actual, 8);
    let replay_expected = render_input_sequence(&mut expected, 8);
    assert_eq!(bits(first), bits(replay_actual.clone()));
    assert_eq!(bits(replay_actual), bits(replay_expected));
}
