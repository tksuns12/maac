use std::fs;

use maac::bundle::SourceBundle;
use maac::cli::{execute_artifact, Command};
use maac::diagnostic::Span;
use maac::editing::{
    apply_transaction, AuthoredDocument, BundleEditContext, FoundationEditContext, Operation,
    SourceDocument, Transaction,
};
use serde_json::json;

const TAIL_SOURCE: &str = r#"maac 1;
// 参考音楽 🎼
project p { score=[0q,1q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
"#;

const NODE_SOURCE: &str = r#"maac 1;
// 参考音楽 🎼
project p { score=[0q,1q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
node gain { type="core.gain/1"; config={channels=1;}; }
"#;

fn span_for(source: &str, token: &str) -> Span {
    let start = source.find(token).expect("source token");
    Span::new(start, start + token.len())
}

fn value_span(source: &str, assignment: &str, value: &str) -> Span {
    let start = source.find(assignment).expect("source assignment") + assignment.len();
    assert!(source[start..].starts_with(value));
    Span::new(start, start + value.len())
}

fn seconds(n: i64) -> serde_json::Value {
    json!({"t":"quantity","n":n.to_string(),"d":"1","u":"s"})
}

fn number(n: i64) -> serde_json::Value {
    json!({"t":"number","n":n.to_string(),"d":"1"})
}

fn tx(document: &AuthoredDocument, operations: Vec<Operation>) -> Transaction {
    Transaction::new(document.revision().to_owned(), operations).unwrap()
}

fn invalid_tail_operation() -> Operation {
    Operation::Set {
        object: vec!["p".into()],
        field: vec!["tail".into()],
        value: seconds(-1),
        expect: Some(seconds(0)),
        expect_absent: false,
    }
}

fn auth(source: &str) -> AuthoredDocument {
    AuthoredDocument::from_document(&maac::parse(source).unwrap()).unwrap()
}

#[test]
fn malformed_source_keeps_the_parser_diagnostic_utf8_span() {
    let malformed = "maac 1;\n// 参考音楽 🎼\nproject p { tail = ; }\n";
    let parser_diagnostics = maac::parse(malformed).unwrap_err();
    let parser_diagnostic = parser_diagnostics.first().expect("parser diagnostic");
    let expected_span = parser_diagnostic.span.expect("parser span");
    assert_eq!(expected_span.slice(malformed), ";");
    assert!(expected_span.start > malformed.find('🎼').unwrap());

    let error = SourceDocument::parse(malformed).unwrap_err();
    assert_eq!(error.code, parser_diagnostic.code.as_str());
    assert_eq!(error.message, parser_diagnostic.message);
    assert_eq!(error.object_path, parser_diagnostic.object_path);
    assert_eq!(error.field_path, parser_diagnostic.field_path);
    assert_eq!(error.span, Some(expected_span));
}

#[test]
fn invalid_set_on_existing_project_tail_points_to_original_value_and_is_atomic() {
    let mut document = SourceDocument::parse(TAIL_SOURCE).unwrap();
    let original_source = document.source().to_owned();
    let original_revision = document.revision().to_owned();
    let old_value_span = span_for(TAIL_SOURCE, "0s");
    let transaction = Transaction::new(
        original_revision.clone(),
        vec![Operation::Set {
            object: vec!["p".into()],
            field: vec!["tail".into()],
            value: json!({"t":"quantity","n":"-1","d":"1","u":"s"}),
            expect: Some(json!({"t":"quantity","n":"0","d":"1","u":"s"})),
            expect_absent: false,
        }],
    )
    .unwrap();

    let error = document
        .apply(&transaction, &FoundationEditContext)
        .unwrap_err();
    assert_eq!(error.code, "E_RANGE");
    assert_eq!(error.object_path, ["p"]);
    assert_eq!(error.field_path, ["tail"]);
    assert_eq!(error.span, Some(old_value_span));
    assert_eq!(old_value_span.slice(&original_source), "0s");
    assert_eq!(document.source(), original_source);
    assert_eq!(document.revision(), original_revision);
}

#[test]
fn invalid_nested_record_leaf_points_to_its_existing_value() {
    let mut document = SourceDocument::parse(NODE_SOURCE).unwrap();
    let original_source = document.source().to_owned();
    let original_revision = document.revision().to_owned();
    let old_value_span = value_span(NODE_SOURCE, "channels=", "1");
    let transaction = Transaction::new(
        original_revision.clone(),
        vec![Operation::Set {
            object: vec!["gain".into()],
            field: vec!["config".into(), "channels".into()],
            value: number(0),
            expect: Some(number(1)),
            expect_absent: false,
        }],
    )
    .unwrap();

    let error = document
        .apply(&transaction, &FoundationEditContext)
        .unwrap_err();
    assert_eq!(error.code, "E_RANGE");
    assert_eq!(error.object_path, ["gain"]);
    assert_eq!(error.field_path, ["config", "channels"]);
    assert_eq!(error.span, Some(old_value_span));
    assert_eq!(old_value_span.slice(&original_source), "1");
    assert_eq!(document.source(), original_source);
    assert_eq!(document.revision(), original_revision);
}

#[test]
fn invalid_nested_child_leaf_points_to_its_existing_value() {
    let source = format!(
        "{TAIL_SOURCE}pattern riff {{ length=1q; note n {{ at=0q; dur=1/2q; pitch=C4; }} }}\n"
    );
    let mut document = SourceDocument::parse(source.clone()).unwrap();
    let original_revision = document.revision().to_owned();
    let old_value_span = span_for(&source, "1/2q");
    let transaction = Transaction::new(
        original_revision.clone(),
        vec![Operation::Set {
            object: vec!["riff".into(), "n".into()],
            field: vec!["dur".into()],
            value: json!({"t":"quantity","n":"-1","d":"1","u":"q"}),
            expect: Some(json!({"t":"quantity","n":"1","d":"2","u":"q"})),
            expect_absent: false,
        }],
    )
    .unwrap();

    let error = document
        .apply(&transaction, &FoundationEditContext)
        .unwrap_err();
    assert_eq!(error.code, "E_RANGE");
    assert_eq!(error.object_path, ["riff", "n"]);
    assert_eq!(error.field_path, ["dur"]);
    assert_eq!(error.span, Some(old_value_span));
    assert_eq!(old_value_span.slice(&source), "1/2q");
    assert_eq!(document.source(), source);
    assert_eq!(document.revision(), original_revision);
}

#[test]
fn renamed_identity_keeps_its_original_leaf_span_on_later_validation_failure() {
    let mut document = SourceDocument::parse(NODE_SOURCE).unwrap();
    let original_source = document.source().to_owned();
    let original_revision = document.revision().to_owned();
    let old_value_span = value_span(NODE_SOURCE, "channels=", "1");
    let transaction = Transaction::new(
        original_revision.clone(),
        vec![
            Operation::RenameId {
                object: vec!["gain".into()],
                new_id: "renamed_gain".into(),
            },
            Operation::Set {
                object: vec!["renamed_gain".into()],
                field: vec!["config".into(), "channels".into()],
                value: number(0),
                expect: Some(number(1)),
                expect_absent: false,
            },
        ],
    )
    .unwrap();

    let error = document
        .apply(&transaction, &FoundationEditContext)
        .unwrap_err();
    assert_eq!(error.code, "E_RANGE");
    assert_eq!(error.object_path, ["renamed_gain"]);
    assert_eq!(error.field_path, ["config", "channels"]);
    assert_eq!(error.span, Some(old_value_span));
    assert_eq!(document.source(), original_source);
    assert_eq!(document.revision(), original_revision);
}

#[test]
fn delete_and_reinsert_with_the_same_id_does_not_reuse_a_stale_span() {
    let mut document = SourceDocument::parse(NODE_SOURCE).unwrap();
    let original_source = document.source().to_owned();
    let original_revision = document.revision().to_owned();
    let inserted = auth(
        r#"maac 1;
node gain { type="core.gain/1"; config={channels=1;}; }
"#,
    )
    .tree()["objects"]["gain"]
        .clone();
    let transaction = Transaction::new(
        original_revision.clone(),
        vec![
            Operation::DeleteObject {
                object: vec!["gain".into()],
                expect_object: None,
            },
            Operation::InsertObject {
                parent: vec![],
                id: "gain".into(),
                object_value: inserted,
            },
            Operation::Set {
                object: vec!["gain".into()],
                field: vec!["config".into(), "channels".into()],
                value: number(0),
                expect: None,
                expect_absent: false,
            },
        ],
    )
    .unwrap();

    let error = document
        .apply(&transaction, &FoundationEditContext)
        .unwrap_err();
    assert_eq!(error.code, "E_RANGE");
    assert_eq!(error.object_path, ["gain"]);
    assert_eq!(error.field_path, ["config", "channels"]);
    assert_eq!(error.span, None);
    assert_eq!(document.source(), original_source);
    assert_eq!(document.revision(), original_revision);
}

#[test]
fn absent_unknown_field_has_no_guessed_source_range() {
    let mut document = SourceDocument::parse(TAIL_SOURCE).unwrap();
    let original_source = document.source().to_owned();
    let original_revision = document.revision().to_owned();
    let transaction = Transaction::new(
        original_revision.clone(),
        vec![Operation::Set {
            object: vec!["p".into()],
            field: vec!["mystery".into()],
            value: number(1),
            expect: None,
            expect_absent: true,
        }],
    )
    .unwrap();

    let error = document
        .apply(&transaction, &FoundationEditContext)
        .unwrap_err();
    assert_eq!(error.code, "E_UNKNOWN_FIELD");
    assert_eq!(error.object_path, ["p"]);
    assert_eq!(error.field_path, ["mystery"]);
    assert_eq!(error.span, None);
    assert_eq!(document.source(), original_source);
    assert_eq!(document.revision(), original_revision);
}

#[test]
fn typed_foundation_and_bundle_errors_do_not_claim_regenerated_source_spans() {
    let foundation_document = auth(TAIL_SOURCE);
    let foundation_error = apply_transaction(
        &mut foundation_document.clone(),
        &tx(&foundation_document, vec![invalid_tail_operation()]),
        &FoundationEditContext,
    )
    .unwrap_err();
    assert_eq!(foundation_error.code, "E_RANGE");
    assert_eq!(foundation_error.object_path, ["p"]);
    assert_eq!(foundation_error.field_path, ["tail"]);
    assert_eq!(foundation_error.span, None);

    let bundle = SourceBundle::new("score.maac", TAIL_SOURCE);
    let bundle_context = BundleEditContext::new(&bundle).unwrap();
    let bundle_document = auth(TAIL_SOURCE);
    let bundle_error = apply_transaction(
        &mut bundle_document.clone(),
        &tx(&bundle_document, vec![invalid_tail_operation()]),
        &bundle_context,
    )
    .unwrap_err();
    assert_eq!(bundle_error.code, "E_RANGE");
    assert_eq!(bundle_error.object_path, ["p"]);
    assert_eq!(bundle_error.field_path, ["tail"]);
    assert_eq!(bundle_error.span, None);
}

#[test]
fn patch_error_keeps_code_path_and_span_in_rendered_and_json_forms_atomically() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input.maac");
    let patch = root.path().join("patch.json");
    let output = root.path().join("output.maac");
    fs::write(&input, TAIL_SOURCE).unwrap();
    fs::write(&output, "owned output\n").unwrap();

    let document = SourceDocument::parse(TAIL_SOURCE).unwrap();
    let transaction = tx(document.authored(), vec![invalid_tail_operation()]);
    fs::write(&patch, transaction.to_json().unwrap()).unwrap();
    let expected_span = span_for(TAIL_SOURCE, "0s");

    let error = execute_artifact(&Command::Patch {
        input: input.clone(),
        patch,
        output: output.clone(),
        force: true,
        project_root: None,
    })
    .unwrap_err();
    assert_eq!(error.code, "E_RANGE");
    assert_eq!(error.path.as_deref(), Some("p.tail"));
    assert_eq!(error.span, Some(expected_span));
    assert!(error.to_string().contains(&format!(
        "at p.tail ({}..{})",
        expected_span.start, expected_span.end
    )));
    let wire = serde_json::to_value(&error).unwrap();
    assert_eq!(wire["code"], "E_RANGE");
    assert_eq!(wire["path"], "p.tail");
    assert_eq!(
        wire["span"],
        json!({"start": expected_span.start, "end": expected_span.end})
    );
    assert_eq!(fs::read_to_string(input).unwrap(), TAIL_SOURCE);
    assert_eq!(fs::read_to_string(output).unwrap(), "owned output\n");
}

#[test]
fn library_resolution_diagnostic_does_not_claim_synthesized_source_span() {
    let library = r#"maac 1;
library sounds { version="1"; }
pattern riff { length=1q; note n { at=0q; dur=1/2q; pitch=C4; } }
"#;
    let bundle = SourceBundle::new("sounds.maac", library);
    let context = BundleEditContext::for_source(&bundle, "sounds.maac").unwrap();
    let mut document = SourceDocument::parse(library).unwrap();
    let original_revision = document.revision().to_owned();
    let old_value_span = span_for(library, "1/2q");
    let transaction = Transaction::new(
        original_revision.clone(),
        vec![Operation::Set {
            object: vec!["riff".into(), "n".into()],
            field: vec!["dur".into()],
            value: json!({"t":"quantity","n":"-1","d":"1","u":"q"}),
            expect: Some(json!({"t":"quantity","n":"1","d":"2","u":"q"})),
            expect_absent: false,
        }],
    )
    .unwrap();

    let error = document.apply(&transaction, &context).unwrap_err();
    assert_eq!(error.code, "E_RANGE");
    assert_eq!(error.object_path, ["riff", "n"]);
    assert_eq!(error.field_path, ["dur"]);
    // Bundle resolution reports this from its merged library graph, which does
    // not prove offsets in the supplied source text.
    assert_eq!(error.span, None);
    assert_ne!(error.span, Some(old_value_span));
    assert_eq!(document.source(), library);
    assert_eq!(document.revision(), original_revision);
}

#[test]
fn foreign_library_error_does_not_capture_a_colliding_local_object_path() {
    let library = r#"maac 1;
library p { version=""; }
"#;
    let pin = maac::bundle::sha256_digest(library.as_bytes());
    let source =
        format!("{TAIL_SOURCE}import sounds {{ path=\"sounds.maac\"; hash=\"{pin}\"; }}\n");
    let mut bundle = SourceBundle::new("main.maac", &source);
    bundle.sources.insert("sounds.maac".into(), library.into());
    let context = BundleEditContext::new(&bundle).unwrap();
    let mut document = SourceDocument::parse(&source).unwrap();
    let original = document.clone();
    let transaction = tx(document.authored(), vec![invalid_tail_operation()]);
    let error = document.apply(&transaction, &context).unwrap_err();
    assert_eq!(error.code, "E_VERSION");
    assert_eq!(error.object_path, ["p"]);
    assert!(error.field_path.is_empty());
    assert_eq!(error.span, None);
    assert_eq!(document.source(), original.source());
    assert_eq!(document.authored(), original.authored());
}
