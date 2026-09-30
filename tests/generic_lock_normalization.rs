use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use maac::generic_lock::DependencyIdentity;
use maac::generic_lock_normalization::{
    generate_generic_lock, GenericLockBuildContext, OutputEvidence, ResolvedAsset, ResolvedEngine,
    ResolvedOutput, ResolvedProcessor,
};
use maac::production_identity::canonical_json_bytes;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn fixture(relative: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("conformance/l4")
            .join(relative),
    )
    .unwrap()
}

fn value(relative: &str) -> Value {
    serde_json::from_slice(&fixture(relative)).unwrap()
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn external_descriptor() -> Vec<u8> {
    canonical_json_bytes(&json!({
        "format": "maac.external-processor-descriptor",
        "wire_version": 1,
        "id": "fixture.external/1",
        "version": "1.0.0",
        "abi": "maac.fixture.native/1",
        "ports": [
            {
                "name":"in","direction":"input","kind":"audio","channels":2,
                "cardinality":"single","zero_default":true,"empty_event_stream":null
            },
            {
                "name":"events","direction":"input","kind":"events","channels":0,
                "cardinality":null,"zero_default":null,"empty_event_stream":true
            },
            {
                "name":"out","direction":"output","kind":"audio","channels":2,
                "cardinality":null,"zero_default":null,"empty_event_stream":null
            }
        ],
        "events": {
            "native_notes":true,"expression_kinds":["pitch"],"hit_keys":[],
            "message_protocols":["midi1"]
        },
        "config": [{
            "name":"amount","value_type":"number","default":1,
            "minimum":0,"maximum":1,"asset_kind":null
        }],
        "parameters": [{
            "id":"gain","source_name":"gain","unit":"1","default":1,
            "minimum":0,"maximum":2,"range_policy":"error"
        }],
        "parameter_rates": [{
            "parameter_id":"gain","rate":"sample","interpolation":["step","linear"]
        }],
        "feedthrough": [{
            "output_port":"out","input_ports":["in"],"parameters":["gain"]
        }],
        "latency_frames": 2,
        "state": {
            "serialization_version":"maac.fixture.state/1",
            "reset_semantics":"reset_frame_zero",
            "save_restore":true
        },
        "determinism": {
            "mode":"declared_deterministic",
            "dependencies":["locked_owner_closure"]
        },
        "permissions": {
            "assets":true,"network":false,"devices":false,
            "process_execution":false,"shared_memory":false
        }
    }))
    .unwrap()
}

fn engine() -> ResolvedEngine {
    ResolvedEngine {
        implementation_id: "maac.fixture.engine/1".into(),
        build_id: "maac.fixture.build/1".into(),
        platform_id: "maac.fixture.portable/1".into(),
        architecture_id: "maac.fixture.generic/1".into(),
        numerical_mode_id: "maac.fixture.exact/1".into(),
        sample_rate: 48_000,
        block_schedule: None,
        block_independent: true,
    }
}

fn engine_dependency() -> (DependencyIdentity, Vec<u8>) {
    (
        DependencyIdentity {
            owner: None,
            role: vec!["engine".into(), "implementation".into()],
        },
        fixture("contracts/fixture-engine.json"),
    )
}

fn minimal_context() -> GenericLockBuildContext {
    GenericLockBuildContext {
        execution_preimage: value("preimages/minimal-execution.json"),
        assets: BTreeMap::new(),
        dependencies: BTreeMap::from([engine_dependency()]),
        processors: BTreeMap::from([(
            vec!["sine".into()],
            ResolvedProcessor {
                processor_type: "core.sine/1".into(),
                implementation_hash: None,
                descriptor_hash: None,
                adapter_id: None,
                state_hash: None,
                normalized_config: json!({
                    "t": "record",
                    "fields": {"voices": {"t":"number","n":"64","d":"1"}}
                }),
                latency_frames: 0,
                determinism: "declared_deterministic".into(),
            },
        )]),
        engine: engine(),
        output: ResolvedOutput {
            port: json!({"t":"ref","path":["sine"],"port":"out"}),
            score: [
                json!({"t":"quantity","n":"0","d":"1","u":"q"}),
                json!({"t":"quantity","n":"1","d":"1","u":"q"}),
            ],
            tail: json!({"t":"quantity","n":"0","d":"1","u":"s"}),
            render_frames: 24_000,
            crop: (0, 4),
            channel_order: vec![0],
        },
        evidence: None,
    }
}

fn verification_context(
    context: &GenericLockBuildContext,
) -> maac::generic_lock::LockVerificationContext {
    maac::generic_lock::LockVerificationContext {
        execution_preimage: context.execution_preimage.clone(),
        assets: context
            .assets
            .iter()
            .map(|(path, asset)| {
                (
                    path.clone(),
                    maac::generic_lock::ExpectedAsset {
                        kind: asset.kind.clone(),
                        bytes: asset.bytes.clone(),
                    },
                )
            })
            .collect(),
        dependencies: context.dependencies.clone(),
        processors: context
            .processors
            .iter()
            .map(|(node, processor)| {
                (
                    node.clone(),
                    maac::generic_lock::ExpectedProcessor {
                        processor_type: processor.processor_type.clone(),
                        implementation_hash: processor.implementation_hash.clone(),
                        descriptor_hash: processor.descriptor_hash.clone(),
                        adapter_id: processor.adapter_id.clone(),
                        state_hash: processor.state_hash.clone(),
                        config: json!({
                            "format": "maac.generic-config",
                            "version": 1,
                            "processor_type": processor.processor_type,
                            "descriptor_hash": processor.descriptor_hash,
                            "config": processor.normalized_config,
                        }),
                        latency_frames: processor.latency_frames,
                        determinism: processor.determinism.clone(),
                    },
                )
            })
            .collect(),
        engine: maac::generic_lock::ExpectedEngine {
            implementation_id: context.engine.implementation_id.clone(),
            build_id: context.engine.build_id.clone(),
            platform_id: context.engine.platform_id.clone(),
            architecture_id: context.engine.architecture_id.clone(),
            numerical_mode_id: context.engine.numerical_mode_id.clone(),
            sample_rate: context.engine.sample_rate,
            block_schedule: context.engine.block_schedule.clone(),
            block_independent: context.engine.block_independent,
        },
        output: maac::generic_lock::ExpectedOutput {
            port: context.output.port.clone(),
            score: context.output.score.clone(),
            tail: context.output.tail.clone(),
            render_frames: context.output.render_frames,
            crop: context.output.crop,
            channel_order: context.output.channel_order.clone(),
        },
        pcm: context.evidence.as_ref().map(|e| e.pcm.clone()),
        file: context.evidence.as_ref().and_then(|e| e.file.clone()),
    }
}

fn external_context(with_state: bool) -> GenericLockBuildContext {
    let descriptor = external_descriptor();
    let implementation = fixture("payloads/fixture-external-implementation.bin");
    let adapter = fixture("payloads/fixture-external-adapter.bin");
    let state = fixture("payloads/fixture-external-state.bin");
    let shared = fixture("payloads/shared-slot.bin");
    let owner = Some(vec!["fx".into()]);
    let mut dependencies = BTreeMap::from([
        (
            DependencyIdentity {
                owner: owner.clone(),
                role: vec![
                    "auxiliary".into(),
                    "maac.fixture.external/1".into(),
                    "alpha".into(),
                ],
            },
            shared.clone(),
        ),
        (
            DependencyIdentity {
                owner: owner.clone(),
                role: vec![
                    "auxiliary".into(),
                    "maac.fixture.external/1".into(),
                    "beta".into(),
                ],
            },
            shared.clone(),
        ),
        (
            DependencyIdentity {
                owner: owner.clone(),
                role: vec![
                    "imported-source".into(),
                    "maac.fixture.external/1".into(),
                    "alias_a".into(),
                ],
            },
            shared.clone(),
        ),
        (
            DependencyIdentity {
                owner: owner.clone(),
                role: vec![
                    "imported-source".into(),
                    "maac.fixture.external/1".into(),
                    "alias_b".into(),
                ],
            },
            shared.clone(),
        ),
        (
            DependencyIdentity {
                owner: owner.clone(),
                role: vec!["processor".into(), "adapter".into()],
            },
            adapter,
        ),
        (
            DependencyIdentity {
                owner: owner.clone(),
                role: vec!["processor".into(), "descriptor".into()],
            },
            descriptor.clone(),
        ),
        (
            DependencyIdentity {
                owner: owner.clone(),
                role: vec!["processor".into(), "implementation".into()],
            },
            implementation.clone(),
        ),
        engine_dependency(),
    ]);
    if with_state {
        dependencies.insert(
            DependencyIdentity {
                owner: owner.clone(),
                role: vec!["processor".into(), "state".into()],
            },
            state.clone(),
        );
    }

    let mut e = engine();
    e.block_schedule = Some(vec![16_000, 16_302]);

    GenericLockBuildContext {
        execution_preimage: value("preimages/ramp-execution.json"),
        assets: BTreeMap::from([
            (
                vec!["asset_a".into()],
                ResolvedAsset {
                    kind: "blob".into(),
                    bytes: shared.clone(),
                },
            ),
            (
                vec!["asset_b".into()],
                ResolvedAsset {
                    kind: "blob".into(),
                    bytes: shared,
                },
            ),
            (
                vec!["descriptor_asset".into()],
                ResolvedAsset {
                    kind: "descriptor".into(),
                    bytes: descriptor.clone(),
                },
            ),
            (
                vec!["impl_asset".into()],
                ResolvedAsset {
                    kind: "module".into(),
                    bytes: implementation.clone(),
                },
            ),
            (
                vec!["state_asset".into()],
                ResolvedAsset {
                    kind: "blob".into(),
                    bytes: state.clone(),
                },
            ),
        ]),
        dependencies,
        processors: BTreeMap::from([(
            vec!["fx".into()],
            ResolvedProcessor {
                processor_type: "fixture.external/1".into(),
                implementation_hash: Some(digest(&implementation)),
                descriptor_hash: Some(digest(&descriptor)),
                adapter_id: Some("maac.fixture.adapter/1".into()),
                state_hash: with_state.then(|| digest(&state)),
                normalized_config: json!({
                    "t":"record",
                    "fields":{
                        "amount":{"t":"number","n":"1","d":"3"},
                        "label":{"t":"string","v":"雪\nline\u{1}"}
                    }
                }),
                latency_frames: 2,
                determinism: "declared_deterministic".into(),
            },
        )]),
        engine: e,
        output: ResolvedOutput {
            port: json!({"t":"ref","path":["fx"],"port":"out"}),
            score: [
                json!({"t":"quantity","n":"1","d":"1","u":"q"}),
                json!({"t":"quantity","n":"3","d":"1","u":"q"}),
            ],
            tail: json!({"t":"quantity","n":"0","d":"1","u":"s"}),
            render_frames: 32_302,
            crop: (0, 32_302),
            channel_order: vec![1, 0],
        },
        evidence: None,
    }
}

#[test]
fn minimal_generation_matches_canonical_v1_artifacts() {
    let generated = generate_generic_lock(&minimal_context()).unwrap();

    assert_eq!(
        generated.configs[&vec!["sine".into()]],
        fixture("canonical/minimal-config.json")
    );
    assert_eq!(
        generated.render_input_bytes,
        fixture("canonical/minimal-render-input.json")
    );
    assert_eq!(generated.lock_bytes, fixture("canonical/minimal-lock.json"));
    generated
        .lock
        .verify(&{
            let context = minimal_context();
            maac::generic_lock::LockVerificationContext {
                execution_preimage: context.execution_preimage,
                assets: BTreeMap::new(),
                dependencies: context.dependencies,
                processors: BTreeMap::from([(
                    vec!["sine".into()],
                    maac::generic_lock::ExpectedProcessor {
                        processor_type: "core.sine/1".into(),
                        implementation_hash: None,
                        descriptor_hash: None,
                        adapter_id: None,
                        state_hash: None,
                        config: value("canonical/minimal-config.json"),
                        latency_frames: 0,
                        determinism: "declared_deterministic".into(),
                    },
                )]),
                engine: maac::generic_lock::ExpectedEngine {
                    implementation_id: context.engine.implementation_id,
                    build_id: context.engine.build_id,
                    platform_id: context.engine.platform_id,
                    architecture_id: context.engine.architecture_id,
                    numerical_mode_id: context.engine.numerical_mode_id,
                    sample_rate: context.engine.sample_rate,
                    block_schedule: None,
                    block_independent: true,
                },
                output: maac::generic_lock::ExpectedOutput {
                    port: context.output.port,
                    score: context.output.score,
                    tail: context.output.tail,
                    render_frames: context.output.render_frames,
                    crop: context.output.crop,
                    channel_order: context.output.channel_order,
                },
                pcm: None,
                file: None,
            }
        })
        .unwrap();
}

#[test]
fn external_generation_retains_distinct_closure_slots_and_schedule_order() {
    let context = external_context(true);
    let generated = generate_generic_lock(&context).unwrap();

    let reparsed = maac::generic_lock::GenericLock::from_json(&generated.lock_bytes).unwrap();
    assert_eq!(reparsed.render_key(), generated.lock.render_key());
    reparsed.verify(&verification_context(&context)).unwrap();

    let dependencies = generated.lock.value()["dependencies"].as_array().unwrap();
    let shared = dependencies
        .iter()
        .filter(|d| d["sha256"] == digest(&fixture("payloads/shared-slot.bin")))
        .count();
    assert_eq!(shared, 4);
    assert_eq!(
        generated.lock.value()["output"]["channel_order"],
        json!(["1", "0"])
    );
    assert_eq!(
        generated.lock.value()["engine"]["block_schedule"],
        json!(["16000", "16302"])
    );
}

#[test]
fn canonical_identity_order_does_not_use_rust_path_order() {
    let mut context = minimal_context();
    context.assets.insert(
        vec!["a".into()],
        ResolvedAsset {
            kind: "blob".into(),
            bytes: b"short".to_vec(),
        },
    );
    context.assets.insert(
        vec!["a".into(), "b".into()],
        ResolvedAsset {
            kind: "blob".into(),
            bytes: b"long".to_vec(),
        },
    );

    let generated = generate_generic_lock(&context).unwrap();
    let assets = generated.lock.value()["assets"].as_array().unwrap();
    assert_eq!(assets[0]["source"], json!(["a", "b"]));
    assert_eq!(assets[1]["source"], json!(["a"]));
}

#[test]
fn external_state_null_is_valid_and_missing_or_mismatched_pins_fail() {
    generate_generic_lock(&external_context(false)).unwrap();

    let mut missing = external_context(true);
    missing.dependencies.remove(&DependencyIdentity {
        owner: Some(vec!["fx".into()]),
        role: vec!["processor".into(), "descriptor".into()],
    });
    assert_eq!(
        generate_generic_lock(&missing).unwrap_err().code,
        "E_REFERENCE"
    );

    let mut tampered = external_context(true);
    *tampered
        .dependencies
        .get_mut(&DependencyIdentity {
            owner: Some(vec!["fx".into()]),
            role: vec!["processor".into(), "implementation".into()],
        })
        .unwrap() = b"tampered".to_vec();
    assert_eq!(
        generate_generic_lock(&tampered).unwrap_err().code,
        "E_DIGEST"
    );

    let mut mismatch = external_context(true);
    mismatch
        .processors
        .get_mut(&vec!["fx".into()])
        .unwrap()
        .processor_type = "fixture.other/1".into();
    assert_eq!(
        generate_generic_lock(&mismatch).unwrap_err().code,
        "E_CAPABILITY"
    );
}

#[test]
fn evidence_is_optional_and_never_changes_render_key() {
    let base = generate_generic_lock(&minimal_context()).unwrap();
    let mut context = minimal_context();
    let pcm = vec![0u8; 16];
    context.evidence = Some(OutputEvidence {
        pcm: pcm.clone(),
        file: Some(pcm),
    });
    let with_evidence = generate_generic_lock(&context).unwrap();

    assert_eq!(base.lock.render_key(), with_evidence.lock.render_key());
    assert_ne!(base.lock_bytes, with_evidence.lock_bytes);
    assert!(with_evidence.lock.value()["evidence"].is_object());
}

#[test]
fn block_schedule_and_core_config_fail_explicitly_when_invalid() {
    let mut context = minimal_context();
    context.engine.block_independent = false;
    assert_eq!(
        generate_generic_lock(&context).unwrap_err().code,
        "E_CAPABILITY"
    );

    let mut context = minimal_context();
    context
        .processors
        .get_mut(&vec!["sine".into()])
        .unwrap()
        .normalized_config = json!({"t":"string","v":"not-a-record"});
    assert_eq!(
        generate_generic_lock(&context).unwrap_err().code,
        "E_SCHEMA"
    );
}

#[test]
fn config_digest_is_exact_section20_canonical_json_hash() {
    let generated = generate_generic_lock(&minimal_context()).unwrap();
    let processor = &generated.lock.value()["processors"][0];
    let config_bytes = canonical_json_bytes(&processor["config"]).unwrap();
    assert_eq!(processor["config_digest"], digest(&config_bytes));
}
