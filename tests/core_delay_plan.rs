use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::DspEngine;
use maac::plan::{PlanLimits, Processor};
use maac::PlanArtifact;
use serde_json::Value;

const OUTPUT_FRAMES: u64 = 2;
const DELAY_PER_FRAME: u64 = 2;
const DELAY_CELLS: usize = 2;

fn source(frames: &str) -> SourceBundle {
    SourceBundle::new(
        "core-delay.maac",
        format!(
            r#"maac 1;
project p {{ score = [0q, 1/24000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &delay:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; params = {{ level = 0; }}; }}
node delay {{ type = "core.delay/1"; config = {{ channels = 1; frames = {frames}; }}; }}
connect route {{ from = &sine:out; to = &delay:in; }}
"#,
            frames = frames,
        ),
    )
}

fn compile(frames: &str) -> PlanArtifact {
    compile_bundle_artifact_with_limits(&source(frames), &PlanLimits::song())
        .expect("core.delay source compiles")
}

fn limits(max_execution_work: u64, max_production_delay_cells: usize) -> PlanLimits {
    PlanLimits {
        max_execution_work,
        max_production_delay_cells,
        ..PlanLimits::song()
    }
}

fn plan_error_code(result: Result<PlanArtifact, maac::plan::PlanError>) -> String {
    result.unwrap_err().code
}

fn execution_identity(frames: &str) -> maac::production_identity::ExecutionIdentity {
    let bundle = source(frames);
    let document = maac::parse(
        bundle
            .sources
            .get(&bundle.entry)
            .expect("delay source entry"),
    )
    .unwrap();
    let plan = maac::compile(&document).unwrap();
    maac::production_identity::execution_identity(&document, &plan).unwrap()
}

#[test]
fn delay_source_lowers_strictly_and_exposes_technical_latency() {
    let artifact = compile("2");
    let json: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    let delay = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "delay")
        .unwrap();
    assert_eq!(delay["processor"]["kind"], "delay");
    assert_eq!(delay["processor"]["channels"], 1);
    assert_eq!(delay["processor"]["frames"], 2);
    assert_eq!(delay["params"], serde_json::json!({}));

    let bytes = artifact.to_json().unwrap();
    assert_eq!(
        PlanArtifact::from_json(&bytes).unwrap().to_json().unwrap(),
        bytes
    );
    assert_ne!(
        execution_identity("2").execution_hash,
        execution_identity("3").execution_hash
    );
    assert_eq!(Processor::delay(1, 2).technical_latency_frames(), 2);
    assert_eq!(Processor::gain(1).technical_latency_frames(), 0);
}

#[test]
fn delay_retained_validation_enforces_dimensions_frames_ports_and_params() {
    let artifact = compile("2");
    let original: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    let index = original["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|node| node["id"] == "delay")
        .unwrap();

    for (field, value, code) in [("channels", 0, "E_RANGE"), ("channels", 3, "E_CAPABILITY")] {
        let mut json = original.clone();
        json["nodes"][index]["processor"][field] = value.into();
        assert_eq!(
            plan_error_code(PlanArtifact::from_json(&serde_json::to_vec(&json).unwrap())),
            code
        );
    }

    let mut zero_frames = original.clone();
    zero_frames["nodes"][index]["processor"]["frames"] = 0.into();
    assert_eq!(
        plan_error_code(PlanArtifact::from_json(
            &serde_json::to_vec(&zero_frames).unwrap()
        )),
        "E_RANGE"
    );

    let mut params = original.clone();
    params["nodes"][index]["params"] = serde_json::json!({"gain": "1/1"});
    assert_eq!(
        plan_error_code(PlanArtifact::from_json(
            &serde_json::to_vec(&params).unwrap()
        )),
        "E_UNKNOWN_FIELD"
    );

    let mut wrong_port = original.clone();
    wrong_port["connections"][0]["to"]["port"] = "out".into();
    assert_eq!(
        plan_error_code(PlanArtifact::from_json(
            &serde_json::to_vec(&wrong_port).unwrap()
        )),
        "E_PORT_TYPE"
    );

    let mut mismatched_channels = original.clone();
    mismatched_channels["nodes"][index]["processor"]["channels"] = 2.into();
    assert_eq!(
        plan_error_code(PlanArtifact::from_json(
            &serde_json::to_vec(&mismatched_channels).unwrap()
        )),
        "E_PORT_TYPE"
    );

    let huge_frames = u64::MAX.to_string();
    let huge_source =
        compile_bundle_artifact_with_limits(&source(&huge_frames), &PlanLimits::song())
            .expect_err("huge delay history must hit the bounded storage limit");
    assert_eq!(
        huge_source.first().unwrap().code,
        maac::diagnostic::DiagnosticCode::ResourceLimit
    );

    let mut huge_retained = original.clone();
    huge_retained["nodes"][index]["processor"]["frames"] = u64::MAX.into();
    assert_eq!(
        plan_error_code(PlanArtifact::from_json(
            &serde_json::to_vec(&huge_retained).unwrap()
        )),
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn delay_input_breaks_only_delay_edges_in_retained_causality() {
    let artifact = compile("2");
    let original: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();

    let mut self_delay = original.clone();
    self_delay["connections"] = serde_json::json!([{
        "id": "self_delay",
        "from": {"node": "delay", "port": "out"},
        "to": {"node": "delay", "port": "in"}
    }]);
    PlanArtifact::from_json(&serde_json::to_vec(&self_delay).unwrap())
        .expect("a delay self edge is causally broken");

    let mut gain_cycle = self_delay;
    gain_cycle["nodes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "gain",
            "processor": {"kind": "gain", "channels": 1},
            "params": {"gain": "1/1"}
        }));
    gain_cycle["connections"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "self_gain",
            "from": {"node": "gain", "port": "out"},
            "to": {"node": "gain", "port": "in"}
        }));
    assert_eq!(
        plan_error_code(PlanArtifact::from_json(
            &serde_json::to_vec(&gain_cycle).unwrap()
        )),
        "E_ALGEBRAIC_LOOP"
    );
}

