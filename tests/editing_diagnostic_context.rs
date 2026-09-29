use maac::editing::{
    EditContext, EditError, EditImpact, EditResult, FoundationEditContext, Operation,
    SourceDocument, Transaction,
};
use maac::Span;
use serde_json::{json, Value};

const SOURCE: &str = r#"maac 1;
// 原本 🎼
project p { score=[0q,1q]; tail=0s; rate=48000Hz; tempo=&t; meter=&m; }
tempo t { points=[(0q,120bpm,step)]; }
meter m { points=[(0q,4,4)]; }
"#;

enum Failure {
    Validation,
    Refinement,
    Rename,
    DeleteExpectation,
    ForeignValidation,
}

struct Context(Failure);

fn synthetic_error(object: &[String]) -> EditError {
    let mut error =
        EditError::new("E_RANGE", "host validation failure").at(object, &["tail".into()]);
    error.span = Some(Span::new(1000, 1005));
    error
}

impl EditContext for Context {
    fn validate_document(&self, authored: &Value) -> EditResult<()> {
        if matches!(self.0, Failure::ForeignValidation) {
            return Err(synthetic_error(&["p".into()]).without_source_mapping());
        }
        if matches!(self.0, Failure::Validation)
            && authored["objects"]["p"]["fields"]["tail"]["n"] == "1"
        {
            return Err(synthetic_error(&["p".into()]));
        }
        FoundationEditContext.validate_document(authored)
    }

    fn normalize_object(
        &self,
        base: &Value,
        base_path: &[String],
        object: &Value,
    ) -> EditResult<Value> {
        if matches!(self.0, Failure::DeleteExpectation) {
            return Err(synthetic_error(base_path));
        }
        FoundationEditContext.normalize_object(base, base_path, object)
    }

    fn normalize_value(
        &self,
        base: &Value,
        base_path: &[String],
        field: &[String],
        value: &Value,
    ) -> EditResult<Value> {
        FoundationEditContext.normalize_value(base, base_path, field, value)
    }

    fn rewrite_structural_references(
        &self,
        before: &Value,
        candidate: &mut Value,
        old_path: &[String],
        new_path: &[String],
    ) -> EditResult<()> {
        if matches!(self.0, Failure::Rename) {
            return Err(synthetic_error(new_path));
        }
        FoundationEditContext.rewrite_structural_references(before, candidate, old_path, new_path)
    }

    fn refine_impact(
        &self,
        _base: &Value,
        _candidate: &Value,
        _impact: &mut EditImpact,
    ) -> EditResult<()> {
        if matches!(self.0, Failure::Refinement) {
            return Err(synthetic_error(&["p".into()]));
        }
        Ok(())
    }
}

fn rename() -> Operation {
    Operation::RenameId {
        object: vec!["p".into()],
        new_id: "renamed".into(),
    }
}

fn assert_refusal(failure: Failure, operations: Vec<Operation>, path: &str, has_span: bool) {
    let mut document = SourceDocument::parse(SOURCE).unwrap();
    let original = document.clone();
    let transaction = Transaction::new(document.revision().into(), operations).unwrap();
    let error = document.apply(&transaction, &Context(failure)).unwrap_err();
    assert_eq!(error.code, "E_RANGE");
    assert_eq!(error.object_path, [path]);
    assert_eq!(error.field_path, ["tail"]);
    let start = SOURCE.find("0s").unwrap();
    assert_eq!(error.span, has_span.then_some(Span::new(start, start + 2)));
    assert_eq!(document.source(), original.source());
    assert_eq!(document.authored(), original.authored());
    assert_eq!(document.revision(), original.revision());
}

fn set_tail() -> Operation {
    Operation::Set {
        object: vec!["p".into()],
        field: vec!["tail".into()],
        value: json!({"t":"quantity","n":"1","d":"1","u":"s"}),
        expect: None,
        expect_absent: false,
    }
}

#[test]
fn context_validation_offset_is_replaced_with_the_original_value_range() {
    assert_refusal(Failure::Validation, vec![set_tail()], "p", true);
}

#[test]
fn unlocated_refinement_failure_cannot_leak_a_context_offset() {
    assert_refusal(Failure::Refinement, vec![set_tail()], "p", false);
}

#[test]
fn host_dependency_diagnostic_can_veto_a_colliding_local_path() {
    assert_refusal(Failure::ForeignValidation, vec![set_tail()], "p", false);
    let error = synthetic_error(&["p".into()])
        .without_source_mapping()
        .at(&["p".into()], &["tail".into()]);
    assert_eq!(error.span, None);
    // Internal provenance does not change public equality or serialized shape.
    let local =
        EditError::new("E_RANGE", "host validation failure").at(&["p".into()], &["tail".into()]);
    assert_eq!(error, local);
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        serde_json::to_value(&local).unwrap()
    );
}

#[test]
fn rename_callback_failure_uses_the_moved_identity() {
    assert_refusal(Failure::Rename, vec![rename()], "renamed", true);
}

#[test]
fn delete_expectation_failure_keeps_its_base_path_after_rename() {
    let document = SourceDocument::parse(SOURCE).unwrap();
    let expectation = document.authored().tree()["objects"]["p"].clone();
    assert_refusal(
        Failure::DeleteExpectation,
        vec![
            rename(),
            Operation::DeleteObject {
                object: vec!["renamed".into()],
                expect_object: Some(expectation),
            },
        ],
        "p",
        true,
    );
}

#[test]
fn failure_before_a_later_rename_uses_only_executed_identity_changes() {
    let mut document = SourceDocument::parse(SOURCE).unwrap();
    let original = document.clone();
    let transaction = Transaction::new(
        document.revision().into(),
        vec![
            Operation::Set {
                object: vec!["p".into()],
                field: vec!["tail".into()],
                value: json!({"t":"quantity","n":"1","d":"1","u":"s"}),
                expect: Some(json!({"t":"quantity","n":"9","d":"1","u":"s"})),
                expect_absent: false,
            },
            rename(),
        ],
    )
    .unwrap();
    let error = document
        .apply(&transaction, &FoundationEditContext)
        .unwrap_err();
    assert_eq!(error.code, "E_CONFLICT");
    assert_eq!(error.object_path, ["p"]);
    assert_eq!(error.field_path, ["tail"]);
    let start = SOURCE.find("0s").unwrap();
    assert_eq!(error.span, Some(Span::new(start, start + 2)));
    assert_eq!(document.source(), original.source());
    assert_eq!(document.authored(), original.authored());
}

#[test]
fn parent_and_child_rename_chain_maps_to_the_original_nested_value() {
    let source =
        format!("{SOURCE}pattern riff {{ length=1q; note n {{ at=0q; dur=1/2q; pitch=C4; }} }}\n");
    let mut document = SourceDocument::parse(&source).unwrap();
    let original = document.clone();
    let transaction = Transaction::new(
        document.revision().into(),
        vec![
            Operation::RenameId {
                object: vec!["riff".into()],
                new_id: "phrase".into(),
            },
            Operation::RenameId {
                object: vec!["phrase".into(), "n".into()],
                new_id: "lead".into(),
            },
            Operation::Set {
                object: vec!["phrase".into(), "lead".into()],
                field: vec!["dur".into()],
                value: json!({"t":"quantity","n":"-1","d":"1","u":"q"}),
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
    assert_eq!(error.object_path, ["phrase", "lead"]);
    assert_eq!(error.field_path, ["dur"]);
    let start = source.find("1/2q").unwrap();
    assert_eq!(error.span, Some(Span::new(start, start + 4)));
    assert_eq!(document.source(), original.source());
    assert_eq!(document.authored(), original.authored());
}
