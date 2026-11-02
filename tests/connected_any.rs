//! Connected notes on any instrument whose voice says `connected = true`
//! (§8.3): a joined note continues the voice; without `slur` it restarts the
//! envelopes that restart on a join, and `slide` moves its pitch.

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::plan::PlanLimits;
use maac::{render_artifact_with_limits, DiagnosticCode, PlanArtifact, SourceBundle};
use serde_json::Value;

/// One instrument with the voice fields `fields` and the nodes `nodes`, whose
/// mix `out` takes `outputs`; the pattern `line` holds `notes`.
fn source(fields: &str, nodes: &str, outputs: &[&str], notes: &str, place: &str) -> String {
    let connections: String = outputs
        .iter()
        .map(|node| format!("    connect {node}_out {{ from = &{node}:out; to = &out:in; }}\n"))
        .collect();
    format!(
        r#"maac 1;
project p {{ score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument reed {{
  channels = 1;
  voice voice {{
    channels = 1; amplitude = &amp; output = &out:out; {fields}
    node amp {{ type = "synth.adsr/1"; params = {{ attack = 10ms; sustain = 1; release = 5ms; }}; }}
    node out {{ type = "synth.mix/1"; config = {{ channels = 1; }}; }}
{nodes}
{connections}  }}
}}
node synth {{ instrument = &reed; config = {{ voices = 2; }}; }}
pattern line {{ length = 4q;
{notes}
}}
track t {{ target = &synth:events; }}
{place}
"#
    )
}

const ONCE: &str = "place main { pattern = &line; track = &t; at = 0q; }";

/// Two notes that touch at 1q, half a second in.
const TWO: &str = r#"  note a { at = 0q; dur = 1q; pitch = A4; }
  note b { at = 1q; dur = 1q; pitch = C5; }"#;

/// A "t" of noise: an envelope that restarts on a re-articulated join.
const TONGUE: &str = r#"    node tongue { type = "synth.adsr/1"; config = { on_join = restart; }; params = { decay = 20ms; sustain = 0; }; }
    node hiss { type = "synth.noise/1"; params = { level = 0; }; }
    modulate shape { from = &tongue:out; to = &hiss.params.level; depth = 1; }"#;

/// The note's key, in octaves from C4, as a constant signal.
const KEY: &str = r#"    node key { type = "synth.key/1"; }"#;

const JOIN: usize = 24_000;

fn compile(source: &str) -> Result<PlanArtifact, maac::Diagnostics> {
    compile_bundle_artifact_with_limits(
        &SourceBundle::new("main.maac", source),
        &PlanLimits::song(),
    )
}

fn render(source: &str) -> Vec<f64> {
    let plan = compile(source).unwrap_or_else(|error| panic!("{error:?}"));
    let mut out = Vec::new();
    render_artifact_with_limits(&plan, &PlanLimits::song(), |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    out
}

fn joins(source: &str) -> Vec<Option<String>> {
    let plan = compile(source).unwrap_or_else(|error| panic!("{error:?}"));
    let wire: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    wire["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["join"]["from"].as_str().map(str::to_owned))
        .collect()
}

fn energy(samples: &[f64]) -> f64 {
    samples.iter().map(|sample| sample * sample).sum()
}

fn codes(source: &str) -> Vec<DiagnosticCode> {
    compile(source)
        .unwrap_err()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn a_connected_voice_joins_notes_that_touch() {
    let connected = source("connected = true;", KEY, &["key"], TWO, ONCE);
    assert_eq!(joins(&connected), [None, Some("main/0/a".to_owned())]);
    // Without it, every note starts its own voice, and a slur is refused.
    let plain = source("", KEY, &["key"], TWO, ONCE);
    assert_eq!(joins(&plain), [None, None]);
    let slurred = TWO.replace("pitch = C5;", "pitch = C5; slur = true;");
    assert!(codes(&source("", KEY, &["key"], &slurred, ONCE)).contains(&DiagnosticCode::Capability));
}

#[test]
fn a_tongued_join_restarts_the_tongue_and_a_slurred_one_does_not() {
    let tongued = render(&source("connected = true;", TONGUE, &["hiss"], TWO, ONCE));
    let slurred = render(&source(
        "connected = true;",
        TONGUE,
        &["hiss"],
        &TWO.replace("pitch = C5;", "pitch = C5; slur = true;"),
        ONCE,
    ));
    let burst = |samples: &[f64]| energy(&samples[JOIN..JOIN + 480]);
    // The first note's burst has long decayed; only a restart sounds again.
    assert!(burst(&tongued) > 1.0, "{}", burst(&tongued));
    assert!(burst(&slurred) < 1e-9, "{}", burst(&slurred));
    // Before the join the two are the same.
    assert_eq!(tongued[..JOIN], slurred[..JOIN]);
}

#[test]
fn the_amplitude_holds_through_a_join() {
    // The key signal is constant per note, so the output is the amplitude
    // envelope times the key: it does not fall to zero at the join.
    let connected = render(&source("connected = true;", KEY, &["key"], TWO, ONCE));
    let separate = render(&source("", KEY, &["key"], TWO, ONCE));
    let c5 = (523.251_130_6_f64 / 440.0).log2() + 0.75;
    assert!(
        (connected[JOIN] - c5).abs() < 1e-6,
        "{:?}",
        &connected[JOIN - 2..JOIN + 3]
    );
    // A separate note starts its attack from zero, while the first one's
    // 5 ms release ends: 5 ms in, it is halfway up its 10 ms attack.
    assert!((connected[JOIN + 240] - c5).abs() < 1e-6);
    assert!(
        (separate[JOIN + 240] - c5 / 2.0).abs() < 1e-2,
        "{}",
        separate[JOIN + 240]
    );
}

#[test]
fn a_slide_moves_the_pitch_in_cents() {
    let sliding = TWO.replace("pitch = C5;", "pitch = C5; slide = 100ms;");
    let samples = render(&source("connected = true;", KEY, &["key"], &sliding, ONCE));
    let (a4, c5) = (0.75, (523.251_130_6_f64 / 440.0).log2() + 0.75);
    // Linear in cents: halfway through, halfway between the keys.
    assert!((samples[JOIN] - a4).abs() < 1e-9);
    assert!((samples[JOIN + 2_400] - (a4 + c5) / 2.0).abs() < 1e-6);
    assert!((samples[JOIN + 4_800] - c5).abs() < 1e-9);
}

#[test]
fn a_varied_line_stays_connected() {
    let varied = "vary loose { seed = 3; velocity = 1/20; timing = 10ms; }\n\
        place main { pattern = &line; track = &t; at = 2q; vary = &loose; }";
    let three = format!("{TWO}\n  note c {{ at = 2q; dur = 2q; pitch = E5; }}");
    let joined = joins(&source("connected = true;", KEY, &["key"], &three, varied));
    assert_eq!(
        joined,
        [
            None,
            Some("main/0/a".to_owned()),
            Some("main/0/b".to_owned()),
        ]
    );
}

#[test]
fn malformed_connected_voices_are_rejected() {
    assert!(
        codes(&source("connected = 1;", KEY, &["key"], TWO, ONCE)).contains(&DiagnosticCode::Unit)
    );
    let restart = TONGUE.replace("on_join = restart", "on_join = always");
    assert!(
        codes(&source("connected = true;", &restart, &["hiss"], TWO, ONCE))
            .contains(&DiagnosticCode::Range)
    );
}

#[test]
fn a_random_value_glides_to_the_joined_notes() {
    let draw = r#"    node draw { type = "synth.random/1"; config = { seed = 5; }; }"#;
    let samples = render(&source("connected = true;", draw, &["draw"], TWO, ONCE));
    let (before, after) = (samples[JOIN - 1], samples[JOIN + 4_800]);
    assert_ne!(before, after);
    assert!((samples[JOIN + 2_400] - (before + after) / 2.0).abs() < 1e-9);
    assert_eq!(samples[JOIN + 4_800], samples[JOIN + 9_600]);
}
