use maac::plan::{AutomationClock, EventKind, Plan};
use maac::{compile, parse, DiagnosticCode};
use num_rational::BigRational;

fn rational(numerator: i64, denominator: i64) -> BigRational {
    BigRational::new(numerator.into(), denominator.into())
}

fn base(extra: &str, score: &str, tempo_points: &str, meter_points: &str) -> String {
    format!(
        r#"maac 1;
project p {{
  score = [{score}];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &sum:out;
}}
tempo clock {{ points = [{tempo_points}]; }}
meter metre {{ points = [{meter_points}]; }}
node sine {{ type = "core.sine/1"; config = {{ voices = 8; }}; }}
node sum {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
connect sine_sum {{ from = &sine:out; to = &sum:in; }}
track t {{ target = &sine:events; }}
{extra}"#
    )
}

fn compile_source(source: &str) -> Plan {
    let document = parse(source).expect("foundation source must parse");
    compile(&document).expect("foundation source must compile")
}

fn event<'a>(plan: &'a Plan, address: &str) -> &'a maac::plan::ResolvedEvent {
    plan.events
        .iter()
        .find(|event| event.address == address)
        .unwrap_or_else(|| panic!("missing event {address}"))
}

fn pitch_hz(event: &maac::plan::ResolvedEvent) -> f64 {
    match event.kind {
        EventKind::Note { pitch_hz, .. } => pitch_hz,
        _ => panic!("expected note event"),
    }
}

