//! Notes in succession (§9): `at = after(id)` or `at = after(id, rest)`
//! starts a note or chord where another of its pattern ends.

use maac::compiler::compile_bundle_artifact;
use maac::editing::{
    apply_transaction, AuthoredDocument, FoundationEditContext, Operation, SourceDocument,
    Transaction,
};
use maac::{parse, DiagnosticCode, SourceBundle};
use serde_json::{json, Value};

/// A sine, the voicings `c` and `g`, a harmony that changes every 2q, and
/// the patterns and placements given.
fn source(patterns: &str, places: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 32q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = 0.1; }}; }}
voicing c {{ pitches = [C4, E4, G4]; }}
voicing g {{ pitches = [B3, D4, G4]; }}
harmony changes {{ length = 4q; chords = [(0q, "C"), (2q, "G")]; }}
voicings piano {{ chords = [("C", &c), ("G", &g)]; }}
{patterns}
track t {{ target = &synth:events; }}
{places}
"#
    )
}

const PLACE: &str = "place main { pattern = &line; track = &t; at = 0q; count = 2; }";
const HARMONY: &str = "place main { pattern = &line; track = &t; at = 0q; count = 2; harmony = &changes; voicings = &piano; }";

const IN_SUCCESSION: &str = r#"pattern line { length = 8q;
  note a { at = 1/2q; dur = 1/2q; pitch = D4; velocity = 0.6; }
  note b { at = after(a); dur = 1/2q; pitch = F4; velocity = 0.66; }
  chord k { at = after(b, 1/2q); dur = 1q; pitches = [A3, C4]; }
  note d { at = after(k); dur = 3/2q; pitch = A4; slur = true; }
  note e { at = after(d, 1q); dur = 1/2q; pitch = G4; }
}"#;

const WRITTEN: &str = r#"pattern line { length = 8q;
  note a { at = 1/2q; dur = 1/2q; pitch = D4; velocity = 0.6; }
  note b { at = 1q; dur = 1/2q; pitch = F4; velocity = 0.66; }
  chord k { at = 2q; dur = 1q; pitches = [A3, C4]; }
  note d { at = 3q; dur = 3/2q; pitch = A4; slur = true; }
  note e { at = 11/2q; dur = 1/2q; pitch = G4; }
}"#;

fn events(source: &str) -> Vec<Value> {
    let artifact = compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_or_else(|e| panic!("compiles: {e}"));
    let wire: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    wire["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| {
            let mut event = event.clone();
            event["source"].as_object_mut().unwrap().remove("span");
            event
        })
        .collect()
}

fn codes(source: &str) -> Vec<DiagnosticCode> {
    compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_err()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn notes_in_succession_land_where_they_would_be_written() {
    // `slur` on a sine joins nothing, so it is refused in both forms alike;
    // compare without it.
    let succession = IN_SUCCESSION.replace(" slur = true;", "");
    let written = WRITTEN.replace(" slur = true;", "");
    assert_eq!(
        events(&source(&succession, PLACE)),
        events(&source(&written, PLACE))
    );
}

#[test]
fn a_chord_in_succession_plays_the_harmony_at_its_place() {
    let line = r#"pattern line { length = 4q;
  chord one { at = 0q; dur = 1q; }
  chord two { at = after(one, 1q); dur = 2q; }
}"#;
    let written = r#"pattern line { length = 4q;
  chord one { at = 0q; dur = 1q; }
  chord two { at = 2q; dur = 2q; }
}"#;
    let succession = events(&source(line, HARMONY));
    assert_eq!(succession, events(&source(written, HARMONY)));
    // The G chord: B3 is the first member of `two`.
    let two = succession
        .iter()
        .find(|e| e["address"] == "main/1/two.0")
        .unwrap();
    assert_eq!(two["kind"]["pitch_hz"].as_f64().unwrap().round(), 247.0);
}

#[test]
fn succession_errors() {
    let with = |line: &str| {
        codes(&source(
            &format!("pattern line {{ length = 4q;\n  note a {{ at = 0q; dur = 1q; pitch = C4; }}\n{line}\n}}"),
            PLACE,
        ))
    };
    let cycle = "  note b { at = after(c); dur = 1q; pitch = D4; }\n  note c { at = after(b); dur = 1q; pitch = E4; }";
    assert!(with(cycle).contains(&DiagnosticCode::PatternCycle));
    assert!(with("  note b { at = after(b); dur = 1q; pitch = D4; }")
        .contains(&DiagnosticCode::PatternCycle));
    assert!(with("  note b { at = after(z); dur = 1q; pitch = D4; }")
        .contains(&DiagnosticCode::Reference));
    assert!(
        with("  note b { at = after(a, -1q); dur = 1q; pitch = D4; }")
            .contains(&DiagnosticCode::Range)
    );
    assert!(
        with("  note b { at = after(a, 1s); dur = 1q; pitch = D4; }")
            .contains(&DiagnosticCode::Unit)
    );
    assert!(
        with("  note b { at = after(); dur = 1q; pitch = D4; }").contains(&DiagnosticCode::Range)
    );
    assert!(with("  phrase p { at = after(a); length = 1q; }").contains(&DiagnosticCode::Range));
    // Past the pattern's length.
    assert!(
        with("  note b { at = after(a, 3q); dur = 1q; pitch = D4; }")
            .contains(&DiagnosticCode::Interval)
    );
    let insert = r#"place main { pattern = &line; track = &t; at = 0q;
  insert extra { note n { at = after(a); dur = 1q; pitch = G4; } }
}"#;
    let line = "pattern line { length = 4q; note a { at = 0q; dur = 1q; pitch = C4; } }";
    assert!(codes(&source(line, insert)).contains(&DiagnosticCode::Range));
}

#[test]
fn lengthening_a_note_reports_the_notes_that_follow_it() {
    let text = source(&IN_SUCCESSION.replace(" slur = true;", ""), PLACE);
    let mut document = AuthoredDocument::from_document(&parse(&text).unwrap()).unwrap();
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["line".into(), "b".into()],
            field: vec!["dur".into()],
            value: json!({"t":"quantity","n":"1","d":"1","u":"q"}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap();
    let applied = apply_transaction(&mut document, &transaction, &FoundationEditContext).unwrap();
    let affected = applied.impact.affected_expanded_event_addresses;
    for address in [
        "main/0/b",
        "main/0/k.0",
        "main/0/k.1",
        "main/0/d",
        "main/1/e",
    ] {
        assert!(
            affected.contains(&address.to_owned()),
            "{address}: {affected:?}"
        );
    }
    assert!(!affected.contains(&"main/0/a".to_owned()), "{affected:?}");
}

#[test]
fn a_materialized_instance_keeps_its_succession() {
    let text = source(
        r#"pattern line { length = 4q;
  chord one { at = 0q; dur = 1q; }
  chord two { at = after(one, 1q); dur = 2q; }
}"#,
        HARMONY,
    );
    let before = events(&text);
    let mut document = SourceDocument::parse(text).unwrap();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "main", "copy")
        .unwrap();
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    let after = events(document.source());
    assert_eq!(before.len(), after.len());
    for event in &before {
        let old = event["address"].as_str().unwrap();
        let new = plan
            .mappings
            .iter()
            .find(|mapping| mapping.old_event_address == old)
            .map_or(old, |mapping| mapping.new_event_address.as_str());
        let moved = after.iter().find(|e| e["address"] == new).unwrap();
        assert_eq!(event["kind"], moved["kind"], "event {old}");
        assert_eq!(event["score_on_q"], moved["score_on_q"], "event {old}");
    }
}
