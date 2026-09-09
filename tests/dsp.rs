use std::collections::BTreeMap;

use maac::dsp::{one_pole_step, pan_sample, render, sum_samples, RenderError};
use maac::plan::{
    Automation, AutomationClock, AutomationPoint, Connection, EventKind, EventTarget,
    Interpolation, Node, OutputSettings, Plan, PortRef, Processor, Rational, ResolvedEvent,
    SourceMapping, TempoMap, TempoPoint,
};

fn r(numerator: i64, denominator: i64) -> Rational {
    maac::parse_rational(&format!("{numerator}/{denominator}")).unwrap()
}

fn rs(value: &str) -> Rational {
    maac::parse_rational(value).unwrap()
}

fn node(id: &str, processor: Processor, params: &[(&str, Rational)]) -> Node {
    let mut value = Node::new(id, processor).unwrap();
    value.params = params
        .iter()
        .map(|(name, parameter)| ((*name).to_owned(), parameter.clone()))
        .collect::<BTreeMap<_, _>>();
    value
}

fn event(
    address: &str,
    target: &str,
    on_frame: u64,
    off_frame: u64,
    pitch_hz: f64,
    velocity: Rational,
) -> ResolvedEvent {
    let on_q = r(on_frame as i64, 24_000);
    let off_q = r(off_frame as i64, 24_000);
    ResolvedEvent {
        address: address.into(),
        source: SourceMapping {
            object: address.into(),
            path: vec![address.into()],
            span: None,
        },
        target: EventTarget::new(target, "events").unwrap(),
        kind: EventKind::Note { pitch_hz, velocity },
        score_on_q: on_q,
        score_off_q: Some(off_q),
        onset_offset_seconds: r(0, 1),
        release_offset_seconds: r(0, 1),
        on_seconds: r(on_frame as i64, 48_000),
        off_seconds: Some(r(off_frame as i64, 48_000)),
        release_velocity: 0.5,
        on_frame,
        off_frame: Some(off_frame),
        order: 0,
    }
}

fn sine_plan(
    frames: u64,
    tail_frames: u64,
    voices: u32,
    params: &[(&str, Rational)],
    events: Vec<ResolvedEvent>,
) -> Plan {
    Plan {
        version: 1,
        output: OutputSettings {
            score_start_q: r(0, 1),
            score_end_q: r(frames as i64, 24_000),
            tail_seconds: r(tail_frames as i64, 48_000),
            sample_rate_hz: 48_000,
            channels: 1,
            total_frames: frames + tail_frames,
            output: PortRef::new("sine", "out").unwrap(),
        },
        tempo: TempoMap {
            points: vec![TempoPoint {
                q: r(0, 1),
                bpm: r(120, 1),
                shape: Interpolation::Step,
            }],
        },
        events,
        nodes: vec![node("sine", Processor::sine(voices), params)],
        connections: Vec::new(),
        automation: Vec::new(),
        regions: Vec::new(),
        source_mappings: Vec::new(),
        instruments: None,
        production: None,
    }
}

fn route_through_pan(plan: &mut Plan, pan: Rational) {
    plan.nodes
        .push(node("pan", Processor::pan(), &[("pan", pan)]));
    plan.connections.push(
        Connection::new(
            "sine_to_pan",
            PortRef::new("sine", "out").unwrap(),
            PortRef::new("pan", "in").unwrap(),
        )
        .unwrap(),
    );
    plan.output.channels = 2;
    plan.output.output = PortRef::new("pan", "out").unwrap();
}

fn collect(plan: &Plan) -> maac::dsp::Result<Vec<Vec<f64>>> {
    let mut frames = Vec::new();
    render(plan, |frame| {
        frames.push(frame.to_vec());
        Ok(())
    })?;
    Ok(frames)
}

#[test]
fn sine_starts_at_zero_and_matches_binary64_phase_and_level() {
    let plan = sine_plan(
        4,
        0,
        4,
        &[
            ("attack", r(0, 1)),
            ("release", r(0, 1)),
            ("level", r(1, 1)),
        ],
        vec![event("n", "sine", 0, 4, 12_000.0, r(1, 1))],
    );
    let frames = collect(&plan).unwrap();
    println!("sine {frames:?}");
    assert_eq!(frames.len(), 4);
    assert!((frames[0][0] - 0.0).abs() < 1e-15);
    assert!((frames[1][0] - 1.0).abs() < 1e-14);
    assert!(frames[2][0].abs() < 1e-14);
    assert!((frames[3][0] + 1.0).abs() < 1e-14);
}

