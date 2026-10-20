//! Connected notes (§8.3): which notes join, the plan's `join` field and its
//! validation, and the renderer handing a joined note to the voice it
//! continues.

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::{render_ports_artifact_span_with_limits, RenderSpan};
use maac::plan::{PlanLimits, PortRef};
use maac::{render_artifact_with_limits, DiagnosticCode, PlanArtifact, SourceBundle};
use serde_json::Value;

/// A song whose `strings` track plays `body` on a bowed section, and whose
/// `winds` track plays on a sine instrument, with `voices` each.
fn source(body: &str, voices: u32) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 16q]; rate = 48000Hz; tail = 1s; tempo = &clock; meter = &metre; output = &mix:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument bowed {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &section:out;
  node amp {{ type = "synth.adsr/1"; params = {{ sustain = 1; release = 400ms; }}; }}
  node section {{ type = "synth.bow/1"; config = {{ players = 2; }}; params = {{ level = 1/10; }}; }}
 }}
}}
instrument tone {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp {{ type = "synth.adsr/1"; params = {{ release = 400ms; }}; }}
  node osc {{ type = "synth.sine/1"; params = {{ level = 1/10; }}; }}
 }}
}}
node strings {{ instrument = &bowed; config = {{ voices = {voices}; }}; }}
node winds {{ instrument = &tone; config = {{ voices = {voices}; }}; }}
node mix {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
connect strings_out {{ from = &strings:out; to = &mix:in; }}
connect winds_out {{ from = &winds:out; to = &mix:in; }}
track strings_line {{ target = &strings:events; }}
track second_line {{ target = &strings:events; }}
track winds_line {{ target = &winds:events; }}
{body}
"#
    )
}

fn compile(body: &str, voices: u32) -> Result<PlanArtifact, maac::Diagnostics> {
    compile_bundle_artifact_with_limits(
        &SourceBundle::new("main.maac", source(body, voices)),
        &PlanLimits::song(),
    )
}

fn events(body: &str) -> Vec<Value> {
    let plan = compile(body, 8).unwrap_or_else(|error| panic!("{error:?}"));
    let wire: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    wire["events"].as_array().unwrap().clone()
}

