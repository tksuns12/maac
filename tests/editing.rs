//! Runtime kernel tests. FixtureContext is deliberately test-only; these tests
//! do NOT certify a general MaaC semantic normalizer or Document implementation.

use std::fs;

use maac::editing::{
    apply_transaction, AuthoredDocument, EditContext, EditError, EditResult, Operation,
    Transaction, MAX_DOCUMENT_BYTES, MAX_OPERATIONS, REVISION_ALGORITHM,
};
use serde_json::{json, Value};

fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

fn number(n: i64) -> Value {
    json!({"t":"number","n":n.to_string(),"d":"1"})
}

fn seconds(n: i64) -> Value {
    json!({"t":"quantity","n":n.to_string(),"d":"1","u":"s"})
}

fn reference(id: &str) -> Value {
    json!({"t":"ref","path":[id],"port":null})
}

fn string(value: &str) -> Value {
    json!({"t":"string","v":value})
}

fn empty_record() -> Value {
    json!({"t":"record","fields":{}})
}

fn base() -> AuthoredDocument {
    AuthoredDocument::from_json(include_bytes!(
        "../conformance/l1/authored/minimal-tail-omitted.typed.json"
    ))
    .unwrap()
}

fn with_object(id: &str, object: Value) -> AuthoredDocument {
    let mut tree = base().tree().clone();
    tree["objects"][id] = object;
    AuthoredDocument::from_json(&serde_json::to_vec(&tree).unwrap()).unwrap()
}

fn constant() -> Value {
    json!({"kind":"node","fields":{"type":{"t":"string","v":"core.constant/1"}},"children":{}})
}

fn set(object: &[&str], field: &[&str], value: Value, expect: Option<Value>) -> Operation {
    Operation::Set {
        object: path(object),
        field: path(field),
        value,
        expect,
        expect_absent: false,
    }
}

fn transaction(document: &AuthoredDocument, operations: Vec<Operation>) -> Transaction {
    Transaction::new(document.revision().to_owned(), operations).unwrap()
}

fn fixture(path: &str) -> Value {
    serde_json::from_slice(&fs::read(format!("conformance/l1/{path}")).unwrap()).unwrap()
}

struct FixtureContext;

fn normalize_scalar(value: &Value, pitch: bool) -> Value {
    if pitch && value == &json!({"t":"symbol","v":"C4"}) {
        return json!({"t":"call","fn":"key","args":[number(60)]});
    }
    let mut result = value.clone();
    if value["t"] == "quantity" && value["u"] == "ms" {
        let r = maac::parse_rational(&format!(
            "{}/{}",
            value["n"].as_str().unwrap(),
            value["d"].as_str().unwrap()
        ))
        .unwrap()
            / maac::Rational::from_integer(1000.into());
        result["n"] = json!(r.numer().to_string());
        result["d"] = json!(r.denom().to_string());
        result["u"] = json!("s");
    }
    if value["t"] == "record" {
        for (name, field) in result["fields"].as_object_mut().unwrap() {
            *field = normalize_scalar(field, pitch && name == "pitch");
        }
    }
    result
}

fn normalized_object(object: &Value) -> Value {
    let mut result = object.clone();
    for (name, field) in result["fields"].as_object_mut().unwrap() {
        *field = normalize_scalar(field, object["kind"] == "note" && name == "pitch");
    }
    if object["kind"] == "project" {
        let fields = result["fields"].as_object_mut().unwrap();
        fields.entry("tail").or_insert_with(|| seconds(0));
        fields.entry("seed").or_insert_with(|| number(0));
        fields
            .entry("requires")
            .or_insert_with(|| json!({"t":"list","items":[]}));
    }
    if object["kind"] == "node" && object["fields"]["type"]["v"] == "core.constant/1" {
        let fields = result["fields"].as_object_mut().unwrap();
        let params = fields.entry("params").or_insert_with(empty_record);
        params["fields"]
            .as_object_mut()
            .unwrap()
            .entry("value")
            .or_insert_with(|| number(0));
    }
    for child in result["children"].as_object_mut().unwrap().values_mut() {
        *child = normalized_object(child);
    }
    result
}

