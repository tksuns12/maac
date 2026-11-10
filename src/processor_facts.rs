//! A snapshot of what the crate knows about each `synth.*/1` processor:
//! its channels, where it may be placed, whether its input is a same-sample
//! dependence, its work and memory, its parameter slots and descriptors in
//! each stage, and the config N(A) writes for it. The snapshot is compared
//! with `tests/fixtures/processor_facts.txt`, so moving where a fact is
//! written cannot change the fact.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{Map, Value};

use crate::graph::{
    delay_line_frames, parameter_descriptor_for_stage, topological_order, GraphNode,
    GraphProcessor, GraphProgram, GraphStage, GraphUnit, ParameterRate, ParameterSpec,
    BOW_STRING_CELLS, PLUCK_DELAY_CELLS,
};
use crate::plan::{Connection, PortRef};

/// Every parameter name any processor has had, in order: the snapshot asks
/// each processor for each, so a parameter a processor loses or gains shows.
const PARAMETER_NAMES: [&str; 30] = [
    "attack",
    "balance",
    "bias",
    "curve",
    "cutoff",
    "damping",
    "decay",
    "detune",
    "drive",
    "dynamics",
    "feedback",
    "frequency",
    "hardness",
    "level",
    "pan",
    "phase",
    "pickup",
    "position",
    "q",
    "rate",
    "ratio",
    "reference",
    "release",
    "spread",
    "stretch",
    "sustain",
    "time",
    "timing",
    "vibrato",
    "width",
];

/// One processor of each kind, and a second where a config value changes a
/// fact: its channels, its body, its clock or its line's length.
const INSTANCES: &[&str] = &[
    r#"{"kind":"synth.sine/1"}"#,
    r#"{"kind":"synth.saw/1"}"#,
    r#"{"kind":"synth.square/1"}"#,
    r#"{"kind":"synth.triangle/1"}"#,
    r#"{"kind":"synth.noise/1","seed":1}"#,
    r#"{"kind":"synth.noise/1","seed":7,"per_note":true}"#,
    r#"{"kind":"synth.pluck/1","seed":5}"#,
    r#"{"kind":"synth.random/1","seed":9}"#,
    r#"{"kind":"synth.modal/1","modes":16,"strings":3}"#,
    r#"{"kind":"synth.modal/1","modes":5,"strings":1}"#,
    r#"{"kind":"synth.bow/1","players":1,"body":"violin","channels":1,"seed":1}"#,
    r#"{"kind":"synth.bow/1","players":8,"body":"none","channels":2,"seed":3}"#,
    r#"{"kind":"synth.wavetable/1","table":"colors"}"#,
    r#"{"kind":"synth.sample/1","zones":[{"sample":"tone","low_key":0,"high_key":127}]}"#,
    r#"{"kind":"synth.sample/1","channels":2,"zones":[{"sample":"tone","low_key":0,"high_key":127}],"fade_shape":"equal_power"}"#,
    r#"{"kind":"synth.adsr/1"}"#,
    r#"{"kind":"synth.adsr/1","on_join":"restart"}"#,
    r#"{"kind":"synth.timbre/1"}"#,
    r#"{"kind":"synth.pressure/1"}"#,
    r#"{"kind":"synth.velocity/1"}"#,
    r#"{"kind":"synth.key/1"}"#,
    r#"{"kind":"synth.lfo/1"}"#,
    r#"{"kind":"synth.gain/1","channels":1}"#,
    r#"{"kind":"synth.gain/1","channels":2}"#,
    r#"{"kind":"synth.onepole/1","channels":1}"#,
    r#"{"kind":"synth.onepole/1","channels":2}"#,
    r#"{"kind":"synth.highpass/1","channels":1}"#,
    r#"{"kind":"synth.highpass/1","channels":2}"#,
    r#"{"kind":"synth.svf/1","channels":1,"mode":"lowpass"}"#,
    r#"{"kind":"synth.svf/1","channels":2,"mode":"bandpass"}"#,
    r#"{"kind":"synth.drive/1","channels":1}"#,
    r#"{"kind":"synth.drive/1","channels":2}"#,
    r#"{"kind":"synth.mix/1","channels":1}"#,
    r#"{"kind":"synth.mix/1","channels":2}"#,
    r#"{"kind":"synth.pan/1"}"#,
    r#"{"kind":"synth.balance/1"}"#,
    r#"{"kind":"synth.width/1"}"#,
    r#"{"kind":"synth.delay/1","channels":1,"clock":"seconds","max":"1/2"}"#,
    r#"{"kind":"synth.delay/1","channels":2,"clock":"score","max":"4/1"}"#,
];

