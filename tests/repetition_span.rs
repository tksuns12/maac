//! The repetitions of a use fit inside its parent pattern, and those of a
//! placement inside the score (§9, §10). Semantic analysis reports each
//! that does not, and an edit that would leave one is refused.

use maac::compiler::compile_bundle_artifact;
use maac::editing::{
    apply_transaction, AuthoredDocument, BundleEditContext, FoundationEditContext, Operation,
    Transaction,
};
use maac::{parse, Diagnostic, DiagnosticCode, SourceBundle};
use serde_json::{json, Value};

/// A 32q score, pattern `bar` of 4q, pattern `song` of 16q holding `uses`,
/// and `places`.
fn source(uses: &str, places: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 32q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 5ms; release = 50ms; level = 0.05; }}; }}
track t {{ target = &synth:events; }}
pattern bar {{ length = 4q; note a {{ at = 0q; dur = 1q; pitch = C4; }} }}
pattern song {{ length = 16q;
{uses}
}}
{places}
"#
    )
}

const ONCE: &str = "place main { pattern = &bar; track = &t; at = 0q; }";

fn diagnostics(text: &str) -> Vec<Diagnostic> {
    match compile_bundle_artifact(&SourceBundle::new("main.maac", text)) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics.into_vec(),
    }
}

/// The object path of each diagnostic of `text`, each of which is `message`
/// with `E_INTERVAL`.
fn overruns(text: &str, message: &str) -> Vec<Vec<String>> {
    diagnostics(text)
        .into_iter()
        .map(|diagnostic| {
            assert_eq!(diagnostic.code, DiagnosticCode::Interval, "{diagnostic:?}");
            assert_eq!(diagnostic.message, message);
            assert!(diagnostic.field_path.is_empty(), "{diagnostic:?}");
            diagnostic.object_path
        })
        .collect()
}

const PAST_SCORE: &str = "placement repetition span must fit in the project score";
const PAST_PARENT: &str = "repeated use does not fit in its parent pattern";

#[test]
fn a_placement_that_ends_with_the_score_fits_and_one_that_ends_later_does_not() {
    let place = |fields: &str| {
        source(
            "",
            &format!("place main {{ pattern = &bar; track = &t; {fields} }}"),
        )
    };
    for fits in [
        "at = 0q; count = 8;",
        "at = 0q; count = 4; stretch = 2;",
        "at = 24q; count = 2;",
        "at = bar(7, 1); count = 2;",
    ] {
        assert_eq!(diagnostics(&place(fits)), [], "{fits}");
    }
    for overruns_score in [
        "at = 0q; count = 9;",
        "at = 0q; stretch = 9;",
        "at = 0q; count = 8; stretch = 33/32;",
        "at = 28q; count = 2;",
        "at = bar(8, 1); count = 2;",
    ] {
        assert_eq!(
            overruns(&place(overruns_score), PAST_SCORE),
            [["main"]],
            "{overruns_score}"
        );
    }
}

#[test]
fn a_use_that_ends_with_its_parent_fits_and_one_that_ends_later_does_not() {
    let used = |fields: &str| {
        source(
            &format!("  use u {{ pattern = &bar; {fields} }}"),
            "place main { pattern = &song; track = &t; at = 0q; }",
        )
    };
    for fits in [
        "at = 0q; count = 4;",
        "at = 0q; count = 2; stretch = 2;",
        "at = 8q; count = 2;",
        "at = 0q; count = 8; stretch = 1/2;",
    ] {
        assert_eq!(diagnostics(&used(fits)), [], "{fits}");
    }
    for overruns_parent in [
        "at = 0q; count = 5;",
        "at = 0q; stretch = 5;",
        "at = 12q; count = 2;",
        "at = 15q;",
    ] {
        assert_eq!(
            overruns(&used(overruns_parent), PAST_PARENT),
            [["song", "u"]],
            "{overruns_parent}"
        );
    }
}

/// One compile reports every overrun, and one in a pattern that nothing
/// places.
#[test]
fn every_overrun_is_reported_at_its_use_or_its_placement() {
    let uses = "  use u { pattern = &bar; at = 0q; count = 5; }\n  use v { pattern = &bar; at = 12q; count = 2; }";
    assert_eq!(
        overruns(&source(uses, ONCE), PAST_PARENT),
        [["song", "u"], ["song", "v"]]
    );
    let places = "place main { pattern = &bar; track = &t; at = 0q; count = 9; }\nplace second { pattern = &bar; track = &t; at = 30q; }";
    assert_eq!(
        overruns(&source("", places), PAST_SCORE),
        [["main"], ["second"]]
    );
}

fn number(value: i64) -> Value {
    json!({"t":"number","n":value.to_string(),"d":"1"})
}

/// The code of the refusal of setting `field` of `object` to `value` in
/// `text`, under the foundation context and under the bundle context.
fn refusals(text: &str, object: &[&str], field: &str, value: Value) -> [Option<String>; 2] {
    assert_eq!(
        diagnostics(text),
        [],
        "the document compiles before the edit"
    );
    [false, true].map(|bundled| {
        let mut document = AuthoredDocument::from_document(&parse(text).unwrap()).unwrap();
        let transaction = Transaction::new(
            document.revision().to_owned(),
            vec![Operation::Set {
                object: object.iter().map(|id| (*id).to_owned()).collect(),
                field: vec![field.into()],
                value: value.clone(),
                expect: None,
                expect_absent: false,
            }],
        )
        .unwrap();
        let error = if bundled {
            let context = BundleEditContext::new(&SourceBundle::new("main.maac", text)).unwrap();
            apply_transaction(&mut document, &transaction, &context).err()
        } else {
            apply_transaction(&mut document, &transaction, &FoundationEditContext).err()
        };
        error.map(|error| error.code)
    })
}

#[test]
fn an_edit_that_leaves_an_overrun_is_refused_in_both_contexts() {
    let text = source(
        "  use u { pattern = &bar; at = 0q; count = 4; }",
        "place main { pattern = &song; track = &t; at = 0q; count = 2; }",
    );
    let refused = [Some("E_INTERVAL".to_owned()), Some("E_INTERVAL".to_owned())];
    assert_eq!(refusals(&text, &["main"], "count", number(3)), refused);
    assert_eq!(refusals(&text, &["song", "u"], "count", number(5)), refused);
    assert_eq!(
        refusals(&text, &["song", "u"], "stretch", number(2)),
        refused
    );
    // A change that still fits applies.
    assert_eq!(
        refusals(&text, &["song", "u"], "count", number(2)),
        [None, None]
    );
}
