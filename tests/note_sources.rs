//! `synth.velocity/1` and `synth.key/1`: the note's velocity and key as
//! voice graph sources.

use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::graph::{validate_graph, GraphProcessor, InstrumentProgram, Modulation, ParameterTarget};
use maac::library::LibrarySet;
use maac::plan::PortRef;
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use maac::{compile, parse, render_artifact};
use std::collections::BTreeMap;
use std::sync::Arc;

const SOURCES: [(&str, GraphProcessor); 2] = [
    ("synth.velocity/1", GraphProcessor::Velocity),
    ("synth.key/1", GraphProcessor::Key),
];

fn resolve(kind: &str, extra: &str) -> Result<LibrarySet, maac::Diagnostics> {
    let source = format!(
        r#"
maac 1;
library studio {{ version = "1.0.0"; }}
instrument lead {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &note:out;
  node amp {{ type = "synth.adsr/1"; }}
  node note {{ type = "{kind}"; {extra} }}
 }}
}}
"#
    );
    LibrarySet::resolve(&SourceBundle::new("main.maac", source).resolve()?)
}

fn program(kind: &str) -> InstrumentProgram {
    resolve(kind, "").unwrap().programs.remove(0)
}

fn runtime(program: &InstrumentProgram) -> InstrumentRuntime {
    let compiled = CompiledInstrument::compile(program, &BTreeMap::new()).unwrap();
    InstrumentRuntime::new(Arc::new(compiled), 4, 48_000.0, &BTreeMap::new()).unwrap()
}

/// The first frame of one note through a graph whose output is the source.
/// The voice path multiplies it by the unit envelope and by velocity.
fn first_frame(kind: &str, pitch_hz: f64, velocity: f64) -> f64 {
    let mut runtime = runtime(&program(kind));
    runtime.note_on("note", pitch_hz, velocity, 0).unwrap();
    runtime.render(0).unwrap()[0]
}

#[test]
fn sources_are_strict_and_round_trip() {
    for (kind, processor) in SOURCES {
        resolve(kind, "").unwrap();
        resolve(kind, "params = {};").unwrap();
        for extra in [
            "config = {};",
            "config = { seed = 1; };",
            "params = { level = 1; };",
        ] {
            assert!(resolve(kind, extra).is_err(), "{kind} {extra}");
        }
        assert_eq!(processor.identity(), kind);
        let wire = serde_json::json!({ "kind": kind });
        assert_eq!(serde_json::to_value(&processor).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<GraphProcessor>(wire).unwrap(),
            processor
        );
        let unversioned = kind.trim_end_matches("/1");
        for wire in [
            serde_json::json!({ "kind": unversioned }),
            serde_json::json!({ "kind": format!("{unversioned}/2") }),
            serde_json::json!({ "kind": kind, "config": {} }),
        ] {
            assert!(serde_json::from_value::<GraphProcessor>(wire).is_err());
        }
    }
}

#[test]
fn sources_are_voice_only_and_have_no_inputs_or_parameters() {
    for (kind, processor) in SOURCES {
        let mut shared = program(kind).voice;
        shared.nodes.retain(|node| node.processor == processor);
        shared.amplitude = None;
        assert_eq!(
            validate_graph(&shared, false, Some(1)).unwrap_err().code,
            "E_CAPABILITY",
            "{kind}"
        );

        let mut fed = program(kind);
        fed.voice.connections.push(
            maac::plan::Connection::new(
                "bad_input",
                PortRef::new("amp", "out").unwrap(),
                PortRef::new("note", "in").unwrap(),
            )
            .unwrap(),
        );
        assert!(fed.validate().is_err(), "{kind}");

        let mut configured = program(kind);
        configured.voice.nodes[1]
            .params
            .insert("level".into(), maac::parse_rational("1").unwrap());
        assert!(configured.validate().is_err(), "{kind}");
    }
}

#[test]
fn velocity_reads_back_the_note_velocity() {
    for velocity in [0.0, 0.2, 0.6, 1.0] {
        assert_eq!(
            first_frame("synth.velocity/1", 440.0, velocity),
            velocity * 1.0 * velocity
        );
    }
}

#[test]
fn velocity_holds_for_the_voice_life() {
    let mut runtime = runtime(&program("synth.velocity/1"));
    runtime.note_on("note", 440.0, 0.5, 0).unwrap();
    for frame in 0..64 {
        assert_eq!(runtime.render(frame).unwrap()[0], 0.25);
    }
}

#[test]
fn key_is_octaves_from_c4() {
    for (pitch_hz, key) in [(440.0, 0.75), (880.0, 1.75), (220.0, -0.25), (55.0, -2.25)] {
        assert_eq!(first_frame("synth.key/1", pitch_hz, 1.0), key);
    }
    let c4 = 440.0 * 2.0_f64.powf(-9.0 / 12.0);
    assert!(first_frame("synth.key/1", c4, 1.0).abs() < 1e-15);
    assert!((first_frame("synth.key/1", c4 / 2.0, 1.0) + 1.0).abs() < 1e-15);
}

