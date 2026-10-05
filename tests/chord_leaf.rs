//! `chord` leaves: simultaneous notes sharing one onset and gate.

use maac::compiler::compile_bundle_artifact;
use maac::editing::{
    apply_transaction, AuthoredDocument, FoundationEditContext, Operation, Transaction,
};
use maac::production_identity::execution_identity;
use maac::{compile, parse, render_artifact, DiagnosticCode, PlanArtifact, SourceBundle};
use serde_json::{json, Value};

/// Two bars of `bar`, placed twice, with optional placement children.
fn source(pattern: &str, place_children: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 5ms; release = 50ms; level = 0.1; }}; }}
pattern bar {{ length = 4q;
{pattern}
}}
track t {{ target = &synth:events; }}
place main {{ pattern = &bar; track = &t; at = 0q; count = 2;
{place_children}
}}
"#
    )
}

const CHORDS: &str = r#"  chord a { at = 0q; dur = 2q; pitches = [A3, C4, E4, G4]; velocity = [0.5, 0.45, 0.45, 0.42]; }
  chord b { at = 2q; dur = 2q; pitches = [G3, B3, D4]; velocity = 0.4; onset_offset = [0ms, 10ms, 20ms]; }"#;

/// The same music as `CHORDS`, written as notes whose IDs sort like the
/// chord members, so voices sum in the same order.
const NOTES: &str = r#"  note a0 { at = 0q; dur = 2q; pitch = A3; velocity = 0.5; }
  note a1 { at = 0q; dur = 2q; pitch = C4; velocity = 0.45; }
  note a2 { at = 0q; dur = 2q; pitch = E4; velocity = 0.45; }
  note a3 { at = 0q; dur = 2q; pitch = G4; velocity = 0.42; }
  note b0 { at = 2q; dur = 2q; pitch = G3; velocity = 0.4; onset_offset = 0ms; }
  note b1 { at = 2q; dur = 2q; pitch = B3; velocity = 0.4; onset_offset = 10ms; }
  note b2 { at = 2q; dur = 2q; pitch = D4; velocity = 0.4; onset_offset = 20ms; }"#;

fn plan(source: &str) -> PlanArtifact {
    compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_or_else(|e| panic!("compiles: {e}"))
}

fn events(source: &str) -> Vec<Value> {
    let wire: Value = serde_json::from_slice(&plan(source).to_json().unwrap()).unwrap();
    wire["events"].as_array().unwrap().clone()
}

