//! Voicings (§8.2): named pitch lists that chords play, with `top` and
//! `bottom` choosing the upper or lower strings.

use maac::compiler::compile_bundle_artifact;
use maac::editing::{
    apply_transaction, AuthoredDocument, FoundationEditContext, Operation, SourceDocument,
    Transaction,
};
use maac::production_identity::execution_identity;
use maac::{compile, parse, DiagnosticCode, SourceBundle};
use serde_json::{json, Value};

/// Voicings `g` and `d`, pattern `bar`, and placement `main` with `place`
/// appended to its fields and children.
fn source(pattern: &str, place: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 16q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 5ms; release = 50ms; level = 0.1; }}; }}
voicing g {{ pitches = [G2, B2, D3, G3, B3, G4]; }}
voicing d {{ pitches = [D3, A3, D4, F#4]; }}
pattern bar {{ length = 4q;
{pattern}
}}
track t {{ target = &synth:events; }}
place main {{ pattern = &bar; track = &t; at = 0q; {place} }}
"#
    )
}

fn compiled(source: &str) -> Result<Vec<Value>, Vec<DiagnosticCode>> {
    match compile_bundle_artifact(&SourceBundle::new("main.maac", source)) {
        Ok(plan) => {
            let wire: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
            Ok(wire["events"].as_array().unwrap().clone())
        }
        Err(diagnostics) => Err(diagnostics.iter().map(|d| d.code).collect()),
    }
}

/// Events without their source spans, which differ between two spellings.
fn performed(source: &str) -> Vec<Value> {
    let mut events = compiled(source).unwrap_or_else(|codes| panic!("compiles: {codes:?}"));
    for event in &mut events {
        event["source"].as_object_mut().unwrap().remove("span");
    }
    events
}

fn has(source: &str, code: DiagnosticCode) {
    let codes = compiled(source).unwrap_err();
    assert!(codes.contains(&code), "{code:?} not in {codes:?}");
}

#[test]
fn a_voicing_plays_like_its_written_pitches() {
    let voiced = r#"  chord d1 { at = 0q; dur = 1q; voicing = &g; strum = down; velocity = 0.78; tilt = -0.12; }
  chord u1 { at = 3/2q; dur = 1q; voicing = &g; top = 4; strum = up; velocity = 0.46; tilt = -0.06; }
  chord b1 { at = 3q; dur = 1q; voicing = &d; bottom = 2; }"#;
    let written = r#"  chord d1 { at = 0q; dur = 1q; pitches = [G2, B2, D3, G3, B3, G4]; strum = down; velocity = 0.78; tilt = -0.12; }
  chord u1 { at = 3/2q; dur = 1q; pitches = [D3, G3, B3, G4]; strum = up; velocity = 0.46; tilt = -0.06; }
  chord b1 { at = 3q; dur = 1q; pitches = [D3, A3]; }"#;
    for place in [
        "",
        "count = 2; stretch = 2; transpose = -200ct; boundary = cut;",
    ] {
        assert_eq!(
            performed(&source(voiced, place)),
            performed(&source(written, place)),
            "{place}"
        );
    }
    // Members count from the first pitch played: `u1.0` is D3.
    let events = performed(&source(voiced, ""));
    let u1: Vec<_> = events
        .iter()
        .filter(|e| e["source"]["object"] == "u1")
        .map(|e| e["address"].as_str().unwrap())
        .collect();
    assert_eq!(
        u1,
        ["main/0/u1.3", "main/0/u1.2", "main/0/u1.1", "main/0/u1.0"]
    );
    // `top` and `bottom` work with written pitches too.
    assert_eq!(
        performed(&source(
            "  chord c { at = 0q; dur = 1q; pitches = [C4, E4, G4]; top = 2; }",
            ""
        ))
        .len(),
        2
    );
}

#[test]
fn malformed_voicings_are_rejected() {
    let chord = |fields: &str| source(&format!("  chord c {{ at = 0q; dur = 1q; {fields} }}"), "");
    has(
        &chord("pitches = [C4]; voicing = &g;"),
        DiagnosticCode::Range,
    );
    // No pitches, and no use or placement supplies a voicing.
    has(&chord("velocity = 0.5;"), DiagnosticCode::Reference);
    has(
        &chord("voicing = &g; top = 2; bottom = 2;"),
        DiagnosticCode::Range,
    );
    has(&chord("voicing = &d; top = 5;"), DiagnosticCode::Range);
    has(&chord("voicing = &g; bottom = 0;"), DiagnosticCode::Range);
    has(&chord("voicing = &g; top = 3/2;"), DiagnosticCode::Range);
    has(&chord("voicing = &bar;"), DiagnosticCode::Reference);
    has(&chord("voicing = &missing;"), DiagnosticCode::Reference);
    // Per-note lists have one entry per pitch played.
    has(
        &chord("voicing = &g; top = 2; velocity = [0.5, 0.4, 0.3];"),
        DiagnosticCode::Range,
    );
    assert!(compiled(&chord("voicing = &g; top = 2; velocity = [0.5, 0.4];")).is_ok());
    let voicing =
        |text: &str| source("", "").replace("voicing d { pitches = [D3, A3, D4, F#4]; }", text);
    has(
        &voicing("voicing d { pitches = []; }"),
        DiagnosticCode::Range,
    );
    has(
        &voicing("voicing d { pitches = [D3, H3]; }"),
        DiagnosticCode::Range,
    );
    has(&voicing("voicing d { }"), DiagnosticCode::Range);
    has(
        &voicing("voicing d { pitches = [D3]; top = 1; }"),
        DiagnosticCode::UnknownField,
    );
}

#[test]
fn spelled_voicing_pitches_normalize_like_note_pitches() {
    let identity = |pitches: &str| {
        let text = source("  chord c { at = 0q; dur = 1q; voicing = &d; }", "")
            .replace("[D3, A3, D4, F#4]", pitches);
        let document = parse(&text).unwrap();
        let plan = compile(&document).unwrap();
        execution_identity(&document, &plan).unwrap().execution_hash
    };
    assert_eq!(identity("[C4, E4]"), identity("[key(60), key(64)]"));
    assert_ne!(identity("[C4, E4]"), identity("[C4, F4]"));
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

fn pitches(pitches: &[&str]) -> Value {
    let items: Vec<Value> = pitches
        .iter()
        .map(|p| json!({"t":"symbol","v":p}))
        .collect();
    json!({"t":"list","items":items})
}

#[test]
fn voicing_edits_never_silently_retarget_a_member_override() {
    let context = FoundationEditContext;
    let text = source(
        "  chord c { at = 0q; dur = 1q; voicing = &d; }\n  chord u { at = 2q; dur = 1q; voicing = &d; top = 2; }",
        r#"override soft { event = "0/c.2"; set = { velocity = 0.1; }; }
  override high { event = "0/u.1"; set = { velocity = 0.2; }; }"#,
    );
    let try_edit = |object: &[&str], field: &str, value: Value| {
        let mut document = authored(&text);
        let transaction = set(&document, object, field, value);
        apply_transaction(&mut document, &transaction, &context)
            .map(|_| ())
            .map_err(|error| error.code)
    };
    // Dropping D3 shifts `c.2` onto F#4.
    assert_eq!(
        try_edit(&["d"], "pitches", pitches(&["A3", "D4", "F#4"])).unwrap_err(),
        "E_INSTANCE_TARGET"
    );
    // A new bass string shifts `c.2` too, but leaves `top = 2` alone.
    assert_eq!(
        try_edit(&["d"], "pitches", pitches(&["A2", "D3", "A3", "D4", "F#4"])).unwrap_err(),
        "E_INSTANCE_TARGET"
    );
    // Appending, and changing a pitch in place, keep every member.
    try_edit(&["d"], "pitches", pitches(&["D3", "A3", "D4", "F#4", "A4"])).unwrap();
    try_edit(&["d"], "pitches", pitches(&["D3", "A3", "D4", "F4"])).unwrap();
    // Widening `top` shifts the upper strings' indexes.
    assert_eq!(
        try_edit(&["bar", "u"], "top", json!({"t":"number","n":"3","d":"1"})).unwrap_err(),
        "E_INSTANCE_TARGET"
    );
}

#[test]
fn a_voicing_edit_reports_the_members_it_moves() {
    let context = FoundationEditContext;
    let text = source(
        "  chord c { at = 0q; dur = 1q; voicing = &d; top = 2; }\n  chord n { at = 2q; dur = 1q; pitches = [C4]; }",
        "",
    );
    let mut document = authored(&text);
    let transaction = set(
        &document,
        &["d"],
        "pitches",
        pitches(&["D3", "A3", "D4", "F4"]),
    );
    let applied = apply_transaction(&mut document, &transaction, &context).unwrap();
    assert_eq!(
        applied.impact.affected_expanded_event_addresses,
        ["main/0/c.0", "main/0/c.1"]
    );
}

#[test]
fn materializing_copies_the_voicing_with_its_members() {
    let text = source(
        "  chord c { at = 0q; dur = 1q; voicing = &g; top = 3; velocity = [0.6, 0.5, 0.4]; }",
        r#"count = 2; override soft { event = "1/c.1"; set = { velocity = 0.1; }; }"#,
    );
    let before = performed(&text);
    let mut document = SourceDocument::parse(&text).unwrap();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "main", "copy")
        .unwrap();
    assert_eq!(plan.mappings.len(), 6);
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

/// `rhythm`, two quarters long, played by the uses in `bar`.
fn progression(rhythm: &str, uses: &str, place: &str) -> String {
    source(uses, place).replace(
        "pattern bar {",
        &format!("pattern rhythm {{ length = 2q;\n{rhythm}\n}}\npattern bar {{"),
    )
}

fn messages(source: &str) -> Vec<String> {
    compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_err()
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

/// `(address, pitch, velocity, onset offset, score onset)` of each event.
fn notes(source: &str) -> Vec<(String, String, String, String, String)> {
    performed(source)
        .iter()
        .map(|e| {
            (
                e["address"].as_str().unwrap().to_owned(),
                e["kind"]["pitch_hz"].to_string(),
                e["kind"]["velocity"].to_string(),
                e["onset_offset_seconds"].to_string(),
                e["score_on_q"].to_string(),
            )
        })
        .collect()
}

const RHYTHM: &str = r#"  chord down { at = 0q; dur = 1q; strum = down; velocity = 0.7; tilt = -0.1; }
  chord up { at = 1q; dur = 1q; top = 3; strum = up; velocity = 0.5; }
  chord own { at = 1q; dur = 1q; pitches = [C5]; }"#;

const USES: &str = r#"  use b1 { pattern = &rhythm; at = 0q; }
  use b2 { pattern = &rhythm; at = 2q; voicing = &d; }"#;

#[test]
fn the_nearest_use_or_placement_supplies_the_voicing() {
    // The same music with every chord naming its voicing.
    let named = |voicing: &str| {
        RHYTHM
            .replace(
                "strum = down;",
                &format!("voicing = &{voicing}; strum = down;"),
            )
            .replace("top = 3;", &format!("voicing = &{voicing}; top = 3;"))
            .replace("pattern rhythm", "unused")
    };
    for place in ["", "transpose = 1200ct; stretch = 2;"] {
        let supplied = progression(RHYTHM, USES, &format!("voicing = &g; {place}"));
        let written = source(
            r#"  use b1 { pattern = &rg; at = 0q; }
  use b2 { pattern = &rd; at = 2q; }"#,
            place,
        )
        .replace(
            "pattern bar {",
            &format!(
                "pattern rg {{ length = 2q;\n{}\n}}\npattern rd {{ length = 2q;\n{}\n}}\npattern bar {{",
                named("g"),
                named("d")
            ),
        );
        assert_eq!(notes(&supplied), notes(&written), "{place}");
    }
    let supplied = notes(&progression(RHYTHM, USES, "voicing = &g;"));
    let count = |prefix: &str| supplied.iter().filter(|n| n.0.starts_with(prefix)).count();
    // The placement's six-string G, then the use's four-string D.
    assert_eq!(count("main/0/b1/0/down."), 6);
    assert_eq!(count("main/0/b2/0/down."), 4);
    assert_eq!(count("main/0/b1/0/up."), 3);
    // A chord's own pitches ignore the supplied voicing.
    assert_eq!(count("main/0/b1/0/own."), 1);
    assert_eq!(count("main/0/b2/0/own."), 1);
}

#[test]
fn a_supplied_voicing_is_checked_at_each_occurrence() {
    // Nothing in scope for `b1`.
    has(&progression(RHYTHM, USES, ""), DiagnosticCode::Reference);
    // `top = 5` fits G's six strings but not D's four, and the error says so.
    let wide = RHYTHM.replace("top = 3;", "top = 5;");
    assert!(compiled(&progression(
        &wide,
        "  use b1 { pattern = &rhythm; at = 0q; }",
        "voicing = &g;"
    ))
    .is_ok());
    let text = progression(&wide, USES, "voicing = &g;");
    has(&text, DiagnosticCode::Range);
    assert!(
        messages(&text)
            .iter()
            .any(|m| m.contains("voicing `d` from use `b2`")),
        "{:?}",
        messages(&text)
    );
    // A per-member list must match each supplied voicing.
    let listed = RHYTHM
        .replace(
            "velocity = 0.5;",
            "velocity = [0.5, 0.45, 0.4, 0.35, 0.3, 0.25]; top = 6;",
        )
        .replace("top = 3; ", "");
    has(
        &progression(&listed, USES, "voicing = &g;"),
        DiagnosticCode::Range,
    );
    // A member that exists under G but not under D.
    let reaching = |event: &str| {
        progression(
            RHYTHM,
            USES,
            &format!(
                r#"voicing = &g; override x {{ event = "{event}"; set = {{ velocity = 0.1; }}; }}"#
            ),
        )
    };
    assert!(compiled(&reaching("0/b1/0/down.5")).is_ok());
    has(&reaching("0/b2/0/down.5"), DiagnosticCode::InstanceTarget);
    // Only a voicing can be supplied.
    has(
        &progression(RHYTHM, USES, "voicing = &rhythm;"),
        DiagnosticCode::Reference,
    );
}

#[test]
fn editing_a_supplied_voicing_never_silently_retargets() {
    let context = FoundationEditContext;
    let text = progression(
        RHYTHM,
        USES,
        r#"voicing = &g; override x { event = "0/b1/0/down.5"; set = { velocity = 0.1; }; }"#,
    );
    let reference = |id: &str| json!({"t":"ref","path":[id],"port":null});
    for (object, value) in [
        (&["bar", "b1"][..], reference("d")),
        (&["main"][..], reference("d")),
    ] {
        let mut document = authored(&text);
        let transaction = set(&document, object, "voicing", value);
        let error = apply_transaction(&mut document, &transaction, &context).unwrap_err();
        assert_eq!(error.code, "E_INSTANCE_TARGET", "{object:?}");
    }
    // Swapping D for another four-string shape keeps every member.
    let mut document = authored(&text);
    let transaction = set(
        &document,
        &["d"],
        "pitches",
        pitches(&["E3", "B3", "E4", "G4"]),
    );
    let applied = apply_transaction(&mut document, &transaction, &context).unwrap();
    // Only the chords that play D move.
    assert_eq!(
        applied.impact.affected_expanded_event_addresses,
        [
            "main/0/b2/0/down.0",
            "main/0/b2/0/down.1",
            "main/0/b2/0/down.2",
            "main/0/b2/0/down.3",
            "main/0/b2/0/up.0",
            "main/0/b2/0/up.1",
            "main/0/b2/0/up.2",
        ]
    );
}

#[test]
fn materializing_keeps_the_supplied_voicings() {
    let text = progression(
        RHYTHM,
        USES,
        r#"voicing = &g; count = 2; override x { event = "1/b2/0/down.3"; set = { velocity = 0.1; }; }"#,
    );
    let before = notes(&text);
    let mut document = SourceDocument::parse(&text).unwrap();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "main", "copy")
        .unwrap();
    // Per repetition: G's 6 + 3 + 1 and D's 4 + 3 + 1.
    assert_eq!(plan.mappings.len(), 36);
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    let after = notes(document.source());
    let music = |notes: Vec<(String, String, String, String, String)>| {
        let mut music: Vec<_> = notes.into_iter().map(|n| (n.4, n.1, n.2, n.3)).collect();
        music.sort();
        music
    };
    assert_eq!(music(before), music(after));
}
