use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle;
use maac::graph::{Control, ControlTarget, GraphProcessor, GraphStage, InstrumentProgram};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

fn source(config: &str, params: &str) -> String {
    format!(
        r#"maac 1;
project song {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument sound {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &noise:out;
  node amp {{ type = "synth.adsr/1"; }}
  node noise {{ type = "synth.noise/1"; {config} {params} }}
 }}
 control level {{ target = &v.noise.params.level; default = 1; }}
}}
node synth {{ instrument = &sound; }}
"#
    )
}

fn program(seed: u32) -> InstrumentProgram {
    compile_bundle(&SourceBundle::new(
        "noise.maac",
        source(&format!("config = {{ seed = {seed}; }};"), ""),
    ))
    .unwrap()
    .instruments
    .unwrap()
    .programs
    .remove(0)
}

fn runtime(program: &InstrumentProgram) -> InstrumentRuntime {
    let compiled = CompiledInstrument::compile(program, &BTreeMap::new()).unwrap();
    InstrumentRuntime::new(Arc::new(compiled), 4, 48_000.0, &BTreeMap::new()).unwrap()
}

fn sample(state: u32) -> f64 {
    state as f64 / 2147483648.0 - 1.0
}

#[test]
fn fixed_vectors_advance_before_output_and_ignore_pitch() {
    for (seed, states) in [
        (
            1831565813,
            [
                1085196063, 2447379481, 2618286376, 1701901981, 265159372, 1030440423,
            ],
        ),
        (
            1,
            [
                270369, 67634689, 2647435461, 307599695, 2398689233, 745495504,
            ],
        ),
        (
            u32::MAX,
            [
                253983, 4228382207, 1958451267, 4056713434, 2049502865, 2560970988,
            ],
        ),
    ] {
        for pitch in [20.0, 440.0, 20000.0] {
            let mut runtime = runtime(&program(seed));
            runtime.note_on("note", pitch, 1.0, 0).unwrap();
            for (frame, state) in states.into_iter().enumerate() {
                assert_eq!(
                    runtime.render(frame as u64).unwrap()[0],
                    sample(state),
                    "seed {seed}, frame {frame}"
                );
            }
        }
    }
}

#[test]
fn voices_restart_independently_and_reset_replays_exactly() {
    let mut runtime = runtime(&program(1));
    runtime.note_on("a", 440.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap()[0], sample(270369));
    runtime.note_on("b", 220.0, 0.5, 1).unwrap();
    assert_eq!(
        runtime.render(1).unwrap()[0],
        sample(67634689) + sample(270369) * 0.5
    );
    runtime.reset(&BTreeMap::new()).unwrap();
    runtime.note_on("a", 440.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap()[0], sample(270369));
}

#[test]
fn sample_rate_level_control_and_envelope_mute_do_not_pause_noise() {
    let mut program = program(1);
    let mut runtime = runtime(&program);
    runtime.note_on("a", 440.0, 1.0, 0).unwrap();
    runtime
        .update_controls(&[("level".into(), 0.0)].into_iter().collect())
        .unwrap();
    assert_eq!(runtime.render(0).unwrap()[0], 0.0);
    runtime
        .update_controls(&[("level".into(), 2.0)].into_iter().collect())
        .unwrap();
    assert_eq!(runtime.render(1).unwrap()[0], sample(67634689) * 2.0);

    program
        .voice
        .nodes
        .iter_mut()
        .find(|n| n.id == "amp")
        .unwrap()
        .params
        .insert("attack".into(), maac::parse_rational("1/48000").unwrap());
    let mut muted = self::runtime(&program);
    muted.note_on("a", 440.0, 1.0, 0).unwrap();
    assert_eq!(muted.render(0).unwrap()[0], 0.0);
    assert_eq!(muted.render(1).unwrap()[0], sample(67634689));
}

#[test]
fn source_seed_defaults_are_resolved_and_plan_wire_is_strict() {
    for config in ["", "config = {};"] {
        let plan = compile_bundle(&SourceBundle::new("noise.maac", source(config, ""))).unwrap();
        let node = plan.instruments.as_ref().unwrap().programs[0]
            .voice
            .nodes
            .iter()
            .find(|n| n.id == "noise")
            .unwrap();
        assert_eq!(
            serde_json::to_value(&node.processor).unwrap(),
            serde_json::json!({"kind":"synth.noise/1","seed":1831565813u32})
        );
        let wire = serde_json::to_vec(&plan).unwrap();
        maac::load_plan(wire.as_slice()).unwrap();
    }
    for wire in [
        r#"{"kind":"synth.noise/1"}"#,
        r#"{"kind":"synth.noise/1","seed":0}"#,
        r#"{"kind":"synth.noise/1","seed":4294967296}"#,
        r#"{"kind":"synth.noise/1","seed":1.5}"#,
        r#"{"kind":"synth.noise/1","seed":-1}"#,
        r#"{"kind":"synth.noise/1","seed":1,"channels":1}"#,
        r#"{"kind":"synth.noise/2","seed":1}"#,
    ] {
        assert!(
            serde_json::from_str::<GraphProcessor>(wire).is_err(),
            "{wire}"
        );
    }
}

