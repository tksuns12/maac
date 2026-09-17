use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use maac::generic_lock::{
    DependencyIdentity, ExpectedEngine, ExpectedOutput, ExpectedProcessor, GenericLock,
    LockVerificationContext,
};
use serde_json::{json, Value};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/l4")
}
fn read(rel: &str) -> Vec<u8> {
    fs::read(root().join(rel)).unwrap()
}
fn value(rel: &str) -> Value {
    serde_json::from_slice(&read(rel)).unwrap()
}

fn locate_mut<'a>(mut current: &'a mut Value, path: &[Value]) -> (&'a mut Value, Value) {
    for part in &path[..path.len() - 1] {
        current = match part {
            Value::String(k) => &mut current[k],
            Value::Number(n) => &mut current[n.as_u64().unwrap() as usize],
            _ => panic!(),
        };
    }
    (current, path.last().unwrap().clone())
}
fn mutate(mut base: Value, descriptor: &Value) -> Value {
    for op in descriptor["operations"].as_array().unwrap() {
        match op["op"].as_str().unwrap() {
            "replace" | "add" => {
                let (parent, key) = locate_mut(&mut base, op["path"].as_array().unwrap());
                match key {
                    Value::String(k) => parent[&k] = op["value"].clone(),
                    Value::Number(n) => parent[n.as_u64().unwrap() as usize] = op["value"].clone(),
                    _ => panic!(),
                }
            }
            "delete" => {
                let (parent, key) = locate_mut(&mut base, op["path"].as_array().unwrap());
                match key {
                    Value::String(k) => {
                        parent.as_object_mut().unwrap().remove(&k);
                    }
                    Value::Number(n) => {
                        parent
                            .as_array_mut()
                            .unwrap()
                            .remove(n.as_u64().unwrap() as usize);
                    }
                    _ => panic!(),
                }
            }
            "delete_index" => {
                let (parent, key) = locate_mut(&mut base, op["path"].as_array().unwrap());
                parent[key.as_str().unwrap()]
                    .as_array_mut()
                    .unwrap()
                    .remove(op["index"].as_u64().unwrap() as usize);
            }
            "append" => {
                let (parent, key) = locate_mut(&mut base, op["path"].as_array().unwrap());
                parent[key.as_str().unwrap()]
                    .as_array_mut()
                    .unwrap()
                    .push(op["value"].clone());
            }
            "swap" => {
                let (parent, key) = locate_mut(&mut base, op["path"].as_array().unwrap());
                let a = op["indices"][0].as_u64().unwrap() as usize;
                let b = op["indices"][1].as_u64().unwrap() as usize;
                parent[key.as_str().unwrap()]
                    .as_array_mut()
                    .unwrap()
                    .swap(a, b);
            }
            "swap_fields" => {
                let paths = op["paths"].as_array().unwrap();
                let left = paths[0].as_array().unwrap();
                let right = paths[1].as_array().unwrap();
                fn get<'a>(mut v: &'a Value, path: &[Value]) -> &'a Value {
                    for p in path {
                        v = match p {
                            Value::String(k) => &v[k],
                            Value::Number(n) => &v[n.as_u64().unwrap() as usize],
                            _ => panic!(),
                        };
                    }
                    v
                }
                let lv = get(&base, left).clone();
                let rv = get(&base, right).clone();
                let (p, k) = locate_mut(&mut base, left);
                match k {
                    Value::String(x) => p[&x] = rv,
                    Value::Number(n) => p[n.as_u64().unwrap() as usize] = rv,
                    _ => panic!(),
                };
                let (p, k) = locate_mut(&mut base, right);
                match k {
                    Value::String(x) => p[&x] = lv,
                    Value::Number(n) => p[n.as_u64().unwrap() as usize] = lv,
                    _ => panic!(),
                };
            }
            other => panic!("unsupported mutation {other}"),
        }
    }
    base
}