#[test]
fn delay_work_and_storage_are_exact_at_source_retained_and_prepare_boundaries() {
    let artifact = compile("2");
    let work = OUTPUT_FRAMES * DELAY_PER_FRAME;
    let exact = limits(work, DELAY_CELLS);
    let below_work = limits(work - 1, DELAY_CELLS);
    let below_storage = limits(work, DELAY_CELLS - 1);

    compile_bundle_artifact_with_limits(&source("2"), &exact)
        .expect("source passes at exact delay work and storage");
    let diagnostics = compile_bundle_artifact_with_limits(&source("2"), &below_work)
        .expect_err("source rejects one unit below delay work");
    assert_eq!(
        diagnostics.first().unwrap().code,
        maac::diagnostic::DiagnosticCode::ResourceLimit
    );
    let storage_diagnostics = compile_bundle_artifact_with_limits(&source("2"), &below_storage)
        .expect_err("source rejects one cell below delay storage");
    assert_eq!(
        storage_diagnostics.first().unwrap().code,
        maac::diagnostic::DiagnosticCode::ResourceLimit
    );

    artifact.validate_with_limits(&exact).unwrap();
    assert_eq!(
        artifact.validate_with_limits(&below_work).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
    assert_eq!(
        artifact
            .validate_with_limits(&below_storage)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );

    let bytes = artifact.to_json_with_limits(&exact).unwrap();
    let retained = PlanArtifact::from_json_with_limits(&bytes, &exact).unwrap();
    assert_eq!(retained.to_json_with_limits(&exact).unwrap(), bytes);
    assert_eq!(
        PlanArtifact::from_json_with_limits(&bytes, &below_work)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
    assert_eq!(
        PlanArtifact::from_json_with_limits(&bytes, &below_storage)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );

    let mut two_delays: Value = serde_json::from_slice(&bytes).unwrap();
    two_delays["nodes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "delay2",
            "processor": {"kind": "delay", "channels": 1, "frames": 1},
            "params": {}
        }));
    two_delays["connections"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "route2",
            "from": {"node": "sine", "port": "out"},
            "to": {"node": "delay2", "port": "in"}
        }));
    let aggregate_exact = limits(work + OUTPUT_FRAMES * DELAY_PER_FRAME, DELAY_CELLS + 1);
    PlanArtifact::from_json_with_limits(
        &serde_json::to_vec(&two_delays).unwrap(),
        &aggregate_exact,
    )
    .expect("two delay buffers fit their aggregate bounds");
    let aggregate_below = limits(work + OUTPUT_FRAMES * DELAY_PER_FRAME, DELAY_CELLS);
    assert_eq!(
        PlanArtifact::from_json_with_limits(
            &serde_json::to_vec(&two_delays).unwrap(),
            &aggregate_below,
        )
        .unwrap_err()
        .code,
        "E_RESOURCE_LIMIT"
    );

    DspEngine::new_artifact_with_limits(&retained, &exact).unwrap();
    match DspEngine::new_artifact_with_limits(&retained, &below_work) {
        Err(maac::dsp::RenderError::Plan(error)) => assert_eq!(error.code, "E_RESOURCE_LIMIT"),
        Ok(_) => panic!("prepare unexpectedly passed below delay work"),
        Err(error) => panic!("unexpected prepare error: {error:?}"),
    }
    match DspEngine::new_artifact_with_limits(&retained, &below_storage) {
        Err(maac::dsp::RenderError::Plan(error)) => assert_eq!(error.code, "E_RESOURCE_LIMIT"),
        Ok(_) => panic!("prepare unexpectedly passed below delay storage"),
        Err(error) => panic!("unexpected prepare error: {error:?}"),
    }
}
