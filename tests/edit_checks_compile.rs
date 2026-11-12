//! `BundleEditContext::checking_compile`: an edit that leaves a document
//! which does not compile is refused with the compile's first error. Without
//! it, the same edit applies: these failures show only once the performance
//! is expanded, or are a limit of the renderer.

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::editing::{BundleEditContext, EditError, Operation, SourceDocument, Transaction};
use maac::plan::PlanLimits;
use maac::SourceBundle;
use serde_json::{json, Value};

/// A 16q score at 120 bpm: pattern `bar` holds `children`, placed as `main`
/// on the synth's track with `fields`; `more` is beside them.
fn source(children: &str, fields: &str, more: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 16q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 5ms; release = 50ms; level = 0.05; }}; }}
track t {{ target = &synth:events; }}
pattern bar {{ length = 4q; note a {{ at = 0q; dur = 1q; pitch = C4; }} {children} }}
place main {{ pattern = &bar; track = &t; at = 0q; {fields} }}
{more}
"#
    )
}

/// The code and the message of the first error of compiling `text`.
fn compile(text: &str, limits: &PlanLimits) -> Option<(String, String)> {
    compile_bundle_artifact_with_limits(&SourceBundle::new("main.maac", text), limits)
        .err()
        .map(|diagnostics| {
            let first = &diagnostics.into_vec()[0];
            (first.code_str().to_owned(), first.message.clone())
        })
}

/// The result of `operation` on `text` with the compile check off, then on
/// under `limits`.
// The editing API's own error type; boxing it here would only obscure tests.
#[allow(clippy::result_large_err)]
fn edit(text: &str, operation: &Operation, limits: &PlanLimits) -> [Result<String, EditError>; 2] {
    [false, true].map(|checking| {
        let bundle = SourceBundle::new("main.maac", text);
        let context = BundleEditContext::new(&bundle).unwrap();
        let context = if checking {
            context.checking_compile(*limits)
        } else {
            context
        };
        let mut document = SourceDocument::parse(text).unwrap();
        let transaction =
            Transaction::new(document.revision().to_owned(), vec![operation.clone()]).unwrap();
        document
            .apply(&transaction, &context)
            .map(|_| document.source().to_owned())
    })
}

fn set(object: &[&str], field: &[&str], value: Value) -> Operation {
    Operation::Set {
        object: object.iter().map(|id| (*id).to_owned()).collect(),
        field: field.iter().map(|name| (*name).to_owned()).collect(),
        value,
        expect: None,
        expect_absent: false,
    }
}

fn quantity(numerator: i64, denominator: i64, unit: &str) -> Value {
    json!({"t":"quantity","n":numerator.to_string(),"d":denominator.to_string(),"u":unit})
}

fn number(value: i64) -> Value {
    json!({"t":"number","n":value.to_string(),"d":"1"})
}

