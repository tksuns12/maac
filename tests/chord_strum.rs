//! Strummed chords (§8.2): `strum`, `stagger` and `tilt` lower to the onset
//! offsets and velocities a written-out strum would list.

use maac::compiler::compile_bundle_artifact;
use maac::production_identity::execution_identity;
use maac::{compile, parse, DiagnosticCode, SourceBundle};
use serde_json::Value;

/// `bar` placed once by `main`, whose extra fields are `place_fields`.
fn source(pattern: &str, place_fields: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 16q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
groove lazy {{ grid = 1/2q; ratio = 2/3; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 5ms; release = 50ms; level = 0.1; }}; }}
pattern bar {{ length = 4q;
{pattern}
}}
track t {{ target = &synth:events; }}
place main {{ pattern = &bar; track = &t; at = 0q; {place_fields} }}
"#
    )
}

fn events(source: &str) -> Vec<Value> {
    let plan = compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_or_else(|e| panic!("compiles: {e}"));
    let wire: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    wire["events"].as_array().unwrap().clone()
}

/// Events without their source spans, which differ between two spellings.
fn performed(source: &str) -> Vec<Value> {
    let mut events = events(source);
    for event in &mut events {
        event["source"].as_object_mut().unwrap().remove("span");
    }
    events
}

fn codes(source: &str) -> Vec<DiagnosticCode> {
    compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_err()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

/// `(address, onset offset, velocity)` of each event, in plan order.
fn strikes(events: &[Value]) -> Vec<(String, String, String)> {
    events
        .iter()
        .map(|event| {
            (
                event["address"].as_str().unwrap().to_owned(),
                event["onset_offset_seconds"].as_str().unwrap().to_owned(),
                event["kind"]["velocity"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

const STRUMS: &str = r#"  chord d { at = 0q; dur = 1q; pitches = [E3, G3, B3, E4]; strum = down; velocity = 0.8; tilt = -0.12; }
  chord u { at = 1q; dur = 1q; pitches = [E3, G3, B3, E4]; strum = up; stagger = 30ms; velocity = 0.5; }"#;

/// `STRUMS` with its offsets and velocities written out.
const LISTS: &str = r#"  chord d { at = 0q; dur = 1q; pitches = [E3, G3, B3, E4]; velocity = [0.8, 0.76, 0.72, 0.68]; onset_offset = [0ms, 8ms, 16ms, 24ms]; }
  chord u { at = 1q; dur = 1q; pitches = [E3, G3, B3, E4]; velocity = 0.5; onset_offset = [90ms, 60ms, 30ms, 0ms]; }"#;

#[test]
fn a_strum_staggers_and_tilts_each_string() {
    let strummed = events(&source(STRUMS, ""));
    let expected = [
        ("main/0/d.0", "0/1", "4/5"),
        ("main/0/d.1", "1/125", "19/25"),
        ("main/0/d.2", "2/125", "18/25"),
        ("main/0/d.3", "3/125", "17/25"),
        // An upstroke strikes the top string first.
        ("main/0/u.3", "0/1", "1/2"),
        ("main/0/u.2", "3/100", "1/2"),
        ("main/0/u.1", "3/50", "1/2"),
        ("main/0/u.0", "9/100", "1/2"),
    ]
    .map(|(a, o, v)| (a.to_owned(), o.to_owned(), v.to_owned()));
    assert_eq!(strikes(&strummed), expected);
    // The default stagger is 8 ms: 384 frames at 48 kHz.
    assert_eq!(strummed[1]["on_frame"], 384);
}

#[test]
fn a_strum_expands_like_its_written_lists_under_every_transform() {
    for place in [
        "",
        "count = 2; stretch = 3/2; transpose = 700ct;",
        "boundary = cut; groove = &lazy;",
    ] {
        assert_eq!(
            performed(&source(STRUMS, place)),
            performed(&source(LISTS, place)),
            "{place}"
        );
    }
    // A member override replaces the strummed values like any other.
    let edited = strikes(&events(&source(
        STRUMS,
        r#"count = 2; override late { event = "1/d.2"; set = { onset_offset = 50ms; velocity = 0.3; }; }"#,
    )));
    assert!(edited.contains(&("main/1/d.2".into(), "1/20".into(), "3/10".into())));
    assert!(edited.contains(&("main/0/d.2".into(), "2/125".into(), "18/25".into())));
}

#[test]
fn a_single_string_strum_is_its_velocity() {
    let one = events(&source(
        "  chord c { at = 0q; dur = 1q; pitches = [C4]; strum = up; velocity = 0.6; tilt = 0.3; }",
        "",
    ));
    assert_eq!(
        strikes(&one),
        [("main/0/c.0".into(), "0/1".into(), "3/5".into())]
    );
}

#[test]
fn malformed_strums_are_rejected() {
    let chord = |fields: &str| {
        source(
            &format!("  chord a {{ at = 0q; dur = 1q; pitches = [C4, E4, G4]; {fields} }}"),
            "",
        )
    };
    let has = |fields: &str, code: DiagnosticCode| {
        let codes = codes(&chord(fields));
        assert!(codes.contains(&code), "{fields}: {code:?} not in {codes:?}");
    };
    has("strum = sideways;", DiagnosticCode::Range);
    has("stagger = 8ms;", DiagnosticCode::Range);
    has("tilt = -0.1;", DiagnosticCode::Range);
    has("strum = down; stagger = -1ms;", DiagnosticCode::Range);
    has("strum = down; stagger = 1q;", DiagnosticCode::Unit);
    has("strum = down; tilt = 1ct;", DiagnosticCode::Unit);
    has(
        "strum = down; velocity = [0.9, 0.8, 0.7]; tilt = -0.1;",
        DiagnosticCode::Range,
    );
    // The first string struck keeps the velocity and the last takes the
    // whole tilt, so a tilt can leave [0, 1] only at the far end.
    has(
        "strum = down; velocity = 0.9; tilt = 0.2;",
        DiagnosticCode::Range,
    );
    has(
        "strum = up; velocity = 0.1; tilt = -0.2;",
        DiagnosticCode::Range,
    );
    assert!(compile_bundle_artifact(&SourceBundle::new(
        "main.maac",
        chord("strum = down; velocity = 0.9; tilt = 0.1;")
    ))
    .is_ok());
}

#[test]
fn strum_defaults_normalize_away() {
    let identity = |fields: &str| {
        let text = source(
            &format!("  chord a {{ at = 0q; dur = 1q; pitches = [C4, E4]; {fields} }}"),
            "",
        );
        let document = parse(&text).unwrap();
        let plan = compile(&document).unwrap();
        execution_identity(&document, &plan).unwrap().execution_hash
    };
    assert_eq!(
        identity("strum = down;"),
        identity("strum = down; stagger = 8ms; tilt = 0;")
    );
    assert_ne!(identity("strum = down;"), identity("strum = up;"));
    assert_ne!(identity("strum = down;"), identity(""));
}
