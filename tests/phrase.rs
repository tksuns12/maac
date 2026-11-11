//! `phrase`: a pattern child whose curves shape the dynamics, pressure and
//! timbre of the events under it (§9.3).

use maac::compiler::compile_bundle_artifact;
use maac::editing::{
    apply_transaction, AuthoredDocument, FoundationEditContext, Operation, SourceDocument,
    Transaction,
};
use maac::{parse, DiagnosticCode, SourceBundle};
use serde_json::{json, Value};

/// One instrument that takes pressure and timbre, the curves `extra`, the
/// patterns `patterns`, and the placements `places`.
fn source(extra: &str, patterns: &str, places: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 32q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument horn {{
  channels = 1;
  voice voice {{
    channels = 1; amplitude = &amp; output = &tone:out;
    node amp {{ type = "synth.adsr/1"; params = {{ attack = 5ms; sustain = 1; release = 50ms; }}; }}
    node tone {{ type = "synth.saw/1"; params = {{ level = 1/16; }}; }}
    node touch {{ type = "synth.pressure/1"; }}
    node colour {{ type = "synth.timbre/1"; }}
    modulate touch_level {{ from = &touch:out; to = &tone.params.level; depth = 1/16; }}
    modulate colour_level {{ from = &colour:out; to = &tone.params.level; depth = 1/32; }}
  }}
}}
node synth {{ instrument = &horn; config = {{ voices = 4; }}; }}
curve arc {{ clock = score; points = [(0q, 1/2, linear), (4q, 3/2, linear), (8q, 1, step)]; }}
curve breath {{ clock = score; points = [(0q, 0, linear), (4q, 4/5, step), (6q, 1/5, linear), (8q, 1/10, step)]; }}
curve level {{ clock = score; points = [(0q, 1/4, step)]; }}
curve own {{ clock = normalized; points = [(0, 1, step), (1, 1, step)]; }}
{extra}
{patterns}
track t {{ target = &synth:events; }}
{places}
"#
    )
}

const LINE: &str = r#"pattern line { length = 8q;
  note a { at = 0q; dur = 2q; pitch = C4; velocity = 4/5; }
  note b { at = 2q; dur = 3q; pitch = D4; velocity = 4/5; }
  note c { at = 5q; dur = 1q; pitch = E4; velocity = 1/2; }
  note d { at = 6q; dur = 2q; pitch = F4; velocity = 4/5; }
  phrase p { at = 0q; length = 8q; dynamics = &arc; pressure = &breath; }
}"#;

/// The velocities and breaths that `LINE`'s phrase gives, written by hand.
const LINE_BY_HAND: &str = r#"curve a_breath { clock = score; points = [(0q, 0, linear), (2q, 2/5, step)]; }
curve b_breath { clock = score; points = [(0q, 2/5, linear), (2q, 4/5, step), (3q, 4/5, step)]; }
curve c_breath { clock = score; points = [(0q, 4/5, step), (1q, 1/5, step)]; }
curve d_breath { clock = score; points = [(0q, 1/5, linear), (2q, 1/10, step)]; }
pattern line { length = 8q;
  note a { at = 0q; dur = 2q; pitch = C4; velocity = 2/5; expression e { kind = pressure; curve = &a_breath; } }
  note b { at = 2q; dur = 3q; pitch = D4; velocity = 4/5; expression e { kind = pressure; curve = &b_breath; } }
  note c { at = 5q; dur = 1q; pitch = E4; velocity = 11/16; expression e { kind = pressure; curve = &c_breath; } }
  note d { at = 6q; dur = 2q; pitch = F4; velocity = 1; expression e { kind = pressure; curve = &d_breath; } }
}"#;

const TWICE: &str = "place main { pattern = &line; track = &t; at = 0q; count = 2; }";

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

fn event<'e>(events: &'e [Value], address: &str) -> &'e Value {
    events
        .iter()
        .find(|e| e["address"] == address)
        .unwrap_or_else(|| panic!("no event {address}"))
}

/// A plan rational as written in source: `2/1` is `2`.
fn exact(value: &Value) -> String {
    let text = value.as_str().unwrap();
    text.strip_suffix("/1").unwrap_or(text).to_owned()
}

fn velocity(events: &[Value], address: &str) -> String {
    exact(&event(events, address)["kind"]["velocity"])
}