fn yes(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn graph(processor: &GraphProcessor, self_loop: bool) -> GraphProgram {
    let port = |port: &str| PortRef::new("x", port).unwrap();
    GraphProgram {
        channels: processor.output_channels(),
        nodes: vec![GraphNode {
            id: "x".into(),
            processor: processor.clone(),
            params: BTreeMap::new(),
        }],
        connections: if self_loop {
            vec![Connection::new("back", port("out"), port("in")).unwrap()]
        } else {
            Vec::new()
        },
        modulations: Vec::new(),
        output: port("out"),
        amplitude: None,
        connected: false,
    }
}

fn spec(spec: &ParameterSpec) -> String {
    let unit = match spec.unit {
        GraphUnit::Dimensionless => "1",
        GraphUnit::Seconds => "s",
        GraphUnit::Hertz => "Hz",
        GraphUnit::Quarter => "q",
    };
    let rate = match spec.rate {
        ParameterRate::Sample => "sample",
        ParameterRate::NoteOn => "note-on",
        ParameterRate::NoteOff => "note-off",
        ParameterRate::Reset => "reset",
    };
    format!(
        "{unit}, {rate}, default {}, {}{}, {}{}",
        spec.default,
        if spec.min_open { "(" } else { "[" },
        spec.min,
        spec.max,
        if spec.max_open { ")" } else { "]" },
    )
}

/// Every fact about `processor`, one per line.
fn facts(wire: &str, processor: &GraphProcessor) -> String {
    let mut out = String::new();
    let identity = processor.identity();
    let delay = matches!(processor, GraphProcessor::Delay { .. });
    let input = processor.required_input_channels();
    writeln!(out, "{wire}").unwrap();
    writeln!(
        out,
        "  channels: out {}, audio in {}, many inputs {}",
        processor.output_channels(),
        input.map_or("none".to_string(), |channels| channels.to_string()),
        yes(processor.accepts_many_inputs()),
    )
    .unwrap();
    writeln!(
        out,
        "  placement: instrument voice {}, instrument shared {}, effect {}",
        yes(!delay),
        yes(!delay && !processor.forbidden_in_shared()),
        yes(!processor.forbidden_in_shared()),
    )
    .unwrap();
    topological_order(&graph(processor, false)).unwrap();
    if input.is_some() {
        // A node fed its own output: a cycle unless its input is read from
        // earlier frames.
        let same_sample = match topological_order(&graph(processor, true)) {
            Ok(_) => false,
            Err(error) if error.code == "E_ALGEBRAIC_LOOP" => true,
            Err(error) => panic!("{identity}: {error:?}"),
        };
        writeln!(
            out,
            "  audio input in the same sample: {}",
            yes(same_sample)
        )
        .unwrap();
    }
    writeln!(
        out,
        "  sample work: {}",
        crate::plan::graph_sample_cost(&graph(processor, false)),
    )
    .unwrap();
    match processor {
        GraphProcessor::Pluck { .. } => {
            writeln!(out, "  memory: {PLUCK_DELAY_CELLS} cells per voice").unwrap()
        }
        GraphProcessor::Bow { players, .. } => writeln!(
            out,
            "  memory: {} cells per voice",
            usize::from(*players) * BOW_STRING_CELLS
        )
        .unwrap(),
        GraphProcessor::Delay { channels, max, .. } => writeln!(
            out,
            "  memory: {} cells",
            delay_line_frames(max).unwrap() * usize::from(*channels)
        )
        .unwrap(),
        _ => {}
    }
    writeln!(
        out,
        "  slots: {}",
        processor
            .declaration()
            .params
            .iter()
            .map(|param| param.name)
            .collect::<Vec<_>>()
            .join(" ")
    )
    .unwrap();
    // What N(A) writes for a node of this type whose config leaves every
    // default out.
    let mut config = Map::new();
    crate::normalization::expand_graph_config_defaults(processor.declaration(), &mut config);
    if crate::normalization::writes_graph_config(processor.declaration()) {
        writeln!(out, "  N(A) config defaults: {}", Value::Object(config)).unwrap();
    } else {
        assert!(config.is_empty(), "{identity}");
        writeln!(out, "  N(A) writes no config").unwrap();
    }
    for parameter in PARAMETER_NAMES {
        if let Some(found) = parameter_descriptor_for_stage(processor, parameter, GraphStage::Voice)
        {
            writeln!(out, "  {parameter}: {}", spec(&found)).unwrap();
        }
    }
    // A shared graph, as in an effect, where the descriptor differs.
    if !processor.forbidden_in_shared() {
        for parameter in PARAMETER_NAMES {
            let voice = parameter_descriptor_for_stage(processor, parameter, GraphStage::Voice);
            let shared = parameter_descriptor_for_stage(processor, parameter, GraphStage::Shared);
            if voice != shared {
                let shared = shared.map_or("none".to_string(), |found| spec(&found));
                writeln!(out, "  {parameter} in a shared graph: {shared}").unwrap();
            }
        }
    }
    out
}

fn snapshot() -> String {
    let mut out = String::new();
    for wire in INSTANCES {
        let processor: GraphProcessor = serde_json::from_str(wire).unwrap();
        out.push_str(&facts(wire, &processor));
    }
    out
}

#[test]
fn every_processor_fact_is_as_pinned() {
    let expected = include_str!("../tests/fixtures/processor_facts.txt");
    let actual = snapshot();
    if actual != expected {
        let path = std::env::temp_dir().join("processor_facts.actual.txt");
        std::fs::write(&path, &actual).unwrap();
        let line = actual
            .lines()
            .zip(expected.lines())
            .position(|(actual, expected)| actual != expected)
            .unwrap_or_else(|| actual.lines().count().min(expected.lines().count()));
        panic!(
            "processor facts differ from line {}: expected {:?}, found {:?}; the full snapshot is at {}",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line),
            path.display()
        );
    }
}

#[test]
fn the_snapshot_covers_every_processor() {
    let mut kinds: Vec<&str> = INSTANCES
        .iter()
        .map(|wire| {
            serde_json::from_str::<GraphProcessor>(wire)
                .unwrap()
                .identity()
        })
        .collect();
    kinds.dedup();
    assert_eq!(kinds.len(), 27, "{kinds:?}");
}

/// Writes the snapshot to the path in `MAAC_PROCESSOR_FACTS`, for review.
#[test]
#[ignore]
fn write_processor_facts() {
    let path = std::env::var("MAAC_PROCESSOR_FACTS").expect("MAAC_PROCESSOR_FACTS");
    std::fs::write(path, snapshot()).unwrap();
}
