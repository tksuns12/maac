//! Public §19 region vectors: accepted span shapes, rejected spans, and the
//! guarantee that regions never change performance events or rendered audio.

use maac::diagnostic::DiagnosticCode;
use maac::editing::{FoundationEditContext, Operation, SourceDocument, Transaction};
use maac::plan::Region;
use maac::production_identity::execution_identity;
use maac::semantic::validate_source;
use maac::{check, compile, parse, parse_rational, render, Plan};
use serde_json::{json, Value};

/// An audible project with a nonzero score start so both score boundaries
/// can be exercised independently.
fn project(regions: &str) -> String {
    format!(
        r#"maac 1;
project p {{
  score = [4q, 16q];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &sine:out;
  tail = 50ms;
  seed = 1;
  requires = [];
}}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
pattern line {{
  length = 4q;
  note a {{ at = 0q; dur = 2q; pitch = A3; velocity = 0.6; }}
  note b {{ at = 2q; dur = 2q; pitch = E4; velocity = 0.5; }}
}}
track melody {{ target = &sine:events; }}
place first {{ pattern = &line; track = &melody; at = 4q; }}
place second {{ pattern = &line; track = &melody; at = 8q; }}
node sine {{
  type = "core.sine/1";
  config = {{ voices = 4; }};
  params = {{ attack = 5ms; release = 40ms; level = 0.15; }};
}}
{regions}
"#
    )
}

fn q(value: &str) -> maac::Rational {
    parse_rational(value).unwrap()
}

fn accepted(regions: &str) -> Plan {
    let source = project(regions);
    let document = parse(&source).expect("region vector parses");
    validate_source(&document)
        .unwrap_or_else(|errors| panic!("validate_source rejected {regions:?}: {errors:?}"));
    check(&document).unwrap_or_else(|errors| panic!("check rejected {regions:?}: {errors:?}"));
    compile(&document).unwrap()
}

fn semantic_codes(regions: &str) -> Vec<DiagnosticCode> {
    let document = parse(&project(regions)).expect("region vector parses");
    match validate_source(&document) {
        Ok(_) => Vec::new(),
        Err(errors) => errors.into_iter().map(|error| error.code).collect(),
    }
}

fn check_codes(regions: &str) -> Vec<DiagnosticCode> {
    let document = parse(&project(regions)).expect("region vector parses");
    check(&document)
        .expect_err("check must reject the region vector")
        .into_iter()
        .map(|error| error.code)
        .collect()
}

fn rejected(regions: &str, code: DiagnosticCode) {
    let semantic = semantic_codes(regions);
    assert!(
        semantic.contains(&code),
        "validate_source must report {code:?} for {regions:?}, got {semantic:?}"
    );
    let checked = check_codes(regions);
    assert!(
        checked.contains(&code),
        "check must report {code:?} for {regions:?}, got {checked:?}"
    );
}