#[test]
fn one_pole_impulse_uses_reset_state_and_reference_coefficient() {
    let mut previous = 0.0;
    let cutoff = 1_000.0;
    let rate = 48_000.0;
    let a = (-2.0 * std::f64::consts::PI * cutoff / rate).exp();
    let first = one_pole_step(1.0, &mut previous, cutoff, rate).unwrap();
    let second = one_pole_step(0.0, &mut previous, cutoff, rate).unwrap();
    assert!((first - (1.0 - a)).abs() < 1e-15);
    assert!((second - a * first).abs() < 1e-15);
}

#[test]
fn pan_is_equal_power_and_sum_preserves_supplied_reduction_order() {
    let center = pan_sample(1.0, 0.0).unwrap();
    assert!((center[0] - 2.0_f64.sqrt().recip()).abs() < 1e-15);
    assert!((center[1] - center[0]).abs() < 1e-15);
    let left = pan_sample(1.0, -1.0).unwrap();
    let right = pan_sample(1.0, 1.0).unwrap();
    assert!((left[0] - 1.0).abs() < 1e-15);
    assert!(left[1].abs() < 1e-15);
    assert!(right[0].abs() < 1e-15);
    assert!((right[1] - 1.0).abs() < 1e-15);

    let ordered = sum_samples(&[1.0e16, -1.0e16, 1.0]).unwrap();
    let reordered = sum_samples(&[1.0e16, 1.0, -1.0e16]).unwrap();
    assert_eq!(ordered, 1.0);
    assert_eq!(reordered, 0.0);
}

#[test]
fn seconds_automation_knots_take_effect_at_the_first_sample_on_or_after_them() {
    let frames = 3;
    let mut plan = sine_plan(
        frames,
        0,
        4,
        &[
            ("attack", r(0, 1)),
            ("release", r(0, 1)),
            ("level", r(1, 1)),
        ],
        vec![event("n", "sine", 0, frames, 12_000.0, r(1, 1))],
    );
    plan.nodes
        .push(node("pan", Processor::pan(), &[("pan", r(-1, 1))]));
    plan.connections.push(
        Connection::new(
            "sine_to_pan",
            PortRef::new("sine", "out").unwrap(),
            PortRef::new("pan", "in").unwrap(),
        )
        .unwrap(),
    );
    plan.output.channels = 2;
    plan.output.output = PortRef::new("pan", "out").unwrap();
    plan.automation.push(Automation {
        id: "pan_step".into(),
        target: PortRef::new("pan", "pan").unwrap(),
        clock: AutomationClock::Seconds,
        at: r(0, 1),
        points: vec![
            AutomationPoint {
                position: r(0, 1),
                value: r(-1, 1),
                shape: Interpolation::Step,
            },
            AutomationPoint {
                position: r(1, 48_000),
                value: r(1, 1),
                shape: Interpolation::Step,
            },
        ],
    });
    let output = collect(&plan).unwrap();
    println!("pan {output:?}");
    assert!(output[0].iter().all(|sample| sample.abs() < 1e-15));
    assert!(output[1][0].abs() < 1e-14);
    assert!((output[1][1] - 1.0).abs() < 1e-14);
}

#[test]
fn attack_and_release_parameters_are_captured_at_their_events() {
    let mut attack_plan = sine_plan(
        3,
        0,
        4,
        &[
            ("attack", r(1, 48_000)),
            ("release", r(0, 1)),
            ("level", r(1, 1)),
        ],
        vec![event("n", "sine", 0, 3, 12_000.0, r(1, 1))],
    );
    attack_plan.automation.push(Automation {
        id: "attack_later".into(),
        target: PortRef::new("sine", "attack").unwrap(),
        clock: AutomationClock::Seconds,
        at: r(0, 1),
        points: vec![
            AutomationPoint {
                position: r(0, 1),
                value: r(1, 48_000),
                shape: Interpolation::Step,
            },
            AutomationPoint {
                position: r(1, 48_000),
                value: r(1, 1),
                shape: Interpolation::Step,
            },
        ],
    });
    let attack_output = collect(&attack_plan).unwrap();
    println!("attack {attack_output:?}");
    assert!((attack_output[1][0] - 1.0).abs() < 1e-14);

    let mut release_plan = sine_plan(
        2,
        2,
        4,
        &[
            ("attack", r(0, 1)),
            ("release", r(1, 48_000)),
            ("level", r(1, 1)),
        ],
        vec![event("n", "sine", 0, 2, 6_000.0, r(1, 1))],
    );
    release_plan.automation.push(Automation {
        id: "release_later".into(),
        target: PortRef::new("sine", "release").unwrap(),
        clock: AutomationClock::Seconds,
        at: r(0, 1),
        points: vec![
            AutomationPoint {
                position: r(0, 1),
                value: r(1, 48_000),
                shape: Interpolation::Step,
            },
            AutomationPoint {
                position: r(3, 48_000),
                value: r(1, 1),
                shape: Interpolation::Step,
            },
        ],
    });
    let release_output = collect(&release_plan).unwrap();
    assert!((release_output[2][0] - 1.0).abs() < 1e-14);
    assert!(release_output[3][0].abs() < 1e-14);
}

