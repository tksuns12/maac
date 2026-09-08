use maac::plan::{
    Automation, AutomationClock, AutomationPoint, Connection, EventKind, EventTarget,
    Interpolation, Node, OutputSettings, Plan, PlanLimits, PortRef, Processor, Rational, Region,
    ResolvedEvent, SourceMapping, TempoMap, TempoPoint,
};
use num_bigint::BigInt;

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
    }
}

#[test]
fn valid_plan_round_trips_with_canonical_rationals_and_exact_ceiling() {
    let mut plan = base_plan();
    plan.events[0].onset_offset_seconds = rat(1, 48_000);
    plan.events[0].on_seconds = rat(1, 48_000);
    plan.events[0].off_seconds = Some(rat(1, 2));
    plan.events[0].on_frame = 1;
    plan.events[0].off_frame = Some(24_000);
    plan.output.total_frames = 96_000;

    plan.validate().unwrap();
    let json = plan.to_json().unwrap();
    let text = String::from_utf8(json).unwrap();
    assert!(text.contains("\"0/1\""));
    assert_eq!(Plan::from_json(text.as_bytes()).unwrap(), plan);
}

#[test]
fn rejects_a_schedule_that_rounds_an_event_earlier_or_loses_a_gate() {
    let mut plan = base_plan();
    plan.events[0].onset_offset_seconds = rat(1, 96_001);
    plan.events[0].on_seconds = rat(1, 96_001);
    plan.events[0].on_frame = 0;
    let err = plan.validate().unwrap_err();
    assert_eq!(err.code, "E_INTERVAL");

    let mut plan = base_plan();
    let epsilon = rat(1, 1_000_000_000);
    plan.events[0].score_off_q = Some(rat(1, 1_000_000_000));
    plan.events[0].onset_offset_seconds = rat(1, 48_000) - epsilon.clone();
    plan.events[0].release_offset_seconds = rat(1, 48_000) - epsilon;
    plan.events[0].on_seconds = plan.events[0].onset_offset_seconds.clone();
    plan.events[0].off_seconds =
        Some(rat(1, 2_000_000_000) + plan.events[0].release_offset_seconds.clone());
    plan.events[0].on_frame = 1;
    plan.events[0].off_frame = Some(1);
    let err = plan.validate().unwrap_err();
    assert_eq!(err.code, "E_SUBSAMPLE_NOTE");
}

#[test]
fn effective_note_off_clamps_after_physical_release_offset() {
    let mut plan = base_plan();
    plan.output.score_end_q = rat(1, 1);
    plan.output.total_frames = 24_000;
    plan.regions[0].end_q = rat(1, 1);
    plan.events[0].score_off_q = Some(rat(2, 1));
    plan.events[0].release_offset_seconds = rat(-1, 4);
    plan.events[0].off_seconds = Some(rat(1, 2));
    plan.events[0].off_frame = Some(24_000);
    plan.validate().unwrap();
}

#[test]
fn large_exact_score_origin_needs_no_binary64_timing_conversion() {
    let mut plan = base_plan();
    let origin = Rational::from_integer(BigInt::from(1u8) << 4_000);
    plan.output.score_start_q = origin.clone();
    plan.output.score_end_q = origin + rat(4, 1);
    plan.output.total_frames = 96_000;
    plan.events.clear();
    plan.regions.clear();
    plan.validate().unwrap();
}

#[test]
fn voice_capacity_zero_is_range_and_excess_is_resource() {
    let mut plan = base_plan();
    plan.nodes[0].processor = Processor::sine(0);
    assert_eq!(plan.validate().unwrap_err().code, "E_RANGE");

    let mut plan = base_plan();
    plan.nodes[0].processor = Processor::sine(PlanLimits::default().max_voices + 1);
    assert_eq!(plan.validate().unwrap_err().code, "E_RESOURCE_LIMIT");
}