fn all_references_exist(value: &Value, root: &Value) -> EditResult<()> {
    if value["t"] == "ref" {
        let id = value["path"][0].as_str().unwrap();
        if root["objects"].get(id).is_none() {
            return Err(EditError::new("E_REFERENCE", "dangling fixture reference"));
        }
    }
    match value {
        Value::Object(fields) => {
            for value in fields.values() {
                all_references_exist(value, root)?;
            }
        }
        Value::Array(items) => {
            for value in items {
                all_references_exist(value, root)?;
            }
        }
        _ => {}
    }
    Ok(())
}

impl EditContext for FixtureContext {
    fn validate_document(&self, authored: &Value) -> EditResult<()> {
        let objects = authored["objects"].as_object().unwrap();
        if objects.values().any(|o| o["kind"] == "extension") {
            return Err(EditError::new(
                "E_CAPABILITY",
                "fixture context cannot normalize extensions",
            ));
        }
        let projects: Vec<_> = objects
            .values()
            .filter(|o| o["kind"] == "project")
            .collect();
        if projects.len() != 1 {
            return Err(EditError::new("E_RANGE", "one project required"));
        }
        for field in ["score", "rate", "tempo", "meter"] {
            if projects[0]["fields"].get(field).is_none() {
                return Err(EditError::new("E_RANGE", "required project field missing")
                    .at(&path(&["p"]), &path(&[field])));
            }
        }
        if let Some(tail) = projects[0]["fields"].get("tail") {
            if tail["t"] != "quantity" || !matches!(tail["u"].as_str(), Some("s" | "ms")) {
                return Err(EditError::new(
                    "E_UNIT",
                    "fixture tail needs physical units",
                ));
            }
            if tail["n"].as_str().unwrap().starts_with('-') {
                return Err(EditError::new("E_RANGE", "tail cannot be negative"));
            }
        }
        all_references_exist(authored, authored)
    }

    fn normalize_object(
        &self,
        _base: &Value,
        _base_path: &[String],
        object: &Value,
    ) -> EditResult<Value> {
        Ok(normalized_object(object))
    }

    fn normalize_value(
        &self,
        base: &Value,
        base_path: &[String],
        field: &[String],
        value: &Value,
    ) -> EditResult<Value> {
        let mut object = &base["objects"][&base_path[0]];
        for id in &base_path[1..] {
            object = &object["children"][id];
        }
        Ok(normalize_scalar(
            value,
            object["kind"] == "note" && field == path(&["pitch"]),
        ))
    }

    fn rewrite_structural_references(
        &self,
        _before: &Value,
        candidate: &mut Value,
        _old: &[String],
        _new: &[String],
    ) -> EditResult<()> {
        // The fixture context supports only cases with no occurrence addresses.
        // A production context must resolve and rewrite those addresses.
        fn contains_override(value: &Value) -> bool {
            match value {
                Value::Object(fields) => {
                    fields.get("kind") == Some(&json!("override"))
                        || fields.values().any(contains_override)
                }
                Value::Array(items) => items.iter().any(contains_override),
                _ => false,
            }
        }
        if contains_override(candidate) {
            Err(EditError::new(
                "E_CAPABILITY",
                "fixture context cannot rewrite occurrences",
            ))
        } else {
            Ok(())
        }
    }
}

fn applied_and_undone(document: &mut AuthoredDocument, operations: Vec<Operation>) {
    let original = document.clone();
    let tx = transaction(document, operations);
    let result = apply_transaction(document, &tx, &FixtureContext).unwrap();
    assert_eq!(result.new_revision, document.revision());
    assert_eq!(result.inverse.base_revision, document.revision());
    assert!(!result.inverse.operations.is_empty());
    let inverse = Transaction::from_json(&result.inverse.to_json().unwrap()).unwrap();
    apply_transaction(document, &inverse, &FixtureContext).unwrap();
    assert_eq!(*document, original);
}

fn fails_without_mutation(document: &mut AuthoredDocument, operations: Vec<Operation>, code: &str) {
    let original = document.clone();
    let tx = transaction(document, operations);
    let error = apply_transaction(document, &tx, &FixtureContext).unwrap_err();
    assert_eq!(error.code, code, "{error}");
    assert_eq!(*document, original);
}

