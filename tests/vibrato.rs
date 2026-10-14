//! Vibrato (§8.1): a `vibrato` named by a pitch expression lowers to the
//! curve points a written-out vibrato would list.

use maac::compiler::compile_bundle_artifact;
use maac::editing::{FoundationEditContext, Operation, SourceDocument, Transaction};
use maac::production_identity::execution_identity;
use maac::{compile, parse, DiagnosticCode, SourceBundle};
use serde_json::{json, Value};

/// One note `n` of `dur` on a sine at 120 bpm, placed by `main` with
/// `place`; `decls` adds top-level declarations and `expression` the note's
/// pitch-expression fields.
fn source(decls: &str, dur: &str, expression: &str, place: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 16q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 5ms; release = 50ms; level = 0.1; }}; }}
{decls}
pattern bar {{ length = 8q;
  note n {{ at = 0q; dur = {dur}; pitch = A4; velocity = 0.5; expression e {{ kind = pitch; {expression} }} }}
}}
track t {{ target = &synth:events; }}
place main {{ pattern = &bar; track = &t; at = 0q; {place} }}
"#
    )
}

fn events(source: &str) -> Vec<Value> {
    let plan = compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_or_else(|e| panic!("compiles: {e}"));
    let wire: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    wire["events"].as_array().unwrap().clone()
}

