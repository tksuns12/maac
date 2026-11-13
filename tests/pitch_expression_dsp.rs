use maac::dsp::{render, DspEngine};
use maac::plan::{EventKind, Plan};

#[path = "support/frames.rs"]
mod frames;
use frames::FrameProject;

/// The sine of the fixture: at full level at once, and a release of four
/// frames.
const RELEASED: &str = "attack = 0s; release = 4/48000s; level = 1;";

/// The field that gives a note the pitch curve `id`.
fn bent(id: &str) -> String {
    format!("expression bend {{ kind = pitch; curve = &{id}; }}")
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

/// A pitch curve `id` on `clock` from `start` cents at its origin to
/// `finish` cents at `end`. `end` is written with the clock's unit: `1`,
/// `8/48000s`, `8/24000q`.
fn curve(id: &str, clock: &str, end: &str, start: i64, finish: i64, shape: &str) -> String {
    let origin = match clock {
        "normalized" => "0",
        "seconds" => "0s",
        "score" => "0q",
        _ => unreachable!("an expression clock"),
    };
    format!(
        "curve {id} {{ clock = {clock}; points = [({origin}, {start}ct, {shape}), ({end}, {finish}ct, step)]; }}"
    )
}

/// Note `n`: eight frames at 3000 Hz and four frames of tail, with the pitch
/// curve `bend` if one is given.
fn fixture(bend: Option<String>) -> FrameProject {
    let project = FrameProject::sine(8, 4, 4, RELEASED);
    match bend {
        Some(curve) => project
            .note("n", 0, 8, "3000Hz", &bent("bend"))
            .with(&curve),
        None => project.note("n", 0, 8, "3000Hz", ""),
    }
}
#[test]
fn constant_octave_matches_double_frequency_exactly() {
    let bent = fixture(Some(curve("bend", "normalized", "1", 1200, 1200, "step"))).plan();
    let plain = FrameProject::sine(8, 4, 4, RELEASED)
        .note("n", 0, 8, "6000Hz", "")
        .plan();
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
        ("normalized", "1"),
        ("seconds", "8/48000s"),
        ("score", "8/24000q"),
    ] {
        for shape in ["step", "linear"] {
            let plan = fixture(Some(curve("bend", clock, end, 0, 1200, shape))).plan();
            close(
                &collect(&plan),
                &expected(0, 8, 12, 3000.0, |n| {
                    if shape == "linear" {
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
        ("4/48000s", 4),
        ("400000000000000000001/4800000000000000000000000s", 5),
        ("399999999999999999999/4800000000000000000000000s", 4),
    ] {
        let plan = fixture(Some(curve("bend", "seconds", position, 0, 1200, "step"))).plan();
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
    // Note `n` starts half a frame late and ends half a frame early, so its
    // gate is frames 1 to 8; note `a` overlaps it.
    let plan = FrameProject::sine(8, 4, 4, RELEASED)
        .note(
            "n",
            0,
            8,
            "3000Hz",
            &format!(
                "onset_offset = 1/96000s; release_offset = -1/96000s; {}",
                bent("rise")
            ),
        )
        .note("a", 2, 6, "1500Hz", &bent("down"))
        .with(&curve("rise", "normalized", "1", 0, 1200, "linear"))
        .with(&curve("down", "normalized", "1", -1200, -1200, "step"))
        .plan();
    let a = expected(1, 8, 12, 3000.0, |n| n as f64 / 7.0 * 1200.0);
    let b = expected(2, 6, 12, 1500.0, |_| -1200.0);
    close(
        &collect(&plan),
        &a.iter().zip(b).map(|(a, b)| a + b).collect::<Vec<_>>(),
    );
}
#[test]
fn score_clock_is_affine_across_tempo_change() {
    // Six score frames: two at 120 bpm, then four at 60 bpm, which last
    // eight frames. The gate is ten frames.
    let mut project = FrameProject::sine(6, 4, 4, RELEASED)
        .note("n", 0, 6, "3000Hz", &bent("bend"))
        .with(&curve("bend", "score", "6/24000q", 0, 1200, "linear"));
    project.tempo = "(0q, 120bpm, step), (2/24000q, 60bpm, step)".to_owned();
    close(
        &collect(&project.plan()),
        &expected(0, 10, 14, 3000.0, |n| n as f64 * 120.0),
    );
}
#[test]
fn source_free_roundtrip_and_engine_reset_match_and_legacy_pcm_is_exact() {
    for expressed in [false, true] {
        let plan =
            fixture(expressed.then(|| curve("bend", "normalized", "1", 0, 1200, "linear"))).plan();
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
        // The compiler refuses a curve that bends this far, so the plan is a
        // compiled one whose cents are changed afterwards.
        let mut plan = fixture(Some(curve("bend", "normalized", "1", 0, 0, "step"))).plan();
        let EventKind::Note {
            pitch_expression: Some(expression),
            ..
        } = &mut plan.events[0].kind
        else {
            panic!("the note has a pitch expression");
        };
        for point in &mut expression.points {
            point.cents = maac::parse_rational(&cents.to_string()).unwrap();
        }
        let mut called = false;
        assert!(render(&plan, |_| {
            called = true;
            Ok(())
        })
        .is_err());
        assert!(!called);
    }
}
