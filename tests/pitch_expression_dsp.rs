use maac::dsp::{render, DspEngine};
use maac::plan::*;
use std::collections::BTreeMap;
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
        kind: EventKind::Note {
            pitch_hz,
            velocity,
            pitch_expression: None,
        },
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

fn collect(plan: &Plan) -> Vec<f64> {
    let mut out = vec![];
    render(plan, |frame| {
        out.push(frame[0]);
        Ok(())
    })
    .unwrap();
    out
}
fn expression(
    clock: PitchExpressionClock,
    end: Rational,
    start: i64,
    finish: i64,
    shape: Interpolation,
) -> PitchExpression {
    PitchExpression {
        clock,
        points: vec![
            PitchExpressionPoint {
                position: r(0, 1),
                cents: r(start, 1),
                shape,
            },
            PitchExpressionPoint {
                position: end,
                cents: r(finish, 1),
                shape: Interpolation::Step,
            },
        ],
    }
}
fn fixture(expr: Option<PitchExpression>) -> Plan {
    let mut note = event("n", "sine", 0, 8, 3000.0, r(1, 1));
    if let EventKind::Note {
        pitch_expression, ..
    } = &mut note.kind
    {
        *pitch_expression = expr;
    }
    sine_plan(
        8,
        4,
        4,
        &[
            ("attack", r(0, 1)),
            ("release", r(4, 48000)),
            ("level", r(1, 1)),
        ],
        vec![note],
    )
}
#[test]
fn constant_octave_matches_double_frequency_exactly() {
    let bent = fixture(Some(expression(
        PitchExpressionClock::Normalized,
        r(1, 1),
        1200,
        1200,
        Interpolation::Step,
    )));
    let mut plain = fixture(None);
    if let EventKind::Note { pitch_hz, .. } = &mut plain.events[0].kind {
        *pitch_hz *= 2.0;
    }
    assert_eq!(collect(&bent), collect(&plain));
}

