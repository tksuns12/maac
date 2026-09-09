use std::panic::{catch_unwind, AssertUnwindSafe};

use maac::plan::{
    Automation, AutomationClock, AutomationPoint, Connection, EventKind, EventTarget,
    Interpolation, Node, OutputSettings, Plan, PortRef, Processor, Rational, Region, ResolvedEvent,
    SourceMapping, TempoMap, TempoPoint,
};
use num_bigint::BigInt;
use serde_json::Value;

fn rat(numerator: i64, denominator: i64) -> Rational {
    maac::parse_rational(&format!("{numerator}/{denominator}")).expect("test rational")
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

fn json_of(plan: &Plan) -> Vec<u8> {
    plan.to_json().expect("base plan must serialize")
}

fn add_filter_chain(plan: &mut Plan) {
    plan.nodes
        .push(Node::new("filter", Processor::one_pole(1)).unwrap());
    plan.connections.push(
        Connection::new(
            "sine_to_filter",
            PortRef::new("sine", "out").unwrap(),
            PortRef::new("filter", "in").unwrap(),
        )
        .unwrap(),
    );
    plan.connections.push(
        Connection::new(
            "filter_to_sum",
            PortRef::new("filter", "out").unwrap(),
            PortRef::new("sum", "in").unwrap(),
        )
        .unwrap(),
    );
}

fn mutate_json(mutator: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(&json_of(&base_plan())).unwrap();
    mutator(&mut value);
    serde_json::to_vec(&value).unwrap()
}

fn imported_code(bytes: &[u8]) -> String {
    let result = catch_unwind(AssertUnwindSafe(|| Plan::from_json(bytes)));
    let result = result.expect("malformed imported plan must not unwind");
    result.unwrap_err().code
}

fn in_memory_code(mutator: impl FnOnce(&mut Plan)) -> String {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut plan = base_plan();
        mutator(&mut plan);
        plan.validate()
    }));
    let result = result.expect("malformed in-memory plan must not unwind");
    result.unwrap_err().code
}

#[test]
fn zero_bpm_is_a_tempo_error_without_unwinding() {
    assert_eq!(
        in_memory_code(|plan| plan.tempo.points[0].bpm = rat(0, 1)),
        "E_TEMPO"
    );
    assert_eq!(
        imported_code(&mutate_json(
            |value| value["tempo"]["points"][0]["bpm"] = Value::String("0/1".into())
        )),
        "E_TEMPO"
    );
}

#[test]
fn duplicate_json_params_are_rejected_before_last_writer_wins() {
    let valid = String::from_utf8(json_of(&base_plan())).unwrap();
    let duplicate = valid.replacen(
        "\"params\":{}",
        "\"params\":{\"level\":\"1/1\"},\"params\":{\"level\":\"0/1\"}",
        1,
    );
    assert!(duplicate.contains("\"params\":{\"level\":\"1/1\"},\"params\""));
    assert_eq!(imported_code(duplicate.as_bytes()), "E_DUPLICATE_FIELD");
}

#[test]
fn exponential_curve_with_zero_right_endpoint_is_rejected() {
    let code = in_memory_code(|plan| {
        plan.automation.push(Automation {
            id: "level_curve".into(),
            target: PortRef::new("sine", "level").unwrap(),
            clock: AutomationClock::Score,
            at: rat(0, 1),
            points: vec![
                AutomationPoint {
                    position: rat(0, 1),
                    value: rat(1, 1),
                    shape: Interpolation::Exponential,
                },
                AutomationPoint {
                    position: rat(1, 1),
                    value: rat(0, 1),
                    shape: Interpolation::Step,
                },
            ],
        });
    });
    assert_eq!(code, "E_RANGE");
}

#[test]
fn negative_processor_parameters_are_rejected() {
    for parameter in ["attack", "release", "level"] {
        let code = in_memory_code(|plan| {
            plan.nodes[0]
                .params
                .insert(parameter.to_owned(), rat(-1, 1));
        });
        assert_eq!(code, "E_RANGE", "negative sine parameter {parameter}");
    }

    let code = in_memory_code(|plan| {
        let mut filter = Node::new("filter", Processor::one_pole(1)).unwrap();
        filter.params.insert("cutoff".into(), rat(-1, 1));
        plan.nodes.push(filter);
    });
    assert_eq!(code, "E_RANGE");
}

#[test]
fn negative_automation_parameters_are_rejected() {
    for parameter in ["attack", "release", "level"] {
        let code = in_memory_code(|plan| {
            plan.automation.push(Automation {
                id: format!("{parameter}_curve"),
                target: PortRef::new("sine", parameter).unwrap(),
                clock: AutomationClock::Score,
                at: rat(0, 1),
                points: vec![AutomationPoint {
                    position: rat(0, 1),
                    value: rat(-1, 1),
                    shape: Interpolation::Step,
                }],
            });
        });
        assert_eq!(code, "E_RANGE", "negative automation parameter {parameter}");
    }

    let code = in_memory_code(|plan| {
        add_filter_chain(plan);
        plan.automation.push(Automation {
            id: "cutoff_curve".into(),
            target: PortRef::new("filter", "cutoff").unwrap(),
            clock: AutomationClock::Score,
            at: rat(0, 1),
            points: vec![AutomationPoint {
                position: rat(0, 1),
                value: rat(-1, 1),
                shape: Interpolation::Step,
            }],
        });
    });
    assert_eq!(code, "E_RANGE", "negative cutoff automation");
}

