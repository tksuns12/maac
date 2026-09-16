use std::fs;

use maac::editing::{
    apply_transaction, AuthoredDocument, EditContext, FoundationEditContext, Operation, Transaction,
};
use serde_json::{json, Value};

fn fixture(path: &str) -> Vec<u8> {
    fs::read(format!("conformance/l1/{path}")).unwrap()
}

fn manifest() -> Value {
    serde_json::from_slice(&fixture("manifest.json")).unwrap()
}

fn authored(path: &str) -> AuthoredDocument {
    AuthoredDocument::from_json(&fixture(path)).unwrap()
}

fn canonical(value: &Value) -> Vec<u8> {
    maac::production_identity::canonical_json_bytes(value).unwrap()
}

#[test]
fn foundation_context_matches_all_l1_normalization_vectors() {
    let context = FoundationEditContext;
    let manifest = manifest();
    let mut members = 0usize;
    for vector in manifest["normalization_vectors"].as_array().unwrap() {
        for member in vector["members"].as_array().unwrap() {
            let source = member["authored"].as_str().unwrap();
            let expected = member["expected_normalized"].as_str().unwrap();
            let document = authored(source);
            let normalized = context.normalize_document(&document).unwrap();
            assert_eq!(canonical(&normalized), fixture(expected), "{source}");
            members += 1;
        }
    }
    assert_eq!(members, 10);
}