fn samples(plan: &Plan) -> Vec<f64> {
    let mut out = Vec::new();
    render(plan, |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    out
}

fn region(id: &str, start: &str, end: &str, label: Option<&str>) -> Region {
    Region {
        id: id.into(),
        start_q: q(start),
        end_q: q(end),
        label: label.map(str::to_owned),
    }
}

#[test]
fn accepts_boundary_overlapping_nested_and_identical_spans() {
    let plan = accepted(
        r#"region whole { span = [4q, 16q]; label = "Whole score"; }
region verse { span = [4q, 8q]; label = "Verse"; }
region chorus { span = [6q, 12q]; }
region hook { span = [7q, 15/2q]; label = "Hook"; }
region twin { span = [6q, 12q]; label = "Chorus again"; }
"#,
    );
    let mut regions = plan.regions.clone();
    regions.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(
        regions,
        vec![
            region("chorus", "6", "12", None),
            region("hook", "7", "15/2", Some("Hook")),
            region("twin", "6", "12", Some("Chorus again")),
            region("verse", "4", "8", Some("Verse")),
            region("whole", "4", "16", Some("Whole score")),
        ]
    );
}

#[test]
fn bar_positions_lower_to_exact_quarter_positions() {
    // Bar 1 begins at 0q under 4/4, so bar 2 is 4q and bar(3, 3) is 10q.
    let plan = accepted("region bars { span = [bar(2, 1), bar(3, 3)]; }");
    assert_eq!(plan.regions, vec![region("bars", "4", "10", None)]);
}

#[test]
fn rejects_reversed_and_empty_spans() {
    rejected("region r { span = [8q, 6q]; }", DiagnosticCode::Interval);
    rejected("region r { span = [6q, 6q]; }", DiagnosticCode::Interval);
    rejected(
        "region r { span = [bar(3, 1), bar(2, 1)]; }",
        DiagnosticCode::Interval,
    );
    rejected(
        "region r { span = [bar(2, 1), bar(2, 1)]; }",
        DiagnosticCode::Interval,
    );
}

#[test]
fn rejects_spans_outside_the_project_score() {
    rejected("region r { span = [2q, 8q]; }", DiagnosticCode::Interval);
    rejected("region r { span = [8q, 17q]; }", DiagnosticCode::Interval);
    rejected("region r { span = [0q, 20q]; }", DiagnosticCode::Interval);
    rejected("region r { span = [16q, 20q]; }", DiagnosticCode::Interval);
    rejected(
        "region r { span = [bar(1, 1), bar(2, 1)]; }",
        DiagnosticCode::Interval,
    );
    rejected(
        "region r { span = [bar(4, 1), bar(5, 2)]; }",
        DiagnosticCode::Interval,
    );
}

#[test]
fn rejects_malformed_span_and_label_values() {
    rejected("region r { label = \"No span\"; }", DiagnosticCode::Range);
    rejected("region r { span = [4q]; }", DiagnosticCode::Interval);
    rejected(
        "region r { span = [4q, 6q, 8q]; }",
        DiagnosticCode::Interval,
    );
    rejected(
        "region r { span = [bar(2, 5), bar(3, 1)]; }",
        DiagnosticCode::Range,
    );
    rejected(
        "region r { span = [bar(5/2, 1), bar(3, 1)]; }",
        DiagnosticCode::Range,
    );
    rejected("region r { span = [4s, 6s]; }", DiagnosticCode::Unit);
    rejected("region r { span = [4, 6]; }", DiagnosticCode::Unit);
    rejected("region r { span = 4q; }", DiagnosticCode::Unit);
    rejected("region r { span = (4q, 6q); }", DiagnosticCode::Unit);
    rejected(
        "region r { span = [4q, 6q]; label = 3; }",
        DiagnosticCode::Unit,
    );
    rejected(
        "region r { span = [4q, 6q]; gain = -3dB; }",
        DiagnosticCode::UnknownField,
    );
}

#[test]
fn regions_are_top_level_leaves() {
    rejected(
        "region r { span = [4q, 8q]; region inner { span = [5q, 6q]; } }",
        DiagnosticCode::Capability,
    );
    let nested_in_pattern = project("").replace(
        "  note b {",
        "  region inner { span = [0q, 1q]; }\n  note b {",
    );
    let document = parse(&nested_in_pattern).unwrap();
    assert!(validate_source(&document).is_err());
    assert!(check(&document).is_err());
}

#[test]
fn regions_do_not_change_events_graph_or_audio() {
    let plain = accepted("");
    let annotated = accepted(
        r#"region whole { span = [4q, 16q]; label = "Whole"; }
region chorus { span = [8q, 12q]; label = "Chorus"; }
region hook { span = [9q, 10q]; }
region cross { span = [7q, 9q]; }
"#,
    );
    assert!(plain.regions.is_empty());
    assert_eq!(annotated.regions.len(), 4);
    assert!(!plain.events.is_empty());
    assert_eq!(plain.events, annotated.events);
    assert_eq!(plain.nodes, annotated.nodes);
    assert_eq!(plain.connections, annotated.connections);
    assert_eq!(plain.automation, annotated.automation);
    assert_eq!(plain.output, annotated.output);
    assert_eq!(plain.tempo, annotated.tempo);

    let plain_audio = samples(&plain);
    assert!(plain_audio.iter().any(|sample| *sample != 0.0));
    assert_eq!(plain_audio, samples(&annotated));
}

#[test]
fn region_spans_enter_the_execution_hash_but_labels_do_not() {
    let identity = |regions: &str| {
        let document = parse(&project(regions)).unwrap();
        let plan = compile(&document).unwrap();
        execution_identity(&document, &plan).unwrap()
    };
    let plain = identity("");
    let chorus = identity("region chorus { span = [8q, 12q]; }");
    let labelled = identity("region chorus { span = [8q, 12q]; label = \"Chorus\"; }");
    let relabelled = identity("region chorus { span = [8q, 12q]; label = \"Refrain\"; }");
    let bar_form = identity("region chorus { span = [bar(3, 1), bar(4, 1)]; }");
    let moved = identity("region chorus { span = [8q, 13q]; }");

    assert_ne!(plain.execution_hash, chorus.execution_hash);
    assert_eq!(chorus.execution_hash, labelled.execution_hash);
    assert_eq!(chorus.execution_hash, relabelled.execution_hash);
    assert_eq!(chorus.execution_hash, bar_form.execution_hash);
    assert_ne!(chorus.execution_hash, moved.execution_hash);
    assert_ne!(labelled.source_input_hash, relabelled.source_input_hash);
}

fn quantity(n: &str) -> Value {
    json!({"t":"quantity","n":n,"d":"1","u":"q"})
}

fn span(start: &str, end: &str) -> Value {
    json!({"t":"list","items":[quantity(start), quantity(end)]})
}

#[test]
fn span_edits_are_validated_atomically() {
    let source = project("region chorus { span = [8q, 12q]; label = \"Chorus\"; }");
    let mut document = SourceDocument::parse(&source).unwrap();
    let set_span = |document: &SourceDocument, value: Value| {
        Transaction::new(
            document.revision().to_owned(),
            vec![Operation::Set {
                object: vec!["chorus".into()],
                field: vec!["span".into()],
                value,
                expect: None,
                expect_absent: false,
            }],
        )
        .unwrap()
    };

    for invalid in [
        span("12", "8"),
        span("8", "8"),
        span("2", "8"),
        span("8", "20"),
    ] {
        let before = document.source().to_owned();
        let revision = document.revision().to_owned();
        let error = document
            .apply(&set_span(&document, invalid), &FoundationEditContext)
            .unwrap_err();
        assert_eq!(error.code, "E_INTERVAL", "{error:?}");
        assert_eq!(document.source(), before);
        assert_eq!(document.revision(), revision);
    }

    document
        .apply(
            &set_span(&document, span("6", "14")),
            &FoundationEditContext,
        )
        .unwrap();
    assert!(document.source().contains("span = [6q, 14q];"));
    assert!(document.source().contains("label = \"Chorus\";"));
    let plan = compile(&parse(document.source()).unwrap()).unwrap();
    assert_eq!(
        plan.regions,
        vec![region("chorus", "6", "14", Some("Chorus"))]
    );
}
