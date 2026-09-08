use std::fs;

use maac::{compiler::compile, parse};

fn base(extra: &str, score_end: &str) -> String {
    format!(
        r#"maac 1;
project p {{
  score = [0q, {score_end}];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &sum:out;
}}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; config = {{ voices = 8; }}; }}
node sum {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
connect sine_sum {{ from = &sine:out; to = &sum:in; }}
track t {{ target = &sine:events; }}
{extra}"#
    )
}

#[test]
fn example_compiles_to_forty_eight_notes_and_expected_frames() {
    let source = fs::read_to_string("example.maac").expect("example source");
    let document = parse(&source).expect("example parses");
    let plan = compile(&document).expect("example compiles");
    assert_eq!(plan.events.len(), 48);
    assert_eq!(plan.output.total_frames, 816_000);
    assert_eq!(plan.output.channels, 2);
}

#[test]
fn evening_window_compiles_to_182_notes_and_expected_frames() {
    let source = fs::read_to_string("evening-window.maac").expect("evening source");
    let document = parse(&source).expect("evening source parses");
    let plan = compile(&document).expect("evening source compiles");
    assert_eq!(plan.events.len(), 182);
    assert_eq!(plan.output.total_frames, 1_968_000);
    assert_eq!(plan.output.channels, 2);
}

#[test]
fn render_duration_over_thirty_minutes_is_a_resource_error() {
    let source = base("", "3601q");
    let diagnostics = compile(&parse(&source).unwrap()).unwrap_err();
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == maac::DiagnosticCode::ResourceLimit));
}

#[test]
fn sine_voices_over_the_foundation_limit_are_a_resource_error() {
    let source = base("", "1q").replace("voices = 8", "voices = 1000001");
    let diagnostics = compile(&parse(&source).unwrap()).unwrap_err();
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == maac::DiagnosticCode::ResourceLimit));
}

#[test]
fn nested_cut_is_applied_before_physical_offsets_and_mapping_is_source_stable() {
    let source = base(
        r#"
pattern leaf { length = 1q; note n { at = 3/4q; dur = 1/2q; onset_offset = 20ms; release_offset = 20ms; pitch = C4; } }
pattern parent { length = 1q; use u { pattern = &leaf; at = 0q; boundary = cut; } }
place pl { pattern = &parent; track = &t; at = 0q; }
"#,
        "2q",
    );
    let plan = compile(&parse(&source).unwrap()).unwrap();
    assert_eq!(plan.events.len(), 1);
    let event = &plan.events[0];
    assert_eq!(event.address, "pl/0/u/0/n");
    assert_eq!(event.score_off_q.as_ref().unwrap().to_string(), "1");
    assert_eq!(event.off_seconds.as_ref().unwrap().to_string(), "13/25");
    assert_eq!(event.source.path, vec!["leaf", "n"]);
}

#[test]
fn occurrence_pitch_replacement_is_final_and_does_not_reapply_transposition() {
    let source = base(
        r#"
pattern riff { length = 1q; note n { at = 0q; dur = 1/2q; pitch = C4; } }
place pl {
  pattern = &riff; track = &t; at = 0q; transpose = 1200ct;
  override edit { event = "0/n"; set = { pitch = C5; }; }
}
"#,
        "1q",
    );
    let plan = compile(&parse(&source).unwrap()).unwrap();
    let pitch = match plan.events[0].kind {
        maac::plan::EventKind::Note { pitch_hz, .. } => pitch_hz,
        _ => unreachable!(),
    };
    assert!((pitch - 523.2511306011972).abs() < 1e-9);
}

#[test]
fn inherited_transposition_is_applied_before_the_final_nyquist_check() {
    let source = base(
        r#"
pattern riff { length = 1q; note n { at = 0q; dur = 1/2q; pitch = key(140); } }
pattern low { length = 1q; use u { pattern = &riff; at = 0q; transpose = -2400ct; } }
place pl { pattern = &low; track = &t; at = 0q; }
"#,
        "1q",
    );
    let plan = compile(&parse(&source).unwrap()).unwrap();
    let pitch = match plan.events[0].kind {
        maac::plan::EventKind::Note { pitch_hz, .. } => pitch_hz,
        _ => unreachable!(),
    };
    assert!(pitch > 0.0 && pitch < 24_000.0);
}

#[test]
fn score_end_truncation_happens_after_release_offset() {
    let source = base(
        r#"
pattern riff { length = 1q; note n { at = 0q; dur = 2q; release_offset = -250ms; pitch = C4; } }
place pl { pattern = &riff; track = &t; at = 0q; }
"#,
        "1q",
    );
    let plan = compile(&parse(&source).unwrap()).unwrap();
    assert_eq!(
        plan.events[0].off_seconds.as_ref().unwrap().to_string(),
        "1/2"
    );
}

#[test]
fn subsample_failure_keeps_source_path_and_span() {
    let source = base(
        r#"
pattern riff { length = 1q; note n { at = 1/1000000q; dur = 1/1000000q; pitch = C4; } }
place pl { pattern = &riff; track = &t; at = 0q; }
"#,
        "1q",
    );
    let diagnostics = compile(&parse(&source).unwrap()).unwrap_err();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == maac::DiagnosticCode::SubsampleNote)
        .expect("subsample diagnostic");
    assert_eq!(diagnostic.object_path, vec!["riff", "n"]);
    assert_eq!(diagnostic.field_path, vec!["dur"]);
    assert!(diagnostic.span.is_some());
}

#[test]
fn oversized_repetition_is_rejected_even_when_the_pattern_is_empty() {
    let source = base(
        r#"
pattern empty { length = 1/100001q; }
place pl { pattern = &empty; track = &t; at = 0q; count = 100001; }
"#,
        "1q",
    );
    let diagnostics = compile(&parse(&source).unwrap()).unwrap_err();
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == maac::DiagnosticCode::ResourceLimit));
}

#[test]
fn nested_empty_repetitions_are_rejected_by_preflight_without_expanding() {
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
        "1q",
    );
    let diagnostics = compile(&parse(&source).unwrap()).unwrap_err();
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == maac::DiagnosticCode::ResourceLimit));
}
