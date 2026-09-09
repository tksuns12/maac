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
fn normalized_gain_compiles() {
    let source = base(
        r#"curve bend { clock = normalized; points = [(0, 0, linear), (1, 2, step)]; }
 pattern phrase { length = 1q; note n { at = 0q; dur = 1q; pitch = A4; expression e { kind = gain; curve = &bend; } } }
 place p1 { pattern = &phrase; track = &t; at = 0q; }"#,
        "4q",
    );
    let plan = compile(&parse(&source).unwrap()).expect("gain expression compiles");
    plan.validate().unwrap();
}

use maac::plan::{EventKind, ExpressionClock, Plan, PlanLimits};
use maac::DiagnosticCode;
fn source(clock: &str, points: &str, arrangement: &str) -> String {
    base(
        &format!(
            r#"curve bend {{ clock = {clock}; points = {points}; }}
pattern phrase {{ length = 1q; note n {{ at = 0q; dur = 2q; pitch = A4; expression e {{ kind = gain; curve = &bend; }} }} }}
{arrangement}"#
        ),
        "16q",
    )
}
const PLACE: &str = "place p1 { pattern = &phrase; track = &t; at = 0q; }";
fn expression(plan: &Plan, index: usize) -> &maac::plan::GainExpression {
    match &plan.events[index].kind {
        EventKind::Note {
            gain_expression: Some(e),
            ..
        } => e,
        _ => panic!("expression missing"),
    }
}
#[test]
fn instance_stretch_changes_only_score_points_and_preserves_shared_definition() {
    for (clock, points, expected, kind) in [
        (
            "score",
            "[(0q, 0, linear), (1q, 100, step)]",
            "6",
            ExpressionClock::Score,
        ),
        (
            "seconds",
            "[(0s, 0, linear), (1s, 100, step)]",
            "1",
            ExpressionClock::Seconds,
        ),
        (
            "normalized",
            "[(0, 0, linear), (1, 100, step)]",
            "1",
            ExpressionClock::Normalized,
        ),
    ] {
        let text = source(
            clock,
            points,
            r#"pattern outer { length = 2q; use u { pattern = &phrase; at = 0q; stretch = 2; } }
place p1 { pattern = &outer; track = &t; at = 0q; stretch = 3; transpose = 1200ct;
override duration { event = "0/u/0/n"; set = { dur = 1q; pitch = 220Hz; onset_offset = 10ms; release_offset = 20ms; }; }
insert extra { note added { at = 2q; dur = 1q; pitch = A4; expression e { kind = gain; curve = &bend; } } }
}
place p2 { pattern = &phrase; track = &t; at = 8q; }"#,
        );
        let plan = compile(&parse(&text).unwrap()).unwrap();
        assert_eq!(expression(&plan, 0).clock, kind);
        assert_eq!(
            expression(&plan, 0).points[1].position.to_string(),
            expected
        );
        assert_eq!(expression(&plan, 1).points[1].position.to_string(), "1");
        assert_eq!(expression(&plan, 2).points[1].position.to_string(), "1");
        assert_eq!(plan.events[0].on_frame, 480);
        assert_eq!(plan.events[0].off_frame, Some(24960));
        assert_eq!(
            plan.events[0].score_off_q.as_ref().unwrap().to_string(),
            "1"
        );
        assert!(matches!(
            plan.events[0].kind,
            EventKind::Note {
                pitch_hz: 220.0,
                ..
            }
        ));
    }
}
#[test]
fn expanded_expression_budget_counts_global_points_and_deletion() {
    let text = source("score", "[(0q, 0, linear), (1q, 100, step)]", "place p1 { pattern = &phrase; track = &t; at = 0q; count = 3; override del { event = \"1/n\"; delete = true; } }");
    let mut limits = PlanLimits {
        max_automation_points: 4,
        ..Default::default()
    };
    let plan = maac::compiler::compile_with_limits(&parse(&text).unwrap(), &limits).unwrap();
    assert_eq!(plan.events.len(), 2);
    limits.max_automation_points = 3;
    assert!(
        maac::compiler::compile_with_limits(&parse(&text).unwrap(), &limits)
            .unwrap_err()
            .iter()
            .any(|e| e.code == DiagnosticCode::ResourceLimit)
    );
    let with_global = format!("{text}\ncurve level {{ clock = score; points = [(0q, 1, step)]; }} automation a {{ target = &sine.params.level; curve = &level; at = 0q; }}");
    limits.max_automation_points = 4;
    assert!(
        maac::compiler::compile_with_limits(&parse(&with_global).unwrap(), &limits)
            .unwrap_err()
            .iter()
            .any(|e| e.code == DiagnosticCode::ResourceLimit)
    );
}
#[test]
fn cut_and_project_end_preserve_scheduled_and_score_gates() {
    let text = source("score", "[(0q, 0, linear), (2q, 100, step)]", "place p1 { pattern = &phrase; track = &t; at = 0q; boundary = cut; } place p2 { pattern = &phrase; track = &t; at = 15q; }");
    let plan = compile(&parse(&text).unwrap()).unwrap();
    assert_eq!(
        plan.events[0].score_off_q.as_ref().unwrap().to_string(),
        "1"
    );
    assert_eq!(plan.events[0].off_frame, Some(24000));
    assert_eq!(expression(&plan, 0).points[1].position.to_string(), "2");
    assert_eq!(
        plan.events[1].score_off_q.as_ref().unwrap().to_string(),
        "17"
    );
    assert_eq!(plan.events[1].off_frame, Some(384000));
}
#[test]
fn source_v1_and_bundle_v2_roundtrip() {
    let text = source("normalized", "[(0, 0, linear), (1, 100, step)]", PLACE);
    let v1 = compile(&parse(&text).unwrap()).unwrap();
    let bundle = maac::bundle::SourceBundle {
        entry: "main.maac".into(),
        sources: std::collections::BTreeMap::from([("main.maac".into(), text)]),
        assets: Default::default(),
    };
    let v2 = maac::compiler::compile_bundle(&bundle).unwrap();
    for plan in [v1, v2] {
        let restored = Plan::from_json(&plan.to_json().unwrap()).unwrap();
        assert_eq!(expression(&restored, 0), expression(&plan, 0));
    }
}