fn diagnostic_codes(source: &str) -> Vec<DiagnosticCode> {
    let document = parse(source).expect("failure source must parse");
    compile(&document)
        .expect_err("source should fail")
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn thirds_and_step_tempo_changes_schedule_exact_frames() {
    let source = base(
        r#"
pattern thirds {
  length = 3q;
  note before { at = 1/3q; dur = 1/3q; pitch = C4; }
  note after { at = 7/3q; dur = 1/3q; pitch = C4; }
}
place p0 { pattern = &thirds; track = &t; at = 0q; }
"#,
        "0q, 3q",
        "(0q, 120bpm, step), (2q, 60bpm, step)",
        "(0q, 4, 4)",
    );
    let plan = compile_source(&source);
    let before = event(&plan, "p0/0/before");
    assert_eq!(before.score_on_q, rational(1, 3));
    assert_eq!(before.score_off_q.as_ref(), Some(&rational(2, 3)));
    assert_eq!(before.on_seconds, rational(1, 6));
    assert_eq!(before.off_seconds.as_ref(), Some(&rational(1, 3)));
    assert_eq!(before.on_frame, 8_000);
    assert_eq!(before.off_frame, Some(16_000));

    let after = event(&plan, "p0/0/after");
    assert_eq!(after.score_on_q, rational(7, 3));
    assert_eq!(after.on_seconds, rational(4, 3));
    assert_eq!(after.on_frame, 64_000);
    assert_eq!(after.off_frame, Some(80_000));
}

#[test]
fn negative_pickup_and_meter_bar_position_share_the_exact_origin() {
    let source = base(
        r#"
pattern pickup { length = 1q; note n { at = 0q; dur = 1/2q; pitch = C4; } }
place pickup_at_bar { pattern = &pickup; track = &t; at = bar(0, 1); }
"#,
        "-4q, 8q",
        "(0q, 120bpm, step)",
        "(0q, 4, 4), (4q, 3, 4)",
    );
    let plan = compile_source(&source);
    let note = event(&plan, "pickup_at_bar/0/n");
    assert_eq!(note.score_on_q, rational(-4, 1));
    assert_eq!(note.on_seconds, rational(-2, 1));
    assert_eq!(note.on_frame, 0);
    assert_eq!(note.off_frame, Some(12_000));
    assert_eq!(plan.output.score_start_q, rational(-4, 1));
    assert_eq!(plan.output.score_end_q, rational(8, 1));
}

#[test]
fn pitch_constructors_and_down_transposed_high_base_resolve_at_the_receiver() {
    let source = base(
        r#"
tuning tiny {
  period = 1200ct;
  steps = [0ct, 600ct];
  reference_index = 0;
  reference_frequency = 440Hz;
}
pattern pitches {
  length = 5q;
  note spelled { at = 0q; dur = 1/4q; pitch = C#4; }
  note keyed { at = 1q; dur = 1/4q; pitch = key(60); }
  note degree { at = 2q; dur = 1/4q; pitch = degree(-1, &tiny); }
  note ratio { at = 3q; dur = 1/4q; pitch = ratio(3/2, 440Hz); }
  note hz { at = 4q; dur = 1/4q; pitch = 440Hz; }
}
place pitches_at_zero { pattern = &pitches; track = &t; at = 0q; }
pattern high { length = 1q; note n { at = 0q; dur = 1/4q; pitch = key(140); } }
place high_down { pattern = &high; track = &t; at = 0q; transpose = -2400ct; }
"#,
        "0q, 5q",
        "(0q, 120bpm, step)",
        "(0q, 4, 4)",
    );
    let plan = compile_source(&source);
    assert!((pitch_hz(event(&plan, "pitches_at_zero/0/spelled")) - 277.1826309768721).abs() < 1e-9);
    assert!((pitch_hz(event(&plan, "pitches_at_zero/0/keyed")) - 261.6255653005986).abs() < 1e-9);
    assert!((pitch_hz(event(&plan, "pitches_at_zero/0/degree")) - 311.1269837220809).abs() < 1e-9);
    assert!((pitch_hz(event(&plan, "pitches_at_zero/0/ratio")) - 660.0).abs() < 1e-9);
    assert!((pitch_hz(event(&plan, "pitches_at_zero/0/hz")) - 440.0).abs() < 1e-9);
    assert!((pitch_hz(event(&plan, "high_down/0/n")) - 6644.875161279122).abs() < 1e-9);
    assert!(pitch_hz(event(&plan, "high_down/0/n")) < 24_000.0);
}

#[test]
fn nested_stretch_cut_keeps_physical_offsets_unscaled() {
    let source = base(
        r#"
pattern leaf {
  length = 1q;
  note n { at = 3/4q; dur = 1/2q; onset_offset = 20ms; release_offset = 20ms; pitch = C4; }
}
pattern parent {
  length = 2q;
  use twice { pattern = &leaf; at = 0q; stretch = 2; boundary = cut; }
}
place pl { pattern = &parent; track = &t; at = 0q; }
"#,
        "0q, 3q",
        "(0q, 120bpm, step)",
        "(0q, 4, 4)",
    );
    let plan = compile_source(&source);
    let note = event(&plan, "pl/0/twice/0/n");
    assert_eq!(note.score_on_q, rational(3, 2));
    assert_eq!(note.score_off_q.as_ref(), Some(&rational(2, 1)));
    assert_eq!(note.onset_offset_seconds, rational(1, 50));
    assert_eq!(note.release_offset_seconds, rational(1, 50));
    assert_eq!(note.on_seconds, rational(77, 100));
    assert_eq!(note.off_seconds.as_ref(), Some(&rational(51, 50)));
    assert_eq!(note.on_frame, 36_960);
    assert_eq!(note.off_frame, Some(48_960));
}

#[test]
fn occurrence_edits_are_isolated_and_use_final_placement_coordinates() {
    let source = base(
        r#"
pattern riff {
  length = 1q;
  note n { at = 0q; dur = 1/4q; pitch = C4; }
  note m { at = 1/2q; dur = 1/4q; pitch = D4; }
}
place left {
  pattern = &riff; track = &t; at = 0q;
  override move { event = "0/n"; set = { at = 1/4q; pitch = C5; }; }
  override drop { event = "0/m"; delete = true; }
  insert add { note extra { at = 3/4q; dur = 1/4q; pitch = E4; } }
}
place right {
  pattern = &riff; track = &t; at = 1q;
  override empty { event = "0/n"; set = {}; }
  insert add { note extra { at = 3/4q; dur = 1/4q; pitch = E4; } }
}
"#,
        "0q, 2q",
        "(0q, 120bpm, step)",
        "(0q, 4, 4)",
    );
    let plan = compile_source(&source);
    assert_eq!(plan.events.len(), 5);

    let moved = event(&plan, "left/0/n");
    assert_eq!(moved.score_on_q, rational(1, 4));
    assert!((pitch_hz(moved) - 523.2511306011972).abs() < 1e-9);
    assert!(plan.events.iter().all(|event| event.address != "left/0/m"));
    assert_eq!(event(&plan, "left/add/extra").score_on_q, rational(3, 4));

    let untouched = event(&plan, "right/0/n");
    assert_eq!(untouched.score_on_q, rational(1, 1));
    assert!((pitch_hz(untouched) - 261.6255653005986).abs() < 1e-9);
    assert_eq!(event(&plan, "right/0/m").score_on_q, rational(3, 2));
    assert_eq!(event(&plan, "right/add/extra").score_on_q, rational(7, 4));
}

#[test]
fn seconds_automation_anchor_at_global_bar_is_lowered_exactly() {
    let source = base(
        r#"
node filter { type = "core.onepole/1"; config = { channels = 1; }; params = { cutoff = 500Hz; }; }
connect sine_filter { from = &sine:out; to = &filter:in; }
connect filter_sum { from = &filter:out; to = &sum:in; }
curve opening { clock = seconds; points = [(0s, 500Hz, linear), (1s, 1000Hz, step)]; }
automation open { target = &filter.params.cutoff; curve = &opening; at = bar(2, 1); }
"#,
        "0q, 8q",
        "(0q, 120bpm, step)",
        "(0q, 4, 4)",
    );
    let plan = compile_source(&source);
    assert_eq!(plan.automation.len(), 1);
    let automation = &plan.automation[0];
    assert_eq!(automation.clock, AutomationClock::Seconds);
    assert_eq!(automation.at, rational(2, 1));
    assert_eq!(automation.points[0].position, rational(0, 1));
    assert_eq!(automation.points[0].value, rational(500, 1));
    assert_eq!(automation.points[1].position, rational(1, 1));
    assert_eq!(automation.points[1].value, rational(1000, 1));
}

#[test]
fn unused_bad_maps_fields_and_cycles_are_still_diagnosed() {
    let source = base(
        r#"
curve bad_curve { clock = score; points = [(0q, 1, step)]; typo = 1; }
meter bad_meter { points = [(0q, 4, 4), (1q, 3, 4)]; }
pattern cycle_a { length = 1q; use to_b { pattern = &cycle_b; at = 0q; } }
pattern cycle_b { length = 1q; use to_a { pattern = &cycle_a; at = 0q; } }
"#,
        "0q, 1q",
        "(0q, 120bpm, step)",
        "(0q, 4, 4)",
    );
    let codes = diagnostic_codes(&source);
    assert!(codes.contains(&DiagnosticCode::UnknownField));
    assert!(codes.contains(&DiagnosticCode::MeterBoundary));
    assert!(codes.contains(&DiagnosticCode::PatternCycle));
}

#[test]
fn deep_unused_pattern_dag_hits_the_bound_before_recursive_work() {
    let mut extra = String::new();
    for index in 0..64 {
        extra.push_str(&format!(
            "pattern p{index} {{ length = 1q; use next {{ pattern = &p{}; at = 0q; }} }}\n",
            index + 1
        ));
    }
    extra.push_str("pattern p64 { length = 1q; }\n");
    let source = base(&extra, "0q, 1q", "(0q, 120bpm, step)", "(0q, 4, 4)");
    let codes = diagnostic_codes(&source);
    assert!(codes.contains(&DiagnosticCode::ResourceLimit));
}

#[test]
fn empty_nested_repetition_work_is_bounded_without_event_expansion() {
    let source = base(
        r#"
pattern leaf { length = 1/25000000q; }
pattern middle {
  length = 1/5000q;
  use u { pattern = &leaf; at = 0q; count = 5000; }
}
pattern outer {
  length = 1q;
  use u { pattern = &middle; at = 0q; count = 5000; }
}
place pl { pattern = &outer; track = &t; at = 0q; }
"#,
        "0q, 1q",
        "(0q, 120bpm, step)",
        "(0q, 4, 4)",
    );
    let codes = diagnostic_codes(&source);
    assert!(codes.contains(&DiagnosticCode::ResourceLimit));
}

#[test]
fn source_limits_remain_separate_for_nodes_and_aggregate_objects() {
    let mut extra = String::new();
    for index in 0..257 {
        extra.push_str(&format!(
            "node n{index} {{ type = \"core.sine/1\"; config = {{ voices = 1; }}; }}\n"
        ));
    }
    let source = base(&extra, "0q, 1q", "(0q, 120bpm, step)", "(0q, 4, 4)");
    let codes = diagnostic_codes(&source);
    assert!(codes.contains(&DiagnosticCode::ResourceLimit));
}
