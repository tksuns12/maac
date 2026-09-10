use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::DspEngine;
use maac::plan::PlanLimits;
use maac::PlanArtifact;
use serde_json::Value;

const OUTPUT_FRAMES: u64 = 2;
const MATRIX_PER_FRAME: u64 = 2;

fn source(coefficients: &str) -> SourceBundle {
    SourceBundle::new(
        "core-matrix.maac",
        format!(
            r#"maac 1;
project p {{ score = [0q, 1/24000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &matrix:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; params = {{ level = 0; }}; }}
node matrix {{ type = "core.matrix/1"; config = {{ inputs = 1; outputs = 1; coefficients = {coefficients}; }}; }}
connect route {{ from = &sine:out; to = &matrix:in; }}
"#,
            coefficients = coefficients,
        ),
    )
}

fn compile(coefficients: &str) -> PlanArtifact {
    compile_bundle_artifact_with_limits(&source(coefficients), &PlanLimits::song())
        .expect("core.matrix source compiles")
}

fn limits(max_execution_work: u64) -> PlanLimits {
    PlanLimits {
        max_execution_work,
        ..PlanLimits::song()
    }
}

fn assert_resource_limit(result: Result<(), maac::plan::PlanError>) {
    assert_eq!(result.unwrap_err().code, "E_RESOURCE_LIMIT");
}

fn execution_identity(coefficients: &str) -> maac::production_identity::ExecutionIdentity {
    let bundle = source(coefficients);
    let document = maac::parse(
        bundle
            .sources
            .get(&bundle.entry)
            .expect("matrix source entry"),
    )
    .unwrap();
    let plan = maac::compile(&document).unwrap();
    maac::production_identity::execution_identity(&document, &plan).unwrap()
}

#[test]
fn matrix_source_lowers_nested_coefficients_and_identity_preserves_sensitivity() {
    let artifact = compile("[[1/2]]");
    let json: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    let matrix = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "matrix")
        .unwrap();
    assert_eq!(matrix["processor"]["kind"], "matrix");
    assert_eq!(matrix["processor"]["inputs"], 1);
    assert_eq!(matrix["processor"]["outputs"], 1);
    assert_eq!(
        matrix["processor"]["coefficients"],
        serde_json::json!([["1/2"]])
    );

    let bytes = artifact.to_json().unwrap();
    assert_eq!(
        PlanArtifact::from_json(&bytes).unwrap().to_json().unwrap(),
        bytes
    );

    let equivalent = execution_identity("[[2/4]]");
    let original = execution_identity("[[1/2]]");
    let changed = execution_identity("[[-1/2]]");
    assert_eq!(original.execution_hash, equivalent.execution_hash);
    assert_ne!(original.execution_hash, changed.execution_hash);
    assert!(original.normalized_source_json.contains("coefficients"));
}

#[test]
fn matrix_retained_validation_enforces_dimensions_shape_and_finite_coefficients() {
    let artifact = compile("[[1/2]]");
    let original: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    let index = original["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|node| node["id"] == "matrix")
        .unwrap();

    for (field, value, code) in [("inputs", 0, "E_RANGE"), ("outputs", 3, "E_CAPABILITY")] {
        let mut json = original.clone();
        json["nodes"][index]["processor"][field] = value.into();
        assert_eq!(
            PlanArtifact::from_json(&serde_json::to_vec(&json).unwrap())
                .unwrap_err()
                .code,
            code
        );
    }

    let mut shape = original.clone();
    shape["nodes"][index]["processor"]["coefficients"] = serde_json::json!([]);
    assert_eq!(
        PlanArtifact::from_json(&serde_json::to_vec(&shape).unwrap())
            .unwrap_err()
            .code,
        "E_RANGE"
    );

    let mut row_shape = original.clone();
    row_shape["nodes"][index]["processor"]["coefficients"] = serde_json::json!([["1/2", "1/2"]]);
    assert_eq!(
        PlanArtifact::from_json(&serde_json::to_vec(&row_shape).unwrap())
            .unwrap_err()
            .code,
        "E_RANGE"
    );

    let mut nonfinite = original.clone();
    nonfinite["nodes"][index]["processor"]["coefficients"] =
        serde_json::json!([[format!("1{}/1", "0".repeat(400))]]);
    assert_eq!(
        PlanArtifact::from_json(&serde_json::to_vec(&nonfinite).unwrap())
            .unwrap_err()
            .code,
        "E_NONFINITE"
    );

    let mut large_finite = original.clone();
    let large_coefficient = format!("1{}1/1{}0", "0".repeat(400), "0".repeat(400));
    large_finite["nodes"][index]["processor"]["coefficients"] =
        serde_json::json!([[large_coefficient]]);
    let large_plan = PlanArtifact::from_json(&serde_json::to_vec(&large_finite).unwrap())
        .expect("large finite coefficient ratio remains representable");
    DspEngine::new_artifact_with_limits(&large_plan, &PlanLimits::song())
        .expect("prepare accepts large finite coefficient ratio");
    compile(&format!("[[{large_coefficient}]]"));

    let tight_limits = PlanLimits {
        max_rational_bits: 512,
        ..PlanLimits::song()
    };
    assert_eq!(
        PlanArtifact::from_json_with_limits(
            &serde_json::to_vec(&large_finite).unwrap(),
            &tight_limits,
        )
        .unwrap_err()
        .code,
        "E_RESOURCE_LIMIT"
    );

    let mut disconnected = original.clone();
    disconnected["connections"] = serde_json::json!([]);
    assert_eq!(
        PlanArtifact::from_json(&serde_json::to_vec(&disconnected).unwrap())
            .unwrap_err()
            .code,
        "E_PORT_TYPE"
    );

    let mut params = original;
    params["nodes"][index]["params"] = serde_json::json!({"level": "1/1"});
    assert_eq!(
        PlanArtifact::from_json(&serde_json::to_vec(&params).unwrap())
            .unwrap_err()
            .code,
        "E_UNKNOWN_FIELD"
    );
}

#[test]
fn matrix_work_is_exact_at_source_retained_and_prepare_boundaries() {
    let artifact = compile("[[1/2]]");
    let budget = OUTPUT_FRAMES * MATRIX_PER_FRAME;
    let exact = limits(budget);
    let below = limits(budget - 1);

    compile_bundle_artifact_with_limits(&source("[[1/2]]"), &exact)
        .expect("source compiles at exact matrix work");
    let diagnostics = compile_bundle_artifact_with_limits(&source("[[1/2]]"), &below)
        .expect_err("source rejects one unit below matrix work");
    assert_eq!(
        diagnostics.first().unwrap().code,
        maac::diagnostic::DiagnosticCode::ResourceLimit
    );

    artifact.validate_with_limits(&exact).unwrap();
    assert_resource_limit(artifact.validate_with_limits(&below));
    let bytes = artifact.to_json_with_limits(&exact).unwrap();
    let retained = PlanArtifact::from_json_with_limits(&bytes, &exact).unwrap();
    assert_eq!(retained.to_json_with_limits(&exact).unwrap(), bytes);
    assert_resource_limit(PlanArtifact::from_json_with_limits(&bytes, &below).map(|_| ()));

    DspEngine::new_artifact_with_limits(&retained, &exact).unwrap();
    match DspEngine::new_artifact_with_limits(&retained, &below) {
        Err(maac::dsp::RenderError::Plan(error)) => assert_eq!(error.code, "E_RESOURCE_LIMIT"),
        Ok(_) => panic!("prepare unexpectedly passed below matrix work"),
        Err(error) => panic!("unexpected prepare error: {error:?}"),
    }
}