#[test]
fn foundation_context_matches_all_l1_position_resolution_vectors() {
    let context = FoundationEditContext;
    let manifest = manifest();
    let vectors = manifest["position_resolution_vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 2);
    for vector in vectors {
        let source = vector["authored"].as_str().unwrap();
        let expected = vector["expected_normalized"].as_str().unwrap();
        let document = authored(source);
        let normalized = context.normalize_document(&document).unwrap();
        assert_eq!(canonical(&normalized), fixture(expected), "{source}");
    }
}

#[test]
fn foundation_context_executes_all_l1_protocol2_patch_cases() {
    let context = FoundationEditContext;
    let manifest = manifest();
    let cases = manifest["patch_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 20);

    for case in cases {
        let id = case["id"].as_str().unwrap();
        let current_path = case["current"].as_str().unwrap();
        let mut current = authored(current_path);
        let original = current.clone();
        let request = fixture(case["request"].as_str().unwrap());
        let expected_error = case["error"].as_str();

        let transaction = match Transaction::from_json(&request) {
            Ok(transaction) => transaction,
            Err(error) => {
                assert_eq!(case["status"], "failure", "{id}");
                assert_eq!(Some(error.code.as_str()), expected_error, "{id}");
                assert_eq!(current, original, "{id}");
                continue;
            }
        };

        if case["status"] == "success" {
            let applied = apply_transaction(&mut current, &transaction, &context)
                .unwrap_or_else(|error| panic!("{id}: {error}"));
            let expected_after = authored(case["expected_after"].as_str().unwrap());
            assert_eq!(current, expected_after, "{id}");
            assert_eq!(
                applied.new_revision,
                case["expected_new_revision"].as_str().unwrap(),
                "{id}"
            );

            let inverse = Transaction::from_json(&applied.inverse.to_json().unwrap()).unwrap();
            apply_transaction(&mut current, &inverse, &context)
                .unwrap_or_else(|error| panic!("{id} inverse: {error}"));
            let expected_restored = authored(case["expected_restored"].as_str().unwrap());
            assert_eq!(current, expected_restored, "{id} inverse");
        } else {
            let error = apply_transaction(&mut current, &transaction, &context)
                .expect_err("failure case unexpectedly committed");
            assert_eq!(Some(error.code.as_str()), expected_error, "{id}: {error}");
            assert_eq!(current, original, "{id}");
        }
    }
}

fn occurrence_source() -> AuthoredDocument {
    let source = r#"maac 1;
project p { score=[0q,2q]; rate=48000Hz; tempo=&clock; meter=&metre; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
node synth { type="core.sine/1"; }
pattern riff {
  length=1q;
  note n { at=0q; dur=1/2q; pitch=C4; }
}
track t { target=&synth:events; }
place pl {
  pattern=&riff; track=&t; at=0q;
  override edit { event="0/n"; set={pitch=D4;}; }
}
"#;
    let document = maac::parse(source).unwrap();
    AuthoredDocument::from_document(&document).unwrap()
}

#[test]
fn foundation_context_rewrites_occurrence_addresses_on_source_id_rename() {
    let context = FoundationEditContext;
    let mut document = occurrence_source();
    let original = document.clone();
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::RenameId {
            object: vec!["riff".into(), "n".into()],
            new_id: "lead".into(),
        }],
    )
    .unwrap();
    let applied = apply_transaction(&mut document, &transaction, &context).unwrap();
    assert_eq!(
        document.tree()["objects"]["pl"]["children"]["edit"]["fields"]["event"]["v"],
        "0/lead"
    );
    let inverse = Transaction::from_json(&applied.inverse.to_json().unwrap()).unwrap();
    apply_transaction(&mut document, &inverse, &context).unwrap();
    assert_eq!(document, original);
}

#[test]
fn foundation_context_rejects_dangling_occurrence_target_atomically() {
    let context = FoundationEditContext;
    let mut document = occurrence_source();
    let original = document.clone();
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["pl".into(), "edit".into()],
            field: vec!["event".into()],
            value: json!({"t":"string","v":"0/missing"}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap();
    let error = apply_transaction(&mut document, &transaction, &context).unwrap_err();
    assert_eq!(error.code, "E_INSTANCE_TARGET");
    assert_eq!(document, original);
}

#[test]
fn foundation_context_refuses_capabilities_without_edit_normalization_contracts() {
    let context = FoundationEditContext;
    let source = r#"maac 1;
project p {
  score=[0q,1q]; rate=48000Hz; tempo=&clock; meter=&metre;
  requires=["maac.production/1"];
}
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
node eq { type="fx.eq/1"; config={channels=1;mode=peak;}; }
"#;
    let document = AuthoredDocument::from_document(&maac::parse(source).unwrap()).unwrap();
    let error = context.validate_document(document.tree()).unwrap_err();
    assert_eq!(error.code, "E_CAPABILITY");
}
fn strip_object_labels(tree: &mut Value) {
    fn visit(objects: &mut serde_json::Map<String, Value>) {
        for object in objects.values_mut() {
            object["fields"].as_object_mut().unwrap().remove("label");
            visit(object["children"].as_object_mut().unwrap());
        }
    }
    visit(tree["objects"].as_object_mut().unwrap());
}

#[test]
fn editing_and_execution_normalizers_share_core_semantics() {
    let source = r#"maac 1;
project p { label="display"; score=[0q,1q]; rate=48000Hz; tempo=&clock; meter=&metre; output=&s:out; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
node s { label="sine"; type="core.sine/1"; }
"#;
    let parsed = maac::parse(source).unwrap();
    let plan = maac::compile(&parsed).unwrap();
    let identity = maac::production_identity::execution_identity(&parsed, &plan).unwrap();
    let authored = AuthoredDocument::from_document(&parsed).unwrap();
    let mut editing = FoundationEditContext.normalize_document(&authored).unwrap();
    strip_object_labels(&mut editing);
    let execution: Value = serde_json::from_str(&identity.normalized_source_json).unwrap();
    assert_eq!(editing, execution);
    assert_eq!(
        canonical(&editing),
        identity.normalized_source_json.as_bytes()
    );
}
#[test]
fn document_editing_validates_messages_without_claiming_performance_execution() {
    let source = r#"maac 1;
project p {score=[0q,1q];rate=48000Hz;tempo=&t;meter=&m;}
tempo t {points=[(0q,120bpm,step)];}
meter m {points=[(0q,4,4)];}
pattern pat {length=1q; message msg {at=0q;protocol="midi1";bytes=[144,60,127];}}
"#;
    let parsed = maac::parse(source).unwrap();
    let authored = AuthoredDocument::from_document(&parsed).unwrap();
    let normalized = FoundationEditContext.normalize_document(&authored).unwrap();
    let message = &normalized["objects"]["pat"]["children"]["msg"]["fields"];
    assert_eq!(
        message["onset_offset"],
        json!({"t":"quantity","n":"0","d":"1","u":"s"})
    );
    assert_eq!(message["order"], json!({"t":"number","n":"0","d":"1"}));

    let compile_error = maac::compile(&parsed).unwrap_err();
    assert!(compile_error
        .iter()
        .any(|diagnostic| diagnostic.code == maac::DiagnosticCode::Capability));
}