/// A note's expression of `kind` as (position, value, shape) strings.
fn expression(events: &[Value], address: &str, kind: &str) -> Vec<(String, String, String)> {
    let expression = &event(events, address)["kind"][format!("{kind}_expression")];
    expression["points"]
        .as_array()
        .unwrap_or_else(|| panic!("{address} has no {kind} expression"))
        .iter()
        .map(|point| {
            (
                exact(&point["position"]),
                exact(&point["value"]),
                point["shape"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn points(spec: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    spec.iter()
        .map(|(p, v, s)| ((*p).into(), (*v).into(), (*s).into()))
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
fn a_phrase_lowers_to_the_velocities_and_breaths_written_by_hand() {
    // c: 1/2 × 11/8 = 11/16; d: 4/5 × 5/4 is clamped to 1.
    assert_eq!(
        events(&source("", LINE, TWICE)),
        events(&source("", LINE_BY_HAND, TWICE))
    );
}

#[test]
fn notes_of_one_phrase_read_one_breath() {
    let events = events(&source("", LINE, TWICE));
    // b starts where a's slice ends, and the step at 4q falls inside b.
    assert_eq!(
        expression(&events, "main/0/a", "pressure")
            .last()
            .unwrap()
            .1,
        expression(&events, "main/0/b", "pressure")[0].1
    );
    assert_eq!(
        expression(&events, "main/1/b", "pressure"),
        points(&[
            ("0", "2/5", "linear"),
            ("2", "4/5", "step"),
            ("3", "4/5", "step"),
        ])
    );
}

#[test]
fn a_notes_own_expression_takes_precedence() {
    let own = LINE.replace(
        "velocity = 1/2; }",
        "velocity = 1/2; expression e { kind = pressure; curve = &own; } }",
    );
    let events = events(&source("", &own, TWICE));
    assert_eq!(
        expression(&events, "main/0/c", "pressure"),
        points(&[("0", "1", "step"), ("1", "1", "step")])
    );
    // Its velocity is still shaped.
    assert_eq!(velocity(&events, "main/0/c"), "11/16");
}

#[test]
fn a_stretched_phrase_stretches_its_curves() {
    let stretched = "place main { pattern = &line; track = &t; at = 0q; count = 2; stretch = 2; }";
    let events = events(&source("", LINE, stretched));
    assert_eq!(velocity(&events, "main/1/a"), "2/5");
    assert_eq!(
        expression(&events, "main/1/a", "pressure"),
        points(&[("0", "0", "linear"), ("4", "2/5", "step")])
    );
}

#[test]
fn the_nearest_phrase_gives_the_breath_and_every_phrase_the_dynamics() {
    let nested = format!(
        r#"{LINE}
pattern song {{ length = 16q;
  use twice {{ pattern = &line; at = 0q; count = 2; }}
  phrase whole {{ at = 0q; length = 16q; dynamics = &arc; pressure = &level; timbre = &level; }}
}}"#
    );
    let place = "place main { pattern = &song; track = &t; at = 0q; }";
    let events = events(&source("", &nested, place));
    // The inner phrase's breath; the outer phrase alone gives timbre.
    assert_eq!(
        expression(&events, "main/0/twice/1/a", "pressure"),
        points(&[("0", "0", "linear"), ("2", "2/5", "step")])
    );
    assert_eq!(
        expression(&events, "main/0/twice/1/a", "timbre"),
        points(&[("0", "1/4", "step"), ("2", "1/4", "step")])
    );
    // At 8q of the song the outer arc is 1, and at 0q of the line 1/2.
    assert_eq!(velocity(&events, "main/0/twice/1/a"), "2/5");
    // At 5q both arcs are 11/8: 1/2 × 121/64, with no clamp in between.
    assert_eq!(velocity(&events, "main/0/twice/0/c"), "121/128");
    // At 6q both are 5/4: 4/5 × 25/16 is clamped.
    assert_eq!(velocity(&events, "main/0/twice/0/d"), "1");
}

#[test]
fn the_later_phrase_gives_the_breath_where_two_overlap() {
    let overlapping = LINE.replace(
        "phrase p {",
        "phrase q { at = 2q; length = 4q; pressure = &level; }\n  phrase p {",
    );
    let overlapping = events(&source("", &overlapping, TWICE));
    assert_eq!(expression(&overlapping, "main/0/a", "pressure")[0].1, "0");
    assert_eq!(
        expression(&overlapping, "main/0/b", "pressure"),
        points(&[("0", "1/4", "step"), ("3", "1/4", "step")])
    );
    // Same start: the ID that sorts last.
    let tied = LINE.replace(
        "phrase p {",
        "phrase o { at = 0q; length = 8q; pressure = &level; }\n  phrase p {",
    );
    let tied = events(&source("", &tied, TWICE));
    assert_eq!(expression(&tied, "main/0/a", "pressure")[0].1, "0");
}

#[test]
fn phrases_reach_chord_members_and_spare_inserts() {
    let chords = r#"pattern line { length = 4q;
  chord k { at = 0q; dur = 1q; pitches = [C4, E4]; velocity = [4/5, 2/5]; }
  phrase p { at = 0q; length = 4q; dynamics = &arc; }
}"#;
    let place = r#"place main { pattern = &line; track = &t; at = 0q;
  insert extra { note n { at = 1q; dur = 1q; pitch = G4; velocity = 4/5; } }
}"#;
    let events = events(&source("", chords, place));
    assert_eq!(velocity(&events, "main/0/k.0"), "2/5");
    assert_eq!(velocity(&events, "main/0/k.1"), "1/5");
    assert_eq!(velocity(&events, "main/extra/n"), "4/5");
}

#[test]
fn an_override_sets_the_final_velocity() {
    let place = r#"place main { pattern = &line; track = &t; at = 0q; count = 2;
  override loud { event = "0/a"; set = { velocity = 9/10; }; }
}"#;
    let events = events(&source("", LINE, place));
    assert_eq!(velocity(&events, "main/0/a"), "9/10");
    assert_eq!(velocity(&events, "main/1/a"), "2/5");
}

#[test]
fn phrase_errors() {
    let with = |phrase: &str, extra: &str| {
        let pattern = format!(
            "pattern line {{ length = 8q; note a {{ at = 0q; dur = 1q; pitch = C4; }} {phrase} }}"
        );
        codes(&source(extra, &pattern, TWICE))
    };
    let seconds = "curve held { clock = seconds; points = [(0s, 1/2, step)]; }";
    assert!(with(
        "phrase p { at = 0q; length = 8q; pressure = &held; }",
        seconds
    )
    .contains(&DiagnosticCode::Unit));
    let curved = "curve bent { clock = score; points = [(0q, 1/2, exponential), (1q, 1, step)]; }";
    assert!(with(
        "phrase p { at = 0q; length = 8q; dynamics = &bent; }",
        curved
    )
    .contains(&DiagnosticCode::Range));
    let over = "curve over { clock = score; points = [(0q, 3/2, step)]; }";
    assert!(
        with("phrase p { at = 0q; length = 8q; pressure = &over; }", over)
            .contains(&DiagnosticCode::Range)
    );
    let hertz = "curve hz { clock = score; points = [(0q, 100Hz, step)]; }";
    assert!(
        with("phrase p { at = 0q; length = 8q; dynamics = &hz; }", hertz)
            .contains(&DiagnosticCode::Unit)
    );
    assert!(with("phrase p { at = 0q; length = 8q; }", "").contains(&DiagnosticCode::Range));
    assert!(
        with("phrase p { at = 0q; length = 0q; dynamics = &arc; }", "")
            .contains(&DiagnosticCode::Range)
    );
    assert!(
        with("phrase p { at = 8q; length = 1q; dynamics = &arc; }", "")
            .contains(&DiagnosticCode::Interval)
    );
    assert!(with(
        "phrase p { at = 0q; length = 8q; dynamics = &missing; }",
        ""
    )
    .contains(&DiagnosticCode::Reference));
    assert!(with(
        "phrase p { at = 0q; length = 8q; dynamics = &arc; note x { at = 0q; dur = 1q; pitch = C4; } }",
        ""
    )
    .contains(&DiagnosticCode::Capability));
}

#[test]
fn a_destination_without_pressure_refuses_a_phrases_breath() {
    let sine = r#"maac 1;
project p { score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node synth { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 0.1; }; }
curve level { clock = score; points = [(0q, 1/4, step)]; }
pattern line { length = 4q;
  note a { at = 0q; dur = 1q; pitch = C4; }
  phrase p { at = 0q; length = 4q; pressure = &level; }
}
track t { target = &synth:events; }
place main { pattern = &line; track = &t; at = 0q; }
"#;
    assert!(codes(sine).contains(&DiagnosticCode::Capability));
}

/// `patterns` and `places` on a sine, which the foundation edit context
/// can edit; phrases there shape dynamics only.
fn sine_source(patterns: &str, places: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 32q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = 0.1; }}; }}
curve arc {{ clock = score; points = [(0q, 1/2, linear), (4q, 3/2, linear), (8q, 1, step)]; }}
{patterns}
track t {{ target = &synth:events; }}
{places}
"#
    )
}

