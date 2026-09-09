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
            timbre_expression: None,
            pitch_hz,
            velocity,
            pitch_expression: None,
            gain_expression: None,
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
    clock: ExpressionClock,
    end: Rational,
    start: Rational,
    finish: Rational,
    shape: Interpolation,
) -> GainExpression {
    GainExpression {
        clock,
        points: vec![
            GainExpressionPoint {
                position: r(0, 1),
                gain: start,
                shape,
            },
            GainExpressionPoint {
                position: end,
                gain: finish,
                shape: Interpolation::Step,
            },
        ],
    }
}
fn set_gain(note: &mut ResolvedEvent, curve: GainExpression) {
    if let EventKind::Note {
        gain_expression, ..
    } = &mut note.kind
    {
        *gain_expression = Some(curve);
    }
}
fn fixture(curve: Option<GainExpression>) -> Plan {
    let mut note = event("n", "sine", 0, 8, 3000.0, r(1, 1));
    if let Some(curve) = curve {
        set_gain(&mut note, curve);
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
fn close(actual: &[f64], expected: &[f64]) {
    assert_eq!(actual.len(), expected.len());
    for (n, (a, b)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (a - b).abs() <= 1e-13 * b.abs().max(1.0),
            "frame {n}: {a} != {b}"
        );
    }
}
#[test]
fn all_clocks_and_shapes_match_analytic_gain_and_hold_gate_end_through_release() {
    let plain = collect(&fixture(None));
    for (clock, end) in [
        (ExpressionClock::Normalized, r(1, 1)),
        (ExpressionClock::Seconds, r(8, 48000)),
        (ExpressionClock::Score, r(8, 24000)),
    ] {
        for shape in [
            Interpolation::Step,
            Interpolation::Linear,
            Interpolation::Exponential,
        ] {
            let plan = fixture(Some(expression(
                clock,
                end.clone(),
                r(1, 4),
                r(1, 1),
                shape,
            )));
            let expected: Vec<_> = plain
                .iter()
                .enumerate()
                .map(|(n, sample)| {
                    let t = n.min(8) as f64 / 8.0;
                    let gain = match shape {
                        Interpolation::Step => {
                            if n < 8 {
                                0.25
                            } else {
                                1.0
                            }
                        }
                        Interpolation::Linear => 0.25 + 0.75 * t,
                        Interpolation::Exponential => 0.25 * 4.0_f64.powf(t),
                    };
                    sample * gain
                })
                .collect();
            close(&collect(&plan), &expected);
        }
    }
}
#[test]
fn exact_step_knots_and_zero_gain_preserve_phase() {
    let plain = collect(&fixture(None));
    for (position, first) in [
        (r(4, 48000), 4),
        (rs("400000000000000000001/4800000000000000000000000"), 5),
        (rs("399999999999999999999/4800000000000000000000000"), 4),
    ] {
        let plan = fixture(Some(expression(
            ExpressionClock::Seconds,
            position,
            r(0, 1),
            r(1, 1),
            Interpolation::Step,
        )));
        let mut expected = plain.clone();
        expected[..first].fill(0.0);
        assert_eq!(collect(&plan), expected);
    }
}
#[test]
fn gain_one_preserves_legacy_and_pitch_pcm_exactly_and_roundtrip_resets() {
    for pitched in [false, true] {
        let mut plain = fixture(None);
        if pitched {
            if let EventKind::Note {
                pitch_expression, ..
            } = &mut plain.events[0].kind
            {
                *pitch_expression = Some(PitchExpression {
                    clock: ExpressionClock::Seconds,
                    points: vec![
                        PitchExpressionPoint {
                            position: r(0, 1),
                            cents: r(0, 1),
                            shape: Interpolation::Linear,
                        },
                        PitchExpressionPoint {
                            position: r(8, 48000),
                            cents: r(1200, 1),
                            shape: Interpolation::Step,
                        },
                    ],
                });
            }
        }
        let mut gained = plain.clone();
        set_gain(
            &mut gained.events[0],
            expression(
                ExpressionClock::Score,
                r(8, 24000),
                r(1, 1),
                r(1, 1),
                Interpolation::Step,
            ),
        );
        assert_eq!(collect(&gained), collect(&plain));
        set_gain(
            &mut gained.events[0],
            expression(
                ExpressionClock::Normalized,
                r(1, 1),
                r(0, 1),
                r(2, 1),
                Interpolation::Linear,
            ),
        );
        let expected: Vec<_> = collect(&plain)
            .iter()
            .enumerate()
            .map(|(n, s)| s * (n.min(8) as f64 / 4.0))
            .collect();
        close(&collect(&gained), &expected);
        let decoded = Plan::from_json(&serde_json::to_vec(&gained).unwrap()).unwrap();
        let mut engine = DspEngine::new(&decoded).unwrap();
        for _ in 0..2 {
            let mut samples = vec![];
            engine
                .render(|frame| {
                    samples.push(frame[0]);
                    Ok(())
                })
                .unwrap();
            assert_eq!(samples, collect(&gained));
        }
    }
}
#[test]
fn fractional_offsets_and_overlapping_voices_use_independent_gain_clocks() {
    let mut first = fixture(None);
    let note = &mut first.events[0];
    note.onset_offset_seconds = r(1, 96000);
    note.release_offset_seconds = r(-1, 96000);
    note.on_seconds = r(1, 96000);
    note.off_seconds = Some(r(15, 96000));
    note.on_frame = 1;
    let mut second = fixture(None);
    second.events[0] = event("a", "sine", 2, 6, 1500.0, r(1, 1));
    let expected: Vec<_> = collect(&first)
        .iter()
        .enumerate()
        .zip(collect(&second))
        .map(|((n, a), b)| {
            a * (n.saturating_sub(1).min(7) as f64 / 7.0)
                + b * (0.25 + 0.75 * n.saturating_sub(2).min(4) as f64 / 4.0)
        })
        .collect();
    set_gain(
        &mut first.events[0],
        expression(
            ExpressionClock::Score,
            r(8, 24000),
            r(0, 1),
            r(1, 1),
            Interpolation::Linear,
        ),
    );
    set_gain(
        &mut second.events[0],
        expression(
            ExpressionClock::Seconds,
            r(4, 48000),
            r(1, 4),
            r(1, 1),
            Interpolation::Linear,
        ),
    );
    first.events.push(second.events.remove(0));
    close(&collect(&first), &expected);
}
#[test]
fn zero_gain_consumes_capacity_through_release_and_reclaims_at_envelope_zero() {
    for (on, overflow) in [(2, true), (9, true), (12, false)] {
        let mut plan = fixture(Some(expression(
            ExpressionClock::Normalized,
            r(1, 1),
            r(0, 1),
            r(0, 1),
            Interpolation::Step,
        )));
        plan.nodes[0].processor = Processor::sine(1);
        plan.output.score_end_q = r(16, 24000);
        plan.output.total_frames = 20;
        plan.events
            .push(event("other", "sine", on, 16, 3000.0, r(1, 1)));
        let result = render(&plan, |_| Ok(()));
        if overflow {
            assert!(matches!(
                result,
                Err(maac::dsp::RenderError::VoiceLimit { .. })
            ));
        } else {
            result.unwrap();
        }
    }
}
#[test]
fn gain_above_one_is_not_clamped_and_nonfinite_product_fails() {
    let plain = collect(&fixture(None));
    let gained = fixture(Some(expression(
        ExpressionClock::Normalized,
        r(1, 1),
        r(4, 1),
        r(4, 1),
        Interpolation::Step,
    )));
    assert_eq!(
        collect(&gained),
        plain.iter().map(|s| s * 4.0).collect::<Vec<_>>()
    );
    let mut sink = std::io::Cursor::new(Vec::<u8>::new());
    assert!(matches!(
        maac::export::write_wav(&mut sink, &gained, maac::export::WavFormat::Pcm16),
        Err(maac::export::ExportError::Pcm16Overload { .. })
    ));
    let mut huge = fixture(Some(expression(
        ExpressionClock::Normalized,
        r(1, 1),
        Rational::from_integer(num_bigint::BigInt::from(10).pow(308)),
        Rational::from_integer(num_bigint::BigInt::from(10).pow(308)),
        Interpolation::Step,
    )));
    huge.nodes[0].params.insert("level".into(), r(4, 1));
    assert!(matches!(
        render(&huge, |_| Ok(())),
        Err(maac::dsp::RenderError::Nonfinite(_))
    ));
}
#[test]
fn extreme_exponential_ratio_renders_finite_interior_gain() {
    let curve = expression(
        ExpressionClock::Normalized,
        r(1, 1),
        Rational::new(1.into(), num_bigint::BigInt::from(10).pow(400)),
        Rational::from_integer(num_bigint::BigInt::from(10).pow(300)),
        Interpolation::Exponential,
    );
    let plan = fixture(Some(curve));
    let plain = collect(&fixture(None));
    let samples = collect(&plan);
    for n in 1..8 {
        let gain = 10.0_f64.powf(-400.0 + 700.0 * n as f64 / 8.0);
        let expected = plain[n] * gain;
        assert!(samples[n].is_finite());
        assert!((samples[n] / expected - 1.0).abs() < 1e-12, "frame {n}");
    }
}
#[test]
fn attack_release_and_velocity_multiply_in_specified_order() {
    let mut plan = fixture(Some(expression(
        ExpressionClock::Normalized,
        r(1, 1),
        r(1, 3),
        r(2, 3),
        Interpolation::Linear,
    )));
    plan.nodes[0].params.insert("attack".into(), r(12, 48000));
    plan.nodes[0].params.insert("level".into(), r(3, 7));
    if let EventKind::Note { velocity, .. } = &mut plan.events[0].kind {
        *velocity = r(2, 5);
    }
    let mut phase: f64 = 0.0;
    let expected: Vec<_> = (0..12)
        .map(|n| {
            let envelope = if n < 8 {
                n as f64 / 12.0
            } else {
                (8.0 / 12.0) * (1.0 - (n - 8) as f64 / 4.0)
            };
            let gain = (8 + n.min(8)) as f64 / 24.0;
            let sample = 0.4 * gain * envelope * (3.0 / 7.0) * phase.sin();
            phase = (phase + 2.0 * std::f64::consts::PI * 3000.0 / 48000.0)
                .rem_euclid(2.0 * std::f64::consts::PI);
            sample
        })
        .collect();
    assert_eq!(collect(&plan), expected);
}