fn composition(output: &str, notes: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 1/1000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument lead {{ channels = 1;
voice v {{ channels = 1; amplitude = &amp; output = &{output}:out;
node amp {{ type = "synth.adsr/1"; params = {{ attack = 0s; decay = 0s; sustain = 1; release = 0s; }}; }}
node key {{ type = "synth.key/1"; }}
node vel {{ type = "synth.velocity/1"; }} }} }}
node synth {{ instrument = &lead; }}
track t {{ target = &synth:events; }}
curve octave_up {{ clock = normalized; points = [(0, 1200ct, step), (1, 1200ct, step)]; }}
pattern phrase {{ length = 1/1000q; {notes} }}
place p1 {{ pattern = &phrase; track = &t; at = 0q; }}"#
    )
}

fn samples(text: &str) -> Vec<f64> {
    let plan = maac::compile(&maac::parse(text).unwrap()).unwrap();
    let mut result = vec![];
    maac::dsp::render(&plan, |frame| {
        result.push(frame[0]);
        Ok(())
    })
    .unwrap();
    result
}

#[test]
fn key_follows_per_note_pitch_expression() {
    let plain = samples(&composition(
        "key",
        "note a { at = 0q; dur = 1/1000q; pitch = A4; }",
    ));
    let raised = samples(&composition(
        "key",
        "note a { at = 0q; dur = 1/1000q; pitch = A4; expression up { kind = pitch; curve = &octave_up; } }",
    ));
    assert!(!plain.is_empty());
    assert!(plain.iter().all(|sample| *sample == 0.75));
    assert!(raised.iter().all(|sample| *sample == 1.75));
}

#[test]
fn velocity_reaches_the_graph_through_a_compiled_score() {
    let result = samples(&composition(
        "vel",
        "note a { at = 0q; dur = 1/1000q; pitch = A4; velocity = 0.4; }",
    ));
    assert!(result.iter().all(|sample| *sample == 0.4 * 0.4));
}

#[test]
fn velocity_drives_a_note_on_parameter() {
    // Velocity sets an envelope's decay curve at note-on.
    let mut program = program("synth.velocity/1");
    program.voice.nodes.push(maac::graph::GraphNode {
        id: "env".into(),
        processor: GraphProcessor::Adsr,
        params: BTreeMap::from([
            ("decay".into(), maac::parse_rational("1/480").unwrap()),
            ("sustain".into(), maac::parse_rational("0").unwrap()),
        ]),
    });
    program.voice.modulations.push(Modulation {
        id: "vel_curve".into(),
        from: PortRef::new("note", "out").unwrap(),
        to: ParameterTarget {
            node: "env".into(),
            parameter: "curve".into(),
        },
        depth: maac::parse_rational("10").unwrap(),
    });
    program.voice.output = PortRef::new("env", "out").unwrap();
    program.validate().unwrap();
    let mut runtime = runtime(&program);
    runtime.note_on("note", 440.0, 0.5, 0).unwrap();
    // A 100-frame decay with curve 10 * 0.5; frame 50 is halfway.
    let expected = maac::synth::curve_shape(5.0, 0.5) * 0.5;
    for frame in 0..50 {
        runtime.render(frame).unwrap();
    }
    assert!((runtime.render(50).unwrap()[0] - expected).abs() < 1e-15);
    // A velocity of 1 asks for curve 10 + 10 = 20, past the bound of 16.
    let mut base = program.clone();
    let env = base
        .voice
        .nodes
        .iter_mut()
        .find(|node| node.id == "env")
        .unwrap();
    env.params
        .insert("curve".into(), maac::parse_rational("10").unwrap());
    let mut runtime = self::runtime(&base);
    assert!(runtime.note_on("loud", 440.0, 1.0, 0).is_err());
    runtime.note_on("soft", 440.0, 0.5, 0).unwrap();
}

#[test]
fn a_kit_passes_each_hits_velocity_to_its_piece() {
    let source = r#"maac 1;
project p { score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &drums:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument tap { channels = 1;
  voice v { channels = 1; amplitude = &amp; output = &vel:out;
    node amp { type = "synth.adsr/1"; params = { attack = 0s; decay = 0s; sustain = 1; release = 0s; }; }
    node vel { type = "synth.velocity/1"; } } }
instrument kit { channels = 1; piece tap { instrument = &tap; key = "tap"; gate = 10ms; } }
node drums { instrument = &kit; }
pattern beat { length = 1q; hit a { at = 0q; key = "tap"; velocity = 0.8; } }
track t { target = &drums:events; }
place main { pattern = &beat; track = &t; at = 0q; }
"#;
    let plan = compile_bundle_artifact(&SourceBundle::new("main.maac", source)).unwrap();
    let mut first = None;
    render_artifact(&plan, |frame| {
        first.get_or_insert(frame[0]);
        Ok(())
    })
    .unwrap();
    assert_eq!(first, Some(0.8 * 0.8));
}

#[test]
fn programs_with_sources_round_trip_through_json() {
    for (kind, _) in SOURCES {
        let program = program(kind);
        let restored: InstrumentProgram =
            serde_json::from_str(&serde_json::to_string(&program).unwrap()).unwrap();
        assert_eq!(restored, program);
    }
    // The compiled plan path accepts both sources.
    let text = composition("key", "note a { at = 0q; dur = 1/1000q; pitch = A4; }");
    compile(&parse(&text).unwrap()).unwrap();
}
