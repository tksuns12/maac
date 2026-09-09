use maac::{
    compiler::{compile, compile_versioned},
    parse,
    plan::PlanLimits,
    plan_v3::VersionedPlan,
};

fn source() -> String {
    r#"maac 1;
project p { score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &s:out; }
tempo clock { points = [(0q, 60bpm, linear), (4q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node s { type = "core.sine/1"; config = { voices = 8; }; params = { attack = 0s; release = 1/10s; level = 1; }; }
track t { target = &s:events; }
pattern pat { length = 4q; note n { at = 0q; dur = 4q; pitch = 440Hz; velocity = 1; } }
place x { pattern = &pat; track = &t; at = 0q; }"#.into()
}
fn audio(plan: &VersionedPlan) -> Vec<f64> {
    let mut out = Vec::new();
    maac::dsp::render_versioned(plan, |frame| {
        out.push(frame[0]);
        Ok(())
    })
    .unwrap();
    out
}

fn compiled(text: &str) -> VersionedPlan {
    compile_versioned(&parse(text).unwrap()).unwrap()
}

#[test]
fn ramp_roundtrip_and_reset() {
    let plan = compiled(&source());
    let samples = audio(&plan);
    assert_eq!(samples.len(), 133085);
    assert_eq!(
        samples,
        audio(&VersionedPlan::from_json(&plan.to_json().unwrap()).unwrap())
    );
    let mut engine = maac::dsp::DspEngine::new_versioned(&plan).unwrap();
    for _ in 0..2 {
        let mut out = Vec::new();
        engine
            .render(|f| {
                out.push(f[0]);
                Ok(())
            })
            .unwrap();
        assert_eq!(samples, out);
    }
}

#[test]
fn score_automation_uses_the_inverse_ramp_at_each_sample() {
    let reference = audio(&compiled(&source()));
    let automated = audio(&compiled(
        &(source()
            + r#"
curve level { clock = score; points = [(0q, 0, linear), (4q, 1, step)]; }
automation level_lane { target = &s.params.level; curve = &level; at = 0q; }"#),
    ));

    // B(q)=60+15q gives T(q)=4*ln(1+q/4), hence
    // q(t)/4=expm1(t/4). Choose a nonzero oscillator sample so the automated
    // level can be observed directly through the public renderer.
    let frame = 24_001usize;
    assert!(reference[frame].abs() > 0.01);
    let actual_level = automated[frame] / reference[frame];
    let expected_level = ((frame as f64 / 48_000.0) / 4.0).exp_m1();
    assert!((actual_level - expected_level).abs() < 1e-12);
}

#[test]
fn an_interior_score_knot_activates_at_its_certified_ramp_time() {
    let reference = audio(&compiled(&source()));
    let automated = audio(&compiled(
        &(source()
            + r#"
curve level { clock = score; points = [(0q, 1/4, linear), (1q, 1/2, linear), (4q, 1/4, step)]; }
automation level_lane { target = &s.params.level; curve = &level; at = 0q; }"#),
    ));
    // This frame is after T(1q) but before one physical second. The outgoing
    // segment from the interior knot must already be active.
    let frame = 45_001usize;
    assert!(reference[frame].abs() > 0.01);
    let q = 4.0 * ((frame as f64 / 48_000.0) / 4.0).exp_m1();
    assert!(q > 1.0);
    let expected_level = 0.5 - (q - 1.0) / 12.0;
    let actual_level = automated[frame] / reference[frame];
    assert!((actual_level - expected_level).abs() < 1e-12);
}

#[test]
fn inverse_ramp_is_stable_for_deceleration_and_a_near_flat_slope() {
    for (tempo, expected_level) in [
        (
            "(0q, 120bpm, linear), (4q, 60bpm, step)",
            -2.0 * (-(24_001.0_f64 / 48_000.0) / 4.0).exp_m1(),
        ),
        (
            "(0q, 60bpm, linear), (4q, 60000000000001/1000000000000bpm, step)",
            {
                let elapsed = 24_001.0_f64 / 48_000.0;
                let slope = 2.5e-13_f64;
                let x = slope * elapsed / 60.0;
                (elapsed * x.exp_m1() / x) / 4.0
            },
        ),
    ] {
        let text = source().replace("(0q, 60bpm, linear), (4q, 120bpm, step)", tempo);
        let reference = audio(&compiled(&text));
        let automated = audio(&compiled(
            &(text
                + r#"
curve level { clock = score; points = [(0q, 0, linear), (4q, 1, step)]; }
automation level_lane { target = &s.params.level; curve = &level; at = 0q; }"#),
        ));
        let frame = 24_001usize;
        assert!(reference[frame].abs() > 0.01);
        let actual_level = automated[frame] / reference[frame];
        assert!(
            (actual_level - expected_level).abs() < 1e-12,
            "expected {expected_level}, found {actual_level} for {tempo}"
        );
    }
}

#[test]
fn a_score_anchored_seconds_lane_adds_physical_offsets_after_tempo_mapping() {
    let reference = audio(&compiled(&source()));
    let automated = audio(&compiled(
        &(source()
            + r#"
curve level { clock = seconds; points = [(0s, 0, step), (1/100s, 1, step)]; }
automation level_lane { target = &s.params.level; curve = &level; at = 1/2q; }"#),
    ));
    let knot = (48_000.0 * (4.0 * 1.125_f64.ln() + 0.01)).ceil() as usize;
    assert!(reference[knot - 1].abs() > 0.01);
    assert_eq!(automated[knot - 1].to_bits(), 0.0_f64.to_bits());
    assert_eq!(automated[knot].to_bits(), reference[knot].to_bits());
}

#[test]
fn automation_knots_before_the_reset_origin_remain_signed() {
    let reference = audio(&compiled(&source()));
    let automated = audio(&compiled(
        &(source()
            + r#"
curve level { clock = score; points = [(0q, 0, step), (1q, 1, step)]; }
automation level_lane { target = &s.params.level; curve = &level; at = -2q; }"#),
    ));
    assert_eq!(automated, reference);
}

#[test]
fn continuous_automation_cancels_a_huge_anchor_before_float_conversion() {
    let reference = audio(&compiled(&source()));
    let automated = audio(&compiled(
        &(source()
            + r#"
curve level { clock = score; points = [(0q, 0, step), (9007199254740993q, 0, linear), (9007199254740994q, 1, step)]; }
automation level_lane { target = &s.params.level; curve = &level; at = -9007199254740993q; }"#),
    ));
    let frame = 24_001usize;
    assert!(reference[frame].abs() > 0.01);
    let actual_level = automated[frame] / reference[frame];
    let expected_level = 4.0 * ((frame as f64 / 48_000.0) / 4.0).exp_m1();
    assert!((actual_level - expected_level).abs() < 1e-12);
}

fn shifted_source(origin: &str, end: &str, tempo_left: &str, tempo_right: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [{origin}q, {end}q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &s:out; }}
tempo clock {{ points = [({tempo_left}q, 60bpm, linear), ({tempo_right}q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node s {{ type = "core.sine/1"; config = {{ voices = 8; }}; params = {{ attack = 0s; release = 1/10s; level = 1; }}; }}
track t {{ target = &s:events; }}
pattern pat {{ length = 1q; note n {{ at = 0q; dur = 1q; pitch = 440Hz; velocity = 1; }} }}
place x {{ pattern = &pat; track = &t; at = {origin}q; }}
curve level {{ clock = score; points = [(0q, 0, linear), (1q, 1, step)]; }}
automation level_lane {{ target = &s.params.level; curve = &level; at = {origin}q; }}"#
    )
}

#[test]
fn an_origin_inside_a_ramp_keeps_local_precision_at_huge_score_positions() {
    let local = compiled(&shifted_source("1", "2", "0", "4"));
    let huge = compiled(&shifted_source(
        "9007199254740993",
        "9007199254740994",
        "9007199254740992",
        "9007199254740996",
    ));
    assert_eq!(audio(&huge), audio(&local));
}

#[test]
fn seconds_automation_freezes_at_a_nonrational_ramp_end_through_the_tail() {
    let base = source()
        .replace("score = [0q, 4q]", "score = [0q, 1q]")
        .replace("output = &s:out; }", "output = &s:out; tail = 1/100s; }")
        .replace("length = 4q", "length = 1q")
        .replace("dur = 4q", "dur = 1q");
    let reference_plan = compiled(&base);
    let automated_plan = compiled(
        &(base.clone()
            + r#"
curve level { clock = seconds; points = [(0s, 0, linear), (1s, 1, step)]; }
automation level_lane { target = &s.params.level; curve = &level; at = 0q; }"#),
    );
    let score_end_frame = match &reference_plan {
        VersionedPlan::V3(plan) => plan.events[0].off_frame.unwrap() as usize,
        VersionedPlan::Legacy(_) => panic!("ramp source must compile as version 3"),
    };
    let reference = audio(&reference_plan);
    let automated = audio(&automated_plan);
    let tail_frame = (score_end_frame..reference.len())
        .find(|frame| reference[*frame].abs() > 0.01)
        .expect("release tail must contain an observable sample");
    let expected_level = 4.0 * 1.25_f64.ln();
    assert!((automated[tail_frame] / reference[tail_frame] - expected_level).abs() < 1e-12);
}

#[test]
fn versioned_render_preserves_legacy_step_plan_samples_bit_for_bit() {
    let text = source().replace("60bpm, linear", "60bpm, step");
    let document = parse(&text).unwrap();
    let legacy = compile(&document).unwrap();
    let versioned = compile_versioned(&document).unwrap();
    assert_eq!(versioned, VersionedPlan::Legacy(legacy.clone()));

    let mut direct = Vec::new();
    maac::dsp::render(&legacy, |frame| {
        direct.extend(frame.iter().map(|sample| sample.to_bits()));
        Ok(())
    })
    .unwrap();
    let through_versioned = audio(&versioned)
        .into_iter()
        .map(f64::to_bits)
        .collect::<Vec<_>>();
    assert_eq!(through_versioned, direct);
}

#[test]
fn renderer_preparation_shares_one_numerical_budget_with_validation() {
    let text = r#"maac 1;
project p { score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &s:out; }
tempo clock { points = [(0q, 120bpm, linear), (4q, 240bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node s { type = "core.sine/1"; config = { voices = 1; }; }"#;
    let plan = compiled(text);
    let limits = PlanLimits {
        max_work: 256,
        ..PlanLimits::default()
    };
    match &plan {
        VersionedPlan::V3(plan) => plan.validate_with_limits(&limits).unwrap(),
        VersionedPlan::Legacy(_) => panic!("ramp source must compile as version 3"),
    }
    let error = match maac::dsp::DspEngine::new_versioned_with_limits(&plan, &limits) {
        Ok(_) => panic!("renderer preparation reset the numerical budget"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "E_RESOURCE_LIMIT");
    let sufficient = PlanLimits {
        max_work: 384,
        ..PlanLimits::default()
    };
    assert!(maac::dsp::DspEngine::new_versioned_with_limits(&plan, &sufficient).is_ok());
}

#[test]
fn dsp_engine_preserves_its_send_and_sync_auto_traits() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<maac::dsp::DspEngine<'static>>();
}