/// Each edit applies with the check off and leaves a document that does not
/// compile; with the check on it is refused with that compile's first error.
#[test]
fn an_edit_that_leaves_a_document_which_does_not_compile_is_refused_when_checking() {
    let limits = PlanLimits::default();
    let curve = "curve c { clock = score; points = [(0q, 0.01, step), (4q, 0.02, step)]; }\nautomation one { target = &synth.params.level; curve = &c; at = 0q; }\nautomation two { target = &synth.params.level; curve = &c; at = 8q; }";
    let cases = [
        // An offset that moves the first onset before the score.
        (
            source("", "", ""),
            set(&["bar", "a"], &["onset_offset"], quantity(-10, 1, "ms")),
            "E_INTERVAL",
        ),
        // A transposition that takes a pitch past what the rate renders.
        (
            source("note high { at = 2q; dur = 1q; pitch = C10; }", "", ""),
            set(&["main"], &["transpose"], quantity(2400, 1, "ct")),
            "E_RANGE",
        ),
        // A release that leaves no gate.
        (
            source("", "", ""),
            set(&["bar", "a"], &["release_offset"], quantity(-1, 1, "s")),
            "E_INTERVAL",
        ),
        // Two automations of one parameter moved onto each other.
        (
            source("", "", curve),
            set(&["two"], &["at"], quantity(0, 1, "q")),
            "E_AUTOMATION_WRITER",
        ),
        // A phrase's pressure over a note on a synth that takes none.
        (
            source(
                "phrase ph { at = 0q; length = 4q; dynamics = &arc; }",
                "",
                "curve arc { clock = score; points = [(0q, 1/2, linear), (4q, 1, step)]; }",
            ),
            set(
                &["bar", "ph"],
                &["pressure"],
                json!({"t":"ref","path":["arc"],"port":null}),
            ),
            "E_CAPABILITY",
        ),
        // A rate the reference profile does not render.
        (
            source("", "", ""),
            set(&["p"], &["rate"], quantity(44100, 1, "Hz")),
            "E_CAPABILITY",
        ),
    ];
    for (text, operation, code) in cases {
        assert_eq!(compile(&text, &limits), None, "{operation:?}");
        let [unchecked, checked] = edit(&text, &operation, &limits);
        let left = unchecked.unwrap_or_else(|error| panic!("{operation:?}: {error:?}"));
        let (compile_code, compile_message) = compile(&left, &limits)
            .unwrap_or_else(|| panic!("{operation:?} leaves a document that compiles"));
        assert_eq!(compile_code, code, "{operation:?}");
        let refused = checked.expect_err("the check refuses the edit");
        assert_eq!(
            (refused.code, refused.message),
            (compile_code, compile_message),
            "{operation:?}"
        );
    }
}

#[test]
fn an_edit_that_leaves_a_document_which_compiles_applies_when_checking() {
    let limits = PlanLimits::default();
    let text = source("", "", "");
    for operation in [
        set(&["bar", "a"], &["onset_offset"], quantity(10, 1, "ms")),
        set(&["main"], &["transpose"], quantity(200, 1, "ct")),
        set(&["main"], &["count"], number(4)),
    ] {
        let [unchecked, checked] = edit(&text, &operation, &limits);
        let left = checked.unwrap_or_else(|error| panic!("{operation:?}: {error:?}"));
        assert_eq!(compile(&left, &limits), None, "{operation:?}");
        assert_eq!(unchecked.unwrap(), left, "{operation:?}");
    }
}

/// The check judges the document an edit leaves, not the one it starts
/// from: a document that does not compile can be mended.
#[test]
fn an_edit_mends_a_document_that_does_not_compile() {
    let limits = PlanLimits::default();
    let broken = source("", "", "").replace("pitch = C4; }", "pitch = C4; onset_offset = -10ms; }");
    assert_eq!(
        compile(&broken, &limits).map(|(code, _)| code),
        Some("E_INTERVAL".to_owned())
    );
    let [_, mended] = edit(
        &broken,
        &set(&["bar", "a"], &["onset_offset"], quantity(0, 1, "ms")),
        &limits,
    );
    assert_eq!(compile(&mended.unwrap(), &limits), None);
    // An edit that leaves it broken is refused.
    let [unchecked, checked] = edit(
        &broken,
        &set(&["main"], &["label"], json!({"t":"string","v":"x"})),
        &limits,
    );
    assert!(unchecked.is_ok());
    assert_eq!(checked.unwrap_err().code, "E_INTERVAL");
}

/// The limits are the host's: the same edit applies under an allowance that
/// holds what it leaves, and is refused under one that does not.
#[test]
fn the_check_compiles_under_the_limits_it_is_given() {
    let text = source("", "", "");
    let operation = set(&["main"], &["count"], number(4));
    let tight = PlanLimits {
        max_events: 2,
        ..PlanLimits::default()
    };
    assert_eq!(compile(&text, &tight), None);
    let [_, checked] = edit(&text, &operation, &tight);
    assert_eq!(checked.unwrap_err().code, "E_RESOURCE_LIMIT");
    let [_, checked] = edit(&text, &operation, &PlanLimits::default());
    assert!(checked.is_ok());
}
