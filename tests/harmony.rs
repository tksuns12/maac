//! Harmonies (§8.2): a `harmony` names which chord sounds when, a part's
//! `voicings` how it voices each chord, and a use or placement supplies both
//! to chords without pitches.

use maac::compiler::compile_bundle_artifact;
use maac::editing::{
    apply_transaction, AuthoredDocument, FoundationEditContext, Operation, SourceDocument,
    Transaction,
};
use maac::{parse, DiagnosticCode, SourceBundle};
use serde_json::{json, Value};

/// Two-note voicings for D, A, Bm and G, a one-bar rhythm `bar` of two
/// chords without pitches, `decls`, and placement `main` with `place`.
fn source(decls: &str, bar: &str, place: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 64q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 5ms; release = 50ms; level = 0.1; }}; }}
voicing d {{ pitches = [D3, F#3]; }}
voicing a {{ pitches = [A2, C#3]; }}
voicing bm {{ pitches = [B2, D3]; }}
voicing g {{ pitches = [G2, B2]; }}
harmony four {{ length = 16q; chords = [(0q, "D"), (4q, "A"), (8q, "Bm"), (12q, "G")]; }}
voicings low {{ chords = [("D", &d), ("A", &a), ("Bm", &bm), ("G", &g)]; }}
{decls}
pattern bar {{ length = 4q;
{bar}
}}
track t {{ target = &synth:events; }}
place main {{ pattern = &bar; track = &t; at = 0q; {place} }}
"#
    )
}

const BAR: &str = "  chord one { at = 0q; dur = 2q; velocity = 0.5; }\n  chord three { at = 2q; dur = 2q; bottom = 1; velocity = 0.5; }";

fn compiled(source: &str) -> Result<Vec<Value>, Vec<maac::Diagnostic>> {
    match compile_bundle_artifact(&SourceBundle::new("main.maac", source)) {
        Ok(plan) => {
            let wire: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
            Ok(wire["events"].as_array().unwrap().clone())
        }
        Err(diagnostics) => Err(diagnostics.iter().cloned().collect()),
    }
}

/// `(address, score onset, pitch in Hz rounded)` of each event.
fn played(source: &str) -> Vec<(String, String, u32)> {
    compiled(source)
        .unwrap_or_else(|d| panic!("compiles: {d:?}"))
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

/// The rounded pitches sounding from each onset, in onset order.
fn chords(source: &str) -> Vec<(String, Vec<u32>)> {
    let mut out: Vec<(String, Vec<u32>)> = Vec::new();
    for (_, at, hz) in played(source) {
        match out.last_mut() {
            Some((last, pitches)) if *last == at => pitches.push(hz),
            _ => out.push((at, vec![hz])),
        }
    }
    for (_, pitches) in &mut out {
        pitches.sort_unstable();
    }
    out
}

fn codes(source: &str) -> Vec<DiagnosticCode> {
    compiled(source)
        .unwrap_err()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

fn has(source: &str, code: DiagnosticCode) {
    let codes = codes(source);
    assert!(codes.contains(&code), "{code:?} not in {codes:?}");
}

// D3 147, F#3 185; A2 110, C#3 139; B2 123, D3 147; G2 98, B2 123.
const D: [u32; 2] = [147, 185];
const A: [u32; 2] = [110, 139];
const BM: [u32; 2] = [123, 147];
const G: [u32; 2] = [98, 123];

fn expect(bars: &[[u32; 2]]) -> Vec<(String, Vec<u32>)> {
    bars.iter()
        .enumerate()
        .flat_map(|(bar, pitches)| {
            [
                (format!("{}/1", 4 * bar), pitches.to_vec()),
                (format!("{}/1", 4 * bar + 2), vec![pitches[0]]),
            ]
        })
        .collect()
}

#[test]
fn each_repetition_plays_the_chord_at_its_position() {
    let text = source("", BAR, "count = 4; harmony = &four; voicings = &low;");
    assert_eq!(chords(&text), expect(&[D, A, BM, G]));
    // Repetitions are the placement's, so addresses count bars.
    let addresses: Vec<String> = played(&text).into_iter().map(|(a, _, _)| a).collect();
    assert_eq!(
        &addresses[..3],
        ["main/0/one.0", "main/0/one.1", "main/0/three.0"]
    );
}

#[test]
fn a_harmony_repeats_past_its_length() {
    let text = source(
        "harmony two { length = 8q; chords = [(0q, \"G\"), (4q, \"D\")]; }",
        BAR,
        "count = 4; harmony = &two; voicings = &low;",
    );
    assert_eq!(chords(&text), expect(&[G, D, G, D]));
}

#[test]
fn stretch_and_groove_do_not_move_the_lookup() {
    // Stretched twice over, each pattern bar still plays its own chord.
    let stretched = source(
        "",
        BAR,
        "count = 4; stretch = 2; harmony = &four; voicings = &low;",
    );
    let pitches: Vec<Vec<u32>> = chords(&stretched).into_iter().map(|(_, p)| p).collect();
    assert_eq!(
        pitches,
        expect(&[D, A, BM, G])
            .into_iter()
            .map(|(_, p)| p)
            .collect::<Vec<_>>()
    );
    // The groove swings 1/2q to 2/3q, past a change at 7/12q; the chord is
    // found at the straight position.
    let swung = source(
        "groove lazy { grid = 1q; ratio = 2/3; }\nharmony early { length = 4q; chords = [(0q, \"D\"), (7/12q, \"A\")]; }",
        "  chord off { at = 1/2q; dur = 1/4q; bottom = 1; velocity = 0.5; }",
        "harmony = &early; voicings = &low; groove = &lazy;",
    );
    assert_eq!(chords(&swung), [("2/3".to_owned(), vec![D[0]])]);
}

#[test]
fn the_nearest_supplier_wins_and_explicit_pitches_win() {
    let decls = "pattern inner { length = 4q;\n  chord c { at = 0q; dur = 1q; bottom = 1; velocity = 0.5; }\n}\npattern outer { length = 8q;\n  use fixed { pattern = &inner; at = 0q; voicing = &g; }\n  use free { pattern = &inner; at = 4q; }\n}";
    let text = source(decls, BAR, "harmony = &four; voicings = &low;").replace(
        "place main { pattern = &bar;",
        "place main { pattern = &outer;",
    );
    // `fixed` names a voicing; `free` inherits the harmony at 4q, A.
    assert_eq!(
        chords(&text),
        [
            ("0/1".to_owned(), vec![G[0]]),
            ("4/1".to_owned(), vec![A[0]])
        ]
    );
    // A harmony inside a voicing replaces it for the chords below.
    let inside = "pattern inner { length = 4q;\n  chord c { at = 0q; dur = 1q; bottom = 1; velocity = 0.5; }\n}\npattern outer { length = 8q;\n  use chords { pattern = &inner; at = 4q; harmony = &four; voicings = &low; }\n  chord own { at = 0q; dur = 1q; bottom = 1; velocity = 0.5; }\n  chord fixed { at = 1q; dur = 1q; pitches = [C4]; velocity = 0.5; }\n}";
    let text = source(inside, BAR, "voicing = &bm;").replace(
        "place main { pattern = &bar;",
        "place main { pattern = &outer;",
    );
    assert_eq!(
        chords(&text),
        [
            ("0/1".to_owned(), vec![BM[0]]),
            ("1/1".to_owned(), vec![262]),
            ("4/1".to_owned(), vec![D[0]]),
        ]
    );
}

#[test]
fn transposition_applies_to_the_voiced_chords() {
    let text = source(
        "",
        BAR,
        "count = 2; transpose = 1200ct; harmony = &four; voicings = &low;",
    );
    assert_eq!(chords(&text)[0].1, [294, 370]);
}

#[test]
fn malformed_harmonies_and_supplies_are_rejected() {
    let with_harmony = |chords: &str| {
        source(
            &format!("harmony bad {{ length = 8q; chords = [{chords}]; }}"),
            BAR,
            "harmony = &bad; voicings = &low;",
        )
    };
    has(&with_harmony("(1q, \"D\")"), DiagnosticCode::Range);
    has(
        &with_harmony("(0q, \"D\"), (0q, \"A\")"),
        DiagnosticCode::Range,
    );
    has(
        &with_harmony("(0q, \"D\"), (8q, \"A\")"),
        DiagnosticCode::Range,
    );
    has(&with_harmony("(0q, \"\")"), DiagnosticCode::Range);
    let with_voicings = |entries: &str| {
        source(
            &format!("voicings bad {{ chords = [{entries}]; }}"),
            BAR,
            "harmony = &four; voicings = &bad;",
        )
    };
    has(
        &with_voicings("(\"D\", &d), (\"D\", &a)"),
        DiagnosticCode::Range,
    );
    has(&with_voicings("(\"D\", &four)"), DiagnosticCode::Reference);
    has(&source("", BAR, "harmony = &four;"), DiagnosticCode::Range);
    has(&source("", BAR, "voicings = &low;"), DiagnosticCode::Range);
    has(
        &source("", BAR, "voicing = &d; harmony = &four; voicings = &low;"),
        DiagnosticCode::Range,
    );
    has(
        &source("", BAR, "harmony = &low; voicings = &low;"),
        DiagnosticCode::Reference,
    );
}

#[test]
fn an_unmapped_chord_names_everything_involved() {
    let text = source(
        "voicings partial { chords = [(\"D\", &d), (\"A\", &a)]; }",
        BAR,
        "count = 4; harmony = &four; voicings = &partial;",
    );
    let diagnostics = compiled(&text).unwrap_err();
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.code == DiagnosticCode::Reference)
        .unwrap_or_else(|| panic!("{diagnostics:?}"));
    for part in [
        "\"Bm\"",
        "`four`",
        "`partial`",
        "\"A\", \"D\"",
        "placement `main`",
    ] {
        assert!(
            diagnostic.message.contains(part),
            "{part} in {}",
            diagnostic.message
        );
    }
}

#[test]
fn a_top_too_large_for_a_harmony_voicing_names_the_chord() {
    let text = source(
        "voicing one { pitches = [E2]; }\nvoicings thin { chords = [(\"D\", &d), (\"A\", &a), (\"Bm\", &bm), (\"G\", &one)]; }",
        "  chord c { at = 0q; dur = 1q; top = 2; velocity = 0.5; }",
        "count = 4; harmony = &four; voicings = &thin;",
    );
    let diagnostics = compiled(&text).unwrap_err();
    assert!(
        diagnostics.iter().any(|d| d.code == DiagnosticCode::Range
            && d.message.contains("\"G\"")
            && d.message.contains("`four`")),
        "{diagnostics:?}"
    );
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

fn timeline(entries: &[(i64, &str)]) -> Value {
    let items: Vec<Value> = entries
        .iter()
        .map(|(q, name)| {
            json!({"t":"tuple","items":[
                {"t":"quantity","n":q.to_string(),"d":"1","u":"q"},
                {"t":"string","v":name},
            ]})
        })
        .collect();
    json!({"t":"list","items":items})
}

#[test]
fn harmony_edits_never_silently_retarget_a_member_override() {
    let text = source(
        "voicing one { pitches = [E2]; }\nvoicings mixed { chords = [(\"D\", &d), (\"A\", &a), (\"Bm\", &bm), (\"G\", &g), (\"E\", &one)]; }",
        BAR,
        r#"count = 4; harmony = &four; voicings = &mixed;
  override soft { event = "1/one.1"; set = { velocity = 0.1; }; }"#,
    );
    let try_edit = |entries: &[(i64, &str)]| {
        let mut document = AuthoredDocument::from_document(&parse(&text).unwrap()).unwrap();
        let transaction = set(&document, &["four"], "chords", timeline(entries));
        apply_transaction(&mut document, &transaction, &FoundationEditContext)
            .map(|applied| applied.impact.affected_expanded_event_addresses)
            .map_err(|error| error.code)
    };
    // Bar 2 becoming a one-note chord would move `one.1` off its member.
    assert_eq!(
        try_edit(&[(0, "D"), (4, "E"), (8, "Bm"), (12, "G")]).unwrap_err(),
        "E_INSTANCE_TARGET"
    );
    // Swapping bar 3's chord keeps every member. The impact is every chord
    // that plays from the harmony, the changed bar among them.
    let affected = try_edit(&[(0, "D"), (4, "A"), (8, "G"), (12, "G")]).unwrap();
    assert!(affected.iter().any(|a| a == "main/2/one.0"), "{affected:?}");
    assert_eq!(affected.len(), 12, "{affected:?}");
}

/// Events without the fields materialization renames.
fn performance(source: &str) -> Vec<Value> {
    compiled(source)
        .unwrap()
        .into_iter()
        .map(|mut event| {
            let object = event.as_object_mut().unwrap();
            object.remove("address");
            object.remove("source");
            event
        })
        .collect()
}

#[test]
fn materializing_keeps_the_harmony_the_placement_supplies() {
    let text = source("", BAR, "count = 4; harmony = &four; voicings = &low;");
    let before = performance(&text);
    let mut document = SourceDocument::parse(&text).unwrap();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "main", "copy")
        .unwrap();
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    assert_eq!(before, performance(document.source()));
    // A stretched placement would take the stretch into the copies and move
    // the lookup, so it is refused.
    let stretched = source(
        "",
        BAR,
        "count = 2; stretch = 2; harmony = &four; voicings = &low;",
    );
    let document = SourceDocument::parse(&stretched).unwrap();
    let refused = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "main", "copy")
        .unwrap_err();
    assert_eq!(refused.code, "E_CAPABILITY");
    // So is a use that repeats a harmony: each copy would start it again.
    let repeated = source(
        "pattern outer { length = 8q;\n  use twice { pattern = &bar; at = 0q; count = 2; harmony = &four; voicings = &low; }\n}",
        BAR,
        "",
    )
    .replace("place main { pattern = &bar;", "place main { pattern = &outer;");
    let document = SourceDocument::parse(&repeated).unwrap();
    let refused = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "main", "copy")
        .unwrap_err();
    assert_eq!(refused.code, "E_CAPABILITY");
}