#[test]
fn rejects_unknown_json_fields_and_oversized_json_before_deserialization() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&base_plan().to_json().unwrap()).unwrap();
    value["unexpected"] = serde_json::json!(true);
    let err = Plan::from_json(serde_json::to_vec(&value).unwrap().as_slice()).unwrap_err();
    assert_eq!(err.code, "E_UNKNOWN_FIELD");

    let oversized = vec![b' '; PlanLimits::default().max_json_bytes + 1];
    let err = Plan::from_json(&oversized).unwrap_err();
    assert_eq!(err.code, "E_RESOURCE_LIMIT");

    let json = String::from_utf8(base_plan().to_json().unwrap())
        .unwrap()
        .replacen(
            "\"params\":{}",
            "\"params\":{\"level\":\"1/1\",\"level\":\"2/1\"}",
            1,
        );
    let err = Plan::from_json(json.as_bytes()).unwrap_err();
    assert_eq!(err.code, "E_DUPLICATE_FIELD");

    let duplicate_root = String::from_utf8(base_plan().to_json().unwrap())
        .unwrap()
        .replacen("\"version\":1,", "\"version\":1,\"version\":1,", 1);
    let err = Plan::from_json(duplicate_root.as_bytes()).unwrap_err();
    assert_eq!(err.code, "E_DUPLICATE_FIELD");

    let mut malformed: serde_json::Value =
        serde_json::from_slice(&base_plan().to_json().unwrap()).unwrap();
    malformed["output"]["score_end_q"] = serde_json::Value::String("1.0".into());
    let err = Plan::from_json(&serde_json::to_vec(&malformed).unwrap()).unwrap_err();
    assert_eq!(err.code, "E_SYNTAX");
}

