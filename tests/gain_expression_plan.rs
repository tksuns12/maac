use maac::plan::*;
use serde_json::json;

fn rat(n: i64, d: i64) -> Rational {
    maac::parse_rational(&format!("{n}/{d}")).expect("test rational")
}

fn base_plan() -> Plan {
    Plan {
        version: 1,
        output: OutputSettings {
            score_start_q: rat(0, 1),
            score_end_q: rat(4, 1),
            tail_seconds: rat(0, 1),
            sample_rate_hz: 48_000,
            channels: 1,
            total_frames: 96_000,
            output: PortRef::new("sum", "out").unwrap(),
        },
        tempo: TempoMap {
            points: vec![TempoPoint {
                q: rat(0, 1),
                bpm: rat(120, 1),
                shape: Interpolation::Step,
            }],
        },
        events: vec![ResolvedEvent {
            address: "main/0/n1".into(),
            source: SourceMapping {
                object: "n1".into(),
                path: vec!["main".into(), "n1".into()],
                span: None,
            },
            target: EventTarget::new("sine", "events").unwrap(),
            kind: EventKind::Note {
                timbre_expression: None,
                pressure_expression: None,
                pitch_expression: None,
                gain_expression: None,
                pitch_hz: 440.0,
                velocity: rat(1, 1),
            },
            score_on_q: rat(0, 1),
            score_off_q: Some(rat(1, 1)),
            onset_offset_seconds: rat(0, 1),
            release_offset_seconds: rat(0, 1),
            on_seconds: rat(0, 1),
            off_seconds: Some(rat(1, 2)),
            release_velocity: 0.0,
            on_frame: 0,
            off_frame: Some(24_000),
            order: 0,
        }],
        nodes: vec![
            Node::new("sine", Processor::sine(16)).unwrap(),
            Node::new("sum", Processor::sum(1)).unwrap(),
        ],
        connections: vec![Connection::new(
            "sine_to_sum",
            PortRef::new("sine", "out").unwrap(),
            PortRef::new("sum", "in").unwrap(),
        )
        .unwrap()],
        automation: Vec::new(),
        regions: vec![Region {
            id: "whole".into(),
            start_q: rat(0, 1),
            end_q: rat(4, 1),
            label: None,
        }],
        source_mappings: Vec::new(),
        instruments: None,
        production: None,
    }
}

#[test]
fn gain_payload_roundtrips() {
    for version in [1, 2] {
        let mut value = serde_json::to_value(base_plan()).unwrap();
        value["version"] = json!(version);
        if version == 2 {
            value["instruments"] = json!({"entry_source":"score.maac", "programs":[],
                "wavetables":[],"wavetable_sources":[], "dependencies":[], "libraries":[],
                "source_files":[{"path":"score.maac","hash":format!("sha256:{}", "1".repeat(64))}]});
        }
        value["events"][0]["kind"]["gain_expression"] = json!({
            "clock":"normalized", "points":[
                {"position":"0/1","gain":"0/1","shape":"linear"},
                {"position":"1/1","gain":"2/1","shape":"step"}
            ]
        });
        let plan = Plan::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(&plan).unwrap(), value);
        assert_eq!(serde_json::from_value::<Plan>(value).unwrap(), plan);
    }
}

