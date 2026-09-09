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
fn expression_payload_roundtrips_in_both_versions() {
    for version in [1, 2] {
        let mut value = serde_json::to_value(base_plan()).unwrap();
        value["version"] = json!(version);
        if version == 2 {
            value["instruments"] = json!({"entry_source":"score.maac", "programs":[],
                "wavetables":[],"wavetable_sources":[], "dependencies":[], "libraries":[],
                "source_files":[{"path":"score.maac","hash":format!("sha256:{}", "1".repeat(64))}]});
        }
        value["events"][0]["kind"]["pitch_expression"] = json!({
            "clock": "normalized", "points": [
                {"position":"0/1", "cents":"0/1", "shape":"linear"},
                {"position":"1/1", "cents":"1200/1", "shape":"step"}
            ]
        });
        let plan = Plan::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(&plan).unwrap(), value);
    }
}

fn expression(
    clock: PitchExpressionClock,
    points: &[(i64, i64, i64, Interpolation)],
) -> PitchExpression {
    PitchExpression {
        clock,
        points: points
            .iter()
            .map(|&(n, d, c, shape)| PitchExpressionPoint {
                position: rat(n, d),
                cents: rat(c, 1),
                shape,
            })
            .collect(),
    }
}
fn with_expression(expression: PitchExpression) -> Plan {
    let mut plan = base_plan();
    let EventKind::Note {
        pitch_expression, ..
    } = &mut plan.events[0].kind
    else {
        unreachable!()
    };
    *pitch_expression = Some(expression);
    plan
}
fn valid_expression() -> PitchExpression {
    expression(
        PitchExpressionClock::Normalized,
        &[
            (0, 1, 0, Interpolation::Linear),
            (1, 1, 1200, Interpolation::Step),
        ],
    )
}
#[test]
fn invalid_structure_is_rejected_after_generic_serde_and_in_memory() {
    let mut variants = Vec::new();
    let valid = valid_expression();
    let mut bad = valid.clone();
    bad.points.clear();
    variants.push(bad);
    let mut bad = valid.clone();
    bad.points[0].position = rat(1, 2);
    variants.push(bad);
    let mut bad = valid.clone();
    bad.points[1].position = rat(0, 1);
    variants.push(bad);
    let mut bad = valid.clone();
    bad.points[1].position = rat(-1, 1);
    variants.push(bad);
    let mut bad = valid.clone();
    bad.points[1].position = rat(2, 1);
    variants.push(bad);
    let mut bad = valid.clone();
    bad.points[1].shape = Interpolation::Linear;
    variants.push(bad);
    let mut bad = valid;
    bad.points[0].shape = Interpolation::Exponential;
    variants.push(bad);
    for bad in variants {
        let plan = with_expression(bad);
        assert!(plan.validate().is_err());
        assert!(serde_json::from_value::<Plan>(serde_json::to_value(&plan).unwrap()).is_err());
    }
}
#[test]
fn strict_wire_payload_rejects_unknown_fields_and_noncanonical_rationals() {
    let valid = serde_json::to_value(with_expression(valid_expression())).unwrap();
    for (field, replacement) in [("clock", json!("beats")), ("surprise", json!(true))] {
        let mut bad = valid.clone();
        bad["events"][0]["kind"]["pitch_expression"][field] = replacement;
        assert!(serde_json::from_value::<Plan>(bad).is_err());
    }
    for (field, replacement) in [
        ("position", json!("0/2")),
        ("cents", json!("2/2")),
        ("extra", json!(true)),
    ] {
        let mut bad = valid.clone();
        bad["events"][0]["kind"]["pitch_expression"]["points"][0][field] = replacement;
        assert!(serde_json::from_value::<Plan>(bad).is_err());
    }
}
#[test]
fn exact_rational_limits_and_invalid_raw_values_fail_without_panics() {
    for value in [
        Rational::new_raw(1.into(), 0.into()),
        Rational::new_raw(1.into(), (-1).into()),
        Rational::new_raw(2.into(), 2.into()),
        Rational::from_integer(num_bigint::BigInt::from(1) << 4097),
    ] {
        for position in [true, false] {
            let mut expression = valid_expression();
            if position {
                expression.points[0].position = value.clone();
            } else {
                expression.points[0].cents = value.clone();
            }
            assert!(with_expression(expression).validate().is_err());
        }
    }
    let limits = PlanLimits {
        max_rational_bits: 16,
        ..PlanLimits::default()
    };
    let mut expression = valid_expression();
    expression.points[1].cents = rat(1, 100_003);
    assert_eq!(
        with_expression(expression)
            .validate_with_limits(&limits)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
}
#[test]
fn all_clocks_validate_effective_domain_and_gate_endpoint() {
    for clock in [
        PitchExpressionClock::Score,
        PitchExpressionClock::Seconds,
        PitchExpressionClock::Normalized,
    ] {
        with_expression(expression(
            clock,
            &[
                (0, 1, 0, Interpolation::Linear),
                (1, 1, 1200, Interpolation::Step),
            ],
        ))
        .validate()
        .unwrap();
    }
    // Seconds gate is 1/2 second; an unused future step may exceed Nyquist.
    with_expression(expression(
        PitchExpressionClock::Seconds,
        &[
            (0, 1, 0, Interpolation::Step),
            (1, 1, 12000, Interpolation::Step),
        ],
    ))
    .validate()
    .unwrap();
    // The future linear endpoint contributes only its interpolation at gate end.
    with_expression(expression(
        PitchExpressionClock::Seconds,
        &[
            (0, 1, 0, Interpolation::Linear),
            (100, 1, 12000, Interpolation::Step),
        ],
    ))
    .validate()
    .unwrap();
    for points in [
        vec![
            (0, 1, 0, Interpolation::Linear),
            (1, 2, 12000, Interpolation::Step),
        ],
        vec![
            (0, 1, 0, Interpolation::Step),
            (1, 2, 12000, Interpolation::Step),
        ],
        vec![
            (0, 1, 0, Interpolation::Linear),
            (1, 1, 24000, Interpolation::Step),
        ],
        vec![(0, 1, -2000000, Interpolation::Step)],
    ] {
        assert_eq!(
            with_expression(expression(PitchExpressionClock::Seconds, &points))
                .validate()
                .unwrap_err()
                .code,
            "E_RANGE"
        );
    }
}
#[test]
fn expression_points_share_global_and_aggregate_budgets() {
    let plain = base_plan();
    let mut plan = with_expression(valid_expression());
    let limits = PlanLimits {
        max_automation_points: 1,
        ..PlanLimits::default()
    };
    assert_eq!(
        plan.validate_with_limits(&limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
    plan.automation.push(Automation {
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
    let limits = PlanLimits {
        max_automation_points: 2,
        ..PlanLimits::default()
    };
    assert_eq!(
        plan.validate_with_limits(&limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
    for objects in [true, false] {
        let first_pass = (0..100)
            .find(|&limit| {
                let mut limits = PlanLimits::default();
                if objects {
                    limits.max_objects = limit;
                } else {
                    limits.max_work = limit as u64;
                }
                plain.validate_with_limits(&limits).is_ok()
            })
            .unwrap();
        let mut limits = PlanLimits::default();
        if objects {
            limits.max_objects = first_pass;
        } else {
            limits.max_work = first_pass as u64;
        }
        assert_eq!(
            with_expression(valid_expression())
                .validate_with_limits(&limits)
                .unwrap_err()
                .code,
            "E_RESOURCE_LIMIT"
        );
    }
}
#[test]
fn expression_free_plan_preserves_legacy_serialized_shape() {
    let plan = base_plan();
    assert!(!String::from_utf8(plan.to_json().unwrap())
        .unwrap()
        .contains("pitch_expression"));
    assert_eq!(Plan::from_json(&plan.to_json().unwrap()).unwrap(), plan);
}

#[test]
fn exact_nyquist_and_nonfinite_cents_are_rejected() {
    let mut plan = with_expression(valid_expression());
    let EventKind::Note { pitch_hz, .. } = &mut plan.events[0].kind else {
        unreachable!()
    };
    *pitch_hz = 12000.0;
    assert_eq!(plan.validate().unwrap_err().code, "E_RANGE");
    let mut expression = valid_expression();
    expression.points[1].cents = Rational::from_integer(num_bigint::BigInt::from(1) << 2000);
    assert_eq!(
        with_expression(expression).validate().unwrap_err().code,
        "E_NONFINITE"
    );
}
#[test]
fn large_finite_exact_ratios_do_not_require_finite_numerators() {
    let huge = num_bigint::BigInt::from(1) << 2000;
    let mut expression = valid_expression();
    expression.points[0].cents = Rational::new(&huge + 1, huge);
    with_expression(expression).validate().unwrap();
}
