//! A `synth.*/1` node's config is read as its processor's declaration says,
//! so a wrong config is reported the same way for every processor.

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::graph::{validate_graph, GraphProgram};
use maac::plan::PlanLimits;
use maac::{Diagnostic, DiagnosticCode, SourceBundle};
use serde_json::json;

/// A song whose instrument has a node `x` of `kind` with `config`.
fn song(kind: &str, config: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 2q]; rate = 48000Hz; tail = 1s; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument tone {{
  channels = 1;
  voice v {{
    channels = 1; amplitude = &amp; output = &osc:out;
    node amp {{ type = "synth.adsr/1"; }}
    node osc {{ type = "synth.sine/1"; }}
    node x {{ type = "{kind}"; {config} }}
  }}
}}
node sound {{ instrument = &tone; config = {{ voices = 1; }}; }}
track t {{ target = &sound:events; }}
pattern one {{ length = 1q; note a {{ at = 0q; dur = 1q; pitch = A4; }} }}
place p1 {{ pattern = &one; track = &t; at = 0q; }}
"#
    )
}

fn refused(kind: &str, config: &str) -> Diagnostic {
    let bundle = SourceBundle::new("main.maac", song(kind, config));
    let diagnostics = compile_bundle_artifact_with_limits(&bundle, &PlanLimits::song())
        .expect_err("the config is wrong");
    let first = diagnostics.iter().next().expect("one diagnostic").clone();
    first
}

#[test]
fn a_wrong_config_is_reported_the_same_way_for_every_processor() {
    use DiagnosticCode::{Range, Unit, UnknownField};
    // (processor, config, code, message, the field it is reported at)
    let cases: &[(&str, &str, DiagnosticCode, &str, &[&str])] = &[
        // A processor without config takes none, not even an empty one.
        (
            "synth.sine/1",
            "config = { };",
            UnknownField,
            "processor `synth.sine/1` does not accept config",
            &[],
        ),
        (
            "synth.key/1",
            "config = { };",
            UnknownField,
            "processor `synth.key/1` does not accept config",
            &[],
        ),
        // A processor with a required key needs a config, and the key.
        (
            "synth.gain/1",
            "",
            UnknownField,
            "processor `synth.gain/1` requires config",
            &[],
        ),
        (
            "synth.gain/1",
            "config = { };",
            UnknownField,
            "processor `synth.gain/1` requires config.channels",
            &[],
        ),
        (
            "synth.svf/1",
            "config = { channels = 1; };",
            UnknownField,
            "processor `synth.svf/1` requires config.mode",
            &[],
        ),
        (
            "synth.modal/1",
            "config = { strings = 2; };",
            UnknownField,
            "processor `synth.modal/1` requires config.modes",
            &[],
        ),
        (
            "synth.delay/1",
            "config = { channels = 1; };",
            UnknownField,
            "processor `synth.delay/1` requires config.max",
            &[],
        ),
        (
            "synth.sample/1",
            "config = { channels = 1; };",
            UnknownField,
            "processor `synth.sample/1` requires config.zones",
            &[],
        ),
        // A key the processor lacks is reported at the key, with the keys
        // it has, optional ones too.
        (
            "synth.random/1",
            "config = { x = 1; };",
            UnknownField,
            "processor `synth.random/1` has one config field, seed",
            &["x"],
        ),
        (
            "synth.noise/1",
            "config = { seed = 1; other = 2; };",
            UnknownField,
            "processor `synth.noise/1` config fields are per_note and seed",
            &["other"],
        ),
        (
            "synth.modal/1",
            "config = { modes = 4; x = 9; };",
            UnknownField,
            "processor `synth.modal/1` config fields are modes and strings",
            &["x"],
        ),
        (
            "synth.bow/1",
            "config = { x = 1; };",
            UnknownField,
            "processor `synth.bow/1` config fields are body, channels, players and seed",
            &["x"],
        ),
        (
            "synth.delay/1",
            "config = { channels = 1; max = 1s; x = 1; };",
            UnknownField,
            "processor `synth.delay/1` config fields are channels, clock and max",
            &["x"],
        ),
        // A value of the wrong kind, or outside the key's range or symbols.
        (
            "synth.noise/1",
            "config = { per_note = 1; };",
            Unit,
            "per_note must be a boolean",
            &["per_note"],
        ),
        (
            "synth.noise/1",
            "config = { seed = 0; };",
            Range,
            "seed is outside 1 through 4294967295",
            &["seed"],
        ),
        (
            "synth.drive/1",
            "config = { channels = 3; };",
            Range,
            "channels is outside 1 through 2",
            &["channels"],
        ),
        (
            "synth.modal/1",
            "config = { modes = 65; };",
            Range,
            "modes is outside 1 through 64",
            &["modes"],
        ),
        (
            "synth.bow/1",
            "config = { players = 17; };",
            Range,
            "players is outside 1 through 16",
            &["players"],
        ),
        (
            "synth.bow/1",
            "config = { body = piano; };",
            Range,
            "body must be violin, viola, cello, bass or none",
            &["body"],
        ),
        (
            "synth.adsr/1",
            "config = { on_join = 1; };",
            Range,
            "on_join must be hold or restart",
            &["on_join"],
        ),
        (
            "synth.svf/1",
            "config = { channels = 1; mode = notch; };",
            Range,
            "mode must be lowpass, bandpass or highpass",
            &["mode"],
        ),
        (
            "synth.delay/1",
            "config = { channels = 1; max = 1s; clock = tape; };",
            Range,
            "clock must be seconds or score",
            &["clock"],
        ),
        (
            "synth.delay/1",
            "config = { channels = 1; max = 1q; };",
            Unit,
            "parameter requires seconds using s or ms",
            &["max"],
        ),
        (
            "synth.sample/1",
            "config = { zones = []; fade_shape = round; };",
            Range,
            "fade_shape must be linear or equal_power",
            &["fade_shape"],
        ),
    ];
    for (kind, config, code, message, field) in cases {
        let diagnostic = refused(kind, config);
        let case = format!("{kind} {config}");
        assert_eq!(diagnostic.code, *code, "{case}: {diagnostic:?}");
        assert!(
            diagnostic.message.ends_with(message),
            "{case}: {diagnostic:?}"
        );
        assert_eq!(
            diagnostic
                .field_path
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            *field,
            "{case}: {diagnostic:?}"
        );
    }
}