#[test]
fn cutoff_at_or_above_nyquist_is_rejected_for_nodes_and_automation() {
    let code = in_memory_code(|plan| {
        let mut filter = Node::new("filter", Processor::one_pole(1)).unwrap();
        filter.params.insert("cutoff".into(), rat(24_000, 1));
        plan.nodes.push(filter);
    });
    assert_eq!(code, "E_RANGE");

    let code = in_memory_code(|plan| {
        add_filter_chain(plan);
        plan.automation.push(Automation {
            id: "cutoff_curve".into(),
            target: PortRef::new("filter", "cutoff").unwrap(),
            clock: AutomationClock::Score,
            at: rat(0, 1),
            points: vec![AutomationPoint {
                position: rat(0, 1),
                value: rat(24_000, 1),
                shape: Interpolation::Step,
            }],
        });
    });
    assert_eq!(code, "E_RANGE");
}

#[test]
fn pan_outside_nominal_range_is_accepted_for_clamp_policy() {
    let mut plan = base_plan();
    let mut pan = Node::new("pan", Processor::pan()).unwrap();
    pan.params.insert("pan".into(), rat(2, 1));
    plan.nodes.push(pan);
    plan.connections.push(
        Connection::new(
            "sum_to_pan",
            PortRef::new("sum", "out").unwrap(),
            PortRef::new("pan", "in").unwrap(),
        )
        .unwrap(),
    );
    plan.validate().expect("pan raw values use clamp policy");
}

#[test]
fn wrong_schedule_frame_and_unsupported_rate_fail_explicitly() {
    assert_eq!(
        in_memory_code(|plan| plan.events[0].on_frame = 1),
        "E_INTERVAL"
    );
    assert_eq!(
        imported_code(&mutate_json(
            |value| value["events"][0]["on_frame"] = Value::from(1)
        )),
        "E_INTERVAL"
    );

    assert_eq!(
        in_memory_code(|plan| plan.output.sample_rate_hz = 44_100),
        "E_CAPABILITY"
    );
    assert_eq!(
        imported_code(&mutate_json(
            |value| value["output"]["sample_rate_hz"] = Value::from(44_100)
        )),
        "E_CAPABILITY"
    );
}

#[test]
fn unknown_processor_and_plan_versions_fail_without_unwinding() {
    assert_eq!(
        imported_code(&mutate_json(|value| {
            value["nodes"][0]["processor"]["kind"] = Value::String("unknown".into());
        })),
        "E_UNKNOWN_KIND"
    );

    assert_eq!(
        imported_code(&mutate_json(|value| value["version"] = Value::from(99))),
        "E_VERSION"
    );
    assert_eq!(in_memory_code(|plan| plan.version = 99), "E_VERSION");
}

#[test]
fn rational_bit_bombs_fail_at_import_and_in_memory_boundaries() {
    let huge = format!("1/{}", "9".repeat(2_000));
    let bytes = mutate_json(|value| value["output"]["score_end_q"] = Value::String(huge));
    assert_eq!(imported_code(&bytes), "E_RESOURCE_LIMIT");

    let too_wide = Rational::from_integer(BigInt::from(1u8) << 4096);
    assert_eq!(
        in_memory_code(|plan| plan.output.score_end_q = too_wide),
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn in_memory_zero_denominator_is_rejected_without_unwinding() {
    let invalid = Rational::new_raw(BigInt::from(1u8), BigInt::from(0u8));
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut plan = base_plan();
        plan.output.score_end_q = invalid;
        plan.validate()
    }));
    let validation = result.expect("zero denominator must return an error, not panic");
    assert_eq!(validation.unwrap_err().code, "E_NONFINITE");
}

#[test]
fn malformed_source_mappings_are_rejected() {
    assert_eq!(
        in_memory_code(|plan| plan.events[0].source.path.clear()),
        "E_REFERENCE"
    );
    assert_eq!(
        in_memory_code(|plan| plan.events[0].source.object.clear()),
        "E_RANGE"
    );

    let mut plan = base_plan();
    plan.source_mappings = vec![
        SourceMapping {
            object: "n1".into(),
            path: vec!["main".into(), "n1".into()],
            span: None,
        },
        SourceMapping {
            object: "n1".into(),
            path: vec!["main".into(), "other".into()],
            span: None,
        },
    ];
    assert_eq!(plan.validate().unwrap_err().code, "E_DUPLICATE_ID");

    assert_eq!(
        in_memory_code(|plan| {
            plan.source_mappings.push(SourceMapping {
                object: "n2".into(),
                path: vec![String::new()],
                span: None,
            });
        }),
        "E_RANGE"
    );
}

#[test]
fn imported_negative_tempo_segments_preserve_asymmetric_inverse() {
    let mut plan = base_plan();
    plan.tempo.points = vec![
        TempoPoint {
            q: rat(-4, 1),
            bpm: rat(60, 1),
            shape: Interpolation::Step,
        },
        TempoPoint {
            q: rat(0, 1),
            bpm: rat(120, 1),
            shape: Interpolation::Step,
        },
    ];
    let imported = Plan::from_json(&json_of(&plan)).expect("negative tempo map should import");
    assert_eq!(imported.tempo.score_at(&rat(-2, 1)).unwrap(), rat(-2, 1));
}