#[test]
fn rejects_bad_references_duplicate_single_inputs_and_graph_cycles() {
    let mut plan = base_plan();
    plan.events[0].target = EventTarget::new("missing", "events").unwrap();
    assert_eq!(plan.validate().unwrap_err().code, "E_REFERENCE");

    let mut plan = base_plan();
    plan.nodes
        .push(Node::new("filter", Processor::one_pole(1)).unwrap());
    plan.connections.push(
        Connection::new(
            "sum_to_filter",
            PortRef::new("sum", "out").unwrap(),
            PortRef::new("filter", "in").unwrap(),
        )
        .unwrap(),
    );
    plan.connections.push(
        Connection::new(
            "filter_to_filter",
            PortRef::new("filter", "out").unwrap(),
            PortRef::new("filter", "in").unwrap(),
        )
        .unwrap(),
    );
    let err = plan.validate().unwrap_err();
    assert_eq!(err.code, "E_PORT_TYPE");

    let mut plan = base_plan();
    plan.connections[0].from = PortRef::new("sum", "out").unwrap();
    plan.connections.push(
        Connection::new(
            "sum_back",
            PortRef::new("sum", "out").unwrap(),
            PortRef::new("sine", "events").unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(plan.validate().unwrap_err().code, "E_PORT_TYPE");
}

#[test]
fn rejects_duplicate_automation_writers_and_invalid_curve_shapes() {
    let mut plan = base_plan();
    plan.automation = vec![
        Automation {
            id: "cutoff_a".into(),
            target: PortRef::new("sine", "level").unwrap(),
            clock: AutomationClock::Score,
            at: rat(0, 1),
            points: vec![
                AutomationPoint {
                    position: rat(0, 1),
                    value: rat(1, 1),
                    shape: Interpolation::Linear,
                },
                AutomationPoint {
                    position: rat(1, 1),
                    value: rat(0, 1),
                    shape: Interpolation::Step,
                },
            ],
        },
        Automation {
            id: "cutoff_b".into(),
            target: PortRef::new("sine", "level").unwrap(),
            clock: AutomationClock::Seconds,
            at: rat(0, 1),
            points: vec![AutomationPoint {
                position: rat(0, 1),
                value: rat(1, 1),
                shape: Interpolation::Step,
            }],
        },
    ];
    assert_eq!(plan.validate().unwrap_err().code, "E_AUTOMATION_WRITER");

    let mut plan = base_plan();
    plan.automation = vec![Automation {
        id: "bad".into(),
        target: PortRef::new("sine", "level").unwrap(),
        clock: AutomationClock::Score,
        at: rat(0, 1),
        points: vec![AutomationPoint {
            position: rat(0, 1),
            value: rat(0, 1),
            shape: Interpolation::Exponential,
        }],
    }];
    assert_eq!(plan.validate().unwrap_err().code, "E_RANGE");
}

#[test]
fn rejects_rational_bit_bombs_and_in_memory_work_limits() {
    let huge = format!("1/{}", "9".repeat(2_000));
    let json = format!(
        r#"{{"version":1,"output":{{"score_start_q":"0/1","score_end_q":"{huge}","tail_seconds":"0/1","sample_rate_hz":48000,"channels":1,"total_frames":24000,"output":{{"node":"sum","port":"out"}}}},"tempo":{{"points":[{{"q":"0/1","bpm":"120/1","shape":"step"}}]}},"events":[],"nodes":[{{"id":"sum","processor":{{"kind":"sum","config":{{"channels":1}}}},"params":{{}}}}],"connections":[],"automation":[],"regions":[],"source_mappings":[]}}"#
    );
    let err = Plan::from_json(json.as_bytes()).unwrap_err();
    assert_eq!(err.code, "E_RESOURCE_LIMIT");

    let mut plan = base_plan();
    plan.events = (0..=PlanLimits::default().max_events)
        .map(|i| {
            let mut event = plan.events[0].clone();
            event.address = format!("main/{i}");
            event
        })
        .collect();
    assert_eq!(plan.validate().unwrap_err().code, "E_RESOURCE_LIMIT");
}

#[test]
fn preserves_source_spans_and_validates_pan_clamp_and_automation_ranges() {
    let mut plan = base_plan();
    plan.nodes.push({
        let mut node = Node::new("pan", Processor::pan()).unwrap();
        node.params.insert("pan".into(), rat(7, 1));
        node
    });
    plan.connections.push(
        Connection::new(
            "sum_to_pan",
            PortRef::new("sum", "out").unwrap(),
            PortRef::new("pan", "in").unwrap(),
        )
        .unwrap(),
    );
    plan.events[0].source.span = Some(maac::plan::SourceSpan { start: 4, end: 12 });
    plan.validate().unwrap();

    let mut invalid_span = plan.clone();
    invalid_span.events[0].source.span = Some(maac::plan::SourceSpan { start: 12, end: 4 });
    assert_eq!(invalid_span.validate().unwrap_err().code, "E_RANGE");

    let mut invalid_automation = base_plan();
    invalid_automation.automation = vec![Automation {
        id: "level_lane".into(),
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
    }];
    assert_eq!(invalid_automation.validate().unwrap_err().code, "E_RANGE");

    let mut duplicate_ids = base_plan();
    duplicate_ids.automation = vec![
        Automation {
            id: "lane".into(),
            target: PortRef::new("sine", "level").unwrap(),
            clock: AutomationClock::Score,
            at: rat(0, 1),
            points: vec![AutomationPoint {
                position: rat(0, 1),
                value: rat(1, 1),
                shape: Interpolation::Step,
            }],
        },
        Automation {
            id: "lane".into(),
            target: PortRef::new("sine", "attack").unwrap(),
            clock: AutomationClock::Score,
            at: rat(0, 1),
            points: vec![AutomationPoint {
                position: rat(0, 1),
                value: rat(1, 1),
                shape: Interpolation::Step,
            }],
        },
    ];
    assert_eq!(duplicate_ids.validate().unwrap_err().code, "E_DUPLICATE_ID");
}

#[test]
fn in_memory_invalid_rational_denom_is_rejected_without_panicking() {
    let mut plan = base_plan();
    plan.output.score_end_q = Rational::new_raw(BigInt::from(1u8), BigInt::from(0u8));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plan.validate()));
    let validation = result.expect("invalid rational must return a diagnostic");
    assert_eq!(validation.unwrap_err().code, "E_NONFINITE");
}