/// The pitch expression of `event` as `(position, cents, shape)`.
fn points_of(event: &Value) -> Vec<(String, String, String)> {
    let expression = &event["kind"]["pitch_expression"];
    assert_eq!(expression["clock"], "seconds");
    expression["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|point| {
            (
                point["position"].as_str().unwrap().to_owned(),
                point["cents"].as_str().unwrap().to_owned(),
                point["shape"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn points(source: &str) -> Vec<(String, String, String)> {
    points_of(&events(source)[0])
}

fn expected(points: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    points
        .iter()
        .map(|(p, c, s)| ((*p).to_owned(), (*c).to_owned(), (*s).to_owned()))
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
fn an_up_vibrato_rises_from_the_pitch_after_its_delay() {
    let source = source(
        "vibrato v { rate = 5Hz; depth = 40ct; delay = 100ms; shape = up; }",
        "1q",
        "vibrato = &v;",
        "",
    );
    // A 1q gate at 120 bpm is 1/2 s; the knots are a half period apart and
    // the last, at the gate's end, is a step.
    assert_eq!(
        points(&source),
        expected(&[
            ("0/1", "0/1", "step"),
            ("1/10", "0/1", "linear"),
            ("1/5", "40/1", "linear"),
            ("3/10", "0/1", "linear"),
            ("2/5", "40/1", "linear"),
            ("1/2", "0/1", "step"),
        ])
    );
}

#[test]
fn a_centered_vibrato_grows_over_its_rise() {
    let source = source(
        "vibrato v { rate = 5Hz; depth = 40ct; rise = 200ms; }",
        "1q",
        "vibrato = &v;",
        "",
    );
    // Centered is the default: half the depth either side, a quarter period
    // apart. The rise scales each knot until 200 ms.
    assert_eq!(
        points(&source),
        expected(&[
            ("0/1", "0/1", "linear"),
            ("1/20", "5/1", "linear"),
            ("1/10", "0/1", "linear"),
            ("3/20", "-15/1", "linear"),
            ("1/5", "0/1", "linear"),
            ("1/4", "20/1", "linear"),
            ("3/10", "0/1", "linear"),
            ("7/20", "-20/1", "linear"),
            ("2/5", "0/1", "linear"),
            ("9/20", "20/1", "linear"),
            ("1/2", "0/1", "step"),
        ])
    );
}

#[test]
fn a_vibrato_follows_its_curve_and_centres_on_where_it_ends() {
    let bend = "curve bend { clock = seconds; points = [(0ms, -200ct, step), (40ms, -200ct, linear), (100ms, 100ct, step)]; }";
    // The curve ends exactly at the delay: the first knot replaces its last point.
    let at_delay = source(
        &format!("{bend}\nvibrato v {{ rate = 5Hz; depth = 40ct; delay = 100ms; shape = up; }}"),
        "1q",
        "curve = &bend; vibrato = &v;",
        "",
    );
    assert_eq!(
        points(&at_delay),
        expected(&[
            ("0/1", "-200/1", "step"),
            ("1/25", "-200/1", "linear"),
            ("1/10", "100/1", "linear"),
            ("1/5", "140/1", "linear"),
            ("3/10", "100/1", "linear"),
            ("2/5", "140/1", "linear"),
            ("1/2", "100/1", "step"),
        ])
    );
    // Ending earlier, the curve's last value holds until the delay.
    let before = source(
        &format!("{bend}\nvibrato v {{ rate = 5Hz; depth = 40ct; delay = 300ms; shape = down; }}"),
        "1q",
        "curve = &bend; vibrato = &v;",
        "",
    );
    assert_eq!(
        points(&before),
        expected(&[
            ("0/1", "-200/1", "step"),
            ("1/25", "-200/1", "linear"),
            ("1/10", "100/1", "step"),
            ("3/10", "100/1", "linear"),
            ("2/5", "60/1", "linear"),
            ("1/2", "100/1", "step"),
        ])
    );
}

#[test]
fn a_cycle_curve_draws_any_waveform() {
    // A square cycle is a whole-step trill.
    let trill = source(
        "curve square { clock = normalized; points = [(0, 0, step), (1/2, 1, step), (1, 0, step)]; }\nvibrato trill { rate = 4Hz; depth = 200ct; shape = &square; }",
        "1q",
        "vibrato = &trill;",
        "",
    );
    assert_eq!(
        points(&trill),
        expected(&[
            ("0/1", "0/1", "step"),
            ("1/8", "200/1", "step"),
            ("1/4", "0/1", "step"),
            ("3/8", "200/1", "step"),
            ("1/2", "0/1", "step"),
        ])
    );
    // Uneven positions repeat in every cycle.
    let lopsided = source(
        "curve lean { clock = normalized; points = [(0, 0, linear), (1/5, 1, linear), (1, 0, step)]; }\nvibrato v { rate = 4Hz; depth = 10ct; shape = &lean; }",
        "1q",
        "vibrato = &v;",
        "",
    );
    assert_eq!(
        points(&lopsided),
        expected(&[
            ("0/1", "0/1", "linear"),
            ("1/20", "10/1", "linear"),
            ("1/4", "0/1", "linear"),
            ("3/10", "10/1", "linear"),
            ("1/2", "0/1", "step"),
        ])
    );
}

#[test]
fn a_vibrato_gives_the_plan_of_its_written_out_curve() {
    // T4's guitar vibrato with a constant swing: 38 cents up every 90 ms
    // from 300 ms, over a 2 s gate.
    let mut written = vec!["(0ms, 0ct, step)".to_owned()];
    let mut ms = 300;
    let mut up = false;
    loop {
        let last = ms >= 2000;
        let cents = if up { 38 } else { 0 };
        let shape = if last { "step" } else { "linear" };
        written.push(format!("({ms}ms, {cents}ct, {shape})"));
        if last {
            break;
        }
        ms += 90;
        up = !up;
    }
    let curve = source(
        &format!(
            "curve vib {{ clock = seconds; points = [{}]; }}",
            written.join(", ")
        ),
        "4q",
        "curve = &vib;",
        "",
    );
    let vibrato = source(
        "vibrato vib { rate = 50/9Hz; depth = 38ct; delay = 300ms; shape = up; }",
        "4q",
        "vibrato = &vib;",
        "",
    );
    let strip = |mut events: Vec<Value>| {
        for event in &mut events {
            event["source"].as_object_mut().unwrap().remove("span");
        }
        events
    };
    assert_eq!(written.len(), 21);
    assert_eq!(strip(events(&curve)), strip(events(&vibrato)));
}

#[test]
fn the_gate_sets_the_knot_count_and_nothing_else_moves_them() {
    let vibrato = "vibrato v { rate = 5Hz; depth = 40ct; shape = up; }";
    let last = |source: &str| points(source).last().unwrap().0.clone();
    assert_eq!(last(&source(vibrato, "1q", "vibrato = &v;", "")), "1/2");
    // Stretch doubles the gate; the knots stay 1/10 s apart.
    let stretched = points(&source(vibrato, "1q", "vibrato = &v;", "stretch = 2;"));
    assert_eq!(stretched.last().unwrap().0, "1/1");
    assert_eq!(stretched[1].0, "1/10");
    // A note shorter than the delay plays its curve alone.
    let late = source(
        "vibrato v { rate = 5Hz; depth = 40ct; delay = 1s; }",
        "1q",
        "vibrato = &v;",
        "",
    );
    assert_eq!(points(&late), expected(&[("0/1", "0/1", "step")]));
}

#[test]
fn every_member_of_a_chord_takes_the_vibrato() {
    let text = source(
        "vibrato v { rate = 5Hz; depth = 40ct; shape = up; }",
        "1q",
        "vibrato = &v;",
        "",
    )
    .replace(
        "note n { at = 0q; dur = 1q; pitch = A4; velocity = 0.5;",
        "chord n { at = 0q; dur = 1q; pitches = [A3, E4]; velocity = 0.5;",
    );
    let events = events(&text);
    assert_eq!(events.len(), 2);
    assert_eq!(points_of(&events[0]), points_of(&events[1]));
    assert_eq!(points_of(&events[0]).len(), 6);
}

#[test]
fn malformed_vibratos_are_rejected() {
    let with = |decls: &str, expression: &str| {
        codes(&source(
            &format!("curve bend {{ clock = seconds; points = [(0ms, -200ct, step), (150ms, 0ct, step)]; }}\ncurve slide {{ clock = normalized; points = [(0, -100ct, linear), (1, 0ct, step)]; }}\n{decls}"),
            "1q",
            expression,
            "",
        ))
    };
    let v = |fields: &str| format!("vibrato v {{ {fields} }}");
    let ok = "rate = 5Hz; depth = 40ct;";
    let has = |codes: Vec<DiagnosticCode>, code: DiagnosticCode| {
        assert!(codes.contains(&code), "{codes:?} lacks {code:?}");
    };
    has(with(&v(ok), "vibrato = &bend;"), DiagnosticCode::Reference);
    has(
        with(&v("rate = 0Hz; depth = 40ct;"), "vibrato = &v;"),
        DiagnosticCode::Range,
    );
    has(
        with(&v("rate = 5Hz; depth = -40ct;"), "vibrato = &v;"),
        DiagnosticCode::Range,
    );
    has(
        with(&v(&format!("{ok} delay = -1ms;")), "vibrato = &v;"),
        DiagnosticCode::Range,
    );
    has(
        with(&v(&format!("{ok} rise = -1ms;")), "vibrato = &v;"),
        DiagnosticCode::Range,
    );
    has(
        with(&v(&format!("{ok} shape = wobble;")), "vibrato = &v;"),
        DiagnosticCode::Range,
    );
    has(
        with(&v("depth = 40ct;"), "vibrato = &v;"),
        DiagnosticCode::Range,
    );
    has(
        with(&v("rate = 5ct; depth = 40ct;"), "vibrato = &v;"),
        DiagnosticCode::Unit,
    );
    // The bend ends at 150 ms, after a 100 ms delay.
    has(
        with(
            &v(&format!("{ok} delay = 100ms;")),
            "curve = &bend; vibrato = &v;",
        ),
        DiagnosticCode::Range,
    );
    // A vibrato is physical time; it joins a seconds-clock curve only.
    has(
        with(&v(ok), "curve = &slide; vibrato = &v;"),
        DiagnosticCode::Unit,
    );
    // Neither a curve nor a vibrato.
    has(with(&v(ok), ""), DiagnosticCode::Range);
    // Only pitch takes a vibrato.
    let gain = source(
        &v(ok),
        "1q",
        "curve = &bend;",
        "",
    )
    .replace(
        "expression e { kind = pitch; curve = &bend; }",
        "expression e { kind = gain; curve = &level; vibrato = &v; }",
    )
    .replace(
        "pattern bar",
        "curve level { clock = seconds; points = [(0ms, 1, step)]; }\ncurve bend { clock = seconds; points = [(0ms, 0ct, step)]; }\npattern bar",
    );
    has(codes(&gain), DiagnosticCode::Range);
}

#[test]
fn a_cycle_curve_must_be_a_closed_dimensionless_cycle() {
    let cycle = |points: &str, clock: &str| {
        codes(&source(
            &format!("curve c {{ clock = {clock}; points = [{points}]; }}\nvibrato v {{ rate = 5Hz; depth = 40ct; shape = &c; }}"),
            "1q",
            "vibrato = &v;",
            "",
        ))
    };
    let has = |codes: Vec<DiagnosticCode>, code: DiagnosticCode| {
        assert!(codes.contains(&code), "{codes:?} lacks {code:?}");
    };
    has(
        cycle("(0s, 0, linear), (1s, 0, step)", "seconds"),
        DiagnosticCode::Unit,
    );
    has(
        cycle("(0, 0ct, linear), (1, 0ct, step)", "normalized"),
        DiagnosticCode::Unit,
    );
    has(
        cycle("(0, 1, exponential), (1, 1, step)", "normalized"),
        DiagnosticCode::Range,
    );
    has(
        cycle("(0, 0, linear), (1, 1, step)", "normalized"),
        DiagnosticCode::Range,
    );
}

#[test]
fn vibrato_points_count_towards_the_point_budget() {
    // 20 kHz on a 2 s gate needs 80,000 knots, over the 65,536 limit.
    let source = source(
        "vibrato v { rate = 20kHz; depth = 1ct; shape = up; }",
        "4q",
        "vibrato = &v;",
        "",
    );
    assert!(codes(&source).contains(&DiagnosticCode::ResourceLimit));
}

#[test]
fn vibrato_defaults_normalize_and_its_fields_change_the_identity() {
    let identity = |vibrato: &str| {
        let text = source(vibrato, "1q", "vibrato = &v;", "");
        let document = parse(&text).unwrap();
        let plan = compile(&document).unwrap();
        execution_identity(&document, &plan).unwrap().execution_hash
    };
    let short = identity("vibrato v { rate = 5Hz; depth = 40ct; }");
    assert_eq!(
        short,
        identity(
            "vibrato v { rate = 5Hz; depth = 40ct; delay = 0s; rise = 0ms; shape = centered; }"
        )
    );
    assert_ne!(short, identity("vibrato v { rate = 5Hz; depth = 30ct; }"));
}

#[test]
fn a_vibrato_edit_changes_the_notes_that_name_it() {
    let text = source(
        "vibrato v { rate = 5Hz; depth = 40ct; shape = up; }",
        "1q",
        "vibrato = &v;",
        "",
    );
    let mut document = SourceDocument::parse(&text).unwrap();
    let transaction = Transaction::new(
        document.authored().revision().to_owned(),
        vec![Operation::Set {
            object: vec!["v".into()],
            field: vec!["depth".into()],
            value: json!({"t":"quantity","n":"30","d":"1","u":"ct"}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap();
    document
        .apply(&transaction, &FoundationEditContext)
        .unwrap();
    assert_eq!(
        points(document.source())[1],
        ("1/10".into(), "30/1".into(), "linear".into())
    );
}

#[test]
fn materializing_copies_the_vibrato_and_its_cycle() {
    let text = source(
        "curve lean { clock = normalized; points = [(0, 0, linear), (1/5, 1, linear), (1, 0, step)]; }\nvibrato v { rate = 4Hz; depth = 10ct; shape = &lean; }",
        "1q",
        "vibrato = &v;",
        "count = 2;",
    );
    let before = events(&text);
    let mut document = SourceDocument::parse(&text).unwrap();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "main", "copy")
        .unwrap();
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    let after = events(document.source());
    let performed = |events: Vec<Value>| -> Vec<Value> {
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
    assert_eq!(performed(before), performed(after));
}
