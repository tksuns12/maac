//! Effects: a graph under an instrument's shared-graph rules that processes
//! the audio summed at its node's `in`, behind named controls.

use std::collections::BTreeMap;

use maac::bundle::sha256_digest;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::editing::{
    apply_transaction, AuthoredDocument, BundleEditContext, Operation, Transaction,
};
use maac::plan::PlanLimits;
use maac::{render_artifact_with_limits, DiagnosticCode, PlanArtifact, SourceBundle};
use serde_json::json;

/// Two sine notes into whatever `graph` declares; the output is `mix`.
fn song(graph: &str, channels: u8) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 8q]; rate = 48000Hz; tail = 200ms; tempo = &clock; meter = &metre; output = &mix:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node src {{ type = "core.sine/1"; params = {{ level = 0.5; }}; }}
pattern c {{ length = 4q;
  note a {{ at = 0q; dur = 2q; pitch = A4; velocity = 0.8; }}
  note b {{ at = 2q; dur = 2q; pitch = E5; velocity = 0.6; }}
}}
track t {{ target = &src:events; }}
place pl {{ pattern = &c; track = &t; at = 0q; count = 2; }}
node mix {{ type = "core.sum/1"; config = {{ channels = {channels}; }}; }}
{graph}
"#
    )
}

/// A trim and a one-pole low-pass, the trim exposed as `amount`.
const DARK: &str = r#"effect dark {
  channels = 1;
  shared main {
    channels = 1; output = &tone:out;
    node trim { type = "synth.gain/1"; config = { channels = 1; }; params = { level = 0.5; }; }
    node tone { type = "synth.onepole/1"; config = { channels = 1; }; params = { cutoff = 800Hz; }; }
    connect in_trim { from = &input:out; to = &trim:in; }
    connect trim_tone { from = &trim:out; to = &tone:in; }
  }
  control amount { target = &main.trim.params.level; default = 0.5; }
}"#;

fn bundle(entry: &str) -> SourceBundle {
    SourceBundle::new("main.maac", entry)
}

fn compile(bundle: &SourceBundle) -> Result<PlanArtifact, maac::Diagnostics> {
    compile_bundle_artifact_with_limits(bundle, &PlanLimits::song())
}

