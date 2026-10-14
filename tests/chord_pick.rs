//! Picked chord members (§8.2): `pick` plays the pitches at its positions in
//! a chord's pitches, its voicing or a supplied voicing, so one pattern can
//! arpeggiate every chord of a harmony.

use maac::compiler::compile_bundle_artifact;
use maac::editing::{
    apply_transaction, AuthoredDocument, FoundationEditContext, Operation, SourceDocument,
    Transaction,
};
use maac::production_identity::execution_identity;
use maac::{compile, parse, DiagnosticCode, SourceBundle};
use serde_json::{json, Value};

/// Voicings `g` (six strings) and `d` (four), a pitch curve `bend`,
/// `decls`, pattern `bar` holding `bar`, and placement `main` with `place`
/// appended to its fields and children.
fn source(decls: &str, bar: &str, place: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 32q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 5ms; release = 50ms; level = 0.1; }}; }}
voicing g {{ pitches = [G2, B2, D3, G3, B3, G4]; }}
voicing d {{ pitches = [D3, A3, D4, F#4]; }}
curve bend {{ clock = seconds; points = [(0ms, -100ct, linear), (50ms, 0ct, step)]; }}
{decls}
pattern bar {{ length = 4q;
{bar}
}}
track t {{ target = &synth:events; }}
place main {{ pattern = &bar; track = &t; at = 0q; {place} }}
"#
    )
}

fn compiled(source: &str) -> Result<Vec<Value>, Vec<maac::Diagnostic>> {
    match compile_bundle_artifact(&SourceBundle::new("main.maac", source)) {
        Ok(plan) => {
            let wire: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
            Ok(wire["events"].as_array().unwrap().clone())
        }
        Err(diagnostics) => Err(diagnostics.iter().cloned().collect()),
    }
}

/// Events without their source spans, which differ between two spellings.
fn performed(source: &str) -> Vec<Value> {
    let mut events = compiled(source).unwrap_or_else(|d| panic!("compiles: {d:?}"));
    for event in &mut events {
        event["source"].as_object_mut().unwrap().remove("span");
    }
    events
}

/// `(address, score onset, pitch in Hz rounded)` of each event.
fn played(source: &str) -> Vec<(String, String, u32)> {
    performed(source)
        .iter()
        .map(|event| {
            (
                event["address"].as_str().unwrap().to_owned(),
                event["score_on_q"].as_str().unwrap().to_owned(),
                event["kind"]["pitch_hz"].as_f64().unwrap().round() as u32,
            )
        })
        .collect()
}

fn has(source: &str, code: DiagnosticCode) {
    let diagnostics = compiled(source).unwrap_err();
    assert!(
        diagnostics.iter().any(|d| d.code == code),
        "{code:?} not in {diagnostics:?}"
    );
}

fn messages(source: &str) -> Vec<String> {
    compiled(source)
        .unwrap_err()
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn a_pick_plays_like_the_pitches_it_names() {
    let picked = r#"  chord one { at = 0q; dur = 1q; voicing = &g; pick = 2; velocity = 0.6; }
  chord few { at = 1q; dur = 1q; voicing = &g; pick = [1, 3, 6]; velocity = [0.7, 0.6, 0.5]; onset_offset = [0ms, 5ms, 10ms]; }
  chord up { at = 2q; dur = 1q; voicing = &d; pick = [2, 4]; strum = up; velocity = 0.5; tilt = -0.1; }
  chord own { at = 3q; dur = 1q; pitches = [C4, E4, G4, C5]; pick = [2, 3]; velocity = 0.4; expression e { kind = pitch; curve = &bend; } }"#;
    let written = r#"  chord one { at = 0q; dur = 1q; pitches = [B2]; velocity = 0.6; }
  chord few { at = 1q; dur = 1q; pitches = [G2, D3, G4]; velocity = [0.7, 0.6, 0.5]; onset_offset = [0ms, 5ms, 10ms]; }
  chord up { at = 2q; dur = 1q; pitches = [A3, F#4]; strum = up; velocity = 0.5; tilt = -0.1; }
  chord own { at = 3q; dur = 1q; pitches = [E4, G4]; velocity = 0.4; expression e { kind = pitch; curve = &bend; } }"#;
    for place in [
        "",
        "count = 2; stretch = 2; transpose = -200ct; boundary = cut;",
    ] {
        assert_eq!(
            performed(&source("", picked, place)),
            performed(&source("", written, place)),
            "{place}"
        );
    }
    // Members are numbered over the picked pitches: `up.0` is A3, struck
    // last by the upstroke, and `up.1` is F#4.
    let up: Vec<_> = played(&source("", picked, ""))
        .into_iter()
        .filter(|(address, _, _)| address.contains("/up."))
        .map(|(address, _, hz)| (address, hz))
        .collect();
    assert_eq!(
        up,
        [
            ("main/0/up.1".to_owned(), 370),
            ("main/0/up.0".to_owned(), 220)
        ]
    );
}

#[test]
fn a_pick_follows_the_supplied_voicing_and_the_harmony() {
    let cell = r#"  chord a { at = 0q; dur = 1q; pick = 1; velocity = 0.6; }
  chord b { at = 1q; dur = 1q; pick = 2; velocity = 0.5; }
  chord c { at = 2q; dur = 1q; pick = 3; velocity = 0.5; }
  chord e { at = 3q; dur = 1q; pick = 2; velocity = 0.5; }"#;
    let pitches =
        |source: &str| -> Vec<u32> { played(source).into_iter().map(|(_, _, hz)| hz).collect() };
    // A supplied voicing: G's 1st, 2nd, 3rd and 2nd strings.
    assert_eq!(
        pitches(&source("", cell, "voicing = &g;")),
        [98, 123, 147, 123]
    );
    // Under a harmony, each repetition picks from the chord sounding there.
    let harmony = r#"harmony two { length = 8q; chords = [(0q, "G"), (4q, "D")]; }
voicings strings { chords = [("G", &g), ("D", &d)]; }"#;
    assert_eq!(
        pitches(&source(
            harmony,
            cell,
            "count = 2; harmony = &two; voicings = &strings;"
        )),
        [98, 123, 147, 123, 147, 220, 294, 220]
    );
}

#[test]
fn malformed_picks_are_rejected() {
    let chord = |fields: &str| {
        source(
            "",
            &format!("  chord c {{ at = 0q; dur = 1q; {fields} }}"),
            "",
        )
    };
    for fields in [
        "voicing = &d; pick = 0;",
        "voicing = &d; pick = 5;",
        "voicing = &d; pick = 3/2;",
        "voicing = &d; pick = [];",
        "voicing = &d; pick = [2, 2];",
        "voicing = &d; pick = [3, 1];",
        "voicing = &d; pick = [1, 5];",
        "voicing = &d; pick = 1; top = 1;",
        "voicing = &d; pick = 1; bottom = 1;",
        "pitches = [C4, E4]; pick = 3;",
        // Per-member lists have one entry per picked pitch.
        "voicing = &d; pick = [1, 2]; velocity = [0.5, 0.4, 0.3];",
    ] {
        has(&chord(fields), DiagnosticCode::Range);
    }
    has(&chord("voicing = &d; pick = D3;"), DiagnosticCode::Unit);
    assert!(compiled(&chord(
        "voicing = &d; pick = [1, 2]; velocity = [0.5, 0.4];"
    ))
    .is_ok());
    // A position beyond a supplied voicing is caught at that occurrence and
    // names the use that supplied it.
    let uses = r#"pattern rhythm { length = 2q;
  chord c { at = 0q; dur = 1q; pick = [1, 5]; }
}"#;
    let text = source(
        uses,
        "  use b1 { pattern = &rhythm; at = 0q; voicing = &g; }\n  use b2 { pattern = &rhythm; at = 2q; voicing = &d; }",
        "",
    );
    has(&text, DiagnosticCode::Range);
    assert!(
        messages(&text)
            .iter()
            .any(|m| m.contains("chord.pick") && m.contains("voicing `d` from use `b2`")),
        "{:?}",
        messages(&text)
    );
}

#[test]
fn one_position_is_a_list_of_one() {
    let identity = |pick: &str| {
        let text = source(
            "",
            &format!("  chord c {{ at = 0q; dur = 1q; voicing = &d; pick = {pick}; }}"),
            "",
        );
        let document = parse(&text).unwrap();
        let plan = compile(&document).unwrap();
        execution_identity(&document, &plan).unwrap().execution_hash
    };
    assert_eq!(identity("2"), identity("[2]"));
    assert_ne!(identity("2"), identity("3"));
}

fn authored(text: &str) -> AuthoredDocument {
    AuthoredDocument::from_document(&parse(text).unwrap()).unwrap()
}

fn set(document: &AuthoredDocument, object: &[&str], field: &str, value: Value) -> Transaction {
    Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: object.iter().map(|id| (*id).to_owned()).collect(),
            field: vec![field.into()],
            value,
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap()
}

fn positions(positions: &[i64]) -> Value {
    let items: Vec<Value> = positions
        .iter()
        .map(|p| json!({"t":"number","n":p.to_string(),"d":"1"}))
        .collect();
    json!({"t":"list","items":items})
}

#[test]
fn pick_edits_never_silently_retarget_a_member_override() {
    let context = FoundationEditContext;
    let text = source(
        "",
        "  chord c { at = 0q; dur = 1q; voicing = &g; pick = [1, 3, 5]; }",
        r#"override soft { event = "0/c.2"; set = { velocity = 0.1; }; }"#,
    );
    let try_edit = |object: &[&str], field: &str, value: Value| {
        let mut document = authored(&text);
        let transaction = set(&document, object, field, value);
        apply_transaction(&mut document, &transaction, &context)
            .map(|_| ())
            .map_err(|error| error.code)
    };
    // Dropping the third string moves `c.2` off the fifth.
    assert_eq!(
        try_edit(&["bar", "c"], "pick", positions(&[1, 5])).unwrap_err(),
        "E_INSTANCE_TARGET"
    );
    // So does picking the second string before it.
    assert_eq!(
        try_edit(&["bar", "c"], "pick", positions(&[1, 2, 3, 5])).unwrap_err(),
        "E_INSTANCE_TARGET"
    );
    // Picking another string in place, or one more on top, keeps `c.2`.
    try_edit(&["bar", "c"], "pick", positions(&[2, 3, 5])).unwrap();
    try_edit(&["bar", "c"], "pick", positions(&[1, 3, 5, 6])).unwrap();
    // A single position plays one member, so `c.2` is gone.
    assert_eq!(
        try_edit(&["bar", "c"], "pick", json!({"t":"number","n":"4","d":"1"})).unwrap_err(),
        "E_INSTANCE_TARGET"
    );
}

#[test]
fn a_pick_edit_reports_the_members_it_moves() {
    let context = FoundationEditContext;
    let text = source(
        "",
        "  chord c { at = 0q; dur = 1q; voicing = &d; pick = [1, 3]; }\n  chord n { at = 2q; dur = 1q; pitches = [C4]; }",
        "",
    );
    let mut document = authored(&text);
    let transaction = set(&document, &["bar", "c"], "pick", positions(&[2, 3]));
    let applied = apply_transaction(&mut document, &transaction, &context).unwrap();
    assert_eq!(
        applied.impact.affected_expanded_event_addresses,
        ["main/0/c.0", "main/0/c.1"]
    );
}

#[test]
fn materializing_copies_the_picked_members() {
    let text = source(
        "",
        "  chord c { at = 0q; dur = 1q; pick = [2, 4]; velocity = [0.6, 0.5]; }",
        r#"voicing = &g; count = 2; override soft { event = "1/c.1"; set = { velocity = 0.1; }; }"#,
    );
    let before = performed(&text);
    let mut document = SourceDocument::parse(&text).unwrap();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "main", "copy")
        .unwrap();
    assert_eq!(plan.mappings.len(), 4);
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    let after = performed(document.source());
    let strip = |events: Vec<Value>| -> Vec<Value> {
        events
            .into_iter()
            .map(|mut event| {
                let object = event.as_object_mut().unwrap();
                object.remove("address");
                object.remove("source");
                event
            })
            .collect()
    };
    assert_eq!(strip(before), strip(after));
}