fn expected(on: u64, off: u64, frames: u64, base: f64, cents: impl Fn(u64) -> f64) -> Vec<f64> {
    let mut phase: f64 = 0.0;
    (0..frames)
        .map(|n| {
            if n < on {
                return 0.0;
            }
            let envelope = if n < off {
                1.0
            } else {
                (1.0 - (n - off) as f64 / 4.0).max(0.0)
            };
            let sample = envelope * phase.sin();
            let frequency = base * 2.0_f64.powf(cents((n - on).min(off - on)) / 1200.0);
            phase = (phase + 2.0 * std::f64::consts::PI * frequency / 48000.0)
                .rem_euclid(2.0 * std::f64::consts::PI);
            sample
        })
        .collect()
}
fn close(actual: &[f64], expected: &[f64]) {
    assert_eq!(actual.len(), expected.len());
    for (n, (a, b)) in actual.iter().zip(expected).enumerate() {
        assert!((a - b).abs() < 1e-13, "frame {n}: {a} != {b}");
    }
}
#[test]
fn all_clocks_step_and_linear_follow_reference_recurrence_and_hold_release() {
    for (clock, end) in [
        (PitchExpressionClock::Normalized, r(1, 1)),
        (PitchExpressionClock::Seconds, r(8, 48000)),
        (PitchExpressionClock::Score, r(8, 24000)),
    ] {
        for shape in [Interpolation::Step, Interpolation::Linear] {
            let plan = fixture(Some(expression(clock, end.clone(), 0, 1200, shape)));
            close(
                &collect(&plan),
                &expected(0, 8, 12, 3000.0, |n| {
                    if shape == Interpolation::Linear {
                        n as f64 * 150.0
                    } else if n == 8 {
                        1200.0
                    } else {
                        0.0
                    }
                }),
            );
        }
    }
}
#[test]
fn step_knots_use_exact_right_continuity_even_when_binary64_coordinates_coincide() {
    for (position, first_bent) in [
        (r(4, 48000), 4),
        (rs("400000000000000000001/4800000000000000000000000"), 5),
        (rs("399999999999999999999/4800000000000000000000000"), 4),
    ] {
        let plan = fixture(Some(expression(
            PitchExpressionClock::Seconds,
            position,
            0,
            1200,
            Interpolation::Step,
        )));
        close(
            &collect(&plan),
            &expected(
                0,
                8,
                12,
                3000.0,
                |n| if n >= first_bent { 1200.0 } else { 0.0 },
            ),
        );
    }
}
#[test]
fn fractional_offsets_use_ceiled_scheduled_gate_and_overlaps_keep_independent_phase() {
    let mut plan = fixture(Some(expression(
        PitchExpressionClock::Normalized,
        r(1, 1),
        0,
        1200,
        Interpolation::Linear,
    )));
    let note = &mut plan.events[0];
    note.onset_offset_seconds = r(1, 96000);
    note.release_offset_seconds = r(-1, 96000);
    note.on_seconds = r(1, 96000);
    note.off_seconds = Some(r(15, 96000));
    note.on_frame = 1;
    let mut other = event("a", "sine", 2, 6, 1500.0, r(1, 1));
    if let EventKind::Note {
        pitch_expression, ..
    } = &mut other.kind
    {
        *pitch_expression = Some(expression(
            PitchExpressionClock::Normalized,
            r(1, 1),
            -1200,
            -1200,
            Interpolation::Step,
        ));
    }
    plan.events.push(other);
    let a = expected(1, 8, 12, 3000.0, |n| n as f64 / 7.0 * 1200.0);
    let b = expected(2, 6, 12, 1500.0, |_| -1200.0);
    close(
        &collect(&plan),
        &a.iter().zip(b).map(|(a, b)| a + b).collect::<Vec<_>>(),
    );
}
#[test]
fn score_clock_is_affine_across_tempo_change() {
    let mut plan = fixture(Some(expression(
        PitchExpressionClock::Score,
        r(6, 24000),
        0,
        1200,
        Interpolation::Linear,
    )));
    plan.tempo.points.push(TempoPoint {
        q: r(2, 24000),
        bpm: r(60, 1),
        shape: Interpolation::Step,
    });
    plan.output.score_end_q = r(6, 24000);
    plan.output.total_frames = 14;
    plan.events[0].score_off_q = Some(r(6, 24000));
    plan.events[0].off_seconds = Some(r(10, 48000));
    plan.events[0].off_frame = Some(10);
    close(
        &collect(&plan),
        &expected(0, 10, 14, 3000.0, |n| n as f64 * 120.0),
    );
}
#[test]
fn source_free_roundtrip_and_engine_reset_match_and_legacy_pcm_is_exact() {
    for expressed in [false, true] {
        let plan = fixture(expressed.then(|| {
            expression(
                PitchExpressionClock::Normalized,
                r(1, 1),
                0,
                1200,
                Interpolation::Linear,
            )
        }));
        let bytes = serde_json::to_vec(&plan).unwrap();
        if !expressed {
            assert!(!String::from_utf8(bytes.clone())
                .unwrap()
                .contains("pitch_expression"));
        }
        let decoded = Plan::from_json(&bytes).unwrap();
        let mut engine = DspEngine::new(&decoded).unwrap();
        for _ in 0..2 {
            let mut samples = vec![];
            engine
                .render(|frame| {
                    samples.push(frame[0]);
                    Ok(())
                })
                .unwrap();
            assert_eq!(samples, collect(&plan));
            if !expressed {
                assert_eq!(samples, expected(0, 8, 12, 3000.0, |_| 0.0));
            }
        }
    }
}
#[test]
fn invalid_bent_frequency_is_rejected_before_output() {
    for cents in [3600, -2000000] {
        let plan = fixture(Some(expression(
            PitchExpressionClock::Normalized,
            r(1, 1),
            cents,
            cents,
            Interpolation::Step,
        )));
        let mut called = false;
        assert!(render(&plan, |_| {
            called = true;
            Ok(())
        })
        .is_err());
        assert!(!called);
    }
}
