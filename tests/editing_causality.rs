use std::fs;

use maac::cli::{execute_artifact, Command};
use maac::editing::{
    BundleEditContext, EditContext, FoundationEditContext, Operation, SourceDocument, Transaction,
};
use maac::{DiagnosticCode, SourceBundle};
use serde_json::{json, Value};

const SOURCE: &str = r#"maac 1;
project p { score=[0q,1q]; rate=48000Hz; tempo=&t; meter=&m; output=&g:out; }
tempo t { points=[(0q,120bpm,step)]; }
meter m { points=[(0q,4,4)]; }
node s { type="core.sine/1"; }
node g { type="core.gain/1"; config={channels=1;}; }
connect feed { from=&s:out; to=&g:in; }
"#;

fn set(object: &str, field: &[&str], value: Value) -> Operation {
    Operation::Set {
        object: vec![object.into()],
        field: field.iter().map(|part| (*part).into()).collect(),
        value,
        expect: None,
        expect_absent: false,
    }
}

fn output(node: &str) -> Value {
    json!({"t":"ref", "path":[node], "port":"out"})
}

fn self_cycle(document: &SourceDocument) -> Transaction {
    Transaction::new(
        document.revision().to_owned(),
        vec![set("feed", &["from"], output("g"))],
    )
    .unwrap()
}

fn assert_atomic_cycle_refusal(context: &impl EditContext) {
    maac::compile(&maac::parse(SOURCE).unwrap()).expect("the starting graph compiles");
    let mut document = SourceDocument::parse(SOURCE).unwrap();
    assert_eq!(
        document.revision(),
        "sha256:17e067666223a2aecef7189ea18bb482da12dae8b4fffbeb4f34fe4cfbbd48ec"
    );
    let original = document.clone();
    let error = document.apply(&self_cycle(&document), context).unwrap_err();
    assert_eq!(error.code, "E_ALGEBRAIC_LOOP");
    assert_eq!(document.authored(), original.authored());
    assert_eq!(document.revision(), original.revision());
    assert_eq!(document.source(), original.source());
}

#[test]
fn foundation_source_edit_rejects_final_cycle_atomically() {
    assert_atomic_cycle_refusal(&FoundationEditContext);
}

#[test]
fn bundle_source_edit_rejects_final_cycle_atomically() {
    let bundle = SourceBundle::new("score.maac", SOURCE);
    assert_atomic_cycle_refusal(&BundleEditContext::new(&bundle).unwrap());
}

#[test]
fn direct_edit_validation_and_normalization_reject_final_cycles() {
    let cyclic = SOURCE.replace("from=&s:out", "from=&g:out");
    let parsed = maac::parse(&cyclic).unwrap();
    assert!(maac::compile(&parsed)
        .unwrap_err()
        .iter()
        .any(|diagnostic| diagnostic.code == DiagnosticCode::AlgebraicLoop));
    let bundle = SourceBundle::new("score.maac", &cyclic);
    assert!(maac::compiler::check_bundle_artifact(&bundle)
        .unwrap_err()
        .iter()
        .any(|diagnostic| diagnostic.code == DiagnosticCode::AlgebraicLoop));
    let document = SourceDocument::parse(cyclic).unwrap();
    assert_eq!(
        FoundationEditContext
            .validate_document(document.authored().tree())
            .unwrap_err()
            .code,
        "E_ALGEBRAIC_LOOP"
    );
    assert_eq!(
        FoundationEditContext
            .normalize_document(document.authored())
            .unwrap_err()
            .code,
        "E_ALGEBRAIC_LOOP"
    );
    assert_eq!(
        BundleEditContext::new(&bundle)
            .unwrap()
            .validate_document(document.authored().tree())
            .unwrap_err()
            .code,
        "E_ALGEBRAIC_LOOP"
    );
}

#[test]
fn source_edit_accepts_a_temporary_cycle_repaired_before_commit() {
    let source = format!("{SOURCE}node other {{ type=\"core.sine/1\"; }}\n");
    let bundle = SourceBundle::new("score.maac", &source);
    let bundle_context = BundleEditContext::new(&bundle).unwrap();
    fn apply_repaired(source: &str, context: &impl EditContext) -> SourceDocument {
        let mut document = SourceDocument::parse(source).unwrap();
        let transaction = Transaction::new(
            document.revision().to_owned(),
            vec![
                set("feed", &["from"], output("g")),
                set("feed", &["from"], output("other")),
            ],
        )
        .unwrap();
        let applied = document.apply(&transaction, context).unwrap();
        assert_eq!(document.revision(), applied.new_revision);
        assert_ne!(document.revision(), transaction.base_revision);
        assert_eq!(
            document.authored().tree()["objects"]["feed"]["fields"]["from"],
            output("other")
        );
        maac::compile(&maac::parse(document.source()).unwrap()).unwrap();
        document
    }
    let foundation = apply_repaired(&source, &FoundationEditContext);
    let bundled = apply_repaired(&source, &bundle_context);
    assert_eq!(foundation.authored(), bundled.authored());
    assert_eq!(foundation.source(), bundled.source());
}

fn delay_source(frames: u64) -> String {
    format!(
        "{SOURCE}node delay {{ type=\"core.delay/1\"; config={{channels=1;frames={frames};}}; }}\nconnect delayed {{ from=&g:out; to=&delay:in; }}\n"
    )
}

