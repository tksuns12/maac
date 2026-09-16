use maac::editing::{FoundationEditContext, Operation, SourceDocument, Transaction};
use serde_json::{json, Value};

fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

fn seconds(n: i64) -> Value {
    json!({"t":"quantity","n":n.to_string(),"d":"1","u":"s"})
}

fn transaction(document: &SourceDocument, operations: Vec<Operation>) -> Transaction {
    Transaction::new(document.revision().to_owned(), operations).unwrap()
}

const MINIMAL: &str = r#"maac 1;
// keep this header comment exactly
project p { score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; /* keep project spacing */ }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
"#;
#[test]
fn set_and_unset_preserve_unrelated_source_text() {
    let mut document = SourceDocument::parse(MINIMAL).unwrap();
    let original_comment = "// keep this header comment exactly";
    let project_comment = "/* keep project spacing */";
    let set = transaction(
        &document,
        vec![Operation::Set {
            object: path(&["p"]),
            field: path(&["tail"]),
            value: seconds(1),
            expect: Some(seconds(0)),
            expect_absent: false,
        }],
    );
    let applied = document.apply(&set, &FoundationEditContext).unwrap();
    assert!(document.source().contains(original_comment));
    assert!(document.source().contains(project_comment));
    assert!(document.source().contains("tail = 1s;"));

    document
        .apply(&applied.inverse, &FoundationEditContext)
        .unwrap();
    assert!(document.source().contains(original_comment));
    // Typed inverses restore authored meaning, not comments/formatting inside a replaced root.
    assert!(!document.source().contains("tail ="));
}
#[test]
fn rename_updates_references_without_reformatting_unrelated_objects() {
    let mut document = SourceDocument::parse(MINIMAL).unwrap();
    let original_meter = "meter metre { points = [(0q, 4, 4)]; }";
    let tx = transaction(
        &document,
        vec![Operation::RenameId {
            object: path(&["clock"]),
            new_id: "groove".into(),
        }],
    );
    document.apply(&tx, &FoundationEditContext).unwrap();
    assert!(document.source().contains("tempo groove"));
    assert!(document.source().contains("tempo = &groove;"));
    assert!(!document.source().contains("tempo = &clock;"));
    assert!(document.source().contains(original_meter));
    assert!(document
        .source()
        .contains("// keep this header comment exactly"));
}

const OCCURRENCE: &str = r#"maac 1;
project p { score=[0q,2q]; rate=48000Hz; tempo=&clock; meter=&metre; output=&sum:out; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
node sine { type="core.sine/1"; }
node sum { type="core.sum/1"; config={channels=1;}; }
connect c { from=&sine:out; to=&sum:in; }
pattern riff { length=1q; note n { at=0q; dur=1q; pitch=C4; } }
track t { target=&sine:events; }
place pl { pattern=&riff; track=&t; at=0q; override edit { event="0/n"; set={pitch=C5;}; } }
"#;
#[test]
fn nested_rename_updates_occurrence_address_text() {
    let mut document = SourceDocument::parse(OCCURRENCE).unwrap();
    let tx = transaction(
        &document,
        vec![Operation::RenameId {
            object: path(&["riff", "n"]),
            new_id: "lead".into(),
        }],
    );
    document.apply(&tx, &FoundationEditContext).unwrap();
    assert!(document.source().contains("note lead"));
    assert!(document.source().contains("event=\"0/lead\""));
    assert!(!document.source().contains("event=\"0/n\""));
    assert!(document.source().contains("set={pitch=C5;};"));
}

#[test]
fn semantic_failure_is_atomic_for_source_and_revision() {
    let mut document = SourceDocument::parse(MINIMAL).unwrap();
    let source = document.source().to_owned();
    let revision = document.revision().to_owned();
    let tx = transaction(
        &document,
        vec![Operation::Set {
            object: path(&["p"]),
            field: path(&["tail"]),
            value: json!({"t":"quantity","n":"-1","d":"1","u":"s"}),
            expect: None,
            expect_absent: true,
        }],
    );
    let error = document.apply(&tx, &FoundationEditContext).unwrap_err();
    assert_eq!(error.code, "E_RANGE");
    assert_eq!(document.source(), source);
    assert_eq!(document.revision(), revision);
}
fn q(n: i64) -> Value {
    json!({"t":"quantity","n":n.to_string(),"d":"1","u":"q"})
}

#[test]
fn insert_and_delete_object_keep_existing_text_byte_stable() {
    let mut document = SourceDocument::parse(MINIMAL).unwrap();
    let before_project = "project p { score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; /* keep project spacing */ }";
    let region = json!({
        "kind":"region",
        "fields":{"span":{"t":"list","items":[q(0),q(1)]}},
        "children":{}
    });
    let insert = transaction(
        &document,
        vec![Operation::InsertObject {
            parent: vec![],
            id: "intro".into(),
            object_value: region,
        }],
    );
    document.apply(&insert, &FoundationEditContext).unwrap();
    assert!(document.source().contains(before_project));
    assert!(document.source().contains("region intro"));

    let delete = transaction(
        &document,
        vec![Operation::DeleteObject {
            object: path(&["intro"]),
            expect_object: None,
        }],
    );
    document.apply(&delete, &FoundationEditContext).unwrap();
    assert!(document.source().contains(before_project));
    assert!(!document.source().contains("region intro"));
}
#[test]
fn nested_record_set_preserves_record_comment_and_neighbor_layout() {
    let source = format!(
        "{MINIMAL}node c {{ type = \"core.constant/1\"; params = {{ /* keep params */ }}; }}\n"
    );
    let mut document = SourceDocument::parse(source).unwrap();
    let tx = transaction(
        &document,
        vec![Operation::Set {
            object: path(&["c"]),
            field: path(&["params", "value"]),
            value: json!({"t":"number","n":"1","d":"2"}),
            expect: Some(json!({"t":"number","n":"0","d":"1"})),
            expect_absent: false,
        }],
    );
    document.apply(&tx, &FoundationEditContext).unwrap();
    assert!(document.source().contains("/* keep params */"));
    assert!(document.source().contains("value = 1/2;"));
    assert!(document.source().contains("type = \"core.constant/1\";"));
}
