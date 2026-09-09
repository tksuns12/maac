use maac::bundle::SourceBundle;
use maac::library::LibrarySet;

fn resolve(extra: &str) -> Result<LibrarySet, maac::Diagnostics> {
    let source = format!(
        r#"
maac 1;
library studio {{ version = "1.0.0"; }}
instrument lead {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp {{ type = "synth.adsr/1"; }}
  node osc {{ type = "synth.sine/1"; }}
  node color {{ type = "synth.timbre/1"; {extra} }}
 }}
}}
"#
    );
    LibrarySet::resolve(&SourceBundle::new("main.maac", source).resolve()?)
}

#[test]
fn source_accepts_timbre() {
    resolve("").expect("timbre is a voice graph source");
}

use maac::graph::{validate_graph, GraphProcessor, Modulation, ParameterTarget};
use maac::plan::PortRef;
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

#[test]
fn source_and_wire_are_strict() {
    resolve("params = {};").unwrap();
    for extra in [
        "config = {};",
        "config = { seed = 1; };",
        "params = { level = 1; };",
    ] {
        assert!(resolve(extra).is_err(), "{extra}");
    }
    let processor = GraphProcessor::Timbre;
    assert_eq!(processor.identity(), "synth.timbre/1");
    let wire = serde_json::json!({"kind": "synth.timbre/1"});
    assert_eq!(serde_json::to_value(&processor).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<GraphProcessor>(wire).unwrap(),
        processor
    );
    for wire in [
        serde_json::json!({"kind":"synth.timbre"}),
        serde_json::json!({"kind":"synth.timbre/2"}),
        serde_json::json!({"kind":"synth.timbre/1", "seed":1}),
        serde_json::json!({"kind":"synth.timbre/1", "config":{}}),
    ] {
        assert!(serde_json::from_value::<GraphProcessor>(wire).is_err());
    }
}

#[test]
fn presence_opts_in_and_shared_stage_is_forbidden() {
    let mut program = resolve("").unwrap().programs.remove(0);
    assert!(program.supports_timbre(), "unused timbre still opts in");
    let mut shared = program.voice.clone();
    shared
        .nodes
        .retain(|node| node.processor == GraphProcessor::Timbre);
    shared.amplitude = None;
    shared.output = PortRef::new("color", "out").unwrap();
    assert_eq!(
        validate_graph(&shared, false, Some(1)).unwrap_err().code,
        "E_CAPABILITY"
    );
    program
        .voice
        .nodes
        .retain(|node| node.processor != GraphProcessor::Timbre);
    assert!(!program.supports_timbre());
}

fn runtime(program: &maac::graph::InstrumentProgram) -> InstrumentRuntime {
    let compiled = CompiledInstrument::compile(program, &BTreeMap::new()).unwrap();
    InstrumentRuntime::new(Arc::new(compiled), 1, 48_000.0, &BTreeMap::new()).unwrap()
}

#[test]
fn default_zero_mapping_preserves_audio_and_program_roundtrip() {
    let mut mapped = resolve("").unwrap().programs.remove(0);
    mapped.voice.modulations.push(Modulation {
        id: "color_level".into(),
        from: PortRef::new("color", "out").unwrap(),
        to: ParameterTarget {
            node: "osc".into(),
            parameter: "level".into(),
        },
        depth: maac::parse_rational("-1/2").unwrap(),
    });
    mapped.validate().unwrap();
    let restored: maac::graph::InstrumentProgram =
        serde_json::from_str(&serde_json::to_string(&mapped).unwrap()).unwrap();
    assert_eq!(restored, mapped);
    assert!(restored.supports_timbre());
    let mut plain = mapped.clone();
    plain.voice.modulations.clear();
    plain
        .voice
        .nodes
        .retain(|node| node.processor != GraphProcessor::Timbre);
    let mut reference = runtime(&plain);
    let mut actual = runtime(&restored);
    reference.note_on("note", 440.0, 0.75, 0).unwrap();
    actual.note_on("note", 440.0, 0.75, 0).unwrap();
    for frame in 0..512 {
        assert_eq!(
            actual.render(frame).unwrap(),
            reference.render(frame).unwrap()
        );
    }
    mapped.voice.modulations[0].to.parameter = "phase".into();
    assert!(
        mapped.validate().is_err(),
        "event-rate targets are forbidden"
    );
    mapped.voice.modulations.clear();
    mapped.voice.output = PortRef::new("color", "out").unwrap();
    mapped.validate().unwrap();
    let mut direct = runtime(&mapped);
    direct.note_on("note", 440.0, 1.0, 0).unwrap();
    for frame in 0..8 {
        assert_eq!(direct.render(frame).unwrap(), &[0.0]);
    }
}

#[test]
fn timbre_has_no_input_ports_or_parameters() {
    let mut program = resolve("").unwrap().programs.remove(0);
    program.voice.connections.push(
        maac::plan::Connection::new(
            "bad_input",
            PortRef::new("osc", "out").unwrap(),
            PortRef::new("color", "in").unwrap(),
        )
        .unwrap(),
    );
    assert!(program.validate().is_err());
    program.voice.connections.clear();
    let color = program
        .voice
        .nodes
        .iter_mut()
        .find(|node| node.id == "color")
        .unwrap();
    color
        .params
        .insert("level".into(), maac::parse_rational("1").unwrap());
    assert!(program.validate().is_err());
}
