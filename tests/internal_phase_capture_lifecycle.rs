use maac::graph::{
    GraphNode, GraphProcessor, GraphProgram, InstrumentProgram, Modulation, ParameterTarget,
    ProgramSource,
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
    values: impl IntoIterator<Item = (&'static str, Rational)>,
) -> GraphNode {
    GraphNode {
        id: id.into(),
        processor,
        params: values
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
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

fn program() -> InstrumentProgram {
    InstrumentProgram {
        id: "phase_lifecycle".into(),
        voice: GraphProgram {
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
                    "onset_source",
                    GraphProcessor::Sine,
                    [
                        ("ratio", r(0, 1)),
                        ("frequency", r(12_000, 1)),
                        ("phase", r(1, 4)),
                        ("level", r(1, 1)),
                    ],
                ),
                node(
                    "target",
                    GraphProcessor::Sine,
                    [
                        ("ratio", r(0, 1)),
                        ("frequency", r(3_000, 1)),
                        ("phase", r(0, 1)),
                        ("level", r(1, 1)),
                    ],
                ),
            ],
            connections: Vec::new(),
            modulations: vec![
                modulation("on_phase", "onset_source", "target", "phase", r(1, 4)),
                modulation("off_release", "target", "amp", "release", r(1, 12_000)),
            ],
            output: port("target"),
            amplitude: Some("amp".into()),
        },
        shared: None,
        controls: BTreeMap::new(),
        source: ProgramSource {
            file: "internal-phase-lifecycle.maac".into(),
            object: "phase_lifecycle".into(),
            span: None,
        },
    }
}

fn lifecycle(runtime: &mut InstrumentRuntime) -> Vec<f64> {
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    let mut samples = vec![runtime.render(0).unwrap()[0], runtime.render(1).unwrap()[0]];
    runtime.note_off("voice", 2).unwrap();
    samples.push(runtime.render(2).unwrap()[0]);
    samples.push(runtime.render(3).unwrap()[0]);
    samples
}

#[test]
fn note_off_uses_live_phase_without_revisiting_its_onset_only_ancestor() {
    let compiled = Arc::new(CompiledInstrument::compile(&program(), &BTreeMap::new()).unwrap());
    let mut runtime = InstrumentRuntime::new(compiled, 1, RATE, &BTreeMap::new()).unwrap();
    let first = lifecycle(&mut runtime);

    let live_off_sample = std::f64::consts::FRAC_1_SQRT_2;
    let release_frames = 4.0 * live_off_sample;
    let expected = [
        1.0,
        (5.0 * std::f64::consts::PI / 8.0).sin(),
        live_off_sample,
        (7.0 * std::f64::consts::PI / 8.0).sin() * (1.0 - 1.0 / release_frames),
    ];
    for (frame, (actual, expected)) in first.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() < 1.0e-12,
            "frame {frame}: {actual} != {expected}"
        );
    }

    // At off, the onset source's live output is -1. Re-evaluating the
    // onset-only phase edge would therefore produce an invalid -1/4 phase.
    // Successful note-off plus this analytic tail proves the captured target
    // phase was retained and previewed at its current accumulated position.
    runtime.reset(&BTreeMap::new()).unwrap();
    assert_eq!(lifecycle(&mut runtime), first);
}
