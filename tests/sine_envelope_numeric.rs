use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::dsp::{DspEngine, RenderError};
use maac::PlanArtifact;

const RATE: u64 = 48_000;

fn envelope_source(
    score_frames: u64,
    note_frames: u64,
    tail_frames: u64,
    attack: &str,
    release: &str,
    level: &str,
) -> String {
    format!(
        r#"maac 1;
project envelope {{ score = [0q, {score_frames}/{RATE}q]; tail = {tail_frames}/{RATE}s; rate = {RATE}Hz; tempo = &clock; meter = &metre; output = &sine:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; config = {{ voices = 1; }}; params = {{ attack = {attack}s; release = {release}s; level = {level}; }}; }}
pattern phrase {{ length = {score_frames}/{RATE}q; note n {{ at = 0q; dur = {note_frames}/{RATE}q; pitch = 12000Hz; velocity = 1; }} }}
track notes {{ target = &sine:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
"#,
        RATE = RATE,
        score_frames = score_frames,
        note_frames = note_frames,
        tail_frames = tail_frames,
        attack = attack,
        release = release,
        level = level,
    )
}

fn artifact(source: String) -> PlanArtifact {
    let compiled = compile_bundle_artifact(&SourceBundle::new("sine-envelope.maac", source))
        .expect("sine envelope source should compile");
    PlanArtifact::from_json(&compiled.to_json().unwrap()).unwrap()
}

fn render(artifact: &PlanArtifact) -> Result<Vec<f64>, RenderError> {
    let mut engine = DspEngine::new_artifact(artifact)?;
    let mut samples = Vec::new();
    engine.render(|frame| {
        samples.push(frame[0]);
        Ok(())
    })?;
    Ok(samples)
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1.0e-12,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn ordinary_attack_keeps_existing_sample_formula_bitwise() {
    for (duration, seconds) in [
        ("1/3", 1.0 / 3.0),
        ("1/7", 1.0 / 7.0),
        ("1/13", 1.0 / 13.0),
        ("1/1000", 1.0 / 1000.0),
    ] {
        let plan = artifact(envelope_source(2, 2, 0, duration, "0", "1"));
        let samples = render(&plan).unwrap();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].to_bits(), 0.0_f64.to_bits());
        let expected = 1.0 / (seconds * RATE as f64);
        assert_eq!(
            samples[1].to_bits(),
            expected.to_bits(),
            "attack duration {duration} changed the finite-product result"
        );
    }
}

#[test]
fn overflowing_attack_product_preserves_finite_progress_and_replays() {
    let huge = format!("1{}", "0".repeat(308));
    let plan = artifact(envelope_source(2, 2, 0, &huge, "0", &huge));
    let first = render(&plan).unwrap();
    assert_eq!(first.len(), 2);
    assert_close(first[0], 0.0);
    assert_close(first[1], 1.0 / RATE as f64);

    let captured = artifact(envelope_source(2, 2, 3, &huge, "2/48000", &huge));
    let captured_samples = render(&captured).unwrap();
    assert_eq!(captured_samples.len(), 5);
    assert_close(captured_samples[3], -1.0 / RATE as f64);
    assert_close(captured_samples[4], 0.0);

    let retained = PlanArtifact::from_json(&plan.to_json().unwrap()).unwrap();
    assert_eq!(render(&retained).unwrap(), first);

    let mut engine = DspEngine::new_artifact(&retained).unwrap();
    let mut replay = Vec::new();
    engine
        .render(|frame| {
            replay.push(frame[0]);
            Ok(())
        })
        .unwrap();
    assert_eq!(replay, first);
}

#[test]
fn huge_release_capture_and_tail_remain_finite_after_reset() {
    let huge = format!("1{}", "0".repeat(308));
    let plan = artifact(envelope_source(1, 1, 3, "0", &huge, &huge));
    let first = render(&plan).unwrap();
    assert_eq!(first.len(), 4);
    assert!(first.iter().all(|sample| sample.is_finite()));
    assert_close(first[0], 0.0);
    assert_close(first[1], 1.0e308);
    assert_close(first[3], -1.0e308);

    let mut engine = DspEngine::new_artifact(&plan).unwrap();
    let mut replay = Vec::new();
    engine
        .render(|frame| {
            replay.push(frame[0]);
            Ok(())
        })
        .unwrap();
    engine.reset();
    let mut reset = Vec::new();
    engine
        .render(|frame| {
            reset.push(frame[0]);
            Ok(())
        })
        .unwrap();
    assert_eq!(replay, reset);
}