#[test]
fn all_l1_authored_snapshots_have_the_recorded_canonical_bytes_and_revisions() {
    let manifest = fixture("manifest.json");
    let mut cases = 0;
    for (name, entry) in manifest["files"].as_object().unwrap() {
        if entry["role"] != "canonical_authored_snapshot" {
            continue;
        }
        let bytes = fs::read(format!("conformance/l1/{name}")).unwrap();
        let document = AuthoredDocument::from_json(&bytes).unwrap();
        assert_eq!(document.canonical_bytes(), bytes, "{name}");
        assert_eq!(
            document.revision(),
            format!("sha256:{}", entry["sha256"].as_str().unwrap()),
            "{name}"
        );
        cases += 1;
    }
    assert_eq!(
        cases, 18,
        "the frozen L1 authored inventory must not silently shrink"
    );
    assert_eq!(REVISION_ALGORITHM, "maac.revision.authored.sha256/1");
}

#[test]
fn canonical_source_hash_excludes_comments_order_and_whitespace() {
    let first = maac::parse("maac 1; region a { span = [0q, 1q]; label = \"A\"; }").unwrap();
    let second =
        maac::parse("maac 1; /* text */ region a { label = \"A\"; span = [0q, 1q]; }").unwrap();
    assert_eq!(
        AuthoredDocument::from_document(&first).unwrap(),
        AuthoredDocument::from_document(&second).unwrap()
    );
}

#[test]
fn authored_distinctions_are_not_normalized_away() {
    for (a, b) in [
        ("minimal-tail-omitted", "minimal-tail-zero"),
        ("minimal-tail-20ms", "minimal-tail-1-50s"),
        ("minimal-label-a", "minimal-label-b"),
        ("pitch-symbol", "pitch-key-60"),
        ("constant-omitted", "constant-empty-records"),
    ] {
        let a = AuthoredDocument::from_json(
            &fs::read(format!("conformance/l1/authored/{a}.typed.json")).unwrap(),
        )
        .unwrap();
        let b = AuthoredDocument::from_json(
            &fs::read(format!("conformance/l1/authored/{b}.typed.json")).unwrap(),
        )
        .unwrap();
        assert_ne!(a.revision(), b.revision());
    }
}

#[test]
fn scalar_reduction_and_negative_zero_are_canonicalized() {
    let mut a = base().tree().clone();
    a["objects"]["p"]["fields"]["tail"] = json!({"t":"quantity","n":"2","d":"4","u":"s"});
    let mut b = a.clone();
    b["objects"]["p"]["fields"]["tail"] = json!({"t":"quantity","n":"1","d":"2","u":"s"});
    assert_eq!(
        AuthoredDocument::from_json(&serde_json::to_vec(&a).unwrap()).unwrap(),
        AuthoredDocument::from_json(&serde_json::to_vec(&b).unwrap()).unwrap()
    );
    a["objects"]["p"]["fields"]["tail"]["n"] = json!("-0");
    let a = AuthoredDocument::from_json(&serde_json::to_vec(&a).unwrap()).unwrap();
    assert_eq!(a.tree()["objects"]["p"]["fields"]["tail"]["n"], "0");
    assert_eq!(a.tree()["objects"]["p"]["fields"]["tail"]["d"], "1");
}

#[test]
fn canonical_output_uses_lowercase_control_escapes_and_no_final_newline() {
    let mut tree = base().tree().clone();
    tree["objects"]["p"]["fields"]["label"] = string("한글/\n\t\u{1f}");
    let document = AuthoredDocument::from_json(&serde_json::to_vec(&tree).unwrap()).unwrap();
    let text = std::str::from_utf8(document.canonical_bytes()).unwrap();
    assert!(text.contains("한글/\\u000a\\u0009\\u001f"));
    assert!(!text.ends_with('\n'));
}

