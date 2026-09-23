use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use maac::generic_lock::{
    DependencyIdentity, ExpectedAsset, ExpectedEngine, ExpectedOutput, ExpectedProcessor,
    GenericLock, LockVerificationContext,
};
use maac::generic_lock_generation::{
    GenericLockBuilder, GenericLockGenerationContext, OutputEvidence, ResolvedProcessor,
};
use maac::production_identity::canonical_json_bytes;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/l4")
}

fn read(rel: &str) -> Vec<u8> {
    fs::read(root().join(rel)).unwrap()
}

fn value(rel: &str) -> Value {
    serde_json::from_slice(&read(rel)).unwrap()
}

fn sha_uri(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn strict_descriptor_bytes() -> Vec<u8> {
    let descriptor = json!({
        "format": "maac.external-processor-descriptor",
        "wire_version": 1,
        "id": "fixture.external/1",
        "version": "1.0.0",
        "abi": "maac.fixture.native/1",
        "ports": [
            {
                "name": "in", "direction": "input", "kind": "audio", "channels": 2,
                "cardinality": "single", "zero_default": true, "empty_event_stream": null
            },
            {
                "name": "events", "direction": "input", "kind": "events", "channels": 0,
                "cardinality": null, "zero_default": null, "empty_event_stream": true
            },
            {
                "name": "out", "direction": "output", "kind": "audio", "channels": 2,
                "cardinality": null, "zero_default": null, "empty_event_stream": null
            }
        ],
        "events": {
            "native_notes": true,
            "expression_kinds": ["pitch"],
            "hit_keys": [],
            "message_protocols": ["midi1"]
        },
        "config": [
            {
                "name": "amount",
                "value_type": "number",
                "default": 1,
                "minimum": 0,
                "maximum": 1,
                "asset_kind": null
            }
        ],
        "parameters": [
            {
                "id": "gain",
                "source_name": "gain",
                "unit": "1",
                "default": 1,
                "minimum": 0,
                "maximum": 2,
                "range_policy": "error"
            }
        ],
        "parameter_rates": [
            {
                "parameter_id": "gain",
                "rate": "sample",
                "interpolation": ["step", "linear"]
            }
        ],
        "feedthrough": [
            {
                "output_port": "out",
                "input_ports": ["in"],
                "parameters": ["gain"]
            }
        ],
        "latency_frames": 2,
        "state": {
            "serialization_version": "maac.fixture.state/1",
            "reset_semantics": "reset_frame_zero",
            "save_restore": true
        },
        "determinism": {
            "mode": "declared_deterministic",
            "dependencies": ["locked_owner_closure"]
        },
        "permissions": {
            "assets": true,
            "network": false,
            "devices": false,
            "process_execution": false,
            "shared_memory": false
        }
    });
    canonical_json_bytes(&descriptor).unwrap()
}

fn expected_external_value(transitive_beta: bool) -> Value {
    let relative = if transitive_beta {
        "canonical/external-transitive-lock.json"
    } else {
        "canonical/external-ramp-lock.json"
    };
    let mut lock = value(relative);
    let descriptor = strict_descriptor_bytes();
    let digest = sha_uri(&descriptor);
    let length = descriptor.len().to_string();

    for asset in lock["assets"].as_array_mut().unwrap() {
        if asset["source"] == json!(["descriptor_asset"]) {
            asset["sha256"] = json!(digest.clone());
            asset["bytes"] = json!(length.clone());
        }
    }
    for dependency in lock["dependencies"].as_array_mut().unwrap() {
        if dependency["owner"] == json!(["fx"])
            && dependency["role"] == json!(["processor", "descriptor"])
        {
            dependency["sha256"] = json!(digest.clone());
            dependency["bytes"] = json!(length.clone());
        }
    }
    let processor = &mut lock["processors"].as_array_mut().unwrap()[0];
    processor["descriptor_hash"] = json!(digest.clone());
    processor["config"]["descriptor_hash"] = json!(digest);
    processor["config_digest"] = json!(sha_uri(
        &canonical_json_bytes(&processor["config"]).unwrap()
    ));

    let mut render_input = lock.as_object().unwrap().clone();
    render_input.remove("render_key");
    render_input.remove("evidence");
    render_input.insert(
        "format".into(),
        Value::String("maac.generic-render-input".into()),
    );
    lock["render_key"] = json!(sha_uri(
        &canonical_json_bytes(&Value::Object(render_input)).unwrap()
    ));
    lock
}

fn dependency(owner: Option<Vec<&str>>, role: &[&str]) -> DependencyIdentity {
    DependencyIdentity {
        owner: owner.map(|path| path.into_iter().map(str::to_owned).collect()),
        role: role.iter().map(|part| (*part).to_owned()).collect(),
    }
}

fn minimal_context() -> GenericLockGenerationContext {
    let lock = value("canonical/minimal-lock.json");
    GenericLockGenerationContext {
        execution_preimage: value("preimages/minimal-execution.json"),
        assets: BTreeMap::new(),
        dependencies: BTreeMap::from([(
            dependency(None, &["engine", "implementation"]),
            read("contracts/fixture-engine.json"),
        )]),
        processors: BTreeMap::from([(
            vec!["sine".into()],
            ResolvedProcessor {
                processor_type: "core.sine/1".into(),
                implementation_hash: None,
                descriptor_hash: None,
                adapter_id: None,
                state_hash: None,
                config: lock["processors"][0]["config"]["config"].clone(),
                latency_frames: 0,
                determinism: "declared_deterministic".into(),
            },
        )]),
        engine: ExpectedEngine {
            implementation_id: "maac.fixture.engine/1".into(),
            build_id: "maac.fixture.build/1".into(),
            platform_id: "maac.fixture.portable/1".into(),
            architecture_id: "maac.fixture.generic/1".into(),
            numerical_mode_id: "maac.fixture.exact/1".into(),
            sample_rate: 48_000,
            block_schedule: None,
            block_independent: true,
        },
        output: ExpectedOutput {
            port: lock["output"]["port"].clone(),
            score: [
                lock["output"]["score"][0].clone(),
                lock["output"]["score"][1].clone(),
            ],
            tail: lock["output"]["tail"].clone(),
            render_frames: 24_000,
            crop: (0, 4),
            channel_order: vec![0],
        },
    }
}

fn external_assets() -> BTreeMap<Vec<String>, ExpectedAsset> {
    BTreeMap::from([
        (
            vec!["asset_a".into()],
            ExpectedAsset {
                kind: "blob".into(),
                bytes: read("payloads/shared-slot.bin"),
            },
        ),
        (
            vec!["asset_b".into()],
            ExpectedAsset {
                kind: "blob".into(),
                bytes: read("payloads/shared-slot.bin"),
            },
        ),
        (
            vec!["descriptor_asset".into()],
            ExpectedAsset {
                kind: "descriptor".into(),
                bytes: strict_descriptor_bytes(),
            },
        ),
        (
            vec!["impl_asset".into()],
            ExpectedAsset {
                kind: "module".into(),
                bytes: read("payloads/fixture-external-implementation.bin"),
            },
        ),
        (
            vec!["state_asset".into()],
            ExpectedAsset {
                kind: "blob".into(),
                bytes: read("payloads/fixture-external-state.bin"),
            },
        ),
    ])
}

fn external_dependencies(transitive_beta: bool) -> BTreeMap<DependencyIdentity, Vec<u8>> {
    let owner = Some(vec!["fx"]);
    BTreeMap::from([
        (
            dependency(
                owner.clone(),
                &["auxiliary", "maac.fixture.external/1", "alpha"],
            ),
            read("payloads/shared-slot.bin"),
        ),
        (
            dependency(
                owner.clone(),
                &["auxiliary", "maac.fixture.external/1", "beta"],
            ),
            if transitive_beta {
                read("payloads/changed-slot.bin")
            } else {
                read("payloads/shared-slot.bin")
            },
        ),
        (
            dependency(
                owner.clone(),
                &["imported-source", "maac.fixture.external/1", "alias_a"],
            ),
            read("payloads/shared-slot.bin"),
        ),
        (
            dependency(
                owner.clone(),
                &["imported-source", "maac.fixture.external/1", "alias_b"],
            ),
            read("payloads/shared-slot.bin"),
        ),
        (
            dependency(owner.clone(), &["processor", "adapter"]),
            read("payloads/fixture-external-adapter.bin"),
        ),
        (
            dependency(owner.clone(), &["processor", "descriptor"]),
            strict_descriptor_bytes(),
        ),
        (
            dependency(owner.clone(), &["processor", "implementation"]),
            read("payloads/fixture-external-implementation.bin"),
        ),
        (
            dependency(owner, &["processor", "state"]),
            read("payloads/fixture-external-state.bin"),
        ),
        (
            dependency(None, &["engine", "implementation"]),
            read("contracts/fixture-engine.json"),
        ),
    ])
}

fn external_context(transitive_beta: bool) -> GenericLockGenerationContext {
    let lock = value("canonical/external-ramp-lock.json");
    let processor = &lock["processors"][0];
    GenericLockGenerationContext {
        execution_preimage: value("preimages/ramp-execution.json"),
        assets: external_assets(),
        dependencies: external_dependencies(transitive_beta),
        processors: BTreeMap::from([(
            vec!["fx".into()],
            ResolvedProcessor {
                processor_type: "fixture.external/1".into(),
                implementation_hash: processor["implementation_hash"].as_str().map(str::to_owned),
                descriptor_hash: Some(sha_uri(&strict_descriptor_bytes())),
                adapter_id: processor["adapter_id"].as_str().map(str::to_owned),
                state_hash: processor["state_hash"].as_str().map(str::to_owned),
                config: processor["config"]["config"].clone(),
                latency_frames: 2,
                determinism: "declared_deterministic".into(),
            },
        )]),
        engine: ExpectedEngine {
            implementation_id: "maac.fixture.engine/1".into(),
            build_id: "maac.fixture.build/1".into(),
            platform_id: "maac.fixture.portable/1".into(),
            architecture_id: "maac.fixture.generic/1".into(),
            numerical_mode_id: "maac.fixture.exact/1".into(),
            sample_rate: 48_000,
            block_schedule: Some(vec![16_000, 16_302]),
            block_independent: true,
        },
        output: ExpectedOutput {
            port: lock["output"]["port"].clone(),
            score: [
                lock["output"]["score"][0].clone(),
                lock["output"]["score"][1].clone(),
            ],
            tail: lock["output"]["tail"].clone(),
            render_frames: 32_302,
            crop: (0, 32_302),
            channel_order: vec![1, 0],
        },
    }
}

fn verification_context(
    generated: &maac::generic_lock_generation::GeneratedGenericLock,
    context: &GenericLockGenerationContext,
    evidence: Option<&OutputEvidence>,
) -> LockVerificationContext {
    let processors = context
        .processors
        .iter()
        .map(|(node, processor)| {
            (
                node.clone(),
                ExpectedProcessor {
                    processor_type: processor.processor_type.clone(),
                    implementation_hash: processor.implementation_hash.clone(),
                    descriptor_hash: processor.descriptor_hash.clone(),
                    adapter_id: processor.adapter_id.clone(),
                    state_hash: processor.state_hash.clone(),
                    config: generated.config(node).unwrap().clone(),
                    latency_frames: processor.latency_frames,
                    determinism: processor.determinism.clone(),
                },
            )
        })
        .collect();
    LockVerificationContext {
        execution_preimage: context.execution_preimage.clone(),
        assets: context.assets.clone(),
        dependencies: context.dependencies.clone(),
        processors,
        engine: context.engine.clone(),
        output: context.output.clone(),
        pcm: evidence.map(|e| e.pcm.clone()),
        file: evidence.and_then(|e| e.file.clone()),
    }
}

#[test]
fn minimal_generation_matches_all_canonical_artifacts() {
    let context = minimal_context();
    let generated = GenericLockBuilder::new(&context).build().unwrap();

    assert_eq!(
        generated.config_bytes(&["sine".into()]).unwrap().unwrap(),
        read("canonical/minimal-config.json")
    );
    assert_eq!(
        generated.render_input_bytes().unwrap(),
        read("canonical/minimal-render-input.json")
    );
    assert_eq!(
        generated.lock_bytes().unwrap(),
        read("canonical/minimal-lock.json")
    );
}

#[test]
fn external_generation_preserves_order_schedule_channels_and_closure() {
    let context = external_context(false);
    let generated = GenericLockBuilder::new(&context).build().unwrap();

    let expected = expected_external_value(false);
    assert_eq!(
        generated.config_bytes(&["fx".into()]).unwrap().unwrap(),
        canonical_json_bytes(&expected["processors"][0]["config"]).unwrap()
    );
    let mut expected_render_input = expected.as_object().unwrap().clone();
    expected_render_input.remove("render_key");
    expected_render_input.remove("evidence");
    expected_render_input.insert(
        "format".into(),
        Value::String("maac.generic-render-input".into()),
    );
    assert_eq!(
        generated.render_input_bytes().unwrap(),
        canonical_json_bytes(&Value::Object(expected_render_input)).unwrap()
    );
    assert_eq!(
        generated.lock_bytes().unwrap(),
        canonical_json_bytes(&expected).unwrap()
    );

    let lock = generated.lock().value();
    assert_eq!(
        lock["output"]["channel_order"],
        serde_json::json!(["1", "0"])
    );
    assert_eq!(
        lock["engine"]["block_schedule"],
        serde_json::json!(["16000", "16302"])
    );
    assert_eq!(lock["dependencies"].as_array().unwrap().len(), 9);
}

#[test]
fn canonical_dependency_order_is_not_btreemap_identity_order() {
    let context = external_context(false);
    let first_map_key = context.dependencies.keys().next().unwrap();
    assert!(first_map_key.owner.is_none());

    let generated = GenericLockBuilder::new(&context).build().unwrap();
    let dependencies = generated.lock().value()["dependencies"].as_array().unwrap();
    assert!(dependencies.first().unwrap()["owner"].is_array());
    assert!(dependencies.last().unwrap()["owner"].is_null());

    let mut reversed = context.dependencies.iter().rev().collect::<Vec<_>>();
    let mut rebuilt = BTreeMap::new();
    for (identity, bytes) in reversed.drain(..) {
        rebuilt.insert(identity.clone(), bytes.clone());
    }
    let mut reordered_context = external_context(false);
    reordered_context.dependencies = rebuilt;
    assert_eq!(
        GenericLockBuilder::new(&reordered_context)
            .build()
            .unwrap()
            .lock_bytes()
            .unwrap(),
        generated.lock_bytes().unwrap()
    );
}

#[test]
fn transitive_imported_auxiliary_and_same_byte_slots_are_retained() {
    let normal = GenericLockBuilder::new(&external_context(false))
        .build()
        .unwrap();
    let transitive_context = external_context(true);
    let transitive = GenericLockBuilder::new(&transitive_context)
        .build()
        .unwrap();

    let expected = expected_external_value(true);
    assert_eq!(
        transitive.lock_bytes().unwrap(),
        canonical_json_bytes(&expected).unwrap()
    );
    let mut expected_render_input = expected.as_object().unwrap().clone();
    expected_render_input.remove("render_key");
    expected_render_input.remove("evidence");
    expected_render_input.insert(
        "format".into(),
        Value::String("maac.generic-render-input".into()),
    );
    assert_eq!(
        transitive.render_input_bytes().unwrap(),
        canonical_json_bytes(&Value::Object(expected_render_input)).unwrap()
    );
    assert_ne!(normal.lock().render_key(), transitive.lock().render_key());

    let deps = transitive.lock().value()["dependencies"]
        .as_array()
        .unwrap();
    let shared_hash = "sha256:88aaa241d93be0e3fcd96902dd9b5bd02f0dd4cada32a3b4f6d820bc0a517276";
    assert_eq!(
        deps.iter()
            .filter(|entry| entry["sha256"] == shared_hash)
            .count(),
        3
    );
}

#[test]
fn external_state_present_and_state_null_are_both_explicit() {
    GenericLockBuilder::new(&external_context(false))
        .build()
        .unwrap();

    let mut context = external_context(false);
    context
        .processors
        .get_mut(&vec!["fx".into()])
        .unwrap()
        .state_hash = None;
    context
        .dependencies
        .remove(&dependency(Some(vec!["fx"]), &["processor", "state"]));
    context.assets.remove(&vec!["state_asset".into()]);
    let generated = GenericLockBuilder::new(&context).build().unwrap();
    assert!(generated.lock().value()["processors"][0]["state_hash"].is_null());
    assert!(!generated.lock().value()["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["role"] == serde_json::json!(["processor", "state"])));
}

#[test]
fn missing_mismatched_and_tampered_external_inputs_fail_explicitly() {
    let mut missing = external_context(false);
    missing
        .dependencies
        .remove(&dependency(Some(vec!["fx"]), &["processor", "descriptor"]));
    assert_eq!(
        GenericLockBuilder::new(&missing).build().unwrap_err().code,
        "E_REFERENCE"
    );

    let mut tampered = external_context(false);
    *tampered
        .dependencies
        .get_mut(&dependency(
            Some(vec!["fx"]),
            &["processor", "implementation"],
        ))
        .unwrap() = b"tampered implementation".to_vec();
    assert_eq!(
        GenericLockBuilder::new(&tampered).build().unwrap_err().code,
        "E_DIGEST"
    );

    let mut mismatch = external_context(false);
    mismatch
        .processors
        .get_mut(&vec!["fx".into()])
        .unwrap()
        .processor_type = "fixture.other/1".into();
    assert_eq!(
        GenericLockBuilder::new(&mismatch).build().unwrap_err().code,
        "E_CAPABILITY"
    );
}

#[test]
fn evidence_is_output_only_and_does_not_change_render_key() {
    let context = minimal_context();
    let plain = GenericLockBuilder::new(&context).build().unwrap();
    let pcm = read("evidence/minimal.pcm");
    let evidence = OutputEvidence {
        pcm: pcm.clone(),
        file: None,
    };
    let with_pcm = GenericLockBuilder::new(&context)
        .evidence(&evidence)
        .build()
        .unwrap();

    assert_eq!(plain.lock().render_key(), with_pcm.lock().render_key());
    assert_eq!(
        plain.render_input_bytes().unwrap(),
        with_pcm.render_input_bytes().unwrap()
    );
    assert_eq!(
        with_pcm.lock_bytes().unwrap(),
        read("canonical/minimal-evidence-pcm-lock.json")
    );

    let with_file_evidence = OutputEvidence {
        pcm: pcm.clone(),
        file: Some(pcm),
    };
    let with_file = GenericLockBuilder::new(&context)
        .evidence(&with_file_evidence)
        .build()
        .unwrap();
    assert_eq!(plain.lock().render_key(), with_file.lock().render_key());
    assert_eq!(
        with_file.lock_bytes().unwrap(),
        read("canonical/minimal-evidence-file-lock.json")
    );
}

#[test]
fn generation_parse_and_independent_verify_round_trip() {
    let context = external_context(false);
    let generated = GenericLockBuilder::new(&context).build().unwrap();
    let bytes = generated.lock_bytes().unwrap();
    let parsed = GenericLock::from_json(&bytes).unwrap();
    let verified = parsed
        .verify(&verification_context(&generated, &context, None))
        .unwrap();

    assert_eq!(verified.render_key, generated.lock().render_key());
    assert_eq!(verified.crop, (0, 32_302));
    assert_eq!(verified.channels, 2);
}

#[test]
fn timing_and_evidence_bounds_fail_without_guessing() {
    let mut bad_schedule = external_context(false);
    bad_schedule.engine.block_schedule = Some(vec![16_000, 16_301]);
    assert_eq!(
        GenericLockBuilder::new(&bad_schedule)
            .build()
            .unwrap_err()
            .code,
        "E_TIMING"
    );

    let context = minimal_context();
    let short = OutputEvidence {
        pcm: vec![0; 12],
        file: None,
    };
    assert_eq!(
        GenericLockBuilder::new(&context)
            .evidence(&short)
            .build()
            .unwrap_err()
            .code,
        "E_EVIDENCE"
    );

    let nonfinite = OutputEvidence {
        pcm: f32::NAN
            .to_le_bytes()
            .into_iter()
            .cycle()
            .take(16)
            .collect(),
        file: None,
    };
    assert_eq!(
        GenericLockBuilder::new(&context)
            .evidence(&nonfinite)
            .build()
            .unwrap_err()
            .code,
        "E_EVIDENCE"
    );
}