fn samples(source: &str) -> Vec<f64> {
    let mut out = Vec::new();
    render_artifact(&plan(source), |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    out
}

fn codes(source: &str) -> Vec<DiagnosticCode> {
    compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_err()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn chords_expand_to_the_notes_they_denote() {
    let chords = events(&source(CHORDS, ""));
    let notes = events(&source(NOTES, ""));
    assert_eq!(chords.len(), 14);
    let addresses: Vec<_> = chords
        .iter()
        .map(|e| e["address"].as_str().unwrap())
        .collect();
    assert_eq!(
        &addresses[..7],
        [
            "main/0/a.0",
            "main/0/a.1",
            "main/0/a.2",
            "main/0/a.3",
            "main/0/b.0",
            "main/0/b.1",
            "main/0/b.2"
        ]
    );
    // Members map back to their chord as the source object.
    assert_eq!(chords[2]["source"]["object"], "a");
    assert_eq!(chords[2]["source"]["path"], json!(["bar", "a"]));
    // Everything but identity matches the written-out notes.
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
    assert_eq!(strip(chords), strip(notes));
    assert_eq!(samples(&source(CHORDS, "")), samples(&source(NOTES, "")));
}

#[test]
fn overrides_address_single_members() {
    let edited = events(&source(
        CHORDS,
        r#"  override soft { event = "1/a.2"; set = { velocity = 0.1; }; }
  override gone { event = "0/b.1"; delete = true; }"#,
    ));
    let find = |address: &str| edited.iter().find(|e| e["address"] == address).cloned();
    assert_eq!(find("main/1/a.2").unwrap()["kind"]["velocity"], "1/10");
    assert_eq!(find("main/0/a.2").unwrap()["kind"]["velocity"], "9/20");
    assert!(find("main/0/b.1").is_none());
    assert!(find("main/1/b.1").is_some());
    assert_eq!(
        codes(&source(
            CHORDS,
            r#"  override x { event = "0/a.4"; set = { velocity = 0.1; }; }"#
        )),
        vec![DiagnosticCode::InstanceTarget]
    );
}

#[test]
fn malformed_chords_are_rejected() {
    let chord = |fields: &str| source(&format!("  chord a {{ at = 0q; dur = 1q; {fields} }}"), "");
    let has = |source: String, code: DiagnosticCode| {
        let codes = codes(&source);
        assert!(codes.contains(&code), "{code:?} not in {codes:?}");
    };
    has(chord("pitches = [];"), DiagnosticCode::Range);
    has(chord("pitches = C4;"), DiagnosticCode::Range);
    has(
        chord("pitches = [C4, E4]; velocity = [0.5];"),
        DiagnosticCode::Range,
    );
    has(
        chord("pitches = [C4, E4]; velocity = [0.5, 2];"),
        DiagnosticCode::Range,
    );
    has(chord("pitches = [C4, H4];"), DiagnosticCode::Range);
    has(
        chord("pitches = [C4]; onset_offset = [1q];"),
        DiagnosticCode::Unit,
    );
    has(
        chord("pitches = [C4]; pitch = C4;"),
        DiagnosticCode::UnknownField,
    );
    has(
        source(
            "  note n { at = 0q; dur = 1q; pitch = C4; }",
            "  insert extra { chord c { at = 0q; dur = 1q; pitches = [C4, E4]; } }",
        ),
        DiagnosticCode::Capability,
    );
}

#[test]
fn spelled_chord_pitches_normalize_like_note_pitches() {
    let identity = |pitches: &str| {
        let text = source(
            &format!("  chord a {{ at = 0q; dur = 1q; pitches = {pitches}; }}"),
            "",
        );
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

fn set_pitches(document: &AuthoredDocument, pitches: &[&str]) -> Transaction {
    let items: Vec<Value> = pitches
        .iter()
        .map(|p| json!({"t":"symbol","v":p}))
        .collect();
    Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["bar".into(), "a".into()],
            field: vec!["pitches".into()],
            value: json!({"t":"list","items":items}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap()
}

#[test]
fn edits_never_silently_retarget_a_member_override() {
    let context = FoundationEditContext;
    let text = source(
        CHORDS
            .lines()
            .next()
            .unwrap()
            .replace("velocity = [0.5, 0.45, 0.45, 0.42]; ", "")
            .as_str(),
        r#"  override soft { event = "0/a.2"; set = { velocity = 0.1; }; }"#,
    );

    // Removing an earlier member would shift `a.2` onto E4's neighbour.
    let mut document = authored(&text);
    let original = document.clone();
    let error = apply_transaction(
        &mut document,
        &set_pitches(&original, &["A3", "E4", "G4"]),
        &context,
    )
    .unwrap_err();
    assert_eq!(error.code, "E_INSTANCE_TARGET");
    assert_eq!(document, original);

    // Removing a later member, appending, and changing a pitch in place keep it.
    for pitches in [
        &["A3", "C4", "E4"][..],
        &["A3", "C4", "E4", "G4", "B4"],
        &["A3", "C4", "Eb4", "G4"],
    ] {
        let mut document = authored(&text);
        let transaction = set_pitches(&document, pitches);
        apply_transaction(&mut document, &transaction, &context)
            .unwrap_or_else(|e| panic!("{pitches:?}: {e}"));
    }

    // Removing the addressed member leaves a dangling target.
    let mut document = authored(&text);
    let error = apply_transaction(
        &mut document,
        &set_pitches(&original, &["A3", "C4"]),
        &context,
    )
    .unwrap_err();
    assert_eq!(error.code, "E_INSTANCE_TARGET");
}

#[test]
fn renaming_a_chord_keeps_member_overrides() {
    let context = FoundationEditContext;
    let mut document = authored(&source(
        CHORDS,
        r#"  override soft { event = "1/a.2"; set = { velocity = 0.1; }; }"#,
    ));
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::RenameId {
            object: vec!["bar".into(), "a".into()],
            new_id: "tonic".into(),
        }],
    )
    .unwrap();
    apply_transaction(&mut document, &transaction, &context).unwrap();
    assert_eq!(
        document.tree()["objects"]["main"]["children"]["soft"]["fields"]["event"]["v"],
        "1/tonic.2"
    );
}
