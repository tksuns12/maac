use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::DspEngine;
use maac::plan::{PlanLimits, Processor};
use maac::PlanArtifact;
use serde_json::Value;

const OUTPUT_FRAMES: u64 = 2;

fn source(node_id: &str, project_seed: Option<&str>, node_seed: Option<&str>) -> SourceBundle {
    let project_seed = project_seed
        .map(|seed| format!("seed = {seed};"))
        .unwrap_or_default();
    let node_seed = node_seed
        .map(|seed| format!("seed = {seed};"))
        .unwrap_or_default();
    SourceBundle::new(
        "core-noise.maac",
        format!(
            r#"maac 1;
project p {{ score = [0q, 1/24000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &{node_id}:out; {project_seed} }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node {node_id} {{ type = "core.noise/1"; config = {{ channels = 1; {node_seed} }}; }}
"#,
            node_id = node_id,
            project_seed = project_seed,
            node_seed = node_seed,
        ),
    )
}

fn compile(node_id: &str, project_seed: Option<&str>, node_seed: Option<&str>) -> PlanArtifact {
    compile_bundle_artifact_with_limits(
        &source(node_id, project_seed, node_seed),
        &PlanLimits::song(),
    )
    .expect("core.noise source compiles")
}

fn limits(max_execution_work: u64) -> PlanLimits {
    PlanLimits {
        max_execution_work,
        ..PlanLimits::song()
    }
}

fn plan_error_code(result: Result<PlanArtifact, maac::plan::PlanError>) -> String {
    result.unwrap_err().code
}

fn execution_identity(
    node_id: &str,
    project_seed: Option<&str>,
    node_seed: Option<&str>,
) -> maac::production_identity::ExecutionIdentity {
    let bundle = source(node_id, project_seed, node_seed);
    let document = maac::parse(
        bundle
            .sources
            .get(&bundle.entry)
            .expect("noise source entry"),
    )
    .unwrap();
    let plan = maac::compile(&document).unwrap();
    maac::production_identity::execution_identity(&document, &plan).unwrap()
}

fn processor_json(artifact: &PlanArtifact, node_id: &str) -> Value {
    let json: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == node_id)
        .unwrap()["processor"]
        .clone()
}

#[test]
fn noise_defaults_full_u64_roundtrip_and_replay_are_strict() {
    let defaulted = compile("noise", Some("7"), None);
    let processor = processor_json(&defaulted, "noise");
    assert_eq!(processor["kind"], "noise");
    assert_eq!(processor["channels"], 1);
    assert_eq!(processor["seed"], 7);

    let max_seed = u64::MAX.to_string();
    let maximum = compile("noise", Some(&max_seed), Some(&max_seed));
    assert_eq!(processor_json(&maximum, "noise")["seed"], u64::MAX);
    assert_eq!(Processor::noise(1, u64::MAX).technical_latency_frames(), 0);

    let bytes = defaulted.to_json().unwrap();
    let retained = PlanArtifact::from_json(&bytes).unwrap();
    assert_eq!(retained.to_json().unwrap(), bytes);
    DspEngine::new_artifact(&retained).expect("retained noise artifact prepares");
}

#[test]
fn noise_identity_resolves_omitted_seed_and_retains_seed_and_id_sensitivity() {
    for seed in [
        "7".to_owned(),
        (1u64 << 63).to_string(),
        u64::MAX.to_string(),
    ] {
        let omitted = execution_identity("noise", Some(&seed), None);
        let explicit_default = execution_identity("noise", Some(&seed), Some(&seed));

        assert_eq!(omitted.execution_hash, explicit_default.execution_hash);
        assert_ne!(
            omitted.source_input_hash,
            explicit_default.source_input_hash
        );
    }

    let omitted = execution_identity("noise", Some("7"), None);
    let explicit_other = execution_identity("noise", Some("7"), Some("8"));
    let renamed = execution_identity("noise_renamed", Some("7"), None);

    assert_ne!(omitted.execution_hash, explicit_other.execution_hash);
    assert_ne!(omitted.execution_hash, renamed.execution_hash);
}

#[test]
fn noise_retained_validation_rejects_bad_channels_ports_and_parameters() {
    let artifact = compile("noise", Some("7"), None);
    let mut original: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    let index = original["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|node| node["id"] == "noise")
        .unwrap();

    let mut missing_seed = original.clone();
    missing_seed["nodes"][index]["processor"]
        .as_object_mut()
        .unwrap()
        .remove("seed");
    assert!(PlanArtifact::from_json(&serde_json::to_vec(&missing_seed).unwrap()).is_err());

    let mut negative_seed = original.clone();
    negative_seed["nodes"][index]["processor"]["seed"] = (-1).into();
    assert!(PlanArtifact::from_json(&serde_json::to_vec(&negative_seed).unwrap()).is_err());

    let mut overflow_seed = serde_json::to_string(&original).unwrap();
    let overflow = u128::from(u64::MAX) + 1;
    assert!(overflow_seed.contains("\"seed\":7"));
    overflow_seed = overflow_seed.replace("\"seed\":7", &format!("\"seed\":{overflow}"));
    assert!(PlanArtifact::from_json(overflow_seed.as_bytes()).is_err());

    for (channels, code) in [(0, "E_RANGE"), (3, "E_CAPABILITY")] {
        let mut json = original.clone();
        json["nodes"][index]["processor"]["channels"] = channels.into();
        assert_eq!(
            plan_error_code(PlanArtifact::from_json(&serde_json::to_vec(&json).unwrap())),
            code
        );
    }

    original["nodes"][index]["params"] = serde_json::json!({"level": "1/1"});
    assert_eq!(
        plan_error_code(PlanArtifact::from_json(
            &serde_json::to_vec(&original).unwrap()
        )),
        "E_UNKNOWN_FIELD"
    );

    let mut wrong_input = original;
    wrong_input["nodes"][index]["params"] = serde_json::json!({});
    wrong_input["connections"] = serde_json::json!([{
        "id": "invalid",
        "from": {"node": "noise", "port": "out"},
        "to": {"node": "noise", "port": "in"}
    }]);
    assert_eq!(
        plan_error_code(PlanArtifact::from_json(
            &serde_json::to_vec(&wrong_input).unwrap()
        )),
        "E_PORT_TYPE"
    );
}

#[test]
fn noise_hash_work_is_exact_at_id_block_boundaries() {
    let short_id = "n".repeat(21);
    let long_id = "n".repeat(22);
    let short_work = OUTPUT_FRAMES * 256;
    let long_work = OUTPUT_FRAMES * 512;

    for (id, work) in [(&short_id, short_work), (&long_id, long_work)] {
        compile_bundle_artifact_with_limits(&source(id, None, None), &limits(work))
            .expect("source passes at exact noise hash work");
        let diagnostics =
            compile_bundle_artifact_with_limits(&source(id, None, None), &limits(work - 1))
                .expect_err("source rejects one unit below noise hash work");
        assert_eq!(
            diagnostics.first().unwrap().code,
            maac::diagnostic::DiagnosticCode::ResourceLimit
        );

        let artifact = compile(id, None, None);
        artifact.validate_with_limits(&limits(work)).unwrap();
        assert_eq!(
            artifact
                .validate_with_limits(&limits(work - 1))
                .unwrap_err()
                .code,
            "E_RESOURCE_LIMIT"
        );
        let bytes = artifact.to_json_with_limits(&limits(work)).unwrap();
        let retained = PlanArtifact::from_json_with_limits(&bytes, &limits(work)).unwrap();
        assert_eq!(
            PlanArtifact::from_json_with_limits(&bytes, &limits(work - 1))
                .unwrap_err()
                .code,
            "E_RESOURCE_LIMIT"
        );
        DspEngine::new_artifact_with_limits(&retained, &limits(work)).unwrap();
        match DspEngine::new_artifact_with_limits(&retained, &limits(work - 1)) {
            Err(maac::dsp::RenderError::Plan(error)) => {
                assert_eq!(error.code, "E_RESOURCE_LIMIT")
            }
            Ok(_) => panic!("prepare unexpectedly passed below noise hash work"),
            Err(error) => panic!("unexpected prepare error: {error:?}"),
        }
    }
}
