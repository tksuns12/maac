use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use maac::external::{
    discover_external_processors, ExternalDeterminismMode, ExternalHostCapabilities,
    ExternalPermissions, ExternalProcessorDescriptor,
};
use maac::generic_lock::{DependencyIdentity, GenericLock};
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

fn sha_uri(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn descriptor_value() -> Value {
    json!({
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
            "native_notes": true, "expression_kinds": ["pitch"], "hit_keys": [],
            "message_protocols": ["midi1"]
        },
        "config": [
            {
                "name": "amount", "value_type": "number", "default": 1,
                "minimum": 0, "maximum": 1, "asset_kind": null
            }
        ],
        "parameters": [
            {
                "id": "gain", "source_name": "gain", "unit": "1",
                "default": 1, "minimum": 0, "maximum": 2, "range_policy": "error"
            }
        ],
        "parameter_rates": [
            {"parameter_id": "gain", "rate": "sample", "interpolation": ["step", "linear"]}
        ],
        "feedthrough": [
            {"output_port": "out", "input_ports": ["in"], "parameters": ["gain"]}
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
    })
}

fn descriptor_bytes(value: &Value) -> Vec<u8> {
    canonical_json_bytes(value).unwrap()
}

fn rebuild_external_lock(descriptor: &[u8]) -> GenericLock {
    let mut lock: Value =
        serde_json::from_slice(&fixture("canonical/external-ramp-lock.json")).unwrap();
    let digest = sha_uri(descriptor);
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
    {
        let processor = &mut lock["processors"].as_array_mut().unwrap()[0];
        processor["descriptor_hash"] = json!(digest.clone());
        processor["config"]["descriptor_hash"] = json!(digest);
        let config_bytes = canonical_json_bytes(&processor["config"]).unwrap();
        processor["config_digest"] = json!(sha_uri(&config_bytes));
    }
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
    GenericLock::from_value(lock).unwrap()
}
fn supplied_dependencies(descriptor: &[u8]) -> BTreeMap<DependencyIdentity, Vec<u8>> {
    let mut supplied = BTreeMap::new();
    let owner = Some(vec!["fx".to_owned()]);
    let files = [
        (
            vec!["auxiliary", "maac.fixture.external/1", "alpha"],
            "payloads/shared-slot.bin",
        ),
        (
            vec!["auxiliary", "maac.fixture.external/1", "beta"],
            "payloads/shared-slot.bin",
        ),
        (
            vec!["imported-source", "maac.fixture.external/1", "alias_a"],
            "payloads/shared-slot.bin",
        ),
        (
            vec!["imported-source", "maac.fixture.external/1", "alias_b"],
            "payloads/shared-slot.bin",
        ),
        (
            vec!["processor", "adapter"],
            "payloads/fixture-external-adapter.bin",
        ),
        (
            vec!["processor", "implementation"],
            "payloads/fixture-external-implementation.bin",
        ),
        (
            vec!["processor", "state"],
            "payloads/fixture-external-state.bin",
        ),
    ];
    for (role, relative) in files {
        supplied.insert(
            DependencyIdentity {
                owner: owner.clone(),
                role: role.into_iter().map(str::to_owned).collect(),
            },
            fixture(relative),
        );
    }
    supplied.insert(
        DependencyIdentity {
            owner,
            role: vec!["processor".into(), "descriptor".into()],
        },
        descriptor.to_vec(),
    );
    supplied
}

#[test]
fn descriptor_parser_is_strict_and_validates_connection_contracts() {
    let value = descriptor_value();
    let descriptor = ExternalProcessorDescriptor::from_json(&descriptor_bytes(&value)).unwrap();
    assert_eq!(descriptor.id, "fixture.external/1");

    let duplicate = br#"{"format":"a","format":"b"}"#;
    assert_eq!(
        ExternalProcessorDescriptor::from_json(duplicate)
            .unwrap_err()
            .code,
        "E_SYNTAX"
    );

    let mut unknown = value.clone();
    unknown["unexpected"] = json!(true);
    assert_eq!(
        ExternalProcessorDescriptor::from_json(&descriptor_bytes(&unknown))
            .unwrap_err()
            .code,
        "E_SCHEMA"
    );

    let mut missing_policy = value.clone();
    missing_policy["ports"][0]["zero_default"] = Value::Null;
    assert_eq!(
        ExternalProcessorDescriptor::from_json(&descriptor_bytes(&missing_policy))
            .unwrap_err()
            .code,
        "E_SCHEMA"
    );

    let mut duplicate_feedthrough = value.clone();
    let entry = duplicate_feedthrough["feedthrough"][0].clone();
    duplicate_feedthrough["feedthrough"]
        .as_array_mut()
        .unwrap()
        .push(entry);
    assert_eq!(
        ExternalProcessorDescriptor::from_json(&descriptor_bytes(&duplicate_feedthrough))
            .unwrap_err()
            .code,
        "E_SCHEMA"
    );

    let mut floating = value;
    floating["parameters"][0]["default"] = json!(0.5);
    assert_eq!(
        ExternalProcessorDescriptor::from_json(&serde_json::to_vec(&floating).unwrap())
            .unwrap_err()
            .code,
        "E_SYNTAX"
    );
}
#[test]
fn discovery_verifies_full_owned_closure_and_state_contract() {
    let descriptor = descriptor_bytes(&descriptor_value());
    let lock = rebuild_external_lock(&descriptor);
    let supplied = supplied_dependencies(&descriptor);

    let discovered = discover_external_processors(&lock, &supplied).unwrap();
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].node, vec!["fx"]);
    assert_eq!(discovered[0].dependencies.closure.len(), 8);
    assert_eq!(discovered[0].dependencies.descriptor, descriptor);

    let mut missing = supplied.clone();
    missing.remove(&DependencyIdentity {
        owner: Some(vec!["fx".into()]),
        role: vec![
            "auxiliary".into(),
            "maac.fixture.external/1".into(),
            "alpha".into(),
        ],
    });
    assert_eq!(
        discover_external_processors(&lock, &missing)
            .unwrap_err()
            .code,
        "E_REFERENCE"
    );
    let mut tampered = supplied.clone();
    *tampered
        .get_mut(&DependencyIdentity {
            owner: Some(vec!["fx".into()]),
            role: vec!["processor".into(), "adapter".into()],
        })
        .unwrap() = b"tampered".to_vec();
    assert_eq!(
        discover_external_processors(&lock, &tampered)
            .unwrap_err()
            .code,
        "E_DIGEST"
    );

    let mut no_state_value = descriptor_value();
    no_state_value["state"]["save_restore"] = json!(false);
    let no_state_descriptor = descriptor_bytes(&no_state_value);
    let no_state_lock = rebuild_external_lock(&no_state_descriptor);
    let no_state_supplied = supplied_dependencies(&no_state_descriptor);
    assert_eq!(
        discover_external_processors(&no_state_lock, &no_state_supplied)
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );
}
#[test]
fn host_authorization_is_explicit_for_abi_adapter_permissions_and_determinism() {
    let descriptor = descriptor_bytes(&descriptor_value());
    let lock = rebuild_external_lock(&descriptor);
    let supplied = supplied_dependencies(&descriptor);
    let discovered = discover_external_processors(&lock, &supplied).unwrap();
    let processor = &discovered[0];

    let host = ExternalHostCapabilities {
        abi_ids: BTreeSet::from(["maac.fixture.native/1".into()]),
        adapter_ids: BTreeSet::from(["maac.fixture.adapter/1".into()]),
        permissions: ExternalPermissions {
            assets: true,
            ..ExternalPermissions::default()
        },
        allow_nondeterministic: false,
    };
    host.authorize(processor).unwrap();

    let denied = ExternalHostCapabilities {
        permissions: ExternalPermissions::default(),
        ..host.clone()
    };
    assert_eq!(
        denied.authorize(processor).unwrap_err().code,
        "E_PERMISSION"
    );
    let wrong_abi = ExternalHostCapabilities {
        abi_ids: BTreeSet::new(),
        ..host.clone()
    };
    assert_eq!(
        wrong_abi.authorize(processor).unwrap_err().code,
        "E_CAPABILITY"
    );

    let mut nondeterministic = processor.clone();
    nondeterministic.descriptor.determinism.mode = ExternalDeterminismMode::Nondeterministic;
    assert_eq!(
        host.authorize(&nondeterministic).unwrap_err().code,
        "E_CAPABILITY"
    );
}
