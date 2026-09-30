//! §14.3 `warp_preserve` through the core `core.stretch.ola/1` reference.

use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::compile_bundle_artifact;
use maac::{DiagnosticCode, PlanArtifact};
use serde_json::Value;

const PERIOD: usize = 96; // 500 Hz at 48 kHz
const FRAMES: usize = 48_000;

/// One exactly periodic second of a 500 Hz sine at the output rate.
fn sine_bytes() -> Vec<u8> {
    let cycle: Vec<f32> = (0..PERIOD)
        .map(|j| (std::f64::consts::TAU * j as f64 / PERIOD as f64).sin() as f32)
        .collect();
    (0..FRAMES)
        .flat_map(|j| cycle[j % PERIOD].to_le_bytes())
        .collect()
}

fn bundle(clip: &str) -> SourceBundle {
    let bytes = sine_bytes();
    let source = format!(
        r#"maac 1;
project p {{ score = [0q, 3q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &clip:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
asset tone {{ kind = audio; path = "tone.pcm"; hash = "{}"; format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = 1; frames = {FRAMES}; }}
{clip}
"#,
        sha256_digest(&bytes)
    );
    let mut bundle = SourceBundle::new("score.maac", source);
    bundle.assets.insert("tone.pcm".into(), bytes);
    bundle
}

fn clip(mode: &str, length_q: &str) -> String {
    format!(
        "audio clip {{ asset = &tone; at = 0q; source = [0frame, {FRAMES}frame]; {mode} warp = [(0q, 0frame), ({length_q}, {FRAMES}frame)]; }}"
    )
}

const PRESERVE: &str = "mode = warp_preserve; processor = \"core.stretch.ola/1\";";
const RATE: &str = "mode = warp_rate;";

fn render(artifact: &PlanArtifact) -> Vec<f64> {
    let mut output = Vec::new();
    maac::render_artifact(artifact, |frame| {
        output.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    output
}

fn compiled(clip: &str) -> PlanArtifact {
    compile_bundle_artifact(&bundle(clip)).unwrap()
}

fn codes(clip: &str) -> Vec<DiagnosticCode> {
    compile_bundle_artifact(&bundle(clip))
        .expect_err("clip must be refused")
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn plan_records_the_core_stretch_identity_only_for_preserve() {
    let preserve = compiled(&clip(PRESERVE, "2q"));
    let wire: Value = serde_json::from_slice(&preserve.to_json().unwrap()).unwrap();
    let node = &wire["nodes"][0]["processor"];
    assert_eq!(node["kind"], "warp_rate");
    assert_eq!(node["clip"]["stretch"], "core.stretch.ola/1");
    assert_eq!(node["clip"]["end_frame"], 96_000);

    let rate = compiled(&clip(RATE, "2q"));
    let wire: Value = serde_json::from_slice(&rate.to_json().unwrap()).unwrap();
    assert!(wire["nodes"][0]["processor"]["clip"]
        .get("stretch")
        .is_none());
}

#[test]
fn identity_warp_matches_rate_warp() {
    let preserve = render(&compiled(&clip(PRESERVE, "1q")));
    let rate = render(&compiled(&clip(RATE, "1q")));
    assert_eq!(preserve.len(), rate.len());
    for (n, (a, b)) in preserve.iter().zip(&rate).enumerate() {
        assert!((a - b).abs() < 1e-9, "frame {n}: {a} vs {b}");
    }
}

#[test]
fn stretching_keeps_pitch_while_rate_warp_lowers_it() {
    let preserve = render(&compiled(&clip(PRESERVE, "2q")));
    let rate = render(&compiled(&clip(RATE, "2q")));
    // Both transports last two seconds; the source lasted one.
    assert!(preserve[96_000..].iter().all(|sample| *sample == 0.));
    assert!(preserve[1_000..95_000]
        .iter()
        .any(|sample| sample.abs() > 0.9));

    // Adjacent grains start five whole source periods apart, so the core
    // reference reconstructs the original 96-frame period exactly.
    for n in 2_000..94_000 {
        assert!(
            (preserve[n + PERIOD] - preserve[n]).abs() < 1e-6,
            "preserve frame {n}"
        );
    }
    // Rate warping halves the frequency: half a new period is a sign flip.
    for n in 2_000..94_000 {
        assert!((rate[n + PERIOD] + rate[n]).abs() < 1e-3, "rate frame {n}");
    }
}

#[test]
fn compression_also_preserves_pitch_and_duration() {
    let preserve = render(&compiled(&clip(PRESERVE, "1/2q")));
    assert!(preserve[24_000..].iter().all(|sample| *sample == 0.));
    assert!(preserve[..24_000].iter().any(|sample| sample.abs() > 0.9));
    // At 2x speed adjacent grains start 15 periods apart (1440 frames).
    for n in 1_000..22_000 {
        assert!((preserve[n + PERIOD] - preserve[n]).abs() < 1e-6);
    }
}

#[test]
fn unsupported_stretches_are_refused_never_rate_warped() {
    for processor in [
        "",
        "processor = \"vendor.stretch/1\";",
        "processor = &tone;",
    ] {
        let clip = clip(&format!("mode = warp_preserve; {processor}"), "2q");
        let codes = codes(&clip);
        let expected = if processor.is_empty() {
            DiagnosticCode::Range
        } else {
            DiagnosticCode::Capability
        };
        assert!(codes.contains(&expected), "{processor:?}: {codes:?}");
    }
    // Rate warping still forbids a processor, and warp_preserve forbids speed.
    assert!(codes(&clip(
        "mode = warp_rate; processor = \"core.stretch.ola/1\";",
        "2q"
    ))
    .contains(&DiagnosticCode::Range));
    assert!(codes(&clip(&format!("{PRESERVE} speed = 1;"), "2q")).contains(&DiagnosticCode::Range));
}

#[test]
fn a_plan_naming_an_unknown_stretch_does_not_load() {
    let preserve = compiled(&clip(PRESERVE, "2q"));
    let mut wire: Value = serde_json::from_slice(&preserve.to_json().unwrap()).unwrap();
    wire["nodes"][0]["processor"]["clip"]["stretch"] = "vendor.stretch/1".into();
    let error = PlanArtifact::from_json(&serde_json::to_vec(&wire).unwrap()).unwrap_err();
    assert_eq!(error.code, "E_CAPABILITY", "{error:?}");
}
