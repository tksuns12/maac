use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::diagnostic::DiagnosticCode;
use maac::dsp::DspEngine;
use maac::plan::PlanLimits;
use maac::PlanArtifact;
use serde_json::Value;

const OUTPUT_FRAMES: u64 = 2;
const FADER_PER_FRAME: u64 = 129;

fn source(level: &str, extras: &str) -> SourceBundle {
    let params = if level.is_empty() {
        String::new()
    } else {
        format!("params = {{ level = {level}; }};")
    };
    SourceBundle::new(
        "core-fader.maac",
        format!(
            r#"maac 1;
project p {{ score = [0q, 1/24000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &fader:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; params = {{ level = 0; }}; }}
node fader {{ type = "core.fader/1"; config = {{ channels = 1; }}; {params} }}
connect route {{ from = &sine:out; to = &fader:in; }}
{extras}
"#,
            params = params,
            extras = extras,
        ),
    )
}

fn compile(level: &str, extras: &str) -> PlanArtifact {
    compile_bundle_artifact_with_limits(&source(level, extras), &PlanLimits::song())
        .expect("core.fader source compiles")
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

#[test]
fn fader_defaults_roundtrip_and_accepts_unrestricted_finite_db_levels() {
    let default = compile("", "");
    let negative = compile("-120dB", "");
    let positive = compile("120dB", "");
    assert_eq!(default.output().total_frames, OUTPUT_FRAMES);

    let default_json: Value = serde_json::from_slice(&default.to_json().unwrap()).unwrap();
    let default_node = default_json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "fader")
        .unwrap();
    assert_eq!(default_node["processor"]["kind"], "fader");
    assert_eq!(default_node["params"]["level"], "0/1");

    let default_bundle = source("", "");
    let default_document = maac::parse(
        default_bundle
            .sources
            .get(&default_bundle.entry)
            .expect("default source entry"),
    )
    .unwrap();
    let default_plan = maac::compile(&default_document).unwrap();
    let explicit_bundle = source("0dB", "");
    let explicit_document = maac::parse(
        explicit_bundle
            .sources
            .get(&explicit_bundle.entry)
            .expect("explicit source entry"),
    )
    .unwrap();
    let explicit_plan = maac::compile(&explicit_document).unwrap();
    let default_identity =
        maac::production_identity::execution_identity(&default_document, &default_plan).unwrap();
    let explicit_identity =
        maac::production_identity::execution_identity(&explicit_document, &explicit_plan).unwrap();
    assert_eq!(
        default_identity.execution_hash,
        explicit_identity.execution_hash
    );
    assert_ne!(
        default_identity.source_input_hash,
        explicit_identity.source_input_hash
    );

    let negative_json: Value = serde_json::from_slice(&negative.to_json().unwrap()).unwrap();
    let negative_node = negative_json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "fader")
        .unwrap();
    assert_eq!(negative_node["params"]["level"], "-120/1");
    positive.validate().unwrap();

    let bytes = negative.to_json().unwrap();
    assert_eq!(
        PlanArtifact::from_json(&bytes).unwrap().to_json().unwrap(),
        bytes
    );
}

#[test]
fn fader_source_lowers_db_automation_and_modulation() {
    let artifact = compile(
        "-6dB",
        r#"node level { type = "core.constant/1"; params = { value = 0; }; }
modulate level_mod { from = &level:out; target = &fader.params.level; amount = -3dB; }
curve level_curve { clock = score; points = [(0q, -6dB, step), (1q, 3dB, step)]; }
automation level_lane { target = &fader.params.level; curve = &level_curve; at = 0q; }"#,
    );
    let json: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    assert_eq!(json["version"], 7);
    assert_eq!(json["modulations"][0]["amount"], "-3/1");
    assert_eq!(json["automation"][0]["points"][0]["value"], "-6/1");
}

#[test]
fn fader_retained_validation_rejects_bad_channels_nonfinite_and_db_exponential() {
    let artifact = compile("-6dB", "");
    let original: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    let index = original["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|node| node["id"] == "fader")
        .unwrap();

    for channels in [0, 3] {
        let mut json = original.clone();
        json["nodes"][index]["processor"]["channels"] = channels.into();
        let result = PlanArtifact::from_json(&serde_json::to_vec(&json).unwrap());
        let error = result.unwrap_err();
        assert_eq!(
            error.code,
            if channels == 0 {
                "E_RANGE"
            } else {
                "E_CAPABILITY"
            }
        );
    }

    let mut nonfinite = original.clone();
    nonfinite["nodes"][index]["params"]["level"] = format!("1{}/1", "0".repeat(309)).into();
    assert_eq!(
        PlanArtifact::from_json(&serde_json::to_vec(&nonfinite).unwrap())
            .unwrap_err()
            .code,
        "E_NONFINITE"
    );

    let mut exponential = original;
    exponential["automation"] = serde_json::json!([{
        "id": "level_lane",
        "target": {"node": "fader", "port": "level"},
        "clock": "score",
        "at": "0/1",
        "points": [
            {"position": "0/1", "value": "1/1", "shape": "exponential"},
            {"position": "1/1", "value": "3/1", "shape": "step"}
        ]
    }]);
    assert_eq!(
        PlanArtifact::from_json(&serde_json::to_vec(&exponential).unwrap())
            .unwrap_err()
            .code,
        "E_RANGE"
    );
}

#[test]
fn fader_work_is_exact_at_source_retained_and_prepare_boundaries() {
    let artifact = compile("-6dB", "");
    let budget = OUTPUT_FRAMES * FADER_PER_FRAME;
    let exact = limits(budget);
    let below = limits(budget - 1);

    compile_bundle_artifact_with_limits(&source("-6dB", ""), &exact)
        .expect("source compiles at exact fader work");
    let diagnostic = compile_bundle_artifact_with_limits(&source("-6dB", ""), &below)
        .expect_err("source rejects one unit below fader work");
    assert_eq!(
        diagnostic.first().unwrap().code,
        DiagnosticCode::ResourceLimit
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
        Ok(_) => panic!("prepare unexpectedly passed below fader work"),
        Err(error) => panic!("unexpected prepare error: {error:?}"),
    }
}