#[test]
fn graph_receiver_rejects_gain_expression_and_cents_are_not_parameter_units() {
    let text = source("normalized", "[(0, 0, linear), (1, 100, step)]", PLACE);
    let graph = text.replace("node sine { type = \"core.sine/1\"; config = { voices = 8; }; }", r#"instrument lead { channels = 1; voice v { channels = 1; amplitude = &amp; output = &osc:out; node amp { type = "synth.adsr/1"; } node osc { type = "synth.sine/1"; } } }
node sine { instrument = &lead; }"#);
    assert_ne!(graph, text);
    assert!(compile(&parse(&graph).unwrap())
        .unwrap_err()
        .iter()
        .any(|e| e.code == DiagnosticCode::Capability));
    let parameter = text.replace(
        "type = \"core.sine/1\";",
        "type = \"core.sine/1\"; params = { level = 1ct; };",
    );
    assert!(compile(&parse(&parameter).unwrap())
        .unwrap_err()
        .iter()
        .any(|e| e.code == DiagnosticCode::Unit));
}
#[test]
fn score_expression_scaling_reports_existing_rational_bound_error() {
    let huge = num_bigint::BigInt::from(1u8) << 4090usize;
    let text = source(
        "score",
        &format!("[(0q, 0, linear), ({huge}q, 0, step)]"),
        "place p1 { pattern = &phrase; track = &t; at = 0q; stretch = 64; }",
    )
    .replace("16q", "128q");
    let errors = compile(&parse(&text).unwrap()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.code == DiagnosticCode::ResourceLimit),
        "{errors:?}"
    );
}