#[test]
fn editing_a_phrase_reports_every_event_it_may_shape() {
    let text = sine_source(&LINE.replace(" pressure = &breath;", ""), TWICE);
    let mut document = AuthoredDocument::from_document(&parse(&text).unwrap()).unwrap();
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["line".into(), "p".into()],
            field: vec!["length".into()],
            value: json!({"t":"quantity","n":"4","d":"1","u":"q"}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap();
    let applied = apply_transaction(&mut document, &transaction, &FoundationEditContext).unwrap();
    assert_eq!(
        applied.impact.affected_expanded_event_addresses.len(),
        8,
        "{:?}",
        applied.impact.affected_expanded_event_addresses
    );
}

#[test]
fn a_materialized_instance_keeps_its_phrases() {
    let nested = format!(
        r#"{LINE}
pattern song {{ length = 16q;
  use twice {{ pattern = &line; at = 0q; count = 2; }}
  phrase whole {{ at = 0q; length = 16q; dynamics = &arc; }}
}}"#,
        LINE = LINE.replace(" pressure = &breath;", "")
    );
    let text = sine_source(
        &nested,
        "place main { pattern = &song; track = &t; at = 0q; }",
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
        assert_eq!(
            event["kind"],
            after.iter().find(|e| e["address"] == new).unwrap()["kind"],
            "event {old}"
        );
    }
}

/// Every diagnostic of `line` with `phrase` beside one note.
fn mistakes(phrase: &str) -> Vec<maac::Diagnostic> {
    let pattern = format!(
        "pattern line {{ length = 8q; note a {{ at = 0q; dur = 1q; pitch = C4; }} {phrase} }}"
    );
    let extra = "curve held { clock = seconds; points = [(0s, 1/2, step)]; }\ncurve over { clock = score; points = [(0q, 3/2, step)]; }";
    let text = source(extra, &pattern, TWICE);
    compile_bundle_artifact(&SourceBundle::new("main.maac", text.as_str()))
        .unwrap_err()
        .into_vec()
}

/// Each mistake is reported once, at its field of the phrase.
#[test]
fn a_wrong_phrase_is_reported_at_its_field() {
    for (fields, code, message, field) in [
        (
            "at = -1q; length = 8q; dynamics = &arc;",
            DiagnosticCode::Range,
            "phrase.at must be nonnegative",
            Some("at"),
        ),
        (
            "at = 0q; length = 0q; dynamics = &arc;",
            DiagnosticCode::Range,
            "phrase.length must be positive",
            Some("length"),
        ),
        (
            "at = 0q; length = 8q;",
            DiagnosticCode::Range,
            "phrase requires `dynamics`, `pressure` or `timbre`",
            None,
        ),
        (
            "at = 0q; length = 8q; pressure = &held;",
            DiagnosticCode::Unit,
            "phrase curves use the score clock",
            Some("pressure"),
        ),
        (
            "at = 0q; length = 8q; timbre = &over;",
            DiagnosticCode::Range,
            "phrase pressure and timbre values must be within 0..=1",
            Some("timbre"),
        ),
        (
            "length = 8q; dynamics = &arc;",
            DiagnosticCode::Range,
            "phrase requires field `at`",
            Some("at"),
        ),
        (
            "at = 0q; length = 8q; dynamics = &arc; swell = 1;",
            DiagnosticCode::UnknownField,
            "unknown field `swell` on phrase",
            Some("swell"),
        ),
    ] {
        let diagnostics = mistakes(&format!("phrase p {{ {fields} }}"));
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("{fields}: expected one diagnostic, got {diagnostics:#?}");
        };
        assert_eq!(diagnostic.code, code, "{fields}");
        assert_eq!(diagnostic.message, message, "{fields}");
        assert_eq!(diagnostic.object_path, ["line", "p"], "{fields}");
        let field_path: Vec<&str> = field.into_iter().collect();
        assert_eq!(diagnostic.field_path, field_path, "{fields}");
    }
}