fn render_plan(plan: &PlanArtifact) -> Vec<f64> {
    let mut out = Vec::new();
    render_artifact_with_limits(plan, &PlanLimits::song(), |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    out
}

fn render(source: &str) -> Vec<f64> {
    render_plan(&compile(&bundle(source)).unwrap_or_else(|error| panic!("{error:?}")))
}

fn codes(source: &str) -> Vec<DiagnosticCode> {
    compile(&bundle(source))
        .unwrap_err()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn an_effect_renders_as_the_same_nodes_built_from_core_processors() {
    let effect = render(&song(
        &format!(
            "{DARK}
node fx {{ effect = &dark; params = {{ amount = 0.25; }}; }}
connect s_fx {{ from = &src:out; to = &fx:in; }}
connect fx_mix {{ from = &fx:out; to = &mix:in; }}"
        ),
        1,
    ));
    let core = render(&song(
        r#"node trim { type = "core.gain/1"; config = { channels = 1; }; params = { gain = 0.25; }; }
node tone { type = "core.onepole/1"; config = { channels = 1; }; params = { cutoff = 800Hz; }; }
connect s_trim { from = &src:out; to = &trim:in; }
connect trim_tone { from = &trim:out; to = &tone:in; }
connect tone_mix { from = &tone:out; to = &mix:in; }"#,
        1,
    ));
    assert!(effect.iter().any(|sample| *sample != 0.0));
    assert_eq!(effect, core);
}

#[test]
fn an_effect_sums_its_inputs_and_follows_automation_and_modulation() {
    let effect = render(&song(
        &format!(
            "{DARK}
node fx {{ effect = &dark; }}
curve ramp {{ clock = score; points = [(0q, 0.2, linear), (8q, 0.8, step)]; }}
automation fade {{ target = &fx.params.amount; curve = &ramp; at = 0q; }}
node wobble {{ type = \"core.lfo/1\"; config = {{ period = 1q; }}; }}
modulate wob {{ from = &wobble:out; target = &fx.params.amount; amount = 0.1; }}
connect s_fx {{ from = &src:out; to = &fx:in; }}
connect again {{ from = &src:out; to = &fx:in; }}
connect fx_mix {{ from = &fx:out; to = &mix:in; }}"
        ),
        1,
    ));
    let core = render(&song(
        r#"node both { type = "core.sum/1"; config = { channels = 1; }; }
node trim { type = "core.gain/1"; config = { channels = 1; }; }
node tone { type = "core.onepole/1"; config = { channels = 1; }; params = { cutoff = 800Hz; }; }
curve ramp { clock = score; points = [(0q, 0.2, linear), (8q, 0.8, step)]; }
automation fade { target = &trim.params.gain; curve = &ramp; at = 0q; }
node wobble { type = "core.lfo/1"; config = { period = 1q; }; }
modulate wob { from = &wobble:out; target = &trim.params.gain; amount = 0.1; }
connect s_both { from = &src:out; to = &both:in; }
connect again { from = &src:out; to = &both:in; }
connect both_trim { from = &both:out; to = &trim:in; }
connect trim_tone { from = &trim:out; to = &tone:in; }
connect tone_mix { from = &tone:out; to = &mix:in; }"#,
        1,
    ));
    assert_eq!(effect, core);
}

#[test]
fn a_mono_input_effect_can_give_stereo_and_its_plan_replays_alone() {
    let source = song(
        r#"effect place {
  channels = 2; input = 1;
  shared main {
    channels = 2; output = &pan:out;
    node heat { type = "synth.drive/1"; config = { channels = 1; }; params = { drive = 4; reference = 1; }; }
    node swing { type = "synth.lfo/1"; params = { frequency = 2Hz; }; }
    node pan { type = "synth.pan/1"; }
    connect in_heat { from = &input:out; to = &heat:in; }
    connect heat_pan { from = &heat:out; to = &pan:in; }
    modulate sway { from = &swing:out; to = &pan.params.pan; depth = 0.6; }
  }
  control drive { target = &main.heat.params.drive; default = 4; }
}
node fx { effect = &place; params = { drive = 8; }; }
connect s_fx { from = &src:out; to = &fx:in; }
connect fx_mix { from = &fx:out; to = &mix:in; }"#,
        2,
    );
    let plan = compile(&bundle(&source)).unwrap();
    let direct = render_plan(&plan);
    let json = plan.to_json().unwrap();
    let text = String::from_utf8(json.clone()).unwrap();
    assert!(text.contains(r#""kind":"effect""#));
    assert!(text.contains(r#""input_channels":1"#));
    let replayed = render_plan(&PlanArtifact::from_json(&json).unwrap());
    assert_eq!(direct, replayed);
    // The pan moves: the channels differ from frame to frame.
    let differs = direct
        .chunks(2)
        .filter(|frame| (frame[0] - frame[1]).abs() > 1e-6)
        .count();
    assert!(differs > 1000);
}

#[test]
fn plans_without_effects_carry_no_effect_resources() {
    let plan = compile(&bundle(&song(
        r#"connect s_mix { from = &src:out; to = &mix:in; }"#,
        1,
    )))
    .unwrap();
    let text = String::from_utf8(plan.to_json().unwrap()).unwrap();
    assert!(!text.contains("effects"));
}

#[test]
fn a_library_exports_effects_and_presets_that_a_song_imports_with_a_pin() {
    let library = format!(
        r#"maac 1;
library fx {{ version = "1.0.0"; }}
{DARK}
preset darker {{ effect = &dark; params = {{ amount = 0.125; }}; }}
"#
    );
    let pin = sha256_digest(library.as_bytes());
    let entry = song(
        &format!(
            r#"import sounds {{ path = "fx.maac"; hash = "{pin}"; }}
node fx {{ effect = &sounds.dark; preset = &sounds.darker; }}
connect s_fx {{ from = &src:out; to = &fx:in; }}
connect fx_mix {{ from = &fx:out; to = &mix:in; }}"#
        ),
        1,
    );
    let mut imported = bundle(&entry);
    imported.sources.insert("fx.maac".into(), library);
    let samples = render_plan(&compile(&imported).unwrap());
    let local = render(&song(
        &format!(
            "{DARK}
node fx {{ effect = &dark; params = {{ amount = 0.125; }}; }}
connect s_fx {{ from = &src:out; to = &fx:in; }}
connect fx_mix {{ from = &fx:out; to = &mix:in; }}"
        ),
        1,
    ));
    assert_eq!(samples, local);
}

#[test]
fn malformed_effects_are_rejected() {
    let with = |node: &str| {
        song(
            &format!(
                "{DARK}
{node}
connect fx_mix {{ from = &fx:out; to = &mix:in; }}"
            ),
            1,
        )
    };
    let input = "connect s_fx { from = &src:out; to = &fx:in; }";
    assert!(codes(&with("node fx { effect = &dark; }")).contains(&DiagnosticCode::PortType));
    assert!(codes(&with(&format!(
        "node fx {{ effect = &dark; params = {{ gain = 1; }}; }}\n{input}"
    )))
    .contains(&DiagnosticCode::UnknownField));
    assert!(codes(&with(&format!(
        "node fx {{ effect = &dark; config = {{ voices = 2; }}; }}\n{input}"
    )))
    .contains(&DiagnosticCode::UnknownField));
    assert!(codes(&with(&format!(
        "node fx {{ effect = &dark; params = {{ amount = 99; }}; }}\n{input}"
    )))
    .contains(&DiagnosticCode::Range));
    assert!(
        codes(&with(&format!("node fx {{ effect = &mix; }}\n{input}")))
            .contains(&DiagnosticCode::Reference)
    );
    // A voice-only node, a voice graph and a channel count that differs from
    // the graph's output.
    let declared = |declaration: &str| {
        song(
            &format!(
                "{declaration}
node fx {{ effect = &bad; }}
{input}
connect fx_mix {{ from = &fx:out; to = &mix:in; }}"
            ),
            1,
        )
    };
    let gain = r#"node g { type = "synth.gain/1"; config = { channels = 1; }; } connect i { from = &input:out; to = &g:in; }"#;
    assert!(codes(&declared(
        r#"effect bad { channels = 1; shared main { channels = 1; output = &osc:out; node osc { type = "synth.sine/1"; } } }"#
    ))
    .contains(&DiagnosticCode::Capability));
    assert!(codes(&declared(&format!(
        "effect bad {{ channels = 1; voice v {{ channels = 1; output = &g:out; }} shared main {{ channels = 1; output = &g:out; {gain} }} }}"
    )))
    .contains(&DiagnosticCode::UnknownKind));
    assert!(codes(&declared(&format!(
        "effect bad {{ channels = 2; shared main {{ channels = 1; output = &g:out; {gain} }} }}"
    )))
    .contains(&DiagnosticCode::PortType));
}

#[test]
fn an_effect_node_and_its_graph_are_editable() {
    let source = song(
        &format!(
            "{DARK}
node fx {{ effect = &dark; }}
connect s_fx {{ from = &src:out; to = &fx:in; }}
connect fx_mix {{ from = &fx:out; to = &mix:in; }}"
        ),
        1,
    );
    let bundle = SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([("main.maac".into(), source.clone())]),
        assets: BTreeMap::new(),
    };
    let context = BundleEditContext::new(&bundle).unwrap();
    let mut document = AuthoredDocument::from_document(&maac::parse(&source).unwrap()).unwrap();
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![
            Operation::Set {
                object: vec!["fx".into()],
                field: vec!["params".into()],
                value: json!({"t":"record","fields":{"amount":{"t":"number","n":"1","d":"8"}}}),
                expect: None,
                expect_absent: true,
            },
            Operation::Set {
                object: vec!["dark".into(), "main".into(), "tone".into()],
                field: vec!["params".into(), "cutoff".into()],
                value: json!({"t":"quantity","n":"500","d":"1","u":"Hz"}),
                expect: None,
                expect_absent: false,
            },
        ],
    )
    .unwrap();
    apply_transaction(&mut document, &transaction, &context).unwrap();
    assert_eq!(
        document.tree()["objects"]["fx"]["fields"]["params"]["fields"]["amount"],
        json!({"t":"number","n":"1","d":"8"})
    );

    // Normalized, an effect node holds its controls' values, defaults
    // included, as an instrument node does: an expectation can name them.
    let mut document = AuthoredDocument::from_document(&maac::parse(&source).unwrap()).unwrap();
    let revision = document.revision().to_owned();
    let expecting = |amount: &str| {
        Transaction::new(
            revision.clone(),
            vec![Operation::Set {
                object: vec!["fx".into()],
                field: vec!["params".into()],
                value: json!({"t":"record","fields":{"amount":{"t":"number","n":"1","d":"4"}}}),
                expect: Some(
                    json!({"t":"record","fields":{"amount":{"t":"number","n":amount,"d":"2"}}}),
                ),
                expect_absent: false,
            }],
        )
        .unwrap()
    };
    let wrong = expecting("0");
    assert!(apply_transaction(&mut document, &wrong, &context).is_err());
    let right = expecting("1");
    apply_transaction(&mut document, &right, &context).unwrap();
}

/// Samples of channel `channel` of an interleaved render.
fn channel(samples: &[f64], channels: usize, channel: usize) -> Vec<f64> {
    samples
        .iter()
        .skip(channel)
        .step_by(channels)
        .copied()
        .collect()
}

/// A stereo source, the sine panned left of centre, through `node`.
fn stereo_through(effect: &str, params: &str) -> Vec<f64> {
    render(&song(
        &format!(
            r#"effect fx_decl {{
  channels = 2;
  shared main {{
    channels = 2; output = &stage:out;
    {effect}
    connect in_stage {{ from = &input:out; to = &stage:in; }}
  }}
}}
node place {{ type = "core.pan/1"; params = {{ pan = -0.4; }}; }}
node fx {{ effect = &fx_decl; }}
connect s_place {{ from = &src:out; to = &place:in; }}
connect place_fx {{ from = &place:out; to = &fx:in; }}
connect fx_mix {{ from = &fx:out; to = &mix:in; }}{params}"#
        ),
        2,
    ))
}

#[test]
fn balance_lowers_the_far_side_and_width_moves_between_mono_and_wide() {
    let dry = stereo_through(
        r#"node stage { type = "synth.gain/1"; config = { channels = 2; }; }"#,
        "",
    );
    let balanced = |value: &str| {
        stereo_through(
            &format!(
                r#"node stage {{ type = "synth.balance/1"; params = {{ balance = {value}; }}; }}"#
            ),
            "",
        )
    };
    assert_eq!(balanced("0"), dry);
    let right = balanced("1");
    assert!(channel(&right, 2, 0).iter().all(|sample| *sample == 0.0));
    assert_eq!(channel(&right, 2, 1), channel(&dry, 2, 1));
    let left = balanced("-1");
    assert!(channel(&left, 2, 1).iter().all(|sample| *sample == 0.0));
    assert_eq!(channel(&left, 2, 0), channel(&dry, 2, 0));

    let wide = |value: &str| {
        stereo_through(
            &format!(
                r#"node stage {{ type = "synth.width/1"; params = {{ width = {value}; }}; }}"#
            ),
            "",
        )
    };
    let unchanged = wide("1");
    for (a, b) in unchanged.iter().zip(&dry) {
        assert!((a - b).abs() < 1e-15);
    }
    let mono = wide("0");
    assert_eq!(channel(&mono, 2, 0), channel(&mono, 2, 1));
    // Wider than the source: the channels differ more than the dry ones do.
    let spread = |samples: &[f64]| {
        samples
            .chunks(2)
            .map(|frame| (frame[0] - frame[1]).abs())
            .sum::<f64>()
    };
    assert!(spread(&wide("2")) > 1.9 * spread(&dry));
}

/// A mono echo whose line is configured by `config` and `params`.
fn echo(config: &str, params: &str, tempo: &str) -> Vec<f64> {
    render(&echo_source(config, params, tempo))
}

fn echo_source(config: &str, params: &str, tempo: &str) -> String {
    let source = song(
        &format!(
            r#"effect echo {{
  channels = 1;
  shared main {{
    channels = 1; output = &line:out;
    node line {{ type = "synth.delay/1"; config = {{ channels = 1; {config} }}; params = {{ {params} }}; }}
    connect in_line {{ from = &input:out; to = &line:in; }}
  }}
}}
node fx {{ effect = &echo; }}
connect s_fx {{ from = &src:out; to = &fx:in; }}
connect fx_mix {{ from = &fx:out; to = &mix:in; }}"#
        ),
        1,
    );
    source.replace("tempo clock { points = [(0q, 120bpm, step)]; }", tempo)
}

const STEADY: &str = "tempo clock { points = [(0q, 120bpm, step)]; }";

#[test]
fn a_delay_in_whole_frames_matches_core_delay_and_one_in_beats_follows_the_tempo() {
    let core = render(&song(
        r#"node line { type = "core.delay/1"; config = { channels = 1; frames = 12000; }; }
connect s_line { from = &src:out; to = &line:in; }
connect line_mix { from = &line:out; to = &mix:in; }"#,
        1,
    ));
    // 250 ms is 12000 frames: no interpolation, the same samples.
    assert_eq!(echo("max = 1s;", "time = 250ms;", STEADY), core);
    // Half a beat at 120 bpm is the same 12000 frames, through the tempo
    // map in binary64.
    let beats = echo("clock = score; max = 1s;", "time = 1/2q;", STEADY);
    for (a, b) in beats.iter().zip(&core) {
        assert!((a - b).abs() < 1e-9);
    }
}

#[test]
fn an_echo_in_beats_lands_on_the_beat_across_a_tempo_ramp() {
    // From 60 bpm at 0q to 180 at 8q: T(q) = 4 ln(1 + q/4) seconds. The
    // pattern's notes start at 0q and 2q; an echo of one beat brings the
    // second note's onset from T(2) to T(3), not to T(2) plus one beat at
    // any single tempo.
    let ramp = "tempo clock { points = [(0q, 60bpm, linear), (8q, 180bpm, step)]; }";
    // One short note at 2q, alone, so that its onset is the first sound.
    let short = |source: String| {
        render(
            &source
                .replace(
                    "note a { at = 0q; dur = 2q; pitch = A4; velocity = 0.8; }",
                    "",
                )
                .replace("count = 2;", "count = 1;")
                .replace(
                    "note b { at = 2q; dur = 2q;",
                    "note b { at = 2q; dur = 1/8q;",
                ),
        )
    };
    let dry = short(echo_source("max = 1s;", "time = 1/48000s;", ramp));
    let wet = short(echo_source("clock = score; max = 2s;", "time = 1q;", ramp));
    let time = |q: f64| 4.0 * (1.0 + q / 4.0).ln();
    // The first sounding frame at or after a position.
    let onset = |samples: &[f64], from: f64| {
        let start = (from * 48_000.0) as usize;
        start
            + samples[start..]
                .iter()
                .position(|sample| sample.abs() > 1e-6)
                .unwrap()
    };
    let note = onset(&dry, 0.0);
    let echoed = onset(&wet, 0.0);
    let expected = time(3.0) * 48_000.0;
    assert!((note as f64 - time(2.0) * 48_000.0).abs() < 3.0, "{note}");
    assert!(
        (echoed as f64 - expected).abs() < 3.0,
        "{echoed} vs {expected}"
    );
    // One beat at the starting tempo would have come far earlier.
    assert!(echoed as f64 > time(2.0) * 48_000.0 + 0.5 * 48_000.0 * 60.0 / 120.0);
}

#[test]
fn a_modulated_delay_wavers_and_feedback_repeats() {
    let steady = echo("max = 50ms;", "time = 10ms;", STEADY);
    let chorus = render(&song(
        r#"effect chorus {
  channels = 1;
  shared main {
    channels = 1; output = &line:out;
    node line { type = "synth.delay/1"; config = { channels = 1; max = 50ms; }; params = { time = 10ms; }; }
    node wobble { type = "synth.lfo/1"; params = { frequency = 3Hz; }; }
    connect in_line { from = &input:out; to = &line:in; }
    modulate sway { from = &wobble:out; to = &line.params.time; depth = 3ms; }
  }
}
node fx { effect = &chorus; }
connect s_fx { from = &src:out; to = &fx:in; }
connect fx_mix { from = &fx:out; to = &mix:in; }"#,
        1,
    ));
    assert_ne!(chorus, steady);
    // The source ends at 8q (4 s) and its release 80 ms later. A fed-back
    // echo keeps sounding in the 200 ms tail; without feedback the line is
    // empty 10 ms after the release.
    let tail = |samples: &[f64]| {
        samples[4 * 48_000 + 6_000..]
            .iter()
            .map(|s| s.abs())
            .sum::<f64>()
    };
    let repeats = echo("max = 50ms;", "time = 10ms; feedback = 0.9;", STEADY);
    assert!(tail(&steady) == 0.0);
    assert!(tail(&repeats) > 0.0);
}