#[test]
fn release_parameter_is_captured_at_note_off() {
    let mut plan = sine_plan(
        4,
        0,
        1,
        &[
            ("attack", r(0, 1)),
            ("release", r(0, 1)),
            ("level", r(1, 1)),
        ],
        vec![event("n", "sine", 0, 1, 6_000.0, r(1, 1))],
    );
    plan.automation.push(Automation {
        id: "release_at_off".into(),
        target: PortRef::new("sine", "release").unwrap(),
        clock: AutomationClock::Seconds,
        at: r(0, 1),
        points: vec![
            AutomationPoint {
                position: r(0, 1),
                value: r(0, 1),
                shape: Interpolation::Step,
            },
            AutomationPoint {
                position: r(1, 48_000),
                value: r(2, 48_000),
                shape: Interpolation::Step,
            },
        ],
    });
    let output = collect(&plan).unwrap();
    assert!((output[2][0] - 0.5).abs() < 1e-14);
    assert!(output[3][0].abs() < 1e-14);
}

#[test]
fn released_voice_reaching_zero_is_reclaimed_before_same_frame_note_on() {
    let plan = sine_plan(
        6,
        0,
        1,
        &[
            ("attack", r(0, 1)),
            ("release", r(3, 48_000)),
            ("level", r(1, 1)),
        ],
        vec![
            event("a", "sine", 0, 1, 440.0, r(1, 1)),
            event("b", "sine", 4, 5, 660.0, r(1, 1)),
        ],
    );
    assert_eq!(collect(&plan).unwrap().len(), 6);
}

#[test]
fn pan_raw_parameter_is_clamped_by_the_reference_processor() {
    let mut plan = sine_plan(
        2,
        0,
        1,
        &[
            ("attack", r(0, 1)),
            ("release", r(0, 1)),
            ("level", r(1, 1)),
        ],
        vec![event("n", "sine", 0, 2, 12_000.0, r(1, 1))],
    );
    route_through_pan(&mut plan, r(2, 1));
    let output = collect(&plan).unwrap();
    assert!(output[1][0].abs() < 1e-14);
    assert!((output[1][1] - 1.0).abs() < 1e-14);
}

#[test]
fn automation_uses_authored_parameter_as_its_pre_anchor_base() {
    let mut plan = sine_plan(
        3,
        0,
        1,
        &[
            ("attack", r(0, 1)),
            ("release", r(0, 1)),
            ("level", r(1, 1)),
        ],
        vec![event("n", "sine", 0, 3, 12_000.0, r(1, 1))],
    );
    route_through_pan(&mut plan, r(-1, 2));
    plan.automation.push(Automation {
        id: "pan_after_start".into(),
        target: PortRef::new("pan", "pan").unwrap(),
        clock: AutomationClock::Seconds,
        at: r(2, 48_000),
        points: vec![AutomationPoint {
            position: r(0, 1),
            value: r(1, 1),
            shape: Interpolation::Step,
        }],
    });
    let output = collect(&plan).unwrap();
    let expected = pan_sample(1.0, -0.5).unwrap();
    assert!((output[1][0] - expected[0]).abs() < 1e-14);
    assert!((output[1][1] - expected[1]).abs() < 1e-14);
}

#[test]
fn seconds_automation_anchor_is_absolute_but_frames_are_reset_relative() {
    let mut plan = sine_plan(
        3,
        0,
        1,
        &[
            ("attack", r(0, 1)),
            ("release", r(0, 1)),
            ("level", r(1, 1)),
        ],
        vec![event("n", "sine", 0, 3, 12_000.0, r(1, 1))],
    );
    let score_start = r(1, 1);
    let score_end = score_start.clone() + r(3, 24_000);
    let mut shifted = event("n", "sine", 0, 3, 12_000.0, r(1, 1));
    shifted.score_on_q = score_start.clone();
    shifted.score_off_q = Some(score_end.clone());
    shifted.on_seconds = r(1, 2);
    shifted.off_seconds = Some(r(1, 2) + r(3, 48_000));
    plan.events = vec![shifted];
    plan.output.score_start_q = score_start;
    plan.output.score_end_q = score_end;
    route_through_pan(&mut plan, r(-1, 1));
    plan.automation.push(Automation {
        id: "absolute_seconds".into(),
        target: PortRef::new("pan", "pan").unwrap(),
        clock: AutomationClock::Seconds,
        at: r(1, 2) + r(1, 48_000),
        points: vec![AutomationPoint {
            position: r(0, 1),
            value: r(1, 1),
            shape: Interpolation::Step,
        }],
    });
    let output = collect(&plan).unwrap();
    assert!(output[1][0].abs() < 1e-14);
    assert!((output[1][1] - 1.0).abs() < 1e-14);
}