#[test]
fn every_mistake_in_a_phrase_is_reported_once() {
    let fields: Vec<String> = mistakes(
        "phrase p { at = -1q; length = 0q; dynamics = &over; pressure = &held; timbre = &over; }",
    )
    .iter()
    .map(|diagnostic| diagnostic.field_path[0].clone())
    .collect();
    assert_eq!(fields, ["at", "length", "pressure", "timbre"]);
}

/// A phrase's ID is its own only within its pattern.
#[test]
fn phrases_that_share_an_id_in_two_patterns_keep_their_own_curves() {
    let nested = format!(
        r#"{LINE}
pattern song {{ length = 16q;
  use twice {{ pattern = &line; at = 0q; count = 2; }}
  phrase p {{ at = 8q; length = 8q; timbre = &level; }}
}}"#
    );
    let place = "place main { pattern = &song; track = &t; at = 0q; }";
    let events = events(&source("", &nested, place));
    // The line's own `p` gives every pass its breath and its dynamics.
    assert_eq!(velocity(&events, "main/0/twice/0/a"), "2/5");
    assert_eq!(velocity(&events, "main/0/twice/1/a"), "2/5");
    assert_eq!(
        expression(&events, "main/0/twice/1/a", "pressure"),
        points(&[("0", "0", "linear"), ("2", "2/5", "step")])
    );
    // The song's `p` gives the second pass alone its timbre.
    assert_eq!(
        expression(&events, "main/0/twice/1/a", "timbre"),
        points(&[("0", "1/4", "step"), ("2", "1/4", "step")])
    );
    assert!(event(&events, "main/0/twice/0/a")["kind"]["timbre_expression"].is_null());
}
