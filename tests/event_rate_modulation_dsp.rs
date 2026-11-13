use maac::dsp::{DspEngine, RenderError};
use maac::PlanArtifact;
use serde_json::Value;

#[path = "support/frames.rs"]
mod frames;
use frames::FrameProject;

/// A `core.constant/1` named `id`.
fn constant(id: &str, value: &str) -> String {
    format!("node {id} {{ type = \"core.constant/1\"; params = {{ value = {value}; }}; }}")
}

fn modulation(id: &str, from: &str, target: &str, parameter: &str, amount: &str) -> String {
    format!(
        "modulate {id} {{ from = &{from}:out; target = &{target}.params.{parameter}; amount = {amount}; }}"
    )
}

/// An automation `id` of `target` from the score start, stepping to each
/// value at its frame.
fn score_automation(id: &str, target: &str, points: &[(u64, &str)]) -> String {
    let points: Vec<String> = points
        .iter()
        .map(|(frame, value)| format!("({frame}/24000q, {value}, step)"))
        .collect();
    format!(
        "curve {id}_curve {{ clock = score; points = [{}]; }}\n\
         automation {id} {{ target = &{target}; curve = &{id}_curve; at = 0q; }}",
        points.join(", ")
    )
}

/// A `core.sine/1` with no attack or release, at full level.
fn core_fixture(total_frames: u64, voices: u32) -> FrameProject {
    FrameProject::sine(
        total_frames,
        0,
        voices,
        "attack = 0s; release = 0s; level = 1;",
    )
}