fn valid_expression() -> GainExpression {
    GainExpression {
        clock: ExpressionClock::Normalized,
        points: vec![
            GainExpressionPoint {
                position: rat(0, 1),
                gain: rat(0, 1),
                shape: Interpolation::Linear,
            },
            GainExpressionPoint {
                position: rat(1, 1),
                gain: rat(2, 1),
                shape: Interpolation::Step,
            },
        ],
    }
}
fn with_expression(expression: GainExpression) -> Plan {
    let mut plan = base_plan();
    let EventKind::Note {
        gain_expression, ..
    } = &mut plan.events[0].kind
    else {
        unreachable!()
    };
    *gain_expression = Some(expression);
    plan
}
fn assert_invalid(expression: GainExpression, code: &str) {
    let plan = with_expression(expression);
    assert_eq!(plan.validate().unwrap_err().code, code);
    let value = serde_json::to_value(&plan).unwrap();
    assert!(serde_json::from_value::<Plan>(value.clone()).is_err());
    assert!(Plan::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}
#[test]
fn all_clocks_and_interpolations_support_zero_and_unbounded_finite_gain() {
    for clock in [
        ExpressionClock::Score,
        ExpressionClock::Seconds,
        ExpressionClock::Normalized,
    ] {
        for shape in [
            Interpolation::Step,
            Interpolation::Linear,
            Interpolation::Exponential,
        ] {
            let mut expression = valid_expression();
            expression.clock = clock;
            expression.points[0].shape = shape;
            if shape == Interpolation::Exponential {
                expression.points[0].gain = rat(1, 1);
            }
            expression.points[1].gain =
                Rational::from_integer(num_bigint::BigInt::from(10).pow(300));
            let plan = with_expression(expression);
            plan.validate().unwrap();
            assert_eq!(Plan::from_json(&plan.to_json().unwrap()).unwrap(), plan);
        }
    }
}
#[test]
fn invalid_structure_and_negative_values_fail_at_all_boundaries() {
    for variant in 0..8 {
        let mut expression = valid_expression();
        match variant {
            0 => expression.points.clear(),
            1 => expression.points[0].position = rat(1, 2),
            2 => expression.points[1].position = rat(0, 1),
            3 => expression.points[1].position = rat(-1, 1),
            4 => expression.points[1].position = rat(2, 1),
            5 => expression.points[1].shape = Interpolation::Linear,
            6 => expression.points[0].gain = rat(-1, 1),
            _ => expression.points[0].shape = Interpolation::Exponential,
        }
        assert_invalid(expression, "E_RANGE");
    }
    // Invalid exponential endpoints after the effective gate still fail.
    for zero_at in [1, 2] {
        let mut expression = valid_expression();
        expression.clock = ExpressionClock::Seconds;
        expression.points[1].shape = Interpolation::Exponential;
        expression.points.push(GainExpressionPoint {
            position: rat(2, 1),
            gain: rat(3, 1),
            shape: Interpolation::Step,
        });
        expression.points[zero_at].gain = rat(0, 1);
        assert_invalid(expression, "E_RANGE");
    }
}
#[test]
fn strict_gain_wire_and_unsupported_expression_fields() {
    let valid = serde_json::to_value(with_expression(valid_expression())).unwrap();
    for field in ["extra", "pressure", "timbre"] {
        let mut bad = valid.clone();
        bad["events"][0]["kind"][field] = json!({});
        assert!(serde_json::from_value::<Plan>(bad).is_err());
    }
    for (field, replacement) in [("clock", json!("beats")), ("extra", json!(true))] {
        let mut bad = valid.clone();
        bad["events"][0]["kind"]["gain_expression"][field] = replacement;
        assert!(serde_json::from_value::<Plan>(bad).is_err());
    }
    for (field, replacement) in [
        ("position", json!("0/2")),
        ("gain", json!("2/2")),
        ("gain", json!("1/0")),
        ("gain", json!("1/-2")),
        ("extra", json!(true)),
    ] {
        let mut bad = valid.clone();
        bad["events"][0]["kind"]["gain_expression"]["points"][0][field] = replacement;
        assert!(serde_json::from_value::<Plan>(bad.clone()).is_err());
        assert!(Plan::from_json(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
}
#[test]
fn raw_rationals_fail_before_math_and_tightened_limits_are_honored() {
    for value in [
        Rational::new_raw(1.into(), 0.into()),
        Rational::new_raw(1.into(), (-1).into()),
        Rational::new_raw(2.into(), 2.into()),
        Rational::from_integer(num_bigint::BigInt::from(1) << 4097),
    ] {
        for index in [0, 1] {
            for position in [true, false] {
                let mut expression = valid_expression();
                if position {
                    expression.points[index].position = value.clone();
                } else {
                    expression.points[index].gain = value.clone();
                }
                assert!(with_expression(expression).validate().is_err());
            }
        }
    }
    let mut expression = valid_expression();
    expression.points[1].gain = rat(1, 100_003);
    assert_eq!(
        with_expression(expression)
            .validate_with_limits(&PlanLimits {
                max_rational_bits: 16,
                ..Default::default()
            })
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
    let mut expression = valid_expression();
    expression.points[1].gain = Rational::from_integer(num_bigint::BigInt::from(1) << 2000);
    assert_invalid(expression, "E_NONFINITE");
    let mut expression = valid_expression();
    expression.points[0].gain = Rational::new(1.into(), num_bigint::BigInt::from(10).pow(400));
    expression.points[0].shape = Interpolation::Exponential;
    with_expression(expression).validate().unwrap();
}
#[test]
fn simultaneous_curves_count_all_points_objects_and_work() {
    let gain = with_expression(valid_expression());
    let mut both = gain.clone();
    let EventKind::Note {
        pitch_expression, ..
    } = &mut both.events[0].kind
    else {
        unreachable!()
    };
    *pitch_expression = Some(PitchExpression {
        clock: PitchExpressionClock::Normalized,
        points: vec![
            PitchExpressionPoint {
                position: rat(0, 1),
                cents: rat(0, 1),
                shape: Interpolation::Linear,
            },
            PitchExpressionPoint {
                position: rat(1, 1),
                cents: rat(1200, 1),
                shape: Interpolation::Step,
            },
        ],
    });
    both.validate().unwrap();
    assert_eq!(Plan::from_json(&both.to_json().unwrap()).unwrap(), both);
    assert_eq!(
        both.validate_with_limits(&PlanLimits {
            max_automation_points: 3,
            ..Default::default()
        })
        .unwrap_err()
        .code,
        "E_RESOURCE_LIMIT"
    );
    both.validate_with_limits(&PlanLimits {
        max_automation_points: 4,
        ..Default::default()
    })
    .unwrap();
    both.automation.push(Automation {
        id: "level".into(),
        target: PortRef::new("sine", "level").unwrap(),
        clock: AutomationClock::Seconds,
        at: rat(0, 1),
        points: vec![AutomationPoint {
            position: rat(0, 1),
            value: rat(1, 1),
            shape: Interpolation::Step,
        }],
    });
    assert_eq!(
        both.validate_with_limits(&PlanLimits {
            max_automation_points: 4,
            ..Default::default()
        })
        .unwrap_err()
        .code,
        "E_RESOURCE_LIMIT"
    );
    both.automation.clear();
    for objects in [true, false] {
        let first_pass = (0..100)
            .find(|&limit| {
                let mut limits = PlanLimits::default();
                if objects {
                    limits.max_objects = limit;
                } else {
                    limits.max_work = limit as u64;
                }
                base_plan().validate_with_limits(&limits).is_ok()
            })
            .unwrap();
        for (plan, additional) in [(&gain, 3), (&both, 6)] {
            let mut limits = PlanLimits::default();
            if objects {
                limits.max_objects = first_pass + additional - 1;
            } else {
                limits.max_work = (first_pass + additional - 1) as u64;
            }
            assert_eq!(
                plan.validate_with_limits(&limits).unwrap_err().code,
                "E_RESOURCE_LIMIT"
            );
            if objects {
                limits.max_objects += 1;
            } else {
                limits.max_work += 1;
            }
            plan.validate_with_limits(&limits).unwrap();
        }
    }
}
#[test]
fn absent_gain_preserves_legacy_json() {
    let plan = base_plan();
    let bytes = plan.to_json().unwrap();
    assert!(!String::from_utf8(bytes.clone())
        .unwrap()
        .contains("gain_expression"));
    assert_eq!(Plan::from_json(&bytes).unwrap(), plan);
}