#[test]
fn canonical_locks_validate_and_derive_exact_render_inputs() {
    for (lock_rel, input_rel) in [
        (
            "canonical/minimal-lock.json",
            "canonical/minimal-render-input.json",
        ),
        (
            "canonical/minimal-evidence-pcm-lock.json",
            "canonical/minimal-render-input.json",
        ),
        (
            "canonical/minimal-evidence-file-lock.json",
            "canonical/minimal-render-input.json",
        ),
        (
            "canonical/equal-config-negative-lock.json",
            "canonical/equal-config-negative-render-input.json",
        ),
        (
            "canonical/external-ramp-lock.json",
            "canonical/external-ramp-render-input.json",
        ),
        (
            "canonical/external-transitive-lock.json",
            "canonical/external-transitive-render-input.json",
        ),
    ] {
        let lock =
            GenericLock::from_json(&read(lock_rel)).unwrap_or_else(|e| panic!("{lock_rel}: {e}"));
        assert_eq!(
            lock.render_input_bytes().unwrap(),
            read(input_rel),
            "{lock_rel}"
        );
    }
}

#[test]
fn l4_lock_mutation_vectors_report_the_normative_error_class() {
    for entry in fs::read_dir(root().join("invalid")).unwrap() {
        let path = entry.unwrap().path();
        let raw = fs::read(&path).unwrap();
        let Ok(desc) = serde_json::from_slice::<Value>(&raw) else {
            continue;
        };
        if desc.get("base").and_then(Value::as_str).is_none() {
            continue;
        }
        let base = desc["base"].as_str().unwrap();
        if !base.ends_with("lock.json") {
            continue;
        }
        let candidate = mutate(value(base), &desc);
        let expected = desc["expected"].as_str().unwrap();
        let error = GenericLock::from_value(candidate).unwrap_err();
        assert_eq!(error.code, expected, "{}: {error}", path.display());
    }
}