#[test]
fn huge_absolute_score_origin_keeps_local_timing_exact() {
    let mut plan = sine_plan(
        96_000,
        0,
        1,
        &[
            ("attack", r(0, 1)),
            ("release", r(0, 1)),
            ("level", r(1, 1)),
        ],
        vec![event("n", "sine", 0, 96_000, 12_000.0, r(1, 1))],
    );
    let score_start = rs("100000000000000000000/1");
    let score_end = score_start.clone() + r(4, 1);
    let origin = plan.tempo.seconds_at(&score_start).unwrap();
    let mut shifted = event("n", "sine", 0, 96_000, 12_000.0, r(1, 1));
    shifted.score_on_q = score_start.clone();
    shifted.score_off_q = Some(score_end.clone());
    shifted.on_seconds = origin.clone();
    shifted.off_seconds = Some(origin.clone() + r(2, 1));
    shifted.on_frame = 0;
    shifted.off_frame = Some(96_000);
    plan.events = vec![shifted];
    plan.output.score_start_q = score_start;
    plan.output.score_end_q = score_end;
    route_through_pan(&mut plan, r(-1, 1));
    plan.automation.push(Automation {
        id: "local_seconds".into(),
        target: PortRef::new("pan", "pan").unwrap(),
        clock: AutomationClock::Seconds,
        at: origin + r(1, 48_000),
        points: vec![AutomationPoint {
            position: r(0, 1),
            value: r(1, 1),
            shape: Interpolation::Step,
        }],
    });
    let output = collect(&plan).unwrap();
    assert!(output[1][0].abs() < 1e-14);
    assert!((output[1][1] - 1.0).abs() < 1e-14);
}

#[test]
fn score_and_seconds_automation_freeze_at_the_exact_score_end_value() {
    for clock in [AutomationClock::Score, AutomationClock::Seconds] {
        let mut plan = sine_plan(
            2,
            2,
            1,
            &[
                ("attack", r(0, 1)),
                ("release", r(3, 48_000)),
                ("level", r(1, 1)),
            ],
            vec![event("n", "sine", 0, 2, 3_000.0, r(1, 1))],
        );
        route_through_pan(&mut plan, r(0, 1));
        let (at, end_position) = match clock {
            AutomationClock::Score => (r(0, 1), r(4, 24_000)),
            AutomationClock::Seconds => (r(0, 1), r(4, 48_000)),
        };
        plan.automation.push(Automation {
            id: format!("freeze_{clock:?}"),
            target: PortRef::new("pan", "pan").unwrap(),
            clock,
            at,
            points: vec![
                AutomationPoint {
                    position: r(0, 1),
                    value: r(0, 1),
                    shape: Interpolation::Linear,
                },
                AutomationPoint {
                    position: end_position,
                    value: r(1, 1),
                    shape: Interpolation::Step,
                },
            ],
        });
        let output = collect(&plan).unwrap();
        let first_tail_ratio = output[2][1] / output[2][0];
        let second_tail_ratio = output[3][1] / output[3][0];
        assert!((first_tail_ratio - second_tail_ratio).abs() < 1e-12);
    }
}

#[test]
fn release_tails_count_toward_voice_capacity_and_overflow_is_explicit() {
    let plan = sine_plan(
        4,
        0,
        1,
        &[
            ("attack", r(0, 1)),
            ("release", r(3, 48_000)),
            ("level", r(1, 1)),
        ],
        vec![
            event("a", "sine", 0, 1, 440.0, r(1, 1)),
            event("b", "sine", 2, 3, 660.0, r(1, 1)),
        ],
    );
    let error = collect(&plan).unwrap_err();
    assert!(matches!(error, RenderError::VoiceLimit { .. }));
    assert_eq!(error.code(), "E_VOICE_LIMIT");
}

#[test]
fn renderer_validates_the_plan_before_producing_frames() {
    let mut plan = sine_plan(
        2,
        0,
        1,
        &[
            ("attack", r(0, 1)),
            ("release", r(0, 1)),
            ("level", r(1, 1)),
        ],
        vec![event("n", "sine", 0, 2, 440.0, r(1, 1))],
    );
    plan.output.total_frames = 1;
    let mut called = false;
    let error = render(&plan, |_| {
        called = true;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code(), "E_INTERVAL");
    assert!(!called);
}
