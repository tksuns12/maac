use maac::dsp::{one_pole_step, pan_sample, render, sum_samples, RenderError};
use maac::plan::Plan;

#[path = "support/frames.rs"]
mod frames;
use frames::FrameProject;

/// A sine that is at full level the moment a note starts and silent the
/// moment it ends.
const GATED: &str = "attack = 0s; release = 0s; level = 1;";

/// An automation `id` of `target`, at `at`, along a curve of `points` on
/// `clock`.
fn automation(id: &str, target: &str, clock: &str, at: &str, points: &str) -> String {
    format!(
        "curve {id}_curve {{ clock = {clock}; points = [{points}]; }}\n\
         automation {id} {{ target = &{target}; curve = &{id}_curve; at = {at}; }}"
    )
}

fn collect(plan: &Plan) -> maac::dsp::Result<Vec<Vec<f64>>> {
    let mut frames = Vec::new();
    render(plan, |frame| {
        frames.push(frame.to_vec());
        Ok(())
    })?;
    Ok(frames)
}

/// Three matrices of one sine, summed: a large value, a small one and the
/// large one negated, so the sum depends on the order it adds them in. The
/// plan lists its fan-in connections in `connection_order` and, when asked,
/// its nodes in reverse.
fn cancellation_plan(
    channels: u8,
    connection_order: [usize; 3],
    fan_in_ids: [&str; 3],
    reverse_node_declarations: bool,
) -> Plan {
    const LARGE: i64 = 18_014_398_509_481_984;
    const PRODUCER_IDS: [&str; 3] = ["a_large", "m_small", "z_negative"];

    let coefficients = match channels {
        1 => [
            format!("[[{LARGE}]]"),
            "[[1]]".to_owned(),
            format!("[[-{LARGE}]]"),
        ],
        2 => [
            format!("[[{LARGE}], [-{LARGE}]]"),
            "[[1], [-1]]".to_owned(),
            format!("[[-{LARGE}], [{LARGE}]]"),
        ],
        _ => unreachable!("the fixture only covers mono and stereo"),
    };
    let mut project = FrameProject::sine(2, 0, 1, GATED)
        .note("n", 0, 2, "12000Hz", "")
        .with(&format!(
            "node sum {{ type = \"core.sum/1\"; config = {{ channels = {channels}; }}; }}"
        ));
    project.output = "sum".to_owned();
    for ((producer_id, coefficients), fan_in_id) in
        PRODUCER_IDS.iter().zip(coefficients).zip(fan_in_ids)
    {
        project = project.with(&format!(
            "node {producer_id} {{ type = \"core.matrix/1\"; config = {{ inputs = 1; outputs = {channels}; coefficients = {coefficients}; }}; }}\n\
             connect source_to_{producer_id} {{ from = &sine:out; to = &{producer_id}:in; }}\n\
             connect {fan_in_id} {{ from = &{producer_id}:out; to = &sum:in; }}"
        ));
    }
    let mut plan = project.plan();

    // The compiler lists nodes and connections by ID. A plan may list them
    // in any order, so put them in the order this case asks for.
    let position = |ids: &[String], id: &str| ids.iter().position(|other| other == id).unwrap();
    let mut node_ids = vec!["sine".to_owned()];
    node_ids.extend(PRODUCER_IDS.map(str::to_owned));
    node_ids.push("sum".to_owned());
    if reverse_node_declarations {
        node_ids.reverse();
    }
    plan.nodes.sort_by_key(|node| position(&node_ids, &node.id));
    let mut connection_ids: Vec<String> = PRODUCER_IDS
        .iter()
        .map(|producer_id| format!("source_to_{producer_id}"))
        .collect();
    connection_ids.extend(
        connection_order
            .iter()
            .map(|producer_index| fan_in_ids[*producer_index].to_owned()),
    );
    plan.connections
        .sort_by_key(|connection| position(&connection_ids, &connection.id));
    plan
}