#[test]
fn duplicate_json_keys_are_rejected_at_every_depth() {
    for bytes in [
        br#"{"version":2,"version":2,"base_revision":"bad","operations":[]}"#.as_slice(),
        br#"{"version":2,"operations":[{"op":"set","op":"unset"}]}"#.as_slice(),
    ] {
        assert_eq!(Transaction::from_json(bytes).unwrap_err().code, "E_SYNTAX");
    }
    assert_eq!(
        AuthoredDocument::from_json(br#"{"version":1,"objects":{},"objects":{}}"#)
            .unwrap_err()
            .code,
        "E_SYNTAX"
    );
}

#[test]
fn unsupported_protocol_precedes_revision_and_operation_checks() {
    assert_eq!(
        Transaction::from_json(br#"{"version":1,"base_revision":false,"operations":"wrong"}"#)
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );
}

#[test]
fn closed_wire_shapes_reject_null_guards_false_absence_and_unknown_fields() {
    let good = json!({"op":"set","object":["p"],"field":["tail"],"value":seconds(1)});
    for (key, value) in [
        ("expect", Value::Null),
        ("expect_absent", json!(false)),
        ("unknown", json!(1)),
    ] {
        let mut bad = good.clone();
        bad[key] = value;
        let tx = json!({"version":2,"base_revision":base().revision(),"operations":[bad]});
        assert_eq!(
            Transaction::from_json(&serde_json::to_vec(&tx).unwrap())
                .unwrap_err()
                .code,
            "E_SYNTAX"
        );
    }
    let mut both = good;
    both["expect"] = seconds(0);
    both["expect_absent"] = json!(true);
    let tx = json!({"version":2,"base_revision":base().revision(),"operations":[both]});
    assert_eq!(
        Transaction::from_json(&serde_json::to_vec(&tx).unwrap())
            .unwrap_err()
            .code,
        "E_SYNTAX"
    );
}

#[test]
fn wire_rejects_empty_paths_unknown_tags_and_floats() {
    for operation in [
        json!({"op":"set","object":[],"field":["tail"],"value":seconds(1)}),
        json!({"op":"set","object":["p"],"field":[],"value":seconds(1)}),
        json!({"op":"set","object":["p"],"field":["tail"],"value":{"t":"mystery"}}),
        json!({"op":"set","object":["p"],"field":["tail"],"value":0.5}),
    ] {
        let tx = json!({"version":2,"base_revision":base().revision(),"operations":[operation]});
        assert_eq!(
            Transaction::from_json(&serde_json::to_vec(&tx).unwrap())
                .unwrap_err()
                .code,
            "E_SYNTAX"
        );
    }
}

#[test]
fn oversized_input_rationals_and_nesting_fail_explicitly() {
    assert_eq!(
        AuthoredDocument::from_json(&vec![b' '; MAX_DOCUMENT_BYTES + 1])
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
    let huge = json!({"t":"number","n":"1".repeat(1235),"d":"1"});
    assert_eq!(
        Transaction::new(
            base().revision().into(),
            vec![set(&["p"], &["seed"], huge, None)]
        )
        .unwrap_err()
        .code,
        "E_RESOURCE_LIMIT"
    );
    let nested = format!("{}0{}", "[".repeat(100), "]".repeat(100));
    assert_eq!(
        AuthoredDocument::from_json(nested.as_bytes())
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
    assert_eq!(AuthoredDocument::from_json(br#"{"version":1,"objects":{"p":{"kind":"project","fields":{"label":{"t":"string","v":"\ud800"}},"children":{}}}}"#).unwrap_err().code, "E_SYNTAX");
}

#[test]
fn programmatic_values_receive_the_same_wire_validation() {
    let mut document = base();
    let original = document.clone();
    let mut tx = transaction(&document, vec![set(&["p"], &["tail"], seconds(1), None)]);
    tx.operations.clear();
    assert_eq!(
        apply_transaction(&mut document, &tx, &FixtureContext)
            .unwrap_err()
            .code,
        "E_SYNTAX"
    );
    assert_eq!(document, original);
    let mut value = number(1);
    for _ in 0..100 {
        value = json!({"t":"list","items":[value]});
    }
    assert_eq!(
        Transaction::new(
            document.revision().into(),
            vec![set(&["p"], &["tail"], value, None)]
        )
        .unwrap_err()
        .code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn stale_revision_does_not_mutate() {
    let mut document = base();
    let original = document.clone();
    let mut tx = transaction(&document, vec![set(&["p"], &["tail"], seconds(1), None)]);
    tx.base_revision = format!("sha256:{}", "0".repeat(64));
    assert_eq!(
        apply_transaction(&mut document, &tx, &FixtureContext)
            .unwrap_err()
            .code,
        "E_CONFLICT"
    );
    assert_eq!(document, original);
}

#[test]
fn default_expectation_and_inverse_restore_authored_omission() {
    applied_and_undone(
        &mut base(),
        vec![set(&["p"], &["tail"], seconds(1), Some(seconds(0)))],
    );
}

#[test]
fn expectations_use_the_original_base_not_successive_candidate_values() {
    applied_and_undone(
        &mut base(),
        vec![
            set(&["p"], &["tail"], seconds(1), Some(seconds(0))),
            set(&["p"], &["tail"], seconds(2), Some(seconds(0))),
        ],
    );
    fails_without_mutation(
        &mut base(),
        vec![
            set(&["p"], &["tail"], seconds(1), Some(seconds(0))),
            set(&["p"], &["tail"], seconds(2), Some(seconds(1))),
        ],
        "E_CONFLICT",
    );
}

#[test]
fn missing_intermediate_record_is_not_synthesized() {
    let mut document = with_object("c", constant());
    fails_without_mutation(
        &mut document,
        vec![Operation::Set {
            object: path(&["c"]),
            field: path(&["params", "value"]),
            value: number(1),
            expect: None,
            expect_absent: true,
        }],
        "E_REFERENCE",
    );
    fails_without_mutation(
        &mut document,
        vec![set(
            &["c"],
            &["params", "value"],
            number(1),
            Some(number(0)),
        )],
        "E_REFERENCE",
    );
}

#[test]
fn explicitly_creating_a_record_preserves_base_defaults_and_inverse_omissions() {
    let mut document = with_object("c", constant());
    applied_and_undone(
        &mut document,
        vec![
            set(&["c"], &["params"], empty_record(), None),
            set(&["c"], &["params", "value"], number(1), Some(number(0))),
        ],
    );
}

#[test]
fn an_absent_leaf_cannot_be_unset_and_a_nonrecord_cannot_be_traversed() {
    fails_without_mutation(
        &mut base(),
        vec![Operation::Unset {
            object: path(&["p"]),
            field: path(&["tail"]),
            expect: Some(seconds(0)),
        }],
        "E_REFERENCE",
    );
    fails_without_mutation(
        &mut base(),
        vec![set(&["p"], &["rate", "value"], number(1), None)],
        "E_REFERENCE",
    );
}

#[test]
fn semantic_unit_equality_does_not_rewrite_authored_units() {
    let mut document = AuthoredDocument::from_json(include_bytes!(
        "../conformance/l1/authored/minimal-tail-20ms.typed.json"
    ))
    .unwrap();
    let original = document.clone();
    let tx = transaction(
        &document,
        vec![set(
            &["p"],
            &["tail"],
            seconds(1),
            Some(json!({"t":"quantity","n":"1","d":"50","u":"s"})),
        )],
    );
    let result = apply_transaction(&mut document, &tx, &FixtureContext).unwrap();
    apply_transaction(&mut document, &result.inverse, &FixtureContext).unwrap();
    assert_eq!(document, original);
    assert_eq!(document.tree()["objects"]["p"]["fields"]["tail"]["u"], "ms");
}

#[test]
fn a_rename_preserves_base_correspondence_but_not_later_target_paths() {
    let mut document = base();
    let old_points = document.tree()["objects"]["clock"]["fields"]["points"].clone();
    applied_and_undone(
        &mut document,
        vec![
            Operation::RenameId {
                object: path(&["clock"]),
                new_id: "renamed".into(),
            },
            set(
                &["renamed"],
                &["points"],
                old_points.clone(),
                Some(old_points.clone()),
            ),
        ],
    );
    fails_without_mutation(
        &mut document,
        vec![
            Operation::RenameId {
                object: path(&["clock"]),
                new_id: "renamed".into(),
            },
            set(&["clock"], &["points"], old_points, None),
        ],
        "E_REFERENCE",
    );
}

#[test]
fn a_rename_does_not_rewrite_remaining_expectation_payloads() {
    fails_without_mutation(
        &mut base(),
        vec![
            Operation::RenameId {
                object: path(&["clock"]),
                new_id: "renamed".into(),
            },
            set(
                &["p"],
                &["tempo"],
                reference("renamed"),
                Some(reference("renamed")),
            ),
        ],
        "E_CONFLICT",
    );
    applied_and_undone(
        &mut base(),
        vec![
            Operation::RenameId {
                object: path(&["clock"]),
                new_id: "renamed".into(),
            },
            set(
                &["p"],
                &["tempo"],
                reference("renamed"),
                Some(reference("clock")),
            ),
        ],
    );
}

#[test]
fn deleting_and_reinserting_the_same_id_breaks_base_correspondence() {
    let mut document = base();
    let clock = document.tree()["objects"]["clock"].clone();
    let points = clock["fields"]["points"].clone();
    fails_without_mutation(
        &mut document,
        vec![
            Operation::DeleteObject {
                object: path(&["clock"]),
                expect_object: None,
            },
            Operation::InsertObject {
                parent: vec![],
                id: "clock".into(),
                object_value: clock,
            },
            set(&["clock"], &["points"], points.clone(), Some(points)),
        ],
        "E_CONFLICT",
    );
}

#[test]
fn inserted_objects_cannot_acquire_a_default_or_absence_precondition() {
    for absent in [false, true] {
        fails_without_mutation(
            &mut base(),
            vec![
                Operation::InsertObject {
                    parent: vec![],
                    id: "c".into(),
                    object_value: constant(),
                },
                Operation::Set {
                    object: path(&["c"]),
                    field: path(&["params"]),
                    value: empty_record(),
                    expect: if absent { None } else { Some(empty_record()) },
                    expect_absent: absent,
                },
            ],
            "E_CONFLICT",
        );
    }
}

#[test]
fn delete_object_expectations_retain_labels_and_compare_normalized_defaults() {
    let mut object = constant();
    object["fields"]["label"] = string("Original");
    let mut document = with_object("c", object.clone());
    let mut wrong = normalized_object(&object);
    wrong["fields"]["label"] = string("Changed");
    fails_without_mutation(
        &mut document,
        vec![Operation::DeleteObject {
            object: path(&["c"]),
            expect_object: Some(wrong),
        }],
        "E_CONFLICT",
    );
    applied_and_undone(
        &mut document,
        vec![Operation::DeleteObject {
            object: path(&["c"]),
            expect_object: Some(normalized_object(&object)),
        }],
    );
}

#[test]
fn final_semantic_failure_rolls_back_all_preceding_operations() {
    fails_without_mutation(
        &mut base(),
        vec![
            set(&["p"], &["tail"], seconds(1), None),
            set(&["p"], &["tail"], seconds(-1), None),
        ],
        "E_RANGE",
    );
    fails_without_mutation(
        &mut base(),
        vec![Operation::Unset {
            object: path(&["p"]),
            field: path(&["tempo"]),
            expect: None,
        }],
        "E_RANGE",
    );
    fails_without_mutation(
        &mut base(),
        vec![Operation::DeleteObject {
            object: path(&["clock"]),
            expect_object: None,
        }],
        "E_REFERENCE",
    );
}

#[test]
fn temporary_dangling_references_are_allowed_until_final_validation() {
    let mut document = base();
    let clock = document.tree()["objects"]["clock"].clone();
    applied_and_undone(
        &mut document,
        vec![
            Operation::DeleteObject {
                object: path(&["clock"]),
                expect_object: None,
            },
            Operation::InsertObject {
                parent: vec![],
                id: "clock".into(),
                object_value: clock,
            },
        ],
    );
}

#[test]
fn rename_rewrites_tagged_references_but_not_labels_or_strings() {
    let mut object = constant();
    object["fields"]["label"] = string("clock");
    object["fields"]["params"] = json!({"t":"record","fields":{"refs":{"t":"list","items":[reference("clock"),{"t":"tuple","items":[reference("clock")]}]}}});
    let mut document = with_object("c", object);
    let tx = transaction(
        &document,
        vec![Operation::RenameId {
            object: path(&["clock"]),
            new_id: "renamed".into(),
        }],
    );
    let result = apply_transaction(&mut document, &tx, &FixtureContext).unwrap();
    assert_eq!(
        document.tree()["objects"]["p"]["fields"]["tempo"]["path"][0],
        "renamed"
    );
    assert_eq!(
        document.tree()["objects"]["c"]["fields"]["params"]["fields"]["refs"]["items"][1]["items"]
            [0]["path"][0],
        "renamed"
    );
    assert_eq!(
        document.tree()["objects"]["c"]["fields"]["label"]["v"],
        "clock"
    );
    assert!(result.impact.full_render_invalidated);
    assert_eq!(result.impact.renamed_identities.len(), 1);
    assert_eq!(result.diagnostics[0].code, "W_IDENTITY_CHANGE");
}

#[test]
fn unsupported_structural_rewrite_fails_instead_of_guessing() {
    let mut object = constant();
    object["children"]["edit"] =
        json!({"kind":"override","fields":{"event":string("0/n")},"children":{}});
    fails_without_mutation(
        &mut with_object("c", object),
        vec![Operation::RenameId {
            object: path(&["clock"]),
            new_id: "renamed".into(),
        }],
        "E_CAPABILITY",
    );
}

#[test]
fn duplicate_ids_and_field_child_collisions_fail_without_mutation() {
    fails_without_mutation(
        &mut base(),
        vec![Operation::RenameId {
            object: path(&["clock"]),
            new_id: "metre".into(),
        }],
        "E_DUPLICATE_ID",
    );
    fails_without_mutation(
        &mut base(),
        vec![Operation::InsertObject {
            parent: vec![],
            id: "p".into(),
            object_value: constant(),
        }],
        "E_DUPLICATE_ID",
    );
    fails_without_mutation(
        &mut base(),
        vec![Operation::InsertObject {
            parent: path(&["p"]),
            id: "tempo".into(),
            object_value: constant(),
        }],
        "E_DUPLICATE_FIELD",
    );
}

#[test]
fn identity_operations_still_return_a_nonempty_usable_inverse() {
    let mut document = base();
    let value = document.tree()["objects"]["p"]["fields"]["rate"].clone();
    applied_and_undone(&mut document, vec![set(&["p"], &["rate"], value, None)]);
}

#[test]
fn inverse_operation_budget_is_checked_before_the_forward_commit() {
    let mut tree = base().tree().clone();
    for i in 0..MAX_OPERATIONS / 2 + 1 {
        let mut object = constant();
        object["fields"]["ref"] = reference("clock");
        tree["objects"][format!("c{i}")] = object;
    }
    let mut document = AuthoredDocument::from_json(&serde_json::to_vec(&tree).unwrap()).unwrap();
    fails_without_mutation(
        &mut document,
        vec![Operation::RenameId {
            object: path(&["clock"]),
            new_id: "renamed".into(),
        }],
        "E_RESOURCE_LIMIT",
    );
}

#[test]
fn hashing_unknown_extensions_does_not_claim_to_normalize_them() {
    let mut document = with_object("ext", json!({"kind":"extension","fields":{},"children":{}}));
    assert!(document.revision().starts_with("sha256:"));
    fails_without_mutation(
        &mut document,
        vec![set(&["p"], &["tail"], seconds(1), None)],
        "E_CAPABILITY",
    );
}

#[test]
fn nested_identities_survive_parent_renames_and_preserve_pitch_spelling_on_undo() {
    let note = json!({"kind":"note","fields":{"pitch":{"t":"symbol","v":"C4"}},"children":{}});
    let riff = json!({"kind":"pattern","fields":{},"children":{"n":note}});
    let mut document = with_object("riff", riff);
    let key = json!({"t":"call","fn":"key","args":[number(60)]});
    applied_and_undone(
        &mut document,
        vec![
            Operation::RenameId {
                object: path(&["riff"]),
                new_id: "other".into(),
            },
            set(&["other", "n"], &["pitch"], key.clone(), Some(key)),
        ],
    );
}

#[test]
fn deleting_a_parent_breaks_correspondence_for_every_descendant() {
    let note = json!({"kind":"note","fields":{"pitch":{"t":"symbol","v":"C4"}},"children":{}});
    let riff = json!({"kind":"pattern","fields":{},"children":{"n":note}});
    let mut document = with_object("riff", riff.clone());
    fails_without_mutation(
        &mut document,
        vec![
            Operation::DeleteObject {
                object: path(&["riff"]),
                expect_object: None,
            },
            Operation::InsertObject {
                parent: vec![],
                id: "riff".into(),
                object_value: riff,
            },
            Operation::Set {
                object: path(&["riff", "n"]),
                field: path(&["label"]),
                value: string("new"),
                expect: None,
                expect_absent: true,
            },
        ],
        "E_CONFLICT",
    );
}

#[test]
fn record_replacement_does_not_break_the_enclosing_objects_base_identity() {
    let mut object = constant();
    object["fields"]["params"] = json!({"t":"record","fields":{"value":number(3)}});
    let mut document = with_object("c", object);
    applied_and_undone(
        &mut document,
        vec![
            set(&["c"], &["params"], empty_record(), None),
            set(&["c"], &["params", "value"], number(5), Some(number(3))),
        ],
    );
}

#[test]
fn expect_absent_checks_base_presence_even_after_an_earlier_set() {
    applied_and_undone(
        &mut base(),
        vec![
            set(&["p"], &["tail"], seconds(1), None),
            Operation::Set {
                object: path(&["p"]),
                field: path(&["tail"]),
                value: seconds(2),
                expect: None,
                expect_absent: true,
            },
        ],
    );
}

#[test]
fn payloads_with_object_ids_or_unknown_tag_fields_are_rejected() {
    let mut object = constant();
    object["id"] = json!("injected");
    assert_eq!(
        Transaction::new(
            base().revision().into(),
            vec![Operation::InsertObject {
                parent: vec![],
                id: "c".into(),
                object_value: object
            }]
        )
        .unwrap_err()
        .code,
        "E_SYNTAX"
    );
    let mut value = seconds(0);
    value["extra"] = json!(true);
    assert_eq!(
        Transaction::new(
            base().revision().into(),
            vec![set(&["p"], &["tail"], value, None)]
        )
        .unwrap_err()
        .code,
        "E_SYNTAX"
    );
}

#[test]
fn manually_modified_source_asts_are_bounded_before_json_conversion() {
    let mut document = maac::parse("maac 1; project p { tail = 0s; }").unwrap();
    let value = &mut document
        .objects
        .get_mut("p")
        .unwrap()
        .fields
        .get_mut("tail")
        .unwrap()
        .value;
    for _ in 0..100 {
        *value = maac::Value::new(maac::ValueKind::List(vec![value.clone()]), value.span);
    }
    assert_eq!(
        AuthoredDocument::from_document(&document).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn missing_without_default_and_authored_presence_are_distinct_conflicts() {
    fails_without_mutation(
        &mut base(),
        vec![set(&["p"], &["label"], string("new"), Some(string("old")))],
        "E_CONFLICT",
    );
    let mut tree = base().tree().clone();
    tree["objects"]["p"]["fields"]["tail"] = seconds(0);
    let mut document = AuthoredDocument::from_json(&serde_json::to_vec(&tree).unwrap()).unwrap();
    fails_without_mutation(
        &mut document,
        vec![Operation::Set {
            object: path(&["p"]),
            field: path(&["tail"]),
            value: seconds(1),
            expect: None,
            expect_absent: true,
        }],
        "E_CONFLICT",
    );
}

#[test]
fn invalid_denominators_are_rejected_before_any_mutation() {
    for denominator in ["0", "-1"] {
        let value = json!({"t":"quantity","n":"1","d":denominator,"u":"s"});
        assert_eq!(
            Transaction::new(
                base().revision().into(),
                vec![set(&["p"], &["tail"], value, None)]
            )
            .unwrap_err()
            .code,
            "E_RANGE"
        );
    }
}