#[test]
fn malformed_delays_are_rejected() {
    let bad = |config: &str, params: &str| {
        let source = song(
            &format!(
                r#"effect echo {{
  channels = 1;
  shared main {{
    channels = 1; output = &line:out;
    node line {{ type = "synth.delay/1"; config = {{ {config} }}; params = {{ {params} }}; }}
    connect in_line {{ from = &input:out; to = &line:in; }}
  }}
}}
node fx {{ effect = &echo; }}
connect s_fx {{ from = &src:out; to = &fx:in; }}
connect fx_mix {{ from = &fx:out; to = &mix:in; }}"#
            ),
            1,
        );
        codes(&source)
    };
    assert!(bad("channels = 1;", "").contains(&DiagnosticCode::UnknownField));
    assert!(bad("max = 1s;", "").contains(&DiagnosticCode::UnknownField));
    assert!(bad("channels = 1; max = 61s;", "").contains(&DiagnosticCode::Range));
    assert!(bad("channels = 1; max = 1s; clock = beats;", "").contains(&DiagnosticCode::Range));
    assert!(bad("channels = 1; max = 1s; size = 2;", "").contains(&DiagnosticCode::UnknownField));
    assert!(bad("channels = 1; max = 1s;", "time = 1/2q;").contains(&DiagnosticCode::Unit));
    assert!(
        bad("channels = 1; max = 1s; clock = score;", "time = 1s;").contains(&DiagnosticCode::Unit)
    );
    assert!(bad("channels = 1; max = 1s;", "feedback = 1;").contains(&DiagnosticCode::Range));

    // A length beyond the line's max stops the render where it occurs.
    let plan = compile(&bundle(&song(
        r#"effect echo {
  channels = 1;
  shared main {
    channels = 1; output = &line:out;
    node line { type = "synth.delay/1"; config = { channels = 1; max = 100ms; }; params = { time = 200ms; }; }
    connect in_line { from = &input:out; to = &line:in; }
  }
}
node fx { effect = &echo; }
connect s_fx { from = &src:out; to = &fx:in; }
connect fx_mix { from = &fx:out; to = &mix:in; }"#,
        1,
    )))
    .unwrap();
    let error = render_artifact_with_limits(&plan, &PlanLimits::song(), |_| Ok(())).unwrap_err();
    assert!(format!("{error:?}").contains("E_RANGE"), "{error:?}");

    // An instrument cannot hold a delay line.
    let instrument = r#"instrument held {
  channels = 1;
  voice v {
    channels = 1; amplitude = &amp; output = &line:out;
    node amp { type = "synth.adsr/1"; }
    node tone { type = "synth.sine/1"; }
    node line { type = "synth.delay/1"; config = { channels = 1; max = 1s; }; }
    connect tone_line { from = &tone:out; to = &line:in; }
  }
}
node keys { instrument = &held; }
connect keys_mix { from = &keys:out; to = &mix:in; }"#;
    assert!(codes(&song(instrument, 1)).contains(&DiagnosticCode::Capability));
}