#[test]
fn sine_starts_at_zero_and_matches_binary64_phase_and_level() {
    let plan = FrameProject::sine(4, 0, 4, GATED)
        .note("n", 0, 4, "12000Hz", "")
        .plan();
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
fn renderer_sums_fan_in_by_connection_id_across_declaration_orders() {
    const CONNECTION_IDS: [&str; 3] = ["fanin_a_large", "fanin_c_small", "fanin_b_negative"];
    const DECLARATION_ORDERS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let bits = |frames: &[Vec<f64>]| {
        frames
            .iter()
            .map(|frame| frame.iter().map(|sample| sample.to_bits()).collect())
            .collect::<Vec<Vec<u64>>>()
    };

    for channels in [1, 2] {
        let expected = match channels {
            1 => vec![vec![0.0], vec![1.0]],
            2 => vec![vec![0.0, 0.0], vec![1.0, -1.0]],
            _ => unreachable!(),
        };
        let expected_bits = bits(&expected);
        for order in DECLARATION_ORDERS {
            for reverse_nodes in [false, true] {
                let plan = cancellation_plan(channels, order, CONNECTION_IDS, reverse_nodes);
                plan.validate().unwrap();
                let output = collect(&plan).unwrap();
                assert_eq!(output.len(), 2, "channels={channels}, order={order:?}");
                assert_eq!(
                    bits(&output),
                    expected_bits,
                    "channels={channels}, order={order:?}, reverse_nodes={reverse_nodes}"
                );

                if channels == 2 && order == DECLARATION_ORDERS[0] && !reverse_nodes {
                    let retained = Plan::from_json(&plan.to_json().unwrap()).unwrap();
                    assert_eq!(bits(&collect(&retained).unwrap()), expected_bits);
                }
            }
        }

        let reassigned_ids = ["fanin_a_large", "fanin_b_small", "fanin_c_negative"];
        let control = cancellation_plan(channels, [2, 0, 1], reassigned_ids, true);
        control.validate().unwrap();
        let control_output = collect(&control).unwrap();
        let zero_output = vec![vec![0.0; channels as usize]; 2];
        assert_eq!(bits(&control_output), bits(&zero_output));
        assert_ne!(bits(&control_output), expected_bits);
    }
}

#[test]
fn seconds_automation_knots_take_effect_at_the_first_sample_on_or_after_them() {
    let frames = 3;
    let plan = FrameProject::sine(frames, 0, 4, GATED)
        .note("n", 0, frames, "12000Hz", "")
        .through_pan("-1")
        .with(&automation(
            "pan_step",
            "pan.params.pan",
            "seconds",
            "0s",
            "(0s, -1, step), (1/48000s, 1, step)",
        ))
        .plan();
    let output = collect(&plan).unwrap();
    println!("pan {output:?}");
    assert!(output[0].iter().all(|sample| sample.abs() < 1e-15));
    assert!(output[1][0].abs() < 1e-14);
    assert!((output[1][1] - 1.0).abs() < 1e-14);
}

#[test]
fn attack_and_release_parameters_are_captured_at_their_events() {
    let attack_plan = FrameProject::sine(3, 0, 4, "attack = 1/48000s; release = 0s; level = 1;")
        .note("n", 0, 3, "12000Hz", "")
        .with(&automation(
            "attack_later",
            "sine.params.attack",
            "seconds",
            "0s",
            "(0s, 1/48000s, step), (1/48000s, 1s, step)",
        ))
        .plan();
    let attack_output = collect(&attack_plan).unwrap();
    println!("attack {attack_output:?}");
    assert!((attack_output[1][0] - 1.0).abs() < 1e-14);

    let release_plan = FrameProject::sine(2, 2, 4, "attack = 0s; release = 1/48000s; level = 1;")
        .note("n", 0, 2, "6000Hz", "")
        .with(&automation(
            "release_later",
            "sine.params.release",
            "seconds",
            "0s",
            "(0s, 1/48000s, step), (3/48000s, 1s, step)",
        ))
        .plan();
    let release_output = collect(&release_plan).unwrap();
    assert!((release_output[2][0] - 1.0).abs() < 1e-14);
    assert!(release_output[3][0].abs() < 1e-14);
}

#[test]
fn release_parameter_is_captured_at_note_off() {
    let plan = FrameProject::sine(4, 0, 1, GATED)
        .note("n", 0, 1, "6000Hz", "")
        .with(&automation(
            "release_at_off",
            "sine.params.release",
            "seconds",
            "0s",
            "(0s, 0s, step), (1/48000s, 2/48000s, step)",
        ))
        .plan();
    let output = collect(&plan).unwrap();
    assert!((output[2][0] - 0.5).abs() < 1e-14);
    assert!(output[3][0].abs() < 1e-14);
}

#[test]
fn released_voice_reaching_zero_is_reclaimed_before_same_frame_note_on() {
    let plan = FrameProject::sine(6, 0, 1, "attack = 0s; release = 3/48000s; level = 1;")
        .note("a", 0, 1, "440Hz", "")
        .note("b", 4, 5, "660Hz", "")
        .plan();
    assert_eq!(collect(&plan).unwrap().len(), 6);
}

#[test]
fn pan_raw_parameter_is_clamped_by_the_reference_processor() {
    let plan = FrameProject::sine(2, 0, 1, GATED)
        .note("n", 0, 2, "12000Hz", "")
        .through_pan("2")
        .plan();
    let output = collect(&plan).unwrap();
    assert!(output[1][0].abs() < 1e-14);
    assert!((output[1][1] - 1.0).abs() < 1e-14);
}

#[test]
fn automation_uses_authored_parameter_as_its_pre_anchor_base() {
    let plan = FrameProject::sine(3, 0, 1, GATED)
        .note("n", 0, 3, "12000Hz", "")
        .through_pan("-1/2")
        .with(&automation(
            "pan_after_start",
            "pan.params.pan",
            "seconds",
            "2/48000s",
            "(0s, 1, step)",
        ))
        .plan();
    let output = collect(&plan).unwrap();
    let expected = pan_sample(1.0, -0.5).unwrap();
    assert!((output[1][0] - expected[0]).abs() < 1e-14);
    assert!((output[1][1] - expected[1]).abs() < 1e-14);
}

#[test]
fn seconds_automation_anchor_is_absolute_but_frames_are_reset_relative() {
    // The score starts at 1q, which is 1/2s; the automation is one frame
    // after that.
    let plan = FrameProject::sine(3, 0, 1, GATED)
        .starting_at("1")
        .note("n", 0, 3, "12000Hz", "")
        .through_pan("-1")
        .with(&automation(
            "absolute_seconds",
            "pan.params.pan",
            "seconds",
            "24001/48000s",
            "(0s, 1, step)",
        ))
        .plan();
    let output = collect(&plan).unwrap();
    assert!(output[1][0].abs() < 1e-14);
    assert!((output[1][1] - 1.0).abs() < 1e-14);
}

#[test]
fn huge_absolute_score_origin_keeps_local_timing_exact() {
    // The score starts at 10^20 q, which is 5 * 10^19 s; the automation is
    // one frame after that.
    let plan = FrameProject::sine(96_000, 0, 1, GATED)
        .starting_at("100000000000000000000")
        .note("n", 0, 96_000, "12000Hz", "")
        .through_pan("-1")
        .with(&automation(
            "local_seconds",
            "pan.params.pan",
            "seconds",
            "2400000000000000000000001/48000s",
            "(0s, 1, step)",
        ))
        .plan();
    let output = collect(&plan).unwrap();
    assert!(output[1][0].abs() < 1e-14);
    assert!((output[1][1] - 1.0).abs() < 1e-14);
}

#[test]
fn score_and_seconds_automation_freeze_at_the_exact_score_end_value() {
    for (clock, at, points) in [
        ("score", "0q", "(0q, 0, linear), (4/24000q, 1, step)"),
        ("seconds", "0s", "(0s, 0, linear), (4/48000s, 1, step)"),
    ] {
        let plan = FrameProject::sine(2, 2, 1, "attack = 0s; release = 3/48000s; level = 1;")
            .note("n", 0, 2, "3000Hz", "")
            .through_pan("0")
            .with(&automation(
                &format!("freeze_{clock}"),
                "pan.params.pan",
                clock,
                at,
                points,
            ))
            .plan();
        let output = collect(&plan).unwrap();
        let first_tail_ratio = output[2][1] / output[2][0];
        let second_tail_ratio = output[3][1] / output[3][0];
        assert!((first_tail_ratio - second_tail_ratio).abs() < 1e-12);
    }
}

#[test]
fn release_tails_count_toward_voice_capacity_and_overflow_is_explicit() {
    let plan = FrameProject::sine(4, 0, 1, "attack = 0s; release = 3/48000s; level = 1;")
        .note("a", 0, 1, "440Hz", "")
        .note("b", 2, 3, "660Hz", "")
        .plan();
    let error = collect(&plan).unwrap_err();
    assert!(matches!(error, RenderError::VoiceLimit { .. }));
    assert_eq!(error.code(), "E_VOICE_LIMIT");
}

#[test]
fn renderer_validates_the_plan_before_producing_frames() {
    let mut plan = FrameProject::sine(2, 0, 1, GATED)
        .note("n", 0, 2, "440Hz", "")
        .plan();
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
