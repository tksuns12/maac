use maac::{
    compiler::compile,
    dsp::DspEngine,
    parse,
    plan::{EventKind, Plan, PlanLimits},
};

fn plan(pitch: &str, gain: bool, ratio: &str, frequency: &str) -> Plan {
    let gain = if gain {
        "expression g { kind = gain; curve = &gain; }"
    } else {
        ""
    };
    let expression = if pitch.is_empty() {
        String::new()
    } else {
        "expression p { kind = pitch; curve = &pitch; }".into()
    };
    let points = if pitch.is_empty() {
        "[(0, 0ct, step), (1, 0ct, step)]"
    } else {
        pitch
    };
    compile(&parse(&format!(r#"maac 1;
project p {{ score = [0q, 1/1000q]; tail = 1/12000s; rate = 48000Hz; tempo = &clock; meter = &meter; output = &lead:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }} meter meter {{ points = [(0q, 4, 4)]; }}
instrument instrument {{ channels = 1; voice v {{ channels = 1; amplitude = &amp; output = &osc:out;
node amp {{ type = "synth.adsr/1"; params = {{ release = 1/12000s; }}; }}
node osc {{ type = "synth.sine/1"; params = {{ ratio = {ratio}; frequency = {frequency}Hz; phase = 1/4; }}; }} }} }}
node lead {{ instrument = &instrument; config = {{ voices = 2; }}; }} track t {{ target = &lead:events; }}
curve pitch {{ clock = normalized; points = {points}; }} curve gain {{ clock = normalized; points = [(0, 0, linear), (1, 2, step)]; }}
pattern notes {{ length = 1/1000q; note a {{ at = 0q; dur = 1/1000q; pitch = 480Hz; velocity = 1/2; {expression} {gain} }} }}
place placement {{ pattern = &notes; track = &t; at = 0q; }}"#)).unwrap()).unwrap()
}
fn collect(plan: &Plan) -> Vec<f64> {
    let restored = Plan::from_json(&plan.to_json().unwrap()).unwrap();
    let mut engine = DspEngine::new(&restored).unwrap();
    let mut first = vec![];
    engine
        .render(|frame| {
            first.push(frame[0]);
            Ok(())
        })
        .unwrap();
    let mut again = vec![];
    engine
        .render(|frame| {
            again.push(frame[0]);
            Ok(())
        })
        .unwrap();
    assert_eq!(first, again);
    first
}
// Independent phase recurrence with the documented linear-interpolated sine table.
fn sine(phase: f64) -> f64 {
    let position = phase * 2048.0;
    let left = position.floor();
    let a = (std::f64::consts::TAU * left / 2048.0).sin();
    let b = (std::f64::consts::TAU * ((left + 1.0) % 2048.0) / 2048.0).sin();
    a + (b - a) * (position - left)
}
#[test]
fn initial_pitch_continuous_phase_tuning_and_release_hold_match_analytic_output() {
    for gain in [false, true] {
        for shape in ["linear", "step"] {
            let p = plan(
                &format!("[(0, 1200ct, {shape}), (1, -1200ct, step)]"),
                gain,
                "2",
                "-1000",
            );
            let mut phase = 0.25;
            for (frame, sample) in collect(&p).into_iter().enumerate() {
                let coordinate = frame.min(24) as f64 / 24.0;
                let cents = if shape == "linear" {
                    1200.0 - 2400.0 * coordinate
                } else if frame >= 24 {
                    -1200.0
                } else {
                    1200.0
                };
                let envelope = if frame < 24 {
                    1.0
                } else {
                    1.0 - (frame - 24) as f64 / 4.0
                };
                let expected =
                    sine(phase) * envelope * 0.5 * if gain { 2.0 * coordinate } else { 1.0 };
                assert!(
                    (sample - expected).abs() < 1e-13,
                    "frame {frame}: {sample} != {expected}"
                );
                let frequency = 480.0 * 2.0_f64.powf(cents / 1200.0) * 2.0 - 1000.0;
                phase = (phase + frequency / 48000.0).rem_euclid(1.0);
            }
        }
    }
}
#[test]
fn zero_pitch_preserves_pcm_and_overlapping_bends_are_independent() {
    assert_eq!(
        collect(&plan("", false, "1", "0")),
        collect(&plan("[(0, 0ct, step), (1, 0ct, step)]", false, "1", "0"))
    );
    let a = plan("[(0, 0ct, linear), (1, 1200ct, step)]", true, "1", "0");
    let b = plan("[(0, -1200ct, linear), (1, 0ct, step)]", false, "1", "0");
    let mut combined = a.clone();
    let mut event = b.events[0].clone();
    event.address = "placement/0/z".into();
    event.order = 1;
    combined.events.push(event);
    for ((actual, a), b) in collect(&combined).iter().zip(collect(&a)).zip(collect(&b)) {
        assert_eq!(*actual, a + b);
    }
}
fn minimum_budget(plan: &Plan) -> u64 {
    let mut low = 0;
    let mut high = 100_000;
    while low < high {
        let mid = (low + high) / 2;
        if plan
            .validate_with_limits(&PlanLimits {
                max_execution_work: mid,
                ..Default::default()
            })
            .is_ok()
        {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    low
}
#[test]
fn expression_work_is_additive_and_includes_release() {
    let plain = plan("", false, "1", "0");
    let pitch = plan("[(0, 0ct, step), (1, 1200ct, step)]", false, "1", "0");
    let both = plan("[(0, 0ct, step), (1, 1200ct, step)]", true, "1", "0");
    let base = minimum_budget(&plain);
    assert_eq!(minimum_budget(&pitch), base + 28 * 18);
    assert_eq!(minimum_budget(&both), base + 28 * 36);
}
#[test]
fn compensating_ratio_does_not_relax_base_nyquist_and_silence_does_not_hide_node_errors() {
    let mut base = plan("[(0, 0ct, step), (1, 0ct, step)]", false, "1/2", "0");
    if let EventKind::Note { pitch_hz, .. } = &mut base.events[0].kind {
        *pitch_hz = 24000.0;
    }
    assert_eq!(base.validate().unwrap_err().code, "E_RANGE");
    for velocity_zero in [false, true] {
        let mut invalid = plan("[(0, 1200ct, step), (1, 1200ct, step)]", true, "32", "0");
        if velocity_zero {
            if let EventKind::Note { velocity, .. } = &mut invalid.events[0].kind {
                *velocity = maac::parse_rational("0").unwrap();
            }
        }
        let mut engine = DspEngine::new(&invalid).unwrap();
        let error = engine.render(|_| Ok(())).unwrap_err().to_string();
        assert!(error.contains("frequency"), "{error}");
    }
}

#[test]
fn expressed_base_domain_and_effective_frequency_endpoints_remain_distinct() {
    let mut expressed = plan("[(0, 1200ct, step), (1, 1200ct, step)]", false, "1/2", "0");
    if let EventKind::Note { pitch_hz, .. } = &mut expressed.events[0].kind {
        *pitch_hz = 12000.0;
    }
    assert_eq!(expressed.validate().unwrap_err().code, "E_RANGE");
    let mut future = plan("[(0, 0ct, step), (1, 0ct, step)]", false, "1", "0");
    if let EventKind::Note {
        pitch_expression: Some(expression),
        ..
    } = &mut future.events[0].kind
    {
        expression.clock = maac::plan::ExpressionClock::Seconds;
        expression.points[1].position = maac::parse_rational("1").unwrap();
        expression.points[1].cents = maac::parse_rational("100000").unwrap();
    }
    collect(&future);
    for (ratio, offset) in [("-1", "-23520"), ("1", "-480"), ("1", "23520")] {
        collect(&plan(
            "[(0, 0ct, step), (1, 0ct, step)]",
            false,
            ratio,
            offset,
        ));
    }
    let outside = plan(
        "[(0, 0ct, step), (1, 0ct, step)]",
        false,
        "1",
        "23520001/1000",
    );
    assert!(DspEngine::new(&outside)
        .unwrap()
        .render(|_| Ok(()))
        .is_err());
}