#[test]
fn a_config_whose_keys_are_all_optional_may_be_left_out_or_empty() {
    for (kind, config) in [
        ("synth.noise/1", ""),
        ("synth.noise/1", "config = { };"),
        ("synth.noise/1", "config = { per_note = true; };"),
        ("synth.random/1", "config = { };"),
        ("synth.pluck/1", "config = { seed = 3; };"),
        ("synth.adsr/1", "config = { };"),
        ("synth.adsr/1", "config = { on_join = restart; };"),
        ("synth.bow/1", ""),
        ("synth.bow/1", "config = { players = 2; body = cello; };"),
    ] {
        let bundle = SourceBundle::new("main.maac", song(kind, config));
        compile_bundle_artifact_with_limits(&bundle, &PlanLimits::song())
            .unwrap_or_else(|error| panic!("{kind} {config}: {error:?}"));
    }
}

#[test]
fn a_plan_refuses_a_whole_number_outside_its_declared_range() {
    // (the node's processor, the key, the message)
    for (processor, key, message) in [
        (
            json!({"kind": "synth.svf/1", "channels": 3, "mode": "lowpass"}),
            "channels",
            "channels must be from 1 to 2",
        ),
        (
            json!({"kind": "synth.bow/1", "players": 17, "body": "violin", "channels": 1, "seed": 1}),
            "players",
            "players must be from 1 to 16",
        ),
        (
            json!({"kind": "synth.bow/1", "players": 1, "body": "violin", "channels": 0, "seed": 1}),
            "channels",
            "channels must be from 1 to 2",
        ),
        (
            json!({"kind": "synth.modal/1", "modes": 65, "strings": 1}),
            "modes",
            "modes must be from 1 to 64",
        ),
        (
            json!({"kind": "synth.modal/1", "modes": 4, "strings": 0}),
            "strings",
            "strings must be from 1 to 3",
        ),
    ] {
        let graph: GraphProgram = serde_json::from_value(json!({
            "channels": 1,
            "nodes": [{"id": "x", "processor": processor}],
            "output": {"node": "x", "port": "out"},
        }))
        .unwrap_or_else(|error| panic!("{processor}: {error}"));
        let error = validate_graph(&graph, true, None).unwrap_err();
        assert_eq!(error.code, "E_RANGE", "{processor}: {error:?}");
        assert!(error.path.ends_with(&format!(".{key}")), "{error:?}");
        assert_eq!(error.message, message, "{processor}");
    }
}