#[test]
fn simultaneous_expressions_ignore_child_ids_and_share_point_budget() {
    let text = source("normalized", "[(0, 0, linear), (1, 2, step)]", PLACE);
    let both = format!("{}\ncurve bend_pitch {{ clock = normalized; points = [(0, 0ct, linear), (1, 100ct, step)]; }}", text.replace("expression e", "expression a { kind = pitch; curve = &bend_pitch; } expression z"));
    let reordered = both
        .replace("expression a", "expression temporary")
        .replace("expression z", "expression a")
        .replace("expression temporary", "expression z");
    for text in [both, reordered] {
        let v1 = compile(&parse(&text).unwrap()).unwrap();
        let bundle = maac::bundle::SourceBundle {
            entry: "main.maac".into(),
            sources: std::collections::BTreeMap::from([("main.maac".into(), text.clone())]),
            assets: Default::default(),
        };
        for plan in [v1, maac::compiler::compile_bundle(&bundle).unwrap()] {
            let restored = Plan::from_json(&plan.to_json().unwrap()).unwrap();
            match &restored.events[0].kind {
                EventKind::Note {
                    pitch_expression: Some(pitch),
                    gain_expression: Some(gain),
                    ..
                } => {
                    assert_eq!(pitch.points[1].cents.to_string(), "100");
                    assert_eq!(gain.points[1].gain.to_string(), "2");
                }
                _ => panic!("both expressions must survive"),
            }
        }
        let limits = PlanLimits {
            max_automation_points: 3,
            ..Default::default()
        };
        assert!(
            maac::compiler::compile_with_limits(&parse(&text).unwrap(), &limits)
                .unwrap_err()
                .iter()
                .any(|e| e.code == DiagnosticCode::ResourceLimit)
        );
    }
}

#[test]
fn invalid_gain_definitions_fail_even_when_unused() {
    let text = source("normalized", "[(0, 0, linear), (1, 2, step)]", "");
    for (bad, code) in [
        (text.replace("(1, 2,", "(1, -2,"), DiagnosticCode::Range),
        (text.replace("(0, 0,", "(0, 0ct,"), DiagnosticCode::Unit),
        (text.replace("linear", "exponential"), DiagnosticCode::Range),
        (
            text.replace("curve = &bend", "curve = &missing"),
            DiagnosticCode::Reference,
        ),
        (
            text.replace(
                "expression e",
                "expression duplicate { kind = gain; curve = &bend; } expression e",
            ),
            DiagnosticCode::Range,
        ),
        (
            text.replace("kind = gain", "kind = pressure"),
            DiagnosticCode::Capability,
        ),
        (
            text.replace("kind = gain", "kind = timbre"),
            DiagnosticCode::Capability,
        ),
        (
            text.replace(
                "curve = &bend;",
                "curve = &bend; expression nested { kind = gain; curve = &bend; }",
            ),
            DiagnosticCode::Capability,
        ),
    ] {
        let errors = compile(&parse(&bad).unwrap()).unwrap_err();
        assert!(errors.iter().any(|e| e.code == code), "{errors:?}");
    }
}

#[test]
fn tiny_positive_exponential_and_large_finite_gain_compile() {
    let denominator = num_bigint::BigInt::from(1u8) << 2000usize;
    let text = source(
        "normalized",
        &format!("[(0, 1/{denominator}, exponential), (1, 2, step)]"),
        PLACE,
    );
    let plan = compile(&parse(&text).unwrap()).unwrap();
    Plan::from_json(&plan.to_json().unwrap()).unwrap();
    let overflow = num_bigint::BigInt::from(1u8) << 2000usize;
    let unused = source(
        "normalized",
        &format!("[(0, 1, linear), (1, {overflow}, step)]"),
        "",
    );
    assert!(compile(&parse(&unused).unwrap())
        .unwrap_err()
        .iter()
        .any(|e| e.code == DiagnosticCode::Nonfinite));
}

#[test]
fn pitch_only_json_omits_gain() {
    let text = source("normalized", "[(0, 0ct, linear), (1, 100ct, step)]", PLACE)
        .replace("kind = gain", "kind = pitch");
    let json =
        String::from_utf8(compile(&parse(&text).unwrap()).unwrap().to_json().unwrap()).unwrap();
    assert!(json.contains("pitch_expression"));
    assert!(!json.contains("gain_expression"));
}
