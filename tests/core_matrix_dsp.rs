use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::dsp::{DspEngine, RenderError};
use maac::PlanArtifact;

fn matrix_source(
    inputs: u8,
    outputs: u8,
    coefficients: &str,
    stereo_input: bool,
    sine_level: &str,
) -> String {
    let (pan, upstream, connection) = if stereo_input {
        (
            r#"node pan { type = "core.pan/1"; params = { pan = -1/2; }; }
connect sine_pan { from = &sine:out; to = &pan:in; }"#,
            "pan",
            "matrix_input",
        )
    } else {
        ("", "sine", "matrix_input")
    };
    format!(
        r#"maac 1;
project matrix_project {{ score = [0q, 1/12000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &matrix:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = {sine_level}; }}; }}
{pan}
node matrix {{ type = "core.matrix/1"; config = {{ inputs = {inputs}; outputs = {outputs}; coefficients = {coefficients}; }}; }}
connect {connection} {{ from = &{upstream}:out; to = &matrix:in; }}
pattern phrase {{ length = 1/12000q; note n {{ at = 0q; dur = 1/12000q; pitch = 12000Hz; velocity = 1; }} }}
track notes {{ target = &sine:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
"#,
        inputs = inputs,
        outputs = outputs,
        coefficients = coefficients,
        pan = pan,
        upstream = upstream,
        connection = connection,
        sine_level = sine_level,
    )
}

fn artifact(source: String) -> PlanArtifact {
    let compiled = compile_bundle_artifact(&SourceBundle::new("core-matrix.maac", source))
        .expect("core matrix source should compile");
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

fn pan_frame_one() -> (f64, f64) {
    (
        (std::f64::consts::PI / 8.0).cos(),
        (std::f64::consts::PI / 8.0).sin(),
    )
}

#[test]
fn matrix_duplicates_mono_and_preserves_identity_scale_without_normalizing() {
    let duplicate = render(&artifact(matrix_source(1, 2, "[[1], [1]]", false, "1"))).unwrap();
    assert_eq!(duplicate[0], vec![0.0, 0.0]);
    assert_eq!(duplicate[1], vec![1.0, 1.0]);
    assert_eq!(duplicate[3], vec![-1.0, -1.0]);

    let scaled = render(&artifact(matrix_source(1, 1, "[[3/2]]", false, "1"))).unwrap();
    assert_eq!(scaled[1], vec![1.5]);
    assert_eq!(scaled[3], vec![-1.5]);
}

#[test]
fn matrix_uses_asymmetric_rows_for_signed_fractional_stereo_mapping_and_downmix() {
    let (left, right) = pan_frame_one();
    let mapped = render(&artifact(matrix_source(
        2,
        2,
        "[[1/2, -1/4], [-0, 3/2]]",
        true,
        "1",
    )))
    .unwrap();
    assert_close(mapped[1][0], 0.5 * left - 0.25 * right);
    assert_close(mapped[1][1], 1.5 * right);
    assert_close(mapped[3][0], -0.5 * left + 0.25 * right);
    assert_close(mapped[3][1], -1.5 * right);

    let downmix = render(&artifact(matrix_source(2, 1, "[[3/4, -1/2]]", true, "1"))).unwrap();
    assert_close(downmix[1][0], 0.75 * left - 0.5 * right);
}

#[test]
fn matrix_reset_and_retained_artifact_replay_are_bitwise_identical() {
    let original = artifact(matrix_source(2, 2, "[[1/2, -1/4], [1/3, 2/3]]", true, "1"));
    let expected = render(&original).unwrap();
    assert_eq!(expected[0], vec![0.0, 0.0]);
    assert!(expected[1].iter().any(|sample| sample.abs() > 0.1));

    let retained = PlanArtifact::from_json(&original.to_json().unwrap()).unwrap();
    assert_eq!(render(&retained).unwrap(), expected);

    let mut engine = DspEngine::new_artifact(&retained).unwrap();
    for _ in 0..2 {
        engine.reset();
        assert_eq!(render_engine(&mut engine).unwrap(), expected);
    }
}

#[test]
fn matrix_rejects_nonfinite_coefficients_and_reports_product_and_sum_overflow() {
    let nonfinite_coefficient = format!("1{}", "0".repeat(309));
    let diagnostics = compile_bundle_artifact(&SourceBundle::new(
        "core-matrix.maac",
        matrix_source(1, 1, &format!("[[{nonfinite_coefficient}]]"), false, "1"),
    ))
    .unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code_str() == "E_NONFINITE"),
        "expected E_NONFINITE, got {diagnostics}"
    );

    let large_coefficient = format!("1{}", "0".repeat(308));
    let large_sine_level = format!("1{}", "0".repeat(100));
    let product_artifact = artifact(matrix_source(
        1,
        1,
        &format!("[[{large_coefficient}]]"),
        false,
        &large_sine_level,
    ));
    let mut product_engine = DspEngine::new_artifact(&product_artifact).unwrap();
    let mut product_callbacks = 0;
    let product_error = product_engine
        .render(|_| {
            product_callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(product_error.code(), "E_NONFINITE");
    assert_eq!(product_callbacks, 1);

    let sum_coefficient = format!("14{}", "0".repeat(307));
    let sum_artifact = artifact(matrix_source(
        2,
        1,
        &format!("[[{sum_coefficient}, {sum_coefficient}]]"),
        true,
        "1",
    ));
    let mut sum_engine = DspEngine::new_artifact(&sum_artifact).unwrap();
    let mut sum_callbacks = 0;
    let sum_error = sum_engine
        .render(|_| {
            sum_callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(sum_error.code(), "E_NONFINITE");
    assert_eq!(sum_callbacks, 1);
}