#[test]
fn source_rejects_bad_seed_unknown_config_and_non_level_parameters() {
    for config in [
        "seed = 0;",
        "seed = -1;",
        "seed = 4294967296;",
        "seed = 1/2;",
        "seed = 1Hz;",
        "seed = true;",
        "seed = 1; channels = 1;",
    ] {
        assert!(
            compile_bundle(&SourceBundle::new(
                "noise.maac",
                source(&format!("config = {{ {config} }};"), "")
            ))
            .is_err(),
            "{config}"
        );
    }
    for params in [
        "level = -1;",
        "level = 17;",
        "level = 1Hz;",
        "frequency = 0Hz;",
        "phase = 0;",
        "ratio = 1;",
    ] {
        assert!(
            compile_bundle(&SourceBundle::new(
                "noise.maac",
                source("", &format!("params = {{ {params} }};"))
            ))
            .is_err(),
            "{params}"
        );
    }
}

#[test]
fn graph_rejects_noise_in_shared_stage_and_unknown_control_parameters() {
    let mut program = program(1);
    let mut shared = program.voice.clone();
    shared.nodes.retain(|n| n.id == "noise");
    shared.amplitude = None;
    program.shared = Some(shared);
    assert_eq!(program.validate().unwrap_err().code, "E_CAPABILITY");
    program.shared = None;
    program.controls.insert(
        "pitch".into(),
        Control {
            target: ControlTarget {
                graph: GraphStage::Voice,
                node: "noise".into(),
                parameter: "frequency".into(),
            },
            default: maac::parse_rational("1").unwrap(),
        },
    );
    assert!(program.validate().is_err());
}

#[test]
fn typed_plan_rejects_zero_seed_and_level_metadata_is_sample_rate() {
    let processor = GraphProcessor::Noise { seed: 1 };
    let spec = maac::graph::parameter_descriptor(&processor, "level").unwrap();
    assert_eq!(spec.rate, maac::graph::ParameterRate::Sample);
    assert_eq!(spec.unit, maac::graph::GraphUnit::Dimensionless);
    assert_eq!(spec.default, maac::parse_rational("1").unwrap());
    spec.validate(&maac::parse_rational("0").unwrap()).unwrap();
    spec.validate(&maac::parse_rational("16").unwrap()).unwrap();
    let mut plan = compile_bundle(&SourceBundle::new("noise.maac", source("", ""))).unwrap();
    plan.instruments.as_mut().unwrap().programs[0]
        .voice
        .nodes
        .iter_mut()
        .find(|n| n.id == "noise")
        .unwrap()
        .processor = GraphProcessor::Noise { seed: 0 };
    assert_eq!(plan.validate().unwrap_err().code, "E_RANGE");
}

#[test]
fn automated_source_and_loaded_plan_render_exactly_and_engine_reset_replays() {
    let source = source("config = { seed = 1; };", "").replace("[0q, 1q]", "[0q, 1/6000q]")
        + r#"
pattern phrase { length = 1/6000q; note n { at = 0q; dur = 1/6000q; pitch = C4; velocity = 1; } }
track melody { target = &synth:events; }
place play { pattern = &phrase; track = &melody; at = 0q; }
curve volume { clock = seconds; points = [(0s, 0, linear), (1/24000s, 1, step)]; }
automation fade { target = &synth.params.level; curve = &volume; at = 0s; }
"#;
    let plan = compile_bundle(&SourceBundle::new("noise.maac", source)).unwrap();
    let wire = serde_json::to_vec(&plan).unwrap();
    let loaded = maac::load_plan(wire.as_slice()).unwrap();
    let mut engine = maac::dsp::DspEngine::new(&plan).unwrap();
    let collect = |engine: &mut maac::dsp::DspEngine<'_>| {
        let mut samples = Vec::new();
        engine
            .render(|frame| {
                samples.push(frame[0]);
                Ok(())
            })
            .unwrap();
        samples
    };
    let expected = vec![
        0.0,
        sample(67634689) * 0.5,
        sample(2647435461),
        sample(307599695),
    ];
    assert_eq!(collect(&mut engine), expected);
    assert_eq!(collect(&mut engine), expected);
    assert_eq!(
        collect(&mut maac::dsp::DspEngine::new(&loaded).unwrap()),
        expected
    );
}