#[test]
fn source_edit_accepts_feedback_through_positive_frame_delay() {
    fn apply_feedback(source: &str, context: &impl EditContext) -> SourceDocument {
        let mut document = SourceDocument::parse(source).unwrap();
        let transaction = Transaction::new(
            document.revision().to_owned(),
            vec![set("feed", &["from"], output("delay"))],
        )
        .unwrap();
        document.apply(&transaction, context).unwrap();
        context
            .validate_document(document.authored().tree())
            .unwrap();
        FoundationEditContext
            .normalize_document(document.authored())
            .unwrap();
        maac::compiler::check_bundle_artifact(&SourceBundle::new("score.maac", document.source()))
            .unwrap();
        document
    }
    for frames in [1, 2] {
        let source = delay_source(frames);
        let bundle = SourceBundle::new("score.maac", &source);
        maac::compiler::check_bundle_artifact(&bundle).unwrap();
        let foundation = apply_feedback(&source, &FoundationEditContext);
        let bundled = apply_feedback(&source, &BundleEditContext::new(&bundle).unwrap());
        assert_eq!(foundation.authored(), bundled.authored());
        assert_eq!(foundation.source(), bundled.source());
    }
}

#[test]
fn delay_does_not_hide_an_unrelated_same_sample_cycle_or_allow_zero_frames() {
    fn assert_refused(source: &str, context: &impl EditContext, zero_delay: bool) {
        let mut document = SourceDocument::parse(source).unwrap();
        let original = document.clone();
        let mut operations = vec![set("feed", &["from"], output("g"))];
        if zero_delay {
            operations = vec![
                set("feed", &["from"], output("delay")),
                set(
                    "delay",
                    &["config", "frames"],
                    json!({"t":"number","n":"0","d":"1"}),
                ),
            ];
        }
        let transaction = Transaction::new(document.revision().to_owned(), operations).unwrap();
        let error = document.apply(&transaction, context).unwrap_err();
        assert_eq!(
            error.code,
            if zero_delay {
                "E_RANGE"
            } else {
                "E_ALGEBRAIC_LOOP"
            }
        );
        assert_eq!(document.authored(), original.authored());
        assert_eq!(document.source(), original.source());
    }
    let source = delay_source(1);
    let bundle = SourceBundle::new("score.maac", &source);
    let context = BundleEditContext::new(&bundle).unwrap();
    for zero_delay in [false, true] {
        assert_refused(&source, &FoundationEditContext, zero_delay);
        assert_refused(&source, &context, zero_delay);
    }
}

#[test]
fn source_edit_rejects_unused_audio_and_zero_amount_modulation_cycles() {
    let audio = SOURCE.replace("output=&g:out", "output=&s:out")
        + "node h { type=\"core.gain/1\"; config={channels=1;}; }\nconnect onward { from=&g:out; to=&h:in; }\n";
    let controls = format!(
        "{SOURCE}node c {{ type=\"core.constant/1\"; }}\nnode d {{ type=\"core.constant/1\"; }}\nmodulate motion {{ from=&c:out; target=&d.params.value; amount=0; }}\nmodulate onward {{ from=&d:out; target=&s.params.level; amount=0; }}\n"
    );
    fn assert_refused(source: &str, operation: Operation, context: &impl EditContext) {
        let mut document = SourceDocument::parse(source).unwrap();
        let original = document.clone();
        let transaction =
            Transaction::new(document.revision().to_owned(), vec![operation]).unwrap();
        let error = document.apply(&transaction, context).unwrap_err();
        assert_eq!(error.code, "E_ALGEBRAIC_LOOP");
        assert_eq!(document.authored(), original.authored());
        assert_eq!(document.source(), original.source());
    }
    for (source, operation) in [
        (audio, set("feed", &["from"], output("h"))),
        (controls.clone(), set("motion", &["from"], output("d"))),
        (
            controls,
            set(
                "onward",
                &["target"],
                json!({"t":"ref","path":["c","params","value"],"port":null}),
            ),
        ),
    ] {
        maac::compiler::check_bundle_artifact(&SourceBundle::new("score.maac", &source)).unwrap();
        let context = BundleEditContext::new(&SourceBundle::new("score.maac", &source)).unwrap();
        assert_refused(&source, operation.clone(), &FoundationEditContext);
        assert_refused(&source, operation, &context);
    }
}

fn assert_cli_refusal(existing_output: bool) {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input.maac");
    let patch = root.path().join("patch.json");
    let output_path = root.path().join("output.maac");
    fs::write(&input, SOURCE).unwrap();
    let document = SourceDocument::parse(SOURCE).unwrap();
    fs::write(&patch, self_cycle(&document).to_json().unwrap()).unwrap();
    if existing_output {
        fs::write(&output_path, "owner output\n").unwrap();
    }
    let error = execute_artifact(&Command::Patch {
        input: input.clone(),
        patch,
        output: output_path.clone(),
        force: existing_output,
        project_root: None,
    })
    .unwrap_err();
    assert_eq!(error.code, "E_ALGEBRAIC_LOOP");
    assert_eq!(fs::read_to_string(input).unwrap(), SOURCE);
    if existing_output {
        assert_eq!(fs::read_to_string(output_path).unwrap(), "owner output\n");
    } else {
        assert!(!output_path.exists());
    }
    assert_eq!(
        fs::read_dir(root.path()).unwrap().count(),
        if existing_output { 3 } else { 2 }
    );
}

#[test]
fn cli_patch_cycle_failure_publishes_no_output() {
    assert_cli_refusal(false);
}

#[test]
fn cli_patch_cycle_failure_preserves_existing_output_with_force() {
    assert_cli_refusal(true);
}
