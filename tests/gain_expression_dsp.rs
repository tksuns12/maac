use maac::dsp::{render, DspEngine};
use maac::plan::Plan;

#[path = "support/sine.rs"]
mod sine;
use sine::SineProject;

/// The sine of the fixture: at full level at once, and a release of four
/// frames.
const RELEASED: &str = "attack = 0s; release = 4/48000s; level = 1;";

fn collect(plan: &Plan) -> Vec<f64> {
    let mut out = vec![];
    render(plan, |frame| {
        out.push(frame[0]);
        Ok(())
    })
    .unwrap();
    out
}

/// A curve `id` on `clock` from `start` at its origin to `finish` at `end`.
/// `end` is written with the clock's unit: `1`, `8/48000s`, `8/24000q`.
fn curve(id: &str, clock: &str, end: &str, start: &str, finish: &str, shape: &str) -> String {
    let origin = match clock {
        "normalized" => "0",
        "seconds" => "0s",
        "score" => "0q",
        _ => unreachable!("an expression clock"),
    };
    format!(
        "curve {id} {{ clock = {clock}; points = [({origin}, {start}, {shape}), ({end}, {finish}, step)]; }}"
    )
}

/// The field that gives a note the gain curve `id`.
fn gained(id: &str) -> String {
    format!("expression gain {{ kind = gain; curve = &{id}; }}")
}

/// Note `n`: eight frames at 3000 Hz with `fields`, and four frames of tail.
fn scene(params: &str, fields: &str, curves: &str) -> Plan {
    SineProject::new(8, 4, 4, params)
        .note("n", 0, 8, "3000Hz", fields)
        .with(curves)
        .plan()
}

/// The scene with the released sine, and with the gain curve `gain` if one
/// is given.
fn fixture(gain: Option<String>) -> Plan {
    match gain {
        Some(curve) => scene(RELEASED, &gained("gain"), &curve),
        None => scene(RELEASED, "", ""),
    }
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
        ("normalized", "1"),
        ("seconds", "8/48000s"),
        ("score", "8/24000q"),
    ] {
        for shape in ["step", "linear", "exponential"] {
            let plan = fixture(Some(curve("gain", clock, end, "1/4", "1", shape)));
            let expected: Vec<_> = plain
                .iter()
                .enumerate()
                .map(|(n, sample)| {
                    let t = n.min(8) as f64 / 8.0;
                    let gain = match shape {
                        "step" => {
                            if n < 8 {
                                0.25
                            } else {
                                1.0
                            }
                        }
                        "linear" => 0.25 + 0.75 * t,
                        "exponential" => 0.25 * 4.0_f64.powf(t),
                        _ => unreachable!(),
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
        ("4/48000s", 4),
        ("400000000000000000001/4800000000000000000000000s", 5),
        ("399999999999999999999/4800000000000000000000000s", 4),
    ] {
        let plan = fixture(Some(curve("gain", "seconds", position, "0", "1", "step")));
        let mut expected = plain.clone();
        expected[..first].fill(0.0);
        assert_eq!(collect(&plan), expected);
    }
}
#[test]
fn gain_one_preserves_legacy_and_pitch_pcm_exactly_and_roundtrip_resets() {
    for pitched in [false, true] {
        let (bend, bend_curve) = if pitched {
            (
                "expression bend { kind = pitch; curve = &bend; }",
                "curve bend { clock = seconds; points = [(0s, 0ct, linear), (8/48000s, 1200ct, step)]; }",
            )
        } else {
            ("", "")
        };
        let with_gain = |gain: String| {
            scene(
                RELEASED,
                &format!("{bend} {}", gained("gain")),
                &format!("{bend_curve}\n{gain}"),
            )
        };
        let plain = scene(RELEASED, bend, bend_curve);
        let gained = with_gain(curve("gain", "score", "8/24000q", "1", "1", "step"));
        assert_eq!(collect(&gained), collect(&plain));
        let gained = with_gain(curve("gain", "normalized", "1", "0", "2", "linear"));
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
    // Note `n` starts half a frame late and ends half a frame early, so its
    // gate is frames 1 to 8; note `a` overlaps it.
    const OFFSETS: &str = "onset_offset = 1/96000s; release_offset = -1/96000s;";
    let project = || SineProject::new(8, 4, 4, RELEASED);
    let first = project().note("n", 0, 8, "3000Hz", OFFSETS).plan();
    let second = project().note("a", 2, 6, "1500Hz", "").plan();
    let expected: Vec<_> = collect(&first)
        .iter()
        .enumerate()
        .zip(collect(&second))
        .map(|((n, a), b)| {
            a * (n.saturating_sub(1).min(7) as f64 / 7.0)
                + b * (0.25 + 0.75 * n.saturating_sub(2).min(4) as f64 / 4.0)
        })
        .collect();
    let both = project()
        .note(
            "n",
            0,
            8,
            "3000Hz",
            &format!("{OFFSETS} {}", gained("rise")),
        )
        .note("a", 2, 6, "1500Hz", &gained("swell"))
        .with(&curve("rise", "score", "8/24000q", "0", "1", "linear"))
        .with(&curve("swell", "seconds", "4/48000s", "1/4", "1", "linear"))
        .plan();
    close(&collect(&both), &expected);
}
#[test]
fn zero_gain_consumes_capacity_through_release_and_reclaims_at_envelope_zero() {
    for (on, overflow) in [(2, true), (9, true), (12, false)] {
        let plan = SineProject::new(16, 4, 1, RELEASED)
            .note("n", 0, 8, "3000Hz", &gained("gain"))
            .note("other", on, 16, "3000Hz", "")
            .with(&curve("gain", "normalized", "1", "0", "0", "step"))
            .plan();
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
    let gained_plan = fixture(Some(curve("gain", "normalized", "1", "4", "4", "step")));
    assert_eq!(
        collect(&gained_plan),
        plain.iter().map(|s| s * 4.0).collect::<Vec<_>>()
    );
    let mut sink = std::io::Cursor::new(Vec::<u8>::new());
    assert!(matches!(
        maac::export::write_wav(&mut sink, &gained_plan, maac::export::WavFormat::Pcm16),
        Err(maac::export::ExportError::Pcm16Overload { .. })
    ));
    // A gain of 10^308 on a level of 4 is past what binary64 holds.
    let huge_gain = format!("1{}", "0".repeat(308));
    let huge = scene(
        "attack = 0s; release = 4/48000s; level = 4;",
        &gained("gain"),
        &curve("gain", "normalized", "1", &huge_gain, &huge_gain, "step"),
    );
    assert!(matches!(
        render(&huge, |_| Ok(())),
        Err(maac::dsp::RenderError::Nonfinite(_))
    ));
}
#[test]
fn extreme_exponential_ratio_renders_finite_interior_gain() {
    // From 10^-400 to 10^300.
    let plan = fixture(Some(curve(
        "gain",
        "normalized",
        "1",
        &format!("1/1{}", "0".repeat(400)),
        &format!("1{}", "0".repeat(300)),
        "exponential",
    )));
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
    let plan = scene(
        "attack = 12/48000s; release = 4/48000s; level = 3/7;",
        &format!("velocity = 2/5; {}", gained("gain")),
        &curve("gain", "normalized", "1", "1/3", "2/3", "linear"),
    );
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
