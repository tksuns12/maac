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
instrument lead {{ channels = 1; voice v {{ channels = 1; amplitude = &amp; output = &color:out; node amp {{ type = "synth.adsr/1"; }} node color {{ type = "synth.timbre/1"; }} }} }}
node sine {{ instrument = &lead; }}
node sum {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
connect sine_sum {{ from = &sine:out; to = &sum:in; }}
track t {{ target = &sine:events; }}
{extra}"#
    )
}

#[test]
fn normalized_timbre_compiles() {
    let source = base(
        r#"curve bend { clock = normalized; points = [(0, 0, linear), (1, 1, step)]; }
 pattern phrase { length = 1q; note n { at = 0q; dur = 1q; pitch = A4; expression e { kind = timbre; curve = &bend; } } }
 place p1 { pattern = &phrase; track = &t; at = 0q; }"#,
        "4q",
    );
    let plan = compile(&parse(&source).unwrap()).expect("timbre expression compiles");
    plan.validate().unwrap();
}

use maac::plan::{EventKind, Plan, PlanLimits};
use maac::DiagnosticCode;
#[test]
fn clocks_inheritance_overrides_inserts_and_roundtrip() {
    for (clock, points, end) in [
        ("normalized", "[(0, 0, linear), (1, 1, step)]", "1"),
        ("seconds", "[(0s, 0, linear), (1s, 1, step)]", "1"),
        ("score", "[(0q, 0, linear), (1q, 1, step)]", "6"),
    ] {
        let text = base(
            &format!(
                r#"curve shade {{ clock = {clock}; points = {points}; }}
pattern phrase {{ length = 1q; note n {{ at = 0q; dur = 2q; pitch = A4; expression e {{ kind = timbre; curve = &shade; }} }} }}
pattern outer {{ length = 2q; use u {{ pattern = &phrase; at = 0q; stretch = 2; }} }}
place p1 {{ pattern = &outer; track = &t; at = 0q; stretch = 3;
override duration {{ event = "0/u/0/n"; set = {{ dur = 1q; onset_offset = 10ms; release_offset = 20ms; }}; }}
insert extra {{ note added {{ at = 2q; dur = 1q; pitch = A4; expression e {{ kind = timbre; curve = &shade; }} }} }} }}
place p2 {{ pattern = &phrase; track = &t; at = 8q; }}"#
            ),
            "16q",
        );
        let plan = compile(&parse(&text).unwrap()).unwrap();
        let restored = Plan::from_json(&plan.to_json().unwrap()).unwrap();
        for (i, expected) in [end, "1", "1"].iter().enumerate() {
            let EventKind::Note {
                timbre_expression: Some(e),
                ..
            } = &restored.events[i].kind
            else {
                panic!()
            };
            assert_eq!(e.points[1].position.to_string(), *expected);
        }
        assert_eq!(restored.events[0].on_frame, 480);
        assert_eq!(restored.events[0].off_frame, Some(24960));
    }
}
#[test]
fn all_expressions_share_expanded_point_budget() {
    let text = base(
        r#"curve shade { clock = normalized; points = [(0, 0, linear), (1, 1, step)]; }
curve bend { clock = normalized; points = [(0, 0ct, linear), (1, 100ct, step)]; }
pattern phrase { length = 1q; note n { at = 0q; dur = 1q; pitch = A4;
expression z { kind = timbre; curve = &shade; } expression a { kind = gain; curve = &shade; } expression m { kind = pitch; curve = &bend; } } }
place p1 { pattern = &phrase; track = &t; at = 0q; count = 2; }"#,
        "4q",
    );
    for limit in [11, 12] {
        let result = maac::compiler::compile_with_limits(
            &parse(&text).unwrap(),
            &PlanLimits {
                max_automation_points: limit,
                ..Default::default()
            },
        );
        if limit == 11 {
            assert!(result
                .unwrap_err()
                .iter()
                .any(|e| e.code == DiagnosticCode::ResourceLimit));
        } else {
            let plan = result.unwrap();
            let restored = Plan::from_json(&plan.to_json().unwrap()).unwrap();
            assert!(matches!(
                &restored.events[1].kind,
                EventKind::Note {
                    timbre_expression: Some(_),
                    gain_expression: Some(_),
                    pitch_expression: Some(_),
                    ..
                }
            ));
        }
    }
}
#[test]
fn invalid_values_units_and_receivers_fail() {
    let text = base(
        r#"curve shade { clock = normalized; points = [(0, 0, linear), (1, 1, step)]; }
pattern phrase { length = 1q; note n { at = 0q; dur = 1q; pitch = A4; expression e { kind = timbre; curve = &shade; } } }
place p1 { pattern = &phrase; track = &t; at = 0q; }"#,
        "4q",
    );
    for (bad, code) in [
        (text.replace("(1, 1,", "(1, 2,"), DiagnosticCode::Range),
        (text.replace("(0, 0,", "(0, -1,"), DiagnosticCode::Range),
        (text.replace("(0, 0,", "(0, 0ct,"), DiagnosticCode::Unit),
        (text.replace("linear", "exponential"), DiagnosticCode::Range),
        (
            text.replace("synth.timbre/1", "synth.sine/1"),
            DiagnosticCode::Capability,
        ),
        (
            text.replace(
                "node sine { instrument = &lead; }",
                "node sine { type = \"core.sine/1\"; }",
            ),
            DiagnosticCode::Capability,
        ),
        (
            text.replace("kind = timbre", "kind = pressure"),
            DiagnosticCode::Capability,
        ),
    ] {
        let errors = compile(&parse(&bad).unwrap()).unwrap_err();
        assert!(errors.iter().any(|e| e.code == code), "{errors:?}");
    }
}
