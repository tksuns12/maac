//! `groove`: a named, exact swing map that placements reference.

use maac::compiler::compile_bundle_artifact;
use maac::{DiagnosticCode, SourceBundle};
use serde_json::Value;

/// Two 4/4 bars (then `meter_tail`), with `pattern` placed by `place`.
fn source(meter_tail: &str, pattern: &str, place: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 16q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4){meter_tail}]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = 0.1; }}; }}
groove lazy {{ grid = 1/2q; ratio = 2/3; }}
pattern beat {{ length = 4q;
{pattern}
}}
track t {{ target = &synth:events; }}
{place}
"#
    )
}

const PLACE: &str =
    "place main { pattern = &beat; track = &t; at = 0q; count = 2; groove = &lazy; }";
const STRAIGHT: &str = "place main { pattern = &beat; track = &t; at = 0q; count = 2; }";

/// Straight eighths and sixteenths, as an author writes them under a groove.
const EVEN: &str = r#"  note n0 { at = 0q; dur = 1/2q; pitch = C4; }
  note n1 { at = 1/2q; dur = 1/4q; pitch = D4; }
  note n2 { at = 7/4q; dur = 1/4q; pitch = E4; }
  note n3 { at = 2q; dur = 2q; pitch = F4; }"#;

/// The same notes at the positions "Late Window" calculated by hand.
const SWUNG: &str = r#"  note n0 { at = 0q; dur = 2/3q; pitch = C4; }
  note n1 { at = 2/3q; dur = 1/6q; pitch = D4; }
  note n2 { at = 11/6q; dur = 1/6q; pitch = E4; }
  note n3 { at = 2q; dur = 2q; pitch = F4; }"#;

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

fn on_off(events: &[Value], address: &str) -> (String, String) {
    let event = events.iter().find(|e| e["address"] == address).unwrap();
    (
        event["score_on_q"].as_str().unwrap().to_owned(),
        event["score_off_q"].as_str().unwrap().to_owned(),
    )
}

fn codes(source: &str) -> Vec<DiagnosticCode> {
    compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_err()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn straight_writing_under_a_groove_lands_on_the_hand_swung_grid() {
    let grooved = events(&source("", EVEN, PLACE));
    assert_eq!(grooved, events(&source("", SWUNG, STRAIGHT)));
    // Second repetition: the map restarts at the bar line.
    assert_eq!(
        on_off(&grooved, "main/1/n1"),
        ("14/3".into(), "29/6".into())
    );
}

#[test]
fn ratio_one_half_is_the_identity() {
    let straight = source("", EVEN, STRAIGHT);
    let neutral = source("", EVEN, PLACE).replace("ratio = 2/3", "ratio = 1/2");
    assert_eq!(events(&neutral), events(&straight));
}

#[test]
fn overrides_and_inserts_keep_their_final_positions() {
    let place = PLACE.replace(
        " }",
        r#"
  override fixed { event = "0/n1"; set = { at = 1/2q; }; }
  override louder { event = "1/n1"; set = { velocity = 0.5; }; }
  insert extra { note x { at = 5/2q; dur = 1/4q; pitch = G4; } }
}"#,
    );
    let events = events(&source("", EVEN, &place));
    // `set.at` is final; its gate keeps the grooved duration.
    assert_eq!(on_off(&events, "main/0/n1").0, "1/2");
    // An override of another field keeps the grooved position.
    assert_eq!(on_off(&events, "main/1/n1").0, "14/3");
    // Inserts are in final coordinates.
    assert_eq!(
        on_off(&events, "main/extra/x"),
        ("5/2".into(), "11/4".into())
    );
}

#[test]
fn a_delay_lays_every_event_back_as_offsets_would() {
    let laid_back = source("", EVEN, PLACE).replace("ratio = 2/3;", "ratio = 2/3; delay = 18ms;");
    let by_hand = source(
        "",
        &SWUNG.replace(
            "pitch =",
            "onset_offset = 18ms; release_offset = 18ms; pitch =",
        ),
        STRAIGHT,
    );
    assert_eq!(events(&laid_back), events(&by_hand));
    // Pushing ahead is a negative delay; the first note may not leave the score.
    let pushed = source("", EVEN, &PLACE.replace("at = 0q", "at = 4q"))
        .replace("ratio = 2/3;", "ratio = 2/3; delay = -10ms;");
    let event = events(&pushed)
        .into_iter()
        .find(|e| e["address"] == "main/1/n0")
        .unwrap();
    assert_eq!(event["onset_offset_seconds"], "-1/100");
}

#[test]
fn a_delay_spares_inserts_and_yields_to_an_override() {
    let place = PLACE.replace(
        " }",
        r#"
  override exact { event = "0/n1"; set = { onset_offset = 5ms; }; }
  insert extra { note x { at = 5/2q; dur = 1/4q; pitch = G4; } }
}"#,
    );
    let source = source("", EVEN, &place).replace("ratio = 2/3;", "ratio = 2/3; delay = 18ms;");
    let events = events(&source);
    let offset = |address: &str| {
        events.iter().find(|e| e["address"] == address).unwrap()["onset_offset_seconds"].clone()
    };
    assert_eq!(offset("main/0/n0"), "9/500");
    assert_eq!(offset("main/0/n1"), "1/200");
    assert_eq!(offset("main/extra/x"), "0/1");
}