fn minimal_context() -> LockVerificationContext {
    let lock = value("canonical/minimal-lock.json");
    let engine_bytes = read("contracts/fixture-engine.json");
    let proc = &lock["processors"][0];
    LockVerificationContext {
        execution_preimage: value("preimages/minimal-execution.json"),
        assets: BTreeMap::new(),
        dependencies: BTreeMap::from([(
            DependencyIdentity {
                owner: None,
                role: vec!["engine".into(), "implementation".into()],
            },
            engine_bytes,
        )]),
        processors: BTreeMap::from([(
            vec!["sine".into()],
            ExpectedProcessor {
                processor_type: "core.sine/1".into(),
                implementation_hash: None,
                descriptor_hash: None,
                adapter_id: None,
                state_hash: None,
                config: proc["config"].clone(),
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
            sample_rate: 48000,
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
            render_frames: 24000,
            crop: (0, 4),
            channel_order: vec![0],
        },
        pcm: None,
        file: None,
    }
}

#[test]
fn runtime_verifier_binds_execution_and_actual_dependency_bytes() {
    let lock = GenericLock::from_json(&read("canonical/minimal-lock.json")).unwrap();
    let verified = lock.verify(&minimal_context()).unwrap();
    assert_eq!(verified.render_key, lock.render_key());
    assert_eq!(verified.crop, (0, 4));
    assert_eq!(verified.channels, 1);
    assert!(!verified.pcm_verified);

    let mut wrong = minimal_context();
    wrong.dependencies.values_mut().next().unwrap().push(0);
    assert_eq!(lock.verify(&wrong).unwrap_err().code, "E_CLOSURE");
    let mut wrong = minimal_context();
    wrong.execution_preimage["objects"]["p"]["fields"]["tail"] =
        json!({"t":"quantity","n":"1","d":"1","u":"s"});
    assert_eq!(lock.verify(&wrong).unwrap_err().code, "E_DIGEST");
}

#[test]
fn runtime_verifier_checks_pcm_and_file_evidence_bytes() {
    let pcm = read("evidence/minimal.pcm");
    for (rel, with_file) in [
        ("canonical/minimal-evidence-pcm-lock.json", false),
        ("canonical/minimal-evidence-file-lock.json", true),
    ] {
        let lock = GenericLock::from_json(&read(rel)).unwrap();
        let mut context = minimal_context();
        context.pcm = Some(pcm.clone());
        if with_file {
            context.file = Some(pcm.clone());
        }
        let verified = lock.verify(&context).unwrap();
        assert!(verified.pcm_verified);
        assert_eq!(verified.file_verified, with_file);
        let mut bad = context;
        bad.pcm.as_mut().unwrap()[0] ^= 1;
        assert_eq!(lock.verify(&bad).unwrap_err().code, "E_EVIDENCE");
    }
}

#[test]
fn strict_json_rejects_duplicate_keys_and_floats() {
    assert_eq!(
        GenericLock::from_json(br#"{"format":"maac.generic-lock","format":"maac.generic-lock"}"#)
            .unwrap_err()
            .code,
        "E_SCHEMA"
    );
    let text = String::from_utf8(read("canonical/minimal-lock.json"))
        .unwrap()
        .replace("\"version\":1", "\"version\":1.0");
    assert_eq!(
        GenericLock::from_json(text.as_bytes()).unwrap_err().code,
        "E_SCHEMA"
    );
}

fn external_context() -> LockVerificationContext {
    let lock = value("canonical/external-ramp-lock.json");
    let proc = &lock["processors"][0];
    let assets = BTreeMap::from([
        (
            vec!["asset_a".into()],
            maac::generic_lock::ExpectedAsset {
                kind: "blob".into(),
                bytes: read("payloads/shared-slot.bin"),
            },
        ),
        (
            vec!["asset_b".into()],
            maac::generic_lock::ExpectedAsset {
                kind: "blob".into(),
                bytes: read("payloads/shared-slot.bin"),
            },
        ),
        (
            vec!["descriptor_asset".into()],
            maac::generic_lock::ExpectedAsset {
                kind: "descriptor".into(),
                bytes: read("contracts/fixture-external-descriptor.json"),
            },
        ),
        (
            vec!["impl_asset".into()],
            maac::generic_lock::ExpectedAsset {
                kind: "module".into(),
                bytes: read("payloads/fixture-external-implementation.bin"),
            },
        ),
        (
            vec!["state_asset".into()],
            maac::generic_lock::ExpectedAsset {
                kind: "blob".into(),
                bytes: read("payloads/fixture-external-state.bin"),
            },
        ),
    ]);
    let owner = Some(vec!["fx".into()]);
    let dependencies = BTreeMap::from([
        (
            DependencyIdentity {
                owner: owner.clone(),
                role: vec![
                    "auxiliary".into(),
                    "maac.fixture.external/1".into(),
                    "alpha".into(),
                ],
            },
            read("payloads/shared-slot.bin"),
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
            read("payloads/shared-slot.bin"),
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
            read("payloads/shared-slot.bin"),
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
            read("payloads/shared-slot.bin"),
        ),
        (
            DependencyIdentity {
                owner: owner.clone(),
                role: vec!["processor".into(), "adapter".into()],
            },
            read("payloads/fixture-external-adapter.bin"),
        ),
        (
            DependencyIdentity {
                owner: owner.clone(),
                role: vec!["processor".into(), "descriptor".into()],
            },
            read("contracts/fixture-external-descriptor.json"),
        ),
        (
            DependencyIdentity {
                owner: owner.clone(),
                role: vec!["processor".into(), "implementation".into()],
            },
            read("payloads/fixture-external-implementation.bin"),
        ),
        (
            DependencyIdentity {
                owner,
                role: vec!["processor".into(), "state".into()],
            },
            read("payloads/fixture-external-state.bin"),
        ),
        (
            DependencyIdentity {
                owner: None,
                role: vec!["engine".into(), "implementation".into()],
            },
            read("contracts/fixture-engine.json"),
        ),
    ]);
    LockVerificationContext {
        execution_preimage: value("preimages/ramp-execution.json"),
        assets,
        dependencies,
        processors: BTreeMap::from([(
            vec!["fx".into()],
            ExpectedProcessor {
                processor_type: "fixture.external/1".into(),
                implementation_hash: proc["implementation_hash"].as_str().map(str::to_owned),
                descriptor_hash: proc["descriptor_hash"].as_str().map(str::to_owned),
                adapter_id: proc["adapter_id"].as_str().map(str::to_owned),
                state_hash: proc["state_hash"].as_str().map(str::to_owned),
                config: proc["config"].clone(),
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
            sample_rate: 48000,
            block_schedule: Some(vec![16000, 16302]),
            block_independent: true,
        },
        output: ExpectedOutput {
            port: lock["output"]["port"].clone(),
            score: [
                lock["output"]["score"][0].clone(),
                lock["output"]["score"][1].clone(),
            ],
            tail: lock["output"]["tail"].clone(),
            render_frames: 32302,
            crop: (0, 32302),
            channel_order: vec![1, 0],
        },
        pcm: None,
        file: None,
    }
}

#[test]
fn runtime_verifier_binds_exact_schedule_crop_and_channel_order() {
    let lock = GenericLock::from_json(&read("canonical/external-ramp-lock.json")).unwrap();
    lock.verify(&external_context()).unwrap();

    let mut wrong = external_context();
    wrong.engine.block_schedule = Some(vec![16001, 16301]);
    assert_eq!(lock.verify(&wrong).unwrap_err().code, "E_CLOSURE");

    let mut wrong = external_context();
    wrong.output.crop = (1, 32302);
    assert_eq!(lock.verify(&wrong).unwrap_err().code, "E_TIMING");

    let mut wrong = external_context();
    wrong.output.channel_order = vec![0, 1];
    assert_eq!(lock.verify(&wrong).unwrap_err().code, "E_TIMING");
}

#[test]
fn lock_schema_rejects_orphan_processor_slots_invalid_ids_and_noncanonical_rationals() {
    let mut orphan = value("canonical/external-ramp-lock.json");
    for dependency in orphan["dependencies"].as_array_mut().unwrap() {
        if dependency["role"][0] == "processor" {
            dependency["owner"] = json!(["ghost"]);
        }
    }
    assert_eq!(
        GenericLock::from_value(orphan).unwrap_err().code,
        "E_CLOSURE"
    );

    let mut bad_id = value("canonical/minimal-lock.json");
    bad_id["processors"][0]["node"] = json!(["bad-id"]);
    assert_eq!(
        GenericLock::from_value(bad_id).unwrap_err().code,
        "E_SCHEMA"
    );

    for spelling in ["-0", "00", "+1"] {
        let mut bad = value("canonical/minimal-lock.json");
        bad["output"]["tail"]["n"] = Value::String(spelling.into());
        assert_eq!(GenericLock::from_value(bad).unwrap_err().code, "E_SCHEMA");
    }
}

#[test]
fn runtime_verifier_binds_transitive_dependency_bytes() {
    let lock = GenericLock::from_json(&read("canonical/external-transitive-lock.json")).unwrap();
    let mut context = external_context();
    let beta = DependencyIdentity {
        owner: Some(vec!["fx".into()]),
        role: vec![
            "auxiliary".into(),
            "maac.fixture.external/1".into(),
            "beta".into(),
        ],
    };
    *context.dependencies.get_mut(&beta).unwrap() = read("payloads/changed-slot.bin");
    lock.verify(&context).unwrap();
    assert_eq!(
        lock.verify(&external_context()).unwrap_err().code,
        "E_CLOSURE"
    );
}

#[test]
fn typed_config_values_follow_the_section20_closed_rules() {
    let mut bad = value("canonical/minimal-lock.json");
    bad["processors"][0]["config"]["config"]["fields"]["bad-key"] =
        json!({"t":"number","n":"1","d":"1"});
    assert_eq!(GenericLock::from_value(bad).unwrap_err().code, "E_SCHEMA");

    let mut bad = value("canonical/minimal-lock.json");
    bad["processors"][0]["config"]["config"]["fields"]["x"] =
        json!({"t":"quantity","n":"1","d":"1","u":"bogus"});
    assert_eq!(GenericLock::from_value(bad).unwrap_err().code, "E_SCHEMA");

    let mut bad = value("canonical/minimal-lock.json");
    bad["processors"][0]["config"]["config"]["fields"]["x"] =
        json!({"t":"tuple","items":[{"t":"number","n":"1","d":"1"}]});
    assert_eq!(GenericLock::from_value(bad).unwrap_err().code, "E_SCHEMA");

    let mut bad = value("canonical/minimal-lock.json");
    bad["output"]["port"]["port"] = Value::String("bad-port".into());
    assert_eq!(GenericLock::from_value(bad).unwrap_err().code, "E_SCHEMA");

    let mut huge = value("canonical/minimal-lock.json");
    huge["output"]["tail"]["n"] = Value::String("9".repeat(1235));
    assert_eq!(
        GenericLock::from_value(huge).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn evidence_size_arithmetic_is_bounded() {
    let mut lock = value("canonical/minimal-evidence-pcm-lock.json");
    lock["output"]["render_frames"] = Value::String("18446744073709551615".into());
    lock["output"]["crop"] = json!(["0", "18446744073709551615"]);
    assert_eq!(
        GenericLock::from_value(lock).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
}
