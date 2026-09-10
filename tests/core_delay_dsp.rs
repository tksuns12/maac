use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::dsp::{DspEngine, RenderError};
use maac::PlanArtifact;

fn shifted_source(channels: u8, frames: u8, stereo_input: bool) -> String {
    let (pan, upstream, connection) = if stereo_input {
        (
            r#"node pan { type = "core.pan/1"; params = { pan = -1/2; }; }
connect sine_pan { from = &sine:out; to = &pan:in; }"#,
            "pan",
            "delay_input",
        )
    } else {
        ("", "sine", "delay_input")
    };
    format!(
        r#"maac 1;
project delay_project {{ score = [0q, 1/12000q]; tail = {frames}/48000s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &delay:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = 1; }}; }}
{pan}
node delay {{ type = "core.delay/1"; config = {{ channels = {channels}; frames = {frames}; }}; }}
connect {connection} {{ from = &{upstream}:out; to = &delay:in; }}
pattern phrase {{ length = 1/12000q; note n {{ at = 0q; dur = 1/12000q; pitch = 12000Hz; velocity = 1; }} }}
track notes {{ target = &sine:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
"#,
        channels = channels,
        frames = frames,
        pan = pan,
        upstream = upstream,
        connection = connection,
    )
}

fn feedback_source(sine_level: &str, gain: &str) -> String {
    format!(
        r#"maac 1;
project feedback_project {{ score = [0q, 1/12000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sum:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = {sine_level}; }}; }}
node sum {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
node delay {{ type = "core.delay/1"; config = {{ channels = 1; frames = 1; }}; }}
node gain {{ type = "core.gain/1"; config = {{ channels = 1; }}; params = {{ gain = {gain}; }}; }}
connect z_source {{ from = &sine:out; to = &sum:in; }}
connect a_delayed {{ from = &delay:out; to = &gain:in; }}
connect b_gain {{ from = &gain:out; to = &sum:in; }}
connect c_sum {{ from = &sum:out; to = &delay:in; }}
pattern phrase {{ length = 1/12000q; note n {{ at = 0q; dur = 1/12000q; pitch = 12000Hz; velocity = 1; }} }}
track notes {{ target = &sine:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
"#,
        sine_level = sine_level,
        gain = gain,
    )
}

fn two_delay_cycle_source() -> String {
    r#"maac 1;
project cycle_project { score = [0q, 1/12000q]; tail = 2/48000s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &z_delay:out; }
tempo clock { points = [(0q, 60bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node sine { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 1; }; }
node sum { type = "core.sum/1"; config = { channels = 1; }; }
node a_delay { type = "core.delay/1"; config = { channels = 1; frames = 1; }; }
node z_delay { type = "core.delay/1"; config = { channels = 1; frames = 1; }; }
node feedback_gain { type = "core.gain/1"; config = { channels = 1; }; params = { gain = 1/2; }; }
connect z_feedback { from = &z_delay:out; to = &feedback_gain:in; }
connect x_source { from = &sine:out; to = &sum:in; }
connect feedback_to_sum { from = &feedback_gain:out; to = &sum:in; }
connect sum_to_a { from = &sum:out; to = &a_delay:in; }
connect a_to_z { from = &a_delay:out; to = &z_delay:in; }
pattern phrase { length = 1/12000q; note n { at = 0q; dur = 1/12000q; pitch = 12000Hz; velocity = 1; } }
track notes { target = &sine:events; }
place play { pattern = &phrase; track = &notes; at = 0q; }
"#
    .to_owned()
}

fn self_delay_source() -> String {
    r#"maac 1;
project self_project { score = [0q, 1/12000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &self_delay:out; }
tempo clock { points = [(0q, 60bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node self_delay { type = "core.delay/1"; config = { channels = 1; frames = 1; }; }
connect self_loop { from = &self_delay:out; to = &self_delay:in; }
"#
    .to_owned()
}

fn artifact(source: String) -> PlanArtifact {
    let compiled = compile_bundle_artifact(&SourceBundle::new("core-delay.maac", source))
        .expect("core delay source should compile");
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
fn delay_shifts_mono_and_stereo_by_exact_frames_with_initial_zeros_and_tail() {
    let mono = render(&artifact(shifted_source(1, 1, false))).unwrap();
    assert_eq!(mono.len(), 5);
    assert_eq!(mono[0], vec![0.0]);
    assert_eq!(mono[1], vec![0.0]);
    assert_close(mono[2][0], 1.0);
    assert_close(mono[3][0], 0.0);
    assert_close(mono[4][0], -1.0);

    let stereo = render(&artifact(shifted_source(2, 2, true))).unwrap();
    assert_eq!(stereo.len(), 6);
    let left = (std::f64::consts::PI / 8.0).cos();
    let right = (std::f64::consts::PI / 8.0).sin();
    assert_eq!(stereo[0], vec![0.0, 0.0]);
    assert_eq!(stereo[1], vec![0.0, 0.0]);
    assert_eq!(stereo[2], vec![0.0, 0.0]);
    assert_close(stereo[3][0], left);
    assert_close(stereo[3][1], right);
    assert_close(stereo[4][0], 0.0);
    assert_close(stereo[4][1], 0.0);
    assert_close(stereo[5][0], -left);
    assert_close(stereo[5][1], -right);
}

#[test]
fn one_frame_delay_feedback_matches_the_sample_recurrence() {
    let output = render(&artifact(feedback_source("1", "1/2"))).unwrap();
    assert_eq!(output.len(), 4);
    for (actual, expected) in output.iter().zip([0.0, 1.0, 0.5, -0.75]) {
        assert_close(actual[0], expected);
    }
}

#[test]
fn two_delay_cycle_reads_history_before_writes_and_self_delay_starts_zero() {
    let cycle = render(&artifact(two_delay_cycle_source())).unwrap();
    assert_eq!(cycle.len(), 6);
    for (actual, expected) in cycle.iter().zip([0.0, 0.0, 0.0, 1.0, 0.0, -0.5]) {
        assert_close(actual[0], expected);
    }

    let self_delay = render(&artifact(self_delay_source())).unwrap();
    assert_eq!(self_delay.len(), 4);
    assert_eq!(self_delay, vec![vec![0.0]; 4]);
}

#[test]
fn delay_reset_and_retained_artifact_replay_are_bitwise_identical() {
    let original = artifact(feedback_source("1", "1/2"));
    let expected = render(&original).unwrap();
    let retained = PlanArtifact::from_json(&original.to_json().unwrap()).unwrap();
    assert_eq!(render(&retained).unwrap(), expected);

    let mut engine = DspEngine::new_artifact(&retained).unwrap();
    for _ in 0..2 {
        engine.reset();
        assert_eq!(render_engine(&mut engine).unwrap(), expected);
    }
}

#[test]
fn feedback_overflow_is_an_explicit_nonfinite_error_without_limiting() {
    let artifact = artifact(feedback_source(&format!("1{}", "0".repeat(308)), "2"));
    let mut engine = DspEngine::new_artifact(&artifact).unwrap();
    let mut callbacks = 0;
    let error = engine
        .render(|_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code(), "E_NONFINITE");
    assert_eq!(callbacks, 2);
}