#[test]
fn adjacent_gates_stay_adjacent() {
    // Back-to-back eighths remain back to back after the map.
    let legato = r#"  note a { at = 0q; dur = 1/2q; pitch = C4; }
  note b { at = 1/2q; dur = 1/2q; pitch = D4; }"#;
    let events = events(&source("", legato, PLACE));
    assert_eq!(on_off(&events, "main/0/a"), ("0/1".into(), "2/3".into()));
    assert_eq!(on_off(&events, "main/0/b"), ("2/3".into(), "1/1".into()));
}

#[test]
fn a_bar_the_grid_does_not_divide_is_an_error() {
    // Bar 3 is 7/8: 7/2q is not a whole number of 1q pairs.
    let meter = ", (8q, 7, 8)";
    let place = "place main { pattern = &beat; track = &t; at = 8q; groove = &lazy; }";
    assert_eq!(
        codes(&source(
            meter,
            "  note n { at = 1/2q; dur = 1/4q; pitch = C4; }",
            place
        )),
        vec![DiagnosticCode::Range]
    );
    // A note on the bar line does not move and needs no fit.
    let on_line = source(
        meter,
        "  note n { at = 0q; dur = 7/2q; pitch = C4; }",
        place,
    );
    assert_eq!(on_off(&events(&on_line), "main/0/n").0, "8/1");
}

#[test]
fn malformed_grooves_are_rejected() {
    let with = |groove: &str, place: &str| {
        source("", EVEN, place).replace("groove lazy { grid = 1/2q; ratio = 2/3; }", groove)
    };
    let has = |source: String, code: DiagnosticCode| {
        let codes = codes(&source);
        assert!(codes.contains(&code), "{code:?} not in {codes:?}");
    };
    has(
        with("groove lazy { grid = 1/2q; ratio = 4/5; }", PLACE),
        DiagnosticCode::Range,
    );
    has(
        with("groove lazy { grid = 1/2q; ratio = 2/5; }", PLACE),
        DiagnosticCode::Range,
    );
    has(
        with("groove lazy { grid = 0q; ratio = 2/3; }", PLACE),
        DiagnosticCode::Range,
    );
    has(
        with("groove lazy { grid = 1s; ratio = 2/3; }", PLACE),
        DiagnosticCode::Unit,
    );
    has(
        with("groove lazy { grid = 1/2q; }", PLACE),
        DiagnosticCode::Range,
    );
    has(
        with(
            "groove lazy { grid = 1/2q; ratio = 2/3; delay = 60ms; }",
            PLACE,
        ),
        DiagnosticCode::Range,
    );
    has(
        with(
            "groove lazy { grid = 1/2q; ratio = 2/3; delay = 1/8q; }",
            PLACE,
        ),
        DiagnosticCode::Unit,
    );
    has(
        with(
            "groove lazy { grid = 1/2q; ratio = 2/3; swing = 1; }",
            PLACE,
        ),
        DiagnosticCode::UnknownField,
    );
    has(
        with(
            "groove lazy { grid = 1/2q; ratio = 2/3; }",
            &PLACE.replace("&lazy", "&beat"),
        ),
        DiagnosticCode::Reference,
    );
}

/// Each wrong field is reported at that field, with the range it must keep.
#[test]
fn a_wrong_groove_is_reported_at_its_field() {
    for (groove, code, message, field) in [
        (
            "grid = 1/2q; ratio = 4/5;",
            DiagnosticCode::Range,
            "groove.ratio must be from 1/2 through 3/4",
            "ratio",
        ),
        (
            "grid = 0q; ratio = 2/3;",
            DiagnosticCode::Range,
            "groove.grid must be positive",
            "grid",
        ),
        (
            "grid = 1/2q; ratio = 2/3; delay = 51ms;",
            DiagnosticCode::Range,
            "groove.delay must be from -50ms to 50ms",
            "delay",
        ),
        (
            "grid = 1/2q;",
            DiagnosticCode::Range,
            "groove requires field `ratio`",
            "ratio",
        ),
        (
            "grid = 1/2q; ratio = 2/3; swing = 1;",
            DiagnosticCode::UnknownField,
            "unknown field `swing` on groove",
            "swing",
        ),
    ] {
        let text = source("", EVEN, PLACE).replace(
            "groove lazy { grid = 1/2q; ratio = 2/3; }",
            &format!("groove lazy {{ {groove} }}"),
        );
        let diagnostics = compile_bundle_artifact(&SourceBundle::new("main.maac", text.as_str()))
            .unwrap_err()
            .into_vec();
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("{groove}: expected one diagnostic, got {diagnostics:#?}");
        };
        assert_eq!(diagnostic.code, code, "{groove}");
        assert_eq!(diagnostic.message, message, "{groove}");
        assert_eq!(diagnostic.object_path, ["lazy"], "{groove}");
        assert_eq!(diagnostic.field_path, [field], "{groove}");
    }
}

/// A groove that no placement names is checked like one that is.
#[test]
fn every_mistake_in_a_groove_is_reported_once() {
    let text = source("", EVEN, STRAIGHT).replace(
        "groove lazy { grid = 1/2q; ratio = 2/3; }",
        "groove lazy { grid = 0q; ratio = 1; delay = 1s; }",
    );
    let diagnostics = compile_bundle_artifact(&SourceBundle::new("main.maac", text.as_str()))
        .unwrap_err()
        .into_vec();
    let fields: Vec<&str> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.field_path[0].as_str())
        .collect();
    assert_eq!(fields, ["delay", "grid", "ratio"], "{diagnostics:#?}");
}
