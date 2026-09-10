use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::dsp::{DspEngine, RenderError};
use maac::PlanArtifact;

fn fader_source(level: Option<&str>, extras: &str) -> String {
    let level = level
        .map(|value| format!("params = {{ level = {value}; }};"))
        .unwrap_or_default();
    format!(
        r#"maac 1;
project fader_project {{ score = [0q, 1/12000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &fader:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = 1; }}; }}
node fader {{ type = "core.fader/1"; config = {{ channels = 1; }}; {level} }}
connect input {{ from = &sine:out; to = &fader:in; }}
pattern phrase {{ length = 1/12000q; note n {{ at = 0q; dur = 1/12000q; pitch = 12000Hz; velocity = 1; }} }}
track notes {{ target = &sine:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
{extras}
"#,
        level = level,
        extras = extras,
    )
}

fn stereo_fader_source(level: Option<&str>) -> String {
    let level = level
        .map(|value| format!("params = {{ level = {value}; }};"))
        .unwrap_or_default();
    format!(
        r#"maac 1;
project fader_project {{ score = [0q, 1/12000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &fader:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = 1; }}; }}
node pan {{ type = "core.pan/1"; params = {{ pan = -1/2; }}; }}
node fader {{ type = "core.fader/1"; config = {{ channels = 2; }}; {level} }}
connect sine_pan {{ from = &sine:out; to = &pan:in; }}
connect input {{ from = &pan:out; to = &fader:in; }}
pattern phrase {{ length = 1/12000q; note n {{ at = 0q; dur = 1/12000q; pitch = 12000Hz; velocity = 1; }} }}
track notes {{ target = &sine:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
"#,
        level = level,
    )
}

fn fader_source_with_sine_level(
    fader_level: Option<&str>,
    sine_level: &str,
    extras: &str,
) -> String {
    fader_source(fader_level, extras)
        .replace("level = 1; };", &format!("level = {sine_level}; }};"))
}

fn artifact(source: String) -> PlanArtifact {
    let compiled = compile_bundle_artifact(&SourceBundle::new("core-fader.maac", source))
        .expect("core fader source should compile");
    PlanArtifact::from_json(&compiled.to_json().unwrap()).unwrap()
}

fn render(artifact: &PlanArtifact) -> Result<Vec<Vec<f64>>, RenderError> {
    let mut engine = DspEngine::new_artifact(artifact)?;
    let mut samples = Vec::new();
    engine.render(|frame| {
        samples.push(frame.to_vec());
        Ok(())
    })?;
    Ok(samples)
}

fn render_engine(engine: &mut DspEngine<'_>) -> Result<Vec<Vec<f64>>, RenderError> {
    let mut samples = Vec::new();
    engine.render(|frame| {
        samples.push(frame.to_vec());
        Ok(())
    })?;
    Ok(samples)
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1.0e-12,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn fader_defaults_to_zero_db_and_applies_signed_twenty_db_in_mono_and_stereo() {
    for (channels, source) in [(1, fader_source(None, "")), (2, stereo_fader_source(None))] {
        let default = render(&artifact(source)).unwrap();
        let explicit = if channels == 1 {
            render(&artifact(fader_source(Some("0dB"), ""))).unwrap()
        } else {
            render(&artifact(stereo_fader_source(Some("0dB")))).unwrap()
        };
        assert_eq!(default, explicit);
        assert!(default.len() > 1);
        if channels == 1 {
            assert_close(default[1][0], 1.0);
        } else {
            assert_close(default[1][0], (std::f64::consts::PI / 8.0).cos());
            assert_close(default[1][1], (std::f64::consts::PI / 8.0).sin());
        }

        for (level, multiplier) in [("20dB", 10.0), ("-20dB", 0.1)] {
            let scaled = if channels == 1 {
                render(&artifact(fader_source(Some(level), ""))).unwrap()
            } else {
                render(&artifact(stereo_fader_source(Some(level)))).unwrap()
            };
            assert_eq!(scaled.len(), default.len());
            for (scaled_frame, default_frame) in scaled.iter().zip(&default) {
                assert_eq!(scaled_frame.len(), channels);
                for (actual, input) in scaled_frame.iter().zip(default_frame) {
                    assert_close(*actual, multiplier * input);
                }
            }
        }
    }
}

#[test]
fn fader_db_automation_and_modulation_are_immediate_and_replay_after_retention() {
    for shape in ["step", "linear"] {
        let extras = format!(
            "node source {{ type = \"core.constant/1\"; params = {{ value = 1; }}; }}\ncurve level_lane {{ clock = score; points = [(0q, 0dB, {shape}), (1/24000q, 12dB, step)]; }}\nautomation level {{ target = &fader.params.level; curve = &level_lane; at = 0q; }}\nmodulate add_db {{ from = &source:out; target = &fader.params.level; amount = 6dB; }}"
        );
        let dynamic = artifact(fader_source(Some("0dB"), &extras));
        let reference = render(&artifact(fader_source(Some("0dB"), ""))).unwrap();
        let mut engine = DspEngine::new_artifact(&dynamic).unwrap();
        let first = render_engine(&mut engine).unwrap();
        engine.reset();
        let replay = render_engine(&mut engine).unwrap();
        assert_eq!(first, replay);
        let retained = PlanArtifact::from_json(&dynamic.to_json().unwrap()).unwrap();
        assert_eq!(first, render(&retained).unwrap());

        for (frame, (actual, input)) in first.iter().zip(&reference).enumerate() {
            let automation_db = if shape == "linear" && frame == 1 {
                6.0
            } else if frame >= 2 {
                12.0
            } else {
                0.0
            };
            let multiplier = 10.0_f64.powf((automation_db + 6.0) / 20.0);
            assert_eq!(actual.len(), 1);
            assert_close(actual[0], input[0] * multiplier);
        }
    }
}

#[test]
fn finite_huge_fader_db_fails_as_nonfinite_instead_of_silencing_zero_input() {
    let huge = format!("1{}dB", "0".repeat(308));
    let huge_artifact = artifact(fader_source(Some(&huge), ""));
    let mut engine = DspEngine::new_artifact(&huge_artifact).unwrap();
    let mut callbacks = 0;
    let error = engine
        .render(|_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code(), "E_NONFINITE");
    assert_eq!(callbacks, 0);

    let sine_level = format!("1{}", "0".repeat(100));
    let overflow_artifact = artifact(fader_source_with_sine_level(
        Some("6000dB"),
        &sine_level,
        "",
    ));
    let mut engine = DspEngine::new_artifact(&overflow_artifact).unwrap();
    let mut callbacks = 0;
    let error = engine
        .render(|_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code(), "E_NONFINITE");
    assert_eq!(callbacks, 1);
}