fn render(plan: &PlanArtifact) -> Result<Vec<f64>, RenderError> {
    let mut engine = DspEngine::new_artifact(plan)?;
    let mut samples = Vec::new();
    engine.render(|frame| {
        samples.push(frame[0]);
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
fn core_sine_attack_modulation_is_captured_at_note_on() {
    let plan = core_fixture(5, 2)
        .with(&constant("attack_source", "1"))
        .with(&score_automation(
            "attack_lane",
            "attack_source.params.value",
            &[(0, "1"), (2, "0")],
        ))
        .with(&modulation(
            "attack_mod",
            "attack_source",
            "sine",
            "attack",
            "1/12000s",
        ))
        .note("first", 0, 5, "12000Hz", "")
        .note("second", 2, 5, "12000Hz", "")
        .artifact();

    let samples = render(&plan).unwrap();
    assert_eq!(samples.len(), 5);
    // The first note captures a four-frame attack. The source changes to zero
    // at frame two, so the second note starts with an immediate attack while
    // the first note keeps its original slope.
    assert_close(samples[0], 0.0);
    assert_close(samples[1], 0.25);
    assert_close(samples[2], 0.0);
    // Each core voice has an independent oscillator phase, so the second
    // voice contributes +1 at frame three while the first contributes -0.75.
    assert_close(samples[3], 0.25);
    assert_close(samples[4], 0.0);

    let mut engine = DspEngine::new_artifact(&plan).unwrap();
    let mut first = Vec::new();
    engine
        .render(|frame| {
            first.push(frame[0]);
            Ok(())
        })
        .unwrap();
    let mut second = Vec::new();
    engine
        .render(|frame| {
            second.push(frame[0]);
            Ok(())
        })
        .unwrap();
    assert_eq!(first, second);
}

#[test]
fn core_sine_release_modulation_is_captured_at_note_off() {
    let plan = core_fixture(6, 1)
        .with(&constant("release_source", "1"))
        .with(&score_automation(
            "release_lane",
            "release_source.params.value",
            &[(0, "1"), (2, "2"), (3, "0")],
        ))
        .with(&modulation(
            "release_mod",
            "release_source",
            "sine",
            "release",
            "1/48000s",
        ))
        .note("released", 0, 2, "12000Hz", "")
        .artifact();

    let samples = render(&plan).unwrap();
    assert_eq!(samples.len(), 6);
    // The source is one frame at onset, two frames at note-off, and zero
    // afterward. Release must retain the two-frame value captured at off.
    assert_close(samples[0], 0.0);
    assert_close(samples[1], 1.0);
    assert_close(samples[2], 0.0);
    assert_close(samples[3], -0.5);
    assert_close(samples[4], 0.0);
    assert_close(samples[5], 0.0);
}

#[test]
fn event_rate_baseline_and_signed_modulations_are_id_ordered() {
    // The sorted reduction is baseline + huge + (-huge) + two frames. If the
    // declaration order were used, the final small contribution would be
    // swallowed by the large intermediate value and the attack would be zero.
    let plan = core_fixture(4, 1)
        .with(&constant("source", "1"))
        .with(&score_automation(
            "attack_baseline",
            "sine.params.attack",
            &[(0, "1/48000s")],
        ))
        .with(&modulation(
            "c_small", "source", "sine", "attack", "1/24000s",
        ))
        .with(&modulation(
            "b_negative",
            "source",
            "sine",
            "attack",
            "-1099511627776s",
        ))
        .with(&modulation(
            "a_positive",
            "source",
            "sine",
            "attack",
            "1099511627776s",
        ))
        .note("n", 0, 4, "12000Hz", "")
        .artifact();

    let expected = render(&plan).unwrap();
    assert_close(expected[1], 0.5);

    // The compiler lists modulations by ID. A plan may list them in any
    // order, so list them in reverse.
    let mut json: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    let modulations = json["modulations"].as_array_mut().unwrap();
    assert_eq!(modulations.len(), 3);
    modulations.reverse();
    let reversed = PlanArtifact::from_json(&serde_json::to_vec(&json).unwrap()).unwrap();
    assert_eq!(render(&reversed).unwrap(), expected);
}

#[test]
fn invalid_event_rate_values_fail_during_render_and_before_callbacks() {
    let range = core_fixture(4, 1)
        .with(&constant("source", "1"))
        .with(&score_automation(
            "bad_lane",
            "source.params.value",
            &[(0, "0"), (2, "-1")],
        ))
        .with(&modulation(
            "bad_attack",
            "source",
            "sine",
            "attack",
            "1/48000s",
        ))
        .note("n", 0, 4, "12000Hz", "")
        .artifact();
    let mut engine = DspEngine::new_artifact(&range).unwrap();
    let mut callbacks = 0;
    let error = engine
        .render(|_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code(), "E_RANGE");
    assert_eq!(callbacks, 2);

    let huge = format!("1{}", "0".repeat(308));
    let nonfinite = core_fixture(1, 1)
        .with(&constant("source", &huge))
        .with(&modulation("overflow", "source", "sine", "attack", "2s"))
        .artifact();
    let mut engine = DspEngine::new_artifact(&nonfinite).unwrap();
    let mut callbacks = 0;
    let error = engine
        .render(|_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code(), "E_NONFINITE");
    assert_eq!(callbacks, 0);
}

/// A node `instrument` whose voice is a sine `osc` under an envelope `amp`
/// that opens and closes at once, with a control for the attack, the release
/// and the phase.
fn instrument_fixture(total_frames: u64, voices: u32) -> FrameProject {
    FrameProject::instrument(
        total_frames,
        voices,
        "program",
        r#"instrument program {
 channels = 1;
 voice voice {
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp { type = "synth.adsr/1"; params = { attack = 0s; decay = 0s; sustain = 1; release = 0s; }; }
  node osc { type = "synth.sine/1"; params = { phase = 0; }; }
 }
 control attack { target = &voice.amp.params.attack; default = 0s; }
 control release { target = &voice.amp.params.release; default = 0s; }
 control phase { target = &voice.osc.params.phase; default = 0; }
}"#,
    )
}

#[test]
fn instrument_public_release_modulation_is_captured_at_note_off() {
    let plan = instrument_fixture(6, 1)
        .with(&constant("release_source", "0"))
        .with(&score_automation(
            "release_lane",
            "release_source.params.value",
            &[(0, "0"), (2, "1"), (3, "0")],
        ))
        .with(&modulation(
            "release",
            "release_source",
            "instrument",
            "release",
            "1/24000s",
        ))
        .note("released", 0, 2, "12000Hz", "")
        .artifact();

    let samples = render(&plan).unwrap();
    assert_eq!(samples.len(), 6);
    assert_close(samples[0], 0.0);
    assert_close(samples[1], 1.0);
    assert_close(samples[2], 0.0);
    assert_close(samples[3], -0.5);
    assert_close(samples[4], 0.0);
    assert_close(samples[5], 0.0);
}

#[test]
fn instrument_public_event_rates_capture_per_voice_and_release_controls_capacity() {
    let plan = instrument_fixture(5, 2)
        .with(&constant("attack_source", "1"))
        .with(&constant("phase_source", "0"))
        .with(&score_automation(
            "attack_lane",
            "attack_source.params.value",
            &[(0, "1"), (2, "0")],
        ))
        .with(&score_automation(
            "phase_lane",
            "phase_source.params.value",
            &[(0, "0"), (2, "1/4")],
        ))
        .with(&modulation(
            "attack",
            "attack_source",
            "instrument",
            "attack",
            "1/12000s",
        ))
        .with(&modulation(
            "phase",
            "phase_source",
            "instrument",
            "phase",
            "1",
        ))
        .note("first", 0, 5, "12000Hz", "")
        .note("second", 2, 5, "12000Hz", "")
        .artifact();
    let samples = render(&plan).unwrap();
    assert_close(samples[0], 0.0);
    assert_close(samples[1], 0.25);
    assert_close(samples[2], 1.0);
    assert_close(samples[3], -0.75);
    assert_close(samples[4], -1.0);

    // Two notes back to back on one voice: a release that the source turns
    // on at the first note's end keeps its voice, and one it leaves at zero
    // frees it.
    let capacity = |release_at_off: &str| {
        instrument_fixture(4, 1)
            .with(&constant("release_source", "0"))
            .with(&score_automation(
                "release_lane",
                "release_source.params.value",
                &[(0, "0"), (2, release_at_off)],
            ))
            .with(&modulation(
                "release",
                "release_source",
                "instrument",
                "release",
                "1s",
            ))
            .note("old", 0, 2, "12000Hz", "")
            .note("new", 2, 4, "12000Hz", "")
            .artifact()
    };
    let plan = capacity("1/48000");
    let mut engine = DspEngine::new_artifact(&plan).unwrap();
    let mut callbacks = 0;
    let error = engine
        .render(|_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code(), "E_VOICE_LIMIT");
    assert_eq!(callbacks, 2);

    assert_eq!(render(&capacity("0")).unwrap().len(), 4);
}
