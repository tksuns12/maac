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

#[test]
fn every_declaration_is_in_the_snapshot() {
    let declarations: Vec<&crate::processor::Declaration> = INSTANCES
        .iter()
        .map(|wire| {
            serde_json::from_str::<GraphProcessor>(wire)
                .unwrap()
                .declaration()
        })
        .collect();
    for declaration in crate::processor::ALL {
        assert!(
            declarations.contains(&declaration),
            "{} has no instance in the snapshot",
            declaration.identity
        );
    }
    for (wire, declaration) in INSTANCES.iter().zip(&declarations) {
        assert!(
            crate::processor::ALL.contains(declaration),
            "{wire}'s declaration is not in ALL"
        );
    }
}

/// Writes the snapshot to the path in `MAAC_PROCESSOR_FACTS`, for review.
#[test]
#[ignore]
fn write_processor_facts() {
    let path = std::env::var("MAAC_PROCESSOR_FACTS").expect("MAAC_PROCESSOR_FACTS");
    std::fs::write(path, snapshot()).unwrap();
}

// The hand-written tables in the docs, checked against the declarations.

/// The rows of the Markdown table whose header line starts with `header`.
fn table_rows<'a>(doc: &'a str, header: &str) -> Vec<Vec<&'a str>> {
    let mut lines = doc.lines().skip_while(|line| !line.starts_with(header));
    assert!(lines.next().is_some(), "no table headed {header:?}");
    lines
        .skip(1)
        .take_while(|line| line.starts_with('|'))
        .map(|line| line.trim_matches('|').split(" | ").map(str::trim).collect())
        .collect()
}

/// The names in backticks in `text`.
fn ticked(text: &str) -> Vec<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

/// A quantity as the docs write it, such as `1`, `-64`, `1/8`, `½ s`,
/// `80 ms` or `28/5 Hz`, as a rational and a unit.
fn quantity(text: &str) -> (crate::plan::Rational, GraphUnit) {
    let text = text.trim().replace('−', "-");
    let (number, unit) = text.split_once(' ').unwrap_or((&text, ""));
    let number = match number {
        "½" => crate::processor::exact(1, 2).rational(),
        number => match number.split_once('/') {
            Some((numerator, denominator)) => {
                crate::processor::exact(numerator.parse().unwrap(), denominator.parse().unwrap())
                    .rational()
            }
            None => crate::processor::int(number.parse().unwrap()).rational(),
        },
    };
    match unit {
        "" => (number, GraphUnit::Dimensionless),
        "Hz" => (number, GraphUnit::Hertz),
        "s" => (number, GraphUnit::Seconds),
        "ms" => (
            number / crate::processor::int(1000).rational(),
            GraphUnit::Seconds,
        ),
        "q" => (number, GraphUnit::Quarter),
        unit => panic!("unknown unit {unit:?} in {text:?}"),
    }
}

#[test]
fn the_processor_table_gives_each_parameter_its_declared_default() {
    use crate::processor::{declaration_of, ALL, SINE};
    let doc = include_str!("../docs/instruments.md");
    let rows = table_rows(doc, "| Versioned processor |");
    let mut documented = Vec::new();
    for row in &rows {
        let (identities, params) = (ticked(row[0]), row[1]);
        for identity in &identities {
            let declaration =
                declaration_of(identity).unwrap_or_else(|| panic!("{identity} is not declared"));
            documented.push(*identity);
            // Each `name` (default; ...) in the row.
            for (index, _) in params.match_indices("` (") {
                let name = &params[params[..index].rfind('`').unwrap() + 1..index];
                let rest = &params[index + 3..];
                let default = &rest[..rest.find([';', ')', ',']).unwrap()];
                if let Some(key) = name.strip_prefix("config.") {
                    check_config_default(declaration, key, default);
                    continue;
                }
                let param = declaration
                    .param(name)
                    .unwrap_or_else(|| panic!("{identity} has no parameter {name}"));
                let spec = param.spec(GraphStage::Voice);
                assert_eq!(
                    quantity(default),
                    (spec.default, spec.unit),
                    "{identity} {name}: the docs give {default}"
                );
            }
            // Every parameter it declares is named in its row.
            let names = ticked(params);
            let oscillator = params.contains("Oscillator parameters");
            for param in declaration.params {
                assert!(
                    names.contains(&param.name) || (oscillator && SINE.param(param.name).is_some()),
                    "the row of {identity} leaves out {}",
                    param.name
                );
            }
        }
    }
    // The plucked string has its own contract.
    for declaration in ALL {
        assert!(
            documented.contains(&declaration.identity) || declaration.identity == "synth.pluck/1",
            "{} has no row in the processor table",
            declaration.identity
        );
    }
}