/// Each event's address and the address it joins, in plan order.
fn joins(body: &str) -> Vec<(String, Option<String>)> {
    events(body)
        .iter()
        .map(|event| {
            (
                event["address"].as_str().unwrap().to_owned(),
                event["join"]["from"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

fn codes(body: &str) -> Vec<DiagnosticCode> {
    compile(body, 8)
        .unwrap_err()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

const LINE: &str = r#"pattern line { length = 8q;
  note a { at = 0q; dur = 2q; pitch = D5; velocity = 0.6; }
  note b { at = 2q; dur = 2q; pitch = A5; velocity = 0.7; slide = 120ms; }
  note c { at = 4q; dur = 1q; pitch = Bb5; velocity = 0.6; slur = true; }
  note d { at = 6q; dur = 2q; pitch = A5; velocity = 0.5; }
}
place first { pattern = &line; track = &strings_line; at = 0q; }"#;

#[test]
fn notes_that_touch_join_and_a_rest_lifts_the_bow() {
    assert_eq!(
        joins(LINE),
        vec![
            ("first/0/a".into(), None),
            ("first/0/b".into(), Some("first/0/a".into())),
            ("first/0/c".into(), Some("first/0/b".into())),
            // `c` ends at 5q, so `d` at 6q starts a new stroke.
            ("first/0/d".into(), None),
        ]
    );
    let events = events(LINE);
    assert_eq!(events[1]["join"]["slide_seconds"], "3/25");
    assert!(events[1]["join"].get("slur").is_none());
    assert_eq!(events[2]["join"]["slur"], true);
    assert!(events[2]["join"].get("slide_seconds").is_none());
}

#[test]
fn chords_join_member_by_member() {
    let body = r#"pattern pads { length = 8q;
  chord c1 { at = 0q; dur = 4q; pitches = [D4, F4, A4]; }
  chord c2 { at = 4q; dur = 4q; pitches = [D4, G4, Bb4]; slur = true; }
}
place pad { pattern = &pads; track = &strings_line; at = 0q; }"#;
    let joined: Vec<_> = joins(body)
        .into_iter()
        .filter_map(|(address, from)| from.map(|from| (address, from)))
        .collect();
    assert_eq!(
        joined,
        vec![
            ("pad/0/c2.0".into(), "pad/0/c1.0".into()),
            ("pad/0/c2.1".into(), "pad/0/c1.1".into()),
            ("pad/0/c2.2".into(), "pad/0/c1.2".into()),
        ]
    );
}

#[test]
fn only_one_track_and_one_destination_that_plays_lines_join() {
    // A second track into the same section does not join the first, and a
    // sine instrument plays every note with its own voice.
    let body = r#"pattern one { length = 2q; note a { at = 0q; dur = 2q; pitch = D5; } }
pattern two { length = 2q; note b { at = 0q; dur = 2q; pitch = E5; } }
place first { pattern = &one; track = &strings_line; at = 0q; }
place other { pattern = &two; track = &second_line; at = 2q; }
place wind_a { pattern = &one; track = &winds_line; at = 0q; }
place wind_b { pattern = &two; track = &winds_line; at = 2q; }"#;
    assert!(joins(body).iter().all(|(_, from)| from.is_none()));
    // The same pattern twice on one track joins across the placements.
    let body = r#"pattern one { length = 2q; note a { at = 0q; dur = 2q; pitch = D5; } }
place first { pattern = &one; track = &strings_line; at = 0q; count = 2; }"#;
    assert_eq!(
        joins(body),
        vec![
            ("first/0/a".into(), None),
            ("first/1/a".into(), Some("first/0/a".into())),
        ]
    );
}

#[test]
fn slur_and_slide_need_a_note_to_continue() {
    let alone = r#"pattern one { length = 4q;
  note a { at = 0q; dur = 1q; pitch = D5; }
  note b { at = 2q; dur = 1q; pitch = E5; slur = true; }
}
place first { pattern = &one; track = &strings_line; at = 0q; }"#;
    assert_eq!(codes(alone), vec![DiagnosticCode::Interval]);
    let slid = alone.replace("slur = true;", "slide = 80ms;");
    assert_eq!(codes(&slid), vec![DiagnosticCode::Interval]);
    let winds = r#"pattern one { length = 4q;
  note a { at = 0q; dur = 2q; pitch = D5; }
  note b { at = 2q; dur = 2q; pitch = E5; slur = true; }
}
place first { pattern = &one; track = &winds_line; at = 0q; }"#;
    assert_eq!(codes(winds), vec![DiagnosticCode::Capability]);
    for bad in [
        "slur = 1;",
        "slide = 251ms;",
        "slide = -1ms;",
        "slide = 1/10;",
    ] {
        let body = LINE.replace("slide = 120ms;", bad);
        assert!(compile(&body, 8).is_err(), "{bad}");
    }
}

#[test]
fn the_plan_validator_checks_every_join() {
    let plan = compile(LINE, 8).unwrap();
    let wire: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    // The plan as compiled reads back; each change below breaks only a join.
    PlanArtifact::from_json(&serde_json::to_vec(&wire).unwrap()).unwrap();
    for (field, value) in [
        ("from", Value::from("first/0/d")),
        ("from", Value::from("missing")),
        ("slide_seconds", Value::from("1/2")),
    ] {
        let mut broken = wire.clone();
        broken["events"][1]["join"][field] = value;
        let bytes = serde_json::to_vec(&broken).unwrap();
        let error = PlanArtifact::from_json(&bytes).unwrap_err();
        assert!(format!("{error:?}").contains("join"), "{field}: {error:?}");
    }
    // Two notes cannot continue the same one.
    let mut twice = wire.clone();
    twice["events"][2]["join"]["from"] = Value::from("first/0/a");
    let error = PlanArtifact::from_json(&serde_json::to_vec(&twice).unwrap()).unwrap_err();
    assert!(format!("{error:?}").contains("join"), "{error:?}");
}

fn render(plan: &PlanArtifact) -> maac::dsp::Result<Vec<f64>> {
    let mut out = Vec::new();
    render_artifact_with_limits(plan, &PlanLimits::song(), |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })?;
    Ok(out)
}

#[test]
fn a_joined_line_plays_on_one_voice() {
    // `a`, `b` and `c` share one voice, so one voice is enough until `d`
    // starts while `c` is still releasing.
    let body = r#"pattern line { length = 6q;
  note a { at = 0q; dur = 2q; pitch = D5; }
  note b { at = 2q; dur = 2q; pitch = A5; }
  note c { at = 4q; dur = 2q; pitch = Bb5; slur = true; }
}
place first { pattern = &line; track = &strings_line; at = 0q; }"#;
    let samples = render(&compile(body, 1).unwrap()).unwrap();
    assert!(samples.iter().all(|sample| sample.is_finite()));
    assert!(samples.iter().any(|sample| sample.abs() > 1e-3));
    // The same notes on the sine instrument overlap their release tails.
    let winds = body
        .replace("&strings_line", "&winds_line")
        .replace(" slur = true;", "");
    assert!(render(&compile(&winds, 1).unwrap()).is_err());
}

#[test]
fn a_render_range_that_starts_inside_a_line_plays_the_rest_of_it() {
    let plan = compile(LINE, 8).unwrap();
    let ports = [PortRef {
        node: "mix".into(),
        port: "out".into(),
    }];
    // From 1.5 s, inside `b`, whose `a` started before the range.
    let mut out = Vec::new();
    render_ports_artifact_span_with_limits(
        &plan,
        &PlanLimits::song(),
        &ports,
        RenderSpan {
            start: 72_000,
            end: 144_000,
        },
        |frame| {
            out.extend_from_slice(&frame[0]);
            Ok(())
        },
    )
    .unwrap();
    assert!(out.iter().any(|sample| sample.abs() > 1e-3));
}

#[test]
fn a_default_slur_or_slide_does_not_change_a_document_identity() {
    use maac::editing::{AuthoredDocument, FoundationEditContext};
    let normalized = |note: &str| {
        let source = format!(
            r#"maac 1;
project p {{ score=[0q,2q]; rate=48000Hz; tempo=&clock; meter=&metre; }}
tempo clock {{ points=[(0q,120bpm,step)]; }}
meter metre {{ points=[(0q,4,4)]; }}
pattern riff {{ length=1q; {note} }}
"#
        );
        let document = maac::parse(&source).unwrap();
        FoundationEditContext
            .normalize_document(&AuthoredDocument::from_document(&document).unwrap())
            .unwrap()
    };
    let plain = normalized("note n { at=0q; dur=1/2q; pitch=C4; }");
    assert_eq!(
        normalized("note n { at=0q; dur=1/2q; pitch=C4; slur=false; slide=0ms; }"),
        plain
    );
    assert_ne!(
        normalized("note n { at=0q; dur=1/2q; pitch=C4; slur=true; }"),
        plain
    );
}
