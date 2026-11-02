//! Per-note values inside an instrument: `synth.random/1`, a constant per
//! note drawn from its address, and `synth.noise/1` with `per_note`, which
//! starts each note's noise from its address.

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::{render_ports_artifact_span_with_limits, RenderSpan};
use maac::plan::{PlanLimits, PortRef};
use maac::{render_artifact_with_limits, DiagnosticCode, PlanArtifact, SourceBundle};

/// Four quarter-second notes a beat apart on an instrument whose voice holds
/// `nodes`, each connected to the output mix.
fn source(nodes: &str, outputs: &[&str]) -> String {
    let connections: String = outputs
        .iter()
        .map(|node| format!("    connect {node}_out {{ from = &{node}:out; to = &out:in; }}\n"))
        .collect();
    format!(
        r#"maac 1;
project p {{ score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument probe {{
  channels = 1;
  voice voice {{
    channels = 1; amplitude = &amp; output = &out:out;
    node amp {{ type = "synth.adsr/1"; params = {{ sustain = 1; }}; }}
    node out {{ type = "synth.mix/1"; config = {{ channels = 1; }}; }}
{nodes}
{connections}  }}
}}
node synth {{ instrument = &probe; config = {{ voices = 2; }}; }}
pattern beat {{ length = 1q; note n {{ at = 0q; dur = 1/2q; pitch = A4; }} }}
track t {{ target = &synth:events; }}
place main {{ pattern = &beat; track = &t; at = 0q; count = 4; }}
"#
    )
}

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

/// The first 10 ms of each of the four notes, a beat (24000 frames) apart.
fn onsets(samples: &[f64]) -> Vec<&[f64]> {
    (0..4)
        .map(|note| &samples[note * 24_000..note * 24_000 + 480])
        .collect()
}

const NOISE: &str = r#"    node air { type = "synth.noise/1"; config = { seed = 3; per_note = true; }; params = { level = 1/2; }; }"#;

/// A random value as a constant: it scales a DC offset through the mix.
const RANDOM: &str = r#"    node draw { type = "synth.random/1"; config = { seed = 9; }; }"#;

#[test]
fn per_note_noise_starts_each_note_afresh() {
    let samples = render(&source(NOISE, &["air"]));
    let notes = onsets(&samples);
    for later in &notes[1..] {
        assert_ne!(notes[0], *later);
    }
    // Without `per_note` every note plays the same burst.
    let samples = render(&source(&NOISE.replace(" per_note = true;", ""), &["air"]));
    let notes = onsets(&samples);
    for later in &notes[1..] {
        assert_eq!(notes[0], *later);
    }
}

#[test]
fn a_random_value_is_constant_through_a_note_and_differs_between_notes() {
    let samples = render(&source(RANDOM, &["draw"]));
    let values: Vec<f64> = onsets(&samples).iter().map(|note| note[240]).collect();
    for (note, value) in onsets(&samples).iter().zip(&values) {
        assert!(note[1..].iter().all(|sample| sample == value));
        assert!((-1.0..1.0).contains(value));
    }
    for later in &values[1..] {
        assert_ne!(values[0], *later);
    }
    // The same source draws the same values.
    assert_eq!(samples, render(&source(RANDOM, &["draw"])));
}

#[test]
fn a_render_range_hears_the_values_of_the_whole() {
    let source = source(&format!("{NOISE}\n{RANDOM}"), &["air", "draw"]);
    let whole = render(&source);
    let plan = compile(&source).unwrap();
    let mut part = Vec::new();
    render_ports_artifact_span_with_limits(
        &plan,
        &PlanLimits::song(),
        &[PortRef {
            node: "synth".into(),
            port: "out".into(),
        }],
        RenderSpan {
            start: 48_000,
            end: 72_000,
        },
        |frame| {
            part.extend_from_slice(&frame[0]);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(part.as_slice(), &whole[48_000..72_000]);
}

#[test]
fn plans_of_plain_noise_are_unchanged() {
    let plan = compile(&source(&NOISE.replace(" per_note = true;", ""), &["air"])).unwrap();
    let json = String::from_utf8(plan.to_json().unwrap()).unwrap();
    assert!(json.contains("synth.noise/1"));
    assert!(!json.contains("per_note"));
}

#[test]
fn malformed_per_note_sources_are_rejected() {
    let codes = |nodes: &str, output: &str| -> Vec<DiagnosticCode> {
        compile(&source(nodes, &[output]))
            .unwrap_err()
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    };
    assert!(
        codes(&NOISE.replace("per_note = true", "per_note = 1"), "air")
            .contains(&DiagnosticCode::Unit)
    );
    assert!(
        codes(&NOISE.replace("seed = 3;", "seed = 3; colour = 1;"), "air")
            .contains(&DiagnosticCode::UnknownField)
    );
    assert!(codes(&RANDOM.replace("seed = 9", "seed = 0"), "draw").contains(&DiagnosticCode::Range));
    assert!(codes(&RANDOM.replace("seed = 9", "depth = 9"), "draw")
        .contains(&DiagnosticCode::UnknownField));
}