/// A config key's default as the docs give it, against the one N(A) writes.
/// For some keys the docs give a range, a choice or a form instead.
fn check_config_default(declaration: &crate::processor::Declaration, key: &str, text: &str) {
    use crate::processor::{ConfigValue, NormalConfig};
    if text.contains(['…', '`']) {
        return;
    }
    let NormalConfig::Defaults(defaults) = declaration.config else {
        return;
    };
    // A required key's parenthesis describes its form.
    let Some(declared) = defaults.iter().find(|default| default.key == key) else {
        return;
    };
    let declared = match declared.value {
        ConfigValue::Number(value) => value.to_string(),
        ConfigValue::Boolean(value) => value.to_string(),
        ConfigValue::Symbol(value) => value.to_string(),
    };
    assert_eq!(
        text, declared,
        "{} config.{key}: the docs give {text}",
        declaration.identity
    );
}

#[test]
fn the_plucked_string_table_is_its_declaration() {
    use crate::processor::PLUCK;
    let doc = include_str!("../docs/plucked-string.md");
    let rows = table_rows(
        doc,
        "| Parameter | Unit | Default | Inclusive range | Rate |",
    );
    let names: Vec<&str> = rows.iter().map(|row| ticked(row[0])[0]).collect();
    let declared: Vec<&str> = PLUCK.params.iter().map(|param| param.name).collect();
    assert_eq!(names, declared);
    for row in &rows {
        let name = ticked(row[0])[0];
        let spec = PLUCK.param(name).unwrap().spec(GraphStage::Voice);
        let unit = match spec.unit {
            GraphUnit::Dimensionless => "Dimensionless",
            GraphUnit::Seconds => "Seconds",
            GraphUnit::Hertz => "Hertz",
            GraphUnit::Quarter => "Quarter",
        };
        assert_eq!(row[1], unit, "{name}");
        assert_eq!(quantity(row[2]).0, spec.default, "{name}");
        let (low, high) = row[3].split_once('…').unwrap();
        let suffix = row[3].rsplit_once(' ').map_or("", |(_, unit)| unit);
        let high = high.trim_end_matches(suffix).trim();
        assert!(!spec.min_open && !spec.max_open, "{name}");
        assert_eq!(quantity(low).0, spec.min, "{name}");
        assert_eq!(quantity(high).0, spec.max, "{name}");
        let rate = match spec.rate {
            ParameterRate::Sample => "Sample",
            ParameterRate::NoteOn => "Note-on",
            ParameterRate::NoteOff => "Note-off",
            ParameterRate::Reset => "Reset",
        };
        assert_eq!(row[4], rate, "{name}");
    }
}

#[test]
fn the_capability_matrix_charges_as_declared() {
    use crate::processor::{Work, ALL, DELAY_SCORE, DELAY_SECONDS, DRIVE, PLUCK, SVF};
    let doc = include_str!("../docs/capabilities.md");
    let units = |declaration: &crate::processor::Declaration| match declaration.work {
        Work::Units(units) => units,
        work => panic!("{} is charged as {work:?}", declaration.identity),
    };
    assert_eq!(units(&SVF), units(&DRIVE));
    for expected in [
        format!(
            "{} cells per pluck voice-node",
            crate::graph::PLUCK_DELAY_CELLS
        ),
        format!("{} units per sample visit plus", units(&PLUCK)),
        format!(
            "{} units per sample visit for each `synth.svf/1` or `synth.drive/1` node; every other graph node is 1",
            units(&SVF)
        ),
        format!(
            "a `synth.delay/1` is {} unit per sample visit, or {} on the score clock",
            units(&DELAY_SECONDS),
            units(&DELAY_SCORE)
        ),
        format!(
            "`ceil(modes × strings / {})`",
            crate::graph::MODAL_RESONATORS_PER_WORK
        ),
        format!(
            "{} units per sample visit for each player of a `synth.bow/1` node, and {} more",
            crate::graph::BOW_PLAYER_WORK,
            crate::graph::BOW_BODY_WORK
        ),
    ] {
        assert!(doc.contains(&expected), "docs/capabilities.md lacks {expected:?}");
    }
    // "Every other graph node is 1."
    for declaration in ALL {
        let named = [
            "synth.pluck/1",
            "synth.svf/1",
            "synth.drive/1",
            "synth.delay/1",
        ];
        if !named.contains(&declaration.identity) {
            assert!(
                matches!(
                    declaration.work,
                    Work::Units(1) | Work::ModalBank | Work::BowSection
                ),
                "{} costs {:?}",
                declaration.identity,
                declaration.work
            );
        }
    }
}
