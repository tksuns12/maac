use maac::plan::*;
use serde_json::json;

fn rat(n: i64, d: i64) -> Rational {
    maac::parse_rational(&format!("{n}/{d}")).expect("test rational")
}

fn base_plan() -> Plan {
    maac::compiler::compile(
        &maac::parse(
            r#"maac 1;
project p { score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &meter; output = &lead:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter meter { points = [(0q, 4, 4)]; }
instrument instrument { channels = 1; voice v { channels = 1; amplitude = &amp; output = &osc:out;
node amp { type = "synth.adsr/1"; params = { release = 0s; }; }
node osc { type = "synth.sine/1"; }
node color { type = "synth.timbre/1"; }
} }
node lead { instrument = &instrument; config = { voices = 2; }; }
track t { target = &lead:events; }
pattern notes { length = 1q; note a { at = 0q; dur = 1q; pitch = A4; } }
place placement { pattern = &notes; track = &t; at = 0q; }
"#,
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn timbre_payload_roundtrips() {
    let plan = with_expression(valid_expression());
    let value = serde_json::to_value(&plan).unwrap();
    assert_eq!(
        Plan::from_json(&serde_json::to_vec(&value).unwrap()).unwrap(),
        plan
    );
    assert_eq!(serde_json::from_value::<Plan>(value).unwrap(), plan);
}

fn valid_expression() -> TimbreExpression {
    TimbreExpression {
        clock: ExpressionClock::Normalized,
        points: vec![
            TimbreExpressionPoint {
                position: rat(0, 1),
                value: rat(0, 1),
                shape: Interpolation::Linear,
            },
            TimbreExpressionPoint {
                position: rat(1, 1),
                value: rat(1, 1),
                shape: Interpolation::Step,
            },
        ],
    }
}
fn with_expression(expression: TimbreExpression) -> Plan {
    let mut plan = base_plan();
    let EventKind::Note {
        timbre_expression, ..
    } = &mut plan.events[0].kind
    else {
        unreachable!()
    };
    *timbre_expression = Some(expression);
    plan
}
fn assert_invalid(expression: TimbreExpression, code: &str) {
    let plan = with_expression(expression);
    assert_eq!(plan.validate().unwrap_err().code, code);
    let value = serde_json::to_value(&plan).unwrap();
    assert!(serde_json::from_value::<Plan>(value.clone()).is_err());
    assert!(Plan::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}
#[test]
fn all_clocks_and_interpolations_support_unit_interval() {
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
                expression.points[0].value = rat(1, 1);
            }
            expression.points[1].value = rat(1, 1);
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
            6 => expression.points[0].value = rat(-1, 1),
            _ => expression.points[0].shape = Interpolation::Exponential,
        }
        assert_invalid(expression, "E_RANGE");
    }
    // Invalid exponential endpoints after the effective gate still fail.
    for zero_at in [1, 2] {
        let mut expression = valid_expression();
        expression.clock = ExpressionClock::Seconds;
        expression.points[1].shape = Interpolation::Exponential;
        expression.points.push(TimbreExpressionPoint {
            position: rat(2, 1),
            value: rat(1, 1),
            shape: Interpolation::Step,
        });
        expression.points[zero_at].value = rat(0, 1);
        assert_invalid(expression, "E_RANGE");
    }
}
#[test]
fn strict_timbre_wire_and_unsupported_expression_fields() {
    let valid = serde_json::to_value(with_expression(valid_expression())).unwrap();
    for field in ["extra", "pressure", "timbre"] {
        let mut bad = valid.clone();
        bad["events"][0]["kind"][field] = json!({});
        assert!(serde_json::from_value::<Plan>(bad).is_err());
    }
    for (field, replacement) in [("clock", json!("beats")), ("extra", json!(true))] {
        let mut bad = valid.clone();
        bad["events"][0]["kind"]["timbre_expression"][field] = replacement;
        assert!(serde_json::from_value::<Plan>(bad).is_err());
    }
    for (field, replacement) in [
        ("position", json!("0/2")),
        ("value", json!("2/2")),
        ("value", json!("1/0")),
        ("value", json!("1/-2")),
        ("extra", json!(true)),
    ] {
        let mut bad = valid.clone();
        bad["events"][0]["kind"]["timbre_expression"]["points"][0][field] = replacement;
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
                    expression.points[index].value = value.clone();
                }
                assert!(with_expression(expression).validate().is_err());
            }
        }
    }
    let mut expression = valid_expression();
    expression.points[1].value = rat(1, 100_003);
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
    expression.points[1].value = Rational::from_integer(num_bigint::BigInt::from(1) << 2000);
    assert_invalid(expression, "E_NONFINITE");
    let mut expression = valid_expression();
    expression.points[0].value = Rational::new(1.into(), num_bigint::BigInt::from(10).pow(400));
    expression.points[0].shape = Interpolation::Exponential;
    with_expression(expression).validate().unwrap();
}
#[test]
fn simultaneous_curves_count_all_points_objects_and_work() {
    let timbre = with_expression(valid_expression());
    let mut both = timbre.clone();
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
        target: PortRef::new("lead", "gain").unwrap(),
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
        for (plan, additional) in [(&timbre, 3), (&both, 6)] {
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
fn absent_timbre_preserves_legacy_json() {
    let plan = base_plan();
    let bytes = plan.to_json().unwrap();
    assert!(!String::from_utf8(bytes.clone())
        .unwrap()
        .contains("timbre_expression"));
    assert_eq!(Plan::from_json(&bytes).unwrap(), plan);
}

#[test]
fn exact_unit_bounds_reject_even_values_that_round_to_one() {
    for value in [
        rat(2, 1),
        rat(-1, 2),
        rat(1, 1) + Rational::new(1.into(), num_bigint::BigInt::from(1) << 200),
    ] {
        let mut expression = valid_expression();
        expression.points[1].value = value;
        assert_invalid(expression, "E_RANGE");
    }
}

#[test]
fn receiver_requires_explicit_timbre_source_even_when_note_is_silent() {
    for core in [false, true] {
        let mut plan = with_expression(valid_expression());
        if core {
            plan.nodes[0].processor = Processor::sine(2);
        } else {
            plan.instruments.as_mut().unwrap().programs[0]
                .voice
                .nodes
                .retain(|node| node.id != "color");
        }
        if let EventKind::Note {
            velocity,
            gain_expression,
            ..
        } = &mut plan.events[0].kind
        {
            *velocity = rat(0, 1);
            *gain_expression = Some(GainExpression {
                clock: ExpressionClock::Seconds,
                points: vec![GainExpressionPoint {
                    position: rat(0, 1),
                    gain: rat(0, 1),
                    shape: Interpolation::Step,
                }],
            });
        }
        assert_eq!(plan.validate().unwrap_err().code, "E_CAPABILITY");
        assert!(Plan::from_json(&serde_json::to_vec(&plan).unwrap()).is_err());
    }
}

fn minimum_execution_budget(plan: &Plan) -> u64 {
    let mut low = 0;
    let mut high = PlanLimits::default().max_execution_work;
    while low < high {
        let middle = low + (high - low) / 2;
        if plan
            .validate_with_limits(&PlanLimits {
                max_execution_work: middle,
                ..Default::default()
            })
            .is_ok()
        {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    plan.validate_with_limits(&PlanLimits {
        max_execution_work: low,
        ..Default::default()
    })
    .unwrap();
    low
}

#[test]
fn all_three_expression_costs_are_additive_even_for_silent_voices() {
    let mut base = base_plan();
    if let EventKind::Note { velocity, .. } = &mut base.events[0].kind {
        *velocity = rat(0, 1);
    }
    let baseline = minimum_execution_budget(&base);
    let frames = base.events[0].off_frame.unwrap() - base.events[0].on_frame;
    let mut plan = base;
    if let EventKind::Note {
        gain_expression,
        pitch_expression,
        ..
    } = &mut plan.events[0].kind
    {
        *gain_expression = Some(GainExpression {
            clock: ExpressionClock::Seconds,
            points: vec![GainExpressionPoint {
                position: rat(0, 1),
                gain: rat(0, 1),
                shape: Interpolation::Step,
            }],
        });
        *pitch_expression = Some(PitchExpression {
            clock: ExpressionClock::Score,
            points: vec![PitchExpressionPoint {
                position: rat(0, 1),
                cents: rat(0, 1),
                shape: Interpolation::Step,
            }],
        });
    }
    assert_eq!(minimum_execution_budget(&plan), baseline + frames * 34);
    for points in [2usize, 3, 4, 5] {
        if let EventKind::Note {
            timbre_expression, ..
        } = &mut plan.events[0].kind
        {
            *timbre_expression = Some(TimbreExpression {
                clock: ExpressionClock::Seconds,
                points: (0..points)
                    .map(|position| TimbreExpressionPoint {
                        position: rat(position as i64, 1),
                        value: rat(0, 1),
                        shape: Interpolation::Step,
                    })
                    .collect(),
            });
        }
        let lookup = u64::from(usize::BITS - (points - 1).leading_zeros());
        assert_eq!(
            minimum_execution_budget(&plan),
            baseline + frames * (34 + 17 + lookup)
        );
        assert_eq!(
            plan.validate_with_limits(&PlanLimits {
                max_automation_points: points + 1,
                ..Default::default()
            })
            .unwrap_err()
            .code,
            "E_RESOURCE_LIMIT"
        );
        plan.validate_with_limits(&PlanLimits {
            max_automation_points: points + 2,
            ..Default::default()
        })
        .unwrap();
    }
}

#[test]
fn global_point_limit_precedes_deep_rational_validation() {
    let mut expression = valid_expression();
    expression.points = vec![expression.points[0].clone(); PlanLimits::MAX_AUTOMATION_POINTS + 1];
    expression.points[0].value = Rational::new_raw(1.into(), 0.into());
    let plan = with_expression(expression);
    let error = plan
        .validate_with_limits(&PlanLimits {
            max_automation_points: usize::MAX,
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(error.code, "E_RESOURCE_LIMIT");
}
